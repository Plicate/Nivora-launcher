use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use super::events;

const ENCODER_NVENC: &str = "h264_nvenc";
const ENCODER_VAAPI: &str = "h264_vaapi";
const ENCODER_X264: &str = "libx264";
const DEFAULT_VAAPI_DEVICE: &str = "/dev/dri/renderD128";

pub(crate) const RECORDING_STATE_FILE: &str = "recording-state.json";

/// один процесс записи на приложение
struct Recording {
    child: Child,
    file: String,
    encoder: String,
    started_at: f64,
    instance: String,
    state_file: PathBuf,
}

static RECORDER: Mutex<Option<Recording>> = Mutex::new(None);
static ENCODERS_CACHE: Mutex<Option<Vec<String>>> = Mutex::new(None);

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

/// собирает аргументы ffmpeg для записи экрана
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_recording_args(
    display: &str,
    fps: i64,
    quality: i64,
    encoder: &str,
    container: &str,
    audio: bool,
    output: &str,
    vaapi_device: &str,
) -> Vec<String> {
    let fps = fps.clamp(15, 120);
    let quality = quality.clamp(15, 35);
    // запись останавливается буквой `q` в stdin ffmpeg
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "warning".into()];
    if encoder == "vaapi" {
        args.extend(["-vaapi_device".into(), vaapi_device.into()]);
    }
    args.extend([
        "-f".into(),
        "x11grab".into(),
        "-draw_mouse".into(),
        "1".into(),
        "-framerate".into(),
        fps.to_string(),
        "-i".into(),
        display.into(),
    ]);
    if audio {
        args.extend([
            "-f".into(),
            "pulse".into(),
            "-fragment_size".into(),
            "1024".into(),
            "-i".into(),
            "default".into(),
        ]);
    }
    if encoder == "nvenc" {
        args.extend([
            "-c:v".into(),
            ENCODER_NVENC.into(),
            "-preset".into(),
            "p4".into(),
            "-tune".into(),
            "ll".into(),
            "-rc".into(),
            "vbr".into(),
            "-cq".into(),
            quality.to_string(),
            "-b:v".into(),
            "0".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
        ]);
    } else if encoder == "vaapi" {
        args.extend([
            "-vf".into(),
            "format=nv12,hwupload".into(),
            "-c:v".into(),
            ENCODER_VAAPI.into(),
            "-bf".into(),
            "0".into(),
            "-qp".into(),
            quality.to_string(),
        ]);
    } else {
        args.extend([
            "-c:v".into(),
            ENCODER_X264.into(),
            "-preset".into(),
            "veryfast".into(),
            "-crf".into(),
            quality.to_string(),
            "-pix_fmt".into(),
            "yuv420p".into(),
        ]);
    }
    if audio {
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "160k".into()]);
    } else {
        args.push("-an".into());
    }
    if container == "mkv" {
        args.extend(["-f".into(), "matroska".into()]);
    } else {
        args.extend([
            "-movflags".into(),
            "+faststart".into(),
            "-f".into(),
            "mp4".into(),
        ]);
    }
    args.push(output.into());
    args
}

pub(crate) fn detect_encoders(ffmpeg: &Path) -> Vec<String> {
    let known = [ENCODER_NVENC, ENCODER_VAAPI, ENCODER_X264];
    let output = super::platform::command(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    known
        .into_iter()
        .filter(|name| text.contains(name))
        .map(str::to_owned)
        .collect()
}

fn vaapi_available() -> bool {
    Path::new(DEFAULT_VAAPI_DEVICE).exists()
}

/// выбирает аппаратный кодировщик или `x264`, если он недоступен
pub(crate) fn pick_encoder(requested: &str, encoders: &[String]) -> &'static str {
    let has = |name: &str| encoders.iter().any(|item| item == name);
    match requested {
        "nvenc" => {
            if has(ENCODER_NVENC) {
                "nvenc"
            } else {
                "x264"
            }
        }
        "vaapi" => {
            if has(ENCODER_VAAPI) && vaapi_available() {
                "vaapi"
            } else {
                "x264"
            }
        }
        "x264" => "x264",
        _ => {
            if has(ENCODER_NVENC) {
                "nvenc"
            } else if has(ENCODER_VAAPI) && vaapi_available() {
                "vaapi"
            } else {
                "x264"
            }
        }
    }
}

fn encoder_name(kind: &str) -> &'static str {
    match kind {
        "nvenc" => ENCODER_NVENC,
        "vaapi" => ENCODER_VAAPI,
        _ => ENCODER_X264,
    }
}

/// возвращает местное время в формате YYYY-MM-DD_HH-MM-SS
fn local_timestamp() -> String {
    #[cfg(unix)]
    {
        unsafe {
            let now: libc::time_t = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&now, &mut tm).is_null() {
                utc_timestamp()
            } else {
                format!(
                    "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
                    tm.tm_year + 1900,
                    tm.tm_mon + 1,
                    tm.tm_mday,
                    tm.tm_hour,
                    tm.tm_min,
                    tm.tm_sec
                )
            }
        }
    }
    #[cfg(not(unix))]
    {
        // на Windows этот участок не выполняется: запись требует X11 (проверка DISPLAY выше)
        utc_timestamp()
    }
}

fn utc_timestamp() -> String {
    let now = unix_now() as i64;
    let days = now.div_euclid(86_400);
    let seconds = now.rem_euclid(86_400);
    // гражданский алгоритм Говарда Хиннанта: дни → год-месяц-день
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        y,
        m,
        d,
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

/// атомарно записать файл состояния записи (см; recording::state_file)
pub(crate) fn write_recording_state(output_dir: &Path, payload: &Value) -> Result<PathBuf, String> {
    fs::create_dir_all(output_dir)
        .map_err(|error| format!("Не удалось создать каталог записей: {error}"))?;
    let state_path = output_dir.join(RECORDING_STATE_FILE);
    let temporary = output_dir.join(format!(
        ".{RECORDING_STATE_FILE}.{}.part",
        std::process::id()
    ));
    {
        let mut handle = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|error| format!("Не удалось записать состояние записи: {error}"))?;
        let body = serde_json::to_string(payload)
            .map_err(|error| format!("Не удалось сериализовать состояние записи: {error}"))?;
        handle
            .write_all(body.as_bytes())
            .map_err(|error| format!("Не удалось записать состояние записи: {error}"))?;
    }
    fs::rename(&temporary, &state_path)
        .map_err(|error| format!("Не удалось зафиксировать состояние записи: {error}"))?;
    Ok(state_path)
}

pub(crate) fn remove_recording_state(state_path: &Path) {
    let _ = fs::remove_file(state_path);
}

fn recorder_status_locked(recording: Option<&mut Recording>) -> Value {
    match recording {
        Some(recording) => {
            let active = recording
                .child
                .try_wait()
                .map(|status| status.is_none())
                .unwrap_or(false);
            let file_size = if recording.file.is_empty() {
                0
            } else {
                fs::metadata(&recording.file)
                    .map(|meta| meta.len())
                    .unwrap_or(0)
            };
            json!({
                "active": active,
                "file": recording.file,
                "size_bytes": file_size,
                "started_at": recording.started_at,
                "elapsed_seconds": if active { (unix_now() - recording.started_at).max(0.0) as i64 } else { 0 },
                "encoder": recording.encoder,
                "instance": recording.instance,
            })
        }
        None => json!({
            "active": false,
            "file": "",
            "size_bytes": 0,
            "started_at": 0.0,
            "elapsed_seconds": 0,
            "encoder": "",
            "instance": "",
        }),
    }
}

pub(crate) fn recording_status(_request: &Value) -> Result<Value, String> {
    let mut guard = RECORDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Ok(recorder_status_locked(guard.as_mut()))
}

pub(crate) fn recording_start(request: &Value) -> Result<Value, String> {
    let store = super::config::ConfigStore::load()?;
    let instance_name = request
        .get("instance_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    let mut guard = RECORDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let already_running = guard
        .as_mut()
        .map(|recording| {
            recording
                .child
                .try_wait()
                .map(|status| status.is_none())
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if already_running {
        return Err("Запись уже идёт — сначала остановите текущую".to_owned());
    }
    let ffmpeg = super::turbo::find_in_path("ffmpeg")
        .ok_or_else(|| "ffmpeg не найден. Установите ffmpeg, чтобы записывать игры".to_owned())?;
    let display = std::env::var("DISPLAY").unwrap_or_default();
    if display.is_empty() {
        return Err(
            "Запись доступна в X11/XWayland-сессии: переменная DISPLAY не задана".to_owned(),
        );
    }

    let overlay = store.object_setting("overlay_settings");
    let value_i64 =
        |key: &str, fallback: i64| overlay.get(key).and_then(Value::as_i64).unwrap_or(fallback);
    let value_str = |key: &str, fallback: &str| -> String {
        overlay
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or(fallback)
            .to_owned()
    };
    let value_bool = |key: &str, fallback: bool| {
        overlay
            .get(key)
            .and_then(Value::as_bool)
            .unwrap_or(fallback)
    };

    let mut cache = ENCODERS_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cache.is_none() {
        *cache = Some(detect_encoders(&ffmpeg));
    }
    let encoders = cache.clone().unwrap_or_default();
    let encoder_kind = pick_encoder(&value_str("recording_encoder", "auto"), &encoders);
    let encoder = encoder_name(encoder_kind);
    let container = value_str("recording_container", "mp4");
    let safe_instance: String = instance_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || "-_ ".contains(c) {
                c
            } else {
                '-'
            }
        })
        .take(48)
        .collect::<String>()
        .trim()
        .to_owned();
    let safe_instance = if safe_instance.is_empty() {
        "game".to_owned()
    } else {
        safe_instance
    };
    let output_dir = store.game_dir()?.join("recordings");
    fs::create_dir_all(&output_dir)
        .map_err(|error| format!("Не удалось создать каталог записей: {error}"))?;
    let output = output_dir
        .join(format!(
            "{} {}.{container}",
            local_timestamp(),
            safe_instance
        ))
        .to_string_lossy()
        .into_owned();
    let log_path = output_dir.join("last-recording.log");

    let args = build_recording_args(
        &display,
        value_i64("recording_fps", 60),
        value_i64("recording_quality", 23),
        encoder_kind,
        &container,
        value_bool("recording_audio", true),
        &output,
        DEFAULT_VAAPI_DEVICE,
    );
    let log_handle = File::create(&log_path)
        .map_err(|error| format!("Не удалось открыть журнал ffmpeg: {error}"))?;
    let mut command = super::platform::command(&ffmpeg);
    command
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_handle));
    // бэкенд работает без консоли (--noconsole), поэтому ffmpeg без этого
    // флага получил бы собственное окно консоли
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // flag CREATE_NO_WINDOW
    }
    let child = command
        .spawn()
        .map_err(|error| format!("Не удалось запустить ffmpeg: {error}"))?;

    let pid = child.id() as i64;
    let started_at = unix_now();
    let mut recording = Recording {
        child,
        file: output.clone(),
        encoder: encoder.to_owned(),
        started_at,
        instance: instance_name.clone(),
        state_file: output_dir.join(RECORDING_STATE_FILE),
    };
    // ошибка записи состояния не должна прерывать уже запущенную запись
    let state_file = write_recording_state(
        &output_dir,
        &json!({
            "pid": pid,
            "file": output,
            "started_at": started_at,
            "encoder": encoder,
            "instance": instance_name,
        }),
    )
    .unwrap_or_else(|_| recording.state_file.clone());
    let summary = recorder_status_locked(Some(&mut recording));
    *guard = Some(Recording {
        state_file,
        ..recording
    });
    drop(guard);
    drop(cache);
    events::emit_status_event("recording-changed", "Запись игры начата");
    Ok(summary)
}

pub(crate) fn recording_stop(_request: &Value) -> Result<Value, String> {
    let mut guard = RECORDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let running: Option<&mut Recording> = match guard.as_mut() {
        Some(recording) => {
            let alive = recording
                .child
                .try_wait()
                .map(|status| status.is_none())
                .unwrap_or(false);
            if alive {
                Some(recording)
            } else {
                None
            }
        }
        None => None,
    };
    if let Some(recording) = running {
        // буква "q" позволяет ffmpeg завершить файл корректно
        if let Some(stdin) = recording.child.stdin.as_mut() {
            let _ = stdin.write_all(b"q");
            let _ = stdin.flush();
        }
        if !wait_with_timeout(&mut recording.child, std::time::Duration::from_secs(10)) {
            terminate_process(&mut recording.child);
            if !wait_with_timeout(&mut recording.child, std::time::Duration::from_secs(5)) {
                let _ = recording.child.kill();
                let _ = recording.child.wait();
            }
        }
    }
    let mut summary = recorder_status_locked(guard.as_mut());
    summary["active"] = json!(false);
    if let Some(recording) = guard.take() {
        remove_recording_state(&recording.state_file);
    }
    drop(guard);
    events::emit_status_event("recording-changed", "Запись игры остановлена");
    Ok(summary)
}

fn wait_with_timeout(child: &mut Child, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => return true,
        }
    }
}

fn terminate_process(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        // посылаем SIGTERM, чтобы дать процессу завершиться корректно
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_x264_mp4_without_audio() {
        let args = build_recording_args(
            ":0",
            60,
            23,
            "x264",
            "mp4",
            false,
            "out.mp4",
            DEFAULT_VAAPI_DEVICE,
        );
        assert_eq!(
            args,
            vec![
                "-hide_banner",
                "-loglevel",
                "warning",
                "-f",
                "x11grab",
                "-draw_mouse",
                "1",
                "-framerate",
                "60",
                "-i",
                ":0",
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "23",
                "-pix_fmt",
                "yuv420p",
                "-an",
                "-movflags",
                "+faststart",
                "-f",
                "mp4",
                "out.mp4",
            ]
        );
    }

    #[test]
    fn args_nvenc_mkv_with_audio_and_clamped_values() {
        let args = build_recording_args(
            "%D",
            500,
            3,
            "nvenc",
            "mkv",
            true,
            "o.mkv",
            DEFAULT_VAAPI_DEVICE,
        );
        assert!(args.windows(2).any(|pair| pair == ["-framerate", "120"]));
        assert!(args.windows(2).any(|pair| pair == ["-cq", "15"]));
        assert!(args.contains(&"-c:a".to_owned()));
        assert!(args.contains(&"matroska".to_owned()));
        assert!(!args.contains(&"-movflags".to_owned()));
    }

    #[test]
    fn args_vaapi_prepends_device() {
        let args = build_recording_args(
            ":1",
            30,
            28,
            "vaapi",
            "mp4",
            false,
            "o.mp4",
            "/dev/dri/renderD1",
        );
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-vaapi_device", "/dev/dri/renderD1"]));
        assert!(args.windows(2).any(|pair| pair == ["-qp", "28"]));
        assert!(args.windows(2).any(|pair| pair == ["-c:v", "h264_vaapi"]));
    }

    #[test]
    fn pick_encoder_falls_back_to_x264() {
        let none: Vec<String> = Vec::new();
        assert_eq!(pick_encoder("auto", &none), "x264");
        assert_eq!(pick_encoder("nvenc", &none), "x264");
        let nvenc = vec![ENCODER_NVENC.to_owned()];
        assert_eq!(pick_encoder("auto", &nvenc), "nvenc");
        let vaapi = vec![ENCODER_VAAPI.to_owned()];
        assert_eq!(
            pick_encoder("vaapi", &vaapi),
            if vaapi_available() { "vaapi" } else { "x264" }
        );
        assert_eq!(pick_encoder("x264", &nvenc), "x264");
    }

    #[test]
    fn timestamp_shape_is_sortable() {
        let stamp = local_timestamp();
        let bytes = stamp.as_bytes();
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[7..8], "-");
        assert_eq!(&stamp[10..11], "_");
        assert_eq!(stamp.len(), 19, "сам штамп: {stamp}");
        assert!(bytes.iter().take(4).all(|b| b.is_ascii_digit()));
    }

    #[test]
    fn state_file_roundtrip_and_remove() {
        let dir =
            std::env::temp_dir().join(format!("nivora-recorder-state-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let payload = json!({ "pid": 42, "file": "/tmp/a.mp4", "started_at": 1.5, "encoder": "libx264", "instance": "A" });
        let path = write_recording_state(&dir, &payload).expect("записать состояние");
        assert!(path.is_file());
        let loaded: serde_json::Map<String, Value> =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded["pid"], json!(42));
        assert_eq!(loaded["started_at"], json!(1.5));
        remove_recording_state(&path);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    /// проверяет полный цикл записи через движок с подменным ffmpeg
    #[cfg(unix)] // на Unix ffmpeg подменяется shell-скриптом
    #[test]
    fn recording_lifecycle_through_engine_handle() {
        let root =
            std::env::temp_dir().join(format!("nivora-recorder-cycle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let bin = root.join("bin");
        let config = root.join("config");
        let data = root.join("data");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(&data).unwrap();
        let fake = bin.join("ffmpeg");
        fs::write(
            &fake,
            "#!/bin/sh\ncase \"$*\" in *-encoders*) echo \" V....D libx264\"; exit 0;; esac\nread line\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let previous = (
            std::env::var_os("PATH"),
            std::env::var_os("XDG_CONFIG_HOME"),
            std::env::var_os("XDG_DATA_HOME"),
            std::env::var_os("DISPLAY"),
        );
        std::env::set_var("PATH", &bin);
        std::env::set_var("XDG_CONFIG_HOME", &config);
        std::env::set_var("XDG_DATA_HOME", &data);
        std::env::set_var("DISPLAY", ":99");
        let restore = || {
            match previous.0 {
                Some(ref value) => std::env::set_var("PATH", value),
                None => std::env::remove_var("PATH"),
            }
            match previous.1 {
                Some(ref value) => std::env::set_var("XDG_CONFIG_HOME", value),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            match previous.2 {
                Some(ref value) => std::env::set_var("XDG_DATA_HOME", value),
                None => std::env::remove_var("XDG_DATA_HOME"),
            }
            match previous.3 {
                Some(ref value) => std::env::set_var("DISPLAY", value),
                None => std::env::remove_var("DISPLAY"),
            }
        };
        // сначала останавливаем прошлую запись из-за общего состояния процесса
        let _ = recording_stop(&json!({}));
        let started = recording_start(&json!({ "instance_name": "Alpha ЭкзMP" }));
        match started {
            Ok(summary) => {
                assert_eq!(
                    summary["active"],
                    json!(true),
                    "ffmpeg-фейк запущен: {summary}"
                );
                assert_eq!(summary["encoder"], json!("libx264"));
                assert!(summary["file"].as_str().unwrap_or("").ends_with(".mp4"));
                let state_path = data
                    .join("nivora-launcher")
                    .join("game")
                    .join("recordings")
                    .join(RECORDING_STATE_FILE);
                assert!(state_path.is_file(), "состояние для overlay_state на диске");
                let status = recording_status(&json!({})).expect("статус");
                assert_eq!(status["active"], json!(true));
                let stopped = recording_stop(&json!({})).expect("стоп");
                assert_eq!(stopped["active"], json!(false));
                assert!(!state_path.exists());
                let again = recording_stop(&json!({})).expect("повторный стоп");
                assert_eq!(again["active"], json!(false));
                let routed = super::super::handle("recording_status", &json!({}))
                    .expect("handle(recording_status)");
                assert_eq!(routed["ok"], json!(true));
                assert_eq!(routed["operation"], json!("recording_status"));
                assert_eq!(routed["data"]["active"], json!(false));
            }
            Err(error) => {
                restore();
                panic!("recording_start упал: {error}");
            }
        }
        restore();
        let _ = fs::remove_dir_all(&root);
    }
}
