// Опциональная публикация Java-серверов с белым IPv4. Без релея, VPN,
// обхода firewall, привилегированных команд, UDP и проброса RCON. UPnP-правила
// живут десять минут и продлеваются только пока сервер работает.
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use super::net::HttpClient;
use super::upnp::{discover_gateway, parse_ipv4, source_ip, Gateway, UpnpError};

pub(crate) const LEASE_SECONDS: i64 = 600;
const RENEW_SECONDS: i64 = 240;

fn wall_time() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

pub(crate) fn port_number(value: &Value) -> Result<u16, String> {
    if value.is_boolean() {
        return Err("Порт должен быть целым числом от 1 до 65535".to_owned());
    }
    let rendered = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => return Err("Порт должен быть целым числом от 1 до 65535".to_owned()),
    };
    if rendered.is_empty() || rendered.len() > 5 || !rendered.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Порт должен быть целым числом от 1 до 65535".to_owned());
    }
    let port: i64 = rendered.parse().map_err(|_| "Порт должен быть целым числом от 1 до 65535".to_owned())?;
    if !(1..=65535).contains(&port) {
        return Err("Порт должен быть от 1 до 65535".to_owned());
    }
    Ok(port as u16)
}

/// Публичный IPv4: как IPv4Address.is_global плюс явные исключения.
pub(crate) fn public_ipv4(value: &str) -> bool {
    let Some(address) = parse_ipv4(value) else { return false };
    let octets = address.octets();
    let [a, b, c, _] = [octets[0], octets[1], octets[2], octets[3]];
    // 100.64/10 (CGNAT) тоже не глобальный по Python-семантике.
    let shared = a == 100 && (64..=127).contains(&b);
    let private = a == 0
        || a == 10
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 0 && (c <= 2 || c == 170))
        || (a == 192 && b == 168)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 240;
    let multicast = (224..=239).contains(&a);
    !(private || shared || multicast)
}

pub(crate) fn nat_kind(wan: &str, observed: &str, local: &str) -> &'static str {
    if public_ipv4(local) && (observed.is_empty() || local == observed) {
        return "direct";
    }
    if !wan.is_empty() {
        let Some(address) = parse_ipv4(wan) else { return "unknown" };
        let octets = address.octets();
        if octets[0] == 100 && (64..=127).contains(&octets[1]) {
            return "cgnat";
        }
        if !public_ipv4(wan) {
            return "private";
        }
        if !observed.is_empty() && observed != wan {
            return "double_nat";
        }
        return "public";
    }
    "unknown"
}

pub(crate) fn properties_info(text: &str, configured_port: u16) -> Value {
    static PROPERTY: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^\s*([\w-]+)(?:\s*[=:]\s*|\s+)(.*?)\s*$").expect("валидный regex properties")
    });
    let mut properties: Map<String, Value> = Map::new();
    for line in text.lines() {
        if line.trim_start().starts_with('#') || line.trim_start().starts_with('!') {
            continue;
        }
        if let Some(captures) = PROPERTY.captures(line) {
            properties.insert(captures[1].to_owned(), json!(captures[2]));
        }
    }
    let get = |name: &str| properties.get(name).and_then(Value::as_str).unwrap_or("").to_owned();
    let bind = get("server-ip");
    let bind_all = matches!(bind.as_str(), "" | "0.0.0.0" | "::");
    json!({
        "bind_address": bind,
        "bind_all": bind_all,
        "online_mode": get("online-mode").to_lowercase() != "false",
        "whitelist": get("white-list").to_lowercase() == "true",
        // Отсутствующий ключ даёт настроенный порт; пустое значение остаётся пустым.
        "properties_port": properties.get("server-port").cloned().unwrap_or(json!(configured_port.to_string())),
    })
}

pub(crate) fn observed_ipv4() -> Result<String, String> {
    // Один фиксированный HTTPS-эхо-эндпоинт: ни данных игроков, ни токенов.
    let client = HttpClient::new(5, 1, "", None)?;
    let raw = client.request("https://api.ipify.org", "GET", None, None, Some(64), None)?;
    let text = String::from_utf8_lossy(&raw).trim().to_owned();
    if !public_ipv4(&text) {
        return Err("Сервис определения IP не вернул публичный IPv4".to_owned());
    }
    Ok(text)
}

pub(crate) fn tcp_local(ip: &str, port: u16) -> bool {
    if ip.is_empty() {
        return false;
    }
    let Some(address) = parse_ipv4(ip) else { return false };
    let socket = std::net::SocketAddr::from((address, port));
    TcpStream::connect_timeout(&socket, Duration::from_millis(700)).is_ok()
}

pub(crate) fn firewall_help(port: u16) -> Vec<Value> {
    // Только для показа. Эти команды никогда не выполняются, firewall не отключается.
    vec![
        json!({
            "platform": "Linux · UFW",
            "command": format!("sudo ufw allow {port}/tcp"),
            "undo": format!("sudo ufw delete allow {port}/tcp"),
        }),
        json!({
            "platform": "Linux · firewalld",
            "command": format!("sudo firewall-cmd --add-port={port}/tcp"),
            "undo": format!("sudo firewall-cmd --remove-port={port}/tcp"),
        }),
        json!({
            "platform": "Windows · PowerShell (администратор)",
            "command": format!("New-NetFirewallRule -DisplayName \"Nivora Launcher TCP {port}\" -Direction Inbound -Action Allow -Protocol TCP -LocalPort {port}"),
            "undo": format!("Remove-NetFirewallRule -DisplayName \"Nivora Launcher TCP {port}\""),
        }),
    ]
}

fn field<'a>(mapping: &'a [(String, String)], name: &str) -> Option<&'a str> {
    mapping.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
}

struct Publication {
    gateway: Gateway,
    root: PathBuf,
    internal_port: u16,
    external_port: u16,
    description: String,
    public_ip: String,
    expires_at: f64,
    renew_at: f64,
    running: Arc<dyn Fn() -> bool + Send + Sync>,
    error: String,
    closing: bool,
    may_exist: bool,
    uncertain_lease: bool,
}

impl Publication {
    fn owns(&self, mapping: &[(String, String)]) -> bool {
        field(mapping, "NewInternalClient") == Some(self.gateway.local_ip.as_str())
            && field(mapping, "NewInternalPort") == Some(self.internal_port.to_string().as_str())
            && field(mapping, "NewPortMappingDescription") == Some(self.description.as_str())
    }

    fn lease_of(mapping: &[(String, String)]) -> Result<i64, String> {
        let raw = field(mapping, "NewLeaseDuration").unwrap_or("");
        let lease: i64 = raw
            .parse()
            .map_err(|_| "Роутер не подтвердил срок UPnP-правила".to_owned())?;
        if !(60..=LEASE_SECONDS).contains(&lease) {
            return Err("Роутер не поддерживает безопасное временное правило. Используйте ручной проброс".to_owned());
        }
        Ok(lease)
    }
}

pub(crate) struct ServerPublisher {
    publications: Mutex<HashMap<String, Publication>>,
    monitor: Mutex<Option<std::thread::JoinHandle<()>>>,
    shutdown: AtomicBool,
}

fn publication_key(root: &Path) -> String {
    // canonicalize требует существования пути — это как resolve() в Python.
    fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()).to_string_lossy().into_owned()
}

fn publication_description(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let hex: String = digest.iter().take(8).map(|byte| format!("{byte:02x}")).collect();
    // Префикс переименован из TrialLauncher: старые правила ограничены арендой
    // и исчезнут сами максимум через десять минут.
    format!("NivoraLauncher-{hex}")
}

impl ServerPublisher {
    pub(crate) fn new() -> Self {
        Self {
            publications: Mutex::new(HashMap::new()),
            monitor: Mutex::new(None),
            shutdown: AtomicBool::new(false),
        }
    }

    fn state_locked(map: &HashMap<String, Publication>, root: &Path) -> Value {
        let key = publication_key(root);
        let Some(value) = map.get(&key) else {
            return json!({"active": false, "error": ""});
        };
        json!({
            "active": !value.closing && value.expires_at > wall_time(),
            "method": "upnp",
            "internal_port": value.internal_port,
            "external_port": value.external_port,
            "local_ip": value.gateway.local_ip,
            "public_ip": value.public_ip,
            "address": format!("{}:{}", value.public_ip, value.external_port),
            "expires_at": value.expires_at,
            "lease_seconds": LEASE_SECONDS,
            "error": value.error,
            "closing": value.closing,
            "uncertain_lease": value.uncertain_lease,
        })
    }

    pub(crate) fn state(&self, root: &Path) -> Value {
        let map = self.publications.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        Self::state_locked(&map, root)
    }

    /// Создание/продление правила. Вызывается при удержанном замке.
    fn map_publication(&self, publication: &mut Publication) -> Result<(), String> {
        let upnp = |error: UpnpError| error.message;
        if let Some(existing) = publication
            .gateway
            .mapping(publication.external_port)
            .map_err(upnp)?
        {
            if !publication.owns(&existing) {
                return Err("Внешний порт уже занят. Выберите другой порт; чужие правила не заменяются".to_owned());
            }
        }
        // След чистки оставляем даже если роутер оборвётся после AddPortMapping.
        publication.may_exist = true;
        publication
            .gateway
            .add(
                publication.external_port,
                publication.internal_port,
                &publication.description,
                LEASE_SECONDS as u32,
            )
            .map_err(upnp)?;
        let mapped = publication
            .gateway
            .mapping(publication.external_port)
            .map_err(upnp)?
            .filter(|mapped| publication.owns(mapped))
            .ok_or_else(|| "Роутер не подтвердил созданный проброс".to_owned())?;
        let outcome = (|| -> Result<i64, String> {
            let lease = Publication::lease_of(&mapped)?;
            let enabled = field(&mapped, "NewEnabled").unwrap_or("");
            if enabled != "1" && enabled != "true" {
                return Err("Роутер создал отключённое UPnP-правило".to_owned());
            }
            Ok(lease)
        })();
        match outcome {
            Ok(lease) => {
                let now = wall_time();
                publication.expires_at = now + lease as f64;
                publication.renew_at = now + RENEW_SECONDS.min((lease / 2).max(1)) as f64;
                publication.error = String::new();
                publication.uncertain_lease = false;
                Ok(())
            }
            Err(message) => {
                publication.uncertain_lease = true;
                let _ = publication.gateway.delete(publication.external_port);
                publication.may_exist = false;
                Err(message)
            }
        }
    }

    pub(crate) fn open(
        &self,
        root: &Path,
        internal_port: &Value,
        external_port: &Value,
        running: Arc<dyn Fn() -> bool + Send + Sync>,
        confirmed: bool,
        info: &Value,
        gateway: Option<Gateway>,
    ) -> Result<Value, String> {
        if !confirmed {
            return Err("Подтвердите публикацию игрового TCP-порта в интернете".to_owned());
        }
        let internal_port = port_number(internal_port)?;
        let external_port = port_number(external_port)?;
        if !running() {
            return Err("Сначала запустите сервер и дождитесь его готовности".to_owned());
        }
        if !info.get("bind_all").and_then(Value::as_bool).unwrap_or(false) {
            return Err("server-ip ограничивает сетевые интерфейсы. Подготовьте server.properties и перезапустите сервер".to_owned());
        }
        let gateway = match gateway {
            Some(gateway) => gateway,
            None => discover_gateway().map_err(|error: UpnpError| error.message)?,
        };
        let wan = gateway.external_ip().map_err(|error| error.message)?;
        let observed = info.get("observed_ip").and_then(Value::as_str).unwrap_or("");
        let kind = nat_kind(&wan, observed, &gateway.local_ip);
        if matches!(kind, "private" | "cgnat" | "double_nat" | "unknown") {
            return Err("WAN роутера не совпадает с белым IPv4 или находится за NAT провайдера. Нужен белый IP / проброс на внешнем роутере".to_owned());
        }
        if !tcp_local(&gateway.local_ip, internal_port) {
            return Err("Игровой TCP-порт ещё не отвечает на локальном IP. Дождитесь запуска или проверьте server-ip / firewall".to_owned());
        }

        let key = publication_key(root);
        let mut map = self.publications.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(previous) = map.get_mut(&key) {
            if previous.external_port != external_port || previous.internal_port != internal_port {
                return Err("Сначала закройте текущий проброс перед изменением порта".to_owned());
            }
            if previous.closing {
                return Err("Предыдущий проброс ещё закрывается. Повторите удаление и проверьте его статус".to_owned());
            }
            if previous.expires_at <= wall_time() || !previous.error.is_empty() {
                previous.public_ip = wan.clone();
                if let Err(message) = self.map_publication(previous) {
                    previous.error = message.clone();
                    return Err(message);
                }
            }
            return Ok(Self::state_locked(&map, root));
        }
        if map
            .values()
            .any(|value| value.gateway.peer == gateway.peer && value.external_port == external_port)
        {
            return Err("Этот внешний порт уже используется другим сервером лаунчера".to_owned());
        }
        let now = wall_time();
        let mut publication = Publication {
            gateway,
            root: root.to_path_buf(),
            internal_port,
            external_port,
            description: publication_description(&key),
            public_ip: wan,
            expires_at: now + LEASE_SECONDS as f64,
            renew_at: now,
            running,
            error: String::new(),
            closing: false,
            may_exist: false,
            uncertain_lease: false,
        };
        if let Err(message) = self.map_publication(&mut publication) {
            if publication.may_exist {
                publication.closing = true;
                publication.error = message.clone();
                map.insert(key.clone(), publication);
                drop(map);
                self.start_monitor();
            }
            return Err(message);
        }
        map.insert(key, publication);
        drop(map);
        self.start_monitor();
        Ok(self.state(root))
    }

    fn start_monitor(&self) {
        let mut monitor = self.monitor.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if monitor.is_none() {
            *monitor = Some(std::thread::spawn(|| publisher().maintain()));
        }
    }

    /// Закрытие правила при удержанном замке. map.pop делает только владелец правила.
    fn close_locked(&self, map: &mut HashMap<String, Publication>, key: &str) -> Result<(), String> {
        let Some(publication) = map.get_mut(key) else {
            return Ok(());
        };
        publication.closing = true;
        let existing = publication.gateway.mapping(publication.external_port);
        match existing {
            Ok(Some(mapping)) => {
                if !publication.owns(&mapping) {
                    // Правило уже не наше: не трогаем чужое.
                    map.remove(key);
                    return Err("Правило порта изменено вне лаунчера; чужое правило не удалено".to_owned());
                }
                let deleted = publication.gateway.delete(publication.external_port);
                match deleted {
                    Ok(()) => {
                        map.remove(key);
                        Ok(())
                    }
                    Err(error) => {
                        publication.error = error.message.clone();
                        Err(error.message)
                    }
                }
            }
            Ok(None) => {
                map.remove(key);
                Ok(())
            }
            Err(error) => {
                publication.error = error.message.clone();
                Err(error.message)
            }
        }
    }

    pub(crate) fn close(&self, root: &Path) -> Result<Value, String> {
        let key = publication_key(root);
        let mut map = self.publications.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !map.contains_key(&key) {
            return Ok(Self::state_locked(&map, root));
        }
        self.close_locked(&mut map, &key)?;
        Ok(Self::state_locked(&map, root))
    }

    pub(crate) fn tick(&self, now: Option<f64>) {
        let now = now.unwrap_or_else(wall_time);
        let mut map = self.publications.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let keys: Vec<String> = map.keys().cloned().collect();
        for key in keys {
            // Как в Python: действия и ошибки обрабатываются одной веткой.
            let mut failure: Option<String> = None;
            let mut do_close = false;
            let mut do_renew = false;
            if let Some(value) = map.get_mut(&key) {
                if value.closing || !(value.running)() {
                    do_close = true;
                } else if now >= value.renew_at {
                    match value.gateway.external_ip() {
                        Ok(current_wan) if public_ipv4(&current_wan) => {
                            value.public_ip = current_wan;
                            do_renew = true;
                        }
                        Ok(_) => {
                            value.closing = true;
                            do_close = true;
                        }
                        Err(error) => failure = Some(error.message),
                    }
                }
            }
            if do_close {
                if let Err(message) = self.close_locked(&mut map, &key) {
                    failure = Some(message);
                }
            }
            if do_renew {
                if let Some(value) = map.get_mut(&key) {
                    if let Err(message) = self.map_publication(value) {
                        failure = Some(message);
                    }
                }
            }
            if let Some(message) = failure {
                if let Some(value) = map.get_mut(&key) {
                    value.error = message;
                    value.renew_at = now + 30.0;
                    if value.uncertain_lease {
                        value.closing = true;
                    }
                    let uncertain = value.uncertain_lease;
                    let may_exist = value.may_exist;
                    let expired = now >= value.expires_at;
                    if !may_exist || (expired && !uncertain) {
                        map.remove(&key);
                    }
                }
            }
        }
    }

    fn maintain(&self) {
        // Пятнадцатисекундный цикл с быстрым выходом при остановке.
        loop {
            if self.shutdown.load(Ordering::Relaxed) {
                return;
            }
            for _ in 0..15 {
                if self.shutdown.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            self.tick(None);
        }
    }

    pub(crate) fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let roots: Vec<PathBuf> = {
            let map = self.publications.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            map.values().map(|value| value.root.clone()).collect()
        };
        for root in roots {
            // Жёсткий выход всё равно ограничен арендой на роутере.
            let _ = self.close(&root);
        }
    }
}

static PUBLISHER: LazyLock<ServerPublisher> = LazyLock::new(ServerPublisher::new);

pub(crate) fn publisher() -> &'static ServerPublisher {
    &PUBLISHER
}

pub(crate) fn diagnose(
    root: &Path,
    port: &Value,
    properties: &str,
    running: bool,
    external_port: Option<&Value>,
) -> Result<Value, String> {
    let port = port_number(port)?;
    let external_port = match external_port {
        Some(value) => port_number(value)?,
        None => port,
    };
    let mut warnings: Vec<String> = Vec::new();
    let mut observed = String::new();
    if let Ok(value) = observed_ipv4() {
        observed = value;
    } else {
        warnings.push("Внешний IPv4 не удалось определить через HTTPS. Это не означает отсутствие белого IP".to_owned());
    }
    let mut local = source_ip("1.1.1.1");
    let mut gateway = None;
    let mut wan = String::new();
    match discover_gateway() {
        Ok(found) => {
            local = found.local_ip.clone();
            match found.external_ip() {
                Ok(address) => wan = address,
                Err(error) => warnings.push(error.message),
            }
            gateway = Some(found);
        }
        Err(error) => warnings.push(error.message),
    }
    let kind = nat_kind(&wan, &observed, &local);
    let public = match kind {
        "direct" => {
            if public_ipv4(&observed) {
                observed.clone()
            } else {
                local.clone()
            }
        }
        "public" => {
            if public_ipv4(&observed) {
                observed.clone()
            } else {
                wan.clone()
            }
        }
        "unknown" => observed.clone(),
        _ => String::new(),
    };
    if matches!(kind, "private" | "cgnat" | "double_nat") {
        warnings.push("Есть NAT провайдера или второй роутер. Проброс на одном роутере не откроет сервер в интернет".to_owned());
    }
    let info = properties_info(properties, port);
    if !info["bind_all"].as_bool().unwrap_or(false) {
        warnings.push("server-ip должен быть пустым или 0.0.0.0 для входящих подключений".to_owned());
    }
    if !info["online_mode"].as_bool().unwrap_or(true) {
        warnings.push("online-mode=false: публичный сервер не проверяет учётные записи. Рекомендуется online-mode=true".to_owned());
    }
    if !info["whitelist"].as_bool().unwrap_or(false) {
        warnings.push("Рекомендуется включить white-list=true и добавить доверенных игроков командой whitelist add <ник>".to_owned());
    }
    let mapping = publisher().state(root);
    let local_check_target = if local.is_empty() { "127.0.0.1".to_owned() } else { local.clone() };
    Ok(json!({
        "bind_address": info["bind_address"],
        "bind_all": info["bind_all"],
        "online_mode": info["online_mode"],
        "whitelist": info["whitelist"],
        "properties_port": info["properties_port"],
        "internal_port": port,
        "external_port": external_port,
        "local_ip": local,
        "observed_ip": observed,
        "wan_ip": wan,
        "public_ip": public,
        "address": if public.is_empty() { String::new() } else { format!("{public}:{external_port}") },
        "nat_kind": kind,
        "upnp_available": gateway.is_some(),
        "router_name": gateway.as_ref().map(|gateway| gateway.name.clone()).unwrap_or_default(),
        "router_ip": gateway.as_ref().map(|gateway| gateway.peer.clone()).unwrap_or_default(),
        "running": running,
        "local_listening": running && tcp_local(&local_check_target, port),
        "external_verified": false,
        "mapping": mapping,
        "warnings": warnings,
        "firewall_help": firewall_help(port),
        "verification_note": "Локальная проверка не доказывает доступность из интернета. Проверьте адрес с другого подключения, например мобильного интернета",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_reject_bools_and_garbage() {
        assert_eq!(port_number(&json!(25565)).unwrap(), 25565);
        assert_eq!(port_number(&json!("25565")).unwrap(), 25565);
        assert!(port_number(&json!(true)).is_err());
        assert!(port_number(&json!(0)).is_err());
        assert!(port_number(&json!(65536)).is_err());
        assert!(port_number(&json!("12a")).is_err());
        assert!(port_number(&json!("99999")).is_err());
        assert!(port_number(&json!(null)).is_err());
        assert!(port_number(&json!(1.5)).is_err());
    }

    #[test]
    fn public_ipv4_classification() {
        assert!(public_ipv4("8.8.8.8"));
        assert!(public_ipv4("1.1.1.1"));
        assert!(!public_ipv4("10.0.0.1"));
        assert!(!public_ipv4("192.168.1.1"));
        assert!(!public_ipv4("172.20.1.1"));
        assert!(!public_ipv4("127.0.0.1"));
        assert!(!public_ipv4("169.254.9.9"));
        assert!(!public_ipv4("100.64.0.1"), "CGNAT не белый");
        assert!(!public_ipv4("224.0.0.1"), "multicast");
        assert!(!public_ipv4("240.0.0.1"), "reserved");
        assert!(!public_ipv4("192.0.2.1"), "TEST-NET");
        assert!(!public_ipv4("0.0.0.0"));
        assert!(!public_ipv4("мусор"));
    }

    #[test]
    fn nat_kinds_follow_wan_and_observed() {
        // Белый локальный и совпадающий наблюдённый — прямое подключение.
        assert_eq!(nat_kind("", "", "8.8.8.8"), "direct");
        assert_eq!(nat_kind("", "8.8.8.8", "8.8.8.8"), "direct");
        // Локальный серый, а WAN белый и совпал с наблюдённым — проброс роутера.
        assert_eq!(nat_kind("45.132.19.55", "45.132.19.55", "192.168.1.10"), "public");
        // Наблюдённый не совпал с WAN — двойной NAT.
        assert_eq!(nat_kind("45.132.19.55", "46.1.2.3", "192.168.1.10"), "double_nat");
        // WAN в диапазоне CGNAT.
        assert_eq!(nat_kind("100.72.0.1", "", "192.168.1.10"), "cgnat");
        // WAN серый и без WAN вообще.
        assert_eq!(nat_kind("10.5.0.1", "", "192.168.1.10"), "private");
        assert_eq!(nat_kind("", "8.8.8.8", "192.168.1.10"), "unknown");
        assert_eq!(nat_kind("кривой", "", "192.168.1.10"), "unknown");
    }

    #[test]
    fn properties_info_parses_assignments_and_colons() {
        let text = "# комментарий\nserver-port=25599\nserver-ip:\nonline-mode: false\nwhite-list=true\nquery.port 25566\n!bang=1\n";
        let info = properties_info(text, 25565);
        assert_eq!(info["properties_port"], json!("25599"));
        assert_eq!(info["bind_address"], json!(""));
        assert_eq!(info["bind_all"], json!(true));
        assert_eq!(info["online_mode"], json!(false));
        assert_eq!(info["whitelist"], json!(true));

        let restricted = properties_info("server-ip=192.168.1.5\n", 25565);
        assert_eq!(restricted["bind_all"], json!(false));
        let ipv6_any = properties_info("server-ip=::\n", 25565);
        assert_eq!(ipv6_any["bind_all"], json!(true));
        let empty_port = properties_info("server-port=\n", 25565);
        assert_eq!(empty_port["properties_port"], json!(""));
        let absent_port = properties_info("motd=Hello\n", 25565);
        assert_eq!(absent_port["properties_port"], json!("25565"));
    }

    #[test]
    fn firewall_help_is_display_only() {
        let help = firewall_help(25565);
        assert_eq!(help.len(), 3);
        assert!(help[0]["command"].as_str().unwrap().contains("25565"));
        assert!(help.iter().all(|entry| entry["undo"].is_string()));
    }

    #[test]
    fn publisher_state_is_inactive_for_unknown_roots() {
        let publisher = ServerPublisher::new();
        let state = publisher.state(Path::new("/tmp"));
        assert_eq!(state["active"], json!(false));
        assert_eq!(state["error"], json!(""));
        // Остановка без публикаций безопасна.
        publisher.shutdown();
    }

    #[test]
    fn descriptions_are_stable_and_prefixed() {
        let key = "/srv/mc-server";
        let first = publication_description(key);
        let second = publication_description(key);
        assert_eq!(first, second);
        assert!(first.starts_with("NivoraLauncher-"));
        assert_eq!(first.len(), "NivoraLauncher-".len() + 16);
        assert!(publication_description("/другой") != first);
    }
}
