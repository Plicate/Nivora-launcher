// порт python-модуля skins.py: библиотека PNG-скинов и управление
// скинами/плащами профиля Minecraft (Microsoft-аккаунты).
use base64::Engine;
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::accounts::{self, MicrosoftAccount};
use super::net::HttpClient;

const PROFILE_SKINS_URL: &str = "https://api.minecraftservices.com/minecraft/profile/skins";
const PROFILE_URL: &str = "https://api.minecraftservices.com/minecraft/profile";
const PROFILE_CAPE_URL: &str = "https://api.minecraftservices.com/minecraft/profile/capes/active";
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const MAX_SKIN_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) struct SkinInfo {
    skin_id: String,
    name: String,
    path: PathBuf,
    model: String,
    image: String,
}

impl SkinInfo {
    fn public(&self) -> Value {
        json!({
            "skin_id": self.skin_id,
            "name": self.name,
            "model": self.model,
            "path": self.path.to_string_lossy(),
            "image": self.image,
        })
    }
}

pub(crate) struct SkinStore {
    root: PathBuf,
    index: PathBuf,
}

fn field_str(record: &Value, key: &str) -> String {
    record
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn field_bool(record: &Value, key: &str) -> bool {
    record.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// uuid4().hex[:N]: случайные hex-символы (splitmix64 без внешних крейтов).
fn random_hex(chars: usize) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos() as u64)
        .unwrap_or(0)
        ^ (std::process::id() as u64).rotate_left(32)
        ^ COUNTER.fetch_add(1, Ordering::Relaxed).rotate_left(17);
    let mut next = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let mut out = String::with_capacity(chars);
    while out.len() < chars {
        out.push_str(&format!("{:08x}", next()));
    }
    out.truncate(chars);
    out
}

impl SkinStore {
    /// SkinStore(config) в Python: корень — data_home/skins.
    pub(crate) fn open() -> Result<Self, String> {
        let roots = super::state::launcher_roots();
        Self::at_root(&roots.data_home.join("skins"))
    }

    /// внедряемый корень — для тестов без env-песочницы.
    fn at_root(root: &Path) -> Result<Self, String> {
        if fs::symlink_metadata(root)
            .map(|meta| meta.is_symlink())
            .unwrap_or(false)
        {
            return Err("Каталог скинов является символьной ссылкой".to_owned());
        }
        fs::create_dir_all(root)
            .map_err(|error| format!("Не удалось создать каталог скинов: {error}"))?;
        if fs::symlink_metadata(root)
            .map(|meta| meta.is_symlink())
            .unwrap_or(false)
            || !root.is_dir()
        {
            return Err("Каталог скинов недоступен".to_owned());
        }
        Ok(Self {
            root: root.to_owned(),
            index: root.join("skins.json"),
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn list(&self) -> Result<Vec<SkinInfo>, String> {
        let mut values: Vec<SkinInfo> = Vec::new();
        for raw in self.read_index()? {
            let skin_id = field_str(&raw, "skin_id");
            let Some(path) = safe_skin_path(&self.root, &field_str(&raw, "filename")) else {
                continue;
            };
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            if skin_id.is_empty() || !meta.is_file() || meta.len() > MAX_SKIN_BYTES {
                continue;
            }
            if validate_png_skin_bytes(&fs::read(&path).unwrap_or_default()).is_err() {
                continue;
            }
            let model = {
                let model = field_str(&raw, "model");
                if model.is_empty() {
                    "classic".to_owned()
                } else {
                    model
                }
            };
            let image = image_data_url(&path);
            values.push(SkinInfo {
                skin_id: skin_id.clone(),
                name: {
                    let raw_name = field_str(&raw, "name");
                    if raw_name.is_empty() {
                        skin_id.clone()
                    } else {
                        raw_name
                    }
                },
                path,
                model,
                image,
            });
        }
        Ok(values)
    }

    pub(crate) fn get(&self, skin_id: &str) -> Result<SkinInfo, String> {
        self.list()?
            .into_iter()
            .find(|value| value.skin_id == skin_id)
            .ok_or_else(|| format!("Скин не найден: {skin_id}"))
    }

    pub(crate) fn import_bytes(
        &self,
        content: &[u8],
        filename: &str,
        name: Option<&str>,
        model: &str,
    ) -> Result<SkinInfo, String> {
        validate_png_skin_bytes(content)?;
        if model != "classic" && model != "slim" {
            return Err(format!("Неизвестная модель скина: {model}"));
        }
        let source_name = Path::new(filename)
            .file_stem()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        let skin_id = random_hex(12);
        let stored_filename = format!("{skin_id}.png");
        let target = self.root.join(&stored_filename);
        write_exclusive_private(&target, content)?;
        let display_name = {
            let raw = name.unwrap_or("").trim();
            let base = if raw.is_empty() { &source_name } else { raw };
            let taken: String = base.chars().take(80).collect();
            let trimmed = taken.trim().to_owned();
            if trimmed.is_empty() {
                skin_id.clone()
            } else {
                trimmed
            }
        };
        let mut values = self.read_index()?;
        values.push(json!({
            "skin_id": skin_id,
            "name": display_name,
            "filename": stored_filename,
            "model": model,
        }));
        self.write_index(&values)?;
        let image = image_data_url(&target);
        Ok(SkinInfo {
            skin_id,
            name: display_name,
            path: target,
            model: model.to_owned(),
            image,
        })
    }

    fn read_index(&self) -> Result<Vec<Value>, String> {
        if !self.index.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&self.index)
            .map_err(|error| format!("Не удалось прочитать библиотеку скинов: {error}"))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|error| format!("Не удалось прочитать библиотеку скинов: {error}"))?;
        let Value::Array(items) = value else {
            return Ok(Vec::new());
        };
        Ok(items.into_iter().filter(|item| item.is_object()).collect())
    }

    fn write_index(&self, values: &[Value]) -> Result<(), String> {
        if fs::symlink_metadata(&self.index)
            .map(|meta| meta.is_symlink())
            .unwrap_or(false)
        {
            return Err("Индекс скинов является символьной ссылкой".to_owned());
        }
        let body = serde_json::to_string(&Value::Array(values.to_vec()))
            .map_err(|error| format!("Не удалось записать библиотеку скинов: {error}"))?;
        write_truncate_private(&self.index, &(body + "\n"))
    }
}

/// os.open(..., O_EXCL | O_NOFOLLOW, 0o600) из Python.
fn write_exclusive_private(path: &Path, content: &[u8]) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        // O_NOFOLLOW: симлинк вместо файла — ошибка, а не перезапись цели.
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut handle = options
        .open(path)
        .map_err(|error| format!("Не удалось сохранить скин: {error}"))?;
    handle
        .write_all(content)
        .map_err(|error| format!("Не удалось сохранить скин: {error}"))
}

fn write_truncate_private(path: &Path, content: &str) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut handle = options
        .open(path)
        .map_err(|error| format!("Не удалось записать библиотеку скинов: {error}"))?;
    handle
        .write_all(content.as_bytes())
        .map_err(|error| format!("Не удалось записать библиотеку скинов: {error}"))
}

/// Проверить, что содержимое — корректный PNG-скин поддерживаемого размера.
pub(crate) fn validate_png_skin_bytes(content: &[u8]) -> Result<(), String> {
    let header = content;
    if header.len() < 24 || &header[..8] != PNG_SIGNATURE || &header[12..16] != b"IHDR" {
        return Err("Файл не является корректным PNG-скином".to_owned());
    }
    let width = u32::from_be_bytes([header[16], header[17], header[18], header[19]]);
    let height = u32::from_be_bytes([header[20], header[21], header[22], header[23]]);
    if !matches!(
        (width, height),
        (64, 32) | (64, 64) | (128, 64) | (128, 128)
    ) {
        return Err("Поддерживаются PNG-скины 64x32, 64x64, 128x64 или 128x128".to_owned());
    }
    Ok(())
}

fn image_data_url(path: &Path) -> String {
    match fs::read(path) {
        Ok(content) => format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(content)
        ),
        Err(_) => String::new(),
    }
}

/// путь внутри корня скинов без каталогов, симлинков и лишних суффиксов.
fn safe_skin_path(root: &Path, filename: &str) -> Option<PathBuf> {
    let candidate = Path::new(filename);
    let name = candidate.file_name()?;
    if filename.is_empty()
        || name != filename
        || candidate.is_absolute()
        || name == "."
        || name == ".."
        || !name.to_string_lossy().to_lowercase().ends_with(".png")
    {
        return None;
    }
    let path = root.join(name);
    // символьная ссылка или «выход» из корня — отказ (resolve-проверка).
    if fs::symlink_metadata(&path)
        .map(|meta| meta.is_symlink())
        .unwrap_or(true)
    {
        return None;
    }
    let canonical = fs::canonicalize(&path).ok()?;
    let root = fs::canonicalize(root).ok()?;
    if !canonical.starts_with(&root) {
        return None;
    }
    Some(path)
}

fn bearer_headers(account: &MicrosoftAccount) -> Vec<(String, String)> {
    vec![(
        "Authorization".to_owned(),
        format!("Bearer {}", account.access_token),
    )]
}

fn require_microsoft(account: &MicrosoftAccount, message: &str) -> Result<(), String> {
    // MicrosoftAccount по построению Microsoft-аккаунт: проверяем токен.
    if account.access_token.is_empty() {
        return Err(message.to_owned());
    }
    Ok(())
}

pub(crate) fn apply_skin(
    http: &HttpClient,
    account: &MicrosoftAccount,
    skin: &SkinInfo,
    model: &str,
) -> Result<(), String> {
    require_microsoft(account, "Скин можно установить только на Microsoft-аккаунт")?;
    if model != "classic" && model != "slim" {
        return Err(format!("Неизвестная модель скина: {model}"));
    }
    let boundary = format!("----TrialLauncher{}", random_hex(32));
    let data =
        fs::read(&skin.path).map_err(|error| format!("Не удалось прочитать скин: {error}"))?;
    let file_name = skin
        .path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "skin.png".to_owned());
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"variant\"\r\n\r\n");
    body.extend_from_slice(format!("{model}\r\n").as_bytes());
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: image/png\r\n\r\n");
    body.extend_from_slice(&data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    http.request(
        PROFILE_SKINS_URL,
        "POST",
        Some(&body),
        Some(&[
            (
                "Authorization".to_owned(),
                format!("Bearer {}", account.access_token),
            ),
            (
                "Content-Type".to_owned(),
                format!("multipart/form-data; boundary={boundary}"),
            ),
            ("Accept".to_owned(), "application/json".to_owned()),
        ]),
        None,
        None,
    )?;
    Ok(())
}

/// профиль скинов: нормализация записей + data-URL для превью (CSP-friendly).
pub(crate) fn profile(
    http: &HttpClient,
    account: &MicrosoftAccount,
    refresh: Option<&dyn Fn() -> Result<MicrosoftAccount, String>>,
) -> Result<Value, String> {
    require_microsoft(account, "Профиль скинов доступен только Microsoft-аккаунту")?;
    let value = match profile_data(http, account) {
        Ok(value) => value,
        Err(error) => {
            // A Minecraft token can be revoked before its local expiry timestamp.
            // retry once with a freshly exchanged token instead of an empty profile.
            let Some(refresh) = refresh else {
                return Err(error);
            };
            if !error.contains("HTTP 401") {
                return Err(error);
            }
            let account = refresh()?;
            if account.access_token.is_empty() {
                return Err("Не удалось обновить Microsoft-аккаунт для загрузки скинов".to_owned());
            }
            profile_data(http, &account)?
        }
    };
    let mut value = value;
    normalize_profile_entries(&mut value);
    hydrate_profile_images(http, &mut value);
    Ok(value)
}

fn profile_data(http: &HttpClient, account: &MicrosoftAccount) -> Result<Value, String> {
    let value = http.get_json(PROFILE_URL, Some(&bearer_headers(account)))?;
    if !value.is_object() {
        return Err("Minecraft Services вернул некорректный профиль".to_owned());
    }
    Ok(value)
}

fn normalize_profile_entries(profile: &mut Value) {
    let Some(object) = profile.as_object_mut() else {
        return;
    };
    for key in ["skins", "capes"] {
        let entries = object.get(key).cloned().unwrap_or(Value::Null);
        let Value::Array(items) = entries else {
            object.insert(key.into(), json!([]));
            continue;
        };
        let mut normalized: Vec<Value> = Vec::new();
        let mut active_index: Option<usize> = None;
        for raw in items {
            if !raw.is_object() {
                continue;
            }
            let state = field_str(&raw, "state").trim().to_uppercase();
            if state == "ACTIVE" && active_index.is_none() {
                active_index = Some(normalized.len());
            }
            let mut entry = raw;
            let alias = field_str(&entry, "alias").trim().to_owned();
            let name = field_str(&entry, "name").trim().to_owned();
            if key == "capes" && !alias.is_empty() && name.is_empty() {
                if let Some(object) = entry.as_object_mut() {
                    object.insert("name".into(), json!(alias));
                }
            }
            normalized.push(entry);
        }
        // плащ может быть не надет вовсе (все INACTIVE) — тогда ничего не
        // «надеваем» за пользователя; угадываем только если API не прислал state.
        let states_reported = normalized
            .iter()
            .any(|item| !field_str(item, "state").trim().is_empty());
        if !normalized.is_empty() && active_index.is_none() && (key == "skins" || !states_reported)
        {
            if let Some(first) = normalized[0].as_object_mut() {
                first.insert("state".into(), json!("ACTIVE"));
            }
        }
        object.insert(key.into(), Value::Array(normalized));
    }
}

/// same-origin data-URL, чтобы WebGL-превью не блокировал CORS.
fn hydrate_profile_images(http: &HttpClient, profile: &mut Value) {
    for key in ["skins", "capes"] {
        let Some(Value::Array(entries)) = profile.get_mut(key) else {
            continue;
        };
        for entry in entries.iter_mut() {
            let url = field_str(entry, "url").trim().to_owned();
            let secure = secure_texture_url(&url);
            if secure.is_empty() {
                continue;
            }
            // даже если загрузка не удастся, интерфейс получит https-адрес:
            // http:// блокируется CSP, и превью оставалось без текстуры.
            if let Some(object) = entry.as_object_mut() {
                object.insert("url".into(), json!(secure));
            }
            let Ok(content) = http.request(&secure, "GET", None, None, Some(4 * 1024 * 1024), None)
            else {
                continue;
            };
            if key == "skins" {
                if validate_png_skin_bytes(&content).is_err() {
                    continue;
                }
            } else if !content.starts_with(PNG_SIGNATURE) {
                continue;
            }
            if let Some(object) = entry.as_object_mut() {
                object.insert(
                    "url".into(),
                    json!(format!(
                        "data:image/png;base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(content)
                    )),
                );
            }
        }
    }
}

/// minecraft Services отдаёт текстуры как http://textures.minecraft.net/…;
/// CSP требует https.
fn secure_texture_url(url: &str) -> String {
    let lowered = url.to_lowercase();
    if lowered.starts_with("http://textures.minecraft.net/")
        || lowered.starts_with("http://textures.mojang.com/")
    {
        return format!("https://{}", &url["http://".len()..]);
    }
    url.to_owned()
}

pub(crate) fn apply_cape(
    http: &HttpClient,
    account: &MicrosoftAccount,
    cape_id: &str,
) -> Result<(), String> {
    require_microsoft(account, "Плащ можно установить только на Microsoft-аккаунт")?;
    let cape_id = cape_id.trim();
    if cape_id.is_empty() {
        return Err("Выберите плащ из профиля Minecraft".to_owned());
    }
    let profile = profile_data(http, account)?;
    let capes = profile.get("capes").cloned().unwrap_or(json!([]));
    let known = capes
        .as_array()
        .map(|items| {
            items
                .iter()
                .any(|cape| field_str(cape, "id").trim() == cape_id)
        })
        .unwrap_or(false);
    if !known {
        return Err("Можно выбрать только плащ, доступный в профиле Minecraft".to_owned());
    }
    let body = json!({ "capeId": cape_id }).to_string();
    http.request(
        PROFILE_CAPE_URL,
        "PUT",
        Some(body.as_bytes()),
        Some(&[
            (
                "Authorization".to_owned(),
                format!("Bearer {}", account.access_token),
            ),
            ("Content-Type".to_owned(), "application/json".to_owned()),
        ]),
        None,
        None,
    )?;
    Ok(())
}

pub(crate) fn clear_cape(http: &HttpClient, account: &MicrosoftAccount) -> Result<(), String> {
    require_microsoft(account, "Плащ можно снять только на Microsoft-аккаунте")?;
    http.request(
        PROFILE_CAPE_URL,
        "DELETE",
        None,
        Some(&bearer_headers(account)),
        None,
        None,
    )?;
    Ok(())
}

pub(crate) fn clear_skin(http: &HttpClient, account: &MicrosoftAccount) -> Result<(), String> {
    require_microsoft(account, "Скин можно сбросить только на Microsoft-аккаунте")?;
    http.request(
        &format!("{PROFILE_SKINS_URL}/active"),
        "DELETE",
        None,
        Some(&bearer_headers(account)),
        None,
        None,
    )?;
    Ok(())
}

fn http_client(store: &super::config::ConfigStore, request: &Value) -> Result<HttpClient, String> {
    let cancellation = request
        .get("operation_id")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|id| std::sync::Arc::new(super::operations::CancellationToken::new(id)));
    let settings = store.settings();
    let timeout = settings
        .get("download_timeout_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(30)
        .max(1) as u32;
    let retries = settings
        .get("download_retries")
        .and_then(Value::as_i64)
        .unwrap_or(3)
        .max(1) as u32;
    HttpClient::new(
        timeout,
        retries,
        &store.str_setting("http_proxy"),
        cancellation,
    )
}

/// _microsoft_account из gui_service: строгий Microsoft-аккаунт по id
/// (или дефолт) с автообновлением токена.
fn microsoft_account(
    store: &super::config::ConfigStore,
    http: &HttpClient,
    account_id: &str,
) -> Result<MicrosoftAccount, String> {
    let roots = super::state::launcher_roots();
    let records: Vec<Value> = accounts::load_accounts_with_tokens(&roots.config_home)?
        .into_iter()
        .map(Value::Object)
        .collect();
    let wanted = account_id.trim();
    let found = if !wanted.is_empty() {
        records
            .iter()
            .find(|record| field_str(record, "account_id") == wanted)
    } else {
        records
            .iter()
            .find(|record| field_bool(record, "is_default"))
            .or_else(|| records.first())
    };
    let record = found.ok_or_else(|| "Нужен Microsoft-аккаунт".to_owned())?;
    if field_str(record, "kind") != "microsoft" {
        return Err("Нужен Microsoft-аккаунт".to_owned());
    }
    let id = field_str(record, "account_id");
    let refreshed =
        accounts::get_valid_account(&roots.config_home, store.settings(), http, &id, false)?;
    if refreshed.account_id != id {
        return Err("Microsoft-аккаунт больше не доступен".to_owned());
    }
    Ok(refreshed)
}

pub(crate) fn skins_list(_request: &Value) -> Result<Value, String> {
    let store = SkinStore::open()?;
    let folder = store.root().to_string_lossy().into_owned();
    let skins: Vec<Value> = store.list()?.iter().map(|value| value.public()).collect();
    Ok(json!({ "skins": skins, "folder": folder }))
}

pub(crate) fn skins_import(request: &Value) -> Result<Value, String> {
    let encoded = request.get("data").and_then(Value::as_str).unwrap_or("");
    let content = decode_skin_payload(encoded)?;
    if content.len() as u64 > MAX_SKIN_BYTES {
        return Err("Файл скина слишком большой".to_owned());
    }
    let file_name = request
        .get("file_name")
        .and_then(Value::as_str)
        .unwrap_or("skin.png");
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("classic");
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let value = SkinStore::open()?.import_bytes(&content, file_name, name, model)?;
    Ok(json!({ "skin": value.public() }))
}

pub(crate) fn skins_export(request: &Value) -> Result<Value, String> {
    // редактор скинов: сохранить результат как PNG-файл.
    let encoded = request.get("data").and_then(Value::as_str).unwrap_or("");
    let content = decode_skin_payload(encoded)?;
    if content.len() as u64 > MAX_SKIN_BYTES {
        return Err("Файл скина слишком большой".to_owned());
    }
    validate_png_skin_bytes(&content)?;
    let raw_name = request
        .get("file_name")
        .and_then(Value::as_str)
        .unwrap_or("skin.png")
        .trim();
    let raw_name = if raw_name.is_empty() {
        "skin.png"
    } else {
        raw_name
    };
    let stem = Path::new(raw_name)
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let non_word = regex::Regex::new(r"[^\w\-. ]+").expect("статическое регулярное выражение");
    let replaced = non_word.replace_all(&stem, "_").into_owned();
    let safe_stem: String = replaced
        .trim_matches(|c: char| c == ' ' || c == '.' || c == '_')
        .chars()
        .take(60)
        .collect::<String>()
        .trim()
        .to_owned();
    let safe_stem = if safe_stem.is_empty() {
        "skin".to_owned()
    } else {
        safe_stem
    };
    let downloads = home_dir().join("Downloads");
    let roots = super::state::launcher_roots();
    let target_dir = if downloads.is_dir() {
        downloads
    } else {
        roots.data_home
    };
    let mut target = target_dir.join(format!("{safe_stem}.png"));
    let mut index = 1;
    while target.exists() && index < 100 {
        target = target_dir.join(format!("{safe_stem}-{index}.png"));
        index += 1;
    }
    fs::write(&target, &content).map_err(|error| format!("Не удалось сохранить скин: {error}"))?;
    let mut opened = false;
    if request
        .get("open_folder")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        opened = super::platform_support::open_path(&target_dir).is_ok();
    }
    Ok(json!({
        "path": target.to_string_lossy(),
        "folder": target_dir.to_string_lossy(),
        "opened": opened,
    }))
}

fn decode_skin_payload(encoded: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| "Файл скина повреждён".to_owned())
}

pub(crate) fn skin_profile(request: &Value) -> Result<Value, String> {
    let store = super::config::ConfigStore::load()?;
    let http = http_client(&store, request)?;
    let account_id = request
        .get("account_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let account = microsoft_account(&store, &http, account_id)?;
    // при 401 токен обновляется принудительно и профиль читается повторно.
    let refresh_account_id = account.account_id.clone();
    let refresh = || -> Result<MicrosoftAccount, String> {
        accounts::get_valid_account(
            &super::state::launcher_roots().config_home,
            store.settings(),
            &http,
            &refresh_account_id,
            true,
        )
    };
    profile(&http, &account, Some(&refresh))
}

pub(crate) fn apply_skin_op(request: &Value) -> Result<Value, String> {
    let store = super::config::ConfigStore::load()?;
    let http = http_client(&store, request)?;
    let account = microsoft_account(
        &store,
        &http,
        request
            .get("account_id")
            .and_then(Value::as_str)
            .unwrap_or(""),
    )?;
    let store_skins = SkinStore::open()?;
    let skin_id = request.get("skin_id").and_then(Value::as_str).unwrap_or("");
    let skin = store_skins.get(skin_id)?;
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&skin.model);
    apply_skin(&http, &account, &skin, model)?;
    Ok(json!({ "ok": true }))
}

pub(crate) fn clear_skin_op(request: &Value) -> Result<Value, String> {
    let store = super::config::ConfigStore::load()?;
    let http = http_client(&store, request)?;
    let account = microsoft_account(
        &store,
        &http,
        request
            .get("account_id")
            .and_then(Value::as_str)
            .unwrap_or(""),
    )?;
    clear_skin(&http, &account)?;
    Ok(json!({ "ok": true }))
}

pub(crate) fn apply_cape_op(request: &Value) -> Result<Value, String> {
    let store = super::config::ConfigStore::load()?;
    let http = http_client(&store, request)?;
    let account = microsoft_account(
        &store,
        &http,
        request
            .get("account_id")
            .and_then(Value::as_str)
            .unwrap_or(""),
    )?;
    apply_cape(
        &http,
        &account,
        request.get("cape_id").and_then(Value::as_str).unwrap_or(""),
    )?;
    Ok(json!({ "ok": true }))
}

pub(crate) fn clear_cape_op(request: &Value) -> Result<Value, String> {
    let store = super::config::ConfigStore::load()?;
    let http = http_client(&store, request)?;
    let account = microsoft_account(
        &store,
        &http,
        request
            .get("account_id")
            .and_then(Value::as_str)
            .unwrap_or(""),
    )?;
    clear_cape(&http, &account)?;
    Ok(json!({ "ok": true }))
}

pub(crate) fn open_skins_folder(_request: &Value) -> Result<Value, String> {
    let root = SkinStore::open()?.root().to_owned();
    super::platform_support::open_path(&root)
        .map_err(|error| format!("Не удалось открыть папку скинов: {error}"))?;
    Ok(json!({ "opened": true }))
}

pub(crate) fn skins_folder(_request: &Value) -> Result<Value, String> {
    // путь нужен встроенному проводнику лаунчера, а не окну ОС.
    let root = SkinStore::open()?;
    Ok(json!({ "path": root.root().to_string_lossy() }))
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-skins-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// минимальный валидный заголовок PNG-скина с указанными размерами.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 13]); // длина чанка
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]); // битность/цвет/...
        bytes
    }

    #[test]
    fn png_validation_accepts_supported_sizes_only() {
        for (width, height) in [(64u32, 32u32), (64, 64), (128, 64), (128, 128)] {
            assert!(
                validate_png_skin_bytes(&png_header(width, height)).is_ok(),
                "{width}x{height}"
            );
        }
        assert!(validate_png_skin_bytes(&png_header(256, 256)).is_err());
        assert!(validate_png_skin_bytes(&png_header(64, 33)).is_err());
        let mut broken = png_header(64, 64);
        broken[14] = b'X'; // не IHDR
        assert!(validate_png_skin_bytes(&broken).is_err());
        assert!(validate_png_skin_bytes(b"not a png at all, really").is_err());
        assert!(validate_png_skin_bytes(&png_header(64, 64)[..20]).is_err());
    }

    #[test]
    fn safe_path_rejects_traversal_and_symlinks() {
        let root = temp_root("safe");
        for name in [
            "a/b.png",
            "../escape.png",
            "/abs.png",
            ".",
            "..",
            "skin.txt",
            "",
        ] {
            assert!(safe_skin_path(&root, name).is_none(), "имя: {name}");
        }
        let good = root.join("ok.png");
        fs::write(&good, b"x").unwrap();
        assert!(safe_skin_path(&root, "ok.png").is_some());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&good, root.join("link.png")).unwrap();
            assert!(
                safe_skin_path(&root, "link.png").is_none(),
                "симлинк отклоняется"
            );
        }
    }

    #[test]
    fn import_and_list_roundtrip() {
        let root = temp_root("roundtrip");
        let store = SkinStore::at_root(&root).expect("создать хранилище");
        let png = png_header(64, 64);
        let imported = store
            .import_bytes(&png, "Cool Skin.png", Some("  Мой скин  "), "slim")
            .expect("импорт");
        assert_eq!(imported.model, "slim");
        assert_eq!(imported.name, "Мой скин");
        assert!(imported.skin_id.len() == 12);
        assert!(imported.image.starts_with("data:image/png;base64,"));
        let listed = store.list().expect("список");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].skin_id, imported.skin_id);
        // модель по умолчанию и неизвестная модель
        let classic = store
            .import_bytes(&png, "plain.png", None, "classic")
            .unwrap();
        assert_eq!(classic.model, "classic");
        assert!(store.import_bytes(&png, "x.png", None, "hd").is_err());
        // индекс на диске — список объектов с filename
        let raw = fs::read_to_string(root.join("skins.json")).unwrap();
        assert!(raw.contains("\"filename\""));
    }

    #[test]
    fn secure_texture_url_upgrades_minecraft_hosts() {
        assert_eq!(
            secure_texture_url("http://textures.minecraft.net/abc"),
            "https://textures.minecraft.net/abc"
        );
        assert_eq!(
            secure_texture_url("http://textures.mojang.com/x"),
            "https://textures.mojang.com/x"
        );
        assert_eq!(
            secure_texture_url("https://example.net/y"),
            "https://example.net/y"
        );
        assert_eq!(secure_texture_url("http://evil.net/z"), "http://evil.net/z");
    }

    #[test]
    fn profile_normalization_picks_active_and_cape_alias() {
        let mut value = json!({
            "skins": [
                { "id": "a", "state": "ACTIVE" },
                { "id": "b", "state": "INACTIVE" },
                "мусор",
            ],
            "capes": [
                { "id": "c1", "alias": "Migrator", "state": "INACTIVE" },
                { "id": "c2", "name": "Founder", "state": "" },
            ],
        });
        normalize_profile_entries(&mut value);
        let skins = value["skins"].as_array().unwrap();
        assert_eq!(skins.len(), 2);
        assert_eq!(skins[0]["state"], "ACTIVE");
        let capes = value["capes"].as_array().unwrap();
        assert_eq!(capes[0]["name"], "Migrator");
        // один из плащей прислал state — угадывание не срабатывает
        assert_eq!(capes[1]["state"], json!(""));
        // ни один плащ не прислал state — угадываем первый
        let mut value = json!({ "capes": [{ "id": "x" }, { "id": "y", "state": "" }] });
        normalize_profile_entries(&mut value);
        assert_eq!(value["capes"][0]["state"], "ACTIVE");
        assert_eq!(value["capes"][1]["state"], json!(""));
        // все INACTIVE у плащей — ничего не «надеваем»
        let mut value = json!({ "capes": [{ "id": "c", "state": "INACTIVE" }] });
        normalize_profile_entries(&mut value);
        assert_eq!(value["capes"][0]["state"], "INACTIVE");
        // не-список превращается в пустой список
        let mut value = json!({ "skins": 42 });
        normalize_profile_entries(&mut value);
        assert_eq!(value["skins"], json!([]));
    }
}
