#![recursion_limit = "512"]

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
#[cfg(desktop)]
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Есть ли живой значок в трее. Если нет, крестик закрывает приложение
/// по-настоящему — прятать окно «в никуда» без способа вернуть было бы хуже.
static TRAY_AVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// Операции GUI, обслуживаемые локально, без воркера.
mod rust_engine;
// Дымовой прогон упакованного приложения (`--smoke-test`).
mod smoke;

#[cfg(not(windows))]
fn python_path() -> Option<String> {
    if Path::new("/usr/lib/nivora-launcher").is_dir() {
        return Some("/usr/lib/nivora-launcher".to_owned());
    }

    let mut directory = std::env::current_exe().ok()?.parent().map(Path::to_path_buf)?;
    for _ in 0..8 {
        if directory.join("nivora_launcher").is_dir() {
            return Some(directory.to_string_lossy().into_owned());
        }
        if !directory.pop() {
            break;
        }
    }
    None
}

fn runtime_dir() -> Result<PathBuf, String> {
    let directory = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("nivora-launcher-runtime"));
    fs::create_dir_all(&directory).map_err(|error| format!("Не удалось создать runtime-каталог: {error}"))?;
    #[cfg(unix)]
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Не удалось ограничить runtime-каталог: {error}"))?;
    Ok(directory)
}

fn backend_log_path() -> Result<PathBuf, String> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if cfg!(windows) {
                std::env::var_os("LOCALAPPDATA")
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("USERPROFILE")
                            .map(|home| PathBuf::from(home).join("AppData/Local"))
                    })
                    .unwrap_or_else(std::env::temp_dir)
            } else {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local/share"))
                    .unwrap_or_else(std::env::temp_dir)
            }
        });
    let directory = data_home.join(rust_engine::state::LAUNCHER_DIR).join("logs");
    fs::create_dir_all(&directory).map_err(|error| format!("Не удалось создать каталог логов: {error}"))?;
    let path = directory.join("backend.log");
    #[cfg(unix)]
    if path.exists() {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Не удалось ограничить лог backend: {error}"))?;
    }
    Ok(path)
}

fn cancellation_marker(operation_id: &str) -> Result<PathBuf, String> {
    if operation_id.is_empty()
        || !operation_id
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || value == '-' || value == '_')
    {
        return Err("Некорректный идентификатор операции".to_owned());
    }
    Ok(runtime_dir()?.join(format!("nivora-launcher-{operation_id}.cancel")))
}

fn backend_executable_name() -> &'static str {
    if cfg!(windows) {
        "nivora-launcher-backend.exe"
    } else {
        "nivora-launcher-backend"
    }
}

fn backend_resource_path(resource_dir: &Path) -> PathBuf {
    resource_dir.join("backend").join(backend_executable_name())
}

fn bundled_backend_path(app: &AppHandle) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    // 1. Установленный режим: ресурс рядом с приложением (resource_dir()/backend/…).
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(backend_resource_path(&resource_dir));
    }
    // 2–3. Portable-режим: каталог самого nivora-launcher.exe. Поддерживаем оба
    //    расклада — <dir>/backend/nivora-launcher-backend.exe и просто два exe
    //    рядом (так лежат артефакты dist/windows — раньше это не работало, и
    //    приложение падало в fallback на системный python).
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            candidates.push(backend_resource_path(exe_dir));
            candidates.push(exe_dir.join(backend_executable_name()));
        }
    }
    // Пустые файлы — заглушки из репозитория исходников: реальный бэкенд
    // кладут сборочные скрипты. На такую нечестно «наткнуться» — иначе
    // запуск падает с невнятной ошибкой exec вместо запасного пути (python3).
    candidates.into_iter().find(|path| path.is_file() && fs::metadata(path).map(|meta| meta.len() > 0).unwrap_or(false))
}

// Встроенный бэкенд: zip-архив onedir-сборки PyInstaller упаковывается внутрь
// nivora-launcher.exe, поэтому портативная сборка — ОДИН файл. Архив, а не
// «голый» exe: вложенный исполняемый файл внутри другого — классический
// признак дроппера, за который эвристики антивирусов рвали экзешник на части
// (10+ срабатываний на VirusTotal). Onedir-сборка сама по себе детектируется
// на порядок реже onefile.
// На Linux (dev и Arch-пакет) архива нет и он не нужен — там работает
// системный python3, поэтому const и функция целиком выключены через cfg(windows).
// Для dev-сборки на Windows бэкенд собирается заранее: scripts/build_windows_exe.sh --only-backend.
#[cfg(all(windows, embedded_backend))]
const EMBEDDED_BACKEND_ARCHIVE: &[u8] =
    include_bytes!("../resources/backend/nivora-launcher-backend.zip");

// Распаковывает встроенный бэкенд в %LOCALAPPDATA%\nivora-launcher\backend\v<версия>\
// (писать рядом с exe нельзя: Program Files доступен только для чтения) и
// возвращает путь к exe. Пересобирается при смене версии или несовпадении
// контрольной суммы архива; каталоги прошлых версий удаляются.
#[cfg(all(windows, not(embedded_backend)))]
fn embedded_backend_exe() -> Result<PathBuf, String> {
    Err("Бэкенд не найден рядом с приложением, а эта сборка собрана без встроенного архива. Переустановите Nivora Launcher установщиком.".to_owned())
}

#[cfg(all(windows, embedded_backend))]
fn embedded_backend_exe() -> Result<PathBuf, String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher as _;

    fn content_hash(bytes: &[u8]) -> u64 {
        let mut hasher = DefaultHasher::new();
        hasher.write(bytes);
        hasher.finish()
    }

    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("TEMP").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let version_dir = base
        .join(rust_engine::state::LAUNCHER_DIR)
        .join("backend")
        .join(format!("v{}", env!("CARGO_PKG_VERSION")));
    let marker = version_dir.join(".archive-hash");
    let expected_hash = content_hash(EMBEDDED_BACKEND_ARCHIVE).to_string();
    let up_to_date = fs::read_to_string(&marker).ok().as_deref() == Some(expected_hash.as_str());
    if !up_to_date {
        if version_dir.exists() {
            fs::remove_dir_all(&version_dir)
                .map_err(|error| format!("Не удалось обновить каталог бэкенда: {error}"))?;
        }
        fs::create_dir_all(&version_dir)
            .map_err(|error| format!("Не удалось создать каталог бэкенда: {error}"))?;
        extract_backend_archive(&version_dir)
            .map_err(|error| format!("Не удалось распаковать встроенный backend: {error}"))?;
        fs::write(&marker, expected_hash)
            .map_err(|error| format!("Не удалось зафиксировать распаковку backend: {error}"))?;
    }
    let executable = version_dir.join(backend_executable_name());
    if !executable.is_file() {
        return Err("В архиве бэкенда нет nivora-launcher-backend.exe".to_owned());
    }
    // Распаковки прошлых версий больше не нужны.
    if let Some(parent) = version_dir.parent() {
        let keep = version_dir.file_name().map(|value| value.to_os_string());
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                if entry.path().is_dir() && Some(entry.file_name()) != keep {
                    let _ = fs::remove_dir_all(entry.path());
                }
            }
        }
    }
    Ok(executable)
}

#[cfg(all(windows, embedded_backend))]
fn extract_backend_archive(target: &Path) -> Result<(), String> {
    let reader = std::io::Cursor::new(EMBEDDED_BACKEND_ARCHIVE);
    let mut archive = zip::ZipArchive::new(reader).map_err(|error| error.to_string())?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if entry.is_dir() {
            continue;
        }
        // Внутри архива может быть как корень onedir-каталога, так и его
        // содержимое — приводим к плоской раскладке «рядом с exe».
        let name = entry.name().replace('\\', "/");
        let stripped = name
            .strip_prefix("nivora-launcher-backend/")
            .unwrap_or(&name)
            .trim_start_matches('/')
            .to_owned();
        if stripped.is_empty() || stripped.contains("..") {
            continue;
        }
        let destination = target.join(&stripped);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut file = fs::File::create(&destination).map_err(|error| error.to_string())?;
        std::io::copy(&mut entry, &mut file).map_err(|error| error.to_string())?;
    }
    Ok(())
}

const BACKEND_LOG_LIMIT: u64 = 5 * 1024 * 1024;
const BACKEND_LOG_KEEP: u32 = 3;

/// Ротация журнала: backend.log → backend.log.1 → … → backend.log.<keep>.
/// Без неё stderr воркера рос бесконечно (логи пишутся только в файл —
/// консоли в production-сборке нет).
fn rotate_log(path: &Path, limit: u64, keep: u32) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.len() < limit || keep == 0 {
        return;
    }
    let numbered = |index: u32| PathBuf::from(format!("{}.{index}", path.display()));
    let _ = fs::remove_file(numbered(keep));
    for index in (1..keep).rev() {
        let _ = fs::rename(numbered(index), numbered(index + 1));
    }
    let _ = fs::rename(path, numbered(1));
}

struct BackendWorker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl BackendWorker {
    fn spawn(app: &AppHandle) -> Result<Self, String> {
        let stderr_path = backend_log_path()?;
        rotate_log(&stderr_path, BACKEND_LOG_LIMIT, BACKEND_LOG_KEEP);
        let stderr_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stderr_path)
            .map_err(|error| format!("Не удалось открыть лог Python backend: {error}"))?;
        #[cfg(unix)]
        fs::set_permissions(&stderr_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("Не удалось ограничить лог Python backend: {error}"))?;
        // Порядок запуска бэкенда:
        //   Windows — 1) exe-файл рядом (ресурс установщика / portable-раскладка),
        //             2) встроенный в nivora-launcher.exe бэкенд (один файл!);
        //   Linux   — 1) exe-файл рядом, 2) системный python3 (dev и Arch-пакет).
        // Системный python на Windows не используется вовсе: на чистой системе
        // это заглушка Store («Python was not found», окно консоли).
        #[cfg(windows)]
        let mut command = if let Some(executable) = bundled_backend_path(app) {
            startup_trace(&format!("бэкенд: файл рядом с приложением: {}", executable.display()));
            Command::new(executable)
        } else {
            match embedded_backend_exe() {
                Ok(executable) => {
                    startup_trace(&format!("бэкенд: встроенный, распакован: {}", executable.display()));
                    Command::new(executable)
                }
                Err(error) => {
                    startup_trace(&format!("бэкенд: распаковка встроенного не удалась: {error}"));
                    return Err(error);
                }
            }
        };
        #[cfg(not(windows))]
        let mut command = if let Some(executable) = bundled_backend_path(app) {
            startup_trace(&format!("бэкенд: файл рядом с приложением: {}", executable.display()));
            Command::new(executable)
        } else {
            startup_trace("бэкенд: системный python3");
            let mut command = Command::new("python3");
            command.args(["-m", "nivora_launcher.gui_service"]);
            if let Some(path) = python_path() {
                command.env("PYTHONPATH", path);
            }
            command
        };
        command
            // Python на Windows по умолчанию пишет в пайпы в ANSI-кодировке
            // (cp1251) — русские строки в JSON ломают UTF-8-протокол
            // («stream did not contain valid UTF-8»). Принудительный UTF-8.
            .env("PYTHONUTF8", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr_file));
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let message = format!("Не удалось запустить Python backend: {error}");
                startup_trace(&format!("бэкенд: {message}"));
                return Err(message);
            }
        };
        startup_trace("бэкенд: процесс запущен, рукопожатие");
        let input = child
            .stdin
            .take()
            .ok_or_else(|| "Python backend не открыл stdin".to_owned())?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| "Python backend не открыл stdout".to_owned())?;
        Ok(Self {
            child,
            input,
            output: BufReader::new(output),
        })
    }

    fn request(&mut self, app: &AppHandle, request: &Value) -> Result<Value, String> {
        validate_backend_request(request)?;
        if self
            .child
            .try_wait()
            .map_err(|error| format!("Не удалось проверить Python backend: {error}"))?
            .is_some()
        {
            return Err("Python backend неожиданно завершился".to_owned());
        }
        serde_json::to_writer(&mut self.input, request)
            .map_err(|error| format!("Не удалось передать GUI-запрос: {error}"))?;
        self.input
            .write_all(b"\n")
            .and_then(|_| self.input.flush())
            .map_err(|error| format!("Не удалось завершить GUI-запрос: {error}"))?;

        let mut buffer: Vec<u8> = Vec::new();
        loop {
            buffer.clear();
            let count = self
                .output
                .read_until(b'\n', &mut buffer)
                .map_err(|error| format!("Не удалось прочитать Python backend: {error}"))?;
            if count == 0 {
                return Err("Python backend закрыл stdout".to_owned());
            }
            // Читаем байты и декодируем с потерями: один «кривой» символ
            // не должен убивать весь поток (read_line падает на не-UTF-8).
            let line = String::from_utf8_lossy(&buffer);
            let text = line.trim_end();
            let value: Value = serde_json::from_str(text)
                .map_err(|error| format!("Некорректное событие Python backend: {error}"))?;
            if let Some(event) = value.get("event") {
                app.emit("backend-event", event)
                    .map_err(|error| format!("Не удалось передать событие в UI: {error}"))?;
            } else {
                return Ok(value);
            }
        }
    }
}

impl Drop for BackendWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

static BACKEND_WORKER: OnceLock<Mutex<Option<BackendWorker>>> = OnceLock::new();

fn backend_worker() -> &'static Mutex<Option<BackendWorker>> {
    BACKEND_WORKER.get_or_init(|| Mutex::new(None))
}

#[tauri::command]
fn cancel_operation(operation_id: String) -> Result<(), String> {
    let path = cancellation_marker(&operation_id)?;
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map(|_| ())
        .map_err(|error| format!("Не удалось остановить операцию: {error}"))
}

// Журнал попыток открыть окно оверлея: каждая попытка дописывается строкой
// (режим, план окна, результат, ошибка). Читается командой той же команды —
// она возвращает путь и хвост журнала. Без этого файла «оверлей не
// открывается» диагностировался вслепую.
fn overlay_log_path() -> Option<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|home| PathBuf::from(home).join(".local/share")))?;
    Some(data_home.join(rust_engine::state::LAUNCHER_DIR).join("logs").join("overlay.log"))
}

fn append_rotating(path: &Path, line: &str, limit: u64) -> Result<(), String> {
    use std::io::Write;
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() >= limit {
            let _ = std::fs::rename(path, path.with_extension("log.old"));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("Не удалось создать каталог журнала: {error}"))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("Не удалось открыть журнал: {error}"))?;
    file.write_all(line.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|error| format!("Не удалось записать журнал: {error}"))
}

#[tauri::command]
fn overlay_attempt_log(entry: Option<String>) -> Result<Value, String> {
    let path = overlay_log_path().ok_or_else(|| "Не удалось определить каталог данных".to_owned())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let line = entry.unwrap_or_default();
    if !line.trim().is_empty() {
        let environment = format!(
            "GDK_BACKEND={} session={} display={}",
            std::env::var("GDK_BACKEND").unwrap_or_default(),
            std::env::var("XDG_SESSION_TYPE").unwrap_or_default(),
            std::env::var("DISPLAY").unwrap_or_default(),
        );
        append_rotating(&path, &format!("[{stamp}] {line} | {environment}"), 128 * 1024)?;
    }
    let tail = std::fs::read_to_string(&path)
        .map(|content| content.chars().rev().take(4096).collect::<String>().chars().rev().collect::<String>())
        .unwrap_or_default();
    Ok(serde_json::json!({ "path": path.to_string_lossy(), "tail": tail }))
}

#[tauri::command]
async fn backend_call(app: AppHandle, request: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || backend_call_sync(app, request))
        .await
        .map_err(|error| format!("Backend task failed: {error}"))?
}

fn backend_call_sync(app: AppHandle, request: Value) -> Result<Value, String> {
    // На случай операций, начатых до setup (или в тестовых сборках без
    // окна), sink обновляется на каждый вызов: AppHandle клонируется
    // дёшево, а гарантированно живой мост важнее сэкономленных наносекунд.
    {
        let handle = app.clone();
        rust_engine::events::set_event_sink(std::sync::Arc::new(move |value| {
            let _ = handle.emit("backend-event", value);
        }));
    }
    // Единая точка диспетчеризации: белый список и маршрут выбирает
    // rust_engine::registry::route. Нативные операции идут мимо воркера
    // (без spawn и очереди), воркер поднимается лениво и только для операций,
    // которых движок ещё не перенял.
    match rust_engine::registry::route(&request)? {
        rust_engine::registry::Route::Native => {
            let operation = rust_engine::registry::operation_of(&request)?;
            return rust_engine::handle(operation, &request);
        }
        rust_engine::registry::Route::BedrockLaunch => {
            return rust_engine::bedrock::launch_intercepted(&request);
        }
        rust_engine::registry::Route::TransientWorker => {
            let mut worker = BackendWorker::spawn(&app)?;
            let response = worker.request(&app, &request)?;
            return parse_backend_response(response);
        }
        rust_engine::registry::Route::Worker => {}
    }

    let worker_mutex = backend_worker();
    let mut worker_guard = worker_mutex
        .lock()
        .map_err(|_| "Python backend lock повреждён".to_owned())?;
    if worker_guard.is_none() {
        *worker_guard = Some(BackendWorker::spawn(&app)?);
    }
    let response = match worker_guard.as_mut().expect("backend worker initialized").request(&app, &request) {
        Ok(value) => value,
        Err(error) => {
            worker_guard.take();
            // Воркер умер прямо посреди запроса (краш Python, битый пайп,
            // антивирус прибил процесс) — это не приговор операции: поднимаем
            // новый процесс и повторяем запрос один раз, чтобы пользователь
            // не увидел ошибку там, где хватает перезапуска подсистемы.
            if !is_worker_death(&error) {
                return Err(error);
            }
            startup_trace(&format!("бэкенд: воркер умер ({error}) — перезапускаю и повторяю запрос"));
            let mut worker = match BackendWorker::spawn(&app) {
                Ok(worker) => worker,
                Err(spawn_error) => {
                    return Err(format!("{error}. Перезапуск не удался: {spawn_error}"));
                }
            };
            match worker.request(&app, &request) {
                Ok(value) => {
                    *worker_guard = Some(worker);
                    value
                }
                Err(retry_error) => return Err(retry_error),
            }
        }
    };
    parse_backend_response(response)
}

/// Смерть/поломка канала связи с Python-воркером — когда повтор через новый
/// процесс имеет смысл. Ошибки самой операции (валидация, allowlist) сюда
/// не входят: их повтор бессмыслен.
fn is_worker_death(message: &str) -> bool {
    message.contains("неожиданно завершился")
        || message.contains("закрыл stdout")
        || message.contains("Не удалось прочитать Python backend")
        || message.contains("Не удалось передать GUI-запрос")
        || message.contains("Некорректное событие Python backend")
}

#[cfg(test)]
fn uses_transient_backend_worker(request: &Value) -> bool {
    rust_engine::registry::uses_transient_worker(request)
}

fn parse_backend_response(response: Value) -> Result<Value, String> {
    if response.get("ok") == Some(&Value::Bool(false)) {
        return Err(response
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| "Python backend завершился с ошибкой".to_owned()));
    }

    Ok(response)
}

fn validate_backend_request(request: &Value) -> Result<(), String> {
    // Белый список = rust_engine::registry (NATIVE_OPS ∪ WORKER_OPS).
    rust_engine::registry::validate(request).map(|_| ())
}

/// Любой сбой старта обязан быть ВИДИМЫМ. У оконной подсистемы нет консоли,
/// «тихое закрытие» не даёт пользователю ничего — поэтому фатальная ошибка
/// показывается системным окном сообщения (MessageBoxW), а не только логом.
/// Вызывается уже после записи в startup.log/panic.log.
#[cfg(windows)]
fn fatal_dialog(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
    };
    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = "Nivora Launcher — ошибка запуска"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND | MB_TOPMOST,
        );
    }
}

#[cfg(not(windows))]
fn fatal_dialog(message: &str) {
    // На Linux «тихая смерть» была нормой: ошибка запуска писалась в stderr,
    // которого при запуске из .desktop нет вовсе. macOS без графических
    // helpers — то же самое. Показываем текст любым доступным способом;
    // если helpers нет — остаётся лог.
    #[cfg(target_os = "macos")]
    let script = format!(
        "display dialog \"{}\" with title \"Nivora Launcher — ошибка запуска\" buttons {{\"OK\"}} with icon stop",
        message.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
    );
    #[cfg(target_os = "macos")]
    let attempts: Vec<(&str, Vec<&str>)> = vec![("osascript", vec!["-e", script.as_str()])];
    #[cfg(not(target_os = "macos"))]
    let attempts: Vec<(&str, Vec<&str>)> = vec![
        (
            "zenity",
            vec!["--error", "--title", "Nivora Launcher — ошибка запуска", "--text", message],
        ),
        (
            "kdialog",
            vec!["--title", "Nivora Launcher — ошибка запуска", "--error", message],
        ),
        ("notify-send", vec!["-u", "critical", "Nivora Launcher", message]),
    ];
    for (program, args) in attempts {
        if Command::new(program)
            .args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
        {
            return;
        }
    }
    eprintln!("{message}");
}

/// Паники оконной сборки никто не видит: у windows-подсистемы нет консоли,
/// stderr отброшен, и падение на старте выглядит как «лаунчер просто
/// закрывается». Хук дописывает панику с местом и трассировкой рядом с
/// остальной диагностикой (Windows — %LOCALAPPDATA%, Linux — ~/.local/share),
/// чтобы краш можно было диагностировать по файлу. В release на Windows
/// паника дополнительно показывается окном — «молча» приложение больше
/// не умирает никогда.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|location| location.to_string())
            .unwrap_or_else(|| "неизвестное место".to_owned());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|value| (*value).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "паника без текста".to_owned());
        let directory = diagnostic_dir();
        let _ = fs::create_dir_all(&directory);
        let trace = std::backtrace::Backtrace::force_capture();
        let content = format!(
            "version: {}\nlocation: {location}\npanic: {payload}\nbacktrace:\n{trace}\n",
            env!("CARGO_PKG_VERSION")
        );
        let _ = fs::write(directory.join("panic.log"), content);
        if cfg!(all(windows, not(debug_assertions))) {
            use std::sync::atomic::{AtomicBool, Ordering};
            static DIALOG_SHOWN: AtomicBool = AtomicBool::new(false);
            if !DIALOG_SHOWN.swap(true, Ordering::SeqCst) {
                fatal_dialog(&format!(
                    "Внутренняя ошибка Nivora Launcher (паника):\n\n{payload}\n\nМесто: {location}\n\nПодробности: {}",
                    diagnostic_dir().join("panic.log").display()
                ));
            }
        }
    }));
}

/// Нативные краши (access violation и другие SEH-исключения) паниками
/// не являются — panic-хук их не видит, и падение остаётся «тихим».
/// Фильтр необработанных исключений дописывает код и адрес сбоя в
/// native-crash.log рядом с panic.log.
#[cfg(windows)]
fn install_native_crash_filter() {
    use windows_sys::Win32::System::Diagnostics::Debug::{SetUnhandledExceptionFilter, EXCEPTION_POINTERS};

    unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
        let record = (*info).ExceptionRecord;
        let code = (*record).ExceptionCode as u32;
        let address = (*record).ExceptionAddress as usize;
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("TEMP").map(PathBuf::from))
            .unwrap_or_else(std::env::temp_dir);
        let directory = base.join(rust_engine::state::LAUNCHER_DIR);
        let _ = fs::create_dir_all(&directory);
        let _ = fs::write(
            directory.join("native-crash.log"),
            format!(
                "version: {}\nexception_code: {code:#010x}\naddress: {address:#x}\n",
                env!("CARGO_PKG_VERSION")
            ),
        );
        // Нативный краш тоже показываем окном: иначе он выглядит как
        // мгновенное «тихое» закрытие без единого сообщения.
        use std::sync::atomic::{AtomicBool, Ordering};
        static DIALOG_SHOWN: AtomicBool = AtomicBool::new(false);
        if !DIALOG_SHOWN.swap(true, Ordering::SeqCst) {
            fatal_dialog(&format!(
                "Аварийный сбой Nivora Launcher.\n\nКод исключения: {code:#010x}\nАдрес: {address:#x}\n\nПодробности: %LOCALAPPDATA%\\nivora-launcher\\native-crash.log"
            ));
        }
        // EXCEPTION_CONTINUE_SEARCH: отчёт об ошибке продолжает Windows.
        0
    }

    unsafe {
        SetUnhandledExceptionFilter(Some(filter));
    }
}

/// Трассировка старта: каждая веха запуска дописывается в startup.log рядом
/// с данными лаунчера — при любом странном поведении файл сразу даёт картину.
fn startup_trace(message: &str) {
    let base = diagnostic_dir();
    let _ = fs::create_dir_all(&base);
    let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(base.join("startup.log"))
    else {
        return;
    };
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    let _ = writeln!(file, "[{seconds}] v{} {message}", env!("CARGO_PKG_VERSION"));
}

/// Каталог диагностики: %LOCALAPPDATA% на Windows, ~/.local/share — на Linux
/// (временный каталог затирается при перезагрузке, логи там не ищут).
fn diagnostic_dir() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join(rust_engine::state::LAUNCHER_DIR);
    }
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(data_home).join(rust_engine::state::LAUNCHER_DIR);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".local/share")
            .join(rust_engine::state::LAUNCHER_DIR);
    }
    std::env::temp_dir().join(rust_engine::state::LAUNCHER_DIR)
}

/// Путь к логу для сообщений об ошибках — платформозависимый, чтобы не
/// отправлять Linux-пользователя искать %LOCALAPPDATA%.
fn diagnostic_log_hint() -> String {
    diagnostic_dir()
        .join("startup.log")
        .display()
        .to_string()
}

pub fn run() {
    install_panic_hook();
    #[cfg(windows)]
    install_native_crash_filter();
    let executable = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "?".to_owned());
    startup_trace(&format!("запуск: exe={executable}"));
    // Ярлык экземпляра стартует лаунчер с `--launch-instance <имя>`:
    // валидное имя запоминается для UI (холодный старт) или уходит живой
    // копии через guard (вторая копия), невалидное — молча отбрасывается.
    let launch_argument = rust_engine::shortcuts::launch_argument(std::env::args().skip(1));
    if let Some(name) = &launch_argument {
        if !rust_engine::shortcuts::set_pending_launch(name) {
            startup_trace(&format!("run: --launch-instance отброшен (невалидное имя): {name}"));
        }
    }
    let app = match tauri::Builder::default()
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" || !TRAY_AVAILABLE.load(std::sync::atomic::Ordering::SeqCst) {
                    // Без трея прятать окно нельзя — его никто не вернёт.
                    return;
                }
                // Крестик НЕ завершает приложение: игрок часто закрывает
                // лаунчер «на время игры», а вместе с ним умирали глобальные
                // клавиши и оверлей. Прячем окно в трей; полный выход —
                // «Выйти» в меню значка. Для окна оверлея — то же самое.
                api.prevent_close();
                let _ = window.hide();
                let _ = window.emit("nivora-hidden-to-tray", ());
            }
        })
        .setup(move |app| {
            // Защита от второй копии — раньше всего: второй запуск будит
            // окно живого лаунчера и тихо выходит, не поднимая свой воркер.
            // Ярлык экземпляра дополнительно передаёт цель запуска.
            if let Err(reason) =
                rust_engine::instance_guard::acquire(app.handle().clone(), launch_argument.as_deref())
            {
                if reason == "already-running" {
                    startup_trace("run: уже запущено — показываю окно первой копии и выхожу");
                    std::process::exit(0);
                }
                startup_trace(&format!("run: guard недоступен ({reason}) — продолжаю без него"));
            }
            // Ошибка setup раньше уходила в никуда: окно не создавалось, и
            // приложение «просто закрывалось». Теперь — лог + видимое окно.
            if let Err(error) = setup_app(app) {
                let message = error.to_string();
                startup_trace(&format!("setup: ошибка: {message}"));
                fatal_dialog(&format!(
                    "Не удалось инициализировать приложение:\n\n{message}\n\nПодробности: {}",
                    diagnostic_log_hint()
                ));
                return Err(error);
            }
            // Дымовой прогон для CI: тот же мост, что у UI; отчёт и код
            // выхода — в diagnostic_dir()/smoke-test.json.
            if smoke::requested() {
                let handle = app.handle().clone();
                std::thread::spawn(move || smoke::run(handle));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![backend_call, cancel_operation, overlay_attempt_log])
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(error) => {
            // Без консоли паника выглядит мгновенным закрытием — показываем
            // причину диалогом и выходим с явным кодом.
            let message = error.to_string();
            startup_trace(&format!("run: ошибка: {message}"));
            fatal_dialog(&format!(
                "Не удалось запустить интерфейс:\n\n{message}\n\nПодробности: {}",
                diagnostic_log_hint()
            ));
            std::process::exit(1);
        }
    };
    app.run(|_app_handle, event| {
        // Чистый выход обязан снимать лок единственного экземпляра сразу:
        // иначе файл переживает процесс, и запуск в течение нескольких
        // секунд после выхода «молча» считал лаунчер живым и не открывал
        // окно. Протухание сердцебиения остаётся запасным путём на случай
        // жёсткого падения, когда обработать выход некому.
        if let tauri::RunEvent::Exit = event {
            rust_engine::instance_guard::release();
        }
    });
}

fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    startup_trace("setup: начало");
    // Мост прогресса нативных операций: rust_engine сообщает о загрузках и
    // стадиях установки через единый sink, который прозрачно доносит их до
    // webview как события "backend-event" — в том же формате, что и
    // Python-воркер. Без этого панель ACTIVITY при установке экземпляра
    // молчала все минуты загрузки (прогресс и логи появлялись только
    // с финальным ответом).
    {
        let handle = app.handle().clone();
        rust_engine::events::set_event_sink(std::sync::Arc::new(move |value| {
            let _ = handle.emit("backend-event", value);
        }));
    }
    // Апдейтер не должен убивать лаунчер: сервера обновлений в этой
    // сборке нет (endpoints пустые), а отсутствие/кривизна его конфига
    // раньше валила всё приложение до создания окна. UI при вызове
    // проверки и так вежливо сообщает, что функция не настроена.
    #[cfg(desktop)]
    if let Err(error) = app.handle().plugin(tauri_plugin_updater::Builder::new().build()) {
        startup_trace(&format!("setup: updater не инициализирован: {error}"));
    }
    #[cfg(desktop)]
    app.handle().plugin(tauri_plugin_process::init())?;
    // Горячие клавиши игрового оверлея (Shift+Tab) и записи (F7).
    // Регистрируются из JS только на время игровой сессии.
    // (в v2.4.x у плагина нет init() — только Builder::new().build())
    #[cfg(desktop)]
    app.handle().plugin(tauri_plugin_global_shortcut::Builder::new().build())?;
    startup_trace("setup: плагины готовы");
    // Одноразовая миграция бренда: данные пользователя из исторического
    // «trial-launcher» переезжают в актуальный «nivora-launcher»
    // (переименование целиком либо слияние без затирания).
    match rust_engine::state::migrate_legacy_roots() {
        Some(note) => startup_trace(&format!("setup: legacy-миграция — {note}")),
        None => startup_trace("setup: legacy-миграция не требуется"),
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
    }
    #[cfg(desktop)]
    match setup_tray(app) {
        Ok(()) => TRAY_AVAILABLE.store(true, std::sync::atomic::Ordering::SeqCst),
        // Трей — не критичная подсистема: на некоторых сборках Windows он
        // падает (занятый идентификатор, битые иконки шелла), и раньше это
        // убивало запуск целиком. Продолжаем без трея: крестик тогда будет
        // закрывать окно по-настоящему, а не прятать его в никуда.
        Err(error) => startup_trace(&format!("setup: трей недоступен ({error}) — без трея")),
    }
    Ok(())
}

#[cfg(desktop)]
fn setup_tray(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, "show", "Показать Nivora Launcher", true, None::<&str>)?;
    // Запасной способ открыть оверлей: на Wayland глобальные клавиши
    // не работают в принципе, а пункт меню в трее — работает везде.
    let overlay = MenuItem::with_id(app, "overlay", "Игровой оверлей (Shift+Tab)", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выйти", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &overlay, &quit])?;
    startup_trace("setup: меню готово");
    let mut tray = TrayIconBuilder::new()
        .menu(&menu)
        .tooltip("Nivora Launcher")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "overlay" => {
                // Главное окно владеет состоянием оверлея и создаст его
                // окно при необходимости — сюда только сигнал.
                let _ = app.emit("nivora-overlay-open", ());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    startup_trace("setup: трей готов, окно создаётся");
    Ok(())
}

#[cfg(desktop)]
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::{backend_resource_path, rotate_log, uses_transient_backend_worker, validate_backend_request};
    use crate::rust_engine;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn rust_native_operations_are_allowlisted_and_not_duplicated_in_python_routing() {
        // Контракт: каждая нативная операция обязана проходить allowlist
        // (иначе мост отдаёт «Операция GUI не разрешена» до движка), а
        // запуски игр по-прежнему уходят в transient-воркер.
        for operation in rust_engine::NATIVE_OPS {
            assert!(rust_engine::is_native(operation), "{operation}");
            assert!(
                validate_backend_request(&json!({ "operation": operation })).is_ok(),
                "нативная операция {operation} не прошла allowlist"
            );
            assert!(!uses_transient_backend_worker(&json!({ "operation": operation })));
        }
    }

    #[test]
    fn backend_log_rotates_and_keeps_a_bounded_history() {
        let directory = std::env::temp_dir().join(format!("nivora-rotate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let log = directory.join("backend.log");
        for round in 0..5 {
            std::fs::write(&log, vec![b'x'; 64 + round]).unwrap();
            rotate_log(&log, 32, 3);
        }
        assert!(!log.exists());
        for index in 1..=3 {
            assert!(directory.join(format!("backend.log.{index}")).exists(), "{index}");
        }
        assert!(!directory.join("backend.log.4").exists());
        assert_eq!(std::fs::metadata(directory.join("backend.log.1")).unwrap().len(), 68);
        std::fs::write(&log, b"small").unwrap();
        rotate_log(&log, 32, 3);
        assert!(log.exists());
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn backend_resource_path_uses_the_platform_executable_name() {
        let executable = if cfg!(windows) {
            "nivora-launcher-backend.exe"
        } else {
            "nivora-launcher-backend"
        };
        assert_eq!(
            backend_resource_path(Path::new("resources")),
            Path::new("resources/backend").join(executable)
        );
    }

    #[test]
    fn transient_backend_worker_is_used_only_for_launch_operations() {
        assert!(uses_transient_backend_worker(&json!({"operation": "launch"})));
        assert!(uses_transient_backend_worker(&json!({"operation": "launch_instance"})));
        assert!(!uses_transient_backend_worker(&json!({"operation": "server_start"})));
        assert!(!uses_transient_backend_worker(&json!({})));
    }

    #[test]
    fn github_catalog_operations_are_allowed() {
        for operation in [
            "github_plugin_status",
            "github_plugin_search",
            "github_plugin_details",
            "github_plugin_install",
            "github_plugin_oauth_start",
            "github_plugin_oauth_poll",
            "github_plugin_sign_out",
            "github_plugin_publish",
        ] {
            assert!(validate_backend_request(&json!({"operation": operation})).is_ok(), "{operation}");
        }
    }

    #[test]
    fn removed_lan_session_operations_are_rejected() {
        for operation in ["lan_session_create", "lan_session_join", "lan_session_status", "lan_session_leave"] {
            assert!(validate_backend_request(&json!({"operation": operation})).is_err(), "{operation}");
        }
    }

    #[test]
    fn instance_repair_version_operation_is_allowed() {
        assert!(validate_backend_request(&json!({"operation": "instance_repair_version"})).is_ok());
        assert!(!uses_transient_backend_worker(&json!({"operation": "instance_repair_version"})));
    }

    #[test]
    fn server_network_operations_are_allowed() {
        for operation in ["server_network_diagnose", "server_network_status", "server_network_prepare", "server_network_open", "server_network_close"] {
            assert!(validate_backend_request(&json!({"operation": operation})).is_ok(), "{operation}");
            assert!(!uses_transient_backend_worker(&json!({"operation": operation})));
        }
    }

    #[test]
    fn bedrock_operations_are_allowed_by_the_desktop_bridge() {
        for operation in [
            "bedrock_runtime_status",
            "bedrock_versions",
            "bedrock_setup_version",
            "bedrock_setup_status",
            "bedrock_setup_cancel",
            "bedrock_create_instance",
            "bedrock_content_list",
            "bedrock_content_import",
            "bedrock_content_remove",
            "bedrock_store_login",
            "bedrock_store_login_status",
            "bedrock_store_login_cancel",
            "bedrock_account_status",
            "bedrock_stop",
            "system_notify",
            "overlay_state",
            "recording_start",
            "recording_stop",
            "recording_status",
        ] {
            assert!(validate_backend_request(&json!({"operation": operation})).is_ok(), "{operation}");
        }
    }

    #[test]
    fn plugin_v2_security_and_runtime_operations_are_allowed() {
        for operation in [
            "plugin_security_status",
            "plugin_permission_grants_set",
            "plugin_local_untrusted_execution",
            "plugin_publisher_pin",
            "plugin_publisher_unpin",
            "plugin_local_revoke",
            "plugin_local_unrevoke",
            "plugin_api_source",
            "plugin_api_runtime_info",
            "plugin_api_capabilities",
            "plugin_api_quota_usage",
            "plugin_api_permissions_get",
            "plugin_api_permissions_set",
            "plugin_api_authorize",
            "plugin_api_open_external",
            "plugin_api_files_list",
            "plugin_api_files_read",
            "plugin_api_files_write",
            "plugin_api_files_remove",
            "plugin_api_config_projected_read",
            "plugin_http_request",
            "plugin_api_instances_list",
            "plugin_api_process_metrics",
            "plugin_api_proxy_routes_get",
            "plugin_api_proxy_routes_save",
            "plugin_api_proxy_routes_add",
            "plugin_api_proxy_routes_remove",
        ] {
            assert!(validate_backend_request(&json!({"operation": operation})).is_ok(), "{operation}");
        }
    }
}
