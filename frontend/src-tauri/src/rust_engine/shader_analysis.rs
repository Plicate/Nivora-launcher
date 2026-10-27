// анализ шейдерпаков и совместимости с версиями игры
use regex::Regex;
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use super::config::ConfigStore;
use super::instances::InstanceStore;

const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_SCANNED_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_SCANNED_TOTAL_BYTES: usize = 128 * 1024 * 1024;

fn feature_weights() -> Vec<(&'static str, i64)> {
    vec![
        ("Тени (shadow pass)", 16),
        ("POM / параллакс", 12),
        ("Волюметрический свет", 10),
        ("Отражения (SSR/raytracing)", 10),
        ("Волюметрические облака", 9),
        ("TAA / временное сглаживание", 6),
        ("Глубина резкости (DOF)", 6),
        ("Bloom", 5),
        ("Motion blur", 3),
        ("Динамическое небо", 3),
        ("Искажение теней", 2),
    ]
}

fn feature_patterns() -> Vec<(&'static str, &'static str)> {
    vec![
        ("POM / параллакс", r"(?i)parallax|relief[_ ]?mapping|pom[_ ]?enabled"),
        ("Волюметрический свет", r"(?i)volumetric|lightshaft|god[_ ]?ray|vl_enabled"),
        ("Отражения (SSR/raytracing)", r"(?i)raytrac|screen[_ ]space[_ ]reflect|reflection"),
        ("Волюметрические облака", r"(?i)volumetric[_ ]cloud|cloud[_ ]raymarch|cumulus"),
        ("TAA / временное сглаживание", r"(?i)\btaa\b|temporal[_ ]?(aa|filter|accum)"),
        ("Bloom", r"(?i)\bbloom\b"),
        ("Глубина резкости (DOF)", r"(?i)\bdof\b|depth[_ ]of[_ ]field|focal[_]"),
        ("Motion blur", r"(?i)motion[_ ]?blur"),
        ("Динамическое небо", r"(?i)\bskybox\b|atmospher|scatter"),
        ("Искажение теней", r"(?i)distort"),
    ]
}

fn pass_patterns() -> Vec<(String, &'static str, i64)> {
    vec![
        (r"(?i)^final\.(fsh|vsh)$".to_owned(), "final", 8),
        (r"(?i)^composite\d*\.(fsh|vsh)$".to_owned(), "composite", 6),
        (r"(?i)^deferred\d*\.(fsh|vsh)$".to_owned(), "deferred", 6),
        (r"(?i)^prepare\d*\.(fsh|vsh)$".to_owned(), "prepare", 4),
        (r"(?i)^shadow(fst|comp)?\.(fsh|vsh)$".to_owned(), "shadow", 14),
        (r"(?i)^gbuffers_[\w]+\.(fsh|vsh)$".to_owned(), "gbuffers", 3),
        (r"(?i)^\w+\.(gsh|csh)$".to_owned(), "compute", 7),
    ]
}

pub fn shader_analyze(request: &Value) -> Result<Value, String> {
    let name = request.get("name").and_then(Value::as_str).unwrap_or_default().trim();
    let shader = request.get("shader").and_then(Value::as_str).unwrap_or_default().trim();
    if name.is_empty() || shader.is_empty() {
        return Err("Укажите экземпляр и файл шейдерпака".to_owned());
    }
    let config = ConfigStore::load()?;
    let store = InstanceStore::new(&config)?;
    let info = store.get(name)?;
    if info.game_edition == "bedrock" || info.kind == "server" {
        return Err("Анализ шейдеров доступен только для клиентских Java-экземпляров".to_owned());
    }
    let shader_path = info.path.join("shaderpacks").join(shader);
    if shader_path.is_symlink() || !(shader_path.is_file() || shader_path.is_dir()) {
        return Err(format!("Шейдерпак не найден в экземпляре: {shader}"));
    }
    let requested = if info.minecraft_version.is_empty() { info.version_id.clone() } else { info.minecraft_version.clone() };
    let analysis = analyze_shaderpack(&shader_path, Some(&requested))?;
    let gpu = gpu_profile();
    let verdict = complexity_verdict(&analysis, &gpu);
    // бенчмарк доступен только при установленном Iris
    let mods_dir = info.path.join("mods");
    let has_iris = fs::read_dir(&mods_dir)
        .map(|entries| {
            entries.filter_map(|entry| entry.ok()).any(|entry| {
                let file_name = entry.file_name().to_string_lossy().to_lowercase();
                entry.path().is_file() && file_name.starts_with("iris") && file_name.ends_with(".jar")
            })
        })
        .unwrap_or(false);
    Ok(json!({
        "analysis": analysis,
        "gpu": gpu,
        "verdict": verdict,
        "benchmark_available": has_iris,
    }))
}

fn complexity_verdict(analysis: &Value, gpu: &Value) -> Value {
    let complexity = analysis.get("complexity").and_then(Value::as_i64).unwrap_or(0);
    let tier = gpu.get("tier").and_then(Value::as_str).unwrap_or("mid");
    let limit = match tier {
        "software" => 0,
        "entry" => 22,
        "high" => 70,
        "enthusiast" => 101,
        _ => 45,
    };
    let (label, tone) = if complexity <= (limit - 20).max(0) {
        ("Потянет с запасом", "success")
    } else if complexity <= limit {
        ("Потянет", "success")
    } else if complexity <= limit + 18 {
        ("На пределе — уменьшите дальность теней", "warning")
    } else {
        ("Не потянет", "danger")
    };
    json!({ "label": label, "tone": tone })
}

#[derive(Default)]
struct PackFacts {
    max_glsl: u32,
    core_profile: bool,
    compatibility_profile: bool,
    compute: bool,
    geometry: bool,
    legacy_syntax: bool,
    iris_extension: bool,
    mc_version_guards: bool,
    shadow_resolution: i64,
    supported_min: Option<String>,
    supported_max: Option<String>,
}

struct Analysis {
    name: String,
    format: String,
    files: i64,
    programs: Vec<String>,
    features: Vec<String>,
    size_bytes: i64,
    complexity: i64,
    estimated_vram_mb: i64,
    notes: Vec<String>,
    facts: PackFacts,
}

pub(crate) fn analyze_shaderpack(source: &Path, requested_version: Option<&str>) -> Result<Value, String> {
    if source.is_symlink() {
        return Err("Шейдерпак является символьной ссылкой".to_owned());
    }
    let mut analysis = if source.is_file() {
        let size = fs::metadata(source).map_err(|error| format!("Шейдерпак не читается: {error}"))?.len();
        if size > MAX_ARCHIVE_BYTES {
            return Err("Архив шейдерпака слишком большой".to_owned());
        }
        let file = File::open(source).map_err(|error| format!("Шейдерпак не читается: {error}"))?;
        let mut archive = zip::ZipArchive::new(file).map_err(|_| "Это не архив шейдерпака".to_owned())?;
        // читаем архив за один проход, не открывая записи повторно
        let mut entries: Vec<(String, u64)> = Vec::new();
        let mut contents: Vec<(String, Vec<u8>)> = Vec::new();
        let mut scanned_total = 0usize;
        for index in 0..archive.len() {
            let mut item = archive.by_index(index).map_err(|_| "Архив шейдерпака повреждён".to_owned())?;
            if item.is_dir() {
                continue;
            }
            let name = item.name().map(|v| v.replace('\\', "/")).unwrap_or_default();
            let size = item.size();
            entries.push((name.clone(), size));
            if scannable(&name) && size <= MAX_SCANNED_FILE_BYTES as u64 && scanned_total < MAX_SCANNED_TOTAL_BYTES {
                let mut buffer = Vec::new();
                if item.read_to_end(&mut buffer).is_ok() && buffer.len() <= MAX_SCANNED_FILE_BYTES {
                    scanned_total += buffer.len();
                    contents.push((name, buffer));
                }
            }
        }
        let mut analysis = analyze_entries(
            source.file_stem().map(|value| value.to_string_lossy().to_string()).unwrap_or_default(),
            &entries,
        )?;
        scan_content(&mut analysis, &contents);
        analysis.size_bytes = entries.iter().map(|(_, size)| *size as i64).sum();
        analysis
    } else if source.is_dir() {
        let mut entries: Vec<(String, u64)> = Vec::new();
        let mut contents: Vec<(String, Vec<u8>)> = Vec::new();
        let mut scanned_total = 0usize;
        collect_directory(source, source, &mut entries, &mut contents, &mut scanned_total)?;
        let mut analysis = analyze_entries(
            source.file_name().map(|value| value.to_string_lossy().to_string()).unwrap_or_default(),
            &entries,
        )?;
        scan_content(&mut analysis, &contents);
        analysis.size_bytes = entries.iter().map(|(_, size)| *size as i64).sum();
        analysis
    } else {
        return Err(format!("Шейдерпак не найден: {}", source.display()));
    };
    if analysis.programs.is_empty() {
        return Err("В шейдерпаке нет программ шейдеров (папка shaders/ с .fsh/.vsh)".to_owned());
    }
    finalize_complexity(&mut analysis);
    let mut payload = analysis_payload(&analysis);
    if let Some(object) = payload.as_object_mut() {
        object.insert("versions".to_owned(), version_block(&analysis, requested_version));
    }
    Ok(payload)
}

fn scannable(name: &str) -> bool {
    let lowered = name.to_lowercase();
    lowered.ends_with(".fsh") || lowered.ends_with(".vsh") || lowered.ends_with(".glsl")
        || lowered.ends_with(".gsh") || lowered.ends_with(".csh") || lowered.ends_with(".properties")
}

fn collect_directory(root: &Path, directory: &Path, entries: &mut Vec<(String, u64)>, contents: &mut Vec<(String, Vec<u8>)>, scanned_total: &mut usize) -> Result<(), String> {
    let items = fs::read_dir(directory).map_err(|error| format!("Не удалось прочитать каталог шейдерпака: {error}"))?;
    for item in items.filter_map(|entry| entry.ok()) {
        let path = item.path();
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            collect_directory(root, &path, entries, contents, scanned_total)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let relative = path.strip_prefix(root).map_err(|_| "Странный путь внутри шейдерпака".to_owned())?;
        let name = relative.to_string_lossy().replace('\\', "/");
        let size = item.metadata().map(|meta| meta.len()).unwrap_or(0);
        entries.push((name.clone(), size));
        if scannable(&name) && size <= MAX_SCANNED_FILE_BYTES as u64 && *scanned_total < MAX_SCANNED_TOTAL_BYTES {
            if let Ok(mut file) = File::open(&path) {
                let mut buffer = Vec::new();
                if file.read_to_end(&mut buffer).is_ok() && buffer.len() <= MAX_SCANNED_FILE_BYTES {
                    *scanned_total += buffer.len();
                    contents.push((name, buffer));
                }
            }
        }
    }
    Ok(())
}

fn analyze_entries(name: String, entries: &[(String, u64)]) -> Result<Analysis, String> {
    let mut analysis = Analysis {
        name,
        format: "iris".to_owned(),
        files: 0,
        programs: Vec::new(),
        features: Vec::new(),
        size_bytes: 0,
        complexity: 0,
        estimated_vram_mb: 0,
        notes: Vec::new(),
        facts: PackFacts::default(),
    };
    let compiled: Vec<(Regex, &'static str)> = pass_patterns()
        .into_iter()
        .filter_map(|(pattern, kind, _)| Regex::new(&pattern).ok().map(|regex| (regex, kind)))
        .collect();
    for (entry_name, _size) in entries {
        if entry_name.split('/').any(|part| part == "..") {
            analysis.notes.push(format!("Подозрительный путь в архиве: {entry_name}"));
            continue;
        }
        if entry_name.starts_with("shaders/") || entry_name.contains("/shaders/") {
            analysis.files += 1;
            let base = entry_name.rsplit('/').next().unwrap_or_default().to_owned();
            for (regex, kind) in &compiled {
                if regex.is_match(&base) && !analysis.programs.iter().any(|value| value == kind) {
                    analysis.programs.push(kind.to_string());
                }
            }
            if base.eq_ignore_ascii_case("shaders.properties") {
                analysis.format = "iris-properties".to_owned();
            }
        }
    }
    if analysis.programs.iter().any(|kind| kind == "shadow") {
        analysis.features.push("Тени (shadow pass)".to_owned());
    }
    Ok(analysis)
}

fn scan_content(analysis: &mut Analysis, contents: &[(String, Vec<u8>)]) {
    let compiled: Vec<(&'static str, Regex)> = feature_patterns()
        .iter()
        .filter_map(|(feature, pattern)| Regex::new(pattern).ok().map(|regex| (*feature, regex)))
        .collect();
    let version_directive = Regex::new(r"(?m)^\s*#\s*version\s+(\d+)\s*(core|compatibility)?").unwrap();
    let shadow_resolution = Regex::new(r"(?m)^\s*shadowMapResolution\s*=\s*(\d+)").unwrap();
    let supported_versions = Regex::new(r"(?m)^\s*supportedVersions\s*=\s*(.+)").unwrap();
    let mc_guard = Regex::new(r"#if\s+MC_VERSION\s*>=\s*\d+").unwrap();
    for (name, content) in contents {
        let lowered = name.to_lowercase();
        let text = String::from_utf8_lossy(content);
        if lowered.ends_with(".properties") {
            if lowered.ends_with("shaders.properties") {
                if let Some(captures) = shadow_resolution.captures(&text) {
                    if let Some(value) = captures.get(1).and_then(|m| m.as_str().parse::<i64>().ok()) {
                        analysis.facts.shadow_resolution = value;
                    }
                }
                if let Some(captures) = supported_versions.captures(&text) {
                    parse_supported_versions(captures.get(1).map(|m| m.as_str()).unwrap_or_default(), &mut analysis.facts);
                }
                if text.contains("iris.") || text.contains("iris_features") {
                    analysis.facts.iris_extension = true;
                }
            }
            continue;
        }
        if let Some(captures) = version_directive.captures(&text) {
            if let Some(version) = captures.get(1).and_then(|m| m.as_str().parse::<u32>().ok()) {
                analysis.facts.max_glsl = analysis.facts.max_glsl.max(version);
                match captures.get(2).map(|m| m.as_str()) {
                    Some("core") => analysis.facts.core_profile = true,
                    Some("compatibility") => analysis.facts.compatibility_profile = true,
                    _ => {}
                }
            }
        }
        if lowered.ends_with(".csh") {
            analysis.facts.compute = true;
        }
        if lowered.ends_with(".gsh") {
            analysis.facts.geometry = true;
        }
        for (feature, regex) in &compiled {
            if !analysis.features.iter().any(|value| value == feature) && regex.is_match(&text) {
                analysis.features.push(feature.to_string());
            }
        }
        if analysis.facts.shadow_resolution == 0 {
            if let Some(captures) = shadow_resolution.captures(&text) {
                if let Some(value) = captures.get(1).and_then(|m| m.as_str().parse::<i64>().ok()) {
                    analysis.facts.shadow_resolution = value;
                }
            }
        }
        if text.contains("iris.") || text.contains("iris_features") {
            analysis.facts.iris_extension = true;
        }
        if mc_guard.is_match(&text) {
            analysis.facts.mc_version_guards = true;
        }
        if text.contains("varying ") || text.contains("gl_ModelViewMatrix") || text.contains("texture2D(") {
            analysis.facts.legacy_syntax = true;
        }
    }
}

// `supportedVersions` может быть списком версий или диапазоном
fn parse_supported_versions(raw: &str, facts: &mut PackFacts) {
    let raw = raw.trim();
    if raw.is_empty() {
        return;
    }
    if let Some((min, max)) = raw.split_once('-') {
        let (min, max) = (min.trim(), max.trim());
        if !min.is_empty() && !max.is_empty() {
            facts.supported_min = Some(min.to_owned());
            facts.supported_max = Some(max.to_owned());
            return;
        }
    }
    let mut parts: Vec<&str> = raw.split(',').map(str::trim).filter(|value| !value.is_empty()).collect();
    if parts.len() <= 1 {
        return;
    }
    parts.sort_by(|a, b| version_tuple(a).cmp(&version_tuple(b)));
    facts.supported_min = Some(parts[0].to_owned());
    facts.supported_max = Some(parts[parts.len() - 1].to_owned());
}

fn version_tuple(value: &str) -> (u32, u32, u32) {
    let mut numbers = [0u32; 3];
    for (index, part) in value.split('.').take(3).enumerate() {
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        numbers[index] = digits.parse().unwrap_or(0);
    }
    (numbers[0], numbers[1], numbers[2])
}

fn finalize_complexity(analysis: &mut Analysis) {
    let mut score: i64 = 0;
    for feature in &analysis.features {
        let weight = feature_weights().iter().find(|(name, _)| name == feature).map(|(_, weight)| *weight).unwrap_or(2);
        score += weight;
    }
    let passes = pass_patterns();
    for kind in &analysis.programs {
        let weight = passes.iter().find(|(_, name, _)| name == kind).map(|(_, _, weight)| *weight).unwrap_or(3);
        score += weight;
    }
    let shadow = if analysis.facts.shadow_resolution > 0 {
        analysis.facts.shadow_resolution
    } else if analysis.features.iter().any(|value| value == "Тени (shadow pass)") {
        2048
    } else {
        0
    };
    if shadow > 0 {
        score += (shadow / 256).min(18);
        analysis.estimated_vram_mb = 256 + (shadow * shadow * 4 * 3) / (1024 * 1024) + 128 * (analysis.programs.len() as i64 - 4).max(0);
    } else {
        analysis.estimated_vram_mb = 256;
    }
    analysis.complexity = score.clamp(1, 100);
}

fn analysis_payload(analysis: &Analysis) -> Value {
    json!({
        "name": analysis.name,
        "format": analysis.format,
        "files": analysis.files,
        "programs": analysis.programs,
        "features": analysis.features,
        "shadow_resolution": analysis.facts.shadow_resolution,
        "size_bytes": analysis.size_bytes,
        "complexity": analysis.complexity,
        "estimated_vram_mb": analysis.estimated_vram_mb,
        "notes": analysis.notes,
        "glsl": {
            "max_version": if analysis.facts.max_glsl > 0 { Value::from(analysis.facts.max_glsl) } else { Value::Null },
            "core_profile": analysis.facts.core_profile,
            "compatibility_profile": analysis.facts.compatibility_profile,
            "compute_shaders": analysis.facts.compute,
            "geometry_shaders": analysis.facts.geometry,
            "legacy_syntax": analysis.facts.legacy_syntax,
            "mc_version_guards": analysis.facts.mc_version_guards,
            "supported_min": analysis.facts.supported_min,
            "supported_max": analysis.facts.supported_max,
        },
    })
}

fn version_block(analysis: &Analysis, requested_version: Option<&str>) -> Value {
    let facts = &analysis.facts;
    let mut versions: Vec<String> = ["1.8.9", "1.12.2", "1.16.5", "1.17.1", "1.19.2", "1.20.1", "1.21.1", "1.21.4"]
        .iter()
        .map(|value| value.to_string())
        .collect();
    if let Some(requested) = requested_version.map(str::trim).filter(|value| !value.is_empty()) {
        if !versions.iter().any(|value| value == requested) {
            versions.push(requested.to_owned());
        }
    }
    versions.sort_by_key(|value| version_tuple(value));
    versions.dedup();
    let mut matrix: Vec<Value> = Vec::new();
    let mut requested_verdict: Option<Value> = None;
    for version in &versions {
        let entry = version_verdict(version, facts);
        if Some(version.as_str()) == requested_version.map(str::trim).filter(|value| !value.is_empty()) {
            requested_verdict = Some(entry.clone());
        }
        matrix.push(entry);
    }
    if requested_verdict.is_none() {
        requested_verdict = matrix
            .iter()
            .rev()
            .find(|entry| entry.get("status").and_then(Value::as_str) == Some("ok"))
            .cloned()
            .or_else(|| matrix.first().cloned());
    }
    let min_mc = facts.supported_min.clone().or_else(|| {
        if facts.compute || facts.iris_extension || (facts.core_profile && facts.max_glsl >= 330) {
            Some("1.16.5".to_owned())
        } else {
            Some("1.7.2".to_owned())
        }
    });
    let max_mc = facts.supported_max.clone();
    json!({
        "requested": requested_version.unwrap_or_default(),
        "verdict": requested_verdict,
        "min_mc": min_mc,
        "max_mc": max_mc,
        "matrix": matrix,
    })
}

fn version_verdict(version: &str, facts: &PackFacts) -> Value {
    let target = version_tuple(version);
    let iris_available = target >= (1, 16, 5);
    let mut loader = if iris_available { "Iris или OptiFine" } else { "OptiFine" }.to_owned();
    let (status, reason) = if let (Some(min), Some(max)) = (&facts.supported_min, &facts.supported_max) {
        let in_range = version_tuple(min) <= target && target <= version_tuple(max);
        if in_range {
            ("ok", format!("пак сам объявляет поддержку {version} (shaders.properties)"))
        } else {
            ("broken", format!("пак заявляет поддержку только {min}–{max}"))
        }
    } else if facts.iris_extension && !iris_available {
        ("broken", "пак использует расширения Iris, которого нет ниже MC 1.16.5".to_owned())
    } else if facts.compute && !iris_available {
        ("broken", "вычислительные шейдеры (.csh) включаются только через Iris — MC 1.16.5+".to_owned())
    } else if facts.compute {
        loader = "Iris".to_owned();
        ("ok", "нужен Iris и видеокарта с OpenGL 4.3+".to_owned())
    } else if facts.core_profile && facts.max_glsl >= 330 && !iris_available {
        ("broken", "core-профиль GLSL требует OptiFine 1.17+ или Iris 1.16.5+".to_owned())
    } else if facts.max_glsl >= 460 && target < (1, 20, 0) {
        ("warn", "нужен драйвер OpenGL 4.6; пак рассчитан на свежие версии игры".to_owned())
    } else if facts.core_profile && facts.max_glsl >= 330 {
        ("ok", "core-профиль GLSL — Iris 1.16.5+ или OptiFine 1.17+".to_owned())
    } else if facts.geometry && iris_available {
        ("warn", "геометрические шейдеры в Iris поддерживаются частично, надёжнее OptiFine".to_owned())
    } else if facts.geometry {
        ("ok", "геометрические шейдеры: формат OptiFine".to_owned())
    } else {
        ("ok", "классический формат OptiFine-пака, ограничений не найдено".to_owned())
    };
    let (label, tone) = match status {
        "ok" => ("Работает", "success"),
        "warn" => ("Работает с оговорками", "warning"),
        _ => ("Не заработает", "danger"),
    };
    json!({
        "version": version,
        "status": status,
        "label": label,
        "tone": tone,
        "loader": loader,
        "reason": reason,
    })
}

fn gpu_profile() -> Value {
    let mut profile = nvidia_profile();
    if profile.is_none() {
        profile = amdgpu_profile();
    }
    if profile.is_none() {
        profile = intel_profile();
    }
    let mut profile = profile.unwrap_or(json!({
        "name": "Видеокарта не определена",
        "vram_mb": 0,
        "tier": "mid",
        "source": "none",
    }));
    let tier = profile.get("tier").and_then(Value::as_str).unwrap_or("mid").to_owned();
    let label = match tier.as_str() {
        "software" => "программный рендер",
        "entry" => "начальный",
        "high" => "высокий",
        "enthusiast" => "энтузиастский",
        _ => "средний",
    };
    if let Some(object) = profile.as_object_mut() {
        object.insert("tier_label".to_owned(), json!(label));
        if !object.contains_key("source") {
            object.insert("source".to_owned(), json!("sysfs"));
        }
    }
    profile
}

fn nvidia_profile() -> Option<Value> {
    let output = super::platform::command("nvidia-smi")
        .args(["--query-gpu=name,memory.total,driver_version", "--format=csv,noheader,nounits"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    let first_line = String::from_utf8_lossy(&output.stdout).lines().next()?.to_owned();
    let fields: Vec<&str> = first_line.split(',').map(str::trim).collect();
    if fields.len() < 2 {
        return None;
    }
    let name = fields[0].to_owned();
    let vram_mb: i64 = fields[1].chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
    Some(json!({ "name": name, "vram_mb": vram_mb, "tier": tier_from_name_and_vram(&name, vram_mb), "source": "nvidia-smi" }))
}

fn amdgpu_profile() -> Option<Value> {
    let mut cards: Vec<PathBuf> = fs::read_dir("/sys/class/drm")
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .map(|name| {
                    let text = name.to_string_lossy();
                    text.len() > 4 && text.starts_with("card") && text[4..].chars().all(|c| c.is_ascii_digit())
                })
                .unwrap_or(false)
        })
        .collect();
    cards.sort();
    for card in cards {
        let device = card.join("device");
        let mut name = fs::read_to_string(device.join("product_name")).unwrap_or_default().trim().to_owned();
        if name.is_empty() {
            let vendor = fs::read_to_string(device.join("vendor")).unwrap_or_default().trim().to_owned();
            if vendor != "0x1002" {
                continue;
            }
            let identifier = fs::read_to_string(card.join("device")).unwrap_or_default().trim().to_owned();
            name = format!("AMD GPU {identifier}");
        }
        let vram_mb: i64 = fs::read_to_string(device.join("mem_info_vram_total"))
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .map(|bytes| bytes / (1024 * 1024))
            .unwrap_or(0);
        return Some(json!({ "name": name, "vram_mb": vram_mb, "tier": tier_from_name_and_vram(&name, vram_mb), "source": "amdgpu" }));
    }
    None
}

fn intel_profile() -> Option<Value> {
    let mut cards: Vec<PathBuf> = fs::read_dir("/sys/class/drm")
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    cards.sort();
    for card in cards {
        let vendor = fs::read_to_string(card.join("device").join("vendor")).unwrap_or_default().trim().to_owned();
        if vendor == "0x8086" {
            return Some(json!({ "name": "Intel integrated graphics", "vram_mb": 0, "tier": "entry", "source": "i915" }));
        }
    }
    None
}

fn tier_from_name_and_vram(name: &str, vram_mb: i64) -> &'static str {
    let lowered = name.to_lowercase();
    if lowered.contains("llvmpipe") || lowered.contains("software") {
        return "software";
    }
    if lowered.contains("rtx 40") || lowered.contains("rtx 50") || lowered.contains("rx 7900") || lowered.contains("rx 90") || lowered.contains("arc a7") || vram_mb >= 16384 {
        return "enthusiast";
    }
    if lowered.contains("rtx 30") || lowered.contains("rtx 20") || lowered.contains("rx 68") || lowered.contains("rx 67") || lowered.contains("rx 78") || lowered.contains("gtx 1080") || vram_mb >= 8192 {
        return "high";
    }
    if lowered.contains("gtx 16") || lowered.contains("gtx 10") || lowered.contains("rx 5") || lowered.contains("rx 64") || vram_mb >= 4096 {
        return "mid";
    }
    "entry"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_dir(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nivora-shader-{tag}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("shaders")).unwrap();
        root
    }

    #[test]
    fn legacy_pack_supports_old_and_new_versions() {
        let root = sample_dir("legacy");
        fs::write(root.join("shaders/composite.vsh"), "#version 120\nvarying vec2 uv;\nvoid main(){ gl_Position = ftransform(); }").unwrap();
        fs::write(root.join("shaders/composite.fsh"), "#version 120\nuniform sampler2D gtexture;\nvoid main(){ gl_FragColor = texture2D(gtexture, vec2(0.5)); }").unwrap();
        let payload = analyze_shaderpack(&root, Some("1.12.2")).unwrap();
        assert_eq!(payload["versions"]["requested"], json!("1.12.2"));
        let verdict = payload["versions"]["verdict"].as_object().unwrap().clone();
        assert_eq!(verdict["status"], json!("ok"));
        let matrix = payload["versions"]["matrix"].as_array().unwrap();
        let old = matrix.iter().find(|entry| entry["version"] == json!("1.8.9")).unwrap();
        assert_eq!(old["status"], json!("ok"));
        assert_eq!(old["loader"], json!("OptiFine"));
        assert_eq!(payload["glsl"]["legacy_syntax"], json!(true));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn compute_pack_requires_iris() {
        let root = sample_dir("compute");
        fs::write(root.join("shaders/final.fsh"), "#version 330 compatibility\nvoid main(){}").unwrap();
        fs::write(root.join("shaders/compute.csh"), "#version 430 core\nvoid main(){}").unwrap();
        let payload = analyze_shaderpack(&root, Some("1.12.2")).unwrap();
        let matrix = payload["versions"]["matrix"].as_array().unwrap();
        let old = matrix.iter().find(|entry| entry["version"] == json!("1.12.2")).unwrap();
        assert_eq!(old["status"], json!("broken"));
        let modern = matrix.iter().find(|entry| entry["version"] == json!("1.20.1")).unwrap();
        assert_eq!(modern["status"], json!("ok"));
        assert_eq!(modern["loader"], json!("Iris"));
        assert_eq!(payload["glsl"]["compute_shaders"], json!(true));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn declared_supported_versions_win() {
        let root = sample_dir("declared");
        fs::write(root.join("shaders/final.fsh"), "#version 120\nvoid main(){}").unwrap();
        fs::write(root.join("shaders/shaders.properties"), "shadowMapResolution=2048\nsupportedVersions=1.16.5-1.21.1\n").unwrap();
        let payload = analyze_shaderpack(&root, Some("1.20.1")).unwrap();
        let matrix = payload["versions"]["matrix"].as_array().unwrap();
        let inside = matrix.iter().find(|entry| entry["version"] == json!("1.20.1")).unwrap();
        assert_eq!(inside["status"], json!("ok"));
        let outside = matrix.iter().find(|entry| entry["version"] == json!("1.8.9")).unwrap();
        assert_eq!(outside["status"], json!("broken"));
        assert_eq!(payload["shadow_resolution"], json!(2048));
        assert_eq!(payload["versions"]["min_mc"], json!("1.16.5"));
        assert_eq!(payload["versions"]["max_mc"], json!("1.21.1"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn core_profile_needs_modern_game() {
        let root = sample_dir("core");
        fs::write(root.join("shaders/composite.fsh"), "#version 330 core\nvoid main(){}").unwrap();
        let payload = analyze_shaderpack(&root, Some("1.8.9")).unwrap();
        let matrix = payload["versions"]["matrix"].as_array().unwrap();
        let old = matrix.iter().find(|entry| entry["version"] == json!("1.8.9")).unwrap();
        assert_eq!(old["status"], json!("broken"));
        let modern = matrix.iter().find(|entry| entry["version"] == json!("1.19.2")).unwrap();
        assert_eq!(modern["status"], json!("ok"));
        assert_eq!(payload["glsl"]["core_profile"], json!(true));
        assert_eq!(payload["glsl"]["max_version"], json!(330));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn no_shader_programs_is_an_error() {
        let root = sample_dir("empty");
        fs::write(root.join("readme.txt"), "just text").unwrap();
        assert!(analyze_shaderpack(&root, None).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
