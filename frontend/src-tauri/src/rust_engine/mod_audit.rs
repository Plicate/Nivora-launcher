use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use zip::ZipArchive;

const AUDIT_MAX_JAR_BYTES: u64 = 192 * 1024 * 1024;
const AUDIT_MAX_ENTRIES: usize = 16384;
const AUDIT_MAX_MEMBER_BYTES: u64 = 96 * 1024 * 1024;
const AUDIT_MAX_CLASS_BYTES: u64 = 24 * 1024 * 1024;
const AUDIT_MAX_UNPACKED_BYTES: u64 = 512 * 1024 * 1024;
const AUDIT_MAX_NESTED_JAR_BYTES: u64 = 48 * 1024 * 1024;
const AUDIT_MAX_NESTED_TOTAL_BYTES: u64 = 192 * 1024 * 1024;
const AUDIT_MAX_NESTED_JARS: usize = 96;
const AUDIT_MAX_NESTED_DEPTH: usize = 2;
const AUDIT_MAX_STRINGS_PER_CLASS: usize = 4096;
const AUDIT_MAX_REFS_PER_CLASS: usize = 8192;
const AUDIT_MAX_SOURCE_LINE: usize = 2048;
const AUDIT_MAX_STRING_CONSTANT: usize = 1024;
const AUDIT_MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const SESSION_TTL_SECONDS: f64 = 30.0 * 60.0;
const SESSION_RESULT_TTL_SECONDS: f64 = 6.0 * 60.0 * 60.0;
const SESSION_MAX_ACTIVE: usize = 4;
const MAX_EVIDENCE_PER_FINDING: usize = 10;
const ESTIMATE_SAMPLE_LINES: usize = 1500;
const ESTIMATE_MIN_SECONDS: i64 = 1;
const ESTIMATE_MAX_SECONDS: i64 = 1800;

const SEVERITY_ORDER: [&str; 4] = ["critical", "warning", "notice", "info"];
const SEVERITY_SCORE: [(&str, i64); 4] = [("critical", 25), ("warning", 8), ("notice", 2), ("info", 0)];
const VERDICT_LABELS: [(&str, &str); 4] = [
    ("dangerous", "Обнаружены опасные сигналы"),
    ("suspicious", "Есть подозрительные сигналы"),
    ("review", "Есть замечания"),
    ("clean", "Угроз не обнаружено"),
];

fn severity_score(severity: &str) -> i64 {
    SEVERITY_SCORE.iter().find(|(key, _)| *key == severity).map(|(_, value)| *value).unwrap_or(0)
}

fn severity_index(severity: &str) -> usize {
    SEVERITY_ORDER.iter().position(|value| *value == severity).unwrap_or(SEVERITY_ORDER.len())
}

fn verdict_label(verdict: &str) -> &'static str {
    VERDICT_LABELS.iter().find(|(key, _)| *key == verdict).map(|(_, value)| *value).unwrap_or("")
}

const CLASS_MAGIC: [u8; 4] = [0xCA, 0xFE, 0xBA, 0xBE];
const CP_UTF8: u8 = 1;
const CP_INTEGER: u8 = 3;
const CP_FLOAT: u8 = 4;
const CP_LONG: u8 = 5;
const CP_DOUBLE: u8 = 6;
const CP_CLASS: u8 = 7;
const CP_STRING: u8 = 8;
const CP_FIELDREF: u8 = 9;
const CP_METHODREF: u8 = 10;
const CP_INTERFACE_METHODREF: u8 = 11;
const CP_NAME_AND_TYPE: u8 = 12;
const CP_METHODHANDLE: u8 = 15;
const CP_METHODTYPE: u8 = 16;
const CP_DYNAMIC: u8 = 17;
const CP_INVOKEDYNAMIC: u8 = 18;
const CP_MODULE: u8 = 19;
const CP_PACKAGE: u8 = 20;

/// факты constant-pool одного class-файла
struct ClassInfo {
    name: String,
    super_name: String,
    major: u16,
    minor: u16,
    interfaces: Vec<String>,
    class_refs: Vec<String>,
    method_refs: Vec<String>,
    field_refs: Vec<String>,
    strings: Vec<String>,
}

struct ClassReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> ClassReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn take(&mut self, size: usize) -> Result<&'a [u8], String> {
        let end = self.position + size;
        if end > self.data.len() {
            return Err("Повреждённый class-файл: неожиданный конец данных".to_owned());
        }
        let chunk = &self.data[self.position..end];
        self.position = end;
        Ok(chunk)
    }

    fn u1(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u2(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }
}

fn decode_mutf8(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

/// разбор class-файла: строки, классы, полные ссылки на методы и поля
fn parse_class_file(data: &[u8], entry: &str) -> Result<ClassInfo, String> {
    let _ = entry;
    if data.len() < 10 {
        return Err("Повреждённый class-файл: файл слишком мал".to_owned());
    }
    if data[..4] != CLASS_MAGIC {
        return Err("Файл не является Java class-файлом".to_owned());
    }
    let mut reader = ClassReader::new(&data[4..]);
    let minor = reader.u2()?;
    let major = reader.u2()?;
    let count = reader.u2()?;
    if count == 0 {
        return Err("Повреждённый class-файл: пустой constant pool".to_owned());
    }

    let mut utf8: HashMap<u16, String> = HashMap::new();
    let mut classes: HashMap<u16, u16> = HashMap::new();
    let mut name_and_type: HashMap<u16, (u16, u16)> = HashMap::new();
    let mut member_refs: Vec<(u16, u16, u8)> = Vec::new();
    let mut string_refs: Vec<u16> = Vec::new();

    let mut index: usize = 1;
    while index < count as usize {
        let tag = reader.u1()?;
        match tag {
            CP_UTF8 => {
                let length = reader.u2()? as usize;
                let raw = reader.take(length)?;
                utf8.insert(index as u16, decode_mutf8(raw));
            }
            CP_INTEGER | CP_FLOAT => {
                reader.take(4)?;
            }
            CP_LONG | CP_DOUBLE => {
                reader.take(8)?;
                index += 1;
                if index >= count as usize {
                    return Err("Повреждённый class-файл: неверный constant pool".to_owned());
                }
            }
            CP_CLASS | CP_STRING | CP_METHODTYPE | CP_MODULE | CP_PACKAGE => {
                let reference = reader.u2()?;
                if tag == CP_CLASS {
                    classes.insert(index as u16, reference);
                } else if tag == CP_STRING {
                    string_refs.push(reference);
                }
            }
            CP_FIELDREF | CP_METHODREF | CP_INTERFACE_METHODREF => {
                let class_index = reader.u2()?;
                let nat_index = reader.u2()?;
                member_refs.push((class_index, nat_index, if tag == CP_FIELDREF { 1 } else { 0 }));
            }
            CP_NAME_AND_TYPE => {
                let name_index = reader.u2()?;
                let descriptor_index = reader.u2()?;
                name_and_type.insert(index as u16, (name_index, descriptor_index));
            }
            CP_METHODHANDLE => {
                reader.u1()?;
                reader.u2()?;
            }
            CP_DYNAMIC | CP_INVOKEDYNAMIC => {
                reader.u2()?;
                reader.u2()?;
            }
            other => return Err(format!("Повреждённый class-файл: неизвестный тег {other}")),
        }
        index += 1;
    }

    let utf8_value = |idx: u16| -> String { utf8.get(&idx).cloned().unwrap_or_default() };
    let class_name = |idx: u16| -> String { classes.get(&idx).map(|name_index| utf8_value(*name_index)).unwrap_or_default() };

    let mut info = ClassInfo {
        name: String::new(),
        super_name: String::new(),
        major,
        minor,
        interfaces: Vec::new(),
        class_refs: Vec::new(),
        method_refs: Vec::new(),
        field_refs: Vec::new(),
        strings: Vec::new(),
    };
    reader.u2()?; // access_flags
    let this_index = reader.u2()?;
    let super_index = reader.u2()?;
    info.name = {
        let name = class_name(this_index);
        if name.is_empty() { "unknown/Unknown".to_owned() } else { name }
    };
    info.super_name = class_name(super_index);
    let interfaces_count = reader.u2()?;
    for _ in 0..interfaces_count.min(256) {
        info.interfaces.push(class_name(reader.u2()?));
    }

    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for name_index in classes.values() {
        let name = utf8_value(*name_index);
        if !name.is_empty() {
            referenced.insert(name);
        }
    }
    referenced.remove(&info.name);
    info.class_refs = referenced.into_iter().collect();

    let mut method_refs: BTreeSet<String> = BTreeSet::new();
    let mut field_refs: BTreeSet<String> = BTreeSet::new();
    for (class_index, nat_index, is_field) in member_refs {
        let owner = class_name(class_index);
        let name = match name_and_type.get(&nat_index) {
            Some((name_index, _)) => utf8_value(*name_index),
            None => String::new(),
        };
        if owner.is_empty() || name.is_empty() {
            continue;
        }
        let target = format!("{owner}.{name}");
        if is_field == 1 {
            field_refs.insert(target);
        } else {
            method_refs.insert(target);
        }
    }
    info.method_refs = method_refs.into_iter().take(AUDIT_MAX_REFS_PER_CLASS).collect();
    info.field_refs = field_refs.into_iter().take(AUDIT_MAX_REFS_PER_CLASS).collect();

    let mut strings: BTreeSet<String> = BTreeSet::new();
    for reference in string_refs {
        let value = utf8_value(reference);
        if !value.is_empty() {
            strings.insert(value.chars().take(AUDIT_MAX_STRING_CONSTANT).collect());
        }
        if strings.len() >= AUDIT_MAX_STRINGS_PER_CLASS {
            break;
        }
    }
    info.strings = strings.into_iter().collect();
    Ok(info)
}

/// псевдоисходник .jasm из фактов constant-pool: по строке на факт,
/// первая буква задаёт тип правила: v/c/s/i/r/m/f/k
fn render_class_source(info: &ClassInfo) -> String {
    let mut lines: Vec<String> = vec![format!("v {}.{}", info.major, info.minor), format!("c {}", info.name)];
    if !info.super_name.is_empty() {
        lines.push(format!("s {}", info.super_name));
    }
    for interface in info.interfaces.iter().take(64) {
        lines.push(format!("i {interface}"));
    }
    for reference in info.class_refs.iter().take(AUDIT_MAX_REFS_PER_CLASS) {
        lines.push(format!("r {reference}"));
    }
    for reference in &info.method_refs {
        lines.push(format!("m {reference}"));
    }
    for reference in &info.field_refs {
        lines.push(format!("f {reference}"));
    }
    for value in &info.strings {
        let encoded = serde_json::to_string(value).unwrap_or_default();
        lines.push(format!("k {encoded}"));
    }
    lines.push(String::new());
    lines.join("\n")
}

// ── обход архива ───────────────────────────────────────────────────

const NATIVE_EXTENSIONS: [&str; 4] = [".dll", ".so", ".dylib", ".jnilib"];

fn manifest_platforms() -> &'static [(&'static str, &'static str)] {
    &[
        ("paper-plugin.yml", "paper"),
        ("neoforge.mods.toml", "neoforge"),
        ("META-INF/mods.toml", "forge"),
        ("fabric.mod.json", "fabric"),
        ("quilt.mod.json", "quilt"),
        ("plugin.yml", "bukkit"),
        ("bungee.yml", "bungee"),
    ]
}

fn platform_priority(platform: &str) -> i64 {
    match platform {
        "paper" => 7,
        "neoforge" => 6,
        "forge" => 5,
        "fabric" => 4,
        "quilt" => 3,
        "bukkit" => 2,
        "bungee" => 1,
        _ => 0,
    }
}

#[derive(Default)]
struct JarStructure {
    platform: String,
    declared: Map<String, Value>,
    class_count: usize,
    string_count: usize,
    reference_count: usize,
    natives: Vec<String>,
    nested_jars: Vec<String>,
    skipped_entries: Vec<String>,
    signed: bool,
    sources_bytes: usize,
    source_lines: usize,
}

impl JarStructure {
    fn new() -> Self {
        Self { platform: "unknown".to_owned(), ..Default::default() }
    }
}

fn entry_name_pattern() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._$/-]{0,255}$").expect("entry name")
    })
}

fn validate_entry_name(name: &str) -> Result<String, String> {
    if name.is_empty() || name.contains('\0') || name.contains('\\') {
        return Err(format!("Недопустимое имя файла в архиве: {}", name.chars().take(120).collect::<String>()));
    }
    if name.starts_with('/') || name.split('/').any(|part| part == "..") || !entry_name_pattern().is_match(name) {
        return Err(format!("Недопустимое имя файла в архиве: {}", name.chars().take(120).collect::<String>()));
    }
    Ok(name.to_owned())
}

fn source_path_for_entry(entry: &str) -> Result<String, String> {
    let name = entry.strip_suffix(".class").unwrap_or(entry);
    validate_entry_name(&format!("{name}.jasm"))
}

/// извлекает из YAML только ключи верхнего уровня
fn parse_simple_yaml(text: &str) -> BTreeMap<String, String> {
    let mut root: BTreeMap<String, String> = BTreeMap::new();
    for raw_line in text.lines() {
        if raw_line.trim().is_empty() || raw_line.trim_start().starts_with('#') {
            continue;
        }
        let line = raw_line.trim_end();
        let indent = line.len() - line.trim_start_matches(' ').len();
        if indent != 0 {
            continue;
        }
        let content = line.trim();
        if content.starts_with("- ") || !content.contains(':') {
            continue;
        }
        let (key, raw_value) = content.split_once(':').unwrap_or((content, ""));
        let key = key.trim().trim_matches(|c| c == '\'' || c == '"');
        let mut value = raw_value.trim();
        if value.starts_with('[') && value.ends_with(']') {
            value = &value[1..value.len() - 1];
        }
        let cleaned = value.trim_matches(|c| c == '\'' || c == '"');
        root.insert(key.to_owned(), cleaned.to_owned());
    }
    root
}

fn clip(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn manifest_metadata(name: &str, data: &[u8]) -> Map<String, Value> {
    let text = String::from_utf8_lossy(data);
    let mut metadata = Map::new();
    metadata.insert("manifest".to_owned(), json!(name));
    if name.ends_with(".json") {
        let Ok(Value::Object(document)) = serde_json::from_str::<Value>(&text) else {
            return metadata;
        };
        let inner = match document.get("quilt_loader") {
            Some(Value::Object(quilt)) => quilt.clone(),
            _ => document,
        };
        let field = |key: &str, limit: usize| -> String {
            inner.get(key).and_then(Value::as_str).map(|value| clip(value, limit)).unwrap_or_default()
        };
        metadata.insert("id".to_owned(), json!(field("id", 160)));
        metadata.insert("version".to_owned(), json!(field("version", 64)));
        metadata.insert("name".to_owned(), json!(field("name", 160)));
        if let Some(Value::Array(authors)) = inner.get("authors") {
            let values: Vec<String> = authors.iter().take(12).map(|item| clip(item.to_string().trim_matches('"'), 80)).collect();
            metadata.insert("authors".to_owned(), json!(values));
        }
        for key in ["homepage", "source"] {
            if let Some(value) = inner.get(key).and_then(Value::as_str) {
                if !value.is_empty() {
                    metadata.insert(key.to_owned(), json!(clip(value, 300)));
                }
            }
        }
        if let Some(Value::Object(entrypoints)) = inner.get("entrypoints") {
            let mut values: Vec<String> = entrypoints
                .values()
                .filter_map(Value::as_str)
                .map(|value| clip(value, 200))
                .collect();
            values.sort();
            metadata.insert("entrypoints".to_owned(), json!(values.into_iter().take(16).collect::<Vec<_>>()));
        }
        if let Some(Value::Object(depends)) = inner.get("depends") {
            let mut keys: Vec<String> = depends.keys().map(|key| clip(key, 80)).collect();
            keys.sort();
            metadata.insert("depends".to_owned(), json!(keys.into_iter().take(24).collect::<Vec<_>>()));
        }
        return metadata;
    }
    let values = parse_simple_yaml(&text);
    for key in ["name", "version", "main", "api-version", "bootstrapper", "author", "authors", "website", "description"] {
        if let Some(value) = values.get(key) {
            if !value.is_empty() {
                metadata.insert(key.to_owned(), json!(clip(value, 300)));
            }
        }
    }
    metadata
}

fn declared_permissions_from_manifest(name: &str, data: &[u8]) -> Vec<String> {
    if !name.ends_with(".yml") {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(data);
    let mut permissions: BTreeSet<String> = BTreeSet::new();
    // (?m)^\s+(?:-\s+)?([A-Za-z0-9.*:_.-]+)\s*:?\s*$
    for raw_line in text.lines() {
        let trimmed = raw_line.trim_end();
        let indent_len = trimmed.len() - trimmed.trim_start_matches(' ').len();
        if indent_len == 0 {
            continue;
        }
        let mut content = trimmed.trim_start();
        if let Some(rest) = content.strip_prefix("- ") {
            content = rest;
        }
        let candidate = content.trim_end_matches(':').trim_end();
        if !candidate.is_empty()
            && candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '*' | ':' | '_' | '-'))
            && !candidate.ends_with(':')
        {
            permissions.insert(clip(candidate, 120));
        }
    }
    permissions.into_iter().take(64).collect()
}

/// проверяет архив и записывает псевдоисходники `.jasm`
fn inspect_and_write_sources<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    sources_dir: &Path,
    structure: &mut JarStructure,
    prefix: &str,
    depth: usize,
    nested: bool,
    total_unpacked: &mut u64,
) -> Result<(), String> {
    if archive.len() > AUDIT_MAX_ENTRIES {
        return Err("В архиве слишком много файлов".to_owned());
    }
    let mut manifests: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for index in 0..archive.len() {
        let Ok(mut member) = archive.by_index(index) else { continue };
        let name = member.name().map(|v| v.to_string()).unwrap_or_default();
        if member.is_dir() {
            continue;
        }
        let lowered = name.to_lowercase();
        if member.size() > AUDIT_MAX_MEMBER_BYTES {
            structure.skipped_entries.push(format!("{prefix}{name} (слишком большой)"));
            continue;
        }
        if NATIVE_EXTENSIONS.iter().any(|extension| lowered.ends_with(extension)) {
            structure.natives.push(format!("{prefix}{name}"));
            continue;
        }
        if name.starts_with("META-INF/") && [".sf", ".rsa", ".dsa", ".ec"].iter().any(|extension| lowered.ends_with(extension)) {
            structure.signed = true;
            continue;
        }
        if !nested && manifest_platforms().iter().any(|(manifest, _)| *manifest == name) {
            if member.size() > AUDIT_MAX_MANIFEST_BYTES {
                structure.skipped_entries.push(format!("{prefix}{name} (manifest слишком большой)"));
                continue;
            }
            let mut data = Vec::new();
            if member.read_to_end(&mut data).is_err() {
                continue;
            }
            let platform = manifest_platforms().iter().find(|(manifest, _)| *manifest == name).map(|(_, value)| *value).unwrap_or("");
            if platform_priority(platform) > platform_priority(&structure.platform) {
                structure.platform = platform.to_owned();
            }
            manifests.insert(name, data);
            continue;
        }
        if lowered.ends_with(".jar") {
            if depth >= AUDIT_MAX_NESTED_DEPTH {
                structure.skipped_entries.push(format!("{prefix}{name} (вложенность jar)"));
                continue;
            }
            if structure.nested_jars.len() >= AUDIT_MAX_NESTED_JARS || member.size() > AUDIT_MAX_NESTED_JAR_BYTES {
                structure.skipped_entries.push(format!("{prefix}{name} (лимит вложенных jar)"));
                continue;
            }
            structure.nested_jars.push(format!("{prefix}{name}"));
            let mut data = Vec::new();
            if member.read_to_end(&mut data).is_err() {
                structure.skipped_entries.push(format!("{prefix}{name} (не читается как zip)"));
                continue;
            }
            inspect_nested_jar(data, &format!("{prefix}{name}"), sources_dir, structure, depth + 1, total_unpacked);
            continue;
        }
        if !lowered.ends_with(".class") {
            if member.size() > 64 * 1024 {
                structure.skipped_entries.push(format!("{prefix}{name} (не class-файл)"));
            }
            continue;
        }
        if member.size() > AUDIT_MAX_CLASS_BYTES {
            structure.skipped_entries.push(format!("{prefix}{name} (class слишком большой)"));
            continue;
        }
        *total_unpacked += member.size();
        if *total_unpacked > AUDIT_MAX_UNPACKED_BYTES {
            return Err("Распакованный объём превышает лимит аудита".to_owned());
        }
        let mut data = Vec::new();
        if member.read_to_end(&mut data).is_err() {
            structure.skipped_entries.push(format!("{prefix}{name} (повреждён)"));
            continue;
        }
        let entry_result = parse_class_file(&data, &format!("{prefix}{name}"));
        let info = match entry_result {
            Ok(info) => info,
            Err(error) => {
                structure.skipped_entries.push(format!("{prefix}{name} ({error})"));
                continue;
            }
        };
        let target_name = if prefix.is_empty() { name.clone() } else { flatten_nested(prefix, &name) };
        let relative = match source_path_for_entry(&target_name) {
            Ok(value) => value,
            Err(error) => {
                structure.skipped_entries.push(format!("{prefix}{name} ({error})"));
                continue;
            }
        };
        let target = sources_dir.join(&relative);
        if let Some(parent) = target.parent() {
            let _ = fs::create_dir_all(parent);
            super::platform::set_mode(parent, 0o700);
        }
        let rendered = render_class_source(&info);
        if fs::write(&target, rendered.as_bytes()).is_err() {
            structure.skipped_entries.push(format!("{prefix}{name} (повреждён)"));
            continue;
        }
        structure.class_count += 1;
        structure.string_count += info.strings.len();
        structure.reference_count += info.method_refs.len() + info.field_refs.len() + info.class_refs.len();
        structure.sources_bytes += rendered.len();
        structure.source_lines += rendered.matches('\n').count();
    }

    if !manifests.is_empty() {
        let mut merged: Map<String, Value> = Map::new();
        let mut declared_permissions: BTreeSet<String> = BTreeSet::new();
        for (name, data) in &manifests {
            for (key, value) in manifest_metadata(name, data) {
                merged.insert(key, value);
            }
            declared_permissions.extend(declared_permissions_from_manifest(name, data));
        }
        if !declared_permissions.is_empty() {
            let mut sorted: Vec<String> = declared_permissions.into_iter().take(64).collect();
            sorted.sort();
            merged.insert("declared_permissions".to_owned(), json!(sorted));
        }
        structure.declared = merged;
    }
    Ok(())
}

fn flatten_nested(prefix: &str, name: &str) -> String {
    let inner = prefix.trim_end_matches('/');
    let Some(stem) = inner.strip_suffix(".jar") else {
        return format!("{prefix}{name}");
    };
    let flattened = format!("{}__nested", stem.replace('/', "__"));
    match validate_entry_name(&format!("{flattened}/{name}")) {
        Ok(value) => value,
        Err(_) => format!("{prefix}{name}"),
    }
}

fn inspect_nested_jar(data: Vec<u8>, label: &str, sources_dir: &Path, structure: &mut JarStructure, depth: usize, total_unpacked: &mut u64) {
    *total_unpacked += data.len() as u64;
    if *total_unpacked > AUDIT_MAX_NESTED_TOTAL_BYTES {
        structure.skipped_entries.push(format!("{label} (распакованный объём превышает лимит аудита)"));
        return;
    }
    let Ok(mut archive) = ZipArchive::new(Cursor::new(data)) else {
        structure.skipped_entries.push(format!("{label} (не читается как zip)"));
        return;
    };
    if let Err(error) = inspect_and_write_sources(&mut archive, sources_dir, structure, &format!("{label}/"), depth, true, total_unpacked) {
        structure.skipped_entries.push(format!("{label} ({error})"));
    }
}

// ── правила аудита ─────────────────────────────────────────────────

struct AuditRule {
    id: &'static str,
    category: &'static str,
    severity: &'static str,
    kind: &'static str, // c class refs, m method refs, f field refs, k strings
    pattern: regex::Regex,
    title: &'static str,
    detail: &'static str,
    recommendation: &'static str,
}

/// правила c/m/f должны совпасть со строкой целиком; правило k ищет подстроку
#[allow(clippy::too_many_arguments)]
fn rule(
    rule_id: &'static str,
    category: &'static str,
    severity: &'static str,
    kind: &'static str,
    pattern: &str,
    title: &'static str,
    detail: &'static str,
    recommendation: &'static str,
) -> AuditRule {
    let pattern = pattern.replace("\\Z", "\\z");
    let compiled = if kind == "k" {
        regex::Regex::new(&pattern).unwrap_or_else(|error| panic!("правило {rule_id}: {error}"))
    } else {
        regex::Regex::new(&format!("^(?:{pattern})$")).unwrap_or_else(|error| panic!("правило {rule_id}: {error}"))
    };
    AuditRule { id: rule_id, category, severity, kind, pattern: compiled, title, detail, recommendation }
}

const SENSITIVE_ACCOUNT_FILES: &str = r"(?i)(launcher[_-]?accounts|launcher[_-]?profiles|accounts\.json|essentials?/userdata|session[_-]?token|refresh[_-]?token|access[_-]?token|client[_-]?secret|logins\.json|signons\.sqlite|key3\.db|key4\.db|id_rsa|known_hosts|\.aws/credentials|webco?okies?\.?dat|privatetokens)";
const SENSITIVE_SYSTEM_FILES: &str = r"(?i)(/etc/(passwd|shadow|sudoers|hosts)\b|\\sam(\.bak)?$|system32\\config|lsass|mimikatz|procdump|/proc/self/environ|\.docker/config\.json|\.kube/config|wallet\.dat|keystore\.jks)";
const SENSITIVE_GAME_FILES: &str = r"(?i)(\.minecraft[/\\]|options\.txt|servers\.dat|usercache\.json|logs/latest\.log|launcher_log|\.minecraft/logs|world/players|playerdata/|session\.lock|screenshots/)";
const EXFIL_ENDPOINTS: &str = r"(?i)(discord(?:app)?\.com/api/webhooks|discord\.com/api/v\d+/webhooks|api\.telegram\.org/bot|pastebin\.com/api|hasteb(?:in|\.cc)|transfer\.sh|anonfiles|file\.io|gofile\.io|hookb\.in|webhook\.site|requestbin|pipedream\.net|beeceptor|ngrok\.(io|com)|trycloudflare\.com|loca\.lt|serveo\.net)";
const SHELL_STRINGS: &str = r"(?i)(/bin/(ba|z|da)?sh\b|cmd\.exe|powershell(\.exe)?\b|pwsh\b|mshta\s|certutil\s+-|bitsadmin\s|certutil\.exe|bash\s+-c|sh\s+-c|reg\s+(add|query)|taskkill\s|net\s+user\b|whoami(\.exe)?\b|nslookup\s|ping\s+-n|del\s+/[fqs]|rm\s+-rf)";
const PERSISTENCE_STRINGS: &str = r"(?i)(schtasks(\.exe)?\s|/etc/cron|crontab|CurrentVersion\\\\?Run|/etc/rc\.local|systemd/system|launchd|~/\.bashrc|~/\.zshrc|Startup\\\\?Folder|autoexec\.bat)";
const RAW_IP_STRING: &str = r"(?i)(?:https?://(?:\d{1,3}\.){3}\d{1,3}(?:[:/]\S*)?|\b(?:\d{1,3}\.){3}\d{1,3}:\d{2,5}\b)";
const ALLOWLISTED_HOSTS: &str = r"(?i)\b((?:api|meta)\.modrinth\.com|modrinth\.com|api\.hangar\.papermc\.io|hangar\.papermc\.io|(?:hub|www)\.spigotmc\.org|api\.spiget\.org|api\.github\.com|github\.com|raw\.githubusercontent\.com|objects\.githubusercontent\.com|api\.curseforge\.com|minecraftforge\.net|maven\.minecraftforge\.net|meta\.fabricmc\.net|maven\.fabricmc\.net|maven\.quiltmc\.org|quiltmc\.org|papermc\.io|api\.papermc\.io|repo\.papermc\.io|bstats\.org|metrics\.bstats\.org|api\.sentry\.io|sentry\.io|launcher\.mojang\.com|piston-meta\.mojang\.com|piston-data\.mojang\.com|libraries\.minecraft\.net|resources\.download\.minecraft\.net)";

fn allowlisted_hosts() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| regex::Regex::new(ALLOWLISTED_HOSTS).expect("allowlisted hosts"))
}

fn class_rules() -> Vec<AuditRule> {
    vec![
        rule("hardware-probe", "system", "notice", "c",
             r"(oshi/SystemInfo|oshi/hardware/[A-Za-z0-9$_]+|com/sun/manage/HotSpotDiagnosticMXBean)",
             "Инвентаризация железа", "Опрашивает характеристики процессора, памяти и дисков (OSHI/HotSpot).",
             "Иногда нужно для настроек производительности; иногда — сортировка жертв перед майнингом."),
        rule("keylogger-native-hook", "keylogger", "critical", "c",
             r"(org/jnativehook/.*|com/github/kwhat/jnativehook/.*)",
             "Глобальный перехват клавиатуры и мыши",
             "Подключается к библиотеке глобальных хуков (JNativeHook), перехватывающей нажатия клавиш и действия мыши во всей системе, а не только в игре.",
             "Такая возможность не нужна обычным модам и плагинам. Не запускайте файл, пока не убедитесь в его происхождении."),
        rule("keylogger-focus-manager", "keylogger", "warning", "c",
             r"(java/awt/KeyboardFocusManager|java/awt/event/KeyEventDispatcher|java/awt/event/KeyEventPostProcessor)",
             "Отслеживание фокуса и клавиатуры AWT",
             "Обращается к системному менеджеру клавиатурного фокуса, который может перехватывать все события клавиатуры приложения.",
             "Убедитесь, что это часть заявленной функциональности."),
        rule("keyboard-game-input", "keylogger", "info", "c",
             r"(org/lwjgl/input/Keyboard|org/lwjgl/input/Mouse|net/minecraft/client/option/KeyBinding|com/mojang/blaze3d/platform/Input)",
             "Чтение игровых клавиш", "Работает с игровым вводом (привязки клавиш) — нормально для модов с хоткеями.",
             "Не является перехватом вне игры."),
        rule("window-swing", "windows", "notice", "c",
             r"javax/swing/(JFrame|JDialog|JWindow|JOptionPane|JPopupMenu|JToolTip)",
             "Создание собственных окон (Swing)", "Может открывать собственные окна и диалоги поверх игры.",
             "Проверьте, что окна соответствуют заявленным функциям."),
        rule("window-awt", "windows", "notice", "c",
             r"java/awt/(Frame|Dialog|Window|FileDialog|TrayIcon)",
             "Создание собственных окон (AWT)", "Может открывать системные окна, диалоги и значки в трее.",
             "Проверьте, что окна соответствуют заявленным функциям."),
        rule("window-javafx", "windows", "notice", "c",
             r"javafx/(application/Application|stage/Stage|scene/Scene)",
             "Создание окон JavaFX", "Может открывать отдельные JavaFX-окна.",
             "Окна вне игры встречаются у лаунчер-модов; проверьте их назначение."),
        rule("window-webview", "windows", "warning", "c",
             r"(javafx/scene/web/WebView|javax/swing/JEditorPane)",
             "Встроенный браузер", "Может показывать веб-страницы внутри собственного окна — вектор фишинга и скрытой загрузки.",
             "Убедитесь, что плагин не открывает сторонние страницы с запросом данных."),
        rule("logging-frameworks", "logging", "info", "c",
             r"(java/util/logging/Logger|org/slf4j/Logger|org/apache/logging/log4j/(Logger|LogManager)|org/jboss/logmanager/Logger|org/apache/commons/logging/Log)",
             "Использование логирования", "Работает со стандартными библиотеками логирования.",
             "Обычное поведение: моды и плагины пишут логи."),
        rule("logging-file-handler", "logging", "notice", "c",
             r"java/util/logging/FileHandler", "Запись логов в файл", "Может создавать файлы журнала на диске.",
             "Проверьте, какие пути используются."),
        rule("files-io", "files", "info", "c",
             r"(java/io/File|java/io/FileInputStream|java/io/FileOutputStream|java/io/RandomAccessFile|java/io/FileReader|java/io/FileWriter|java/nio/file/Files|java/nio/file/Path|java/nio/file/Paths)",
             "Работа с файлами", "Использует файловый ввод-вывод — нормально для конфигураций и данных плагина.",
             "Обратите внимание на находки о чувствительных путях ниже."),
        rule("files-zip", "files", "notice", "c",
             r"java/util/zip/(ZipFile|ZipInputStream|ZipOutputStream)", "Работа с ZIP-архивами", "Читает или создаёт ZIP-архивы.",
             "Создание архивов вместе с сетевой отправкой может означать упаковку данных для выгрузки."),
        rule("network-url", "network", "notice", "c",
             r"(java/net/URL|java/net/URI|java/net/URLEncoder)", "Сетевые адреса", "Формирует сетевые адреса и URL.",
             "Само по себе не опасно; смотрите остальные находки сети."),
        rule("network-http", "network", "notice", "c",
             r"(java/net/HttpURLConnection|java/net/http/HttpClient|okhttp3/OkHttpClient|org/apache/http/(client|impl)/[A-Za-z0-9$]+|com/google/api/client/http/[A-Za-z0-9$]+)",
             "HTTP-клиент", "Может отправлять HTTP-запросы.", "Проверьте, куда отправляются данные."),
        rule("network-socket", "network", "warning", "c",
             r"(java/net/Socket|java/nio/channels/SocketChannel)", "Прямые TCP-соединения",
             "Может устанавливать прямые сокет-соединения с произвольными адресами.",
             "Для большинства модов это нетипично."),
        rule("network-listener", "network", "warning", "c",
             r"(java/net/ServerSocket|java/nio/channels/ServerSocketChannel)", "Открытие сетевого порта",
             "Может слушать входящие соединения на порту.", "Игровым модам и большинству плагинов это не нужно."),
        rule("network-websocket", "network", "notice", "c",
             r"(java/net/http/WebSocket|org/java_websocket/[A-Za-z0-9$]+|io/socket/client/[A-Za-z0-9$]+)",
             "WebSocket-соединения", "Использует WebSocket-клиент.",
             "Часто встречается у чат-интеграций; проверьте адрес."),
        rule("exec-script-engine", "process", "warning", "c",
             r"javax/script/ScriptEngineManager", "Встроенный движок скриптов", "Может исполнять JavaScript/Groovy-скрипты из строк.",
             "Скрипты из сети в сочетании с этим — признак скрытой загрузки кода."),
        rule("exec-compiler", "process", "warning", "c",
             r"(javax/tools/JavaCompiler|com/sun/source/util/JavacTask|javax/tools/ToolProvider)",
             "Компиляция кода на лету", "Может компилировать Java-код во время работы.",
             "Легитимно лишь для специализированных инструментов."),
        rule("exec-attach-agent", "process", "critical", "c",
             r"com/sun/tools/attach/[A-Za-z0-9$]+", "Подключение к чужой JVM",
             "Может внедряться в другие запущенные Java-процессы (java agent attach).",
             "Крайне опасно: так перехватывают чужие процессы, включая лаунчер."),
        rule("reflection-unsafe", "reflection", "warning", "c",
             r"(sun/misc/Unsafe|jdk/internal/misc/Unsafe|sun/misc/Signal)", "Использование Unsafe/сигналов",
             "Обращается к низкоуровневым API в обход безопасности JVM.",
             "Может использоваться как для оптимизаций, так и для обхода защит."),
        rule("native-jna", "native", "warning", "c",
             r"net/java/dev/jna/[A-Za-z0-9$]+", "Прямые вызовы нативных функций (JNA)",
             "Работает с нативным кодом напрямую через JNA.", "Оцените, какие нативные файлы лежат в самом JAR."),
        rule("crypto-cipher", "crypto", "notice", "c",
             r"javax/crypto/(Cipher|SecretKey|spec/[A-Za-z0-9$]+)", "Шифрование данных", "Использует javax.crypto.",
             "Вместе с обфускацией или сетью шифрование может скрывать полезную нагрузку."),
        rule("crypto-hash", "crypto", "info", "c",
             r"(java/security/MessageDigest|java/util/Base64)", "Хеширование и Base64", "Считает хеши или декодирует Base64.",
             "Обычное поведение; в сочетании с шифрованием строк может означать скрытие данных."),
        rule("server-console-sender", "server", "notice", "c",
             r"(org/bukkit/command/ConsoleCommandSender|org/bukkit/command/CommandMap)",
             "Работа с консолью сервера", "Плагин может отправлять команды от имени консоли.",
             "Проверьте, какие команды выполняются."),
        rule("chat-observer", "chat", "info", "c",
             r"(org/bukkit/event/player/AsyncPlayerChatEvent|io/papermc/paper/event/player/AsyncChatEvent|org/bukkit/event/player/PlayerChatEvent)",
             "Чтение игрового чата", "Плагин видит сообщения игроков в чате.",
             "Нормально для чат-плагинов; вместе с сетью может означать пересылку переписки."),
        rule("command-observer", "chat", "notice", "c",
             r"org/bukkit/event/player/PlayerCommandPreprocessEvent", "Наблюдение за командами игроков",
             "Плагин видит все команды, которые вводят игроки (включая пароли из /login и /register).",
             "В сочетании с сетью — канал утечки паролей."),
        rule("telemetry-bstats", "telemetry", "info", "c",
             r"org/bstats/[A-Za-z0-9$/.]+", "Стандартная метрика bStats", "Использует bStats — распространённую метрику плагинов.",
             "Отправляет только анонимную статистику использования."),
        rule("telemetry-analytics", "telemetry", "notice", "c",
             r"io/sentry/[A-Za-z0-9$/.]+", "Отчёты об ошибках (Sentry)", "Отправляет отчёты об ошибках на Sentry.",
             "Проверьте, настроен ли DSN на сервер автора."),
        rule("jndi-context", "jndi", "warning", "c",
             r"javax/naming/(InitialContext|ldap/[A-Za-z0-9$]+)", "JNDI-вызовы",
             "Может выполнять удалённые JNDI/LDAP-вызовы (вектор атак класса Log4Shell).",
             "Не типично для игровых модов."),
        rule("persistence-registry", "persistence", "notice", "c",
             r"java/util/prefs/Preferences", "Системный реестр/настройки", "Читает или пишет системные настройки пользователя.",
             "Само по себе допустимо; смотрите другие находки."),
        rule("deserialization", "deserialization", "notice", "c",
             r"java/io/ObjectInputStream", "Десериализация Java-объектов",
             "Десериализует данные; уязвима к подложенным объектам из сети.",
             "Опасно в сочетании с сетевым вводом."),
        rule("reflection-proxy-class", "reflection", "notice", "c",
             r"java/lang/reflect/Proxy", "Динамические прокси", "Создаёт прокси-объекты через рефлексию.",
             "Часто легитимно; в сочетании с загрузкой классов — подозрительно."),
    ]
}

fn string_rules() -> Vec<AuditRule> {
    vec![
        rule("keylogger-jna-hook", "keylogger", "critical", "k",
             r"(?i)(SetWindowsHookEx|GetAsyncKeyState|GetKeyState|user32\.dll|keybd_event|RegisterRawInputDevices|XGrabKey|XQueryKeymap)",
             "Системный перехват клавиатуры",
             "Найдены вызовы Windows/Linux API для перехвата клавиш на уровне всей системы.",
             "Явный признак кейлоггера."),
        rule("miner-pool-endpoints", "miner", "critical", "k",
             r"(?i)(stratum\+(tcp|ssl)|xmrig|xmr-stak|cryptonight|randomx\b|pool\.(minexmr|supportxmr|nanopool|c3pool|2miners)\.org|(xmr|eth|btc)\.(mine|pool)|nicehash\.com/api|hashrate.{0,20}worker)",
             "Признаки майнера криптовалют",
             "Найдены адреса майнинговых пулов или названия известных майнеров (XMRig, CryptoNight, RandomX).",
             "Игровым модам нечего считать хеши на вашем процессоре. Не запускайте файл."),
        rule("wallet-files", "wallets", "critical", "k",
             r"(?i)(metamask|exodus[ /\\-]wallet|electrum\.wallet|atomic[ /\\-]wallet|coinbase[ /\\]wallet|UTC--\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}|wallet\.dat|keystore\.json|seed[ -]?phrase|mnemonic[ -]?phrase|bip39)",
             "Доступ к криптокошелькам",
             "Код обращается к файлам или формату криптокошельков (MetaMask, Exodus, keystore, seed-фразы).",
             "Легитимной причины у мода читать кошельки нет — это кража средств."),
        rule("browser-credential-files", "wallets", "critical", "k",
             r"(?i)(Login Data( For Microsoft Edge)?|Web Data|cookies\.sqlite|Local Storage[\\/]+leveldb|AppData[\\/]+Local[\\/]+(Google[\\/]+Chrome|Microsoft[\\/]+Edge|BraveSoftware)|\.mozilla[\\/]firefox|Login Data For Chrome)",
             "Чтение данных браузеров",
             "Найдены пути к базам паролей и cookie-файлов браузеров (Chrome, Edge, Firefox).",
             "Мод не должен трогать профили браузеров — это извлечение паролей и сессий."),
        rule("discord-session-theft", "wallets", "critical", "k",
             r"(?i)(discord(?:app)?[\\/]+Local Storage|discord\.com/api/users/@me|discord\.com/api/v\d+/users/@me|Local Storage[\\/]+leveldb[\\/]+\S+\.ldb)",
             "Кража сессии Discord",
             "Код читает локальное хранилище Discord или обращается к API от имени пользователя.",
             "Так вытаскивают токен Discord, чтобы угнать аккаунт."),
        rule("disk-wiper", "destruction", "critical", "k",
             r"(?i)(rm\s+-rf\s+[/~]|del\s+/[fqs].*:\\\w+|format\s+[c-z]:|cipher\s+/w:|rd\s+/s\s+/q|shred\s+-|wipe\s+disk|mkfs\.\w+)",
             "Уничтожение данных", "Найдены команды удаления или форматирования больших областей диска.",
             "Такие команды в моде означают вандализм: файл будет стирать ваши данные."),
        rule("obfuscation-marker", "obfuscation", "warning", "k",
             r"(?i)(allatori|zelix|klassmaster|stringer\s*demo|dexguard|dasho|smokescreen|confuserex|obfuscat(ed|or))",
             "Признаки обфускатора", "В коде найдены строки-маркеры известных обфускаторов.",
             "Обфускация скрывает реальные имена классов и часто маскирует вредоносный код."),
        rule("anti-analysis", "obfuscation", "warning", "k",
             r"(?i)(isDebuggerPresent|CheckRemoteDebuggerPresent|anti-?debug|VMwareDetect|VirtualBoxDetector|sandbox_detect|frida_server)",
             "Анти-анализ и защита от отладки", "Код пытается обнаружить отладчик, виртуальную машину или анализ.",
             "Легитимный софт для игры так себя не ведёт."),
        rule("mc-auth-urls", "tokens", "warning", "k",
             r"(?i)(login\.microsoftonline\.com|login\.live\.com|auth\.minecraftservices\.com|sessionserver\.mojang\.com/session/minecraft/join|accounts\.google\.com/o/oauth2|api\.mojang\.com/users/profiles)",
             "Обращение к сервисам авторизации", "Содержит адреса сервисов входа Microsoft/Mojang.",
             "Моды-авторизации так не работают: это может быть перехватом учётных данных."),
        rule("tokens-launcher-config", "tokens", "warning", "k",
             r"(?i)(trial[-_]?launcher|launcher[_-]?settings\.json|launcher[_-]?config\.json|launcher[_-]?accounts\.json)",
             "Доступ к файлам лаунчера", "Упоминает файлы и каталоги лаунчера с аккаунтами и настройками.",
             "Моду не нужны файлы лаунчера."),
        rule("mc-session-files", "tokens", "warning", "k",
             r"(?i)((access|refresh)[-_]?token|msa[_-]?token|bearer\s+[a-z0-9._-]{8,}|xuid|player[_-]?session)",
             "Игровые токены и сессии", "Упоминает маркеры сессий Minecraft/Microsoft.",
             "Перехват токена даёт доступ к аккаунту."),
        rule("exfil-webhook", "network", "critical", "k", EXFIL_ENDPOINTS,
             "Канал утечки данных",
             "Содержит жёстко прописанные адреса рассылки данных (Discord webhook, Telegram bot, pastebin и т.п.).",
             "Такие адреса внутри кода означают отправку данных автору кода."),
        rule("network-raw-ip", "network", "warning", "k", RAW_IP_STRING,
             "Прямой IP-адрес", "Содержит строку с прямым IP-адресом вместо домена.",
             "Обход DNS и блок-листов; часто встречается в малвари."),
        rule("jndi-payload", "jndi", "critical", "k",
             r"\$\{jndi:(ldap|ldaps|rmi|dns|iiop)://", "JNDI-инъекция",
             "Содержит полезную нагрузку ${jndi:...} (класс Log4Shell).",
             "Эксплуатирует подстановку JNDI для удалённой загрузки кода."),
        rule("persistence-os", "persistence", "critical", "k", PERSISTENCE_STRINGS,
             "Закрепление в системе", "Создаёт автозапуск, задачи планировщика или правит системные скрипты.",
             "Моды и плагины так не распространяются."),
        rule("exec-shell-string", "process", "critical", "k", SHELL_STRINGS,
             "Команды операционной системы",
             "Найдены строки запуска системных команд и утилит (cmd, powershell, sh, certutil и т.п.).",
             "Модам почти никогда не нужны команды ОС."),
        rule("self-update-download", "update", "notice", "k",
             r"(?i)(github\.com/[\w.-]+/[\w.-]+/releases/download|/latest\.jar|self[-_]?update|autoupdate\.json|update\.json)",
             "Самообновление", "Содержит адреса автообновления и загрузки новых версий.",
             "Обновляемый код обходит эту аудиту и может измениться."),
        rule("files-sensitive-account", "files", "critical", "k", SENSITIVE_ACCOUNT_FILES,
             "Чтение учётных данных", "Ссылается на файлы аккаунтов, токенов, ключей и сессий.",
             "Моду не нужны ваши учётные данные."),
        rule("files-sensitive-system", "files", "critical", "k", SENSITIVE_SYSTEM_FILES,
             "Чтение системных файлов", "Ссылается на системные файлы с учётными данными ОС.",
             "Явный признак малвари."),
        rule("files-sensitive-game", "files", "warning", "k", SENSITIVE_GAME_FILES,
             "Чтение игровых данных вне своей папки",
             "Обращается к файлам .minecraft: настройки, логи, серверы, данные игроков.",
             "Часть данных (servers.dat, логи) может раскрывать приватную информацию."),
        rule("files-executable-write", "files", "warning", "k",
             r"(?i)[\w./\\-]+\.(exe|msi|bat|cmd|ps1|vbs|scr)\Z",
             "Запись исполняемых файлов", "Ссылается на создание исполняемых файлов Windows.",
             "Моды не должны создавать исполняемые файлы ОС."),
        rule("server-op-command", "server", "warning", "k",
             r"(?i)\A(op|minecraft:op)\s+\S+", "Выдача OP командой",
             "Содержит команду выдачи оператора с жёстко прописанным именем игрока.",
             "Признак бэкдора в плагине."),
        rule("server-files", "server", "warning", "k",
             r"(?i)(ops\.json|usercache\.json|whitelist\.json)\b", "Правка служебных файлов сервера",
             "Обращается к файлам операторов, белого списка или кэша игроков напрямую.",
             "Обычно такие файлы меняет только серверное ПО."),
        rule("rcon-client", "server", "notice", "k",
             r"(?i)\brcon\b", "RCON-подключение", "Работает с удалённым управлением сервером (RCON).",
             "Проверьте, куда подключается RCON и зачем."),
    ]
}

fn method_rules() -> Vec<AuditRule> {
    vec![
        rule("keylogger-robot", "keylogger", "critical", "m",
             r"java/awt/Robot\.(keyPress|keyRelease|mouseMove|mousePress|mouseRelease|mouseWheel|getPixelColor)",
             "Симуляция ввода через Robot",
             "Может программно нажимать клавиши и двигать мышь (java.awt.Robot).",
             "Используется для автоматизации ввода; в руках мода — управление игроком без вашего ведома."),
        rule("env-harvest", "system", "notice", "m",
             r"java/lang/System\.(getenv|getProperty)", "Чтение переменных окружения и свойств системы",
             "Читает переменные окружения и системные свойства (System.getenv/getProperty).",
             "Само по себе типично (пути, версия Java), но вместе с сетью позволяет снять окружение с токенами."),
        rule("dns-beacon", "network", "notice", "m",
             r"java/net/InetAddress\.(getAllByName|getByName|getHostName)", "DNS-запросы к произвольным именам",
             "Разрешает произвольные имена в адреса (InetAddress).",
             "Так устраивают DNS-маячки: сигнал «живой жертвы» уходит на подконтрольный домен."),
        rule("screenshot-capture", "surveillance", "critical", "m",
             r"java/awt/Robot\.createScreenCapture", "Съёмка скриншотов экрана", "Делает снимки всего экрана.",
             "В сочетании с сетью — слежка за экраном пользователя."),
        rule("keylogger-clipboard", "keylogger", "warning", "m",
             r"java/awt/datatransfer/Clipboard\.(getContents|setContents)", "Доступ к буферу обмена",
             "Читает или меняет системный буфер обмена.",
             "Чтение буфера позволяет копировать пароли и ссылки, скопированные вами."),
        rule("window-desktop-open", "windows", "notice", "m",
             r"java/awt/Desktop\.(browse|open|openFile|openMail|mail)", "Открытие внешних окон",
             "Открывает ссылки, файлы или почту во внешних приложениях (браузер, проводник).",
             "Проверяйте, что открываются только заявленные адреса."),
        rule("window-host-services", "windows", "notice", "m",
             r"javafx/application/HostServices\.showDocument", "Открытие ссылок JavaFX", "Открывает документы в браузере.",
             "Проверяйте открываемые адреса."),
        rule("window-dispose", "windows", "info", "m",
             r"(java/awt/Window|javax/swing/JFrame|java/awt/Frame)\.dispose", "Закрытие окон",
             "Закрывает собственные окна приложения.", "Обычное поведение GUI."),
        rule("window-event-queue", "windows", "warning", "m",
             r"java/awt/Toolkit\.getSystemEventQueue", "Доступ к очереди событий окон",
             "Получает системную очередь событий и может перехватывать или синтезировать события всех окон.",
             "Мощный API: используется кейлоггерами и автоматизацией."),
        rule("close-system-exit", "close", "warning", "m",
             r"java/lang/System\.exit", "Завершение JVM", "Может принудительно закрыть игру, сервер или лаунчер.",
             "Проверьте, в каких ситуациях вызывается выход."),
        rule("close-runtime-halt", "close", "warning", "m",
             r"java/lang/Runtime\.(halt|exit)", "Жёсткая остановка JVM",
             "Может немедленно завершить процесс без сохранения данных.",
             "Похоже на аварийное завершение или вредоносный выход."),
        rule("close-process-destroy", "close", "warning", "m",
             r"java/lang/(Process|ProcessHandle)\.(destroy|destroyForcibly)", "Убийство процессов",
             "Может завершать другие процессы (включая игру, сервер, лаунчер).",
             "Легитимно для менеджеров серверов; иначе подозрительно."),
        rule("close-game-shutdown", "close", "warning", "m",
             r"(net/minecraft/server/MinecraftServer\.(halt|stopServer|shutdownServer)|org/bukkit/Bukkit\.shutdown|net/minecraft/client/Minecraft\.(scheduleStop|close))",
             "Остановка игры/сервера", "Может останавливать сервер или закрывать игровой клиент.",
             "Проверьте, является ли это заявленной функцией."),
        rule("logging-get-logger", "logging", "info", "m",
             r"[A-Za-z0-9/$_]+\.getLogger", "Получение логгера", "Стандартное получение логгера.",
             "Не представляет риска."),
        rule("logging-stdout", "logging", "info", "m",
             r"java/io/PrintStream\.(print|println|printf|write)", "Вывод в консоль",
             "Пишет текст в стандартный вывод (консоль).", "Не представляет риска."),
        rule("files-dir-scan", "files", "notice", "m",
             r"(java/io/File\.(listFiles|list)|java/nio/file/Files\.(walk|walkFileTree|newDirectoryStream|find|lines))",
             "Перебор файлов и каталогов", "Мод сканирует содержимое каталогов, а не только читает отдельные файлы.",
             "Вместе с чувствительными путями или сетью — признак сбора данных."),
        rule("files-config-read", "files", "info", "m",
             r"(org/bukkit/configuration/file/YamlConfiguration\.(load|save)|org/bukkit/plugin/java/JavaPlugin\.(getConfig|saveConfig|saveDefaultConfig))",
             "Чтение собственной конфигурации", "Работает со своей конфигурацией Bukkit/YAML.",
             "Нормальное поведение плагина."),
        rule("network-open-connection", "network", "notice", "m",
             r"(java/net/URL\.(openConnection|openStream)|java/net/URLConnection\.connect)", "Сетевые запросы",
             "Открывает сетевые соединения по URL.", "Проверьте, куда отправляются данные."),
        rule("network-dns", "network", "info", "m",
             r"java/net/InetAddress\.(getByName|getAllByName|getLocalHost)", "Определение адресов (DNS)",
             "Резолвит имена хостов и локальный адрес.", "Может раскрывать ваш IP-адрес и используемые домены."),
        rule("network-proxy-selector", "network", "notice", "m",
             r"java/net/(ProxySelector|Proxy)\.", "Настройка прокси", "Может перенаправлять трафик через прокси.",
             "Перехват трафика возможен при подмене прокси."),
        rule("exec-runtime", "process", "critical", "m",
             r"java/lang/Runtime\.exec", "Запуск внешних программ",
             "Запускает произвольные команды операционной системы.",
             "Классический признак вредоносного кода в моде."),
        rule("exec-process-builder", "process", "critical", "m",
             r"java/lang/ProcessBuilder\.(<init>|start|command)", "Запуск процессов (ProcessBuilder)",
             "Создаёт и запускает внешние процессы.", "Классический признак вредоносного кода в моде."),
        rule("reflection-class-forname", "reflection", "notice", "m",
             r"java/lang/Class\.forName", "Загрузка классов по имени", "Загружает классы динамически по строковому имени.",
             "Часто легитимно; в сочетании с сетью — скрытая загрузка кода."),
        rule("reflection-url-classloader", "reflection", "warning", "m",
             r"java/net/URLClassLoader\.(<init>|newInstance)", "Загрузка кода по URL",
             "Может загружать и исполнять Java-код из произвольных адресов или файлов.",
             "В сочетании с сетью — скачивание и запуск кода."),
        rule("reflection-define-class", "reflection", "warning", "m",
             r"java/lang/ClassLoader\.defineClass", "Определение классов из байтов",
             "Конструирует классы напрямую из байтового кода.",
             "Используется для запуска кода, отсутствующего в файле."),
        rule("reflection-method-invoke", "reflection", "notice", "m",
             r"java/lang/reflect/Method\.invoke", "Вызов методов через рефлексию",
             "Вызывает методы по имени через рефлексию.",
             "Часто легитимно; скрывает реальные вызовы от анализа."),
        rule("reflection-set-accessible", "reflection", "info", "m",
             r"java/lang/reflect/(Field|Method|Constructor)\.setAccessible", "Снятие инкапсуляции",
             "Обращается к приватным полям Minecraft/сервера (стандартная практика модов).",
             "Не представляет риска само по себе."),
        rule("reflection-module-opens", "reflection", "notice", "m",
             r"java/lang/Module\.(addOpens|addReads|open)", "Открытие модулей JDK", "Снимает изоляцию модулей Java.",
             "Иногда нужно для совместимости; иногда — для обхода защит."),
        rule("reflection-proxy-new", "reflection", "notice", "m",
             r"java/lang/reflect/Proxy\.newProxyInstance", "Создание динамических прокси",
             "Подменяет реализации интерфейсов на лету.",
             "Часто легитимно; может подменять игровые объекты."),
        rule("native-load", "native", "warning", "m",
             r"java/lang/System\.(load|loadLibrary)", "Загрузка нативного кода",
             "Загружает внешние библиотеки .dll/.so в процесс.",
             "Нативный код невидим для Java-анализа: относитесь к нему как к исполняемому файлу."),
        rule("server-op-elevation", "server", "critical", "m",
             r"[A-Za-z0-9/$_]+\.setOp", "Выдача прав оператора (OP)",
             "Может выдавать себе или другим игрокам права оператора сервера.",
             "Явный признак бэкдора в плагине."),
        rule("server-permission-grab", "server", "warning", "m",
             r"(org/bukkit/permissions/Permissible\.addAttachment|org/bukkit/permissions/PermissionAttachment\.setPermission)",
             "Изменение прав игроков", "Может менять права доступа игроков на сервере.",
             "Проверьте, для кого и какие права выдаются."),
        rule("server-dispatch-command", "server", "notice", "m",
             r"org/bukkit/Bukkit\.dispatchCommand", "Выполнение команд от имени консоли",
             "Выполняет команды сервера от имени консоли.",
             "Легитимно для многих плагинов; важно, какие команды."),
        rule("server-plugin-manager", "server", "notice", "m",
             r"org/bukkit/plugin/PluginManager\.(disablePlugin|enablePlugin)", "Управление другими плагинами",
             "Может отключать или включать другие плагины сервера.",
             "Может маскировать конфликты или вредоносную активность."),
        rule("deserialization-read", "deserialization", "notice", "m",
             r"java/io/ObjectInputStream\.readObject", "Чтение сериализованных объектов",
             "Десериализует объекты — опасно, если источник данных сетевой.",
             "Уязвимость класса «deserialization gadget»."),
    ]
}

fn field_rules() -> Vec<AuditRule> {
    vec![
        rule("logging-system-fields", "logging", "info", "f",
             r"java/lang/System\.(out|err)", "Стандартный вывод", "Использует поля стандартного вывода.",
             "Обычный вывод в консоль."),
    ]
}

fn rules() -> &'static Vec<AuditRule> {
    static RULES: OnceLock<Vec<AuditRule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut all = class_rules();
        all.extend(string_rules());
        all.extend(method_rules());
        all.extend(field_rules());
        all
    })
}

fn rules_by_kind(kind: &str) -> Vec<&'static AuditRule> {
    rules().iter().filter(|rule| rule.kind == kind).collect()
}

fn known_host_rule() -> &'static AuditRule {
    static RULE: OnceLock<AuditRule> = OnceLock::new();
    RULE.get_or_init(|| AuditRule {
        id: "network-known-host",
        category: "network",
        severity: "info",
        kind: "k",
        pattern: regex::Regex::new(ALLOWLISTED_HOSTS).expect("allowlisted hosts"),
        title: "Обращение к известным сервисам",
        detail: "Найдены адреса известных сервисов (Modrinth, PaperMC, GitHub, bStats, Mojang и т.п.) — типично для ссылок и обновлений.",
        recommendation: "Не представляет риска.",
    })
}

fn audit_categories() -> &'static Vec<(&'static str, &'static str)> {
    static CATEGORIES: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    CATEGORIES.get_or_init(|| {
        vec![
            ("keylogger", "Клавиатурные шпионы, перехват ввода и буфера обмена"),
            ("surveillance", "Съёмка скриншотов экрана"),
            ("miner", "Майнеры криптовалют"),
            ("wallets", "Кража кошельков, данных браузеров и Discord"),
            ("destruction", "Уничтожение данных на диске"),
            ("system", "Инвентаризация системы и окружения"),
            ("windows", "Открытие окон, ссылок и встроенных браузеров"),
            ("close", "Закрытие окон, завершение игры, сервера и процессов"),
            ("logging", "Логирование и запись логов"),
            ("files", "Работа с файлами и сканирование каталогов"),
            ("network", "Сетевая активность, прямые IP и открытые порты"),
            ("process", "Запуск процессов и команд ОС"),
            ("reflection", "Рефлексия и динамическая загрузка кода"),
            ("native", "Встроенный нативный код (.dll/.so)"),
            ("obfuscation", "Обфускация, шифрование строк и анти-анализ"),
            ("crypto", "Шифрование и кодирование данных"),
            ("server", "Захват сервера: OP-права, команды консоли"),
            ("tokens", "Кража аккаунтов, токенов и сессий"),
            ("chat", "Перехват чата и команд игроков"),
            ("telemetry", "Телеметрия и аналитика"),
            ("persistence", "Закрепление в системе (автозапуск, задачи)"),
            ("jndi", "JNDI/LDAP-инъекции (Log4Shell)"),
            ("deserialization", "Опасная десериализация"),
            ("update", "Самообновление и загрузка кода"),
            ("combo", "Комбинированные схемы (ввод/файлы + сеть и др.)"),
            ("structure", "Структура архива, манифесты и подписи"),
        ]
    })
}

// ── сканер исходников ──────────────────────────────────────────────

struct FindingAccumulator {
    findings: BTreeMap<String, Value>,
}

impl FindingAccumulator {
    fn new() -> Self {
        Self { findings: BTreeMap::new() }
    }

    fn add(&mut self, rule: &AuditRule, source: &str, value: &str) {
        let finding = self
            .findings
            .entry(rule.id.to_owned())
            .or_insert_with(|| {
                json!({
                    "id": rule.id,
                    "category": rule.category,
                    "severity": rule.severity,
                    "title": rule.title,
                    "detail": rule.detail,
                    "recommendation": rule.recommendation,
                    "evidence": [],
                })
            });
        if let Some(evidence) = finding.get_mut("evidence").and_then(Value::as_array_mut) {
            if evidence.len() < MAX_EVIDENCE_PER_FINDING {
                evidence.push(json!({"class": source, "value": clip(value, 300)}));
            }
        }
    }

    fn result(self) -> Vec<Value> {
        let mut findings: Vec<Value> = self.findings.into_values().collect();
        findings.sort_by(|left, right| {
            let key = |item: &Value| {
                (
                    severity_index(item["severity"].as_str().unwrap_or("info")),
                    item["category"].as_str().unwrap_or_default().to_owned(),
                    item["id"].as_str().unwrap_or_default().to_owned(),
                )
            };
            key(left).cmp(&key(right))
        });
        findings
    }
}

fn obfuscated_name_pattern() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| regex::Regex::new(r"^[a-z]{1,3}\d?$").expect("obfuscated name"))
}

fn structural_findings(structure: &JarStructure, source_files: &[PathBuf]) -> Vec<Value> {
    let mut findings: Vec<Value> = Vec::new();
    let mut add = |rule_id: &str, category: &str, severity: &str, title: String, detail: String, recommendation: &str, evidence: Vec<Value>| {
        findings.push(json!({
            "id": rule_id,
            "category": category,
            "severity": severity,
            "title": title,
            "detail": detail,
            "recommendation": recommendation,
            "evidence": evidence.into_iter().take(MAX_EVIDENCE_PER_FINDING).collect::<Vec<_>>(),
        }));
    };

    if !structure.natives.is_empty() {
        let listed = structure.natives.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
        let extra = if structure.natives.len() > 8 { format!(" (+{})", structure.natives.len() - 8) } else { String::new() };
        add(
            "native-bundled", "native", "critical",
            "В JAR лежат нативные библиотеки".to_owned(),
            format!("Внутри архива найдены файлы нативного кода: {listed}{extra}. Их содержимое не декомпилируется и исполняется на уровне ОС."),
            "Нативный код исполняется напрямую вашей системой и не проверяется правилами Java. Убедитесь в доверии к источнику.",
            structure.natives.iter().map(|name| json!({"class": name, "value": "нативная библиотека"})).collect(),
        );
    }

    if structure.class_count >= 25 && !source_files.is_empty() {
        let class_names: Vec<String> = source_files
            .iter()
            .map(|path| path.file_stem().map(|value| value.to_string_lossy().into_owned()).unwrap_or_default())
            .collect();
        let suspicious = class_names.iter().filter(|name| obfuscated_name_pattern().is_match(name)).count();
        if suspicious as f64 / class_names.len() as f64 > 0.5 {
            let mut sorted_names = class_names.clone();
            sorted_names.sort();
            add(
                "obfuscation-names", "obfuscation", "warning",
                "Обфусцированные имена классов".to_owned(),
                format!("{suspicious} из {} классов имеют короткие нечитаемые имена (a, b, c1…). Это признак обфускации кода.", class_names.len()),
                "Обфускация не делает код вредоносным, но скрывает его назначение от любой проверки.",
                sorted_names.iter().map(|name| json!({"class": name, "value": "имя класса"})).collect(),
            );
        }
    }

    if structure.declared.is_empty() && structure.class_count > 0 {
        add(
            "no-manifest", "structure", "notice",
            "Нет манифеста мода или плагина".to_owned(),
            "В архиве отсутствуют plugin.yml, fabric.mod.json, mods.toml и другие стандартные манифесты.".to_owned(),
            "Легальные дистрибутивы модов и плагинов почти всегда содержат манифест.",
            Vec::new(),
        );
    }

    if structure.signed {
        add(
            "signed-jar", "structure", "info",
            "Архив подписан".to_owned(),
            "JAR содержит подписи META-INF (*.SF/*.RSA). Это подтверждает целостность файла, но не его безопасность: подпись может принадлежать кому угодно.".to_owned(),
            "Проверьте, кому принадлежит сертификат подписи.",
            Vec::new(),
        );
    }

    if structure.nested_jars.len() > 40 {
        add(
            "many-nested-jars", "structure", "notice",
            format!("Много вложенных jar: {}", structure.nested_jars.len()),
            "Внутри архива более 40 вложенных jar-файлов. Это нетипично даже для shaded-зависимостей.".to_owned(),
            "Проверьте список вложенных библиотек.",
            structure.nested_jars.iter().map(|name| json!({"class": name, "value": "вложенный jar"})).collect(),
        );
    }

    if !structure.skipped_entries.is_empty() {
        let listed = structure.skipped_entries.iter().take(6).cloned().collect::<Vec<_>>().join("; ");
        let extra = if structure.skipped_entries.len() > 6 { format!(" (+{})", structure.skipped_entries.len() - 6) } else { String::new() };
        add(
            "skipped-entries", "structure", "notice",
            format!("Пропущено записей при разборе: {}", structure.skipped_entries.len()),
            format!("Часть файлов не удалось декомпилировать: {listed}{extra}. Их содержимое не проверялось."),
            "Неполная проверка снижает гарантию отчёта.",
            structure
                .skipped_entries
                .iter()
                .map(|entry| json!({"class": entry.split(" (").next().unwrap_or_default(), "value": entry}))
                .collect(),
        );
    }

    let declared_permissions = structure.declared.get("declared_permissions").and_then(Value::as_array);
    if let Some(permissions) = declared_permissions {
        if permissions.iter().any(|item| item.as_str() == Some("*") || item.as_str() == Some("**")) {
            add(
                "server-wildcard-permissions", "server", "warning",
                "Плагин запрашивает права-подстановку".to_owned(),
                "Манифест объявляет permission «*» — полный доступ ко всем правам сервера.".to_owned(),
                "Проверьте, действительно ли плагину нужны все права.",
                Vec::new(),
            );
        }
    }
    findings
}

type ComboCondition = fn(&BTreeSet<String>, &BTreeSet<String>) -> bool;
type ComboRule = (&'static str, &'static str, &'static str, ComboCondition, &'static str, &'static str, &'static str);

fn combo_rules() -> &'static Vec<ComboRule> {
    static RULES: OnceLock<Vec<ComboRule>> = OnceLock::new();
    RULES.get_or_init(|| {
        vec![
            ("combo-keylogger-network", "combo", "critical",
             |ids, cats| cats.contains("keylogger") && ids.contains("exfil-webhook"),
             "Перехват ввода + канал утечки",
             "Код совмещает перехват клавиатуры/экрана с жёстко прописанными каналами отправки данных. Схема типичного шпиона.",
             "Настоятельно не рекомендуется запускать этот файл."),
            ("combo-input-network", "combo", "warning",
             |ids, cats| cats.contains("keylogger") && cats.contains("network") && !ids.contains("exfil-webhook"),
             "Перехват ввода + сеть",
             "Код читает ввод пользователя и одновременно имеет сетевые возможности.",
             "Комбинация позволяет отправлять перехваченные данные наружу."),
            ("combo-sensitive-network", "combo", "critical",
             |ids, cats| (ids.contains("files-sensitive-account") || ids.contains("files-sensitive-system")) && cats.contains("network"),
             "Чтение учётных данных + сеть",
             "Код ссылается на файлы с паролями/токенами и одновременно может отправлять данные в сеть.",
             "Схема кражи учётных данных."),
            ("combo-dynamic-network", "combo", "critical",
             |ids, cats| (ids.contains("reflection-url-classloader") || ids.contains("reflection-define-class")) && cats.contains("network"),
             "Динамическая загрузка кода + сеть",
             "Код может загружать классы из байтов или URL и имеет сетевые возможности — то есть скачивать и запускать новый код.",
             "Загруженный код не проходит эту проверку."),
            ("combo-exec-network", "combo", "critical",
             |ids, cats| (ids.contains("exec-runtime") || ids.contains("exec-process-builder") || ids.contains("exec-shell-string")) && cats.contains("network"),
             "Запуск команд ОС + сеть",
             "Код запускает процессы ОС и работает с сетью — схема дроппера.",
             "Классическое поведение загрузчиков вредоносного кода."),
            ("combo-command-network", "combo", "critical",
             |ids, _cats| ids.contains("command-observer")
                 && (ids.contains("network-socket") || ids.contains("exfil-webhook") || ids.contains("network-http")),
             "Перехват команд игроков + сеть",
             "Плагин видит команды игроков (включая пароли /login) и может отправлять их в сеть.",
             "Кража паролей с сервера."),
            ("combo-update-exec", "combo", "critical",
             |ids, _cats| ids.contains("self-update-download")
                 && ["reflection-url-classloader", "reflection-define-class", "exec-runtime", "exec-process-builder"].iter().any(|rule| ids.contains(*rule)),
             "Самообновление + запуск кода",
             "Код скачивает обновления и умеет исполнять загруженный код.",
             "Обновлённый код обходит эту проверку и может быть чем угодно."),
            ("combo-obfuscated-suspicious", "combo", "warning",
             |ids, cats| cats.contains("obfuscation")
                 && ["keylogger-native-hook", "keylogger-jna-hook", "keylogger-robot", "exec-runtime",
                     "exec-process-builder", "exec-shell-string", "exfil-webhook", "reflection-url-classloader",
                     "persistence-os", "files-sensitive-account"].iter().any(|rule| ids.contains(*rule)),
             "Обфускация + опасные сигналы",
             "Опасные вызовы скрыты за обфускацией кода.",
             "Сочетание резко повышает риск: автор намеренно скрывал поведение."),
            ("combo-tokens-network", "combo", "critical",
             |ids, cats| (ids.contains("mc-session-files") || ids.contains("tokens-launcher-config") || ids.contains("mc-auth-urls")) && cats.contains("network"),
             "Токены аккаунтов + сеть",
             "Код обращается к маркерам сессий и одновременно работает с сетью.",
             "Возможна отправка ваших игровых токенов злоумышленнику."),
            ("combo-screenshot-network", "combo", "critical",
             |ids, cats| ids.contains("screenshot-capture") && cats.contains("network"),
             "Съёмка экрана + сеть",
             "Код снимает экран и может отправлять данные в сеть.",
             "Слежка за экраном пользователя."),
            ("combo-chat-network", "combo", "warning",
             |ids, _cats| ids.contains("chat-observer") && (ids.contains("network-socket") || ids.contains("exfil-webhook")),
             "Чтение чата + канал утечки",
             "Плагин читает чат и содержит адреса отправки данных.",
             "Пересылка переписки игроков третьим лицам."),
            ("combo-miner-network", "combo", "critical",
             |_ids, cats| cats.contains("miner") && cats.contains("network"),
             "Майнер + сеть",
             "Код содержит признаки майнинга и умеет работать с сетью: пул, задачи и статистика ходят по соединению.",
             "Ставить такой мод — отдать процессор чужому майнингу."),
            ("combo-miner-exec", "combo", "critical",
             |ids, cats| cats.contains("miner")
                 && (ids.contains("exec-runtime") || ids.contains("exec-process-builder") || ids.contains("exec-shell-string")),
             "Майнер + запуск процессов",
             "Признаки майнинга совмещены с запуском процессов ОС: часто это запуск внешнего майнера-исполняемого файла.",
             "Вредоносный файл может остаться в системе после удаления мода."),
            ("combo-wallet-network", "combo", "critical",
             |_ids, cats| cats.contains("wallets") && cats.contains("network"),
             "Кошельки и данные браузеров + сеть",
             "Код читает файлы кошельков/браузеров/Discord и одновременно работает с сетью.",
             "Это законченная схема кражи: сначала читает, потом отправляет."),
            ("combo-env-exfil", "combo", "warning",
             |ids, _cats| ids.contains("env-harvest")
                 && (ids.contains("network-http") || ids.contains("network-socket") || ids.contains("exfil-webhook")),
             "Сбор окружения + сеть",
             "Код снимает переменные окружения и может отправлять их наружу.",
             "В переменных окружения часто лежат токены CI, API-ключи и пароли."),
        ]
    })
}


/// строка без вида-префикса и пробелов, обрезанная до лимита
fn line_rest(line: &str) -> String {
    let rest = if line.is_empty() { "" } else { &line[1..] };
    let trimmed = rest.trim();
    if trimmed.chars().count() > AUDIT_MAX_SOURCE_LINE {
        trimmed.chars().take(AUDIT_MAX_SOURCE_LINE).collect()
    } else {
        trimmed.to_owned()
    }
}

/// применение правил к одной строке IR (для сканирования и оценки скорости)
/// `k` ищет подстроку, `c/s/i/r` задают классы, `m/f` задают методы и поля
fn run_rules_on_line(line: &str) {
    let kind = line.chars().next().unwrap_or_default();
    let rest = line_rest(line);
    if kind == 'k' {
        let mut value = rest.clone();
        if value.starts_with('"') {
            if let Ok(Value::String(decoded)) = serde_json::from_str(&value) {
                value = decoded;
            }
        }
        if allowlisted_hosts().is_match(&value) {
            return;
        }
        for rule in rules_by_kind("k") {
            if rule.pattern.is_match(&value) {
                return;
            }
        }
        return;
    }
    let rules = line_rules(kind);
    for rule in rules {
        if rule.pattern.is_match(&rest) {
            return;
        }
    }
}

/// правила для строки вида: c/s/i/r → классы, m → методы, f → поля
fn line_rules(kind: char) -> &'static [&'static AuditRule] {
    static BY_KIND: OnceLock<HashMap<char, Vec<&'static AuditRule>>> = OnceLock::new();
    let map = BY_KIND.get_or_init(|| {
        let class = rules_by_kind("c");
        let mut map: HashMap<char, Vec<&'static AuditRule>> = HashMap::new();
        for key in ['c', 's', 'i', 'r'] {
            map.insert(key, class.clone());
        }
        map.insert('m', rules_by_kind("m"));
        map.insert('f', rules_by_kind("f"));
        map
    });
    map.get(&kind).map(Vec::as_slice).unwrap_or(&[])
}

fn iter_source_files(sources_dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![sources_dir.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = fs::read_dir(&directory) else { continue };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else { continue };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() && path.extension().map(|value| value == "jasm").unwrap_or(false) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// прогон правил по декомпилированным исходникам и сборка отчёта
fn scan_sources(
    sources_dir: &Path,
    structure: &mut JarStructure,
    progress: &dyn Fn(i64, &str),
    cancelled: &dyn Fn() -> bool,
) -> Result<Map<String, Value>, String> {
    let started = Instant::now();
    let mut accumulator = FindingAccumulator::new();
    let files = iter_source_files(sources_dir);
    if files.is_empty() {
        return Err("Декомпилированные исходники не найдены или уже уничтожены".to_owned());
    }
    let total_bytes: u64 = files.iter().filter_map(|path| fs::metadata(path).ok()).map(|meta| meta.len()).sum::<u64>().max(1);
    let mut done_bytes: u64 = 0;
    for (index, path) in files.iter().enumerate() {
        if cancelled() {
            return Err("Проверка отменена пользователем".to_owned());
        }
        let source_name = path.strip_prefix(sources_dir).map(|value| value.to_string_lossy().into_owned()).unwrap_or_default();
        let Ok(content) = fs::read_to_string(path) else {
            structure.skipped_entries.push(format!("{source_name} (не читается)"));
            continue;
        };
        for line in content.lines() {
            let kind = line.chars().next().unwrap_or_default();
            let rest = line_rest(line);
            if kind == 'k' {
                let mut value = rest.clone();
                if value.starts_with('"') {
                    if let Ok(Value::String(decoded)) = serde_json::from_str(&value) {
                        value = decoded;
                    }
                }
                if let Some(allowed) = allowlisted_hosts().find(&value) {
                    accumulator.add(known_host_rule(), &source_name, allowed.as_str());
                }
                for rule in rules_by_kind("k") {
                    if rule.pattern.is_match(&value) {
                        accumulator.add(rule, &source_name, &clip(&value, 200));
                    }
                }
                continue;
            }
            let rules = line_rules(kind);
            if rules.is_empty() {
                continue;
            }
            for rule in rules {
                if rule.pattern.is_match(&rest) {
                    accumulator.add(rule, &source_name, &rest);
                }
            }
        }
        let Ok(meta) = fs::metadata(path) else { continue };
        done_bytes += meta.len();
        if index % 25 == 0 || index == files.len() - 1 {
            progress(((done_bytes * 100 / total_bytes) as i64).min(99), &source_name);
        }
    }

    let mut findings = accumulator.result();
    findings.extend(structural_findings(structure, &files));

    let notable: Vec<&Value> = findings.iter().filter(|finding| finding["severity"].as_str() != Some("info")).collect();
    let finding_ids: BTreeSet<String> = notable.iter().filter_map(|finding| finding["id"].as_str()).map(str::to_owned).collect();
    let categories: BTreeSet<String> = findings.iter().filter_map(|finding| finding["category"].as_str()).map(str::to_owned).collect();
    let notable_categories: BTreeSet<String> = notable.iter().filter_map(|finding| finding["category"].as_str()).map(str::to_owned).collect();
    let mut categories = categories;
    for (rule_id, category, severity, condition, title, detail, recommendation) in combo_rules() {
        if condition(&finding_ids, &notable_categories) {
            findings.push(json!({
                "id": rule_id,
                "category": category,
                "severity": severity,
                "title": title,
                "detail": detail,
                "recommendation": recommendation,
                "evidence": [],
            }));
            categories.insert(category.to_string());
        }
    }

    let mut counts: Map<String, Value> = Map::new();
    for severity in SEVERITY_ORDER {
        counts.insert(severity.to_owned(), json!(0));
    }
    let mut risk_score: i64 = 0;
    for finding in &findings {
        let severity = finding["severity"].as_str().unwrap_or("info");
        if let Some(entry) = counts.get_mut(severity) {
            *entry = json!(entry.as_i64().unwrap_or(0) + 1);
        }
        risk_score += severity_score(severity);
    }
    risk_score = risk_score.min(100);
    let critical = counts["critical"].as_i64().unwrap_or(0);
    let warning = counts["warning"].as_i64().unwrap_or(0);
    let notice = counts["notice"].as_i64().unwrap_or(0);
    let verdict = if critical > 0 || risk_score >= 40 {
        "dangerous"
    } else if warning > 0 || risk_score >= 15 {
        "suspicious"
    } else if notice >= 3 {
        "review"
    } else {
        "clean"
    };
    findings.sort_by(|left, right| {
        let key = |item: &Value| {
            (
                severity_index(item["severity"].as_str().unwrap_or("info")),
                item["category"].as_str().unwrap_or_default().to_owned(),
                item["id"].as_str().unwrap_or_default().to_owned(),
            )
        };
        key(left).cmp(&key(right))
    });

    let mut report = Map::new();
    report.insert("findings".to_owned(), json!(findings));
    report.insert("counts".to_owned(), Value::Object(counts));
    report.insert("risk_score".to_owned(), json!(risk_score));
    report.insert("verdict".to_owned(), json!(verdict));
    report.insert("verdict_label".to_owned(), json!(verdict_label(verdict)));
    report.insert("categories".to_owned(), json!(categories.into_iter().collect::<Vec<_>>()));
    report.insert("scanned_files".to_owned(), json!(files.len()));
    report.insert("duration_seconds".to_owned(), json!((started.elapsed().as_secs_f64() * 1000.0).round() / 1000.0));
    Ok(report)
}

// ── оценка времени и бенчмарк ──────────────────────────────────────

const CPU_BENCHMARK_MIB: f64 = 4.0;

fn cpu_benchmark() -> f64 {
    static CACHE: OnceLock<Mutex<Option<(Instant, f64)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap();
    if let Some((at, value)) = *guard {
        if at.elapsed().as_secs_f64() < 600.0 {
            return value;
        }
    }
    let block = super::snapshots::random_bytes(512 * 1024);
    let started = Instant::now();
    for _ in 0..(CPU_BENCHMARK_MIB as usize * 2) {
        let _ = Sha256::digest(&block);
    }
    let elapsed = started.elapsed().as_secs_f64().max(1e-6);
    let value = CPU_BENCHMARK_MIB / elapsed;
    *guard = Some((Instant::now(), value));
    value
}

fn sample_lines(sources_dir: &Path) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let files = iter_source_files(sources_dir);
    if files.is_empty() {
        return lines;
    }
    let step = (files.len() / 24).max(1);
    for path in files.iter().step_by(step) {
        let Ok(content) = fs::read_to_string(path) else { continue };
        for line in content.lines() {
            lines.push(line.chars().take(AUDIT_MAX_SOURCE_LINE).collect());
            if lines.len() >= ESTIMATE_SAMPLE_LINES {
                return lines;
            }
        }
    }
    lines
}

fn measure_lines_per_second(sample: &[String]) -> f64 {
    if sample.is_empty() {
        return 20_000.0;
    }
    let started = Instant::now();
    for line in sample {
        run_rules_on_line(line);
    }
    let elapsed = started.elapsed().as_secs_f64().max(1e-6);
    sample.len() as f64 / elapsed
}

fn format_estimate(seconds: i64) -> String {
    if seconds < 60 {
        return format!("около {seconds} с");
    }
    let (minutes, sec) = (seconds / 60, seconds % 60);
    if minutes < 60 {
        let tail = if sec > 0 { format!(" {sec} с") } else { String::new() };
        return format!("около {minutes} мин{tail}");
    }
    let (hours, minutes) = (minutes / 60, minutes % 60);
    format!("около {hours} ч {minutes} мин")
}

// ── гарантированное уничтожение исходников ─────────────────────────

/// перезапись каждого файла случайными байтами перед удалением
fn secure_wipe(directory: &Path) {
    let mut entries: Vec<PathBuf> = Vec::new();
    let mut stack = vec![directory.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(read) = fs::read_dir(&current) else { continue };
        for entry in read.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if let Ok(meta) = fs::symlink_metadata(&path) {
                if meta.is_dir() {
                    stack.push(path.clone());
                }
            }
            entries.push(path);
        }
    }
    entries.sort_by(|left, right| right.cmp(left));
    for path in entries {
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if meta.is_symlink() {
            let _ = fs::remove_file(&path);
            continue;
        }
        if meta.is_file() {
            if let Ok(mut handle) = fs::OpenOptions::new().write(true).open(&path) {
                let mut remaining = meta.len() as usize;
                while remaining > 0 {
                    let chunk = super::snapshots::random_bytes(remaining.min(1024 * 1024));
                    if handle.write_all(&chunk).is_err() {
                        break;
                    }
                    remaining -= chunk.len();
                }
                let _ = handle.flush();
                let _ = handle.sync_all();
            }
            if fs::remove_file(&path).is_err() {
                super::platform::set_mode(&path, 0o600);
                let _ = fs::remove_file(&path);
            }
        } else if meta.is_dir() {
            let _ = fs::remove_dir(&path);
        }
    }
    if fs::remove_dir(directory).is_err() {
        let _ = fs::remove_dir_all(directory);
    }
}

// ── хранилище сессий ───────────────────────────────────────────────

fn session_id_pattern() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| regex::Regex::new(r"^[a-f0-9]{32}$").expect("session id"))
}

fn epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

struct ModAuditStore {
    root: PathBuf,
}

impl ModAuditStore {
    fn new(root: &Path) -> Result<Self, String> {
        if root.is_symlink() {
            return Err("Каталог аудита является символьной ссылкой".to_owned());
        }
        fs::create_dir_all(root).map_err(|error| format!("Каталог аудита недоступен: {error}"))?;
        super::platform::set_mode(root, 0o700);
        Ok(Self { root: root.to_path_buf() })
    }

    fn session_dir(&self, session_id: &str) -> Result<PathBuf, String> {
        if !session_id_pattern().is_match(session_id) {
            return Err("Некорректный идентификатор сессии аудита".to_owned());
        }
        Ok(self.root.join(session_id))
    }

    fn read_record(&self, session_id: &str) -> Result<Map<String, Value>, String> {
        let directory = self.session_dir(session_id)?;
        let record_path = directory.join("session.json");
        let Ok(meta) = fs::symlink_metadata(&record_path) else {
            return Err("Сессия аудита не найдена".to_owned());
        };
        if meta.is_symlink() || !meta.is_file() || meta.len() > AUDIT_MAX_MANIFEST_BYTES {
            return Err("Файл сессии аудита повреждён".to_owned());
        }
        let Ok(raw) = fs::read_to_string(&record_path) else {
            return Err("Файл сессии аудита повреждён".to_owned());
        };
        let Ok(Value::Object(record)) = serde_json::from_str::<Value>(&raw) else {
            return Err("Файл сессии аудита повреждён".to_owned());
        };
        if !record.get("state").map(Value::is_string).unwrap_or(false) {
            return Err("Файл сессии аудита повреждён".to_owned());
        }
        Ok(record)
    }

    fn write_record(&self, directory: &Path, record: &Value) -> Result<(), String> {
        let payload = format!("{}\n", serde_json::to_string_pretty(record).unwrap_or_default());
        let temporary = directory.join(format!(".session.{}.tmp", super::snapshots::random_hex(3)));
        let write = (|| -> Result<(), String> {
            let mut handle = fs::File::create(&temporary).map_err(|error| format!("Не удалось записать сессию аудита: {error}"))?;
            handle
                .write_all(payload.as_bytes())
                .and_then(|_| handle.flush())
                .and_then(|_| handle.sync_all())
                .map_err(|error| format!("Не удалось записать сессию аудита: {error}"))?;
            fs::rename(&temporary, directory.join("session.json"))
                .map_err(|error| format!("Не удалось записать сессию аудита: {error}"))
        })();
        let _ = fs::remove_file(&temporary);
        write
    }

    /// удаление просроченных сессий; возвращает число удалённых
    fn cleanup(&self) -> usize {
        let mut removed = 0;
        let now = epoch_seconds();
        let Ok(entries) = fs::read_dir(&self.root) else { return removed };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !session_id_pattern().is_match(&name) {
                continue;
            }
            let record = self.read_record(&name).ok();
            let created = record
                .as_ref()
                .and_then(|value| value.get("created_at"))
                .and_then(Value::as_f64)
                .or_else(|| entry.metadata().ok().and_then(|meta| meta.modified().ok()).map(|time| time.duration_since(std::time::UNIX_EPOCH).map(|value| value.as_secs_f64()).unwrap_or(0.0)))
                .unwrap_or(0.0);
            let state = record.as_ref().and_then(|value| value.get("state")).and_then(Value::as_str).unwrap_or("unknown").to_owned();
            let sources_present = entry.path().join("sources").exists();
            let ttl = if sources_present || state == "awaiting" { SESSION_TTL_SECONDS } else { SESSION_RESULT_TTL_SECONDS };
            if now - created > ttl {
                secure_wipe(&entry.path());
                removed += 1;
            }
        }
        removed
    }

    /// не более SESSION_MAX_ACTIVE ожидающих сессий: старые уничтожаются
    fn limit_active(&self) {
        let mut sessions: Vec<(f64, String)> = Vec::new();
        let Ok(entries) = fs::read_dir(&self.root) else { return };
        for entry in entries.filter_map(|entry| entry.ok()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !session_id_pattern().is_match(&name) {
                continue;
            }
            let Ok(record) = self.read_record(&name) else { continue };
            if record.get("state").and_then(Value::as_str) == Some("awaiting") {
                let created = record.get("created_at").and_then(Value::as_f64).unwrap_or(0.0);
                sessions.push((created, name));
            }
        }
        sessions.sort_by(|left, right| left.0.total_cmp(&right.0));
        while sessions.len() > SESSION_MAX_ACTIVE {
            let (_, oldest) = sessions.remove(0);
            let directory = self.root.join(oldest);
            if directory.exists() {
                secure_wipe(&directory);
            }
        }
    }

    /// декомпиляция JAR в приватную сессию и оценка времени проверки
    fn prepare(&self, jar_path: &Path, label: &str, kind: &str) -> Result<Map<String, Value>, String> {
        self.cleanup();
        if jar_path.is_symlink() || !jar_path.is_file() {
            return Err("Файл не найден".to_owned());
        }
        let size = fs::metadata(jar_path).map_err(|_| "Не удалось прочитать файл".to_owned())?.len();
        if size == 0 || size > AUDIT_MAX_JAR_BYTES {
            return Err(format!("Размер файла должен быть от 1 байта до {} MiB", AUDIT_MAX_JAR_BYTES / (1024 * 1024)));
        }

        let session_id = super::snapshots::random_hex(16);
        let directory = self.root.join(&session_id);
        let sources_dir = directory.join("sources");
        let create = (|| -> Result<(), String> {
            fs::create_dir(&directory).map_err(|error| format!("Не удалось создать сессию аудита: {error}"))?;
            super::platform::set_mode(&directory, 0o700);
            fs::create_dir(&sources_dir).map_err(|error| format!("Не удалось создать сессию аудита: {error}"))?;
            super::platform::set_mode(&sources_dir, 0o700);
            Ok(())
        })();
        if let Err(error) = create {
            let _ = fs::remove_dir_all(&directory);
            return Err(error);
        }

        let outcome = (|| -> Result<Map<String, Value>, String> {
            let mut sha256 = Sha256::new();
            let mut handle = fs::File::open(jar_path).map_err(|_| "Не удалось прочитать файл".to_owned())?;
            let mut chunk = vec![0u8; 1024 * 1024];
            loop {
                let read = handle.read(&mut chunk).map_err(|_| "Не удалось прочитать файл".to_owned())?;
                if read == 0 {
                    break;
                }
                sha256.update(&chunk[..read]);
            }
            let digest = format!("{:x}", sha256.finalize());

            let file = fs::File::open(jar_path).map_err(|_| "Не удалось прочитать файл".to_owned())?;
            let mut archive = ZipArchive::new(file).map_err(|_| "Файл не является корректным ZIP/JAR архивом".to_owned())?;
            let mut structure = JarStructure::new();
            let mut total_unpacked: u64 = 0;
            inspect_and_write_sources(&mut archive, &sources_dir, &mut structure, "", 0, false, &mut total_unpacked)
                .map_err(|error| format!("Не удалось разобрать архив: {error}"))?;

            if structure.class_count == 0 {
                return Err("В архиве нет Java class-файлов: проверять нечего".to_owned());
            }

            let sample = sample_lines(&sources_dir);
            let lines_per_second = measure_lines_per_second(&sample);
            let mut estimated = (structure.source_lines as f64 / lines_per_second.max(1.0)) as i64 + 1;
            estimated = estimated.clamp(ESTIMATE_MIN_SECONDS, ESTIMATE_MAX_SECONDS);
            let kind = if kind.is_empty() {
                if ["bukkit", "paper", "bungee"].contains(&structure.platform.as_str()) { "plugin" } else { "mod" }
            } else {
                kind
            };
            let source = json!({
                "path": jar_path.to_string_lossy(),
                "label": clip(label, 160),
                "kind": clip(kind, 32),
                "sha256": digest,
                "size": size,
            });
            let structure_payload = structure_payload(&structure);
            let estimate = json!({
                "seconds": estimated,
                "label": format_estimate(estimated),
                "source_lines": structure.source_lines,
                "measured_lines_per_second": (lines_per_second * 10.0).round() / 10.0,
                "cpu_mib_per_second": (cpu_benchmark() * 10.0).round() / 10.0,
            });
            let mut record = Map::new();
            record.insert("session_id".to_owned(), json!(session_id));
            record.insert("created_at".to_owned(), json!(epoch_seconds()));
            record.insert("state".to_owned(), json!("awaiting"));
            record.insert("source".to_owned(), source.clone());
            record.insert("structure".to_owned(), structure_payload.clone());
            record.insert("estimate".to_owned(), estimate.clone());
            self.write_record(&directory, &Value::Object(record))?;

            let mut payload = Map::new();
            payload.insert("session_id".to_owned(), json!(session_id));
            payload.insert("state".to_owned(), json!("awaiting"));
            payload.insert("source".to_owned(), source);
            payload.insert("structure".to_owned(), structure_payload);
            payload.insert("estimate".to_owned(), estimate);
            payload.insert("checks".to_owned(), json!(audit_categories().iter().map(|(id, title)| json!({"id": id, "title": title})).collect::<Vec<_>>()));
            payload.insert("checks_count".to_owned(), json!(audit_categories().len()));
            Ok(payload)
        })();
        if outcome.is_err() {
            secure_wipe(&directory);
            return outcome;
        }
        self.limit_active();
        outcome
    }

    /// прогон правил, затем гарантированное уничтожение исходников
    fn accept(
        &self,
        session_id: &str,
        progress: &dyn Fn(i64, &str),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Map<String, Value>, String> {
        let directory = self.session_dir(session_id)?;
        let mut record = self.read_record(session_id)?;
        if record.get("state").and_then(Value::as_str) != Some("awaiting") {
            return Err("Проверка этой сессии уже завершена или отменена".to_owned());
        }
        let sources_dir = directory.join("sources");
        let scan = || -> Result<Map<String, Value>, String> {
            let mut structure = structure_from_record(&record);
            scan_sources(&sources_dir, &mut structure, progress, cancelled)
        };
        let report = match scan() {
            Ok(report) => report,
            Err(error) => {
                // исходники уничтожаются при любом исходе
                if sources_dir.exists() {
                    secure_wipe(&sources_dir);
                }
                if error.contains("отменена") {
                    record.insert("finished_at".to_owned(), json!(epoch_seconds()));
                    record.insert("state".to_owned(), json!("cancelled"));
                    let _ = self.write_record(&directory, &Value::Object(record));
                }
                return Err(error);
            }
        };
        if sources_dir.exists() {
            secure_wipe(&sources_dir);
        }
        let source = record.get("source").cloned().unwrap_or(json!({}));
        let structure = record.get("structure").cloned().unwrap_or(json!({}));
        let mut payload = report;
        payload.insert("session_id".to_owned(), json!(session_id));
        payload.insert("file".to_owned(), source.get("path").cloned().unwrap_or(json!("")));
        payload.insert("label".to_owned(), source.get("label").cloned().unwrap_or(json!("")));
        payload.insert("kind".to_owned(), source.get("kind").cloned().unwrap_or(json!("")));
        payload.insert("sha256".to_owned(), source.get("sha256").cloned().unwrap_or(json!("")));
        payload.insert("size_bytes".to_owned(), source.get("size").cloned().unwrap_or(json!(0)));
        payload.insert("platform".to_owned(), structure.get("platform").cloned().unwrap_or(json!("")));
        payload.insert("declared".to_owned(), structure.get("declared").cloned().unwrap_or(json!({})));
        payload.insert("sources_wiped".to_owned(), json!(true));
        payload.insert("scanned_at".to_owned(), json!(epoch_seconds()));
        record.insert("finished_at".to_owned(), json!(epoch_seconds()));
        record.insert("state".to_owned(), json!("completed"));
        record.insert("report".to_owned(), Value::Object(payload.clone()));
        self.write_record(&directory, &Value::Object(record))?;
        Ok(payload)
    }

    /// уничтожение исходников без проверки
    fn cancel(&self, session_id: &str) -> Result<Map<String, Value>, String> {
        let directory = self.session_dir(session_id)?;
        let mut record = self.read_record(session_id)?;
        if record.get("state").and_then(Value::as_str) != Some("awaiting") {
            return Err("Сессия уже завершена или отменена".to_owned());
        }
        let sources_dir = directory.join("sources");
        if sources_dir.exists() {
            secure_wipe(&sources_dir);
        }
        record.insert("state".to_owned(), json!("cancelled"));
        record.insert("finished_at".to_owned(), json!(epoch_seconds()));
        self.write_record(&directory, &Value::Object(record))?;
        Ok(json!({"session_id": session_id, "cancelled": true, "sources_wiped": true})
            .as_object()
            .cloned()
            .unwrap_or_default())
    }

    fn status(&self, session_id: &str) -> Result<Map<String, Value>, String> {
        let record = self.read_record(session_id)?;
        let directory = self.session_dir(session_id)?;
        let mut payload = Map::new();
        payload.insert("session_id".to_owned(), json!(session_id));
        payload.insert("state".to_owned(), record.get("state").cloned().unwrap_or(Value::Null));
        payload.insert("source".to_owned(), record.get("source").cloned().unwrap_or(json!({})));
        payload.insert("structure".to_owned(), record.get("structure").cloned().unwrap_or(json!({})));
        payload.insert("estimate".to_owned(), record.get("estimate").cloned().unwrap_or(json!({})));
        payload.insert("sources_wiped".to_owned(), json!(!directory.join("sources").exists()));
        if record.get("state").and_then(Value::as_str) == Some("completed") {
            payload.insert("report".to_owned(), record.get("report").cloned().unwrap_or(Value::Null));
        }
        Ok(payload)
    }
}

fn structure_payload(structure: &JarStructure) -> Value {
    json!({
        "platform": structure.platform,
        "declared": Value::Object(structure.declared.clone()),
        "classes": structure.class_count,
        "strings": structure.string_count,
        "references": structure.reference_count,
        "natives": structure.natives.iter().take(64).cloned().collect::<Vec<_>>(),
        "nested_jars": structure.nested_jars.iter().take(96).cloned().collect::<Vec<_>>(),
        "skipped_entries": structure.skipped_entries.iter().take(64).cloned().collect::<Vec<_>>(),
        "signed": structure.signed,
        "sources_bytes": structure.sources_bytes,
        "source_lines": structure.source_lines,
    })
}

fn structure_from_record(record: &Map<String, Value>) -> JarStructure {
    let raw = record.get("structure").cloned().unwrap_or(json!({}));
    let declared = raw.get("declared").and_then(Value::as_object).cloned().unwrap_or_default();
    JarStructure {
        platform: raw.get("platform").and_then(Value::as_str).unwrap_or("unknown").to_owned(),
        declared,
        class_count: raw.get("classes").and_then(Value::as_u64).unwrap_or(0) as usize,
        string_count: raw.get("strings").and_then(Value::as_u64).unwrap_or(0) as usize,
        reference_count: raw.get("references").and_then(Value::as_u64).unwrap_or(0) as usize,
        natives: raw.get("natives").and_then(Value::as_array).map(|items| items.iter().take(64).filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default(),
        nested_jars: raw.get("nested_jars").and_then(Value::as_array).map(|items| items.iter().take(96).filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default(),
        skipped_entries: raw.get("skipped_entries").and_then(Value::as_array).map(|items| items.iter().take(64).filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default(),
        signed: raw.get("signed").and_then(Value::as_bool).unwrap_or(false),
        sources_bytes: raw.get("sources_bytes").and_then(Value::as_u64).unwrap_or(0) as usize,
        source_lines: raw.get("source_lines").and_then(Value::as_u64).unwrap_or(0) as usize,
    }
}

// ── операции движка ────────────────────────────────────────────────

fn audit_store() -> Result<ModAuditStore, String> {
    let config = super::config::ConfigStore::load()?;
    ModAuditStore::new(&config.paths.data_home.join("mod-audit"))
}

/// выбирает JAR для проверки и возвращает путь, подпись и тип
fn audit_target(request: &Value) -> Result<(PathBuf, String, String), String> {
    let raw_path = request.get("path").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let instance_name = request.get("instance_name").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    if raw_path.is_empty() {
        return Err("Укажите файл мода или плагина для проверки".to_owned());
    }
    if !instance_name.is_empty() {
        let config = super::config::ConfigStore::load()?;
        let store = super::instances::InstanceStore::new(&config)?;
        let info = store.get(&instance_name)?;
        let is_server = info.kind.trim().to_lowercase() == "server";
        let mut parts: Vec<String> = raw_path
            .replace('\\', "/")
            .split('/')
            .filter(|part| !part.is_empty() && *part != "." && *part != "..")
            .map(str::to_owned)
            .collect();
        if raw_path.starts_with('/') || raw_path.starts_with('\\') || regex::Regex::new(r"^[A-Za-z]:").unwrap().is_match(&raw_path) {
            return Err("Путь должен быть относительным путём внутри экземпляра".to_owned());
        }
        if parts.is_empty() {
            return Err("Путь должен быть относительным путём внутри экземпляра".to_owned());
        }
        if is_server && parts.first().map(|part| part == "plugins").unwrap_or(false) {
            parts.remove(0);
        }
        let base = info.path.join(if is_server { "plugins" } else { "mods" });
        let base = base.canonicalize().map_err(|_| "Файл мода или плагина не найден".to_owned())?;
        let mut target = base.clone();
        for part in &parts {
            target.push(part);
        }
        if target.is_symlink() || !target.is_file() {
            return Err("Файл мода или плагина не найден".to_owned());
        }
        let resolved = target.canonicalize().map_err(|_| "Файл мода или плагина не найден".to_owned())?;
        if !resolved.starts_with(&base) {
            return Err("Путь выходит за пределы экземпляра".to_owned());
        }
        return Ok((resolved, parts.last().cloned().unwrap_or_default(), if is_server { "plugin".to_owned() } else { "mod".to_owned() }));
    }
    let target = super::opener::expand_home(&raw_path);
    if target.is_symlink() || !target.is_file() {
        return Err("Файл не найден".to_owned());
    }
    let label = target.file_name().map(|value| value.to_string_lossy().into_owned()).unwrap_or_default();
    Ok((target, label, String::new()))
}

pub(crate) fn mod_audit_prepare(request: &Value) -> Result<Value, String> {
    let store = audit_store()?;
    let (target, label, kind) = audit_target(request)?;
    let payload = store.prepare(&target, &label, &kind)?;
    let session_id = payload.get("session_id").and_then(Value::as_str).unwrap_or_default().to_owned();
    super::events::emit_status_event_extra(
        "mod-audit-prepared",
        &format!("Файл декомпилирован для проверки: {label}"),
        json!({"session_id": session_id}),
    );
    Ok(Value::Object(payload))
}

pub(crate) fn mod_audit_cancel(request: &Value) -> Result<Value, String> {
    let store = audit_store()?;
    let session_id = request.get("session_id").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let result = store.cancel(&session_id)?;
    super::events::emit_status_event_extra(
        "mod-audit-cancelled",
        "Декомпилированные исходники уничтожены без проверки.",
        json!({"session_id": session_id}),
    );
    Ok(Value::Object(result))
}

pub(crate) fn mod_audit_status(request: &Value) -> Result<Value, String> {
    let store = audit_store()?;
    let session_id = request.get("session_id").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    Ok(Value::Object(store.status(&session_id)?))
}

pub(crate) fn mod_audit_accept(request: &Value) -> Result<Value, String> {
    let store = audit_store()?;
    let session_id = request.get("session_id").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
    let progress = |percent: i64, _source: &str| {
        super::events::emit_status_event_extra(
            "mod-audit-progress",
            &format!("Проверка исходников: {percent}%"),
            json!({"percent": percent, "session_id": session_id}),
        );
    };
    // отмена: маркер операции, как в сетевых запросах движка
    let operation_id = request.get("operation_id").and_then(Value::as_str).unwrap_or_default();
    let token = super::operations::CancellationToken::new(operation_id);
    let cancelled = move || token.check().is_err();
    let report = store.accept(&session_id, &progress, &cancelled)?;
    let verdict = report.get("verdict_label").and_then(Value::as_str).unwrap_or_default().to_owned();
    super::events::emit_status_event_extra(
        "mod-audit-finished",
        &format!("Проверка завершена: {verdict}"),
        json!({"session_id": session_id}),
    );
    Ok(json!({"report": report}))
}

// ── тесты ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn scratch(tag: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "nivora-mod-audit-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("каталог теста");
        directory
    }

    /// минимальный class-файл с заданными строками и Methodref
    fn class_bytes(name: &str, strings: &[&str], method: Option<(&str, &str)>) -> Vec<u8> {
        let mut pool: Vec<(u8, Vec<u8>)> = Vec::new(); // (tag, payload после tag)
        let push_utf8 = |pool: &mut Vec<(u8, Vec<u8>)>, value: &str| -> u16 {
            let bytes = value.as_bytes();
            pool.push((CP_UTF8, [(bytes.len() as u16).to_be_bytes().to_vec(), bytes.to_vec()].concat()));
            pool.len() as u16
        };
        let name_index = push_utf8(&mut pool, name);
        pool.push((CP_CLASS, name_index.to_be_bytes().to_vec()));
        let this_class = pool.len() as u16;
        let object_index = push_utf8(&mut pool, "java/lang/Object");
        pool.push((CP_CLASS, object_index.to_be_bytes().to_vec()));
        let super_class = pool.len() as u16;
        let mut string_indexes: Vec<u16> = Vec::new();
        for value in strings {
            let index = push_utf8(&mut pool, value);
            pool.push((CP_STRING, index.to_be_bytes().to_vec()));
            string_indexes.push(pool.len() as u16);
        }
        let mut method_ref = 0;
        if let Some((owner, method_name)) = method {
            let owner_index = push_utf8(&mut pool, owner);
            pool.push((CP_CLASS, owner_index.to_be_bytes().to_vec()));
            let owner_class = pool.len() as u16;
            let name_idx = push_utf8(&mut pool, method_name);
            let descriptor_index = push_utf8(&mut pool, "()V");
            pool.push((CP_NAME_AND_TYPE, [name_idx.to_be_bytes(), descriptor_index.to_be_bytes()].concat()));
            let nat = pool.len() as u16;
            pool.push((CP_METHODREF, [owner_class.to_be_bytes(), nat.to_be_bytes()].concat()));
            method_ref = pool.len() as u16;
        }
        let _ = string_indexes;

        let mut data = Vec::new();
        data.extend_from_slice(&CLASS_MAGIC);
        data.extend_from_slice(&0u16.to_be_bytes()); // minor
        data.extend_from_slice(&52u16.to_be_bytes()); // major
        data.extend_from_slice(&((pool.len() + 1) as u16).to_be_bytes());
        for (tag, payload) in &pool {
            data.push(*tag);
            data.extend_from_slice(payload);
        }
        data.extend_from_slice(&0x0021u16.to_be_bytes()); // access_flags
        data.extend_from_slice(&this_class.to_be_bytes());
        data.extend_from_slice(&super_class.to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes()); // interfaces_count
        let _ = method_ref;
        data
    }

    #[test]
    fn parse_and_render_class_file() {
        let data = class_bytes(
            "com/example/Evil",
            &["https://discord.com/api/webhooks/123/abc", "hello"],
            Some(("java/lang/Runtime", "exec")),
        );
        let info = parse_class_file(&data, "com/example/Evil.class").expect("разбор");
        assert_eq!(info.name, "com/example/Evil");
        assert_eq!(info.super_name, "java/lang/Object");
        assert!(info.method_refs.contains(&"java/lang/Runtime.exec".to_owned()));
        assert!(info.strings.contains(&"hello".to_owned()));
        let rendered = render_class_source(&info);
        assert!(rendered.starts_with("v 52.0\nc com/example/Evil\ns java/lang/Object\n"));
        assert!(rendered.contains("m java/lang/Runtime.exec\n"));
        assert!(rendered.contains("k \"hello\"\n"));
        assert!(parse_class_file(b"nope", "x.class").is_err());
        assert!(parse_class_file(&data[..20], "x.class").is_err());
    }

    #[test]
    fn yaml_and_manifest_metadata() {
        let values = parse_simple_yaml("name: Demo\nversion: '1.0'\nmain:\n  com.example.Demo\nnested:\n  key: value\n");
        assert_eq!(values.get("name").map(String::as_str), Some("Demo"));
        assert_eq!(values.get("version").map(String::as_str), Some("1.0"));
        // вложенные ключи наружу не попадают
        assert!(!values.contains_key("key"));

        let yml = manifest_metadata(
            "plugin.yml",
            b"name: Demo\nversion: 1.0\nmain: com.example.Demo\npermissions:\n  - minecraft.command.op\n  - '*'\n",
        );
        assert_eq!(yml.get("manifest").and_then(Value::as_str), Some("plugin.yml"));
        assert_eq!(yml.get("name").and_then(Value::as_str), Some("Demo"));
        assert_eq!(yml.get("main").and_then(Value::as_str), Some("com.example.Demo"));

        let permissions = declared_permissions_from_manifest(
            "plugin.yml",
            b"permissions:\n  - minecraft.command.op\n  - *\nother: x\n",
        );
        // незаключённая в кавычки * остаётся допустимым разрешением
        assert_eq!(permissions, vec!["*".to_owned(), "minecraft.command.op".to_owned()]);

        let fabric = manifest_metadata(
            "fabric.mod.json",
            br#"{"schemaVersion":1,"id":"demo","version":"1.2.3","name":"Demo Mod","entrypoints":{"main":"net.demo.Entry"},"depends":{"fabricloader":">=0.15"}}"#,
        );
        assert_eq!(fabric.get("id").and_then(Value::as_str), Some("demo"));
        assert_eq!(fabric.get("version").and_then(Value::as_str), Some("1.2.3"));
        assert_eq!(fabric.get("entrypoints").and_then(Value::as_array).map(Vec::len), Some(1));
        assert_eq!(fabric.get("depends").and_then(Value::as_array).map(Vec::len), Some(1));
    }

    #[test]
    fn entry_name_validation() {
        assert!(validate_entry_name("com/example/Evil.jasm").is_ok());
        assert!(validate_entry_name("../escape.class").is_err());
        assert!(validate_entry_name("/absolute.class").is_err());
        assert!(validate_entry_name("back\\slash.class").is_err());
        assert!(validate_entry_name("").is_err());
        assert!(source_path_for_entry("com/example/Evil.class").is_ok());
    }

    #[test]
    fn estimate_format() {
        assert_eq!(format_estimate(45), "около 45 с");
        assert_eq!(format_estimate(60), "около 1 мин");
        assert_eq!(format_estimate(95), "около 1 мин 35 с");
        assert_eq!(format_estimate(3600), "около 1 ч 0 мин");
    }

    #[test]
    fn scan_sources_reports_dangerous_combo() {
        let directory = scratch("scan");
        let sources = directory.join("sources");
        fs::create_dir_all(sources.join("com/example")).unwrap();
        fs::write(
            sources.join("com/example/Evil.jasm"),
            format!(
                "{rendered}k \"https://api.modrinth.com/v2/project/x\"\n",
                rendered = render_class_source(
                    &parse_class_file(
                        &class_bytes(
                            "com/example/Evil",
                            &["https://discord.com/api/webhooks/123/abc"],
                            Some(("java/lang/Runtime", "exec")),
                        ),
                        "x",
                    )
                    .unwrap()
                ),
            ),
        )
        .unwrap();
        // разрешённый хост даёт находку network-known-host уровня info
        let mut structure = JarStructure::new();
        structure.class_count = 1;
        let report = scan_sources(&sources, &mut structure, &|_, _| {}, &|| false).expect("скан");
        let findings = report["findings"].as_array().unwrap();
        let ids: Vec<&str> = findings.iter().filter_map(|item| item["id"].as_str()).collect();
        assert!(ids.contains(&"exec-runtime"));
        assert!(ids.contains(&"exfil-webhook"));
        assert!(ids.contains(&"combo-exec-network"));
        assert!(ids.contains(&"network-known-host"));
        assert_eq!(report["verdict"].as_str(), Some("dangerous"));
        assert!(report["risk_score"].as_i64().unwrap_or(0) >= 40);
        // сортировка: критичные находки впереди
        assert_eq!(findings[0]["severity"].as_str(), Some("critical"));
        let counts = report["counts"].as_object().unwrap();
        assert!(counts["critical"].as_i64().unwrap() >= 3);
        assert_eq!(report["scanned_files"].as_i64(), Some(1));
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn secure_wipe_destroys_files() {
        let directory = scratch("wipe");
        fs::create_dir_all(directory.join("inner")).unwrap();
        fs::write(directory.join("inner/secret.jasm"), "k \"token\"").unwrap();
        fs::write(directory.join("plain.txt"), "data").unwrap();
        secure_wipe(&directory);
        assert!(!directory.exists());
    }

    fn write_jar(path: &Path) {
        let file = fs::File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        archive.start_file("plugin.yml", options).unwrap();
        archive.write_all(b"name: Demo\nversion: 1.0\nmain: com.example.Demo\n").unwrap();
        archive.start_file("com/example/Demo.class", options).unwrap();
        archive.write_all(&class_bytes("com/example/Demo", &["hello world"], None)).unwrap();
        archive.start_file("com/example/Evil.class", options).unwrap();
        archive.write_all(&class_bytes(
            "com/example/Evil",
            &["https://discord.com/api/webhooks/123"],
            Some(("java/lang/Runtime", "exec")),
        ))
        .unwrap();
        archive.finish().unwrap();
    }

    #[test]
    fn store_prepare_accept_status_roundtrip() {
        let root = scratch("store");
        let store = ModAuditStore::new(&root).expect("хранилище");
        let jar = root.join("demo.jar");
        write_jar(&jar);

        let payload = store.prepare(&jar, "demo.jar", "plugin").expect("prepare");
        let session_id = payload["session_id"].as_str().unwrap().to_owned();
        assert_eq!(payload["state"].as_str(), Some("awaiting"));
        assert_eq!(payload["source"]["kind"].as_str(), Some("plugin"));
        let structure = payload["structure"].as_object().unwrap();
        assert_eq!(structure["platform"].as_str(), Some("bukkit"));
        assert_eq!(structure["classes"].as_i64(), Some(2));
        assert_eq!(structure["declared"]["name"].as_str(), Some("Demo"));
        assert!(structure["source_lines"].as_i64().unwrap() > 0);
        assert!(payload["estimate"]["seconds"].as_i64().unwrap() >= 1);
        assert_eq!(payload["checks_count"].as_i64(), Some(audit_categories().len() as i64));

        // исходники на месте до accept
        assert!(root.join(&session_id).join("sources").exists());

        let report = store.accept(&session_id, &|_, _| {}, &|| false).expect("accept");
        assert_eq!(report["verdict"].as_str(), Some("dangerous"));
        assert_eq!(report["kind"].as_str(), Some("plugin"));
        assert_eq!(report["platform"].as_str(), Some("bukkit"));
        assert_eq!(report["sources_wiped"].as_bool(), Some(true));
        assert!(!root.join(&session_id).join("sources").exists());

        let status = store.status(&session_id).expect("status");
        assert_eq!(status["state"].as_str(), Some("completed"));
        assert!(status["report"].is_object());
        assert_eq!(status["sources_wiped"].as_bool(), Some(true));

        // повторный accept отклоняется
        assert!(store.accept(&session_id, &|_, _| {}, &|| false).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn store_cancel_and_cleanup() {
        let root = scratch("cancel");
        let store = ModAuditStore::new(&root).expect("хранилище");
        let jar = root.join("demo.jar");
        write_jar(&jar);
        let payload = store.prepare(&jar, "demo.jar", "").expect("prepare");
        let session_id = payload["session_id"].as_str().unwrap().to_owned();

        let cancelled = store.cancel(&session_id).expect("cancel");
        assert_eq!(cancelled["cancelled"].as_bool(), Some(true));
        assert!(!root.join(&session_id).join("sources").exists());
        assert!(store.cancel(&session_id).is_err());

        // храним не более пяти активных сессий
        let mut ids = Vec::new();
        for _ in 0..(SESSION_MAX_ACTIVE + 1) {
            let payload = store.prepare(&jar, "demo.jar", "").expect("prepare");
            ids.push(payload["session_id"].as_str().unwrap().to_owned());
        }
        assert!(!root.join(&ids[0]).exists(), "самая старая сессия уничтожена");
        assert!(root.join(&ids[1]).exists());
        assert!(root.join(&ids[SESSION_MAX_ACTIVE]).exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn line_rules_dispatch() {
        assert!(line_rules('c').iter().any(|rule| rule.id == "hardware-probe"));
        assert!(line_rules('s').len() == line_rules('r').len());
        assert!(line_rules('m').iter().any(|rule| rule.id == "exec-runtime"));
        assert!(line_rules('f').iter().any(|rule| rule.id == "logging-system-fields"));
        assert!(line_rules('k').is_empty());
        assert!(line_rules('v').is_empty());
    }

    #[test]
    fn fullmatch_semantics_for_ref_rules() {
        // m-строка должна совпадать целиком: суффикс мимо правила не считается
        let executed = rules().iter().find(|rule| rule.id == "exec-runtime").unwrap();
        assert!(executed.pattern.is_match("java/lang/Runtime.exec"));
        assert!(!executed.pattern.is_match("java/lang/Runtime.executedLater"));
    }
}
