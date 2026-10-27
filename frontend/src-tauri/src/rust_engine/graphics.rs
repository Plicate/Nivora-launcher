//! проверяет графические утилиты в окружении запуска игры
use serde_json::{json, Map, Value};
use std::io::Read;
use std::process::Stdio;
use std::time::{Duration, Instant};

use super::config::ConfigStore;
use super::launcher::GameLauncher;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const OUTPUT_LIMIT: usize = 4000;

fn drain<R: Read>(mut pipe: R) -> String {
    let mut text = String::new();
    let mut buffer = [0u8; 4096];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => text.push_str(&String::from_utf8_lossy(&buffer[..count])),
        }
    }
    text
}

/// проба внешней утилиты: доступность, код выхода, последние OUTPUT_LIMIT
/// символов объединённого вывода (stdout + "\\n" + stderr)
fn probe(executable: &str, args: &[&str], environment: &std::collections::BTreeMap<String, String>) -> Value {
    let Some(path) = super::runtime::which(executable) else {
        return json!({ "available": false, "output": "Не найдено" });
    };
    let mut command = super::platform::command(&path);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in environment {
        command.env(key, value);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return json!({ "available": true, "output": error.to_string() }),
    };
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_handle = std::thread::spawn(move || stdout_pipe.map(drain).unwrap_or_default());
    let stderr_handle = std::thread::spawn(move || stderr_pipe.map(drain).unwrap_or_default());
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if started.elapsed() >= PROBE_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => break None,
        }
    };
    let Some(status) = status else {
        // после таймаута возвращаем ошибку сразу: потомки процесса могут удерживать каналы открытыми
        drop(stdout_handle);
        drop(stderr_handle);
        return json!({ "available": true, "output": format!("Команда не завершилась за {} секунд", PROBE_TIMEOUT.as_secs()) });
    };
    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();
    let mut output = format!("{stdout}\n{stderr}");
    output = output.trim().to_owned();
    if output.len() > OUTPUT_LIMIT {
        let split = output.len() - OUTPUT_LIMIT;
        output = output[split..].to_owned();
    }
    json!({ "available": true, "exit_code": status.code(), "output": output })
}

/// graphics_diagnostics: режим графики, сессия и пробы драйвера
pub fn graphics_diagnostics(_request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let http = super::net::client_from_settings(config.settings(), "")?;
    let configured_mode = config.str_setting("graphics_mode");
    let configured_mode = if configured_mode.is_empty() { "auto".to_owned() } else { configured_mode };
    let launcher = GameLauncher::new(config, http, None)?;
    let environment = launcher.launch_environment(&json!({}), None);
    let mut exposed = Map::new();
    for key in [
        "TRIAL_LAUNCHER_GRAPHICS_MODE",
        "__GLX_VENDOR_LIBRARY_NAME",
        "__NV_PRIME_RENDER_OFFLOAD",
        "__VK_LAYER_NV_optimus",
        "__GL_THREADED_OPTIMIZATIONS",
        "LIBGL_ALWAYS_SOFTWARE",
        "GALLIUM_DRIVER",
    ] {
        exposed.insert(key.to_owned(), json!(environment.get(key).map(String::as_str).unwrap_or("")));
    }
    Ok(json!({
        "mode": configured_mode,
        "effective_mode": environment.get("TRIAL_LAUNCHER_GRAPHICS_MODE").map(String::as_str).unwrap_or("auto"),
        "session": std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_owned()),
        "display": std::env::var("WAYLAND_DISPLAY")
            .or_else(|_| std::env::var("DISPLAY"))
            .unwrap_or_else(|_| "none".to_owned()),
        "environment": Value::Object(exposed),
        "glxinfo": probe("glxinfo", &["-B"], &environment),
        "nvidia_smi": probe("nvidia-smi", &["--query-gpu=name,driver_version", "--format=csv,noheader"], &environment),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_binary_reports_not_found() {
        let empty = std::collections::BTreeMap::new();
        let value = probe("definitely-not-a-real-binary-42", &["-B"], &empty);
        assert_eq!(value["available"], json!(false));
        assert_eq!(value["output"], json!("Не найдено"));
    }

    #[test]
    fn probe_captures_output_and_exit_code() {
        let empty = std::collections::BTreeMap::new();
        let value = probe("sh", &["-c", "echo first; echo second >&2; exit 3"], &empty);
        assert_eq!(value["available"], json!(true));
        assert_eq!(value["exit_code"], json!(3));
        let output = value["output"].as_str().unwrap();
        assert!(output.contains("first"), "{output}");
        assert!(output.contains("second"), "{output}");
    }

    #[test]
    fn probe_timeout_kills_runaway_process() {
        let empty = std::collections::BTreeMap::new();
        let started = Instant::now();
        let value = probe("sh", &["-c", "sleep 30"], &empty);
        assert!(started.elapsed() < Duration::from_secs(15), "таймаут сработал");
        assert_eq!(value["available"], json!(true));
        assert!(value.get("exit_code").is_none());
    }
}
