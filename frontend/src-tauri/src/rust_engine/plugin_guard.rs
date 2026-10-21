// бюджеты вызовов/данных на стороне хоста и красный (без значений) аудит операций.
use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

pub(crate) const MAX_CALLS: i64 = 360;
pub(crate) const MAX_BYTES_PER_MINUTE: usize = 8 * 1024 * 1024;
pub(crate) const MAX_PAYLOAD: usize = 4 * 1024 * 1024;
const MAX_AUDIT_BYTES: u64 = 4 * 1024 * 1024;

const IDENTITY: &str = r"^[A-Za-z0-9][A-Za-z0-9._-]{0,79}\z";
const LISTENER: &str = r"^listener-[0-9]{1,16}\z";

struct Windows {
    map: HashMap<(String, String), VecDeque<(Instant, usize, i32)>>,
}

pub(crate) struct PluginOperationGuard {
    lock: Mutex<Windows>,
}

impl PluginOperationGuard {
    pub(crate) fn new() -> Self {
        Self {
            lock: Mutex::new(Windows { map: HashMap::new() }),
        }
    }

    pub(crate) fn consume(
        &self,
        plugin_id: &str,
        payload: &Value,
        bucket: &str,
        counts_as_call: bool,
    ) -> Result<usize, String> {
        static IDENTITY_RE: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new(IDENTITY).expect("валидный regex идентичности"));
        if !IDENTITY_RE.is_match(plugin_id) {
            return Err("Invalid plugin identity".to_owned());
        }
        // serde_json не умеет выдавать NaN/Infinity, так что «bounded JSON» ломается
        // только на слишком глубокой вложенности.
        let data = serde_json::to_vec(payload).map_err(|_| "Plugin request is not bounded JSON".to_owned())?;
        if data.len() > MAX_PAYLOAD {
            return Err("Plugin request exceeds the payload limit".to_owned());
        }
        let now = Instant::now();
        let identity = (plugin_id.to_owned(), bucket.to_owned());
        let mut windows = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !windows.map.contains_key(&identity) && windows.map.len() >= 256 {
            // активный бюджет не выселяем — иначе вызывающий код потеряет свой лимит.
            let stale: Vec<(String, String)> = windows
                .map
                .iter()
                .filter(|(_, entries)| entries.is_empty() || now.duration_since(entries.back().unwrap().0).as_secs_f64() >= 60.0)
                .map(|(key, _)| key.clone())
                .collect();
            for key in stale {
                windows.map.remove(&key);
            }
            if windows.map.len() >= 256 {
                return Err("Plugin budget registry is full".to_owned());
            }
        }
        let values = windows.map.entry(identity).or_default();
        while let Some(front) = values.front() {
            if now.duration_since(front.0).as_secs_f64() >= 60.0 {
                values.pop_front();
            } else {
                break;
            }
        }
        let calls: i64 = values.iter().map(|item| item.2 as i64).sum();
        let bytes: usize = values.iter().map(|item| item.1).sum();
        if (counts_as_call && calls >= MAX_CALLS) || bytes + data.len() > MAX_BYTES_PER_MINUTE {
            return Err("Plugin rate/data limit exceeded; retry after the one-minute window".to_owned());
        }
        values.push_back((now, data.len(), i32::from(counts_as_call)));
        Ok(data.len())
    }

    pub(crate) fn usage(&self, plugin_id: &str, bucket: &str) -> Value {
        let now = Instant::now();
        let windows = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let empty = VecDeque::new();
        let values = windows
            .map
            .get(&(plugin_id.to_owned(), bucket.to_owned()))
            .unwrap_or(&empty);
        let active: Vec<&(Instant, usize, i32)> = values
            .iter()
            .filter(|item| now.duration_since(item.0).as_secs_f64() < 60.0)
            .collect();
        json!({
            "calls": active.iter().map(|item| item.2 as i64).sum::<i64>(),
            "bytes": active.iter().map(|item| item.1).sum::<usize>(),
            "callLimit": MAX_CALLS,
            "byteLimit": MAX_BYTES_PER_MINUTE,
            "windowSeconds": 60,
            "enforced": true,
        })
    }

    pub(crate) fn audit(
        &self,
        root: &Path,
        plugin_id: &str,
        operation: &str,
        args: &Value,
        outcome: &str,
        elapsed_ms: f64,
    ) -> Result<(), String> {
        static LISTENER_RE: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new(LISTENER).expect("валидный regex слушателя"));
        // намеренно не храним значения: URL, заголовки, тела, куки и токены
        // могут содержать учётные данные.
        let mut keys: Vec<String> = args
            .as_object()
            .map(|fields| fields.keys().map(|key| key.chars().take(80).collect()).collect())
            .unwrap_or_default();
        keys.sort();
        keys.truncate(64);

        let mut entry = Map::new();
        entry.insert(
            "at".to_owned(),
            json!(std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_secs_f64())
                .unwrap_or(0.0)),
        );
        entry.insert("plugin".to_owned(), json!(plugin_id.chars().take(64).collect::<String>()));
        entry.insert(
            "operation".to_owned(),
            json!(operation.chars().take(128).collect::<String>()),
        );
        entry.insert("argumentKeys".to_owned(), json!(keys));
        entry.insert("outcome".to_owned(), json!(outcome));
        entry.insert("elapsedMs".to_owned(), json!((elapsed_ms * 100.0).round() / 100.0));
        if let Some(event) = args.get("event").and_then(Value::as_str) {
            if super::plugins::PLUGIN_V2_EVENTS.contains(&event) || super::plugins::PLUGIN_V3_EVENTS.contains(&event) {
                entry.insert("event".to_owned(), json!(event));
            }
        }
        if let Some(listener) = args.get("listener_id").and_then(Value::as_str) {
            if LISTENER_RE.is_match(listener) {
                entry.insert("listenerId".to_owned(), json!(listener));
            }
        }
        let mut encoded = serde_json::to_string(&Value::Object(entry)).map_err(|_| "Audit entry cannot be encoded".to_owned())?;
        encoded.push('\n');
        let encoded = encoded.into_bytes();

        // весь ввод-вывод аудита — под общим замком, как в Python.
        let _windows = self.lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if fs::symlink_metadata(root).map(|meta| meta.is_symlink()).unwrap_or(false) {
            return Err("Audit directory cannot be a link".to_owned());
        }
        fs::create_dir_all(root).map_err(|_| "Audit directory is not writable".to_owned())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(root, fs::Permissions::from_mode(0o700));
        }
        let path = root.join("plugin-operations.jsonl");
        if fs::symlink_metadata(&path).map(|meta| meta.is_symlink()).unwrap_or(false) {
            return Err("Audit file cannot be a link".to_owned());
        }
        if matches!(fs::metadata(&path), Ok(metadata) if metadata.len() > MAX_AUDIT_BYTES) {
            let backup = root.join("plugin-operations.previous.jsonl");
            if fs::symlink_metadata(&backup).map(|meta| meta.is_symlink()).unwrap_or(false) {
                return Err("Audit backup cannot be a link".to_owned());
            }
            fs::rename(&path, &backup).map_err(|_| "Audit rotation failed".to_owned())?;
        }
        append_audit_line(&path, &encoded)
    }
}

fn append_audit_line(path: &Path, encoded: &[u8]) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "Could not open the audit log".to_owned())?;
        let flags = libc::O_APPEND | libc::O_WRONLY | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        let fd = unsafe { libc::open(c_path.as_ptr(), flags, 0o600) };
        if fd < 0 {
            return Err("Could not open the audit log".to_owned());
        }
        let mut written = 0usize;
        while written < encoded.len() {
            let count = unsafe { libc::write(fd, encoded[written..].as_ptr().cast(), encoded.len() - written) };
            if count <= 0 {
                unsafe { libc::close(fd) };
                return Err("Could not append to the audit log".to_owned());
            }
            written += count as usize;
        }
        let status = if unsafe { libc::fchmod(fd, 0o600) } != 0 { 1 } else { 0 };
        unsafe { libc::close(fd) };
        if status != 0 {
            return Err("Could not secure the audit log".to_owned());
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new().append(true).create(true).open(path)
            .map_err(|_| "Could not open the audit log".to_owned())?;
        file.write_all(encoded).map_err(|_| "Could not append to the audit log".to_owned())
    }
}

static GUARD: LazyLock<PluginOperationGuard> = LazyLock::new(PluginOperationGuard::new);

pub(crate) fn guard() -> &'static PluginOperationGuard {
    &GUARD
}

pub fn plugin_api_quota_usage(request: &Value) -> Result<Value, String> {
    let plugin_id = request
        .get("plugin_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let config = super::config::ConfigStore::load()?;
    let store = super::plugins::PluginStore::new(&config)?;
    store.authorize_operation(&plugin_id, "runtime:observe")?;
    let mut usage = store.quota_usage(&plugin_id)?;
    if let Some(object) = usage.as_object_mut() {
        object.insert("operations".to_owned(), guard().usage(&plugin_id, "rpc"));
    }
    Ok(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_must_match_host_pattern() {
        let guard = PluginOperationGuard::new();
        assert!(guard.consume("demo", &json!({}), "rpc", true).is_ok());
        assert_eq!(
            guard.consume("../evil", &json!({}), "rpc", true).unwrap_err(),
            "Invalid plugin identity"
        );
        assert_eq!(
            guard.consume("", &json!({}), "rpc", true).unwrap_err(),
            "Invalid plugin identity"
        );
    }

    #[test]
    fn call_budget_is_per_identity_and_window() {
        let guard = PluginOperationGuard::new();
        for _ in 0..MAX_CALLS {
            guard.consume("counter", &json!({"ok": true}), "rpc", true).unwrap();
        }
        let error = guard.consume("counter", &json!({}), "rpc", true).unwrap_err();
        assert!(error.starts_with("Plugin rate/data limit"));
        // другой плагин не делит бюджет.
        assert!(guard.consume("other", &json!({}), "rpc", true).is_ok());
        // не считаемые за вызов — только байтовый лимит.
        assert!(guard.consume("counter", &json!({}), "rpc", false).is_ok());
        let usage = guard.usage("counter", "rpc");
        assert_eq!(usage["callLimit"], json!(MAX_CALLS));
        assert_eq!(usage["windowSeconds"], json!(60));
        assert_eq!(usage["enforced"], json!(true));
    }

    #[test]
    fn byte_budget_is_shared_across_buckets() {
        let guard = PluginOperationGuard::new();
        // полезная нагрузка ~3 МиБ (меньше MAX_PAYLOAD в 4 МиБ): пара вызовов
        // исчерпывает восьмимебайтное окно.
        let payload: String = "x".repeat(3 * 1024 * 1024);
        guard.consume("heavy", &json!({ "blob": payload }), "rpc", false).unwrap();
        guard.consume("heavy", &json!({ "blob": payload }), "rpc", false).unwrap();
        let error = guard.consume("heavy", &json!({ "blob": payload }), "rpc", false).unwrap_err();
        assert!(error.starts_with("Plugin rate/data limit"));
    }

    #[test]
    fn audit_writes_redacted_entries_and_rotates() {
        let guard = PluginOperationGuard::new();
        let root = std::env::temp_dir().join(format!("nivora-plugin-guard-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        guard
            .audit(
                &root,
                "demo",
                "net.fetch",
                &json!({"url": "https://secret", "event": "launcher.ready", "listener_id": "listener-123", "token": "x"}),
                "ok",
                12.3456,
            )
            .unwrap();
        let log = fs::read_to_string(root.join("plugin-operations.jsonl")).unwrap();
        let entry: Value = serde_json::from_str(log.trim_end()).unwrap();
        assert_eq!(entry["plugin"], json!("demo"));
        assert_eq!(entry["operation"], json!("net.fetch"));
        assert_eq!(entry["argumentKeys"], json!(["event", "listener_id", "token", "url"]));
        // значения не попадают в аудит никогда.
        assert!(!log.contains("secret"));
        assert_eq!(entry["event"], json!("launcher.ready"));
        assert_eq!(entry["listenerId"], json!("listener-123"));
        assert_eq!(entry["elapsedMs"], json!(12.35));

        guard
            .audit(&root, "demo", "net.fetch", &json!({"event": "custom.event", "listener_id": "bad-listener"}), "ok", 1.0)
            .unwrap();
        let log = fs::read_to_string(root.join("plugin-operations.jsonl")).unwrap();
        assert!(!log.contains("custom.event"));
        assert!(!log.contains("bad-listener"));

        // ротация: файл больше лимита уезжает в previous.
        let path = root.join("plugin-operations.jsonl");
        fs::write(&path, vec![b'x'; (MAX_AUDIT_BYTES + 1) as usize]).unwrap();
        guard.audit(&root, "demo", "op", &json!({}), "ok", 0.5).unwrap();
        assert!(root.join("plugin-operations.previous.jsonl").exists());
        let rotated = fs::read_to_string(root.join("plugin-operations.jsonl")).unwrap();
        assert!(rotated.ends_with("}\n"));
        let _ = fs::remove_dir_all(&root);
    }
}
