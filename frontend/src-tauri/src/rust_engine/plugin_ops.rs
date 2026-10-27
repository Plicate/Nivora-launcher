//! операции плагинов лаунчера

use base64::Engine;
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::{Duration, Instant};

use super::config::ConfigStore;
use super::events::emit_status_event;
use super::net::{client_from_settings, HttpClient};
use super::plugins::PluginStore;

const PLUGIN_MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const INSTANCE_LIVE_STATE_FILE: &str = ".nivora-launcher-live.json";
const MAX_INSTANCE_LIVE_STATE_JSON_BYTES: u64 = 64 * 1024;

fn plugin_store() -> Result<PluginStore, String> {
    let config = ConfigStore::load()?;
    PluginStore::new(&config)
}

fn plugin_id_of(request: &Value) -> String {
    request
        .get("plugin_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn require_permission(store: &PluginStore, plugin_id: &str, permission: &str) -> Result<Value, String> {
    store.authorize_operation(plugin_id, permission)
}

fn require_permission_any(store: &PluginStore, plugin_id: &str, permissions: &[&str]) -> Result<Value, String> {
    let mut last_error = "Разрешение плагина не выдано".to_owned();
    for permission in permissions {
        match store.authorize_operation(plugin_id, permission) {
            Ok(manifest) => return Ok(manifest),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

// ------------------------------------------------------------ простые ----

pub fn plugin_security_status(_request: &Value) -> Result<Value, String> {
    plugin_store()?.security_status()
}

pub fn plugin_permission_grants_set(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin = store.set_permission_grants(
        &plugin_id_of(request),
        request.get("permissions").unwrap_or(&Value::Null),
    )?;
    Ok(json!({ "plugin": plugin, "plugins": store.list()? }))
}

pub fn plugin_local_untrusted_execution(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin = store.set_local_untrusted_execution(
        &plugin_id_of(request),
        request.get("allowed").and_then(Value::as_bool).unwrap_or(false),
    )?;
    Ok(json!({ "plugin": plugin, "plugins": store.list()? }))
}

pub fn plugin_publisher_pin(request: &Value) -> Result<Value, String> {
    plugin_store()?.pin_publisher(
        request.get("key_id").and_then(Value::as_str).unwrap_or_default().trim(),
        request.get("public_key").and_then(Value::as_str).unwrap_or_default(),
        request.get("revocation_authority").and_then(Value::as_bool).unwrap_or(false),
    )
}

pub fn plugin_publisher_unpin(request: &Value) -> Result<Value, String> {
    plugin_store()?.unpin_publisher(request.get("key_id").and_then(Value::as_str).unwrap_or_default().trim())
}

pub fn plugin_publisher_revocation_authority_set(request: &Value) -> Result<Value, String> {
    plugin_store()?.set_publisher_revocation_authority(
        request.get("key_id").and_then(Value::as_str).unwrap_or_default().trim(),
        request.get("enabled").and_then(Value::as_bool).unwrap_or(false),
    )
}

pub fn plugin_revocations_import(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let imported = store.import_revocations_json(
        request
            .get("document_json")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    )?;
    let mut result = store.security_status()?;
    if let Some(object) = result.as_object_mut() {
        object.insert("imported".to_owned(), imported);
    }
    Ok(result)
}

pub fn plugin_local_revoke(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin = store.revoke_plugin(
        &plugin_id_of(request),
        request.get("reason").and_then(Value::as_str).unwrap_or_default(),
    )?;
    Ok(json!({ "plugin": plugin, "plugins": store.list()? }))
}

pub fn plugin_local_unrevoke(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin = store.unrevoke_plugin(&plugin_id_of(request))?;
    Ok(json!({ "plugin": plugin, "plugins": store.list()? }))
}

pub fn plugin_install(request: &Value) -> Result<Value, String> {
    let encoded = request.get("data").and_then(Value::as_str).unwrap_or_default();
    if encoded.len() > 4 * ((PLUGIN_MAX_ARCHIVE_BYTES + 2) / 3) {
        return Err("Архив плагина слишком большой".to_owned());
    }
    let content = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Архив плагина повреждён".to_owned())?;
    let store = plugin_store()?;
    let plugin = store.install_archive(
        &content,
        request.get("enable").and_then(Value::as_bool).unwrap_or(false),
        request.get("replace").and_then(Value::as_bool).unwrap_or(false),
    )?;
    let name = plugin.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
    emit_status_event("plugins-changed", &format!("Плагин «{name}» установлен."));
    Ok(json!({ "plugin": plugin, "plugins": store.list()? }))
}

pub fn plugin_set_enabled(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin = store.set_enabled(
        &plugin_id_of(request),
        request.get("enabled").and_then(Value::as_bool).unwrap_or(false),
    )?;
    let name = plugin.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
    let state = if plugin.get("enabled").and_then(Value::as_bool).unwrap_or(false) { "включён" } else { "выключен" };
    emit_status_event("plugins-changed", &format!("Плагин «{name}» {state}."));
    Ok(json!({ "plugin": plugin, "plugins": store.list()? }))
}

pub fn plugin_uninstall(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    let name = store
        .list()?
        .iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(plugin_id.as_str()))
        .map(|item| item.get("name").and_then(Value::as_str).unwrap_or_default().to_owned())
        .unwrap_or_else(|| plugin_id.clone());
    store.uninstall(&plugin_id)?;
    emit_status_event("plugins-changed", &format!("Плагин «{name}» удалён."));
    Ok(json!({ "removed": true, "plugins": store.list()? }))
}

// ------------------------------------------------------- runtime/api ----

pub fn plugin_api_source(request: &Value) -> Result<Value, String> {
    plugin_store()?.runtime_source(&plugin_id_of(request))
}

pub fn plugin_api_runtime_info(request: &Value) -> Result<Value, String> {
    plugin_store()?.runtime_info(&plugin_id_of(request))
}

pub fn plugin_api_operation_trace(request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let store = PluginStore::new(&config)?;
    let plugin_id = request.get("plugin_id").and_then(Value::as_str).unwrap_or_default().to_owned();
    store.runtime_source(&plugin_id)?;
    let guard = super::plugin_guard::guard();
    let method = request.get("method").and_then(Value::as_str).unwrap_or_default();
    if method.is_empty() || method.len() > 128 || !method.chars().all(|char| char.is_ascii_alphanumeric() || char == '.') {
        return Err("Invalid plugin audit method".to_owned());
    }
    let arguments = request.get("arguments").cloned().unwrap_or_else(|| json!({}));
    if !arguments.is_object() {
        return Err("Invalid audit arguments".to_owned());
    }
    let outcome = request.get("outcome").and_then(Value::as_str).unwrap_or("begin");
    if !matches!(outcome, "begin" | "ok" | "denied") {
        return Err("Invalid audit outcome".to_owned());
    }
    let elapsed = request.get("elapsed_ms").and_then(Value::as_f64).unwrap_or(0.0);
    let elapsed = if !(0.0..=300_000.0).contains(&elapsed) { 0.0 } else { elapsed };
    let mut arguments = arguments;
    if let Some(object) = arguments.as_object_mut() {
        object.insert("event".to_owned(), request.get("event").cloned().unwrap_or(Value::Null));
        object.insert("listener_id".to_owned(), request.get("listener_id").cloned().unwrap_or(Value::Null));
    }
    // сбой записи аудита не должен мешать ответу плагину
    let _ = guard.audit(
        &config.paths.data_home.join("audit"),
        &plugin_id,
        method,
        &arguments,
        outcome,
        elapsed,
    );
    Ok(json!({ "recorded": true }))
}

pub fn plugin_api_capabilities(request: &Value) -> Result<Value, String> {
    let info = plugin_store()?.runtime_info(&plugin_id_of(request))?;
    Ok(json!({
        "plugin_id": info.get("plugin_id").cloned().unwrap_or(Value::Null),
        "api_version": info.get("api_version").cloned().unwrap_or(Value::Null),
        "capabilities": info.get("capabilities").cloned().unwrap_or(Value::Null),
        "sandbox_profile": info.get("sandbox_profile").cloned().unwrap_or(Value::Null),
    }))
}

pub fn plugin_api_permissions_get(request: &Value) -> Result<Value, String> {
    plugin_store()?.permission_summary(&plugin_id_of(request))
}

fn validate_plugin_permission_reduction(granted: &[String], requested: &Value) -> Result<(), String> {
    let permissions = requested
        .as_array()
        .ok_or_else(|| "Список grants должен содержать строки".to_owned())?;
    if permissions.iter().any(|permission| {
        permission
            .as_str()
            .map(|permission| !granted.iter().any(|item| item == permission))
            .unwrap_or(true)
    }) {
        return Err("Плагин может только отзывать выданные разрешения; новые выдаются в менеджере плагинов".to_owned());
    }
    Ok(())
}

pub fn plugin_api_permissions_set(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    let current = store.permission_summary(&plugin_id)?;
    let granted: Vec<String> = current
        .get("granted_permissions")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let empty = Value::Array(Vec::new());
    let requested = request.get("permissions").unwrap_or(&empty);
    validate_plugin_permission_reduction(&granted, requested)?;
    let plugin = store.reduce_permission_grants(&plugin_id, requested)?;
    let permissions = store.permission_summary(&plugin_id)?;
    Ok(json!({ "plugin": plugin, "permissions": permissions }))
}

pub fn plugin_api_authorize(request: &Value) -> Result<Value, String> {
    let plugin_id = plugin_id_of(request);
    let Some(permission) = request.get("permission").and_then(Value::as_str) else {
        return Err("Разрешение плагина должно быть строкой".to_owned());
    };
    plugin_store()?.authorize_operation(&plugin_id, permission)?;
    Ok(json!({ "authorized": true }))
}

pub fn plugin_api_open_external(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    let manifest = require_permission_any(&store, &plugin_id, &["os:open-url", "links:open"])?;
    let Some(url) = request.get("url").and_then(Value::as_str) else {
        return Err("Некорректная внешняя ссылка плагина".to_owned());
    };
    if url.is_empty() || url != url.trim() || url.chars().any(|char| char < ' ' || char == '\u{7F}') {
        return Err("Некорректная внешняя ссылка плагина".to_owned());
    }
    let parsed = url::Url::parse(url).map_err(|_| "Некорректная внешняя ссылка плагина".to_owned())?;
    let scheme = parsed.scheme().to_lowercase();
    let host = parsed.host_str().unwrap_or_default().to_lowercase();
    let host = host.trim_end_matches('.');
    if (scheme != "http" && scheme != "https") || host.is_empty() {
        return Err("Плагинам разрешены только HTTP(S)-ссылки без credentials".to_owned());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Плагинам разрешены только HTTP(S)-ссылки без credentials".to_owned());
    }
    if scheme == "http" {
        let hosts = manifest.get("network_hosts").and_then(Value::as_array);
        let allowed = hosts.map(|items| items.iter().any(|item| item.as_str() == Some(host))).unwrap_or(false);
        if hosts.map(|items| !items.is_empty()).unwrap_or(false) && !allowed {
            return Err("Хост не входит в allowlist плагина".to_owned());
        }
    }
    super::platform_support::open_path(Path::new(url))
        .map_err(|error| format!("Не удалось открыть ссылку плагина: {error}"))?;
    Ok(json!({ "opened": true }))
}

pub fn plugin_api_instances_list(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "instances:read")?;
    let config = ConfigStore::load()?;
    let instances = super::instances::InstanceStore::new(&config)?;
    Ok(json!({ "instances": instances.list().iter().map(|item| item.public()).collect::<Vec<Value>>() }))
}

pub fn plugin_api_process_metrics(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "instances:read")?;
    require_permission(&store, &plugin_id, "process:observe")?;
    let name = request.get("instance_name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let config = ConfigStore::load()?;
    let instances = super::instances::InstanceStore::new(&config)?;
    let info = instances.get(&name)?;
    // читаем running, pid и started_at из JSON с ограниченным размером
    let state = super::state::read_bounded_json(&info.path.join(INSTANCE_LIVE_STATE_FILE), MAX_INSTANCE_LIVE_STATE_JSON_BYTES)
        .unwrap_or(Value::Null);
    let mut running = info.running;
    let mut started_at = 0.0f64;
    let mut pid = 0i32;
    if state.is_object() {
        if state.get("running").and_then(Value::as_bool).unwrap_or(false) {
            running = true;
            pid = state.get("pid").and_then(Value::as_i64).unwrap_or(0) as i32;
            if pid <= 0 {
                running = false;
            }
        } else {
            running = false;
        }
        started_at = state.get("started_at").and_then(Value::as_f64).unwrap_or(0.0);
    }
    let metrics = super::metrics::instance_metrics(&info.name, pid, started_at, running, &info.path, None);
    Ok(json!({
        "instance": info.public(),
        "running": running,
        "pid": pid,
        "started_at": started_at,
        "metrics": metrics,
    }))
}

pub fn plugin_api_config_projected_read(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "files:launcher-config-projected")?;
    let schema_id = request.get("schema_id").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    store.projected_launcher_config(&schema_id)
}

// -------------------------------------------------------- сеть/DNS ----

/// проверяет DNS-имя из списка разрешённых хостов; IP-адреса не допускаются
fn plugin_network_host(manifest: &Value, value: Option<&Value>) -> Result<String, String> {
    let host = value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_lowercase()
        .trim_end_matches('.')
        .to_owned();
    if host.is_empty() || host.len() > 253 || host.chars().any(|char| char.is_whitespace()) || host.contains('/') || host.contains('@') {
        return Err("Некорректное имя хоста плагина".to_owned());
    }
    if host.parse::<IpAddr>().is_ok() {
        return Err("Плагинам запрещены IP-адреса; укажите DNS-имя".to_owned());
    }
    let allowed = manifest
        .get("network_hosts")
        .and_then(Value::as_array)
        .map(|items| items.iter().any(|item| item.as_str() == Some(host.as_str())))
        .unwrap_or(false);
    if !allowed {
        return Err("Хост не входит в allowlist плагина".to_owned());
    }
    Ok(host)
}

/// все адреса, полученные через DNS, должны быть глобальными
fn resolve_public_plugin_host(manifest: &Value, host: &str, port: Option<u16>) -> Result<Vec<SocketAddr>, String> {
    let host_value = json!(host);
    let host = plugin_network_host(manifest, Some(&host_value))?;
    let service = port.unwrap_or(0);
    let records: Vec<SocketAddr> = (host.as_str(), service)
        .to_socket_addrs()
        .map_err(|error| format!("DNS не разрешил {host}: {error}"))?
        .collect();
    if records.is_empty() {
        return Err(format!("DNS не вернул адреса для {host}"));
    }
    for record in &records {
        let ip = record.ip();
        if super::net::is_local_address(&ip) {
            return Err(format!("Плагинам запрещены localhost и private network: {host} ({ip})"));
        }
    }
    Ok(records)
}

/// порт может быть от 1 до 65535
fn plugin_port(value: Option<&Value>, default: Option<u16>) -> Result<Option<u16>, String> {
    let Some(value) = value else { return Ok(default); };
    if value.is_null() {
        return Ok(default);
    }
    if let Some(text) = value.as_str() {
        if text.trim().is_empty() {
            return Ok(default);
        }
    }
    let port = value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse::<u64>().ok()))
        .ok_or_else(|| "Порт должен быть числом от 1 до 65535".to_owned())?;
    if !(1..=65535).contains(&port) {
        return Err("Порт должен быть числом от 1 до 65535".to_owned());
    }
    Ok(Some(port as u16))
}

/// таймаут задаётся от 100 до 5000 мс
fn plugin_timeout_ms(value: Option<&Value>) -> Result<i64, String> {
    let fallback = json!(3000);
    let value = value.unwrap_or(&fallback);
    let timeout = value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse::<i64>().ok()))
        .ok_or_else(|| "Timeout должен быть числом от 100 до 5000 мс".to_owned())?;
    if !(100..=5000).contains(&timeout) {
        return Err("Timeout должен быть числом от 100 до 5000 мс".to_owned());
    }
    Ok(timeout)
}

fn validated_headers(raw: Option<&Value>, base: Vec<(String, String)>) -> Result<Vec<(String, String)>, String> {
    let Some(object) = raw.filter(|value| value.is_object()).and_then(Value::as_object) else {
        return Err("Заголовки сетевого запроса плагина имеют неверный формат".to_owned());
    };
    if object.len() > 64 {
        return Err("Заголовки сетевого запроса плагина имеют неверный формат".to_owned());
    }
    let mut headers = base;
    for (name, value) in object {
        let valid_name = !name.is_empty()
            && name.len() <= 64
            && name.chars().all(|char| char.is_ascii_alphanumeric() || char == '-');
        let forbidden = [
            "host", "cookie", "content-length", "connection", "origin", "referer", "user-agent",
            "proxy-authorization",
        ];
        let Some(text) = value.as_str() else {
            return Err("Сетевой запрос плагина содержит небезопасный заголовок".to_owned());
        };
        if !valid_name || forbidden.contains(&name.to_lowercase().as_str()) || text.len() > 8192 || text.contains('\r') || text.contains('\n') {
            return Err("Сетевой запрос плагина содержит небезопасный заголовок".to_owned());
        }
        headers.push((name.clone(), text.to_owned()));
    }
    Ok(headers)
}

fn request_body_bytes(body: Option<&Value>, method: &str, allowed_methods: &[&str]) -> Result<Option<Vec<u8>>, String> {
    let Some(body) = body else { return Ok(None); };
    if body.is_null() {
        return Ok(None);
    }
    if !allowed_methods.contains(&method) {
        return Err("JSON-тело плагина разрешено только для POST и PUT".to_owned());
    }
    let bytes = serde_json::to_vec(body).map_err(|_| "Тело сетевого запроса плагина должно быть JSON".to_owned())?;
    if bytes.len() > 1024 * 1024 {
        return Err("Тело сетевого запроса плагина слишком большое".to_owned());
    }
    Ok(Some(bytes))
}

fn http_client() -> Result<std::sync::Arc<HttpClient>, String> {
    // клиент не сохраняется: запросы плагинов краткосрочные
    let settings = ConfigStore::load()?.settings().clone();
    client_from_settings(&settings, "plugin-network")
}

pub fn plugin_network_fetch(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    let manifest = require_permission_any(&store, &plugin_id, &["network:fetch", "network:https"])?;
    let http = http_client()?;
    if !http.proxy().is_empty() {
        return Err("Сетевые запросы плагинов через прокси запрещены: адрес узла невозможно проверить на стороне соединения".to_owned());
    }
    let url = request.get("url").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let parsed = url::Url::parse(&url).map_err(|_| "Некорректный адрес плагина".to_owned())?;
    let host = parsed.host_str().unwrap_or_default().to_lowercase().trim_end_matches('.').to_owned();
    let port = parsed.port();
    if parsed.scheme().to_lowercase() != "https" || host.is_empty() || !parsed.username().is_empty() || parsed.password().is_some() || port.is_some() {
        return Err("Плагинам разрешены только HTTPS-адреса без credentials и нестандартного порта".to_owned());
    }
    let allowlist: Vec<String> = manifest
        .get("network_hosts")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    if !allowlist.contains(&host) {
        return Err("Домен не входит в allowlist плагина".to_owned());
    }
    resolve_public_plugin_host(&manifest, &host, Some(443))?;

    let request_options = request.get("request").filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({}));
    let method = request_options.get("method").and_then(Value::as_str).unwrap_or("GET").to_uppercase();
    if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "DELETE") {
        return Err("Метод сетевого запроса плагина не поддерживается".to_owned());
    }
    let headers = validated_headers(
        request_options.get("headers"),
        vec![
            ("Accept".to_owned(), "application/json".to_owned()),
        ],
    )?;
    let mut headers = headers;
    let body = request_body_bytes(request_options.get("body"), &method, &["POST", "PUT"])?;
    if body.is_some() && !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("content-type")) {
        headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
    }
    let validate_redirect = |target: &str| -> Result<(), String> {
        let redirected = url::Url::parse(target).map_err(|_| "Сервис плагина вернул некорректный redirect".to_owned())?;
        let redirected_host = redirected.host_str().unwrap_or_default().to_lowercase().trim_end_matches('.').to_owned();
        let redirected_port = redirected.port();
        if redirected.scheme().to_lowercase() != "https"
            || redirected_host.is_empty()
            || !redirected.username().is_empty()
            || redirected.password().is_some()
            || redirected_port.is_some()
        {
            return Err("Redirect плагина должен оставаться HTTPS без credentials и нестандартного порта".to_owned());
        }
        if !allowlist.contains(&redirected_host) {
            return Err("Redirect плагина вышел за пределы allowlist".to_owned());
        }
        resolve_public_plugin_host(&manifest, &redirected_host, Some(443))?;
        Ok(())
    };
    let raw = http.request_detailed(
        &url,
        &method,
        body.as_deref(),
        Some(&headers),
        Some(4 * 1024 * 1024),
        Some(&validate_redirect),
    )?;
    if !(200..300).contains(&raw.status) {
        return Err(format!("HTTP {} при запросе плагина", raw.status));
    }
    let value: Value = serde_json::from_slice(&raw.body).map_err(|_| "Сервис плагина вернул некорректный JSON".to_owned())?;
    Ok(json!({ "value": value }))
}

fn require_plugin_https(scheme: &str) -> Result<(), String> {
    if scheme == "https" {
        Ok(())
    } else {
        Err("Плагинам разрешены только HTTPS-запросы".to_owned())
    }
}

fn require_plugin_direct_network(proxy: &str) -> Result<(), String> {
    if proxy.is_empty() {
        Ok(())
    } else {
        Err("HTTP-запросы плагинов через прокси запрещены: адрес узла невозможно проверить на стороне соединения".to_owned())
    }
}

pub fn plugin_http_request(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    let request_options = request.get("request").filter(|value| value.is_object()).cloned().unwrap_or_else(|| json!({}));
    let url = request_options.get("url").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let parsed = url::Url::parse(&url).map_err(|_| "Некорректный адрес HTTP-запроса плагина".to_owned())?;
    let host = parsed.host_str().unwrap_or_default().to_lowercase().trim_end_matches('.').to_owned();
    let port = parsed.port();
    let scheme = parsed.scheme().to_lowercase();
    require_plugin_https(&scheme)?;
    if host.is_empty() || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Плагинам разрешены только HTTPS-адреса без credentials".to_owned());
    }
    let manifest = require_permission_any(&store, &plugin_id, &["network:https", "network:fetch"])?;
    let allowlist: Vec<String> = manifest
        .get("network_hosts")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(|item| item.as_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    if !allowlist.contains(&host) {
        return Err("Домен не входит в allowlist плагина".to_owned());
    }
    let default_port = if scheme == "https" { 443 } else { 80 };
    resolve_public_plugin_host(&manifest, &host, port.or(Some(default_port)))?;

    let method = request_options.get("method").and_then(Value::as_str).unwrap_or("GET").to_uppercase();
    if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS") {
        return Err("HTTP-метод плагина не поддерживается".to_owned());
    }
    let timeout_ms = plugin_timeout_ms(request_options.get("timeout_ms"))?;
    let mut headers = validated_headers(request_options.get("headers"), Vec::new())?;
    let body: Option<Vec<u8>> = match request_options.get("body") {
        None => None,
        Some(value) if value.is_null() => None,
        Some(Value::String(text)) => Some(text.clone().into_bytes()),
        Some(other) => {
            let bytes = serde_json::to_vec(other)
                .map_err(|_| "Тело HTTP-запроса плагина должно быть сериализуемым JSON или строкой".to_owned())?;
            if !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("content-type")) {
                headers.push(("Content-Type".to_owned(), "application/json".to_owned()));
            }
            Some(bytes)
        }
    };
    if let Some(bytes) = &body {
        if bytes.len() > 1024 * 1024 {
            return Err("Тело HTTP-запроса плагина слишком большое".to_owned());
        }
    }
    let http = http_client()?;
    let proxy = http.proxy();
    require_plugin_direct_network(&proxy)?;
    let validate_redirect = |target: &str| -> Result<(), String> {
        let redirected = url::Url::parse(target).map_err(|_| "Некорректный адрес плагина".to_owned())?;
        let redirected_host = redirected.host_str().unwrap_or_default().to_lowercase().trim_end_matches('.').to_owned();
        let redirected_port = redirected.port().unwrap_or(if redirected.scheme().to_lowercase() == "https" { 443 } else { 80 });
        resolve_public_plugin_host(&manifest, &redirected_host, Some(redirected_port))?;
        Ok(())
    };
    let raw = http.request_detailed(
        &url,
        &method,
        body.as_deref(),
        Some(&headers),
        Some(4 * 1024 * 1024),
        Some(&validate_redirect),
    )?;
    let body_text = String::from_utf8(raw.body.clone()).unwrap_or_else(|_| String::from_utf8_lossy(&raw.body).to_string());
    let content_type = raw
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    let body_json: Option<Value> = if content_type.to_lowercase().contains("json") {
        serde_json::from_str(&body_text).ok()
    } else {
        None
    };
    Ok(json!({
        "status": raw.status,
        "url": raw.url,
        "headers": raw.headers,
        "body_text": body_text,
        "body_json": body_json,
        "timeout_ms": timeout_ms,
    }))
}

pub fn plugin_network_dns(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    let manifest = require_permission(&store, &plugin_id, "network:dns")?;
    let host = plugin_network_host(&manifest, request.get("host"))?;
    let port = plugin_port(request.get("port"), None)?;
    let records = resolve_public_plugin_host(&manifest, &host, port)?;
    let mut addresses: Vec<Value> = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new();
    for record in records {
        let family = if record.is_ipv6() { "ipv6" } else { "ipv4" };
        let address = record.ip().to_string();
        let key = (family.to_owned(), address.clone());
        if !seen.contains(&key) {
            seen.push(key);
            addresses.push(json!({ "family": family, "address": address }));
        }
    }
    addresses.truncate(64);
    Ok(json!({ "host": host, "port": port, "addresses": addresses }))
}

pub fn plugin_network_ping(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    let manifest = require_permission(&store, &plugin_id, "network:ping")?;
    let host = plugin_network_host(&manifest, request.get("host"))?;
    let port = plugin_port(request.get("port"), Some(25565))?.unwrap_or(25565);
    let timeout_ms = plugin_timeout_ms(request.get("timeout_ms"))?;
    let records = resolve_public_plugin_host(&manifest, &host, Some(port))?;
    let started = Instant::now();
    let mut reachable = false;
    let mut error = String::new();
    for record in records {
        match TcpStream::connect_timeout(&record, Duration::from_millis(timeout_ms as u64)) {
            Ok(_) => {
                reachable = true;
                error.clear();
                break;
            }
            Err(excuse) => error = excuse.to_string(),
        }
    }
    Ok(json!({
        "host": host,
        "port": port,
        "reachable": reachable,
        "latency_ms": (started.elapsed().as_secs_f64() * 1000.0 * 100.0).round() / 100.0,
        "error": error,
    }))
}

// --------------------------------------------------- proxy routes ----

fn text_list(raw: Option<&Value>, label: &str, maximum: usize) -> Result<Vec<String>, String> {
    let Some(value) = raw else { return Ok(Vec::new()); };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let Some(items) = value.as_array() else {
        return Err(format!("{label} должен быть списком ограниченного размера"));
    };
    if items.len() > maximum {
        return Err(format!("{label} должен быть списком ограниченного размера"));
    }
    let mut values = Vec::new();
    for item in items {
        let text = match item {
            Value::String(text) => text.trim().to_owned(),
            other => other.to_string(),
        };
        if text.is_empty() || text.len() > 253 {
            return Err(format!("{label} содержит некорректное значение"));
        }
        values.push(text);
    }
    Ok(values)
}

fn port_list(raw: Option<&Value>, label: &str) -> Result<Vec<u16>, String> {
    let Some(value) = raw else { return Ok(Vec::new()); };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let Some(items) = value.as_array() else {
        return Err(format!("{label} должен быть списком портов"));
    };
    if items.len() > 64 {
        return Err(format!("{label} должен быть списком портов"));
    }
    let mut ports = Vec::new();
    for item in items {
        ports.push(plugin_port(Some(item), None)?.unwrap_or(25565));
    }
    Ok(ports)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 80 && value.chars().all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-'))
}

fn proxy_route_entry(value: Option<&Value>, fallback_id: &str) -> Result<Value, String> {
    let Some(object) = value.filter(|value| value.is_object()) else {
        return Err("Proxy route policy плагина должна быть объектом".to_owned());
    };
    let route_id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| fallback_id.to_owned())
        .trim()
        .to_owned();
    if !valid_id(&route_id) {
        return Err("Proxy route policy содержит небезопасный id".to_owned());
    }
    let priority_value = object.get("priority").cloned().unwrap_or(json!(100));
    let priority = priority_value
        .as_f64()
        .ok_or_else(|| "Proxy route policy.priority должен быть числом".to_owned())? as i64;
    if !(0..=100_000).contains(&priority) {
        return Err("Proxy route policy.priority вне допустимого диапазона".to_owned());
    }
    let empty = json!({});
    let match_object = object.get("match").unwrap_or(&empty);
    if !match_object.is_object() {
        return Err("Proxy route policy.match должен быть объектом".to_owned());
    }
    let action = object.get("action").unwrap_or(&empty);
    if !action.is_object() {
        return Err("Proxy route policy.action должен быть объектом".to_owned());
    }
    let action_type = action.get("type").and_then(Value::as_str).unwrap_or_default().trim().to_uppercase();
    if !matches!(action_type.as_str(), "DIRECT" | "PROXY" | "RELAY" | "DROP" | "MIRROR") {
        return Err("Proxy route policy.action.type не поддерживается".to_owned());
    }
    let protocols = text_list(match_object.get("protocol"), "Proxy route protocol", 8)?
        .into_iter()
        .map(|item| item.to_lowercase())
        .collect::<Vec<String>>();
    for item in &protocols {
        if !matches!(item.as_str(), "tcp" | "udp" | "quic") {
            return Err("Proxy route protocol поддерживает только tcp/udp/quic".to_owned());
        }
    }
    let mut normalized_match = Map::new();
    normalized_match.insert("hostEquals".to_owned(), json!(text_list(match_object.get("hostEquals"), "Proxy route hostEquals", 64)?));
    normalized_match.insert("hostSuffix".to_owned(), json!(text_list(match_object.get("hostSuffix"), "Proxy route hostSuffix", 64)?));
    normalized_match.insert("portIn".to_owned(), json!(port_list(match_object.get("portIn"), "Proxy route portIn")?));
    normalized_match.insert("protocol".to_owned(), json!(protocols));
    normalized_match.insert("sniEquals".to_owned(), json!(text_list(match_object.get("sniEquals"), "Proxy route sniEquals", 64)?));
    normalized_match.insert("processTag".to_owned(), json!(text_list(match_object.get("processTag"), "Proxy route processTag", 64)?));
    normalized_match.insert("instanceId".to_owned(), json!(text_list(match_object.get("instanceId"), "Proxy route instanceId", 64)?));

    let mut normalized_action = Map::new();
    normalized_action.insert("type".to_owned(), json!(action_type));
    if let Some(session_id) = action.get("sessionId") {
        let session_id = session_id.as_str().unwrap_or_default().trim();
        if session_id.is_empty()
            || session_id.len() > 120
            || !session_id.chars().all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | ':' | '-'))
        {
            return Err("Proxy route sessionId содержит небезопасное значение".to_owned());
        }
        normalized_action.insert("sessionId".to_owned(), json!(session_id));
    }
    if let Some(rewrite) = action.get("rewrite").filter(|value| !value.is_null()) {
        let Some(rewrite_object) = rewrite.as_object() else {
            return Err("Proxy route rewrite должен быть объектом".to_owned());
        };
        let mut normalized_rewrite = Map::new();
        if let Some(target_host) = rewrite_object.get("targetHost") {
            let target_host = target_host.as_str().unwrap_or_default().trim();
            if target_host.is_empty() || target_host.len() > 253 {
                return Err("Proxy route rewrite.targetHost содержит некорректный host".to_owned());
            }
            normalized_rewrite.insert("targetHost".to_owned(), json!(target_host));
        }
        if let Some(target_port) = rewrite_object.get("targetPort") {
            normalized_rewrite.insert("targetPort".to_owned(), json!(plugin_port(Some(target_port), None)?));
        }
        if let Some(override_sni) = rewrite_object.get("overrideSni") {
            let override_sni = override_sni.as_str().unwrap_or_default().trim();
            if override_sni.is_empty() || override_sni.len() > 253 {
                return Err("Proxy route rewrite.overrideSni содержит некорректное значение".to_owned());
            }
            normalized_rewrite.insert("overrideSni".to_owned(), json!(override_sni));
        }
        if let Some(add_headers) = rewrite_object.get("addHeaders") {
            let Some(headers_object) = add_headers.as_object() else {
                return Err("Proxy route rewrite.addHeaders должен быть ограниченным объектом".to_owned());
            };
            if headers_object.len() > 32 {
                return Err("Proxy route rewrite.addHeaders должен быть ограниченным объектом".to_owned());
            }
            let mut headers = Map::new();
            for (name, value) in headers_object {
                headers.insert(name.clone(), json!(value.as_str().unwrap_or_default()));
            }
            normalized_rewrite.insert("addHeaders".to_owned(), Value::Object(headers));
        }
        if let Some(remove_headers) = rewrite_object.get("removeHeaders") {
            normalized_rewrite.insert("removeHeaders".to_owned(), json!(text_list(Some(remove_headers), "Proxy route removeHeaders", 32)?));
        }
        normalized_action.insert("rewrite".to_owned(), Value::Object(normalized_rewrite));
    }

    let mut result = Map::new();
    result.insert("id".to_owned(), json!(route_id));
    result.insert("priority".to_owned(), json!(priority));
    result.insert("match".to_owned(), Value::Object(normalized_match));
    result.insert("action".to_owned(), Value::Object(normalized_action));
    if let Some(failover) = object.get("failover").filter(|value| !value.is_null()) {
        let Some(failover_object) = failover.as_object() else {
            return Err("Proxy route failover должен быть объектом".to_owned());
        };
        let strategy = failover_object.get("strategy").and_then(Value::as_str).unwrap_or("none").trim().to_lowercase();
        if !matches!(strategy.as_str(), "none" | "next-route" | "direct-fallback" | "retry-same") {
            return Err("Proxy route failover.strategy не поддерживается".to_owned());
        }
        let mut normalized_failover = Map::new();
        normalized_failover.insert("strategy".to_owned(), json!(strategy));
        if let Some(max_attempts) = failover_object.get("maxAttempts") {
            let attempts = max_attempts.as_i64().unwrap_or(0);
            if !(0..=32).contains(&attempts) {
                return Err("Proxy route failover.maxAttempts вне диапазона".to_owned());
            }
            normalized_failover.insert("maxAttempts".to_owned(), json!(attempts));
        }
        result.insert("failover".to_owned(), Value::Object(normalized_failover));
    }
    Ok(Value::Object(result))
}

fn proxy_route_list(value: Option<&Value>) -> Result<Vec<Value>, String> {
    let Some(items) = value.filter(|value| !value.is_null()).and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    if items.len() > 128 {
        return Err("Список proxy route policy плагина должен быть ограниченным списком".to_owned());
    }
    let mut result = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let entry = proxy_route_entry(Some(item), &format!("route-{}", index + 1))?;
        let id = entry.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
        if seen.contains(&id) {
            return Err(format!("Повторяющийся id proxy route policy: {id}"));
        }
        seen.push(id);
        result.push(entry);
    }
    Ok(result)
}

pub fn plugin_api_proxy_routes_get(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "network:proxy-control")?;
    let stored = store.read_storage(&plugin_id, "proxy-routes")?;
    Ok(json!({ "routes": proxy_route_list(stored.as_array().map(|items| Value::Array(items.clone())).as_ref())? }))
}

pub fn plugin_api_proxy_routes_save(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "network:proxy-control")?;
    let routes = proxy_route_list(request.get("routes"))?;
    store.write_storage(&plugin_id, "proxy-routes", &json!(routes))?;
    Ok(json!({ "routes": routes }))
}

pub fn plugin_api_proxy_routes_add(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "network:proxy-control")?;
    let stored = store.read_storage(&plugin_id, "proxy-routes")?;
    let mut routes = proxy_route_list(stored.as_array().map(|items| Value::Array(items.clone())).as_ref())?;
    let fallback = format!("route-{}", super::mods::random_hex(6));
    let route = proxy_route_entry(request.get("route"), &fallback)?;
    let route_id = route.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
    if routes.iter().any(|item| item.get("id").and_then(Value::as_str) == Some(route_id.as_str())) {
        return Err(format!("Правило маршрутизации уже существует: {route_id}"));
    }
    if routes.len() >= 128 {
        return Err("Список proxy route policy плагина заполнен".to_owned());
    }
    routes.push(route.clone());
    store.write_storage(&plugin_id, "proxy-routes", &json!(routes))?;
    Ok(json!({ "route": route, "routes": routes }))
}

pub fn plugin_api_proxy_routes_remove(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "network:proxy-control")?;
    let route_id = request.get("route_id").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let stored = store.read_storage(&plugin_id, "proxy-routes")?;
    let routes = proxy_route_list(stored.as_array().map(|items| Value::Array(items.clone())).as_ref())?;
    let next_routes: Vec<Value> = routes
        .iter()
        .filter(|item| item.get("id").and_then(Value::as_str) != Some(route_id.as_str()))
        .cloned()
        .collect();
    let removed = next_routes.len() != routes.len();
    store.write_storage(&plugin_id, "proxy-routes", &json!(next_routes))?;
    Ok(json!({ "removed": removed, "routes": next_routes }))
}

// ------------------------------------------------------ server list ----

fn server_entry(value: Option<&Value>, fallback_id: &str) -> Result<Value, String> {
    let Some(object) = value.filter(|value| value.is_object()) else {
        return Err("Сервер плагина должен быть объектом".to_owned());
    };
    let server_id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| fallback_id.to_owned())
        .trim()
        .to_owned();
    if !valid_id(&server_id) {
        return Err("Некорректный id сервера плагина".to_owned());
    }
    let name = object.get("name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let address = object
        .get("address")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .trim_matches(|char| char == '[' || char == ']')
        .to_owned();
    if name.is_empty() || name.len() > 120 {
        return Err("Имя сервера плагина обязательно и должно быть не длиннее 120 символов".to_owned());
    }
    if address.is_empty()
        || address.len() > 253
        || address.chars().any(|char| char.is_whitespace())
        || address.contains('/')
        || address.contains('@')
    {
        return Err("Некорректный адрес сервера плагина".to_owned());
    }
    let port = plugin_port(object.get("port"), None)?.unwrap_or(25565);
    let tags = object.get("tags").and_then(Value::as_array).cloned().unwrap_or_default();
    let normalized_tags: Vec<String> = tags
        .iter()
        .filter_map(|tag| tag.as_str().map(str::to_owned))
        .map(|tag| tag.trim().chars().take(40).collect::<String>())
        .filter(|tag| !tag.is_empty())
        .take(16)
        .collect();
    Ok(json!({
        "id": server_id,
        "name": name,
        "address": address,
        "port": port,
        "description": object.get("description").and_then(Value::as_str).unwrap_or_default().trim().chars().take(500).collect::<String>(),
        "tags": normalized_tags,
    }))
}

fn server_list(value: Option<&Value>) -> Result<Vec<Value>, String> {
    let Some(items) = value.filter(|value| !value.is_null()).and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    if items.len() > 200 {
        return Err("Список серверов плагина должен содержать не более 200 записей".to_owned());
    }
    let mut result = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let entry = server_entry(Some(item), &format!("server-{}", index + 1))?;
        let id = entry.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
        if seen.contains(&id) {
            return Err(format!("Повторяющийся id сервера: {id}"));
        }
        seen.push(id);
        result.push(entry);
    }
    Ok(result)
}

pub fn plugin_server_list_get(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "launcher:servers")?;
    let stored = store.read_storage(&plugin_id, "server-list")?;
    Ok(json!({ "servers": server_list(stored.as_array().map(|items| Value::Array(items.clone())).as_ref())? }))
}

pub fn plugin_server_list_save(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "launcher:servers")?;
    let servers = server_list(request.get("servers"))?;
    store.write_storage(&plugin_id, "server-list", &json!(servers))?;
    Ok(json!({ "servers": servers }))
}

pub fn plugin_server_list_add(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "launcher:servers")?;
    let stored = store.read_storage(&plugin_id, "server-list")?;
    let mut servers = server_list(stored.as_array().map(|items| Value::Array(items.clone())).as_ref())?;
    let fallback = format!("server-{}", super::mods::random_hex(6));
    let entry = server_entry(request.get("server"), &fallback)?;
    let entry_id = entry.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
    if servers.iter().any(|item| item.get("id").and_then(Value::as_str) == Some(entry_id.as_str())) {
        return Err(format!("Сервер уже есть в списке: {entry_id}"));
    }
    if servers.len() >= 200 {
        return Err("Список серверов плагина заполнен".to_owned());
    }
    servers.push(entry.clone());
    store.write_storage(&plugin_id, "server-list", &json!(servers))?;
    Ok(json!({ "server": entry, "servers": servers }))
}

pub fn plugin_server_list_remove(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "launcher:servers")?;
    let server_id = request.get("server_id").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let stored = store.read_storage(&plugin_id, "server-list")?;
    let servers = server_list(stored.as_array().map(|items| Value::Array(items.clone())).as_ref())?;
    let next_servers: Vec<Value> = servers
        .iter()
        .filter(|item| item.get("id").and_then(Value::as_str) != Some(server_id.as_str()))
        .cloned()
        .collect();
    let removed = next_servers.len() != servers.len();
    store.write_storage(&plugin_id, "server-list", &json!(next_servers))?;
    Ok(json!({ "removed": removed, "servers": next_servers }))
}

// ------------------------------------------- storage / config / файлы ----

pub fn plugin_storage_get(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "storage:workspace")?;
    Ok(json!({ "value": store.read_storage(&plugin_id, request.get("key").and_then(Value::as_str).unwrap_or_default())? }))
}

pub fn plugin_storage_set(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "storage:workspace")?;
    store.write_storage(
        &plugin_id,
        request.get("key").and_then(Value::as_str).unwrap_or_default(),
        request.get("value").unwrap_or(&Value::Null),
    )?;
    Ok(json!({ "saved": true }))
}

pub fn plugin_storage_remove(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "storage:workspace")?;
    store.remove_storage(&plugin_id, request.get("key").and_then(Value::as_str).unwrap_or_default())?;
    Ok(json!({ "removed": true }))
}

pub fn plugin_config_get(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "storage:workspace")?;
    Ok(json!({ "value": store.config_get(&plugin_id, request.get("key").and_then(Value::as_str).unwrap_or_default())? }))
}

pub fn plugin_config_set(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "storage:workspace")?;
    store.config_set(
        &plugin_id,
        request.get("key").and_then(Value::as_str).unwrap_or_default(),
        request.get("value").unwrap_or(&Value::Null),
    )?;
    Ok(json!({ "saved": true }))
}

pub fn plugin_config_remove(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "storage:workspace")?;
    store.config_remove(&plugin_id, request.get("key").and_then(Value::as_str).unwrap_or_default())?;
    Ok(json!({ "removed": true }))
}

pub fn plugin_file_list(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "files:plugin")?;
    Ok(json!({ "files": store.list_files(&plugin_id, request.get("directory").and_then(Value::as_str).unwrap_or_default())? }))
}

pub fn plugin_file_read(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "files:plugin")?;
    Ok(json!({ "content": store.read_file(&plugin_id, request.get("path").and_then(Value::as_str).unwrap_or_default())? }))
}

pub fn plugin_file_write(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "files:plugin")?;
    store.write_file(
        &plugin_id,
        request.get("path").and_then(Value::as_str).unwrap_or_default(),
        request.get("content").and_then(Value::as_str).unwrap_or_default(),
    )?;
    Ok(json!({ "saved": true }))
}

pub fn plugin_file_remove(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "files:plugin")?;
    store.remove_file(&plugin_id, request.get("path").and_then(Value::as_str).unwrap_or_default())?;
    Ok(json!({ "removed": true }))
}

pub fn plugin_api_files(request: &Value, action: &str) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    let scope = request.get("scope").and_then(Value::as_str).unwrap_or("plugin");
    let path = request.get("path").and_then(Value::as_str).unwrap_or_default();
    if scope == "plugin" {
        require_permission(&store, &plugin_id, "files:plugin")?;
        return match action {
            "list" => Ok(json!({ "files": store.list_file_info(&plugin_id, request.get("directory").and_then(Value::as_str).unwrap_or_default())? })),
            "read" => Ok(json!({ "content": store.read_file(&plugin_id, path)? })),
            "write" => {
                let Some(content) = request.get("content").and_then(Value::as_str) else {
                    return Err("Текстовый файл должен содержать UTF-8 текст".to_owned());
                };
                store.write_file(&plugin_id, path, content)?;
                Ok(json!({ "saved": true }))
            }
            _ => {
                store.remove_file(&plugin_id, path)?;
                Ok(json!({ "removed": true }))
            }
        };
    }
    if scope == "instance" {
        require_permission(&store, &plugin_id, "files:instance")?;
        let instance_name = request.get("instance_name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
        let config = ConfigStore::load()?;
        let files = super::instance_files::InstanceFiles::new(&config, None)?;
        return match action {
            "list" => {
                let entries = files.list_files(&instance_name, request.get("directory").and_then(Value::as_str).unwrap_or_default())?;
                let rows: Vec<Value> = entries
                    .iter()
                    .filter(|item| item.get("kind").and_then(Value::as_str) == Some("file"))
                    .map(|item| {
                        json!({
                            "path": item.get("path").cloned().unwrap_or(Value::Null),
                            "size": item.get("size").cloned().unwrap_or(Value::Null),
                            "modified_at": item.get("modified_at").cloned().unwrap_or(Value::Null),
                        })
                    })
                    .collect();
                Ok(json!({ "files": rows }))
            }
            "read" => files.read_file(&instance_name, path, "utf-8"),
            "write" => {
                let Some(content) = request.get("content").and_then(Value::as_str) else {
                    return Err("Текстовый файл должен содержать UTF-8 текст".to_owned());
                };
                files.write_file(&instance_name, path, content)?;
                Ok(json!({ "saved": true }))
            }
            _ => {
                files.delete(&instance_name, path)?;
                Ok(json!({ "removed": true }))
            }
        };
    }
    if scope == "launcher-root-readonly" {
        require_permission(&store, &plugin_id, "files:launcher-root-read")?;
        return match action {
            "list" => Ok(json!({ "files": store.list_launcher_files(request.get("directory").and_then(Value::as_str).unwrap_or_default())? })),
            "read" => Ok(json!({ "content": store.read_launcher_file(path)? })),
            _ => Err("launcher-root-readonly поддерживает только list/read".to_owned()),
        };
    }
    Err("Область файлов плагина должна быть plugin, instance или launcher-root-readonly".to_owned())
}

// ------------------------------------------------------------ база ----

/// разрешает один SQL-оператор; режим чтения или записи определяется первым словом
fn safe_plugin_sql(sql: &str, read_only: bool) -> Result<String, String> {
    let value = sql.trim();
    if value.is_empty() || value.len() > 16 * 1024 || value.contains(';') || value.contains('\0') {
        return Err("SQL-запрос плагина имеет небезопасный формат".to_owned());
    }
    let lowered = value.to_lowercase();
    for token in ["attach", "detach", "load_extension", "pragma", "vacuum"] {
        if lowered.contains(token) {
            return Err("SQL-запрос плагина содержит запрещённую операцию".to_owned());
        }
    }
    let first_word = lowered.split_whitespace().next().unwrap_or_default();
    if read_only && !matches!(first_word, "select" | "explain") {
        return Err("Разрешены только read-only SQL-запросы".to_owned());
    }
    if !read_only && !matches!(first_word, "create" | "insert" | "update" | "delete" | "replace" | "alter" | "drop") {
        return Err("SQL-запрос плагина не поддерживается".to_owned());
    }
    Ok(value.to_owned())
}

fn open_plugin_database(store: &PluginStore, plugin_id: &str, read_only: bool) -> Result<Connection, String> {
    store.database(plugin_id, read_only)
}

pub fn plugin_database_query(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "storage:database")?;
    let connection = open_plugin_database(&store, &plugin_id, true)?;
    query_plugin_database(&connection, request, true)
}

pub fn plugin_database_execute(request: &Value) -> Result<Value, String> {
    let store = plugin_store()?;
    let plugin_id = plugin_id_of(request);
    require_permission(&store, &plugin_id, "storage:database")?;
    let connection = open_plugin_database(&store, &plugin_id, false)?;
    query_plugin_database(&connection, request, false)
}

fn query_plugin_database(connection: &Connection, request: &Value, read_only: bool) -> Result<Value, String> {
    use rusqlite::types::Value as SqlValue;
    let sql = safe_plugin_sql(request.get("sql").and_then(Value::as_str).unwrap_or_default(), read_only)?;
    let parameters = request.get("parameters").cloned().unwrap_or_else(|| json!([]));
    let Some(items) = parameters.as_array() else {
        return Err("Параметры SQL должны быть списком".to_owned());
    };
    let params: Vec<SqlValue> = items
        .iter()
        .map(|item| match item {
            Value::Null => SqlValue::Null,
            Value::Bool(value) => SqlValue::Integer(*value as i64),
            Value::Number(number) => match number.as_i64() {
                Some(value) => SqlValue::Integer(value),
                None => SqlValue::Real(number.as_f64().unwrap_or_default()),
            },
            other => SqlValue::Text(other.as_str().unwrap_or_default().to_owned()),
        })
        .collect();
    // прерываем запрос, если он выполняется дольше двух секунд
    let deadline = Instant::now() + Duration::from_secs(2);
    connection.progress_handler(1000, Some(move || Instant::now() >= deadline));
    let result = (|| -> Result<Value, String> {
        let mut statement = connection
            .prepare(&sql)
            .map_err(|_| "SQL-запрос плагина не выполнен".to_owned())?;
        if read_only {
            let columns: Vec<String> = statement.column_names().into_iter().map(str::to_owned).collect();
            let mut rows = statement
                .query(rusqlite::params_from_iter(params.iter()))
                .map_err(|_| "SQL-запрос плагина не выполнен".to_owned())?;
            let mut result_rows: Vec<Value> = Vec::new();
            while let Some(row) = rows.next().map_err(|_| "SQL-запрос плагина не выполнен".to_owned())? {
                if result_rows.len() >= 500 {
                    break;
                }
                let mut object = Map::new();
                for (index, column) in columns.iter().enumerate() {
                    let value: SqlValue = row
                        .get(index)
                        .map_err(|_| "SQL-запрос плагина не выполнен".to_owned())?;
                    object.insert((*column).to_owned(), sqlite_value_to_json(value));
                }
                result_rows.push(Value::Object(object));
            }
            return Ok(json!({ "rows": result_rows }));
        }
        let changes = statement
            .execute(rusqlite::params_from_iter(params.iter()))
            .map_err(|_| "SQL-запрос плагина не выполнен".to_owned())?;
        Ok(json!({ "changes": changes }))
    })();
    connection.progress_handler(1000, None::<fn() -> bool>);
    result
}

fn sqlite_value_to_json(value: rusqlite::types::Value) -> Value {
    use rusqlite::types::Value as SqlValue;
    match value {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(number) => json!(number),
        SqlValue::Real(number) => json!(number),
        SqlValue::Text(text) => json!(text),
        SqlValue::Blob(bytes) => json!(bytes.iter().map(|byte| *byte as i64).collect::<Vec<i64>>()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_sql_accepts_read_only_select_and_rejects_pragma() {
        assert!(safe_plugin_sql("SELECT 1", true).is_ok());
        assert!(safe_plugin_sql("SELECT 1", true).is_ok());
        assert!(safe_plugin_sql("INSERT INTO t VALUES (1)", false).is_ok());
        assert_eq!(
            safe_plugin_sql("INSERT INTO t VALUES (1)", true).unwrap_err(),
            "Разрешены только read-only SQL-запросы"
        );
        assert_eq!(
            safe_plugin_sql("PRAGMA journal_mode=wal", false).unwrap_err(),
            "SQL-запрос плагина содержит запрещённую операцию"
        );
        assert_eq!(
            safe_plugin_sql("SELECT 1; DROP TABLE t", true).unwrap_err(),
            "SQL-запрос плагина имеет небезопасный формат"
        );
    }

    #[test]
    fn plugin_network_ip_filter_matches_policy() {
        let is_local = |value: &str| super::super::net::is_local_address(&value.parse::<IpAddr>().unwrap());
        assert!(!is_local("93.184.216.34"));
        assert!(is_local("127.0.0.1"));
        assert!(is_local("10.1.2.3"));
        assert!(is_local("192.168.1.1"));
        assert!(is_local("169.254.1.1"));
        assert!(is_local("::1"));
        assert!(is_local("fe80::1"));
        assert!(is_local("fec0::1"));
        assert!(is_local("3fff::1"));
        assert!(is_local("64:ff9b::a00:1"));
        assert!(!is_local("64:ff9b::5db8:d822"));
    }

    #[test]
    fn plugin_runtime_can_only_revoke_existing_permissions() {
        let granted = vec!["ui:compose".to_owned(), "network:https".to_owned()];
        assert!(validate_plugin_permission_reduction(&granted, &json!([])).is_ok());
        assert!(validate_plugin_permission_reduction(&granted, &json!(["ui:compose"])).is_ok());
        assert!(validate_plugin_permission_reduction(&granted, &json!(["files:instance"])).is_err());
        assert!(validate_plugin_permission_reduction(&granted, &json!(["ui:compose", 1])).is_err());
    }

    #[test]
    fn plugin_http_requires_https_and_a_direct_connection() {
        assert!(require_plugin_https("https").is_ok());
        assert!(require_plugin_https("http").is_err());
        assert!(require_plugin_direct_network("").is_ok());
        assert!(require_plugin_direct_network("http://127.0.0.1:8080").is_err());
    }

    #[test]
    fn proxy_route_normalizes_entries() {
        let entry = proxy_route_entry(
            Some(&json!({
                "priority": 10,
                "match": { "hostSuffix": ["example.com"], "protocol": ["TCP"] },
                "action": { "type": "direct" },
            })),
            "route-1",
        )
        .unwrap();
        assert_eq!(entry["id"], json!("route-1"));
        assert_eq!(entry["match"]["protocol"], json!(["tcp"]));
        assert_eq!(entry["action"]["type"], json!("DIRECT"));
        assert!(proxy_route_entry(Some(&json!({ "action": { "type": "weird" } })), "route-2").is_err());
    }

    #[test]
    fn server_entries_validate_and_normalize() {
        let entry = server_entry(
            Some(&json!({ "id": "srv-1", "name": "Мой сервер", "address": "example.com", "port": 25565, "tags": [" survival ", ""] })),
            "server-1",
        )
        .unwrap();
        assert_eq!(entry["tags"], json!(["survival"]));
        assert!(server_entry(Some(&json!({ "name": "x", "address": "bad host" })), "server-2").is_err());
        assert_eq!(
            server_list(Some(&json!([{ "id": "a", "name": "n", "address": "h" }, { "id": "a", "name": "n", "address": "h" }])))
                .unwrap_err(),
            "Повторяющийся id сервера: a"
        );
    }
}
