// HTTP-клиент лаунчера: только HTTPS, проверка адресов перед запросом
// (SSRF), рейт-лимиты по доменам, повторные попытки, ограничение размера
// ответов и скачивание во временный файл с проверкой хешей.
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::net::{IpAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::operations::CancellationToken;

pub const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

const PUBLIC_RESERVED_HOSTS: &[&str] = &["piston-meta.mojang.com"];
const MAX_REDIRECTS: usize = 5;

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

// Без потолка ожидания один интерактивный поиск, вставший в очередь за
// пакетной проверкой обновлений, крутится бы минутами.
const MAX_RATE_LIMIT_WAIT_SECONDS: f64 = 15.0;

fn rate_limits() -> &'static Mutex<HashMap<String, Instant>> {
    static LIMITS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    LIMITS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub type Progress = Box<dyn Fn(u64, Option<u64>) + Send + Sync>;
pub type DownloadObserver = Box<dyn Fn(&str, &str, &Path, u64, Option<u64>) + Send + Sync>;
pub type RequestObserver = Box<dyn Fn(&str, &str, u32) + Send + Sync>;
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

pub struct HttpClient {
    timeout: Duration,
    retries: u32,
    user_agent: String,
    proxy: String,
    agent: ureq::Agent,
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
        let mut builder = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout(Duration::from_secs(timeout_seconds.max(1) as u64))
            .redirects(0);
        if !proxy.is_empty() {
            // socks5h = SOCKS5 с remote-DNS; ureq различия не делает, DNS
            // и так резолвит прокси.
            let normalized = proxy.replacen("socks5h://", "socks5://", 1);
            let parsed = ureq::Proxy::new(&normalized)
                .map_err(|error| format!("Некорректный прокси {proxy}: {error}"))?;
            builder = builder.proxy(parsed);
        }
        Ok(Self {
            timeout: Duration::from_secs(timeout_seconds.max(1) as u64),
            retries: retries.max(1),
            user_agent: format!("{}/{}", super::constants::APP_NAME, super::constants::APP_VERSION),
            proxy: proxy.to_owned(),
            agent: builder.build(),
            cancellation,
            download_observer: None,
            request_observer: None,
        })
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

    /// Один HTTP-ход без автоматических редиректов: адрес каждого перехода
    /// проверяется заново, при смене origin авторизационные заголовки
    /// снимаются.
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
        let mut headers_vec: Vec<(String, String)> = vec![
            ("User-Agent".into(), self.user_agent.clone()),
            ("Accept".into(), "application/json".into()),
        ];
        headers_vec.extend(headers.unwrap_or(&[]).to_vec());
        let limit = max_bytes.unwrap_or(MAX_RESPONSE_BYTES);
        let mut last_error = String::new();
        for attempt in 1..=self.retries {
            self.check_cancelled()?;
            self.rate_limit_before(url)?;
            self.notify_request("start", url, attempt);
            match self.open_once(url, method, data, &headers_vec, redirect_validator) {
                Ok(response) => {
                    if let Some(declared) = declared_length(&response) {
                        if declared > limit {
                            return Err("Ответ сервиса слишком большой".to_owned());
                        }
                    }
                    if response.body.len() as u64 > limit {
                        return Err("Ответ сервиса слишком большой".to_owned());
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
                            self.wait(retry_after)?;
                            continue;
                        }
                        return Err(format!(
                            "HTTP 429: сервер временно ограничил запросы к {url}. Повторите позже."
                        ));
                    }
                    if response.status >= 500 && attempt < self.retries {
                        self.notify_request("retry", url, attempt);
                        self.wait(1.0 + attempt as f64)?;
                        continue;
                    }
                    if response.status >= 400 {
                        return Err(format!("HTTP {} при запросе {}", response.status, response.url));
                    }
                    return Ok(response.body);
                }
                Err(error) => {
                    last_error = error;
                    if attempt < self.retries {
                        self.notify_request("retry", url, attempt);
                        self.wait(1.0 + attempt as f64)?;
                        continue;
                    }
                }
            }
        }
        Err(format!("Сетевая ошибка при запросе {url}: {last_error}"))
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
            match self.open_once(url, method, data, &headers_vec, None) {
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

    /// POST формы со статусом ответа: OAuth device-code использует HTTP 400
    /// для промежуточных состояний, это не ошибка.
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
        validate_public_https_url(url)?;
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
        let _ = std::fs::remove_file(&temporary);
        let mut last_error = String::new();
        for attempt in 1..=self.retries {
            self.check_cancelled()?;
            self.notify_request("download", url, attempt);
            let mut headers_vec: Vec<(String, String)> = vec![("User-Agent".into(), self.user_agent.clone())];
            headers_vec.extend(headers.unwrap_or(&[]).to_vec());
            match self.open_stream(url, &headers_vec, redirect_validator) {
                Ok(mut stream) => {
                    let total_size = stream.total_size.or(size);
                    if let Some(total) = total_size {
                        if total > MAX_DOWNLOAD_BYTES {
                            return Err("Файл загрузки слишком большой".to_owned());
                        }
                    }
                    let mut written: u64 = 0;
                    let outcome = (|| -> Result<(), String> {
                        let mut handle = File::create(&temporary)
                            .map_err(|error| format!("Не удалось создать временный файл: {error}"))?;
                        let mut buffer = [0u8; 1024 * 1024];
                        loop {
                            self.check_cancelled()?;
                            let count = stream
                                .reader
                                .read(&mut buffer)
                                .map_err(|error| format!("Обрыв загрузки: {error}"))?;
                            if count == 0 {
                                break;
                            }
                            written += count as u64;
                            if written > MAX_DOWNLOAD_BYTES {
                                return Err("Файл загрузки слишком большой".to_owned());
                            }
                            handle
                                .write_all(&buffer[..count])
                                .map_err(|error| format!("Не удалось записать файл: {error}"))?;
                            if let Some(progress) = progress {
                                progress(written, total_size);
                            }
                            self.notify_download("progress", url, destination, written, total_size);
                        }
                        Ok(())
                    })();
                    if let Err(error) = outcome {
                        let _ = std::fs::remove_file(&temporary);
                        return Err(error);
                    }
                    if !verify_file(&temporary, sha1, sha256, sha512, size) {
                        let _ = std::fs::remove_file(&temporary);
                        return Err(format!("Проверка хеша не пройдена: {url}"));
                    }
                    std::fs::rename(&temporary, destination)
                        .map_err(|error| format!("Не удалось сохранить файл: {error}"))?;
                    self.notify_download("complete", url, destination, written, total_size);
                    return Ok(true);
                }
                Err(error) => {
                    let _ = std::fs::remove_file(&temporary);
                    last_error = error;
                    if attempt < self.retries {
                        self.notify_request("retry", url, attempt);
                        self.wait(1.0 + attempt as f64)?;
                        continue;
                    }
                }
            }
        }
        Err(format!("Не удалось скачать {url}: {last_error}"))
    }

    /// Ответ в виде потока — для download(); редиректы обрабатывает open_once.
    fn open_stream(
        &self,
        url: &str,
        headers: &[(String, String)],
        redirect_validator: Option<RedirectValidator<'_>>,
    ) -> Result<StreamResponse, String> {
        validate_public_https_url(url)?;
        self.check_cancelled()?;
        let outcome = self.open_once_raw(url, "GET", None, headers, redirect_validator)?;
        if outcome.status >= 400 {
            return Err(format!("HTTP {} при запросе {}", outcome.status, url));
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

fn into_stream(response: ureq::Response, url: url::Url, status: u16) -> Result<StreamResponse, String> {
    if status >= 400 {
        return Err(format!("HTTP {status} при запросе {url}"));
    }
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

fn declared_length(response: &DetailedResponse) -> Option<u64> {
    response.header("content-length").and_then(|value| value.trim().parse::<u64>().ok())
}

fn form_urlencoded(values: &[(String, String)]) -> String {
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

/// Клиент из нормализованных настроек: таймаут/повторы/прокси общие для всех
/// подсистем операции, отмена — по operation_id запроса.
pub(crate) fn client_from_settings(settings: &Value, operation_id: &str) -> Result<Arc<HttpClient>, String> {
    let timeout = settings.get("download_timeout_seconds").and_then(Value::as_i64).unwrap_or(30).max(1) as u32;
    let retries = settings.get("download_retries").and_then(Value::as_i64).unwrap_or(3).max(1) as u32;
    let proxy = settings.get("http_proxy").and_then(Value::as_str).unwrap_or("");
    let cancellation = if operation_id.trim().is_empty() {
        None
    } else {
        Some(Arc::new(super::operations::CancellationToken::new(operation_id)))
    };
    Ok(Arc::new(HttpClient::new(timeout, retries, proxy, cancellation)?))
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

/// Проверка адреса перед запросом: только HTTPS, без учётных данных в URL,
/// никаких localhost и непубличных адресов. Через SOCKS-прокси DNS резолвит
/// прокси, поэтому локальный резолв не выполняется.
pub fn validate_public_https_url(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|_| format!("Некорректный сетевой адрес: {url}"))?;
    let host = parsed.host_str().unwrap_or_default().to_lowercase();
    let host = host.trim_end_matches('.').to_owned();
    if parsed.scheme().to_lowercase() != "https" || host.is_empty() || parsed.username() != "" || parsed.password().is_some() {
        return Err(format!("Разрешены только безопасные HTTPS-адреса: {url}"));
    }
    if host == "localhost" || host == "localhost.localdomain" || host.ends_with(".local") {
        return Err(format!("Сетевой запрос к localhost запрещён: {host}"));
    }
    if let Ok(address) = host.parse::<IpAddr>() {
        check_public_address(&address, &host)?;
        return Ok(());
    }
    let port = parsed.port_or_known_default().unwrap_or(443);
    let resolved = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| format!("Не удалось проверить сетевой адрес: {host}"))?
        .filter_map(|socket| socket.ip().is_ipv4().then_some(socket.ip()).or_else(|| Some(socket.ip())))
        .collect::<Vec<_>>();
    if resolved.is_empty() {
        return Err(format!("Не удалось проверить сетевой адрес: DNS не вернул адреса для {host}"));
    }
    for address in resolved {
        check_public_address(&address, &host)?;
    }
    Ok(())
}

fn check_public_address(address: &IpAddr, host: &str) -> Result<(), String> {
    // У Mojang есть публично маршрутизируемое выделение, которое таблицы
    // помечают как reserved; исключение действует только для этого хоста.
    let exception = address.is_ipv6() && is_reserved_v6(address) && PUBLIC_RESERVED_HOSTS.contains(&host);
    if exception {
        return Ok(());
    }
    let blocked = address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        || is_private(address)
        || is_link_local(address)
        || is_reserved(address);
    if blocked {
        return Err(format!("Сетевой запрос к непубличному адресу запрещён: {host} ({address})"));
    }
    Ok(())
}

fn is_private(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(value) => {
            let octets = value.octets();
            octets[0] == 10 || (octets[0] == 172 && (16..=31).contains(&octets[1])) || (octets[0] == 192 && octets[1] == 168)
        }
        IpAddr::V6(value) => (value.segments()[0] & 0xfe00) == 0xfc00,
    }
}

fn is_link_local(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(value) => value.is_link_local(),
        IpAddr::V6(value) => (value.segments()[0] & 0xffc0) == 0xfe80,
    }
}

fn is_reserved(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(value) => {
            let octets = value.octets();
            octets[0] >= 240 || value.is_broadcast()
        }
        IpAddr::V6(_) => false,
    }
}

fn is_reserved_v6(address: &IpAddr) -> bool {
    matches!(address, IpAddr::V6(value) if (value.segments()[0] & 0xff00) == 0xff00)
        || matches!(address, IpAddr::V6(value) if value.segments()[0] == 0x2001 && value.segments()[1] == 0x0db8)
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
            cancellation: self.cancellation.clone(),
            download_observer: None,
            request_observer: None,
        }
    }
}

// Запись JSON с атомарной заменой и правами 0600.
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
