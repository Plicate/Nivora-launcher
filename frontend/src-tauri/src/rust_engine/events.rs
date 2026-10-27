use serde_json::{json, Value};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Instant;

pub type EventSink = Arc<dyn Fn(&Value) + Send + Sync>;

static SINK: OnceLock<RwLock<Option<EventSink>>> = OnceLock::new();

fn slot() -> &'static RwLock<Option<EventSink>> {
    SINK.get_or_init(|| RwLock::new(None))
}

/// регистрирует получателя событий (вызывается один раз из setup Tauri)
pub fn set_event_sink(sink: EventSink) {
    let mut guard = slot().write().unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(sink);
}

pub fn emit_event(value: &Value) {
    let guard = slot().read().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(sink) = guard.as_ref() {
        sink(value);
    }
}

pub fn emit_status(message: &str) {
    emit_event(&json!({ "kind": "status", "message": message }));
}

pub fn emit_status_event(event: &str, message: &str) {
    emit_event(&json!({ "kind": "status", "event": event, "message": message }));
}

/// статус-событие с доп. полями (session_id/percent у аудита модов)
pub fn status_event_with_extra(event: &str, message: &str, extra: Value) -> Value {
    let mut payload = json!({ "kind": "status", "event": event, "message": message });
    if let (Some(target), Some(fields)) = (payload.as_object_mut(), extra.as_object()) {
        for (key, value) in fields {
            target.insert(key.clone(), value.clone());
        }
    }
    payload
}

pub fn emit_status_event_extra(event: &str, message: &str, extra: Value) {
    emit_event(&status_event_with_extra(event, message, extra));
}

/// события запуска должны быть адресованы экземпляру: UI использует это имя,
/// чтобы закрыть именно его состояние и не тронуть соседние профили
pub fn emit_instance_status_event(event: &str, message: &str, instance_name: &str) {
    emit_status_event_extra(event, message, json!({ "instance_name": instance_name }));
}

/// формирует событие журнала запуска с ключом для устранения повторной доставки
pub fn launch_log_event(
    line: &str,
    instance_name: &str,
    launch_id: &str,
    log_id: &str,
    sequence: u64,
) -> Value {
    json!({
        "kind": "log",
        "line": line,
        "instance_name": instance_name,
        "launch_id": launch_id,
        "log_id": log_id,
        "sequence": sequence,
    })
}

pub fn emit_launch_log(
    line: &str,
    instance_name: &str,
    launch_id: &str,
    log_id: &str,
    sequence: u64,
) {
    emit_event(&launch_log_event(line, instance_name, launch_id, log_id, sequence));
}

pub fn emit_download(event: &str, url: &str, destination: &str, current: u64, total: Option<u64>) {
    emit_event(&json!({
        "kind": "download",
        "event": event,
        "url": url,
        "destination": destination,
        "current": current,
        "total": total,
    }));
}

pub struct DownloadThrottle {
    last: RwLock<Option<Instant>>,
    interval: std::time::Duration,
}

impl DownloadThrottle {
    pub fn per_second(events_per_second: u32) -> Self {
        Self {
            last: RwLock::new(None),
            interval: std::time::Duration::from_millis(1000 / events_per_second.max(1) as u64),
        }
    }

    pub fn allow(&self, force: bool) -> bool {
        if force {
            return true;
        }
        let mut guard = self.last.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        match *guard {
            Some(previous) if now.duration_since(previous) < self.interval => false,
            _ => {
                *guard = Some(now);
                true
            }
        }
    }
}

pub fn download_observer() -> super::net::DownloadObserver {
    let throttle = Arc::new(DownloadThrottle::per_second(8));
    Arc::new(move |event: &str, url: &str, destination: &std::path::Path, current: u64, total: Option<u64>| {
        if !throttle.allow(event != "progress") {
            return;
        }
        let name = destination
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        emit_download(event, url, &name, current, total);
    })
}

pub fn request_observer() -> super::net::RequestObserver {
    Arc::new(|event: &str, url: &str, attempt: u32| {
        let (host, endpoint) = url_host_endpoint(url);
        let message = match event {
            "retry" => format!("Сеть: повторяю {host}{endpoint} (попытка {})", attempt + 1),
            "mirror" => format!("Сеть: загружаю через зеркало {host}"),
            "download" => format!("Загрузка: {host}{endpoint} (попытка {attempt})"),
            _ => format!("Сеть: подключаюсь к {host}{endpoint} (попытка {attempt})"),
        };
        emit_status(&message);
    })
}

fn url_host_endpoint(url: &str) -> (String, String) {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, ""),
    };
    let host = authority.split('@').next_back().unwrap_or(authority).to_owned();
    let endpoint = path.rsplit('/').next().filter(|value| !value.is_empty()).unwrap_or("/");
    (host, endpoint.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_passes_first_event_and_limits_the_rest() {
        let throttle = DownloadThrottle::per_second(10);
        assert!(throttle.allow(false));
        assert!(!throttle.allow(false));
        assert!(throttle.allow(true), "форсированное событие проходит всегда");
    }

    #[test]
    fn instance_status_payload_is_scoped_to_its_instance() {
        let payload = status_event_with_extra(
            "operation-finished",
            "Игра завершила работу.",
            json!({ "instance_name": "Survival" }),
        );
        assert_eq!(payload["kind"], "status");
        assert_eq!(payload["event"], "operation-finished");
        assert_eq!(payload["instance_name"], "Survival");
    }

    #[test]
    fn launch_log_events_identify_a_run_and_preserve_repeated_lines() {
        let first = launch_log_event("same output", "Survival", "launch-a", "log-a.log", 1);
        let second = launch_log_event("same output", "Survival", "launch-a", "log-a.log", 2);
        let next_run = launch_log_event("same output", "Survival", "launch-b", "log-b.log", 1);

        assert_eq!(first["kind"], "log");
        assert_eq!(first["instance_name"], "Survival");
        assert_eq!(first["launch_id"], "launch-a");
        assert_eq!(first["log_id"], "log-a.log");
        assert_eq!(first["sequence"], 1);
        assert_eq!(second["line"], first["line"]);
        assert_ne!(second["sequence"], first["sequence"]);
        assert_eq!(next_run["sequence"], 1);
        assert_ne!(next_run["launch_id"], first["launch_id"]);
    }

    #[test]
    fn sink_is_optional() {
        // без зарегистрированного обработчика события игнорируются
        emit_status("тест");
        emit_download("progress", "https://example.invalid", "file.jar", 1, Some(2));
    }
}
