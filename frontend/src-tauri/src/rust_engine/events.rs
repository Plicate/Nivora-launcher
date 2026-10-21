// мост событий движка в UI. Нативные операции (install, bedrock-настройка,
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Instant;

pub type EventSink = Arc<dyn Fn(&Value) + Send + Sync>;

static SINK: OnceLock<RwLock<Option<EventSink>>> = OnceLock::new();

fn slot() -> &'static RwLock<Option<EventSink>> {
    SINK.get_or_init(|| RwLock::new(None))
}

/// регистрирует получателя событий (вызывается один раз из setup Tauri).
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
    fn sink_is_optional() {
        // без зарегистрированного sink события просто исчезают — не паникуют.
        emit_status("тест");
        emit_download("progress", "https://example.invalid", "file.jar", 1, Some(2));
    }
}
