// HTTP-клиент лаунчера: только HTTPS, проверка адресов перед запросом
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::operations::CancellationToken;

pub const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

const MAX_REDIRECTS: usize = 5;
// сегменты включаются для файлов заведомо известного размера: client.jar,
// рантаймы Java, крупные серверные ядра
const PARALLEL_DOWNLOAD_MIN_BYTES: u64 = 8 * 1024 * 1024;
const PARALLEL_DOWNLOAD_SEGMENT_BYTES: u64 = 4 * 1024 * 1024;
const PARALLEL_DOWNLOAD_MAX_SEGMENTS: u64 = 6;

fn rate_limit_intervals() -> &'static HashMap<&'static str, f64> {
    static TABLE: OnceLock<HashMap<&'static str, f64>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut map = HashMap::new();
        map.insert("api.modrinth.com", 0.35);
        map.insert("cdn.modrinth.com", 0.15);
        map.insert("api.curseforge.com", 0.35);
        map.insert("api.cfwidget.com", 0.25);
        map.insert("edge.forgecdn.net", 0.1);
        map.insert("mediafilez.forgecdn.net", 0.1);
        map
    })
}

const MAX_RATE_LIMIT_WAIT_SECONDS: f64 = 15.0;

fn rate_limits() -> &'static Mutex<HashMap<String, Instant>> {
    static LIMITS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    LIMITS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub type Progress = Box<dyn Fn(u64, Option<u64>) + Send + Sync>;
// наблюдатели — Arc, не Box: клон клиента (например, в параллельных
// загрузчиках) обязан сохранять мост прогресса в UI, а не терять его.
pub type DownloadObserver = std::sync::Arc<dyn Fn(&str, &str, &Path, u64, Option<u64>) + Send + Sync>;
pub type RequestObserver = std::sync::Arc<dyn Fn(&str, &str, u32) + Send + Sync>;
pub type RedirectValidator<'a> = &'a dyn Fn(&str) -> Result<(), String>;

pub struct DetailedResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub url: String,
    pub body: Vec<u8>,
}

impl DetailedResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadSource {
    Auto,
    Official,
    Mirror,
}

impl DownloadSource {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "official" => Self::Official,
            "mirror" | "bmclapi" => Self::Mirror,
            _ => Self::Auto,
        }
    }
}

const MIRROR_BASE: &str = "https://bmclapi2.bangbang93.com";

/// (хост, префикс пути у оригинала, префикс пути у зеркала)
const MIRROR_RULES: &[(&str, &str, &str)] = &[
    ("piston-meta.mojang.com", "", ""),
    ("piston-data.mojang.com", "", ""),
    ("launchermeta.mojang.com", "", ""),
    ("launcher.mojang.com", "", ""),
    ("resources.download.minecraft.net", "", "/assets"),
    ("libraries.minecraft.net", "", "/maven"),
    ("maven.minecraftforge.net", "", "/maven"),
    ("files.minecraftforge.net", "/maven", "/maven"),
    ("maven.neoforged.net", "/releases", "/maven"),
    ("meta.fabricmc.net", "", "/fabric-meta"),
    ("maven.fabricmc.net", "", "/maven"),
    ("meta.quiltmc.org", "", "/quilt-meta"),
    ("maven.quiltmc.org", "/repository/release", "/maven"),
];

pub fn mirror_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "https" || parsed.port().is_some() {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    let path = parsed.path();
    for (rule_host, from_prefix, to_prefix) in MIRROR_RULES {
        if host != *rule_host {
            continue;
        }
        let Some(rest) = path.strip_prefix(from_prefix) else { continue };
        if !rest.is_empty() && !rest.starts_with('/') {
            continue;
        }
        let mut mirrored = format!("{MIRROR_BASE}{to_prefix}{rest}");
        if let Some(query) = parsed.query() {
            mirrored.push('?');
            mirrored.push_str(query);
        }
        return Some(mirrored);
    }
    None
}

struct Candidate {
    url: String,
    mirror: bool,
}

/// ошибка одного источника: можно ли пробовать следующий источник.
enum SourceError {
    /// отмена пользователем, нехватка места, ошибка записи на диск —
    /// другой источник не поможет.
    Fatal(String),
    /// источник не отдал файл (сеть, таймаут, 4xx/5xx, битый хеш).
    Failed { message: String, status: Option<u16> },
}

struct PreferIpv4Resolver {
    strict: bool,
}

impl ureq::Resolver for PreferIpv4Resolver {
    fn resolve(&self, netloc: &str) -> std::io::Result<Vec<std::net::SocketAddr>> {
        let mut addresses: Vec<std::net::SocketAddr> = netloc.to_socket_addrs()?.collect();
        addresses.sort_by_key(|address| if address.is_ipv4() { 0 } else { 1 });
        addresses.dedup();
        if self.strict {
            addresses.retain(|address| !is_local_address(&address.ip()));
            if addresses.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "DNS вернул только непубличные адреса",
                ));
            }
        }
        Ok(addresses)
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn platform_tls_config() -> Option<Arc<rustls::ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = rustls_platform_verifier::Verifier::new(provider.clone()).ok()?;
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .ok()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Some(Arc::new(config))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_tls_config() -> Option<Arc<rustls::ClientConfig>> {
    None
}

pub struct HttpClient {
    timeout: Duration,
    retries: u32,
    user_agent: String,
    proxy: String,
    agent: ureq::Agent,
    source: DownloadSource,
    pub cancellation: Option<Arc<CancellationToken>>,
    download_observer: Option<DownloadObserver>,
    request_observer: Option<RequestObserver>,
}

impl HttpClient {
    pub fn new(
        timeout_seconds: u32,
        retries: u32,
        proxy: &str,
        cancellation: Option<Arc<CancellationToken>>,
    ) -> Result<Self, String> {
        let proxy = proxy.trim();
        let idle = Duration::from_secs(timeout_seconds.max(1) as u64);
        // таймаут — на простой соединения (нет байтов N секунд), а не на
        let mut builder = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(idle)
            .timeout_write(idle)
            .resolver(PreferIpv4Resolver { strict: proxy.is_empty() })
            .redirects(0);
        if let Some(tls) = platform_tls_config() {
            builder = builder.tls_config(tls);
        }
        if !proxy.is_empty() {
            // socks5h = SOCKS5 с remote-DNS; ureq различия не делает, DNS
            // и так резолвит прокси.
            let normalized = proxy.replacen("socks5h://", "socks5://", 1);
            let parsed = ureq::Proxy::new(&normalized)
                .map_err(|error| format!("Некорректный прокси {proxy}: {error}"))?;
            builder = builder.proxy(parsed);
        }
        Ok(Self {
            timeout: idle,
            retries: retries.max(1),
            user_agent: format!("{}/{}", super::constants::APP_NAME, super::constants::APP_VERSION),
            proxy: proxy.to_owned(),
            agent: builder.build(),
            source: DownloadSource::Auto,
            cancellation,
            download_observer: None,
            request_observer: None,
        })
    }

    pub fn set_download_source(&mut self, source: DownloadSource) {
        self.source = source;
    }

    fn candidates(&self, url: &str, method: &str) -> Vec<Candidate> {
        let original = Candidate { url: url.to_owned(), mirror: false };
        if method != "GET" || self.source == DownloadSource::Official {
            return vec![original];
        }
        let Some(mirrored) = mirror_url(url) else {
            return vec![original];
        };
        let mirror = Candidate { url: mirrored, mirror: true };
        match self.source {
            DownloadSource::Mirror => vec![mirror, original],
            _ => vec![original, mirror],
        }
    }

    pub fn set_observers(&mut self, download: Option<DownloadObserver>, request: Option<RequestObserver>) {
        self.download_observer = download;
        self.request_observer = request;
    }

    pub fn proxy(&self) -> &str {
        &self.proxy
    }

    fn check_cancelled(&self) -> Result<(), String> {
        if let Some(token) = &self.cancellation {
            token.check()?;
        }
        Ok(())
    }

    fn wait(&self, seconds: f64) -> Result<(), String> {
        if let Some(token) = &self.cancellation {
            token.wait(seconds)
        } else {
            std::thread::sleep(Duration::from_secs_f64(seconds.max(0.0)));
            Ok(())
        }
    }

    fn notify_request(&self, event: &str, url: &str, attempt: u32) {
        if let Some(observer) = &self.request_observer {
            observer(event, url, attempt);
        }
    }

    fn notify_download(&self, event: &str, url: &str, destination: &Path, current: u64, total: Option<u64>) {
        if let Some(observer) = &self.download_observer {
            observer(event, url, destination, current, total);
        }
    }

    fn rate_limit_before(&self, url: &str) -> Result<(), String> {
        let host = url_host(url);
        let Some(interval) = rate_limit_intervals().get(host.as_str()) else {
            return Ok(());
        };
        let now = Instant::now();
        let delay = {
            let mut limits = rate_limits().lock().unwrap_or_else(|poison| poison.into_inner());
            let next_allowed = limits.get(&host).copied().unwrap_or(now);
            let delay = (next_allowed - now).as_secs_f64().clamp(0.0, MAX_RATE_LIMIT_WAIT_SECONDS);
            limits.insert(host.clone(), next_allowed.max(now) + Duration::from_secs_f64(*interval));
            delay
        };
        if delay > 0.0 {
            self.wait(delay)?;
        }
        Ok(())
    }

    fn rate_limit_after_429(&self, url: &str, retry_after: f64) {
        let host = url_host(url);
        let cooldown = Instant::now() + Duration::from_secs_f64(retry_after.max(0.0));
        let mut limits = rate_limits().lock().unwrap_or_else(|poison| poison.into_inner());
        limits.entry(host).and_modify(|at| {
            if *at < cooldown {
                *at = cooldown;
            }
        }).or_insert(cooldown);
    }

    fn open_once(
        &self,
        url: &str,
        method: &str,
        data: Option<&[u8]>,
        headers: &[(String, String)],
        redirect_validator: Option<RedirectValidator<'_>>,
    ) -> Result<DetailedResponse, String> {
        let mut current = url::Url::parse(url).map_err(|error| format!("Некорректный сетевой адрес: {error}"))?;
        let mut method = method.to_owned();
        let mut body = data.map(|bytes| bytes.to_vec());
        let mut headers: Vec<(String, String)> = headers.to_vec();
        for _ in 0..=MAX_REDIRECTS {
            validate_public_https_url(current.as_str())?;
            if let Some(validator) = redirect_validator {
                validator(current.as_str())?;
            }
            let mut request = self.agent.request(&method, current.as_str());
            for (name, value) in &headers {
                request = request.set(name, value);
            }
            let outcome = match body.as_deref() {
                Some(bytes) => request.send_bytes(bytes),
                None => request.call(),
            };
            let response = match outcome {
                Ok(response) => response,
                Err(ureq::Error::Status(_, response)) => response,
                Err(error) => return Err(format!("Сетевая ошибка при запросе {current}: {error}")),
            };
            let status = response.status();
            if (300..400).contains(&status) {
                let location = response.header("location").unwrap_or_default().to_owned();
                if location.is_empty() {
                    return finish_response(response, current, status);
                }
                let target = current
                    .join(&location)
                    .map_err(|_| format!("Некорректный редирект: {location}"))?;
                let origin_changed = target.scheme() != current.scheme()
                    || target.host_str() != current.host_str()
                    || target.port_or_known_default() != current.port_or_known_default();
                if origin_changed {
                    headers.retain(|(name, _)| {
                        !matches!(name.to_ascii_lowercase().as_str(), "authorization" | "cookie" | "proxy-authorization")
                    });
                }
                // 303 и классика 301/302 для POST переводятся в GET — как в браузерах.
                if status == 303 || ((status == 301 || status == 302) && method == "POST") {
                    method = "GET".to_owned();
                    body = None;
                }
                current = target;
                continue;
            }
            return finish_response(response, current, status);
        }
        Err(format!("Слишком много редиректов при запросе {url}"))
    }

    fn request_inner(
        &self,
        url: &str,
        method: &str,
        data: Option<&[u8]>,
        headers: Option<&[(String, String)]>,
        max_bytes: Option<u64>,
        redirect_validator: Option<RedirectValidator<'_>>,
    ) -> Result<Vec<u8>, String> {
        let candidates = self.candidates(url, method);
        let mut failures: Vec<String> = Vec::new();
        for candidate in &candidates {
            if candidate.mirror {
                self.notify_request("mirror", &candidate.url, 1);
            }
            // белый список редиректов (например, только maven Forge) относится
            let validator = if candidate.mirror { None } else { redirect_validator };
            match self.request_from(&candidate.url, method, data, headers, max_bytes, validator) {
                Ok(body) => return Ok(body),
                Err(SourceError::Fatal(message)) => return Err(message),
                Err(SourceError::Failed { message, status }) => {
                    self.check_cancelled()?;
                    // 404/410 от официального сервера — это ответ, а не сбой:
                    // зеркало такого файла тоже не найдёт.
                    let definitive = !candidate.mirror && matches!(status, Some(404) | Some(410));
                    if candidates.len() == 1 || definitive {
                        return Err(message);
                    }
                    failures.push(source_failure(&candidate.url, candidate.mirror, &message));
                }
            }
        }
        Err(format!("Не удалось получить {url} ни с одного источника: {}", failures.join("; ")))
    }

    fn request_from(
        &self,
        url: &str,
        method: &str,
        data: Option<&[u8]>,
        headers: Option<&[(String, String)]>,
        max_bytes: Option<u64>,
        redirect_validator: Option<RedirectValidator<'_>>,
    ) -> Result<Vec<u8>, SourceError> {
        let mut headers_vec: Vec<(String, String)> = vec![
            ("User-Agent".into(), self.user_agent.clone()),
            ("Accept".into(), "application/json".into()),
        ];
        headers_vec.extend(headers.unwrap_or(&[]).to_vec());
        let limit = max_bytes.unwrap_or(MAX_RESPONSE_BYTES);
        let fatal = SourceError::Fatal;
        let mut last_error = String::new();
        for attempt in 1..=self.retries {
            self.check_cancelled().map_err(fatal)?;
            self.rate_limit_before(url).map_err(fatal)?;
            self.notify_request("start", url, attempt);
            match self.open_once(url, method, data, &headers_vec, redirect_validator) {
                Ok(response) => {
                    if let Some(declared) = declared_length(&response) {
                        if declared > limit {
                            return Err(fatal("Ответ сервиса слишком большой".to_owned()));
                        }
                    }
                    if response.body.len() as u64 > limit {
                        return Err(fatal("Ответ сервиса слишком большой".to_owned()));
                    }
                    if response.status == 429 {
                        let retry_after = response
                            .header("retry-after")
                            .and_then(|value| value.trim().parse::<f64>().ok())
                            .map(|value| value.clamp(1.0, 60.0))
                            .unwrap_or(2.0 + attempt as f64);
                        if attempt < self.retries {
                            self.rate_limit_after_429(url, retry_after);
                            self.notify_request("retry", url, attempt);
                            self.wait(retry_after).map_err(fatal)?;
                            continue;
                        }
                        return Err(SourceError::Failed {
                            message: format!("HTTP 429: сервер временно ограничил запросы к {url}. Повторите позже."),
                            status: Some(429),
                        });
                    }
                    if response.status >= 500 && attempt < self.retries {
                        self.notify_request("retry", url, attempt);
                        self.wait(1.0 + attempt as f64).map_err(fatal)?;
                        continue;
                    }
                    if response.status >= 400 {
                        return Err(SourceError::Failed {
                            message: format!("HTTP {} при запросе {}", response.status, response.url),
                            status: Some(response.status),
                        });
                    }
                    return Ok(response.body);
                }
                Err(error) => {
                    last_error = error;
                    if attempt < self.retries {
                        self.notify_request("retry", url, attempt);
                        self.wait(1.0 + attempt as f64).map_err(fatal)?;
                        continue;
                    }
                }
            }
        }
        Err(SourceError::Failed {
            message: format!("Сетевая ошибка при запросе {url}: {last_error}"),
            status: None,
        })
    }

    pub fn request(
        &self,
        url: &str,
        method: &str,
        data: Option<&[u8]>,
        headers: Option<&[(String, String)]>,
        max_bytes: Option<u64>,
        redirect_validator: Option<RedirectValidator<'_>>,
    ) -> Result<Vec<u8>, String> {
        self.request_inner(url, method, data, headers, max_bytes, redirect_validator)
    }

    pub fn request_detailed(
        &self,
        url: &str,
        method: &str,
        data: Option<&[u8]>,
        headers: Option<&[(String, String)]>,
        max_bytes: Option<u64>,
        redirect_validator: Option<RedirectValidator<'_>>,
    ) -> Result<DetailedResponse, String> {
        let mut headers_vec: Vec<(String, String)> = vec![
            ("User-Agent".into(), self.user_agent.clone()),
            ("Accept".into(), "*/*".into()),
        ];
        headers_vec.extend(headers.unwrap_or(&[]).to_vec());
        let limit = max_bytes.unwrap_or(MAX_RESPONSE_BYTES);
        for attempt in 1..=self.retries {
            self.check_cancelled()?;
            self.rate_limit_before(url)?;
            self.notify_request("start", url, attempt);
            match self.open_once(url, method, data, &headers_vec, redirect_validator) {
                Ok(mut response) => {
                    if let Some(declared) = declared_length(&response) {
                        if declared > limit {
                            return Err("Ответ сервиса слишком большой".to_owned());
                        }
                    }
                    response.body.truncate(limit as usize + 1);
                    if response.body.len() as u64 > limit {
                        return Err("Ответ сервиса слишком большой".to_owned());
                    }
                    return Ok(response);
                }
                Err(error) => {
                    if attempt < self.retries {
                        self.notify_request("retry", url, attempt);
                        self.wait(1.0 + attempt as f64)?;
                        continue;
                    }
                    return Err(format!("Сетевая ошибка при запросе {url}: {error}"));
                }
            }
        }
        unreachable!()
    }

    pub fn get_json(&self, url: &str, headers: Option<&[(String, String)]>) -> Result<Value, String> {
        let raw = self.request(url, "GET", None, headers, None, None)?;
        serde_json::from_slice(&raw).map_err(|_| format!("Сервер вернул некорректный JSON: {url}"))
    }

    pub fn post_form(&self, url: &str, values: &[(String, String)]) -> Result<Value, String> {
        let body = form_urlencoded(values);
        let headers = vec![
            ("Content-Type".to_owned(), "application/x-www-form-urlencoded".to_owned()),
            ("Accept".to_owned(), "application/json".to_owned()),
        ];
        let raw = self.request(url, "POST", Some(body.as_bytes()), Some(&headers), None, None)?;
        serde_json::from_slice(&raw).map_err(|_| format!("Сервер авторизации вернул некорректный ответ: {url}"))
    }

    pub fn post_form_soft(&self, url: &str, values: &[(String, String)]) -> Result<(u16, Value), String> {
        let body = form_urlencoded(values);
        let headers = vec![
            ("User-Agent".to_owned(), self.user_agent.clone()),
            ("Content-Type".to_owned(), "application/x-www-form-urlencoded".to_owned()),
            ("Accept".to_owned(), "application/json".to_owned()),
        ];
        let mut last_error = String::new();
        for attempt in 1..=self.retries {
            self.check_cancelled()?;
            self.rate_limit_before(url)?;
            self.notify_request("start", url, attempt);
            match self.open_once(url, "POST", Some(body.as_bytes()), &headers, None) {
                Ok(mut response) => {
                    response.body.truncate(MAX_RESPONSE_BYTES as usize);
                    let parsed = serde_json::from_slice(&response.body)
                        .map_err(|_| format!("Сервер авторизации вернул некорректный ответ: {url}"))?;
                    return Ok((response.status, parsed));
                }
                Err(error) => {
                    last_error = error;
                    if attempt < self.retries {
                        self.wait(1.0 + attempt as f64)?;
                        continue;
                    }
                }
            }
        }
        Err(format!("Сетевая ошибка OAuth: {last_error}"))
    }

    pub fn post_json(
        &self,
        url: &str,
        value: &Value,
        headers: Option<&[(String, String)]>,
    ) -> Result<Value, String> {
        let body = serde_json::to_vec(value).unwrap_or_default();
        let mut send_headers = vec![
            ("Content-Type".to_owned(), "application/json".to_owned()),
            ("Accept".to_owned(), "application/json".to_owned()),
        ];
        send_headers.extend(headers.unwrap_or(&[]).to_vec());
        let raw = self.request(url, "POST", Some(&body), Some(&send_headers), None, None)?;
        serde_json::from_slice(&raw).map_err(|_| format!("Сервер вернул некорректный JSON: {url}"))
    }

    pub fn post_json_soft(&self, url: &str, value: &Value) -> Result<(u16, Value), String> {
        let body = serde_json::to_vec(value).unwrap_or_default();
        let headers = vec![
            ("User-Agent".to_owned(), self.user_agent.clone()),
            ("Content-Type".to_owned(), "application/json".to_owned()),
            ("Accept".to_owned(), "application/json".to_owned()),
        ];
        let mut last_error = String::new();
        for attempt in 1..=self.retries {
            self.check_cancelled()?;
            self.rate_limit_before(url)?;
            self.notify_request("start", url, attempt);
            match self.open_once(url, "POST", Some(&body), &headers, None) {
                Ok(mut response) => {
                    response.body.truncate(MAX_RESPONSE_BYTES as usize);
                    let parsed = serde_json::from_slice(&response.body)
                        .map_err(|_| format!("Сервер авторизации вернул некорректный ответ: {url}"))?;
                    return Ok((response.status, parsed));
                }
                Err(error) => {
                    last_error = error;
                    if attempt < self.retries {
                        self.wait(1.0 + attempt as f64)?;
                        continue;
                    }
                }
            }
        }
        Err(format!("Сетевая ошибка авторизации: {last_error}"))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn download(
        &self,
        url: &str,
        destination: &Path,
        sha1: Option<&str>,
        sha256: Option<&str>,
        sha512: Option<&str>,
        size: Option<u64>,
        progress: Option<&Progress>,
        headers: Option<&[(String, String)]>,
        redirect_validator: Option<RedirectValidator<'_>>,
    ) -> Result<bool, String> {
        url::Url::parse(url).map_err(|_| format!("Некорректный сетевой адрес: {url}"))?;
        self.check_cancelled()?;
        if let Some(size) = size {
            if size > MAX_DOWNLOAD_BYTES {
                return Err("Файл загрузки слишком большой".to_owned());
            }
        }
        reject_symlink_path(destination)?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("Не удалось создать каталог: {error}"))?;
        }
        self.notify_download("start", url, destination, 0, size);
        if destination.exists() {
            let mut trusted = (sha1.is_some() || sha256.is_some() || sha512.is_some() || size.is_some())
                && verify_file(destination, sha1, sha256, sha512, size);
            if trusted {
                if let (Some(expected), Some(actual)) = (sha256, file_hash(destination, "sha256").ok()) {
                    trusted = actual.to_lowercase() == expected.to_lowercase();
                }
            }
            if trusted {
                let actual_size = std::fs::metadata(destination).map(|meta| meta.len()).unwrap_or(0);
                if let Some(progress) = progress {
                    progress(actual_size, Some(actual_size));
                }
                self.notify_download("cached", url, destination, actual_size, size.or(Some(actual_size)));
                return Ok(false);
            }
        }

        let temporary = destination.with_file_name(format!(".{}.part", file_name_of(destination)));
        reject_symlink_path(&temporary)?;
        // большой файл с известным размером и хешем качаем сегментами;
        // при любой неудаче молча уходим в обычный одиночный режим
        if let Some(known_size) = size.filter(|value| *value >= PARALLEL_DOWNLOAD_MIN_BYTES) {
            let sha1_digest = sha1.filter(|value| !value.is_empty());
            if sha1_digest.is_some() {
                let attempt = self.download_segmented(url, destination, &temporary, known_size, sha1_digest, progress);
                match attempt {
                    Ok(()) => {
                        self.notify_download("complete", url, destination, known_size, Some(known_size));
                        return Ok(true);
                    }
                    Err(error) if error.contains("остановлена") => return Err(error),
                    Err(_) => {
                        let _ = std::fs::remove_file(&temporary);
                    }
                }
            }
        }
        let candidates = self.candidates(url, "GET");
        let mut failures: Vec<String> = Vec::new();
        for candidate in &candidates {
            self.check_cancelled()?;
            if candidate.mirror {
                self.notify_request("mirror", &candidate.url, 1);
            }
            let validator = if candidate.mirror { None } else { redirect_validator };
            let outcome = self.download_from(
                &candidate.url,
                destination,
                &temporary,
                sha1,
                sha256,
                sha512,
                size,
                progress,
                headers,
                validator,
            );
            match outcome {
                Ok(()) => return Ok(true),
                Err(SourceError::Fatal(message)) => {
                    let _ = std::fs::remove_file(&temporary);
                    return Err(message);
                }
                Err(SourceError::Failed { message, status }) => {
                    let _ = std::fs::remove_file(&temporary);
                    self.check_cancelled()?;
                    let definitive = !candidate.mirror && matches!(status, Some(404) | Some(410));
                    if candidates.len() == 1 || definitive {
                        return Err(format!("Не удалось скачать {url}: {message}"));
                    }
                    failures.push(source_failure(&candidate.url, candidate.mirror, &message));
                }
            }
        }
        Err(format!("Не удалось скачать {url} ни с одного источника: {}", failures.join("; ")))
    }

    #[allow(clippy::too_many_arguments)]
    fn download_from(
        &self,
        url: &str,
        destination: &Path,
        temporary: &Path,
        sha1: Option<&str>,
        sha256: Option<&str>,
        sha512: Option<&str>,
        size: Option<u64>,
        progress: Option<&Progress>,
        headers: Option<&[(String, String)]>,
        redirect_validator: Option<RedirectValidator<'_>>,
    ) -> Result<(), SourceError> {
        let fatal = SourceError::Fatal;
        let _ = std::fs::remove_file(temporary);
        let mut offset: u64 = 0;
        let mut integrity_restarts = 0u32;
        let mut last_error = String::new();
        let mut last_status: Option<u16> = None;
        let mut attempt: u32 = 0;
        while attempt < self.retries {
            attempt += 1;
            self.check_cancelled().map_err(fatal)?;
            self.rate_limit_before(url).map_err(fatal)?;
            self.notify_request("download", url, attempt);
            let mut headers_vec: Vec<(String, String)> = vec![
                ("User-Agent".into(), self.user_agent.clone()),
                // без gzip: иначе счётчик байтов не совпадает с Range-смещением.
                ("Accept-Encoding".into(), "identity".into()),
            ];
            headers_vec.extend(headers.unwrap_or(&[]).to_vec());
            if offset > 0 {
                headers_vec.push(("Range".into(), format!("bytes={offset}-")));
            }
            let mut stream = match self.open_stream(url, &headers_vec, redirect_validator) {
                Ok(stream) => stream,
                Err((status, message)) => {
                    last_error = message;
                    last_status = status;
                    match status {
                        Some(416) if offset > 0 => {
                            // сервер не принял докачку — начинаем файл заново,
                            // попытку не засчитываем.
                            let _ = std::fs::remove_file(temporary);
                            offset = 0;
                            attempt -= 1;
                            continue;
                        }
                        Some(429) => {
                            let delay = 2.0 + attempt as f64;
                            self.rate_limit_after_429(url, delay);
                            if attempt < self.retries {
                                self.notify_request("retry", url, attempt);
                                self.wait(delay).map_err(fatal)?;
                            }
                            continue;
                        }
                        // остальные 4xx повторять бессмысленно — сразу к
                        // следующему источнику.
                        Some(code) if (400..500).contains(&code) && code != 408 => {
                            return Err(SourceError::Failed { message: last_error, status });
                        }
                        _ => {
                            if attempt < self.retries {
                                self.notify_request("retry", url, attempt);
                                self.wait(1.0 + attempt as f64).map_err(fatal)?;
                            }
                            continue;
                        }
                    }
                }
            };
            let resumed = offset > 0 && stream.status == 206;
            if offset > 0 && !resumed {
                offset = 0;
            }
            let total_size = if resumed {
                stream.total_size.map(|remaining| remaining + offset).or(size)
            } else {
                stream.total_size.or(size)
            };
            if let Some(total) = total_size {
                if total > MAX_DOWNLOAD_BYTES {
                    return Err(fatal("Файл загрузки слишком большой".to_owned()));
                }
            }
            let mut handle = if resumed {
                OpenOptions::new().append(true).open(temporary)
            } else {
                File::create(temporary)
            }
            .map_err(|error| fatal(format!("Не удалось создать временный файл {}: {error}", temporary.display())))?;
            let mut written = offset;
            let mut buffer = vec![0u8; 256 * 1024];
            let mut interrupted: Option<String> = None;
            loop {
                self.check_cancelled().map_err(fatal)?;
                let count = match stream.reader.read(&mut buffer) {
                    Ok(count) => count,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        interrupted = Some(format!("Обрыв загрузки: {error}"));
                        break;
                    }
                };
                if count == 0 {
                    break;
                }
                written += count as u64;
                if written > MAX_DOWNLOAD_BYTES {
                    return Err(fatal("Файл загрузки слишком большой".to_owned()));
                }
                handle
                    .write_all(&buffer[..count])
                    .map_err(|error| fatal(format!("Не удалось записать файл {}: {error}", temporary.display())))?;
                if let Some(progress) = progress {
                    progress(written, total_size);
                }
                self.notify_download("progress", url, destination, written, total_size);
            }
            handle
                .flush()
                .map_err(|error| fatal(format!("Не удалось записать файл {}: {error}", temporary.display())))?;
            drop(handle);
            let expected = size.or(total_size);
            if interrupted.is_none() {
                if let Some(expected) = expected {
                    if written < expected {
                        interrupted = Some(format!("Соединение закрыто на {written} из {expected} байт"));
                    }
                }
            }
            if let Some(message) = interrupted {
                last_error = message;
                last_status = None;
                // сохранённая часть докачивается следующей попыткой.
                offset = std::fs::metadata(temporary).map(|meta| meta.len()).unwrap_or(0);
                if offset > MAX_DOWNLOAD_BYTES {
                    offset = 0;
                    let _ = std::fs::remove_file(temporary);
                }
                if attempt < self.retries {
                    self.notify_request("retry", url, attempt);
                    self.wait(1.0 + attempt as f64).map_err(fatal)?;
                }
                continue;
            }
            if !verify_file(temporary, sha1, sha256, sha512, size) {
                let _ = std::fs::remove_file(temporary);
                offset = 0;
                last_error = format!("Проверка хеша не пройдена: {url}");
                last_status = None;
                // битая докачка заслуживает одну чистую попытку с нуля.
                if resumed && integrity_restarts == 0 {
                    integrity_restarts += 1;
                    continue;
                }
                return Err(SourceError::Failed { message: last_error, status: None });
            }
            replace_file(temporary, destination)
                .map_err(|error| fatal(format!("Не удалось сохранить файл {}: {error}", destination.display())))?;
            self.notify_download("complete", url, destination, written, total_size);
            return Ok(());
        }
        Err(SourceError::Failed { message: last_error, status: last_status })
    }

    fn download_segmented(
        &self,
        url: &str,
        destination: &Path,
        temporary: &Path,
        size: u64,
        sha1: Option<&str>,
        progress: Option<&Progress>,
    ) -> Result<(), String> {
        let segments = (size / PARALLEL_DOWNLOAD_SEGMENT_BYTES).clamp(2, PARALLEL_DOWNLOAD_MAX_SEGMENTS) as usize;
        let base = size / segments as u64;
        let mut ranges: Vec<(u64, u64)> = (0..segments).map(|index| (index as u64 * base, 0)).collect();
        for (index, range) in ranges.iter_mut().enumerate() {
            range.1 = if index + 1 == segments { size - 1 } else { (index as u64 + 1) * base - 1 };
        }
        // пробный запрос первого сегмента: без 206 от сервера сегменты не работают
        let first_headers: Vec<(String, String)> = vec![
            ("User-Agent".into(), self.user_agent.clone()),
            ("Accept-Encoding".into(), "identity".into()),
            ("Range".into(), format!("bytes={}-{}", ranges[0].0, ranges[0].1)),
        ];
        let first = self
            .open_stream(url, &first_headers, None)
            .map_err(|(_, message)| message)?;
        if first.status != 206 {
            return Err("сервер не поддерживает докачку диапазонами".to_owned());
        }
        let written_total = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let failed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let first_error: std::sync::Arc<std::sync::Mutex<Option<String>>> = std::sync::Arc::new(std::sync::Mutex::new(None));
        let run_segment = |index: usize, mut stream: StreamResponse| -> Result<(), String> {
            let segment_path = temporary.with_file_name(format!("{}.seg{index}", file_name_of(temporary)));
            let mut handle = File::create(&segment_path)
                .map_err(|error| format!("Не удалось создать временный файл {}: {error}", segment_path.display()))?;
            let expected = ranges[index].1 - ranges[index].0 + 1;
            let mut buffer = vec![0u8; 128 * 1024];
            let mut segment_written = 0u64;
            loop {
                self.check_cancelled()?;
                if failed.load(std::sync::atomic::Ordering::Relaxed) {
                    return Ok(());
                }
                let count = match stream.reader.read(&mut buffer) {
                    Ok(count) => count,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(format!("Обрыв загрузки сегмента: {error}")),
                };
                if count == 0 {
                    break;
                }
                handle
                    .write_all(&buffer[..count])
                    .map_err(|error| format!("Не удалось записать файл {}: {error}", segment_path.display()))?;
                segment_written += count as u64;
                let total = written_total.fetch_add(count as u64, std::sync::atomic::Ordering::Relaxed) + count as u64;
                if let Some(progress) = progress {
                    progress(total, Some(size));
                }
                self.notify_download("progress", url, destination, total, Some(size));
            }
            handle.flush().map_err(|error| format!("Не удалось записать файл {}: {error}", segment_path.display()))?;
            if segment_written != expected {
                return Err(format!("Сегмент {index} короткий: {segment_written} из {expected} байт"));
            }
            Ok(())
        };
        let result = std::thread::scope(|scope| -> Result<(), String> {
            let mut handles = Vec::new();
            for (index, (start, end)) in ranges.iter().enumerate().skip(1) {
                let url = url.to_owned();
                let headers: Vec<(String, String)> = vec![
                    ("User-Agent".into(), self.user_agent.clone()),
                    ("Accept-Encoding".into(), "identity".into()),
                    ("Range".into(), format!("bytes={start}-{end}")),
                ];
                let failed = failed.clone();
                let first_error = first_error.clone();
                handles.push(scope.spawn(move || {
                    if failed.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    if let Err(error) = self.rate_limit_before(&url) {
                        failed.store(true, std::sync::atomic::Ordering::Relaxed);
                        *first_error.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(error);
                        return;
                    }
                    let attempt = self.open_stream(&url, &headers, None).and_then(|stream| {
                        if stream.status != 206 {
                            Err((None, "сервер перестал отдавать диапазоны".to_owned()))
                        } else {
                            Ok(stream)
                        }
                    });
                    match attempt {
                        Ok(stream) => {
                            if let Err(error) = run_segment(index, stream) {
                                failed.store(true, std::sync::atomic::Ordering::Relaxed);
                                *first_error.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(error);
                            }
                        }
                        Err((_, error)) => {
                            failed.store(true, std::sync::atomic::Ordering::Relaxed);
                            *first_error.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(error);
                        }
                    }
                }));
            }
            let first_result = run_segment(0, first);
            for handle in handles {
                let _ = handle.join();
            }
            first_result
        });
        // сборка сегментов в итоговый файл — только когда всё скачалось
        match result {
            Ok(()) if !failed.load(std::sync::atomic::Ordering::Relaxed) => {
                let mut assembled = File::create(temporary)
                    .map_err(|error| format!("Не удалось создать временный файл {}: {error}", temporary.display()))?;
                for index in 0..segments {
                    let segment_path = temporary.with_file_name(format!("{}.seg{index}", file_name_of(temporary)));
                    let mut bytes = Vec::new();
                    File::open(&segment_path)
                        .and_then(|mut file| file.read_to_end(&mut bytes))
                        .map_err(|error| format!("Не удалось прочитать сегмент {}: {error}", segment_path.display()))?;
                    assembled
                        .write_all(&bytes)
                        .map_err(|error| format!("Не удалось записать файл {}: {error}", temporary.display()))?;
                    let _ = std::fs::remove_file(&segment_path);
                }
                assembled.flush().map_err(|error| format!("Не удалось записать файл {}: {error}", temporary.display()))?;
                drop(assembled);
                if !verify_file(temporary, sha1, None, None, Some(size)) {
                    let _ = std::fs::remove_file(temporary);
                    return Err("Проверка хеша не пройдена после сборки сегментов".to_owned());
                }
                replace_file(temporary, destination)
                    .map_err(|error| format!("Не удалось сохранить файл {}: {error}", destination.display()))?;
                Ok(())
            }
            otherwise => {
                for index in 0..segments {
                    let _ = std::fs::remove_file(temporary.with_file_name(format!("{}.seg{index}", file_name_of(temporary))));
                }
                let _ = std::fs::remove_file(temporary);
                if let Some(error) = first_error.lock().unwrap_or_else(|poison| poison.into_inner()).take() {
                    return Err(error);
                }
                otherwise.map(|_| ())
            }
        }
    }

    /// ответ в виде потока — для download(); редиректы обрабатывает open_once_raw.
    fn open_stream(
        &self,
        url: &str,
        headers: &[(String, String)],
        redirect_validator: Option<RedirectValidator<'_>>,
    ) -> Result<StreamResponse, (Option<u16>, String)> {
        let outcome = self
            .open_once_raw(url, "GET", None, headers, redirect_validator)
            .map_err(|error| (None, error))?;
        if outcome.status >= 400 {
            return Err((Some(outcome.status), format!("HTTP {} при загрузке {}", outcome.status, url)));
        }
        Ok(outcome)
    }
    fn open_once_raw(
        &self,
        url: &str,
        method: &str,
        data: Option<&[u8]>,
        headers: &[(String, String)],
        redirect_validator: Option<RedirectValidator<'_>>,
    ) -> Result<StreamResponse, String> {
        let mut current = url::Url::parse(url).map_err(|error| format!("Некорректный сетевой адрес: {error}"))?;
        let mut method = method.to_owned();
        let mut body = data.map(|bytes| bytes.to_vec());
        let mut headers = headers.to_vec();
        for _ in 0..=MAX_REDIRECTS {
            validate_public_https_url(current.as_str())?;
            if let Some(validator) = redirect_validator {
                validator(current.as_str())?;
            }
            let mut request = self.agent.request(&method, current.as_str());
            for (name, value) in &headers {
                request = request.set(name, value);
            }
            let outcome = match body.as_deref() {
                Some(bytes) => request.send_bytes(bytes),
                None => request.call(),
            };
            let response = match outcome {
                Ok(response) => response,
                Err(ureq::Error::Status(_, response)) => response,
                Err(error) => return Err(format!("Сетевая ошибка при запросе {current}: {error}")),
            };
            let status = response.status();
            if (300..400).contains(&status) {
                let location = response.header("location").unwrap_or_default().to_owned();
                if location.is_empty() {
                    return into_stream(response, current, status);
                }
                let target = current
                    .join(&location)
                    .map_err(|_| format!("Некорректный редирект: {location}"))?;
                let origin_changed = target.scheme() != current.scheme()
                    || target.host_str() != current.host_str()
                    || target.port_or_known_default() != current.port_or_known_default();
                if origin_changed {
                    headers.retain(|(name, _)| {
                        !matches!(name.to_ascii_lowercase().as_str(), "authorization" | "cookie" | "proxy-authorization")
                    });
                }
                if status == 303 || ((status == 301 || status == 302) && method == "POST") {
                    method = "GET".to_owned();
                    body = None;
                }
                current = target;
                continue;
            }
            return into_stream(response, current, status);
        }
        Err(format!("Слишком много редиректов при запросе {url}"))
    }
}

struct StreamResponse {
    reader: Box<dyn Read + Send + Sync>,
    total_size: Option<u64>,
    status: u16,
}

fn into_stream(response: ureq::Response, _url: url::Url, status: u16) -> Result<StreamResponse, String> {
    let total_size = response
        .header("content-length")
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|_| !response.header("content-length").unwrap_or_default().is_empty());
    Ok(StreamResponse { reader: response.into_reader(), total_size, status })
}

fn finish_response(response: ureq::Response, url: url::Url, status: u16) -> Result<DetailedResponse, String> {
    let mut headers = HashMap::new();
    for name in response.headers_names() {
        if let Some(value) = response.header(&name) {
            headers.insert(name.to_ascii_lowercase(), value.to_owned());
        }
    }
    let mut body = Vec::new();
    let mut reader = response.into_reader();
    let mut capped = reader.by_ref().take(MAX_RESPONSE_BYTES + 1);
    capped
        .read_to_end(&mut body)
        .map_err(|error| format!("Не удалось прочитать ответ {url}: {error}"))?;
    Ok(DetailedResponse {
        status,
        headers,
        url: url.to_string(),
        body,
    })
}

fn source_failure(url: &str, mirror: bool, message: &str) -> String {
    let label = if mirror { "зеркало" } else { "официальный сервер" };
    format!("{label} {}: {message}", url_host(url))
}

fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    let mut last_error = None;
    for attempt in 0..8u32 {
        match std::fs::rename(source, destination) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last_error = Some(error);
                if !cfg!(windows) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100 * (attempt as u64 + 1)));
            }
        }
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::other("rename failed")))
}

fn declared_length(response: &DetailedResponse) -> Option<u64> {
    response.header("content-length").and_then(|value| value.trim().parse::<u64>().ok())
}

pub(crate) fn form_urlencoded(values: &[(String, String)]) -> String {
    let mut encoded = String::new();
    for (index, (key, value)) in values.iter().enumerate() {
        if index > 0 {
            encoded.push('&');
        }
        encoded.push_str(&percent_encode(key));
        encoded.push('=');
        encoded.push_str(&percent_encode(value));
    }
    encoded
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*byte as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

pub(crate) fn client_from_settings(settings: &Value, operation_id: &str) -> Result<Arc<HttpClient>, String> {
    client_from_settings_opt(settings, operation_id, false)
}

pub(crate) fn progress_client_from_settings(settings: &Value, operation_id: &str) -> Result<Arc<HttpClient>, String> {
    client_from_settings_opt(settings, operation_id, true)
}

fn client_from_settings_opt(settings: &Value, operation_id: &str, progress: bool) -> Result<Arc<HttpClient>, String> {
    let timeout = settings.get("download_timeout_seconds").and_then(Value::as_i64).unwrap_or(30).max(1) as u32;
    let retries = settings.get("download_retries").and_then(Value::as_i64).unwrap_or(3).max(1) as u32;
    let proxy = settings.get("http_proxy").and_then(Value::as_str).unwrap_or("");
    let cancellation = if operation_id.trim().is_empty() {
        None
    } else {
        Some(Arc::new(super::operations::CancellationToken::new(operation_id)))
    };
    let mut client = HttpClient::new(timeout, retries, proxy, cancellation)?;
    client.set_download_source(DownloadSource::parse(
        settings.get("download_source").and_then(Value::as_str).unwrap_or("auto"),
    ));
    if progress {
        client.set_observers(Some(super::events::download_observer()), Some(super::events::request_observer()));
    }
    Ok(Arc::new(client))
}

pub fn file_hash(path: &Path, algorithm: &str) -> Result<String, String> {
    let file = File::open(path).map_err(|error| format!("Не удалось открыть {}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut buffer = [0u8; 1024 * 1024];
    let mut sha1_hasher = Sha1::new();
    let mut sha256_hasher = Sha256::new();
    let mut sha512_hasher = Sha512::new();
    let mut wrote_any = false;
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("Не удалось прочитать {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        wrote_any = true;
        match algorithm {
            "sha1" => sha1_hasher.update(&buffer[..count]),
            "sha256" => sha256_hasher.update(&buffer[..count]),
            "sha512" => sha512_hasher.update(&buffer[..count]),
            other => return Err(format!("Неподдерживаемый алгоритм хеша: {other}")),
        }
    }
    let _ = wrote_any;
    Ok(match algorithm {
        "sha1" => format!("{:x}", sha1_hasher.finalize()),
        "sha256" => format!("{:x}", sha256_hasher.finalize()),
        "sha512" => format!("{:x}", sha512_hasher.finalize()),
        other => return Err(format!("Неподдерживаемый алгоритм хеша: {other}")),
    })
}

pub fn verify_file(
    path: &Path,
    sha1: Option<&str>,
    sha256: Option<&str>,
    sha512: Option<&str>,
    size: Option<u64>,
) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    if let Some(size) = size {
        if meta.len() != size {
            return false;
        }
    }
    if let Some(expected) = sha1.filter(|value| !value.is_empty()) {
        if file_hash(path, "sha1").map(|value| value.to_lowercase()).unwrap_or_default() != expected.to_lowercase() {
            return false;
        }
    }
    if let Some(expected) = sha256.filter(|value| !value.is_empty()) {
        if file_hash(path, "sha256").map(|value| value.to_lowercase()).unwrap_or_default() != expected.to_lowercase() {
            return false;
        }
    }
    if let Some(expected) = sha512.filter(|value| !value.is_empty()) {
        if file_hash(path, "sha512").map(|value| value.to_lowercase()).unwrap_or_default() != expected.to_lowercase() {
            return false;
        }
    }
    true
}

pub fn validate_public_https_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|_| format!("Некорректный сетевой адрес: {url}"))?;
    // host_str для IPv6-литералов возвращает адрес В СКОБКАХ («[2606::…]»),
    // из-за чего парс IpAddr падал и публичные IPv6-URL отвергались.
    let host = parsed
        .host_str()
        .unwrap_or_default()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_lowercase();
    let host = host.trim_end_matches('.').to_owned();
    if parsed.scheme().to_lowercase() != "https" || host.is_empty() || parsed.username() != "" || parsed.password().is_some() {
        return Err(format!("Разрешены только безопасные HTTPS-адреса: {url}"));
    }
    if host == "localhost" || host == "localhost.localdomain" || host.ends_with(".local") {
        return Err(format!("Сетевой запрос к localhost запрещён: {host}"));
    }
    if let Ok(address) = host.parse::<IpAddr>() {
        return check_public_address(&address, &host);
    }
    let port = parsed.port_or_known_default().unwrap_or(443);
    let resolved = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| format!("Не удалось проверить сетевой адрес: {host}"))?
        .collect::<Vec<_>>();
    if resolved.is_empty() {
        return Err(format!("Не удалось проверить сетевой адрес: DNS не вернул адреса для {host}"));
    }
    for address in resolved {
        check_public_address(&address.ip(), &host)?;
    }
    Ok(())
}

fn check_public_address(address: &IpAddr, host: &str) -> Result<(), String> {
    if is_local_address(address) {
        return Err(format!("Сетевой запрос к непубличному адресу запрещён: {host} ({address})"));
    }
    Ok(())
}

fn is_local_address(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(value) => is_local_v4(*value),
        IpAddr::V6(value) => {
            if value.is_unspecified() || value.is_loopback() || value.is_multicast() {
                return true;
            }
            if let Some(mapped) = value.to_ipv4_mapped() {
                return is_local_v4(mapped);
            }
            let segments = value.segments();
            // доисторическая IPv4-совместимая форма ::a.b.c.d
            if segments[..6] == [0, 0, 0, 0, 0, 0] && (segments[6] | segments[7]) != 0 {
                let compatible = Ipv4Addr::new(
                    (segments[6] >> 8) as u8,
                    segments[6] as u8,
                    (segments[7] >> 8) as u8,
                    segments[7] as u8,
                );
                return is_local_v4(compatible);
            }
            // ULA fc00::/7
            if (segments[0] & 0xfe00) == 0xfc00 {
                return true;
            }
            // link-local fe80::/10
            if (segments[0] & 0xffc0) == 0xfe80 {
                return true;
            }
            // documentation 2001:db8::/32
            if segments[0] == 0x2001 && segments[1] == 0x0db8 {
                return true;
            }
            // teredo 2001::/32
            if segments[0] == 0x2001 && segments[1] == 0x0000 {
                return true;
            }
            // 6to4 2002::/16
            if segments[0] == 0x2002 {
                return true;
            }
            // discard-only 100::/64
            if segments[..4] == [0x0100, 0, 0, 0] {
                return true;
            }
            false
        }
    }
}

fn is_local_v4(value: Ipv4Addr) -> bool {
    let octets = value.octets();
    value.is_loopback()
        || value.is_private()
        || value.is_link_local()
        || value.is_broadcast()
        || value.is_multicast()
        || value.is_documentation()
        // CGNAT 100.64.0.0/10 — «серый» адрес за провайдерским NAT
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        // benchmarking 198.18.0.0/15
        || (octets[0] == 198 && (18..=19).contains(&octets[1]))
        // 0.0.0.0/8 — «this network», весь блок, не только 0.0.0.0
        || octets[0] == 0
        // 240.0.0.0/4 reserved
        || octets[0] >= 240
        // 192.0.0.0/24 — IETF protocol assignments
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
}

fn reject_symlink_path(path: &Path) -> Result<(), String> {
    let mut current = path.to_path_buf();
    loop {
        if std::fs::symlink_metadata(&current).map(|meta| meta.is_symlink()).unwrap_or(false) {
            return Err(format!("Небезопасный путь загрузки: {}", path.display()));
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return Ok(()),
        }
    }
}

fn file_name_of(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default()
}

fn url_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|host| host.to_lowercase()))
        .unwrap_or_default()
}

impl Clone for HttpClient {
    fn clone(&self) -> Self {
        Self {
            timeout: self.timeout,
            retries: self.retries,
            user_agent: self.user_agent.clone(),
            proxy: self.proxy.clone(),
            agent: self.agent.clone(),
            source: self.source,
            cancellation: self.cancellation.clone(),
            download_observer: self.download_observer.clone(),
            request_observer: self.request_observer.clone(),
        }
    }
}

// запись JSON с атомарной заменой и правами 0600.
pub fn atomic_json_write(path: &Path, value: &Value, private: bool) -> Result<(), String> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(directory).map_err(|error| format!("Не удалось создать каталог {}: {error}", directory.display()))?;
    let temporary = path.with_file_name(format!(".{}.tmp", file_name_of(path)));
    let body = serde_json::to_vec(value).map_err(|error| format!("Не удалось сериализовать JSON: {error}"))?;
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|error| format!("Не удалось открыть временный файл: {error}"))?;
        let mut text = String::from_utf8_lossy(&body).to_string();
        text.push('\n');
        file.write_all(text.as_bytes())
            .and_then(|_| file.flush())
            .map_err(|error| format!("Не удалось записать файл: {error}"))?;
    }
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&temporary, path).map_err(|error| format!("Не удалось сохранить файл: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ipv4_matrix() {
        let blocked = [
            "10.0.0.5", "172.16.1.1", "172.31.255.255", "192.168.0.1",
            "127.0.0.1", "169.254.1.1", "0.0.0.0", "0.1.2.3",
            "100.64.0.1", "100.127.255.254",
            "198.18.0.1", "198.19.255.254",
            "192.0.2.1", "198.51.100.7", "203.0.113.9", "192.0.0.8",
            "224.0.0.1", "240.0.0.1", "255.255.255.255",
        ];
        for text in blocked {
            let address: IpAddr = text.parse().expect(text);
            assert!(is_local_address(&address), "{text} должен блокироваться");
        }
        let public = [
            "1.1.1.1", "93.184.216.34", "172.15.255.255", "172.32.0.1",
            "192.169.0.1", "100.63.255.255", "100.128.0.1",
            "198.17.255.255", "198.20.0.1", "8.8.8.8",
        ];
        for text in public {
            let address: IpAddr = text.parse().expect(text);
            assert!(!is_local_address(&address), "{text} не должен блокироваться");
        }
    }

    #[test]
    fn local_ipv6_matrix() {
        let blocked = [
            "::1", "::", "fe80::1", "fc00::1", "fd12:3456:789a::1",
            "2001:db8::1", "ff02::1",
            "100::1",
            // IPv4-mapped: раньше проходили как «публичные» IPv6
            "::ffff:10.0.0.1", "::ffff:127.0.0.1", "::ffff:192.168.0.1", "::ffff:169.254.0.1",
            // 6to4 и teredo блокируются целиком — как is_private в CPython
            "2002:0a00:0001::1",
            "2002:5db8:d822::1",
            "2001:0000:0000:0000:0000:0000:3f57:fffe",
        ];
        for text in blocked {
            let address: IpAddr = text.parse().expect(text);
            assert!(is_local_address(&address), "{text} должен блокироваться");
        }
        let public = [
            "2606:4700:4700::1111",
            "2a00:1450:4002::1",
            "::ffff:93.184.216.34",
        ];
        for text in public {
            let address: IpAddr = text.parse().expect(text);
            assert!(!is_local_address(&address), "{text} не должен блокироваться");
        }
    }

    #[test]
    fn url_literals_are_validated_without_dns() {
        assert!(validate_public_https_url("https://93.184.216.34/index.html").is_ok());
        assert!(validate_public_https_url("https://[2606:4700:4700::1111]/").is_ok());
        assert!(validate_public_https_url("https://[::1]/").is_err());
        assert!(validate_public_https_url("https://[::ffff:10.0.0.1]/").is_err());
        assert!(validate_public_https_url("https://[fe80::1]/").is_err());
        assert!(validate_public_https_url("http://93.184.216.34/").is_err());
        assert!(validate_public_https_url("https://localhost/").is_err());
        assert!(validate_public_https_url("https://user:pass@example.com/").is_err());
    }

    #[test]
    fn resolver_keeps_public_and_drops_local_addresses() {
        // та же таблица, что использует строгий резолвер: публичный остаётся,
        // приватный и mapped отбрасываются до подключения
        let public: IpAddr = "93.184.216.34".parse().unwrap();
        let private: IpAddr = "10.0.0.8".parse().unwrap();
        let mapped: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(!is_local_address(&public));
        assert!(is_local_address(&private));
        assert!(is_local_address(&mapped));
    }

    #[test]
    fn mirror_url_maps_known_hosts() {
        assert_eq!(
            mirror_url("https://piston-data.mojang.com/v1/objects/abc/client.jar").as_deref(),
            Some("https://bmclapi2.bangbang93.com/v1/objects/abc/client.jar")
        );
        assert_eq!(
            mirror_url("https://resources.download.minecraft.net/ab/abcdef").as_deref(),
            Some("https://bmclapi2.bangbang93.com/assets/ab/abcdef")
        );
        assert_eq!(
            mirror_url("https://maven.neoforged.net/releases/net/neoforged/x.jar").as_deref(),
            Some("https://bmclapi2.bangbang93.com/maven/net/neoforged/x.jar")
        );
        assert_eq!(
            mirror_url("https://meta.fabricmc.net/v2/versions/loader?x=1").as_deref(),
            Some("https://bmclapi2.bangbang93.com/fabric-meta/v2/versions/loader?x=1")
        );
        assert!(mirror_url("https://api.modrinth.com/v2/search").is_none());
        assert!(mirror_url("https://maven.neoforged.net/releasesX/a.jar").is_none());
        assert!(mirror_url("https://piston-meta.mojang.com:8443/a.json").is_none());
    }

    #[test]
    fn candidates_follow_download_source() {
        let mut client = HttpClient::new(5, 1, "", None).unwrap();
        let url = "https://libraries.minecraft.net/a/b.jar";
        let urls = |client: &HttpClient, method: &str| -> Vec<(String, bool)> {
            client.candidates(url, method).into_iter().map(|item| (item.url, item.mirror)).collect()
        };
        assert_eq!(
            urls(&client, "GET"),
            vec![(url.to_owned(), false), ("https://bmclapi2.bangbang93.com/maven/a/b.jar".to_owned(), true)]
        );
        client.set_download_source(DownloadSource::Mirror);
        assert!(urls(&client, "GET")[0].1);
        assert_eq!(urls(&client, "POST").len(), 1);
        client.set_download_source(DownloadSource::Official);
        assert_eq!(urls(&client, "GET").len(), 1);
        assert_eq!(DownloadSource::parse("BMCLAPI"), DownloadSource::Mirror);
        assert_eq!(DownloadSource::parse("weird"), DownloadSource::Auto);
    }

    /// реальная сеть: `cargo test --lib net::tests::live -- --ignored`.
    #[test]
    #[ignore]
    fn live_download_official_and_mirror() {
        let url = "https://piston-data.mojang.com/v1/objects/0c3ec587af28e5a785c0b4a7b8a30f9a8f78f838/client.jar";
        let sha1 = "0c3ec587af28e5a785c0b4a7b8a30f9a8f78f838";
        for source in [DownloadSource::Official, DownloadSource::Mirror] {
            let mut client = HttpClient::new(2, 3, "", None).unwrap();
            client.set_download_source(source);
            let directory = std::env::temp_dir().join(format!("nivora-live-{source:?}"));
            let _ = std::fs::remove_dir_all(&directory);
            let destination = directory.join("client.jar");
            let started = Instant::now();
            assert!(client.download(url, &destination, Some(sha1), None, None, None, None, None, None).unwrap());
            eprintln!("{source:?}: {:?}", started.elapsed());
            assert!(verify_file(&destination, Some(sha1), None, None, None));
            let _ = std::fs::remove_dir_all(&directory);
        }
    }
}
