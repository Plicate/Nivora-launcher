use super::*;
use super::content::BOL_CONTENT_ROOT_PARTS;

pub(super) fn profile_name_re() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^nivora-[0-9a-f]{16}\z").unwrap());
    &RE
}

/// внутренний путь BoL к данным игры относительно профиля

pub(super) fn profile_name(instance_name: &str) -> String {
    format!("nivora-{}", &sha256_hex(instance_name.as_bytes())[..16])
}

pub(super) struct ProfilePaths {
    pub(super) root: PathBuf,
    pub(super) profiles_root: PathBuf,
}

pub(super) fn profile_paths(ctx: &Ctx) -> Result<ProfilePaths, String> {
    let root = ctx.game_dir.join("bedrock");
    let profiles = root.join("profiles");
    if is_reparse_point(&root) || is_reparse_point(&profiles) {
        return Err("Каталог Bedrock является символьной ссылкой".to_owned());
    }
    fs::create_dir_all(&profiles).map_err(|_| "Каталог Bedrock недоступен".to_owned())?;
    if is_symlink_or_missing_dir(&root) || is_symlink_or_missing_dir(&profiles) {
        return Err("Каталог Bedrock недоступен".to_owned());
    }
    super::platform::restrict_dir(&root);
    super::platform::restrict_dir(&profiles);
    Ok(ProfilePaths { root, profiles_root: profiles })
}

fn validate_profile_path(paths: &ProfilePaths, profile_name: &str) -> Result<PathBuf, String> {
    if !profile_name_re().is_match(profile_name) {
        return Err("Недопустимый идентификатор профиля Bedrock".to_owned());
    }
    let profile = paths.profiles_root.join(profile_name);
    if is_reparse_point(&profile) || !profile.is_dir() {
        return Err("Профиль Bedrock не найден".to_owned());
    }
    match (profile.canonicalize(), paths.profiles_root.canonicalize()) {
        (Ok(resolved), Ok(root)) if resolved.starts_with(&root) => {}
        _ => return Err("Путь профиля Bedrock выходит за пределы хранилища".to_owned()),
    }
    let metadata = profile.join("profile.json");
    if is_reparse_point(&metadata) || !metadata.is_file() {
        return Err("Метаданные профиля Bedrock не найдены".to_owned());
    }
    let metadata =
        read_bounded_json(&metadata, 64 * 1024).ok_or_else(|| "Не удалось прочитать профиль Bedrock".to_owned())?;
    if metadata.get("slug").and_then(Value::as_str) != Some(profile_name) {
        return Err("Идентификатор профиля Bedrock не совпадает".to_owned());
    }
    Ok(profile)
}

pub(super) fn profile_settings(profile: &Path) -> Value {
    read_bounded_json(&profile.join("settings.json"), 1024 * 1024).unwrap_or(Value::Null)
}

pub(super) fn selected_version(profile: &Path) -> (String, String) {
    let settings = profile_settings(profile);
    if !settings.is_object() {
        return (String::new(), String::new());
    }
    let edition = settings
        .get("mc_edition")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let version = settings.get("mc_version").and_then(Value::as_str).unwrap_or("").trim().to_owned();
    let valid_edition = edition == "release" || edition == "preview";
    if !valid_edition || !game_version_re().is_match(&version) {
        return (String::new(), String::new());
    }
    (edition, version)
}

pub(super) fn engine_ready(profile: &Path) -> bool {
    // без выбранного Proton запуск BoL завершается ошибкой "GDK-Proton missing"
    let proton = profile_settings(profile)
        .get("proton")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_owned();
    if proton.is_empty() {
        return false;
    }
    Path::new(&proton).is_dir()
}

pub(super) fn store_status(profile: &Path) -> (bool, String) {
    let mut signed_in = false;
    let mut gamertag = String::new();
    if let Some(token) = read_bounded_json(&profile.join("msa").join("token.json"), 1024 * 1024) {
        if token.get("refresh_token").is_some() {
            signed_in = true;
        }
    }
    if let Some(device) = read_bounded_json(&profile.join("winegdk-preauth").join("device.json"), 1024 * 1024) {
        if let Some(tag) = device.get("xbl_gamertag").and_then(Value::as_str) {
            gamertag = tag.to_owned();
        }
    }
    (signed_in, gamertag)
}

pub fn store_login(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?.to_owned();
    let store = instance_store()?;
    let info = bedrock_instance(&store, &name)?;
    if info.running {
        return Err("Сначала закройте Minecraft Bedrock".to_owned());
    }
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => {
            let job_name = name.clone();
            spawn_job("login", &name, move |job, cancellation| {
                match linux_store_login(&job_name, Some(cancellation), &job) {
                    Ok(value) => job_finish(&job, value),
                    Err(error) => job_fail(&job, &error, cancellation.cancelled()),
                }
            })
        }
        BedrockPlatform::WindowsOfficial => spawn_job("login", &name, move |job, _cancellation| {
            job_log(&job, "Официальный Minecraft for Windows выполняет вход внутри игры:");
            job_log(&job, "запустите игру и войдите в аккаунт Microsoft/Xbox — Nivora ничего не хранит.");
            job_finish(
                &job,
                json!({
                    "exit_code": 0,
                    "logs": [],
                    "logs_tail": [],
                    "error_hint": "",
                    "failed": false,
                }),
            )
        }),
        BedrockPlatform::Unsupported => Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
}

fn linux_store_login(
    name: &str,
    cancellation: Option<&CancellationToken>,
    job: &Arc<Mutex<JobState>>,
) -> Result<Value, String> {
    let ctx = context()?;
    let store = instance_store()?;
    let info = bedrock_instance(&store, name)?;
    let profile = linux_profile_path(&ctx, &info)?;
    let mut logger = |line: &str| job_log(job, line);
    let (code, logs) = bol_run(
        &["store-login".to_owned()],
        &profile,
        &format!("bedrock-login:{name}"),
        cancellation,
        Some(&mut logger),
        Duration::ZERO,
        500,
        false,
        None,
        None,
        None,
    )?;
    let error_hint: String = logs
        .iter()
        .rev()
        .map(|line| line.trim())
        .find(|line| !line.is_empty() && bol_error_line_re().is_match(line))
        .unwrap_or("")
        .chars()
        .take(300)
        .collect();
    let tail_start = logs.len().saturating_sub(40);
    Ok(json!({
        "exit_code": code,
        "logs": logs.clone(),
        "logs_tail": logs[tail_start..],
        "error_hint": error_hint,
        "failed": !error_hint.is_empty() || code != 0,
    }))
}

pub fn store_login_status(request: &Value) -> Result<Value, String> {
    Ok(job_snapshot("login", request_field(request, "name")?))
}

pub(super) fn bol_profile_signed_in(profile: &Path) -> bool {
    keyring_file_signed_in(&profile.join("xodus-home").join(".xodus-keyring.ron"))
}

pub(super) fn keyring_file_signed_in(keyring: &Path) -> bool {
    const MAX_KEYRING_BYTES: u64 = 1024 * 1024;
    let Ok(meta) = fs::metadata(keyring) else { return false };
    if !meta.is_file() || meta.len() > MAX_KEYRING_BYTES {
        return false;
    }
    let Ok(blob) = fs::read(keyring) else { return false };
    let contains = |marker: &[u8]| blob.windows(marker.len()).any(|window| window == marker);
    contains(b"\"user-tokens\"") && contains(b"\"user-DA\"")
}

pub(super) fn share_xodus_keyring(paths: &ProfilePaths) {
    let shared = paths.root.join("xodus-home");
    if xodus_is_symlink(&shared) || fs::create_dir_all(&shared).is_err() {
        return;
    }
    let shared_keyring = shared.join(".xodus-keyring.ron");
    let mut shared_signed = keyring_file_signed_in(&shared_keyring);
    let Ok(entries) = fs::read_dir(&paths.profiles_root) else { return };
    for entry in entries.flatten() {
        let profile = entry.path();
        if xodus_is_symlink(&profile) || !profile.is_dir() {
            continue;
        }
        let local = profile.join("xodus-home");
        if xodus_is_symlink(&local) {
            continue; // уже на общем доме
        }
        if !local.try_exists().unwrap_or(false) {
            let _ = link_shared_xodus(&shared, &local);
            continue;
        }
        if !local.is_dir() {
            continue;
        }
        let local_keyring = local.join(".xodus-keyring.ron");
        // переносим первый обнаруженный вход в общее хранилище
        if !shared_signed && keyring_file_signed_in(&local_keyring) {
            if fs::rename(&local_keyring, &shared_keyring).is_ok() {
                shared_signed = true;
            }
        }
        let retired = profile.join("xodus-home.local");
        if !retired.exists() {
            let _ = fs::rename(&local, &retired);
        } else {
            let _ = fs::remove_dir_all(&local);
        }
        let _ = link_shared_xodus(&shared, &local);
    }
}

fn link_shared_xodus(shared: &Path, local: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(shared, local)
    }
    #[cfg(not(unix))]
    {
        let _ = (shared, local);
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "символические ссылки недоступны"))
    }
}

/// проверяет символические ссылки в Unix и reparse points в Windows
fn xodus_is_symlink(path: &Path) -> bool {
    #[cfg(unix)]
    {
        fs::symlink_metadata(path).map(|meta| meta.file_type().is_symlink()).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        is_reparse_point(path)
    }
}

pub(super) const BEDROCK_LOGIN_REQUIRED_MESSAGE: &str = "Для запуска Bedrock нужен подключённый аккаунт Microsoft, которому принадлежит игра. Нажмите «Подключить аккаунт» в карточке «Лицензия Microsoft Store» на вкладке «Обзор» и запустите игру снова.";

pub fn account_status(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?;
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => {
            let ctx = context()?;
            let store = instance_store()?;
            let info = bedrock_instance(&store, &name)?;
            let profile = linux_profile_path(&ctx, &info)?;
            Ok(json!({
                "signed_in": bol_profile_signed_in(&profile),
                "platform": "linux-bol",
            }))
        }
        BedrockPlatform::WindowsOfficial => Ok(json!({
            "signed_in": true,
            "platform": "windows-official",
        })),
        BedrockPlatform::Unsupported => Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
}

pub fn store_login_cancel(request: &Value) -> Result<Value, String> {
    let name = request_field(request, "name")?.to_owned();
    let stopped_job = cancel_job("login", &name);
    let stopped_process = stop_registered_process(&format!("bedrock-login:{name}"));
    Ok(json!({ "stopped": stopped_job || stopped_process }))
}

pub fn create_instance(request: &Value) -> Result<Value, String> {
    let raw_name = request_field(request, "name")?;
    let display_name = request_field(request, "display_name").unwrap_or_default().trim().to_owned();
    let instance_name = safe_instance_name(raw_name);
    if instance_name.is_empty() || instance_name == "." || instance_name == ".." {
        return Err("Укажите имя экземпляра Bedrock".to_owned());
    }
    let ctx = context()?;
    let store = instance_store()?;
    let instance_path = store.root().join(&instance_name);
    if instance_path.exists() {
        return Err(format!("Экземпляр уже существует: {instance_name}"));
    }
    let pretty_name = if display_name.is_empty() { instance_name.clone() } else { display_name.chars().take(80).collect() };
    match bedrock_platform() {
        BedrockPlatform::LinuxBol => {
            let paths = profile_paths(&ctx)?;
            bol_require_runtime(&paths.root)?;
            // новый экземпляр сразу получает доступ к общему входу Microsoft
            share_xodus_keyring(&paths);
            let profile = profile_name(&instance_name);
            let profile_path = paths.profiles_root.join(&profile);
            if profile_path.exists() {
                return Err("Для этого имени уже существует профиль Bedrock; сначала удалите его в библиотеке".to_owned());
            }
            let shortcut_storage = paths.root.join(".nivora-xdg-data");
            fs::create_dir_all(&shortcut_storage).map_err(|_| "Каталог ярлыков Bedrock недоступен".to_owned())?;
            let (code, _logs) = bol_run(
                &["profiles".to_owned(), "create".to_owned(), profile.clone()],
                &paths.root,
                "profiles-create",
                None,
                None,
                Duration::from_secs(120),
                100,
                true,
                Some(&shortcut_storage),
                None,
                None,
            )?;
            if code != 0 {
                return Err(format!("Не удалось создать профиль Bedrock (код {code})"));
            }
            if let Err(error) = validate_profile_path(&paths, &profile) {
                let _ = fs::remove_dir_all(&profile_path);
                return Err(error);
            }
            let metadata = json!({
                "kind": "client",
                "game_edition": "bedrock",
                "bedrock_profile": profile,
                "display_name": pretty_name,
                "description": "Minecraft Bedrock Edition",
                "group": "",
                "played_seconds": 0,
            });
            fs::create_dir_all(&instance_path).map_err(|_| "Не удалось создать каталог экземпляра".to_owned())?;
            super::platform::restrict_dir(&instance_path);
            if let Err(error) = write_instance_metadata(&instance_path, &metadata) {
                let _ = fs::remove_dir_all(&instance_path);
                let _ = fs::remove_dir_all(&profile_path);
                return Err(error);
            }
        }
        BedrockPlatform::WindowsOfficial => {
            // для официальной версии создаём только экземпляр, без профиля среды
            let metadata = json!({
                "kind": "client",
                "game_edition": "bedrock",
                "bedrock_profile": "",
                "display_name": pretty_name,
                "description": "Minecraft Bedrock Edition (Microsoft Store)",
                "group": "",
                "played_seconds": 0,
            });
            fs::create_dir_all(&instance_path).map_err(|_| "Не удалось создать каталог экземпляра".to_owned())?;
            super::platform::restrict_dir(&instance_path);
            write_instance_metadata(&instance_path, &metadata)?;
        }
        BedrockPlatform::Unsupported => return Err("Minecraft Bedrock не поддерживается на этой ОС".to_owned()),
    }
    let refreshed = instance_store()?;
    let info = bedrock_instance(&refreshed, &instance_name)?;
    Ok(json!({
        "instance": info.public(),
        "instances": refreshed.list().iter().map(|item| item.public()).collect::<Vec<Value>>(),
    }))
}

/// возвращает профиль BoL экземпляра в Linux
pub(super) fn linux_profile_path(ctx: &Ctx, info: &InstanceInfo) -> Result<PathBuf, String> {
    let expected = profile_name(&info.name);
    if info.bedrock_profile != expected {
        return Err("Профиль Bedrock не совпадает с экземпляром".to_owned());
    }
    let paths = profile_paths(ctx)?;
    // сводит локальные keyring к общему хранилищу при каждом обращении
    share_xodus_keyring(&paths);
    validate_profile_path(&paths, &expected)
}

/// удаляет профиль только в Linux и только если он не используется
/// данные официальной версии Windows здесь не удаляются
pub(crate) fn delete_profile(info: &InstanceInfo) -> Result<(), String> {
    if info.game_edition != "bedrock" {
        return Ok(());
    }
    if cfg!(target_os = "windows") {
        return Ok(());
    }
    let profile_name = profile_name(&info.name);
    if info.bedrock_profile != profile_name {
        return Err("Профиль Bedrock не совпадает с экземпляром".to_owned());
    }
    let ctx = context()?;
    let paths = profile_paths(&ctx)?;
    let profile_path = paths.profiles_root.join(&profile_name);
    if fs::symlink_metadata(&profile_path).is_err() {
        return Ok(());
    }
    let profile_path = validate_profile_path(&paths, &profile_name)?;
    require_profile_idle(&profile_path)?;
    remove_profile_path(&paths, &profile_path, &profile_name)
}

/// отклоняет удаление профиля, если его заняли процесс или lock-файл
#[cfg(unix)]
fn require_profile_idle(profile_path: &Path) -> Result<(), String> {
    let resolved = profile_path.canonicalize().unwrap_or_else(|_| profile_path.to_path_buf());
    let prefix = resolved.join("compatdata").join("pfx");
    let bol_home = format!("BOL_HOME={}", resolved.display());
    let wine_prefix = format!("WINEPREFIX={}", prefix.display());
    let self_pid = std::process::id().to_string();
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.filter_map(|item| item.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.is_empty() || !name.chars().all(|character| character.is_ascii_digit()) || name == self_pid {
                continue;
            }
            let Ok(raw) = fs::read(format!("/proc/{name}/environ")) else { continue };
            let used = raw
                .split(|byte| *byte == 0)
                .any(|item| item == bol_home.as_bytes() || item == wine_prefix.as_bytes());
            if used {
                return Err("Профиль Minecraft Bedrock ещё используется".to_owned());
            }
        }
    }
    let lock_path = profile_path.join(".launch.lock");
    if fs::symlink_metadata(&lock_path).map(|meta| meta.is_symlink()).unwrap_or(false) {
        return Err("Блокировка профиля Bedrock является ссылкой".to_owned());
    }
    if !lock_path.exists() {
        return Ok(());
    }
    use std::os::unix::io::AsRawFd;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|_| "Не удалось проверить блокировку профиля Bedrock".to_owned())?;
    let taken = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0;
    if taken {
        return Err("Профиль Bedrock используется запущенной игрой".to_owned());
    }
    unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) };
    Ok(())
}

#[cfg(not(unix))]
fn require_profile_idle(_profile_path: &Path) -> Result<(), String> {
    Err("Управление профилем Minecraft Bedrock доступно только в Linux".to_owned())
}

fn remove_profile_path(paths: &ProfilePaths, profile_path: &Path, profile_name: &str) -> Result<(), String> {
    let data_root = paths.root.join(".nivora-xdg-data").join("applications");
    let shortcuts = [
        data_root.join(format!("bedrock-on-linux-profile-{profile_name}.desktop")),
        data_root.join(format!("bedrock-on-linux-play-{profile_name}.desktop")),
    ];
    for shortcut in &shortcuts {
        if fs::symlink_metadata(shortcut).map(|meta| meta.is_symlink()).unwrap_or(false) {
            return Err("Ярлык профиля Bedrock является ссылкой".to_owned());
        }
    }
    fs::remove_dir_all(profile_path).map_err(|error| format!("Не удалось удалить профиль Bedrock: {error}"))?;
    for shortcut in &shortcuts {
        if shortcut.is_file() {
            let _ = fs::remove_file(shortcut);
        }
    }
    Ok(())
}

/// возвращает каталог `com.mojang` для снимков Bedrock
pub(crate) fn snapshot_root_for(info: &InstanceInfo) -> Result<PathBuf, String> {
    if info.game_edition != "bedrock" {
        return Err("Операция доступна только для Minecraft Bedrock".to_owned());
    }
    if cfg!(target_os = "windows") {
        let (edition, _) = windows_selected(&info.path);
        let edition = if edition.is_empty() { "release".to_owned() } else { edition };
        return win_content_root(&edition);
    }
    let ctx = context()?;
    let profile = linux_profile_path(&ctx, info)?;
    let users_root: PathBuf = profile.join(BOL_CONTENT_ROOT_PARTS.iter().collect::<PathBuf>());
    if users_root.is_dir() {
        if let Ok(entries) = fs::read_dir(&users_root) {
            let mut scopes: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| !is_reparse_point(path) && path.is_dir())
                .collect();
            scopes.sort_by_key(|path| path.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default());
            for scope in scopes {
                let name = scope.file_name().map(|value| value.to_string_lossy().into_owned()).unwrap_or_default();
                if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                    continue;
                }
                let candidate = scope.join("games").join("com.mojang");
                if candidate.is_dir() {
                    return Ok(candidate);
                }
            }
        }
    }
    Ok(profile.join("games").join("com.mojang"))
}

/// возвращает корень данных игры Bedrock
pub(crate) fn profile_path_for(info: &InstanceInfo) -> Result<PathBuf, String> {
    if info.game_edition != "bedrock" {
        return Err("Операция доступна только для Minecraft Bedrock".to_owned());
    }
    if cfg!(target_os = "windows") {
        let (edition, _) = windows_selected(&info.path);
        let edition = if edition.is_empty() { "release".to_owned() } else { edition };
        return win_content_root(&edition);
    }
    let ctx = context()?;
    linux_profile_path(&ctx, info)
}
