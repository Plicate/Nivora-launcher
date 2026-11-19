// апгрейд-мастер версий: план обновления экземпляра и его применение
// обновление не трогает миры, моды, конфиги и ресурсы: меняются только файлы версии,
// а несовместимые моды заранее переезжают в карантинную папку внутри экземпляра
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::config::ConfigStore;
use super::installer::MinecraftInstaller;
use super::instances::{loader_version_of, InstanceStore};

const MAX_MOD_ENTRIES: usize = 600;
const MOD_SIDECAR_SUFFIX: &str = ".trial-project.json";

#[derive(Clone, Copy, PartialEq)]
enum Verdict {
    Ok,
    Incompatible,
    Unknown,
}

#[derive(Clone, Copy, PartialEq)]
enum Operator {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Tilde,
    Caret,
}

fn manifest_field(manifest: &Value, key: &str) -> String {
    manifest
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// разбирает «1.21.4» в числовой кортеж; снапшоты и пре-релизы не сравниваются численно
fn parse_mc_version(value: &str) -> Option<Vec<u64>> {
    let base = value.trim().to_lowercase();
    if base.ends_with('+') || base.ends_with('-') {
        return None;
    }
    let base = base.split(['-', '+', ' ']).next().unwrap_or_default();
    if base.is_empty() || base.chars().any(|c| c.is_alphabetic()) {
        return None;
    }
    let parts: Option<Vec<u64>> = base
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect();
    parts.filter(|parts| !parts.is_empty() && parts.len() <= 4)
}

fn compare_versions(reference: &[u64], target: &[u64]) -> Ordering {
    let length = reference.len().max(target.len());
    for index in 0..length {
        let left = reference.get(index).copied().unwrap_or(0);
        let right = target.get(index).copied().unwrap_or(0);
        if left != right {
            return left.cmp(&right);
        }
    }
    Ordering::Equal
}

fn prefix_matches(reference: &[u64], target: &[u64], depth: usize) -> bool {
    for index in 0..depth {
        let left = reference.get(index).copied().unwrap_or(0);
        let right = target.get(index).copied().unwrap_or(0);
        if left != right {
            return false;
        }
    }
    true
}

fn parse_operator(part: &str) -> (Operator, &str) {
    for (prefix, operator) in [
        (">=", Operator::Ge),
        ("<=", Operator::Le),
        ("==", Operator::Eq),
        ("!=", Operator::Ne),
        (">", Operator::Gt),
        ("<", Operator::Lt),
        ("~", Operator::Tilde),
        ("^", Operator::Caret),
        ("=", Operator::Eq),
    ] {
        if let Some(rest) = part.strip_prefix(prefix) {
            return (operator, rest);
        }
    }
    (Operator::Eq, part)
}

/// проверяет одно ограничение семвера мода против целевой версии
/// None значит «не удалось понять»: помечаем мод как требующий проверки вручную
fn constraint_allows(constraint: &str, target: &str) -> Option<bool> {
    let constraint = constraint.trim();
    if constraint.is_empty() || constraint == "*" || constraint == "любая" {
        return Some(true);
    }
    let mut parts = constraint.split_whitespace();
    let first = parts.next()?;
    if parts.next().is_some() {
        // диапазон вида ">=1.20 <1.22": выполняться должны все части
        let mut result = Some(true);
        for part in constraint.split_whitespace() {
            let verdict = constraint_allows(part, target)?;
            result = Some(result? && verdict);
        }
        return result;
    }
    let (operator, rest) = parse_operator(first);
    if rest.trim().is_empty() {
        return Some(true);
    }
    let reference = parse_mc_version(rest)?;
    let target_parts = parse_mc_version(target)?;
    if operator == Operator::Tilde {
        // ~1.21: та же пара major.minor
        return Some(prefix_matches(&reference, &target_parts, 2));
    }
    if operator == Operator::Caret {
        // ^1.21: тот же major
        return Some(prefix_matches(&reference, &target_parts, 1));
    }
    let ordering = compare_versions(&reference, &target_parts);
    Some(match operator {
        Operator::Eq => ordering == Ordering::Equal,
        Operator::Ne => ordering != Ordering::Equal,
        Operator::Gt => ordering == Ordering::Less,
        Operator::Ge => ordering != Ordering::Greater,
        Operator::Lt => ordering == Ordering::Greater,
        Operator::Le => ordering != Ordering::Less,
        Operator::Tilde | Operator::Caret => unreachable!(),
    })
}

/// достаёт из манифеста мода ограничение на версию Minecraft, если оно заявлено
fn minecraft_constraint(manifest: &Value) -> Option<String> {
    let dependencies = manifest.get("dependencies")?.as_array()?;
    for dependency in dependencies {
        let id = dependency
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !id.eq_ignore_ascii_case("minecraft") {
            continue;
        }
        let constraint = dependency
            .get("constraint")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if constraint.is_empty() || constraint == "любая" {
            return None;
        }
        return Some(constraint.to_owned());
    }
    None
}

fn directory_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack: Vec<PathBuf> = vec![path.to_path_buf()];
    let mut visited = 0usize;
    while let Some(directory) = stack.pop() {
        visited += 1;
        if visited > 200_000 {
            break;
        }
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_symlink() {
                continue;
            }
            if entry_path.is_dir() {
                stack.push(entry_path);
            } else if let Ok(meta) = entry.metadata() {
                total += meta.len();
            }
        }
    }
    total
}

/// миры экземпляра: папки в saves с level.dat
fn world_entries(instance_path: &Path) -> Vec<Value> {
    let saves = instance_path.join("saves");
    let mut worlds = Vec::new();
    let Ok(entries) = fs::read_dir(&saves) else {
        return worlds;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if fs::symlink_metadata(&path)
            .map(|meta| meta.is_symlink())
            .unwrap_or(true)
        {
            continue;
        }
        if !path.is_dir() || !path.join("level.dat").is_file() {
            continue;
        }
        worlds.push(json!({
            "name": path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default(),
            "size_bytes": directory_size(&path),
        }));
    }
    worlds.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    worlds
}

/// имена ресурсов в папке: архивы и подпапки, скрытые и выключенные пропускаем
fn collect_resource_names(path: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(entries) = fs::read_dir(path) else {
        return names;
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        if fs::symlink_metadata(&entry_path)
            .map(|meta| meta.is_symlink())
            .unwrap_or(true)
        {
            continue;
        }
        let name = entry_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.is_empty() || name.starts_with('.') || name.ends_with(".disabled") {
            continue;
        }
        let lower = name.to_lowercase();
        if entry_path.is_dir() || lower.ends_with(".zip") {
            names.push(name);
        }
    }
    names.sort();
    names
}

/// считает датапаки по всем мирам экземпляра
fn datapack_count(instance_path: &Path) -> usize {
    let mut total = 0usize;
    for world in world_entries(instance_path) {
        let name = world
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        total +=
            collect_resource_names(&instance_path.join("saves").join(name).join("datapacks")).len();
    }
    total
}

/// раскладывает активные моды экземпляра по совместимости с целевой версией
fn scan_mod_compatibility(mods_dir: &Path, target_version: &str) -> Value {
    let mut ok = Vec::new();
    let mut incompatible = Vec::new();
    let mut unknown = Vec::new();
    let mut active = 0usize;
    let mut disabled = 0usize;
    let Ok(entries) = fs::read_dir(mods_dir) else {
        return json!({
            "active": 0,
            "disabled": 0,
            "ok": [],
            "incompatible": [],
            "unknown": [],
        });
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if fs::symlink_metadata(&path)
            .map(|meta| meta.is_symlink())
            .unwrap_or(true)
        {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let lower = name.to_lowercase();
        if !(lower.ends_with(".jar") || lower.ends_with(".zip")) {
            continue;
        }
        if lower.ends_with(".disabled") {
            disabled += 1;
            continue;
        }
        active += 1;
        if active > MAX_MOD_ENTRIES {
            break;
        }
        let manifest = super::mod_dependencies::read_mod_manifest(&path);
        let (verdict, reason) = match manifest.as_ref() {
            None => (Verdict::Unknown, "манифест мода не распознан".to_owned()),
            Some(manifest) => match minecraft_constraint(manifest).as_deref() {
                None => (
                    Verdict::Unknown,
                    "мод не заявляет поддерживаемую версию Minecraft".to_owned(),
                ),
                Some(constraint) => match constraint_allows(constraint, target_version) {
                    Some(true) => (Verdict::Ok, String::new()),
                    Some(false) => (Verdict::Incompatible, format!("поддерживает {constraint}")),
                    None => (
                        Verdict::Unknown,
                        format!("не удалось разобрать диапазон «{constraint}»"),
                    ),
                },
            },
        };
        let entry = match manifest.as_ref() {
            Some(manifest) => {
                let mod_id = manifest_field(manifest, "mod_id");
                let title = {
                    let name = manifest_field(manifest, "name");
                    if name.is_empty() {
                        mod_id.clone()
                    } else {
                        name
                    }
                };
                json!({
                    "file": name,
                    "mod_id": mod_id,
                    "title": title,
                    "version": manifest_field(manifest, "version"),
                    "loader": manifest_field(manifest, "loader"),
                    "reason": reason,
                })
            }
            None => json!({
                "file": name,
                "mod_id": "",
                "title": "",
                "version": "",
                "loader": "",
                "reason": reason,
            }),
        };
        match verdict {
            Verdict::Ok => ok.push(entry),
            Verdict::Incompatible => incompatible.push(entry),
            Verdict::Unknown => unknown.push(entry),
        }
    }
    let by_file = |left: &Value, right: &Value| {
        left.get("file")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("file")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    };
    ok.sort_by(by_file);
    incompatible.sort_by(by_file);
    unknown.sort_by(by_file);
    json!({
        "active": active,
        "disabled": disabled,
        "ok": ok,
        "incompatible": incompatible,
        "unknown": unknown,
    })
}

fn java_major_from_version_json(instance_path: &Path, version_id: &str) -> Option<u64> {
    if version_id.is_empty() {
        return None;
    }
    let metadata_path = instance_path
        .join("versions")
        .join(version_id)
        .join(format!("{version_id}.json"));
    let text = fs::read_to_string(&metadata_path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    java_major_of(&value)
}

fn java_major_of(value: &Value) -> Option<u64> {
    value
        .get("javaVersion")
        .and_then(|java| java.get("majorVersion"))
        .and_then(|major| {
            major.as_u64().or_else(|| {
                major
                    .as_str()
                    .and_then(|text| text.trim().parse::<u64>().ok())
            })
        })
}

fn java_major_from_repository(config: &ConfigStore, version: &str) -> Option<u64> {
    let http = super::net::client_from_settings(config.settings(), "").ok()?;
    let mut repository = super::metadata::VersionRepository::new(config, Some(http)).ok()?;
    let resolved = repository.resolve(version).ok()?;
    java_major_of(&resolved)
}

fn version_status(version: &str) -> &'static str {
    match parse_mc_version(version) {
        Some(_) => "release",
        None => "неизвестная",
    }
}

fn loader_label(loader: &str) -> String {
    match loader {
        "vanilla" => "Vanilla".to_owned(),
        "fabric" => "Fabric Loader".to_owned(),
        "quilt" => "Quilt Loader".to_owned(),
        "forge" => "Forge".to_owned(),
        "neoforge" => "NeoForge".to_owned(),
        other => other.to_owned(),
    }
}

struct UpgradeRequest {
    info: super::instances::InstanceInfo,
    config: ConfigStore,
    store: InstanceStore,
    target_version: String,
    loader: String,
    loader_version: Option<String>,
}

/// общая валидация: клиентский Java-экземпляр, остановлен, целевая версия указана
fn load_upgrade_request(request: &Value) -> Result<UpgradeRequest, String> {
    let name = request
        .get("instance_name")
        .or_else(|| request.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if name.is_empty() {
        return Err("Сначала выберите экземпляр".to_owned());
    }
    let target_version = request
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if target_version.is_empty() {
        return Err("Не указана целевая версия Minecraft".to_owned());
    }
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let info = store.get(&name)?;
    if info.game_edition.trim().eq_ignore_ascii_case("bedrock") {
        return Err("Обновление доступно только для Java-экземпляров".to_owned());
    }
    if info.kind.trim().eq_ignore_ascii_case("server") {
        return Err("Обновление доступно только для клиентских экземпляров".to_owned());
    }
    if info.running {
        return Err("Остановите экземпляр перед обновлением".to_owned());
    }
    let current_version = info.minecraft_version.trim().to_owned();
    if current_version.is_empty() {
        return Err(format!(
            "У экземпляра «{}» не определена версия Minecraft",
            info.name
        ));
    }
    let loader = {
        let requested = request
            .get("loader")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let value = if requested.is_empty() {
            info.loader.trim().to_lowercase()
        } else {
            requested
        };
        if value.is_empty() {
            "vanilla".to_owned()
        } else {
            value
        }
    };
    if !matches!(
        loader.as_str(),
        "vanilla" | "fabric" | "forge" | "neoforge" | "quilt"
    ) {
        return Err(format!("Неизвестный loader: {loader}"));
    }
    let loader_version = request
        .get("loader_version")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Ok(UpgradeRequest {
        info,
        config,
        store,
        target_version,
        loader,
        loader_version,
    })
}

fn build_plan(request: &UpgradeRequest) -> Value {
    let info = &request.info;
    let current_version = info.minecraft_version.trim().to_owned();
    let target_version = request.target_version.clone();
    let current_loader = {
        let value = info.loader.trim().to_lowercase();
        if value.is_empty() {
            "vanilla".to_owned()
        } else {
            value
        }
    };
    let current_loader_version = {
        let resolved = loader_version_of(
            &info.path,
            &info.version_id,
            &current_version,
            &current_loader,
        );
        (!resolved.is_empty()).then_some(resolved)
    };
    let direction = match (
        parse_mc_version(&current_version),
        parse_mc_version(&request.target_version),
    ) {
        (Some(current), Some(target)) => match compare_versions(&current, &target) {
            Ordering::Less => "upgrade",
            Ordering::Greater => "downgrade",
            Ordering::Equal => "same",
        },
        _ => "unknown",
    };
    let same_setup = direction == "same" && current_loader == request.loader;

    let current_java = java_major_from_version_json(&info.path, info.version_id.trim())
        .or_else(|| java_major_from_repository(&request.config, &current_version));
    let target_java = java_major_from_repository(&request.config, &request.target_version);

    let worlds = world_entries(&info.path);
    let worlds_bytes: u64 = worlds
        .iter()
        .filter_map(|world| world.get("size_bytes").and_then(Value::as_u64))
        .sum();

    let mods = scan_mod_compatibility(&info.path.join("mods"), &request.target_version);
    let resourcepacks = collect_resource_names(&info.path.join("resourcepacks"));
    let shaders = collect_resource_names(&info.path.join("shaderpacks"));
    let datapacks = datapack_count(&info.path);

    let bucket = |key: &str| -> Vec<Value> {
        mods.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let ok_mods = bucket("ok");
    let incompatible_mods = bucket("incompatible");
    let unknown_mods = bucket("unknown");

    let mut warnings: Vec<String> = Vec::new();
    if direction == "downgrade" {
        warnings.push(
            "Это откат на более старую версию: миры, запущенные на новой версии, могут не открыться на старой.".to_owned(),
        );
    }
    if same_setup {
        warnings
            .push("Эта версия с этим модлоадером уже установлена: обновлять нечего.".to_owned());
    }
    match (&current_java, &target_java) {
        (Some(current), Some(target)) if target > current => warnings.push(format!(
            "Для {target_version} нужна Java {target}, сейчас используется Java {current}. Лаунчер проверит Java при запуске."
        )),
        (None, Some(target)) => warnings.push(format!(
            "Для {target_version} нужна Java {target}: убедитесь, что она установлена."
        )),
        _ => {}
    }
    if !unknown_mods.is_empty() {
        warnings.push(format!(
            "{} мод(ов) не заявляют поддерживаемую версию: их совместимость стоит проверить вручную.",
            unknown_mods.len()
        ));
    }
    if datapacks > 0 && direction == "upgrade" {
        warnings.push(format!(
            "В мирах найдено датапаков: {datapacks}. После обновления игры они могут требовать обновления формата."
        ));
    }

    let mut steps: Vec<String> = Vec::new();
    if !worlds.is_empty() {
        steps.push("Создаётся резервная копия всех миров экземпляра.".to_owned());
    }
    if !incompatible_mods.is_empty() {
        steps.push(format!(
            "{} несовместимых мод(ов) переносятся в папку карантина внутри экземпляра, файлы не удаляются.",
            incompatible_mods.len()
        ));
    }
    steps.push(format!(
        "Скачиваются файлы версии {target_version} и {}.",
        loader_label(&request.loader)
    ));
    steps.push(
        "Миры, моды, конфиги, ресурспаки и шейдеры остаются на месте без изменений.".to_owned(),
    );
    steps.push("Старые файлы версии остаются в папке versions: откат возможен вручную.".to_owned());

    json!({
        "current": {
            "version": current_version,
            "version_id": info.version_id,
            "loader": current_loader,
            "loader_title": loader_label(&current_loader),
            "loader_version": current_loader_version.unwrap_or_default(),
            "java": current_java,
        },
        "target": {
            "version": request.target_version,
            "loader": request.loader,
            "loader_title": loader_label(&request.loader),
            "loader_version": request.loader_version.clone().unwrap_or_default(),
            "java": target_java,
        },
        "direction": direction,
        "no_changes": same_setup,
        "worlds": {
            "count": worlds.len(),
            "size_bytes": worlds_bytes,
            "list": worlds,
        },
        "mods": {
            "active": mods.get("active").and_then(Value::as_u64).unwrap_or(0),
            "disabled": mods.get("disabled").and_then(Value::as_u64).unwrap_or(0),
            "ok": ok_mods,
            "incompatible": incompatible_mods,
            "unknown": unknown_mods,
        },
        "resources": {
            "resourcepacks": resourcepacks,
            "shaders": shaders,
            "datapacks": datapacks,
        },
        "steps": steps,
        "warnings": warnings,
    })
}

/// план обновления: что поменяется и что может сломаться
pub fn instance_upgrade_plan(request: &Value) -> Result<Value, String> {
    let request = load_upgrade_request(request)?;
    Ok(build_plan(&request))
}

fn unique_quarantine_dir(instance_path: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let base = instance_path.join(format!("mods_incompatible_upgrade-{stamp}"));
    if !base.exists() {
        return base;
    }
    for index in 1..1000 {
        let candidate = instance_path.join(format!("mods_incompatible_upgrade-{stamp}-{index}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    base
}

fn mod_sidecar_path(mod_path: &Path) -> PathBuf {
    let name = mod_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    mod_path.with_file_name(format!(".{name}{MOD_SIDECAR_SUFFIX}"))
}

fn move_mod_with_sidecar(source: &Path, destination_dir: &Path) -> Result<PathBuf, String> {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .ok_or_else(|| "Не удалось определить имя файла мода".to_owned())?;
    let mut destination = destination_dir.join(&name);
    if destination.exists() {
        let stem = destination
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_default();
        let extension = destination
            .extension()
            .map(|extension| extension.to_string_lossy().to_string())
            .unwrap_or_default();
        for index in 1..1000 {
            let candidate = if extension.is_empty() {
                destination_dir.join(format!("{stem}-{index}"))
            } else {
                destination_dir.join(format!("{stem}-{index}.{extension}"))
            };
            if !candidate.exists() {
                destination = candidate;
                break;
            }
        }
    }
    fs::rename(source, &destination)
        .map_err(|error| format!("Не удалось переместить {}: {error}", name))?;
    let sidecar = mod_sidecar_path(source);
    if sidecar.is_file() {
        let sidecar_name = sidecar
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let _ = fs::rename(&sidecar, destination_dir.join(sidecar_name));
    }
    Ok(destination)
}

/// возвращает моды из карантина обратно в mods (используется при неудачном обновлении)
fn restore_quarantine(quarantine_dir: &Path, mods_dir: &Path) -> usize {
    let mut restored = 0usize;
    let Ok(entries) = fs::read_dir(quarantine_dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        let name = match path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
        {
            Some(name) => name,
            None => continue,
        };
        let mut destination = mods_dir.join(&name);
        if destination.exists() {
            continue;
        }
        if fs::rename(&path, &destination).is_ok() {
            restored += 1;
        } else {
            let _ = &mut destination;
        }
    }
    restored
}

/// применяет обновление: бэкап миров, карантин несовместимых модов, установка версии
pub fn instance_upgrade_apply(request: &Value) -> Result<Value, String> {
    let upgrade = load_upgrade_request(request)?;
    let info = &upgrade.info;
    let backup_worlds = request
        .get("backup_worlds")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let _lock = super::server::InstanceLock::acquire(&upgrade.config.paths.data_home, &info.name)?;

    // план пересчитывается в момент применения: состояние могло измениться
    let plan = build_plan(&upgrade);
    let no_changes = plan
        .get("no_changes")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if no_changes {
        return Err("Эта версия с этим модлоадером уже установлена".to_owned());
    }

    // 1. резервные копии миров до любых изменений
    let backup = if backup_worlds {
        let worlds = plan
            .pointer("/worlds/list")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if worlds.is_empty() {
            None
        } else {
            match super::backups::world_backup_create(&json!({
                "name": info.name,
                "destination": "local",
                "reason": "upgrade",
            })) {
                Ok(value) => value.get("backup").cloned(),
                Err(error) => {
                    return Err(format!("Не удалось создать резервную копию миров: {error}"))
                }
            }
        }
    } else {
        None
    };

    // 2. карантин несовместимых модов
    let incompatible = plan
        .pointer("/mods/incompatible")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mods_dir = info.path.join("mods");
    let quarantine_dir = unique_quarantine_dir(&info.path);
    let mut moved: Vec<String> = Vec::new();
    if !incompatible.is_empty() {
        fs::create_dir_all(&quarantine_dir)
            .map_err(|_| "Не удалось создать папку карантина для несовместимых модов".to_owned())?;
        for entry in &incompatible {
            let file = entry
                .get("file")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if file.is_empty() {
                continue;
            }
            let source = mods_dir.join(file);
            if !source.is_file() {
                continue;
            }
            match move_mod_with_sidecar(&source, &quarantine_dir) {
                Ok(_) => moved.push(file.to_owned()),
                Err(error) => {
                    // неудача одного файла не должна оставлять остальные в подвешенном состоянии:
                    // возвращаем уже перемещённые и прерываем обновление
                    let restored = restore_quarantine(&quarantine_dir, &mods_dir);
                    let _ = fs::remove_dir(&quarantine_dir);
                    return Err(format!("Моды возвращены на место ({restored}): {error}"));
                }
            }
        }
    }

    // 3. установка новой версии и модлоадера поверх существующего экземпляра
    let install_result = {
        let http = super::net::client_from_settings(upgrade.config.settings(), "")?;
        let mut installer = MinecraftInstaller::new(&upgrade.config, http)?;
        installer.install(
            &upgrade.target_version,
            &upgrade.loader,
            upgrade.loader_version.as_deref(),
            Some(&info.name),
            true,
            true,
        )
    };
    let installed = match install_result {
        Ok(result) => result,
        Err(error) => {
            // откат: моды возвращаются в mods, бэкапы миров остаются
            let restored = restore_quarantine(&quarantine_dir, &mods_dir);
            if moved.is_empty() {
                let _ = fs::remove_dir(&quarantine_dir);
            }
            return Err(format!(
                "Обновление не удалось, несовместимые моды возвращены на место ({restored}). Причина: {error}"
            ));
        }
    };

    super::events::emit_status_event(
        "workspace-changed",
        &format!(
            "Экземпляр «{}» обновлён до версии {}.",
            info.name, upgrade.target_version
        ),
    );
    let list: Vec<Value> = upgrade.store.list().iter().map(|i| i.public()).collect();
    let warnings = plan
        .get("warnings")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(json!({
        "version_id": installed.version_id,
        "minecraft_version": upgrade.target_version,
        "instance_dir": installed.instance_dir.to_string_lossy(),
        "moved_mods": moved,
        "quarantine_dir": if moved.is_empty() { String::new() } else { quarantine_dir.to_string_lossy().to_string() },
        "backup": backup.unwrap_or(Value::Null),
        "warnings": warnings,
        "instances": list,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_versions() {
        assert_eq!(parse_mc_version("1.21.4"), Some(vec![1, 21, 4]));
        assert_eq!(parse_mc_version("1.21"), Some(vec![1, 21]));
        assert_eq!(parse_mc_version("1.21.4-rc1"), Some(vec![1, 21, 4]));
        assert_eq!(parse_mc_version("25w46a"), None);
        assert_eq!(parse_mc_version(""), None);
    }

    #[test]
    fn orders_versions() {
        assert_eq!(compare_versions(&[1, 21], &[1, 21, 4]), Ordering::Less);
        assert_eq!(compare_versions(&[1, 21, 4], &[1, 21]), Ordering::Greater);
        assert_eq!(compare_versions(&[1, 21], &[1, 21, 0]), Ordering::Equal);
    }

    #[test]
    fn exact_and_star_constraints() {
        assert_eq!(constraint_allows("*", "1.21.4"), Some(true));
        assert_eq!(constraint_allows("", "1.21.4"), Some(true));
        assert_eq!(constraint_allows("1.21.4", "1.21.4"), Some(true));
        assert_eq!(constraint_allows("1.21.4", "1.21.10"), Some(false));
    }

    #[test]
    fn range_constraints() {
        assert_eq!(constraint_allows(">=1.20", "1.21.4"), Some(true));
        assert_eq!(constraint_allows("<1.22", "1.21.4"), Some(true));
        assert_eq!(constraint_allows(">=1.20 <1.22", "1.21.4"), Some(true));
        assert_eq!(constraint_allows(">=1.20 <1.21", "1.21.4"), Some(false));
        assert_eq!(constraint_allows(">1.16.5", "1.16.5"), Some(false));
        assert_eq!(constraint_allows("<=1.21.4", "1.21.4"), Some(true));
        assert_eq!(constraint_allows("!=1.21.4", "1.21.4"), Some(false));
    }

    #[test]
    fn tilde_and_caret_constraints() {
        assert_eq!(constraint_allows("~1.21", "1.21.4"), Some(true));
        assert_eq!(constraint_allows("~1.21", "1.22"), Some(false));
        assert_eq!(constraint_allows("^1.21", "1.22.3"), Some(true));
        assert_eq!(constraint_allows("^1.21", "2.0"), Some(false));
    }

    #[test]
    fn unparsable_is_unknown() {
        assert_eq!(constraint_allows(">=1.21.x", "1.21.4"), None);
        assert_eq!(constraint_allows("1.21.4+", "1.21.4"), None);
    }
}
