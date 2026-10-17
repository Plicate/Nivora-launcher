// Учётные записи: файл accounts.json с приватной записью (indent-2 JSON),
// оффлайн-аккаунт с детерминированным UUID v5, secret-tool для удаления
// Microsoft-токенов. Сетевой вход Microsoft и миграция legacy-токенов
// остаются за воркером (см. needs_worker в mod.rs).
use serde_json::{json, Map, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::constants::{DEFAULT_MSA_CLIENT_ID, MSA_LOGIN_API_URL};
use super::net::HttpClient;
use super::state::{launcher_roots, read_bounded_json};

const ACCOUNTS_FILE_MAX_BYTES: u64 = 4 * 1024 * 1024;
const STATUS_FILE_MAX_BYTES: u64 = 256 * 1024;
const STATUS_FILE: &str = "microsoft-login.json";

fn err(message: &str) -> String {
    message.to_owned()
}

fn accounts_file(config_home: &Path) -> PathBuf {
    config_home.join("accounts.json")
}

/// str(x) из Python для строковых параметров: отсутствующий ключ — дефолт,
/// null — "None", числа — десятичная запись.
fn py_str(request: &Value, key: &str, fallback: &str) -> String {
    match request.get(key) {
        None | Some(Value::Null) => match request.get(key) {
            None => fallback.to_owned(),
            Some(Value::Null) => "None".to_owned(),
            _ => unreachable!(),
        },
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(other) => other.to_string(),
    }
}

/// Чтение accounts.json с теми же ограничениями, что и Python: символические
/// ссылки и файлы больше 4 МиБ отвергаются, повреждённый JSON — ошибка.
fn read_records(config_home: &Path) -> Result<Vec<Map<String, Value>>, String> {
    let path = accounts_file(config_home);
    let Ok(meta) = fs::symlink_metadata(&path) else {
        return Ok(Vec::new());
    };
    if meta.is_symlink() || meta.len() > ACCOUNTS_FILE_MAX_BYTES {
        return Err(err("Файл аккаунтов слишком большой или является ссылкой"));
    }
    let text = fs::read_to_string(&path).map_err(|error| format!("Не удалось прочитать аккаунты: {error}"))?;
    let value: Value = serde_json::from_str(&text).map_err(|error| format!("Не удалось прочитать аккаунты: {error}"))?;
    let items = value.as_array().cloned().unwrap_or_default();
    Ok(items.into_iter().filter_map(|item| item.as_object().cloned()).collect())
}

/// Приватная атомарная запись JSON: indent 2, завершающий перевод строки,
/// права 0600 — байт-в-байт как _write_private_json в Python.
fn write_private_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("Не удалось записать аккаунты: {error}"))?;
        super::platform::restrict_dir(parent);
    }
    let temporary = path.with_file_name(format!(".{}.{}", path.file_name().and_then(|name| name.to_str()).unwrap_or("accounts.json"), std::process::id()));
    {
        let mut file = super::platform::create_private(&temporary)
            .map_err(|error| format!("Не удалось записать аккаунты: {error}"))?;
        let mut text = serde_json::to_string_pretty(value).map_err(|error| format!("Не удалось записать аккаунты: {error}"))?;
        text.push('\n');
        file.write_all(text.as_bytes()).map_err(|error| format!("Не удалось записать аккаунты: {error}"))?;
    }
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("Не удалось записать аккаунты: {error}")
    })
}

fn records_value(records: &[Map<String, Value>]) -> Value {
    Value::Array(records.iter().map(|record| Value::Object(record.clone())).collect())
}

fn record_str(record: &Map<String, Value>, key: &str) -> String {
    // str(value.get(key, "")): отсутствующий ключ — пустая строка.
    match record.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

fn record_f64(record: &Map<String, Value>, key: &str) -> f64 {
    record.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn record_i64(record: &Map<String, Value>, key: &str) -> i64 {
    // int(x): строки тоже приводятся, дробные отбрасывают дробную часть.
    match record.get(key) {
        Some(Value::Number(number)) => number.as_f64().map(|value| value as i64).unwrap_or(0),
        Some(Value::String(text)) => text.trim().parse::<f64>().ok().map(|value| value as i64).unwrap_or(0),
        _ => 0,
    }
}

/// Публичное представление аккаунта без токенов.
fn account_public(record: &Map<String, Value>) -> Value {
    let alias = record_str(record, "alias");
    let username = record_str(record, "username");
    json!({
        "account_id": record_str(record, "account_id"),
        "kind": record_str(record, "kind"),
        "username": username,
        "alias": alias,
        "display_name": if alias.is_empty() { username } else { alias },
        "expires_at": record_f64(record, "expires_at"),
        "limit_seconds": record_i64(record, "limit_seconds"),
        "used_seconds": record_i64(record, "used_seconds"),
        "is_default": record.get("is_default").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// Признак legacy-токенов Microsoft прямо в файле: такие записи Python
/// переносит в Secret Service при первом чтении — миграция остаётся ему.
pub(crate) fn has_legacy_tokens(config_home: &Path) -> bool {
    let Ok(records) = read_records(config_home) else {
        return false;
    };
    records.iter().any(|record| {
        record_str(record, "kind") == "microsoft" && (!record_str(record, "access_token").is_empty() || !record_str(record, "refresh_token").is_empty())
    })
}

/// Ник Minecraft: 3–16 символов, только латиница, цифры и «_». Раньше
/// недопустимые символы молча выбрасывались (а кириллица проходила через
/// is_alphanumeric) — теперь это явная ошибка, как и в интерфейсе.
pub(crate) fn validate_nickname(value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Err(err("Введите ник"));
    }
    let mut invalid: Vec<char> = Vec::new();
    for character in value.chars() {
        if !(character.is_ascii_alphanumeric() || character == '_') && !invalid.contains(&character) {
            invalid.push(character);
        }
    }
    if !invalid.is_empty() {
        let mut shown: Vec<String> = invalid
            .iter()
            .take(6)
            .map(|&character| match character {
                ' ' => "«пробел»".to_owned(),
                c if c.is_whitespace() => "«пробельный символ»".to_owned(),
                c => format!("«{c}»"),
            })
            .collect();
        if invalid.len() > 6 {
            shown.push("…".to_owned());
        }
        return Err(format!(
            "Ник содержит недопустимые символы: {}. Разрешены только латинские буквы, цифры и _",
            shown.join(", ")
        ));
    }
    let length = value.chars().count();
    if length < 3 {
        return Err(err("Ник должен быть не короче 3 символов"));
    }
    if length > 16 {
        return Err(err("Ник должен быть не длиннее 16 символов"));
    }
    Ok(value.to_owned())
}

pub fn create_offline_account(request: &Value) -> Result<Value, String> {
    create_offline_account_in(&launcher_roots().config_home, request)
}

pub(crate) fn create_offline_account_in(config_home: &Path, request: &Value) -> Result<Value, String> {
    let username = py_str(request, "username", "Player");
    let safe_name = validate_nickname(&username)?;
    let uuid = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, format!("nivora-launcher:{safe_name}").as_bytes()).simple().to_string();
    let mut records = read_records(config_home)?;
    let mut record = Map::new();
    // Порядок ключей — как to_dict() у Account: файл байт-в-байт тот же.
    record.insert("account_id".into(), json!("offline-debug"));
    record.insert("kind".into(), json!("offline"));
    record.insert("username".into(), json!(safe_name));
    record.insert("uuid".into(), json!(uuid));
    record.insert("expires_at".into(), json!(0.0));
    record.insert("xuid".into(), json!(""));
    record.insert("used_seconds".into(), json!(0));
    record.insert("limit_seconds".into(), json!(0));
    record.insert("alias".into(), json!(""));
    record.insert("is_default".into(), json!(records.is_empty()));
    record.insert("access_token".into(), json!("nivora-launcher-offline"));
    record.insert("refresh_token".into(), json!(""));
    let mut replaced = false;
    for item in records.iter_mut() {
        if record_str(item, "account_id") == "offline-debug" {
            *item = record.clone();
            replaced = true;
            break;
        }
    }
    if !replaced {
        records.push(record.clone());
    }
    write_private_json(&accounts_file(config_home), &records_value(&records))?;
    Ok(json!({ "account": account_public(&record) }))
}

pub fn delete_account(request: &Value) -> Result<Value, String> {
    delete_account_in(&launcher_roots().config_home, request)
}

pub(crate) fn delete_account_in(config_home: &Path, request: &Value) -> Result<Value, String> {
    let account_id = py_str(request, "account_id", "");
    let records = read_records(config_home)?;
    let updated: Vec<Map<String, Value>> = records.iter().filter(|item| record_str(item, "account_id") != account_id).cloned().collect();
    if updated.len() == records.len() {
        return Ok(json!({ "removed": false }));
    }
    write_private_json(&accounts_file(config_home), &records_value(&updated))?;
    if records.iter().any(|item| record_str(item, "account_id") == account_id && record_str(item, "kind") == "microsoft") {
        remove_file_tokens(config_home, &account_id)?;
        clear_secret_payload_if_available(&account_id)?;
    }
    Ok(json!({ "removed": true }))
}

pub fn set_default_account(request: &Value) -> Result<Value, String> {
    set_default_account_in(&launcher_roots().config_home, request)
}

pub(crate) fn set_default_account_in(config_home: &Path, request: &Value) -> Result<Value, String> {
    let account_id = py_str(request, "account_id", "");
    let mut records = read_records(config_home)?;
    let mut selected: Option<Map<String, Value>> = None;
    for item in records.iter_mut() {
        let is_selected = record_str(item, "account_id") == account_id;
        item.insert("is_default".into(), json!(is_selected));
        if is_selected {
            selected = Some(item.clone());
        }
    }
    let Some(selected) = selected else {
        return Err(err(&format!("Аккаунт не найден: {account_id}")));
    };
    write_private_json(&accounts_file(config_home), &records_value(&records))?;
    set_settings_default_account(config_home, &account_id)?;
    Ok(json!({
        "account": account_public(&selected),
        "accounts": records.iter().map(account_public).collect::<Vec<_>>(),
    }))
}

pub fn move_account(request: &Value) -> Result<Value, String> {
    move_account_in(&launcher_roots().config_home, request)
}

pub(crate) fn move_account_in(config_home: &Path, request: &Value) -> Result<Value, String> {
    let account_id = py_str(request, "account_id", "");
    let direction = py_str(request, "direction", "down");
    let mut records = read_records(config_home)?;
    let index = records.iter().position(|item| record_str(item, "account_id") == account_id);
    let Some(index) = index else {
        return Err(err(&format!("Аккаунт не найден: {account_id}")));
    };
    // Выход за границы — просто бездействие, как в Python (без записи файла).
    let target: i64 = if direction == "up" {
        index as i64 - 1
    } else if direction == "down" {
        index as i64 + 1
    } else {
        index as i64
    };
    if target >= 0 && (target as usize) < records.len() {
        records.swap(index, target as usize);
        write_private_json(&accounts_file(config_home), &records_value(&records))?;
    }
    Ok(json!({ "accounts": records.iter().map(account_public).collect::<Vec<_>>() }))
}

pub fn microsoft_login_status(_request: &Value) -> Result<Value, String> {
    let roots = launcher_roots();
    let path = roots.cache_home.join(STATUS_FILE);
    let Ok(meta) = fs::symlink_metadata(&path) else {
        return Ok(json!({ "status": "idle" }));
    };
    if meta.is_symlink() || meta.len() > STATUS_FILE_MAX_BYTES {
        return Ok(json!({ "status": "idle" }));
    }
    let value = read_bounded_json(&path, STATUS_FILE_MAX_BYTES).unwrap_or(Value::Null);
    if !value.is_object() {
        return Ok(json!({ "status": "idle" }));
    }
    Ok(value)
}

/// Точечная запись default_account_id в settings.json: файл уже содержит
/// нормализованные настройки, поэтому достаточно заменить одно значение,
/// не прогоняя весь набор нормализаторов.
fn set_settings_default_account(config_home: &Path, value: &str) -> Result<(), String> {
    let normalized: String = value.trim().chars().take(200).collect();
    let normalized = if normalized.is_empty() { "auto".to_owned() } else { normalized };
    let settings_path = config_home.join("settings.json");
    let mut settings = read_bounded_json(&settings_path, 1024 * 1024)
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    settings.insert("default_account_id".into(), json!(normalized));
    write_private_json(&settings_path, &Value::Object(settings))
}

fn secret_tool_path() -> Result<PathBuf, String> {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|directory| directory.join("secret-tool"))
                .find(|candidate| candidate.is_file())
        })
        .map(PathBuf::from)
        .ok_or_else(|| err("Для Microsoft-токенов нужны libsecret, secret-tool и работающий gnome-keyring"))
}

fn valid_account_id(account_id: &str) -> Result<(), String> {
    if account_id.is_empty() || !account_id.chars().all(|character| character.is_alphanumeric() || character == '.' || character == '_' || character == '-') {
        return Err(err("Некорректный идентификатор Microsoft-аккаунта"));
    }
    Ok(())
}

struct ToolOutput {
    code: i32,
    stdout: String,
    stderr: String,
}

/// secret-tool отсутствует (нужна libsecret) либо процесс не запустился
/// или не уложился в таймаут — Python различает эти случаи через типы
/// исключений, вызывающие стороны подставляют свои тексты ошибок.
enum SecretToolError {
    Missing,
    Spawn,
}

fn secret_tool_error(context: &str) -> impl Fn(SecretToolError) -> String + '_ {
    move |error| match error {
        SecretToolError::Missing => err("Для Microsoft-токенов нужны libsecret, secret-tool и работающий gnome-keyring"),
        SecretToolError::Spawn => err(context),
    }
}

/// Один запуск внешней команды с потолком 5 секунд: зависший secret-tool
/// не должен вешать операцию GUI. Поток-исполнитель по таймауту бросается,
/// он ничего не пишет и завершится сам.
fn spawn_timed(path: &Path, arguments: &[&str], environment: &[(String, String)], input_text: Option<&str>) -> Result<ToolOutput, SecretToolError> {
    let path = path.to_path_buf();
    let arguments: Vec<String> = arguments.iter().map(|value| value.to_string()).collect();
    let environment = environment.to_vec();
    let input_text = input_text.map(str::to_owned);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let outcome = (|| -> std::io::Result<ToolOutput> {
            let mut command = Command::new(&path);
            command.args(&arguments).env_clear().envs(environment.iter().cloned()).stdout(Stdio::piped()).stderr(Stdio::piped());
            let output = if let Some(payload) = input_text.as_deref() {
                command.stdin(Stdio::piped());
                let mut child = command.spawn()?;
                if let Some(stdin) = child.stdin.as_mut() {
                    stdin.write_all(payload.as_bytes())?;
                }
                child.wait_with_output()
            } else {
                command.stdin(Stdio::null());
                command.output()
            };
            output.map(|raw| ToolOutput {
                code: raw.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&raw.stdout).to_string(),
                stderr: String::from_utf8_lossy(&raw.stderr).to_string(),
            })
        })();
        let _ = sender.send(outcome);
    });
    match receiver.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(_)) | Err(_) => Err(SecretToolError::Spawn),
    }
}

fn run_secret_tool(arguments: &[&str], input_text: Option<&str>) -> Result<ToolOutput, SecretToolError> {
    let command_path = secret_tool_path().map_err(|_| SecretToolError::Missing)?;
    let mut environment: Vec<(String, String)> = std::env::vars().collect();
    let first = spawn_timed(&command_path, arguments, &environment, input_text)?;
    if first.code == 0 {
        return Ok(first);
    }
    // У minimal-сессий libsecret бывает установлен, пока Secret Service ещё
    // не запущен: поднимаем только компонент secrets и повторяем запрос.
    let keyring = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|directory| directory.join("gnome-keyring-daemon"))
            .find(|candidate| candidate.is_file())
    });
    let Some(keyring) = keyring else {
        return Ok(first);
    };
    if keyring == command_path {
        return Ok(first);
    }
    let Ok(started) = spawn_timed(&keyring, &["--start", "--components=secrets"], &environment, None) else {
        return Ok(first);
    };
    if started.code != 0 {
        return Ok(first);
    }
    for line in started.stdout.lines() {
        if let Some((key, value)) = line.split_once('=') {
            if !key.is_empty() && !value.is_empty() {
                environment.push((key.to_owned(), value.to_owned()));
            }
        }
    }
    spawn_timed(&command_path, arguments, &environment, input_text)
}

/// Удаление Microsoft-токенов из Secret Service; коды 0 и 1 — успех.
/// Чистим и канонический сервис, и legacy-имя переходного периода.
fn clear_account_tokens(account_id: &str) -> Result<(), String> {
    valid_account_id(account_id)?;
    for service in [KEYRING_SERVICE, LEGACY_KEYRING_SERVICE] {
        let output = run_secret_tool(&["clear", "service", service, "account-id", account_id], None)
            .map_err(secret_tool_error("Не удалось удалить Microsoft-токен из Secret Service"))?;
        if output.code != 0 && output.code != 1 {
            let detail = {
                let raw = if output.stderr.trim().is_empty() { output.stdout } else { output.stderr };
                raw.trim().replace('\n', " ").chars().take(240).collect::<String>()
            };
            if detail.is_empty() {
                return Err(err("Secret Service не удалил Microsoft-токен"));
            }
            return Err(err(&format!("Secret Service не удалил Microsoft-токен: {detail}")));
        }
    }
    Ok(())
}

// --------------------------- Microsoft OAuth ---------------------------
// Device-code flow: devicecode → poll token → XBL → XSTS → launcher/login →
// profile → entitlements. Токены живут в Secret Service (Linux) или в
// диспетчере учётных данных (Windows), в accounts.json — только публичные
// поля, поэтому to_dict() у Microsoft-аккаунтов не содержит токенов.

const XBOX_USER_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const LAUNCHER_LOGIN_URL: &str = "https://api.minecraftservices.com/launcher/login";
const LICENSE_URL: &str = "https://api.minecraftservices.com/entitlements/license";
const PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";
const MSA_SCOPE: &str = "XboxLive.signin offline_access";

fn unix_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_secs_f64()).unwrap_or(0.0)
}

fn setting_str(settings: &Value, key: &str, fallback: &str) -> String {
    settings
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| fallback.to_owned())
}

fn str_field(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
}

fn int_field(value: &Value, key: &str, fallback: i64) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(fallback)
}

/// str() из Python для подстановки в сообщения: числа — десятично, строки —
/// как есть, остальное — JSON.
fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

/// bool() из Python: пустая строка/ноль/null — ложь, остальное — истина.
fn py_bool(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0) != 0.0,
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}

pub(crate) fn write_microsoft_status(cache_home: &Path, value: &Value) -> Result<(), String> {
    let mut object = value.as_object().cloned().unwrap_or_default();
    object.insert("updated_at".into(), json!(unix_now()));
    write_private_json(&cache_home.join(STATUS_FILE), &Value::Object(object))
}

/// Аккаунт Microsoft с токенами в памяти: в файл пишется публичная часть.
pub(crate) struct MicrosoftAccount {
    pub(crate) account_id: String,
    pub(crate) username: String,
    pub(crate) uuid: String,
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    pub(crate) xuid: String,
    pub(crate) expires_at: f64,
    pub(crate) alias: String,
    pub(crate) is_default: bool,
}

/// Каноническая запись to_dict(): порядок ключей фиксирован, токенов нет.
fn microsoft_stored_record(account: &MicrosoftAccount) -> Map<String, Value> {
    let mut record = Map::new();
    record.insert("account_id".into(), json!(account.account_id));
    record.insert("kind".into(), json!("microsoft"));
    record.insert("username".into(), json!(account.username));
    record.insert("uuid".into(), json!(account.uuid));
    record.insert("expires_at".into(), json!(account.expires_at));
    record.insert("xuid".into(), json!(account.xuid));
    record.insert("used_seconds".into(), json!(0));
    record.insert("limit_seconds".into(), json!(0));
    record.insert("alias".into(), json!(account.alias));
    record.insert("is_default".into(), json!(account.is_default));
    record
}

/// Запись массива аккаунтов в файл: у Microsoft вытираются токены, чтобы
/// случайно не оставить их на диске после миграции или refresh.
fn stored_value(records: &[Map<String, Value>]) -> Value {
    Value::Array(
        records
            .iter()
            .map(|record| {
                if record_str(record, "kind") == "microsoft" {
                    let mut clean = record.clone();
                    clean.remove("access_token");
                    clean.remove("refresh_token");
                    Value::Object(clean)
                } else {
                    Value::Object(record.clone())
                }
            })
            .collect(),
    )
}

fn record_bool(record: &Map<String, Value>, key: &str) -> bool {
    py_bool(record.get(key))
}

// --- хранилище токенов -------------------------------------------------

/// Достать JSON-пейлоад токенов для аккаунта; пустая строка — «нет»
/// (любая ошибка хранилища трактуется как отсутствие токенов, как в Python).
/// Имя сервиса ключницы. НЕ ПЕРЕИМЕНОВЫВАТЬ: каноническое имя исторически
/// `trial-launcher-microsoft`, и у действующих пользователей токены уже
/// лежат под ним (Python-сторона читает только его). `nivora-launcher-microsoft`
/// — имя переходного периода: читаем из него как из legacy и переносим
/// в каноническое, иначе свежий вход «не виден» операциям Python (скины).
const KEYRING_SERVICE: &str = "trial-launcher-microsoft";
const LEGACY_KEYRING_SERVICE: &str = "nivora-launcher-microsoft";

#[cfg(not(windows))]
fn lookup_secret_payload(account_id: &str) -> String {
    if valid_account_id(account_id).is_err() {
        return String::new();
    }
    let lookup = |service: &str| -> Option<String> {
        let output = run_secret_tool(&["lookup", "service", service, "account-id", account_id], None).ok()?;
        if output.code != 0 {
            return None;
        }
        let payload = output.stdout.trim_end_matches('\n').to_owned();
        if payload.is_empty() {
            None
        } else {
            Some(payload)
        }
    };
    if let Some(payload) = lookup(KEYRING_SERVICE) {
        return payload;
    }
    // Лечение переходного периода: токен нашли под старым именем —
    // переносим в каноническое и убираем дубль.
    if let Some(payload) = lookup(LEGACY_KEYRING_SERVICE) {
        if store_secret_payload(account_id, &payload).is_ok() {
            let _ = run_secret_tool(&["clear", "service", LEGACY_KEYRING_SERVICE, "account-id", account_id], None);
        }
        return payload;
    }
    String::new()
}

#[cfg(windows)]
fn lookup_secret_payload(account_id: &str) -> String {
    let canonical = windows_read_chunked(KEYRING_SERVICE, account_id);
    if !canonical.is_empty() {
        return canonical;
    }
    // Лечение переходного периода: токен нашли под старым именем —
    // переносим в каноническое и убираем дубль.
    let mut legacy = windows_keyring::get(LEGACY_KEYRING_SERVICE, account_id);
    // Токены, сохранённые прежним Python-бэкендом через пакет keyring.
    if legacy.is_empty() {
        legacy = windows_keyring::get_target(&format!("{account_id}@{KEYRING_SERVICE}"), None);
    }
    if legacy.is_empty() {
        legacy = windows_keyring::get_target(KEYRING_SERVICE, Some(account_id));
    }
    if !legacy.is_empty() {
        if windows_store_chunked(KEYRING_SERVICE, account_id, &legacy).is_ok() {
            let _ = windows_keyring::delete(LEGACY_KEYRING_SERVICE, account_id);
        }
        return legacy;
    }
    String::new()
}

#[cfg(not(windows))]
fn store_secret_payload(account_id: &str, payload: &str) -> Result<(), String> {
    // Secret Service допускает дубликаты с теми же атрибутами: сначала
    // чистим, чтобы lookup не вернул протухший токен после refresh.
    clear_secret_payload(account_id)?;
    let output = run_secret_tool(
        &["store", "--label", "Nivora Launcher Microsoft", "service", KEYRING_SERVICE, "account-id", account_id],
        Some(payload),
    )
    .map_err(secret_tool_error("Не удалось сохранить Microsoft-токен в Secret Service"))?;
    if output.code != 0 {
        let detail = secret_detail(&output);
        return Err(match detail {
            Some(text) => err(&format!("Secret Service не сохранил Microsoft-токен: {text}")),
            None => err("Secret Service не сохранил Microsoft-токен"),
        });
    }
    Ok(())
}

#[cfg(windows)]
fn store_secret_payload(account_id: &str, payload: &str) -> Result<(), String> {
    windows_store_chunked(KEYRING_SERVICE, account_id, payload)
}

#[cfg(not(windows))]
fn clear_secret_payload(account_id: &str) -> Result<(), String> {
    clear_account_tokens(account_id)
}

#[cfg(windows)]
fn clear_secret_payload(account_id: &str) -> Result<(), String> {
    windows_delete_chunked(KEYRING_SERVICE, account_id)
}

/// Удаление из хранилища, если оно вообще есть: без secret-tool токены
/// могли лежать только в резервном файле — удаление аккаунта не должно падать.
fn clear_secret_payload_if_available(account_id: &str) -> Result<(), String> {
    #[cfg(not(windows))]
    if secret_tool_path().is_err() {
        return Ok(());
    }
    clear_secret_payload(account_id)
}

// --- Windows: запись токенов частями ----------------------------------------
// Диспетчер учётных данных ограничивает секрет 2560 байтами
// (CRED_MAX_CREDENTIAL_BLOB_SIZE), а пара токенов Minecraft + Microsoft в
// UTF-16 обычно больше: CredWriteW отказывал, и вход «молча» не сохранялся.
// Длинный пейлоад режется на части «<id>:partN», в основной записи — маркер.
const CHUNK_MARKER: &str = "nivora-chunked:";
const CHUNK_CHARS: usize = 1000;
const MAX_CHUNKS: usize = 32;

fn split_secret(payload: &str) -> Vec<String> {
    let chars: Vec<char> = payload.chars().collect();
    if chars.len() <= CHUNK_CHARS {
        return vec![payload.to_owned()];
    }
    chars.chunks(CHUNK_CHARS).map(|part| part.iter().collect()).collect()
}

fn chunk_count(main: &str) -> Option<usize> {
    main.strip_prefix(CHUNK_MARKER)?.trim().parse::<usize>().ok().filter(|count| (1..=MAX_CHUNKS).contains(count))
}

fn chunk_user(account_id: &str, index: usize) -> String {
    format!("{account_id}:part{index}")
}

#[cfg(windows)]
fn windows_read_chunked(service: &str, account_id: &str) -> String {
    let main = windows_keyring::get(service, account_id);
    let Some(count) = chunk_count(&main) else { return main };
    let mut payload = String::new();
    for index in 0..count {
        let part = windows_keyring::get(service, &chunk_user(account_id, index));
        if part.is_empty() {
            return String::new();
        }
        payload.push_str(&part);
    }
    payload
}

#[cfg(windows)]
fn windows_delete_chunked(service: &str, account_id: &str) -> Result<(), String> {
    let main = windows_keyring::get(service, account_id);
    if let Some(count) = chunk_count(&main) {
        for index in 0..count {
            windows_keyring::delete(service, &chunk_user(account_id, index))?;
        }
    }
    windows_keyring::delete(service, account_id)
}

#[cfg(windows)]
fn windows_store_chunked(service: &str, account_id: &str, payload: &str) -> Result<(), String> {
    windows_delete_chunked(service, account_id)?;
    let parts = split_secret(payload);
    if parts.len() == 1 {
        return windows_keyring::set(service, account_id, payload);
    }
    if parts.len() > MAX_CHUNKS {
        return Err(err("Microsoft-токен слишком длинный для диспетчера учётных данных Windows"));
    }
    for (index, part) in parts.iter().enumerate() {
        windows_keyring::set(service, &chunk_user(account_id, index), part)?;
    }
    windows_keyring::set(service, account_id, &format!("{CHUNK_MARKER}{}", parts.len()))
}

// --- Резервный файл токенов -------------------------------------------------
// Если системное хранилище недоступно или временное (Secret Service без
// разблокированной связки «login» кладёт секреты в session-коллекцию, которая
// живёт только до перезапуска), токены пишутся в приватный файл рядом с
// accounts.json (0600 / ACL пользователя). Так делают и другие лаунчеры;
// иначе вход Microsoft приходилось бы повторять при каждом запуске.
const TOKEN_FILE: &str = "microsoft-tokens.json";

fn token_file(config_home: &Path) -> PathBuf {
    config_home.join(TOKEN_FILE)
}

fn read_file_tokens(config_home: &Path) -> Map<String, Value> {
    match read_bounded_json(&token_file(config_home), ACCOUNTS_FILE_MAX_BYTES) {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

fn write_file_tokens(config_home: &Path, tokens: &Map<String, Value>) -> Result<(), String> {
    let path = token_file(config_home);
    if tokens.is_empty() {
        if path.exists() {
            fs::remove_file(&path).map_err(|error| format!("Не удалось удалить файл токенов: {error}"))?;
        }
        return Ok(());
    }
    write_private_json(&path, &Value::Object(tokens.clone()))
}

fn store_file_tokens(config_home: &Path, account_id: &str, payload: &str) -> Result<(), String> {
    valid_account_id(account_id)?;
    let value: Value = serde_json::from_str(payload).map_err(|_| err("Некорректные данные токена"))?;
    let mut tokens = read_file_tokens(config_home);
    tokens.insert(account_id.to_owned(), value);
    write_file_tokens(config_home, &tokens)
}

fn remove_file_tokens(config_home: &Path, account_id: &str) -> Result<(), String> {
    let mut tokens = read_file_tokens(config_home);
    if tokens.remove(account_id).is_some() {
        write_file_tokens(config_home, &tokens)?;
    }
    Ok(())
}

fn lookup_file_tokens(config_home: &Path, account_id: &str) -> String {
    read_file_tokens(config_home)
        .get(account_id)
        .filter(|value| value.is_object())
        .map(|value| value.to_string())
        .unwrap_or_default()
}

/// Сохранилось ли значение в постоянной коллекции Secret Service.
#[cfg(not(windows))]
fn secret_is_persistent(account_id: &str, payload: &str) -> bool {
    if let Ok(search) = run_secret_tool(&["search", "--all", "service", KEYRING_SERVICE, "account-id", account_id], None) {
        if search.code == 0 && search.stdout.contains("/collection/session") {
            return false;
        }
    }
    lookup_secret_payload(account_id) == payload
}

#[cfg(windows)]
fn secret_is_persistent(account_id: &str, payload: &str) -> bool {
    lookup_secret_payload(account_id) == payload
}

fn secret_detail(output: &ToolOutput) -> Option<String> {
    let raw = if output.stderr.trim().is_empty() { &output.stdout } else { &output.stderr };
    let text = raw.trim().replace('\n', " ");
    let text: String = text.chars().take(240).collect();
    if text.is_empty() { None } else { Some(text) }
}

fn store_account_tokens(config_home: &Path, account: &MicrosoftAccount) -> Result<(), String> {
    if account.access_token.is_empty() {
        return Err(err("Microsoft access token отсутствует; повторите вход"));
    }
    let payload = serde_json::to_string(&json!({
        "access_token": account.access_token,
        "refresh_token": account.refresh_token,
    }))
    .unwrap_or_default();
    match store_secret_payload(&account.account_id, &payload) {
        Ok(()) if secret_is_persistent(&account.account_id, &payload) => {
            // Хранилище надёжно — резервная копия больше не нужна.
            remove_file_tokens(config_home, &account.account_id)
        }
        _ => store_file_tokens(config_home, &account.account_id, &payload),
    }
}

/// Токены аккаунта: системное хранилище, затем резервный файл; любая
/// ошибка — «пусто», как в Python.
fn load_account_tokens(config_home: &Path, account_id: &str) -> (String, String) {
    let mut payload = lookup_secret_payload(account_id);
    if payload.is_empty() && valid_account_id(account_id).is_ok() {
        payload = lookup_file_tokens(config_home, account_id);
    }
    let Ok(value) = serde_json::from_str::<Value>(&payload) else {
        return (String::new(), String::new());
    };
    if !value.is_object() {
        return (String::new(), String::new());
    }
    (str_field(&value, "access_token"), str_field(&value, "refresh_token"))
}

/// Записи accounts.json с токенами Microsoft в памяти. Legacy-токены,
/// оставшиеся в файле от старых версий, переносятся в хранилище и вытираются.
pub(crate) fn load_accounts_with_tokens(config_home: &Path) -> Result<Vec<Map<String, Value>>, String> {
    let mut records = read_records(config_home)?;
    let mut changed = false;
    for record in records.iter_mut() {
        if record_str(record, "kind") != "microsoft" {
            continue;
        }
        let file_access = record_str(record, "access_token");
        let file_refresh = record_str(record, "refresh_token");
        if file_access.is_empty() && file_refresh.is_empty() {
            let (access_token, refresh_token) = load_account_tokens(config_home, &record_str(record, "account_id"));
            record.insert("access_token".into(), json!(access_token));
            record.insert("refresh_token".into(), json!(refresh_token));
            continue;
        }
        let migrated = MicrosoftAccount {
            account_id: record_str(record, "account_id"),
            username: record_str(record, "username"),
            uuid: record_str(record, "uuid"),
            access_token: file_access,
            refresh_token: file_refresh,
            xuid: record_str(record, "xuid"),
            expires_at: record_f64(record, "expires_at"),
            alias: record_str(record, "alias"),
            is_default: record_bool(record, "is_default"),
        };
        store_account_tokens(config_home, &migrated)?;
        *record = microsoft_stored_record(&migrated);
        changed = true;
    }
    if changed {
        write_private_json(&accounts_file(config_home), &stored_value(&records))?;
    }
    Ok(records)
}

fn record_is_microsoft(record: &Map<String, Value>) -> bool {
    record_str(record, "kind") == "microsoft"
}

/// Используется при запуске игры — см. launcher.rs (интеграция).
#[allow(dead_code)]
fn item_to_account(record: &Map<String, Value>) -> MicrosoftAccount {
    MicrosoftAccount {
        account_id: record_str(record, "account_id"),
        username: record_str(record, "username"),
        uuid: record_str(record, "uuid"),
        access_token: record_str(record, "access_token"),
        refresh_token: record_str(record, "refresh_token"),
        xuid: record_str(record, "xuid"),
        expires_at: record_f64(record, "expires_at"),
        alias: record_str(record, "alias"),
        is_default: record_bool(record, "is_default"),
    }
}

/// Сохранение Microsoft-аккаунта: токены — в хранилище, публичная часть — в
/// accounts.json (замещение по account_id, первая запись становится default).
fn save_microsoft_account(config_home: &Path, account: &MicrosoftAccount) -> Result<Map<String, Value>, String> {
    store_account_tokens(config_home, account)?;
    let mut records = read_records(config_home)?;
    let mut replacement = microsoft_stored_record(account);
    for record in records.iter_mut() {
        if record_str(record, "account_id") == account.account_id {
            *record = replacement.clone();
            write_private_json(&accounts_file(config_home), &stored_value(&records))?;
            return Ok(replacement);
        }
    }
    if records.is_empty() {
        replacement.insert("is_default".into(), json!(true));
    }
    records.push(replacement.clone());
    write_private_json(&accounts_file(config_home), &stored_value(&records))?;
    Ok(replacement)
}

// --- сам flow ------------------------------------------------------------

enum PollOutcome {
    Token(Value),
    Pending,
    SlowDown,
    Failed(String),
}

/// Разбор ответа token-endpoint: HTTP 400 с authorization_pending — рабочее
/// состояние device-code flow, а не ошибка.
fn classify_poll(status: u16, response: &Value) -> PollOutcome {
    if status == 200 && !str_field(response, "access_token").is_empty() {
        return PollOutcome::Token(response.clone());
    }
    let raw = response.get("error").filter(|value| !value.is_null());
    let text = raw.map(|value| value.as_str().map(str::to_owned).unwrap_or_else(|| value_to_text(value)));
    match text.as_deref() {
        Some("authorization_pending") => PollOutcome::Pending,
        Some("slow_down") => PollOutcome::SlowDown,
        Some(other) => PollOutcome::Failed(format!("Microsoft OAuth завершился ошибкой: {other}")),
        None => PollOutcome::Failed(err("Microsoft OAuth завершился ошибкой: неизвестная ошибка")),
    }
}

fn wait_seconds(http: &HttpClient, seconds: f64) -> Result<(), String> {
    match &http.cancellation {
        Some(token) => token.wait(seconds),
        None => {
            std::thread::sleep(Duration::from_secs_f64(seconds.max(0.0)));
            Ok(())
        }
    }
}

fn poll_msa_token(login_api_url: &str, client_id: &str, device_code: &str, mut interval: i64, expires_in: i64, http: &HttpClient) -> Result<Value, String> {
    let deadline = Instant::now() + Duration::from_secs(expires_in.max(1) as u64);
    loop {
        if Instant::now() >= deadline {
            return Err(err("Время ожидания входа Microsoft истекло"));
        }
        let (status, response) = http.post_form_soft(
            &format!("{login_api_url}/token"),
            &[
                ("grant_type".to_owned(), "urn:ietf:params:oauth:grant-type:device_code".to_owned()),
                ("client_id".to_owned(), client_id.to_owned()),
                ("device_code".to_owned(), device_code.to_owned()),
            ],
        )?;
        match classify_poll(status, &response) {
            PollOutcome::Token(value) => return Ok(value),
            PollOutcome::Pending => wait_seconds(http, interval as f64)?,
            PollOutcome::SlowDown => {
                interval += 5;
                wait_seconds(http, interval as f64)?;
            }
            PollOutcome::Failed(message) => return Err(message),
        }
    }
}

/// requestId для entitlement-запроса: нужна уникальность, не криптография —
/// счётчик + наносекунды через splitmix64 достаточно.
fn random_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos() as u64)
        .unwrap_or(0)
        ^ (std::process::id() as u64).rotate_left(32)
        ^ COUNTER.fetch_add(1, Ordering::Relaxed).rotate_left(17);
    let mut next = || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let (mut high, mut low) = (next().to_be_bytes(), next().to_be_bytes());
    high[6] = (high[6] & 0x0F) | 0x40;
    low[0] = (low[0] & 0x3F) | 0x80;
    let hex = |bytes: &[u8]| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    format!("{}-{}-{}-{}-{}", hex(&high[..4]), hex(&high[4..6]), hex(&high[6..8]), hex(&low[..2]), hex(&low[2..]))
}

fn fetch_profile(token: &str, http: &HttpClient) -> Result<Value, String> {
    let headers = vec![
        ("Authorization".to_owned(), format!("Bearer {token}")),
        ("Accept".to_owned(), "application/json".to_owned()),
    ];
    let raw = http.request(PROFILE_URL, "GET", None, Some(&headers), None, None)?;
    serde_json::from_slice(&raw).map_err(|_| err("Minecraft Services вернул некорректные profile"))
}

fn owns_minecraft(response: &Value) -> bool {
    response
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                matches!(
                    item.get("name").and_then(Value::as_str),
                    Some("game_minecraft") | Some("product_minecraft")
                )
            })
        })
        .unwrap_or(false)
}

fn check_ownership(token: &str, http: &HttpClient) -> Result<(), String> {
    let headers = vec![
        ("Authorization".to_owned(), format!("Bearer {token}")),
        ("Accept".to_owned(), "application/json".to_owned()),
    ];
    let raw = http.request(&format!("{LICENSE_URL}?requestId={}", random_request_id()), "GET", None, Some(&headers), None, None)?;
    let response: Value = serde_json::from_slice(&raw).map_err(|_| err("Minecraft Services вернул некорректные entitlement данные"))?;
    if !owns_minecraft(&response) {
        return Err(err("У Microsoft-аккаунта не найдено право на Minecraft Java Edition"));
    }
    Ok(())
}

fn finish_microsoft_login(msa: &Value, http: &HttpClient) -> Result<MicrosoftAccount, String> {
    let access_token = str_field(msa, "access_token");
    let xbl = http.post_json(
        XBOX_USER_URL,
        &json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={access_token}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }),
        None,
    )?;
    let xsts = http.post_json(
        XSTS_URL,
        &json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl.get("Token").cloned().unwrap_or(Value::Null)] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }),
        Some(&[("x-xbl-contract-version".to_owned(), "1".to_owned())]),
    )?;
    if let Some(xerr) = xsts.get("XErr").filter(|value| !value.is_null()) {
        return Err(format!("Xbox XSTS отклонил аккаунт, код {}", value_to_text(xerr)));
    }
    let uhs = xsts.pointer("/DisplayClaims/xui/0/uhs").and_then(Value::as_str).unwrap_or("").to_owned();
    let xsts_token = str_field(&xsts, "Token");
    if uhs.is_empty() || xsts_token.is_empty() {
        return Err(err("Xbox XSTS не вернул пользовательский токен"));
    }
    let minecraft = http.post_json(
        LAUNCHER_LOGIN_URL,
        &json!({
            "platform": "PC_LAUNCHER",
            "xtoken": format!("XBL3.0 x={uhs};{xsts_token}"),
        }),
        None,
    )?;
    let token = str_field(&minecraft, "access_token");
    if token.is_empty() {
        return Err(err("Minecraft Services не выдал access token"));
    }
    let profile = fetch_profile(&token, http)?;
    check_ownership(&token, http)?;
    let profile_id = value_to_text(profile.get("id").unwrap_or(&Value::Null)).trim().to_owned();
    let account_id = profile_id.to_lowercase();
    if valid_account_id(&account_id).is_err() {
        return Err(err("Minecraft Services не вернул корректный id профиля"));
    }
    let username = profile
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| minecraft.get("username").and_then(Value::as_str))
        .unwrap_or("Microsoft")
        .to_owned();
    Ok(MicrosoftAccount {
        account_id,
        username,
        uuid: profile_id,
        expires_at: unix_now() + int_field(&minecraft, "expires_in", 86400) as f64 - 120.0,
        access_token: token,
        refresh_token: str_field(msa, "refresh_token"),
        xuid: str_field(&minecraft, "xuid"),
        alias: String::new(),
        is_default: false,
    })
}

/// Уже знакомый аккаунт (тот же id или UUID профиля) сохраняет свой
/// account_id, alias и позицию default — повторный вход не плодит записи.
fn adopt_existing(records: &[Map<String, Value>], account: &mut MicrosoftAccount) {
    let identity = account.uuid.replace('-', "").to_lowercase();
    let existing = records
        .iter()
        .find(|record| record_str(record, "account_id") == account.account_id)
        .or_else(|| {
            if identity.is_empty() {
                return None;
            }
            records
                .iter()
                .find(|record| record_is_microsoft(record) && record_str(record, "uuid").replace('-', "").to_lowercase() == identity)
        });
    if let Some(existing) = existing {
        account.account_id = record_str(existing, "account_id");
        account.alias = record_str(existing, "alias");
        account.is_default = record_bool(existing, "is_default");
    }
}

pub(crate) fn login_device_code(
    config_home: &Path,
    cache_home: &Path,
    settings: &Value,
    http: &HttpClient,
    notify: &dyn Fn(&str),
) -> Result<MicrosoftAccount, String> {
    let client_id = setting_str(settings, "client_id", DEFAULT_MSA_CLIENT_ID).trim().to_owned();
    if client_id.is_empty() {
        return Err(err("Microsoft-вход недоступен в этой сборке: public client ID не настроен"));
    }
    let login_api_url = setting_str(settings, "login_api_url", MSA_LOGIN_API_URL).trim_end_matches('/').to_owned();
    write_microsoft_status(cache_home, &json!({ "status": "requesting" }))?;
    let response = http.post_form(
        &format!("{login_api_url}/devicecode"),
        &[("client_id".to_owned(), client_id.clone()), ("scope".to_owned(), MSA_SCOPE.to_owned())],
    )?;
    let verification_uri = str_field(&response, "verification_uri")
        .is_empty()
        .then(|| str_field(&response, "verification_uri_complete"))
        .unwrap_or_else(|| str_field(&response, "verification_uri"));
    let user_code = str_field(&response, "user_code");
    let device_code = str_field(&response, "device_code");
    if verification_uri.is_empty() || user_code.is_empty() || device_code.is_empty() {
        return Err(err("Microsoft не выдал корректный device code"));
    }
    let expires_in = int_field(&response, "expires_in", 900);
    write_microsoft_status(
        cache_home,
        &json!({
            "status": "device_code",
            "code": user_code,
            "verification_uri": verification_uri,
            "expires_in": expires_in,
        }),
    )?;
    notify(&format!("КОД MICROSOFT: {user_code}"));
    notify(&format!("Откройте {verification_uri} и введите этот код в браузере."));
    let interval = int_field(&response, "interval", 5).max(5);
    let msa = poll_msa_token(&login_api_url, &client_id, &device_code, interval, expires_in, http)?;
    let mut account = finish_microsoft_login(&msa, http)?;
    let records = load_accounts_with_tokens(config_home)?;
    adopt_existing(&records, &mut account);
    save_microsoft_account(config_home, &account)?;
    write_microsoft_status(cache_home, &json!({ "status": "completed" }))?;
    Ok(account)
}

/// Валидный аккаунт для запуска игры: освежает токен, если до конца меньше
/// минуты. Счётчики игрового времени сбрасываются — так ведёт себя Python.
/// Используется при запуске игры — см. launcher.rs (интеграция).
#[allow(dead_code)]
pub(crate) fn get_valid_account(
    config_home: &Path,
    settings: &Value,
    http: &HttpClient,
    account_id: &str,
    force_refresh: bool,
) -> Result<MicrosoftAccount, String> {
    let records = load_accounts_with_tokens(config_home)?;
    let by_id = |id: &str| records.iter().find(|record| record_str(record, "account_id") == id);
    let microsoft_default = || {
        records
            .iter()
            .find(|record| record_is_microsoft(record) && record_bool(record, "is_default"))
            .or_else(|| records.iter().find(|record| record_is_microsoft(record)))
    };
    // Пара (аккаунт, найден ли он как Microsoft): по id может прилететь и
    // оффлайн-аккаунт — тогда это ошибка, а не замена, как в Python.
    let mut found: Option<(MicrosoftAccount, bool)> = if !account_id.is_empty() {
        by_id(account_id).map(|record| (item_to_account(record), record_is_microsoft(record)))
    } else {
        None
    };
    if found.is_none() && account_id.is_empty() {
        let configured = setting_str(settings, "default_account_id", "auto");
        if configured.to_lowercase() != "auto" {
            found = by_id(&configured).map(|record| (item_to_account(record), record_is_microsoft(record)));
        }
        if !matches!(&found, Some((_, true))) {
            found = microsoft_default().map(|record| (item_to_account(record), true));
        }
    }
    let Some((account, is_microsoft)) = found else {
        return Err(err("Microsoft-аккаунт не найден. Используйте: login microsoft"));
    };
    if !is_microsoft {
        return Err(err("Microsoft-аккаунт не найден. Используйте: login microsoft"));
    }
    if !force_refresh && account.expires_at > unix_now() + 60.0 && !account.access_token.is_empty() {
        return Ok(account);
    }
    if account.refresh_token.is_empty() {
        return Err(err("Microsoft-токен истёк. Повторите login microsoft"));
    }
    let client_id = setting_str(settings, "client_id", DEFAULT_MSA_CLIENT_ID);
    let login_api_url = setting_str(settings, "login_api_url", MSA_LOGIN_API_URL).trim_end_matches('/').to_owned();
    let refresh = http.post_form(
        &format!("{login_api_url}/token"),
        &[
            ("client_id".to_owned(), client_id.clone()),
            ("grant_type".to_owned(), "refresh_token".to_owned()),
            ("refresh_token".to_owned(), account.refresh_token.clone()),
            ("scope".to_owned(), MSA_SCOPE.to_owned()),
        ],
    )?;
    if str_field(&refresh, "access_token").is_empty() {
        return Err(err("Не удалось обновить Microsoft-токен"));
    }
    let mut refreshed = finish_microsoft_login(&refresh, http)?;
    refreshed.account_id = account.account_id.clone();
    refreshed.alias = account.alias.clone();
    refreshed.is_default = account.is_default;
    if refreshed.refresh_token.is_empty() {
        refreshed.refresh_token = account.refresh_token.clone();
    }
    save_microsoft_account(config_home, &refreshed)?;
    Ok(refreshed)
}

pub fn login_microsoft(request: &Value) -> Result<Value, String> {
    let store = super::config::ConfigStore::load()?;
    if store.str_setting("login_source") != "microsoft" {
        return Err(err("Источник входа Microsoft отключён в настройках сторонних источников"));
    }
    login_microsoft_with(store.settings(), &store.str_setting("http_proxy"), request)
}

/// Полный вход с собранным по настройкам клиентом; notify дублирует
/// сообщения статуса (gui-слой превращает их в события microsoft-auth).
pub(crate) fn login_microsoft_with(settings: &Value, proxy: &str, request: &Value) -> Result<Value, String> {
    let cancellation = request
        .get("operation_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|id| std::sync::Arc::new(super::operations::CancellationToken::new(id)));
    let http = HttpClient::new(
        setting_int(settings, "download_timeout_seconds", 30).max(1) as u32,
        setting_int(settings, "download_retries", 3).max(1) as u32,
        proxy,
        cancellation,
    )?;
    // RefCell: замыкание уведомлений должно быть Fn, а не FnMut.
    let messages = std::cell::RefCell::new(Vec::<String>::new());
    let notify = |message: &str| messages.borrow_mut().push(message.to_owned());
    let roots = launcher_roots();
    let account = match login_device_code(&roots.config_home, &roots.cache_home, settings, &http, &notify) {
        Ok(account) => account,
        Err(error) => {
            // Без этого окно входа продолжало показывать код устройства, и
            // казалось, что лаунчер «молчит» после входа на сайте Microsoft.
            let _ = write_microsoft_status(&roots.cache_home, &json!({ "status": "failed", "error": error }));
            return Err(error);
        }
    };
    let messages = messages.into_inner();
    let record = microsoft_stored_record(&account);
    Ok(json!({ "account": account_public(&record), "messages": messages }))
}

fn setting_int(settings: &Value, key: &str, fallback: i64) -> i64 {
    settings.get(key).and_then(Value::as_i64).unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-accounts-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("создать временный каталог");
        dir
    }

    fn write_accounts(config: &Path, records: Value) {
        write_private_json(&accounts_file(config), &records).expect("запись accounts.json");
    }

    #[test]
    fn offline_account_uuid_is_stable_and_replaces_previous() {
        let config = temp_root("offline");
        let first = create_offline_account_in(&config, &json!({ "username": "Steve_77" })).expect("создание");
        assert_eq!(first["account"]["username"], json!("Steve_77"));
        assert_eq!(first["account"]["account_id"], json!("offline-debug"));
        let uuid_first = fs::read_to_string(accounts_file(&config)).unwrap();
        assert!(uuid_first.contains("\"kind\": \"offline\""));

        // Повторное создание с тем же именем замещает запись, а не дублирует.
        create_offline_account_in(&config, &json!({ "username": "Steve_77" })).expect("повторное создание");
        let raw = fs::read_to_string(accounts_file(&config)).unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 1);
        // Формат записи: indent 2 (элементы массива — 4) и завершающий
        // перевод строки, как json.dump в Python.
        assert!(raw.ends_with("]\n") && raw.contains("\n    \"account_id\": \"offline-debug\","));

        // UUIDv5 детерминирован: тот же вход — то же значение. is_default
        // присваивается только первой записи (записей уже нет/есть).
        let second = create_offline_account_in(&config, &json!({ "username": "Steve_77" })).expect("ещё раз");
        assert_eq!(second["account"]["uuid"], first["account"]["uuid"]);
        assert_eq!(second["account"]["username"], first["account"]["username"]);
        assert_eq!(first["account"]["is_default"], json!(true));
        assert_eq!(second["account"]["is_default"], json!(false));

        // Недопустимые для Minecraft ники отвергаются, а не обрезаются молча.
        let error = create_offline_account_in(&config, &json!({ "username": "  Steve_77!  " })).unwrap_err();
        assert_eq!(error, "Ник содержит недопустимые символы: «пробел», «!». Разрешены только латинские буквы, цифры и _");
        let error = create_offline_account_in(&config, &json!({ "username": "Игрок" })).unwrap_err();
        assert!(error.starts_with("Ник содержит недопустимые символы: «И»"));
        assert_eq!(create_offline_account_in(&config, &json!({ "username": "ab" })).unwrap_err(), "Ник должен быть не короче 3 символов");
        assert_eq!(create_offline_account_in(&config, &json!({ "username": "a".repeat(17) })).unwrap_err(), "Ник должен быть не длиннее 16 символов");
        assert_eq!(create_offline_account_in(&config, &json!({ "username": "" })).unwrap_err(), "Введите ник");
    }

    #[test]
    fn default_and_move_and_delete_follow_python_semantics() {
        let config = temp_root("store");
        write_accounts(
            &config,
            json!([
                { "account_id": "alpha", "kind": "offline", "username": "Alpha", "alias": "", "is_default": true },
                { "account_id": "beta", "kind": "offline", "username": "Beta", "alias": "Бета", "is_default": false },
            ]),
        );

        let moved = move_account_in(&config, &json!({ "account_id": "beta", "direction": "up" })).expect("сдвиг");
        let order: Vec<&str> = moved["accounts"].as_array().unwrap().iter().map(|item| item["account_id"].as_str().unwrap()).collect();
        assert_eq!(order, vec!["beta", "alpha"]);
        // display_name берётся из alias, когда он есть.
        assert_eq!(moved["accounts"][0]["display_name"], json!("Бета"));

        let default = set_default_account_in(&config, &json!({ "account_id": "beta" })).expect("default");
        assert_eq!(default["account"]["is_default"], json!(true));
        let settings_raw = fs::read_to_string(config.join("settings.json")).expect("settings");
        assert!(settings_raw.contains("\"default_account_id\": \"beta\""));

        let removed = delete_account_in(&config, &json!({ "account_id": "alpha" })).expect("удаление");
        assert_eq!(removed["removed"], json!(true));
        let missing = delete_account_in(&config, &json!({ "account_id": "alpha" })).expect("повторное удаление");
        assert_eq!(missing["removed"], json!(false));

        let error = set_default_account_in(&config, &json!({ "account_id": "ghost" })).unwrap_err();
        assert_eq!(error, "Аккаунт не найден: ghost");
    }

    #[test]
    fn legacy_token_gate_detects_microsoft_records_only() {
        let config = temp_root("legacy");
        assert!(!has_legacy_tokens(&config));
        write_accounts(
            &config,
            json!([
                { "account_id": "offline-debug", "kind": "offline", "username": "S", "access_token": "nivora-launcher-offline" },
                { "account_id": "msid", "kind": "microsoft", "username": "M", "access_token": "", "refresh_token": "" },
            ]),
        );
        assert!(!has_legacy_tokens(&config), "пустые токены Microsoft — не legacy");
        write_accounts(
            &config,
            json!([{ "account_id": "msid", "kind": "microsoft", "username": "M", "access_token": "", "refresh_token": "rt" }]),
        );
        assert!(has_legacy_tokens(&config));
    }

    #[test]
    #[cfg(unix)] // symlink без DevMode на Windows не создаётся
    fn oversized_or_linked_accounts_file_is_rejected() {
        let config = temp_root("guards");
        write_accounts(&config, json!([]));
        let error = read_records(&config);
        assert!(error.is_ok());
        // Символическая ссылка вместо файла.
        let linked = temp_root("linked");
        std::os::unix::fs::symlink(accounts_file(&config), accounts_file(&linked)).expect("ссылка");
        let error = read_records(&linked).unwrap_err();
        assert_eq!(error, "Файл аккаунтов слишком большой или является ссылкой");
    }

    #[test]
    fn login_status_defaults_to_idle_and_passes_object_through() {
        let roots = temp_root("status");
        fs::create_dir_all(&roots).expect("корень");
        // Нет файла — idle.
        let value = microsoft_login_status(&json!({})).expect("статус");
        assert_eq!(value["status"], json!("idle"));
        // Повреждённый файл — idle.
        fs::write(roots.join(STATUS_FILE), "not-json").unwrap();
        assert_eq!(microsoft_login_status(&json!({})).unwrap()["status"], json!("idle"));
        // Корректный объект проходит как есть.
        fs::write(roots.join(STATUS_FILE), json!({ "status": "device_code", "code": "AB12" }).to_string()).unwrap();
        // microsoft_login_status читает launcher_roots() — проверяем парсер напрямую.
        let parsed = read_bounded_json(&roots.join(STATUS_FILE), STATUS_FILE_MAX_BYTES).unwrap();
        assert_eq!(parsed["code"], json!("AB12"));
    }

    #[test]
    fn account_id_validation_rejects_injection() {
        assert!(valid_account_id("a.b-c_d").is_ok());
        assert!(valid_account_id("id with space").is_err());
        assert!(valid_account_id("").is_err());
        assert!(valid_account_id("../etc").is_err());
    }
}

/// Диспетчер учётных данных Windows: generic-запись с целевым именем
/// «service:username», пароль — UTF-16 blob (совместимо с keyring).
#[cfg(windows)]
mod windows_keyring {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Пустая строка — записи нет или читать не удалось; вызывающая сторона
    /// трактует это как отсутствие токенов.
    pub fn get(service: &str, username: &str) -> String {
        get_target(&format!("{service}:{username}"), None)
    }

    /// Чтение произвольной цели; expected_user — проверка поля UserName
    /// (формат пакета keyring: цель = имя сервиса, UserName = account_id).
    pub fn get_target(target: &str, expected_user: Option<&str>) -> String {
        let target = wide(target);
        let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: target — NUL-terminated буфер, credential — out-параметр.
        // В windows-sys 0.59 у CredReadW четыре параметра (третий — flags).
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
            return String::new();
        }
        unsafe {
            let user_matches = match expected_user {
                None => true,
                Some(expected) => {
                    let raw = (*credential).UserName;
                    if raw.is_null() {
                        false
                    } else {
                        let mut length = 0usize;
                        while *raw.add(length) != 0 {
                            length += 1;
                        }
                        String::from_utf16_lossy(std::slice::from_raw_parts(raw, length)) == expected
                    }
                }
            };
            let size = (*credential).CredentialBlobSize as usize / 2;
            let text = if user_matches && size > 0 {
                let data = std::slice::from_raw_parts((*credential).CredentialBlob as *const u16, size);
                String::from_utf16_lossy(data)
            } else {
                String::new()
            };
            CredFree(credential.cast());
            text
        }
    }

    pub fn set(service: &str, username: &str, payload: &str) -> Result<(), String> {
        let target = wide(&format!("{service}:{username}"));
        let user = wide(username);
        let mut blob: Vec<u16> = payload.encode_utf16().collect();
        let mut credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_ptr() as *mut u16,
            Comment: std::ptr::null_mut(),
            LastWritten: unsafe { std::mem::zeroed() },
            CredentialBlobSize: (blob.len() * 2) as u32,
            CredentialBlob: blob.as_mut_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: std::ptr::null_mut(),
            UserName: user.as_ptr() as *mut u16,
        };
        // SAFETY: credential заполнен целиком, буферы живут до вызова.
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err("Не удалось сохранить Microsoft-токен в диспетчере учётных данных Windows".into());
        }
        Ok(())
    }

    pub fn delete(service: &str, username: &str) -> Result<(), String> {
        if get(service, username).is_empty() {
            return Ok(());
        }
        let target = wide(&format!("{service}:{username}"));
        // SAFETY: target — NUL-terminated буфер.
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            return Err("Не удалось удалить Microsoft-токен из диспетчера учётных данных Windows".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod oauth_tests {
    use super::*;

    #[test]
    fn stored_record_drops_tokens_and_keeps_field_order() {
        let account = MicrosoftAccount {
            account_id: "abc".into(),
            username: "Steve".into(),
            uuid: "ID".into(),
            access_token: "secret".into(),
            refresh_token: "rt".into(),
            xuid: "42".into(),
            expires_at: 123.5,
            alias: "".into(),
            is_default: false,
        };
        let record = microsoft_stored_record(&account);
        let keys: Vec<&str> = record.keys().map(String::as_str).collect();
        // Порядок to_dict() у Account — файл байт-в-байт как у Python.
        assert_eq!(keys, vec!["account_id", "kind", "username", "uuid", "expires_at", "xuid", "used_seconds", "limit_seconds", "alias", "is_default"]);
        let raw = serde_json::to_string(&stored_value(&[record])).unwrap();
        assert!(!raw.contains("secret") && !raw.contains("refresh"));
    }

    #[test]
    fn stored_value_keeps_offline_tokens_but_strips_microsoft() {
        let mut offline = Map::new();
        offline.insert("account_id".into(), json!("offline-debug"));
        offline.insert("kind".into(), json!("offline"));
        offline.insert("access_token".into(), json!("nivora-launcher-offline"));
        let mut microsoft = Map::new();
        microsoft.insert("account_id".into(), json!("mid"));
        microsoft.insert("kind".into(), json!("microsoft"));
        microsoft.insert("access_token".into(), json!("at"));
        microsoft.insert("refresh_token".into(), json!("rt"));
        let value = stored_value(&[offline.clone(), microsoft]);
        let items = value.as_array().unwrap();
        assert_eq!(items[0]["access_token"], json!("nivora-launcher-offline"));
        assert!(items[1].get("access_token").is_none());
        assert!(items[1].get("refresh_token").is_none());
    }

    #[test]
    fn classify_poll_covers_device_code_states() {
        assert!(matches!(classify_poll(200, &json!({ "access_token": "t" })), PollOutcome::Token(_)));
        assert!(matches!(classify_poll(400, &json!({ "error": "authorization_pending" })), PollOutcome::Pending));
        assert!(matches!(classify_poll(400, &json!({ "error": "slow_down" })), PollOutcome::SlowDown));
        let failed = classify_poll(400, &json!({ "error": "access_denied" }));
        assert!(matches!(&failed, PollOutcome::Failed(message) if message == "Microsoft OAuth завершился ошибкой: access_denied"));
        let unknown = classify_poll(400, &json!({}));
        assert!(matches!(&unknown, PollOutcome::Failed(message) if message == "Microsoft OAuth завершился ошибкой: неизвестная ошибка"));
        // 200 без токена — не успех.
        assert!(matches!(classify_poll(200, &json!({})), PollOutcome::Failed(_)));
    }

    #[test]
    fn ownership_requires_minecraft_entitlement() {
        assert!(owns_minecraft(&json!({ "items": [{ "name": "game_minecraft" }] })));
        assert!(owns_minecraft(&json!({ "items": [{ "name": "other" }, { "name": "product_minecraft" }] })));
        assert!(!owns_minecraft(&json!({ "items": [{ "name": "other" }] })));
        assert!(!owns_minecraft(&json!({})));
    }

    #[test]
    fn request_id_is_uuid4_shaped_and_unique() {
        let first = random_request_id();
        let second = random_request_id();
        assert_ne!(first, second);
        let parts: Vec<&str> = first.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len() + parts[1].len() + parts[2].len() + parts[3].len() + parts[4].len(), 32);
        // Версия 4 и вариант RFC 4122.
        let bytes: Vec<char> = first.chars().collect();
        assert_eq!(bytes[14], '4');
        assert!(matches!(bytes[19], '8'..='b'));
    }

    #[test]
    fn adopt_existing_matches_by_id_then_by_profile_uuid() {
        let mut account = MicrosoftAccount {
            account_id: "new-id".into(),
            username: "X".into(),
            uuid: "AAAA-BBBB".into(),
            access_token: "t".into(),
            refresh_token: "r".into(),
            xuid: String::new(),
            expires_at: 0.0,
            alias: String::new(),
            is_default: false,
        };
        let mut other = Map::new();
        other.insert("account_id".into(), json!("unrelated"));
        other.insert("kind".into(), json!("offline"));
        adopt_existing(&[other], &mut account);
        assert_eq!(account.account_id, "new-id");

        let mut known = Map::new();
        known.insert("account_id".into(), json!("old-id"));
        known.insert("kind".into(), json!("microsoft"));
        known.insert("uuid".into(), json!("aaaabbbb"));
        known.insert("alias".into(), json!("Псевдоним"));
        known.insert("is_default".into(), json!(true));
        adopt_existing(&[known], &mut account);
        assert_eq!(account.account_id, "old-id");
        assert_eq!(account.alias, "Псевдоним");
        assert!(account.is_default);
    }

    #[test]
    fn status_file_gains_updated_at() {
        let cache = std::env::temp_dir().join(format!("nivora-msa-status-{}", std::process::id()));
        let _ = fs::remove_dir_all(&cache);
        fs::create_dir_all(&cache).unwrap();
        write_microsoft_status(&cache, &json!({ "status": "device_code", "code": "AB" })).unwrap();
        let raw = fs::read_to_string(cache.join(STATUS_FILE)).unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(value["status"], json!("device_code"));
        assert!(value["updated_at"].as_f64().unwrap() > 0.0);
        let _ = fs::remove_dir_all(&cache);
    }

    #[test]
    fn token_store_rejects_empty_access_token() {
        let account = MicrosoftAccount {
            account_id: "abc".into(),
            username: String::new(),
            uuid: String::new(),
            access_token: String::new(),
            refresh_token: "rt".into(),
            xuid: String::new(),
            expires_at: 0.0,
            alias: String::new(),
            is_default: false,
        };
        assert_eq!(store_account_tokens(Path::new("/nonexistent"), &account).unwrap_err(), "Microsoft access token отсутствует; повторите вход");
    }

    #[test]
    fn py_bool_follows_python_truthiness() {
        assert!(py_bool(Some(&json!("любая строка"))));
        assert!(!py_bool(Some(&json!(""))));
        assert!(!py_bool(Some(&json!(0))));
        assert!(py_bool(Some(&json!(0.5))));
        assert!(!py_bool(None));
        assert!(!py_bool(Some(&Value::Null)));
        assert!(py_bool(Some(&json!({})))); // непустой контейнер — истина
    }

    #[test]
    fn load_account_tokens_tolerates_garbage() {
        let root = std::env::temp_dir().join(format!("nivora-tokens-garbage-{}", std::process::id()));
        let _ = fs::create_dir_all(&root);
        let (access, refresh) = load_account_tokens(&root, "not-an-id!");
        assert_eq!((access.as_str(), refresh.as_str()), ("", ""));
        // Валидный id, но хранилище в тестовой среде недоступно — пусто.
        let (access, refresh) = load_account_tokens(&root, "abc.123");
        assert_eq!((access.as_str(), refresh.as_str()), ("", ""));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn file_fallback_round_trip_survives_restart() {
        let root = std::env::temp_dir().join(format!("nivora-tokens-file-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        store_file_tokens(&root, "abc.123", r#"{"access_token":"a","refresh_token":"r"}"#).unwrap();
        // «Перезапуск»: читаем заново с диска.
        assert_eq!(load_account_tokens(&root, "abc.123"), ("a".to_owned(), "r".to_owned()));
        remove_file_tokens(&root, "abc.123").unwrap();
        assert!(!token_file(&root).exists());
        assert!(store_file_tokens(&root, "bad id", "{}").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn long_secrets_are_split_under_credential_manager_limit() {
        let payload = "x".repeat(2500);
        let parts = split_secret(&payload);
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|part| part.encode_utf16().count() * 2 <= 2560));
        assert_eq!(parts.concat(), payload);
        assert_eq!(split_secret("short"), vec!["short".to_owned()]);
        assert_eq!(chunk_count("nivora-chunked:3"), Some(3));
        assert_eq!(chunk_count("nivora-chunked:999"), None);
        assert_eq!(chunk_count("{\"access_token\":\"a\"}"), None);
    }
}
