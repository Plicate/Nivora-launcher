// средства разработки плагинов
// фиксированные порядок файлов и метаданные обеспечивают повторяемую упаковку
use base64::Engine as _;
use ed25519_dalek::SigningKey;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use super::plugin_security::{parse_strict_json, sign_manifest, verify_manifest};
use super::plugins::{
    validate_plugin_json_manifest, PLUGIN_MAX_ARCHIVE_BYTES, PLUGIN_MAX_FILES,
    PLUGIN_MAX_MANIFEST_BYTES, PLUGIN_MAX_UNPACKED_BYTES,
};

const PLUGIN_MANIFEST: &str = "plugin.json";
const EXCLUDED_DIRECTORIES: [&str; 4] = [".git", "node_modules", ".venv", "__pycache__"];
const NETWORK_APIS: [&str; 5] = ["fetch", "XMLHttpRequest", "WebSocket", "EventSource", "WebTransport"];
const FILESYSTEM_APIS: [&str; 12] = [
    "readFile", "writeFile", "readFileSync", "writeFileSync", "createReadStream",
    "createWriteStream", "readdir", "unlink", "mkdir", "openSync", "Deno", "Bun",
];
const SIGNATURE_KEYS: [&str; 4] = ["algorithm", "key_id", "public_key", "value"];
// префикс PKCS#8 для закрытого ключа Ed25519
const PKCS8_PREFIX: &[u8] = &[
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
];
// префикс SubjectPublicKeyInfo для открытого ключа Ed25519
const SPKI_PREFIX: &[u8] = &[
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

// ── операции движка ─────────────────────────────────────────────────

pub(crate) fn plugin_dev_init(request: &Value) -> Result<Value, String> {
    let directory = request
        .get("directory")
        .and_then(Value::as_str)
        .ok_or_else(|| "Plugin init requires a directory.".to_owned())?;
    let root = init_plugin(Path::new(directory))?;
    Ok(json!({ "directory": root.to_string_lossy() }))
}

pub(crate) fn plugin_dev_lint(request: &Value) -> Result<Value, String> {
    let directory = request
        .get("directory")
        .and_then(Value::as_str)
        .ok_or_else(|| "Plugin lint requires a directory.".to_owned())?;
    let warnings = lint_plugin(Path::new(directory))?;
    Ok(json!({ "warnings": warnings }))
}

pub(crate) fn plugin_dev_pack(request: &Value) -> Result<Value, String> {
    let directory = request
        .get("directory")
        .and_then(Value::as_str)
        .ok_or_else(|| "Plugin pack requires a source directory.".to_owned())?;
    let output = request
        .get("output")
        .and_then(Value::as_str)
        .ok_or_else(|| "Plugin pack requires an output path.".to_owned())?;
    let key = request.get("key").and_then(Value::as_str);
    let archive = pack_plugin(Path::new(directory), Path::new(output), key.map(Path::new))?;
    Ok(json!({ "archive": archive.to_string_lossy() }))
}

pub(crate) fn plugin_dev_verify(request: &Value) -> Result<Value, String> {
    let archive = request
        .get("archive")
        .and_then(Value::as_str)
        .ok_or_else(|| "Plugin verify requires an archive path.".to_owned())?;
    verify_plugin(Path::new(archive))
}

pub(crate) fn plugin_dev_keypair(request: &Value) -> Result<Value, String> {
    let private_pem = request
        .get("private_pem")
        .and_then(Value::as_str)
        .ok_or_else(|| "Plugin keygen requires a private key path.".to_owned())?;
    let public_pem = request
        .get("public_pem")
        .and_then(Value::as_str)
        .ok_or_else(|| "Plugin keygen requires a public key path.".to_owned())?;
    let (private_path, public_path) = generate_keypair(Path::new(private_pem), Path::new(public_pem))?;
    Ok(json!({
        "private_pem": private_path.to_string_lossy(),
        "public_pem": public_path.to_string_lossy(),
    }))
}

// ── публичные операции инструмента ──────────────────────────────────

pub(crate) fn init_plugin(directory: &Path) -> Result<PathBuf, String> {
    let requested = expand_home(directory);
    if symlink_metadata(&requested).map(|meta| meta.is_symlink()).unwrap_or(false) {
        return Err("Plugin directory may not be a symbolic link.".to_owned());
    }
    let created_directory = !requested.exists();
    if let Err(error) = fs::create_dir_all(&requested) {
        return Err(format!("Could not create plugin directory: {error}"));
    }
    if !requested.is_dir() {
        return Err("Plugin path must be a directory.".to_owned());
    }
    let root = canonicalize_strict(&requested, "Could not resolve plugin directory")?;

    let directory_name = requested
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let plugin_id = plugin_id_from_directory(directory_name);
    let manifest = json!({
        "id": plugin_id,
        "name": "Sample Plugin",
        "version": "0.1.0",
        "api_version": "3",
        "author": "Plugin author",
        "description": "A Nivora Launcher plugin.",
        "entrypoint": "main.js",
        "permissions": [],
        "sandbox_profile": "strict",
    });
    let validated = validate_author_manifest(&manifest)?;
    let name = validated
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Sample Plugin");
    let mut starter_files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    starter_files.insert(
        PLUGIN_MANIFEST.to_owned(),
        json_bytes(&manifest)?,
    );
    starter_files.insert(
        "main.js".to_owned(),
        concat!(
            "\"use strict\";\n\n",
            "const plugin = {\n",
            "  async activate(api) {\n",
            "    const runtime = await api.runtime.getInfo();\n",
            "    void runtime;\n",
            "  },\n",
            "};\n\n",
            "export default plugin;\n",
        )
        .as_bytes()
        .to_vec(),
    );
    starter_files.insert(
        "README.md".to_owned(),
        format!(
            "# {name}\n\n\
             A Nivora Launcher v3 plugin starter.\n\n\
             Edit `plugin.json` for metadata and permissions, then implement the plugin in `main.js`.\n\
             Use the host-provided API only; do not rely on Node.js or browser globals.\n\n\
             Run `trial-launcher plugin lint .` before packaging.\n"
        )
        .into_bytes(),
    );

    for file_name in starter_files.keys() {
        let path = root.join(file_name);
        let is_symlink = symlink_metadata(&path).map(|meta| meta.is_symlink()).unwrap_or(false);
        if is_symlink || path.exists() {
            return Err("Plugin starter files already exist; init will not overwrite files.".to_owned());
        }
    }

    let mut created_files: Vec<PathBuf> = Vec::new();
    for (file_name, content) in &starter_files {
        let path = root.join(file_name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o644);
        }
        match options.open(&path) {
            Ok(mut handle) => {
                created_files.push(path);
                if let Err(error) = handle.write_all(content) {
                    for created in &created_files {
                        let _ = fs::remove_file(created);
                    }
                    if created_directory {
                        let _ = fs::remove_dir(&root);
                    }
                    return Err(format!("Could not write plugin starter files: {error}"));
                }
            }
            Err(error) => {
                for created in &created_files {
                    let _ = fs::remove_file(created);
                }
                if created_directory {
                    let _ = fs::remove_dir(&root);
                }
                return Err(format!("Could not write plugin starter files: {error}"));
            }
        }
    }
    Ok(root)
}

pub(crate) fn lint_plugin(directory: &Path) -> Result<Vec<String>, String> {
    let (_root, _manifest, _validated, files) = load_project(directory)?;
    lint_sources(&files)
}

pub(crate) fn pack_plugin(directory: &Path, output: &Path, key: Option<&Path>) -> Result<PathBuf, String> {
    let (root, manifest, _validated, source_files) = load_project(directory)?;
    lint_sources(&source_files)?;

    let mut output_path = expand_home(output);
    if symlink_metadata(&output_path).map(|meta| meta.is_symlink()).unwrap_or(false) {
        return Err("Plugin archive output may not be a symbolic link.".to_owned());
    }
    let file_name = output_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_owned();
    if file_name.is_empty() || file_name == "." || file_name == ".." {
        return Err("Plugin archive output must be a file path.".to_owned());
    }
    let parent = output_path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    if !parent.is_dir() {
        return Err("Plugin archive output directory does not exist.".to_owned());
    }
    let parent = canonicalize_strict(&parent, "Could not resolve plugin archive output directory")?;
    output_path = parent.join(&file_name);
    if output_path.exists() && !output_path.is_file() {
        return Err("Plugin archive output must be a regular file.".to_owned());
    }
    if is_within(&resolve_non_strict(&output_path), &root) {
        return Err("Plugin archive output must be outside the plugin directory.".to_owned());
    }

    let file_bytes: BTreeMap<String, Vec<u8>> = source_files
        .iter()
        .filter(|(name, _)| name.as_str() != PLUGIN_MANIFEST)
        .map(|(name, content)| (name.clone(), content.clone()))
        .collect();
    let mut package_manifest = manifest
        .as_object()
        .cloned()
        .ok_or_else(|| "plugin.json must contain a JSON object.".to_owned())?;
    package_manifest.remove("signature");
    package_manifest.insert("integrity".to_owned(), integrity_value(&file_bytes));

    if let Some(key) = key {
        let private_key_pem = read_private_key(key)?;
        let key_resolved = canonicalize_strict(
            &expand_home(key),
            "Plugin signing key must be a regular PEM file",
        )?;
        if key_resolved == output_path {
            return Err("Plugin archive output must not overwrite the signing key.".to_owned());
        }
        let candidate = Value::Object(package_manifest.clone());
        let signed = match sign_manifest(&candidate, &file_bytes, &private_key_pem) {
            Ok(signed) => signed,
            Err(_error) => {
                return Err(
                    "Could not sign the plugin manifest; check the private key and crypto support."
                        .to_owned(),
                )
            }
        };
        package_manifest = signed
            .as_object()
            .cloned()
            .ok_or_else(|| "The plugin signing provider returned an invalid manifest.".to_owned())?;
        validate_integrity(&Value::Object(package_manifest.clone()), &file_bytes)?;
        validate_signature_shape(package_manifest.get("signature"))?;
        verify_signature(&Value::Object(package_manifest.clone()), &file_bytes)?;
    }

    let mut packaged_files = file_bytes;
    packaged_files.insert(PLUGIN_MANIFEST.to_owned(), json_bytes(&Value::Object(package_manifest))?);
    if packaged_files.len() > PLUGIN_MAX_FILES {
        return Err("Plugin package contains too many files.".to_owned());
    }

    let temporary = parent.join(format!(
        ".{file_name}.{}.tmp",
        super::mods::random_hex(8)
    ));
    let result = write_archive(&temporary, &packaged_files);
    match result {
        Ok(()) => {
            let size = fs::metadata(&temporary)
                .map(|meta| meta.len())
                .unwrap_or(u64::MAX);
            if size > PLUGIN_MAX_ARCHIVE_BYTES as u64 {
                let _ = fs::remove_file(&temporary);
                return Err("Plugin archive exceeds the compressed size limit.".to_owned());
            }
            if let Err(error) = fs::rename(&temporary, &output_path) {
                let _ = fs::remove_file(&temporary);
                return Err(format!("Could not create plugin archive: {error}"));
            }
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
    }
    Ok(output_path)
}

fn write_archive(temporary: &Path, packaged_files: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let file = File::options()
        .write(true)
        .create_new(true)
        .open(temporary)
        .map_err(|error| format!("Could not create plugin archive: {error}"))?;
    let mut writer = zip::ZipWriter::new(file);
    let timestamp = zip::DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)
        .map_err(|error| format!("Could not create plugin archive: {error}"))?;
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(9))
        .last_modified_time(timestamp)
        .unix_permissions(0o100644);
    for (name, content) in packaged_files {
        writer
            .start_file(name.as_str(), options)
            .map_err(|error| format!("Could not create plugin archive: {error}"))?;
        writer
            .write_all(content)
            .map_err(|error| format!("Could not create plugin archive: {error}"))?;
    }
    writer
        .finish()
        .map_err(|error| format!("Could not create plugin archive: {error}"))?;
    // fsync каталога не требуется: rename ниже атомарен в рамках того же каталога
    Ok(())
}

pub(crate) fn verify_plugin(archive_path: &Path) -> Result<Value, String> {
    let path = expand_home(archive_path);
    let metadata = fs::symlink_metadata(&path).map_err(|error| format!("Could not read plugin archive: {error}"))?;
    if metadata.is_symlink() || !metadata.is_file() {
        return Err("Plugin archive must be a regular file, not a symbolic link.".to_owned());
    }
    let mut content = Vec::new();
    File::open(&path)
        .map_err(|error| format!("Could not read plugin archive: {error}"))?
        .take(PLUGIN_MAX_ARCHIVE_BYTES as u64 + 1)
        .read_to_end(&mut content)
        .map_err(|error| format!("Could not read plugin archive: {error}"))?;
    if content.is_empty() || content.len() > PLUGIN_MAX_ARCHIVE_BYTES {
        return Err("Plugin archive is empty or exceeds the compressed size limit.".to_owned());
    }

    let (manifest, validated, file_bytes) = read_archive(&content)?;
    validate_integrity(&manifest, &file_bytes)?;
    let signed = manifest.get("signature").is_some();
    if signed {
        validate_signature_shape(manifest.get("signature"))?;
        verify_signature(&manifest, &file_bytes)?;
    }
    let field = |key: &str| -> Value {
        validated
            .get(key)
            .or_else(|| manifest.get(key))
            .cloned()
            .unwrap_or(Value::String(String::new()))
    };
    Ok(json!({
        "id": field("id"),
        "name": field("name"),
        "version": field("version"),
        "signed": signed,
    }))
}

pub(crate) fn generate_keypair(private_path: &Path, public_path: &Path) -> Result<(PathBuf, PathBuf), String> {
    let (private_file, public_file) = key_output_paths(private_path, public_path)?;

    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|error| format!("Could not write Ed25519 key files: {error}"))?;
    let signing_key = SigningKey::from_bytes(&seed);
    let public_key = signing_key.verifying_key().to_bytes();

    let mut private_der = Vec::with_capacity(PKCS8_PREFIX.len() + seed.len());
    private_der.extend_from_slice(PKCS8_PREFIX);
    private_der.extend_from_slice(&seed);
    let mut public_der = Vec::with_capacity(SPKI_PREFIX.len() + public_key.len());
    public_der.extend_from_slice(SPKI_PREFIX);
    public_der.extend_from_slice(&public_key);

    write_new_file(&private_file, pem_wrap("PRIVATE KEY", &private_der).as_bytes(), 0o600)?;
    if let Err(error) = write_new_file(&public_file, pem_wrap("PUBLIC KEY", &public_der).as_bytes(), 0o644) {
        let _ = fs::remove_file(&private_file);
        return Err(error);
    }
    Ok((private_file, public_file))
}

// ── загрузка проекта ────────────────────────────────────────────────

type ProjectFiles = BTreeMap<String, Vec<u8>>;

fn load_project(directory: &Path) -> Result<(PathBuf, Value, Value, ProjectFiles), String> {    let root = project_root(directory)?;
    let files = collect_project_files(&root)?;
    let manifest_bytes = files
        .get(PLUGIN_MANIFEST)
        .ok_or_else(|| "Plugin directory must contain a root plugin.json manifest.".to_owned())?;
    if manifest_bytes.len() > PLUGIN_MAX_MANIFEST_BYTES {
        return Err("plugin.json exceeds the manifest size limit.".to_owned());
    }
    let manifest = parse_manifest_bytes(manifest_bytes)?;
    let validated = validate_author_manifest(&manifest)?;
    let entrypoint = validated
        .get("entrypoint")
        .or_else(|| manifest.get("entrypoint"))
        .and_then(Value::as_str)
        .ok_or_else(|| "plugin.json must name a JavaScript entrypoint.".to_owned())?;
    let entrypoint = normalize_archive_path(entrypoint, false)?;
    if !files.contains_key(&entrypoint) {
        return Err("The plugin entrypoint is missing or is not a regular file.".to_owned());
    }
    Ok((root, manifest, validated, files))
}

fn project_root(directory: &Path) -> Result<PathBuf, String> {
    let path = expand_home(directory);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| "Plugin directory does not exist or is not a directory.".to_owned())?;
    if metadata.is_symlink() {
        return Err("Plugin directory may not be a symbolic link.".to_owned());
    }
    if !metadata.is_dir() {
        return Err("Plugin directory does not exist or is not a directory.".to_owned());
    }
    canonicalize_strict(&path, "Could not resolve plugin directory")
}

fn collect_project_files(root: &Path) -> Result<ProjectFiles, String> {
    let mut result: ProjectFiles = BTreeMap::new();
    let mut total_size: u64 = 0;
    walk_project(root, root, &mut result, &mut total_size)?;
    Ok(result)
}

fn walk_project(
    root: &Path,
    directory: &Path,
    result: &mut ProjectFiles,
    total_size: &mut u64,
) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(directory)
        .map_err(|error| format!("Could not inspect plugin directory: {error}"))?
        .filter_map(|entry| entry.ok())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    // сначала обходим каталоги, затем добавляем файлы для стабильной упаковки
    let mut files: Vec<_> = Vec::new();
    for entry in &entries {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if EXCLUDED_DIRECTORIES.contains(&name) {
            continue;
        }
        let child = directory.join(name);
        let Ok(metadata) = fs::symlink_metadata(&child) else {
            continue;
        };
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            let relative = child.strip_prefix(root).unwrap_or(&child);
            return Err(format!(
                "Plugin package contains a symbolic link: {}",
                relative.to_string_lossy()
            ));
        }
        if file_type.is_dir() {
            let relative = child.strip_prefix(root).unwrap_or(&child);
            let normalized = relative.to_string_lossy().replace('\\', "/");
            normalize_archive_path(&normalized, false)?;
            walk_project(root, &child, result, total_size)?;
            continue;
        }
        files.push((child, metadata));
    }
    for (path, metadata) in files {
        if !metadata.file_type().is_file() {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            return Err(format!(
                "Plugin package contains a non-regular file: {}",
                relative.to_string_lossy()
            ));
        }
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let normalized = normalize_archive_path(&relative.to_string_lossy().replace('\\', "/"), false)?;
        let remaining = PLUGIN_MAX_UNPACKED_BYTES.saturating_sub(*total_size as usize);
        let per_file_limit = if normalized == PLUGIN_MANIFEST {
            PLUGIN_MAX_MANIFEST_BYTES
        } else {
            PLUGIN_MAX_UNPACKED_BYTES
        };
        let file_limit = remaining.min(per_file_limit);
        let size_error = if normalized == PLUGIN_MANIFEST {
            "plugin.json exceeds the manifest size limit."
        } else {
            "Plugin package exceeds the unpacked size limit."
        };
        if metadata.len() as usize > file_limit {
            return Err(size_error.to_owned());
        }
        let mut content = Vec::new();
        File::open(&path)
            .map_err(|error| format!("Could not read plugin package files: {error}"))?
            .take(file_limit as u64 + 1)
            .read_to_end(&mut content)
            .map_err(|error| format!("Could not read plugin package files: {error}"))?;
        if content.len() > file_limit || content.len() as u64 != metadata.len() {
            return Err(size_error.to_owned());
        }
        *total_size += content.len() as u64;
        result.insert(normalized, content);
        if result.len() > PLUGIN_MAX_FILES {
            return Err("Plugin package contains too many files.".to_owned());
        }
    }
    Ok(())
}

// ── манифест и целостность ──────────────────────────────────────────

fn validate_author_manifest(manifest: &Value) -> Result<Value, String> {
    let Some(object) = manifest.as_object() else {
        return Err("plugin.json must contain a JSON object.".to_owned());
    };
    let mut candidate = object.clone();
    candidate
        .entry("integrity")
        .or_insert_with(|| json!({ "files": {} }));
    validate_plugin_json_manifest(&Value::Object(candidate))
        .map_err(|error| format!("plugin.json manifest validation failed: {error}"))
}

fn parse_manifest_bytes(content: &[u8]) -> Result<Value, String> {
    if content.len() > PLUGIN_MAX_MANIFEST_BYTES {
        return Err("plugin.json exceeds the manifest size limit.".to_owned());
    }
    let manifest = parse_strict_json(content, "plugin.json")
        .map_err(|error| format!("plugin.json is not valid UTF-8 JSON: {error}"))?;
    if !manifest.is_object() {
        return Err("plugin.json must contain a JSON object.".to_owned());
    }
    Ok(manifest)
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn integrity_value(file_bytes: &BTreeMap<String, Vec<u8>>) -> Value {
    let mut files = Map::new();
    for (name, content) in file_bytes {
        files.insert(name.clone(), json!(sha256_hex(content)));
    }
    json!({ "files": files })
}

fn validate_integrity(manifest: &Value, file_bytes: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let expected = integrity_value(file_bytes)
        .get("files")
        .cloned()
        .unwrap_or(Value::Null);
    let Some(integrity) = manifest.get("integrity") else {
        return Err("Plugin manifest integrity metadata is missing or malformed.".to_owned());
    };
    let integrity_keys: HashSet<&str> = integrity
        .as_object()
        .map(|fields| fields.keys().map(String::as_str).collect())
        .unwrap_or_default();
    if integrity_keys != ["files"].into_iter().collect::<HashSet<_>>() {
        return Err("Plugin manifest integrity metadata is missing or malformed.".to_owned());
    }
    let Some(actual) = integrity.get("files") else {
        return Err("Plugin integrity hashes must cover every package file except plugin.json.".to_owned());
    };
    let expected_keys: HashSet<&str> = expected
        .as_object()
        .map(|fields| fields.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let actual_keys: HashSet<&str> = actual
        .as_object()
        .map(|fields| fields.keys().map(String::as_str).collect())
        .unwrap_or_default();
    if !actual.is_object() || actual_keys != expected_keys {
        return Err("Plugin integrity hashes must cover every package file except plugin.json.".to_owned());
    }
    for (name, digest) in expected.as_object().unwrap() {
        let Some(supplied) = actual.get(name).and_then(Value::as_str) else {
            return Err(format!("Plugin integrity hash is invalid for {name}."));
        };
        let mut valid = supplied.len() == 64;
        if valid {
            valid = supplied.bytes().all(|byte| byte.is_ascii_hexdigit());
        }
        if !valid {
            return Err(format!("Plugin integrity hash is invalid for {name}."));
        }
        // хеш не секретный; сравнение по байтам нужно только для единообразной проверки
        if supplied.bytes().ne(digest.as_str().unwrap_or("").bytes()) {
            return Err(format!("Plugin integrity check failed for {name}."));
        }
    }
    Ok(())
}

fn validate_signature_shape(value: Option<&Value>) -> Result<(), String> {
    let Some(object) = value.and_then(Value::as_object) else {
        return Err("Plugin signature metadata is missing or malformed.".to_owned());
    };
    let keys: HashSet<&str> = object.keys().map(String::as_str).collect();
    if keys != SIGNATURE_KEYS.iter().copied().collect::<HashSet<_>>() {
        return Err("Plugin signature metadata is missing or malformed.".to_owned());
    }
    if object.get("algorithm").and_then(Value::as_str) != Some("ed25519") {
        return Err("Plugin signature algorithm must be ed25519.".to_owned());
    }
    let Some(key_id) = object.get("key_id").and_then(Value::as_str) else {
        return Err("Plugin signature key_id is invalid.".to_owned());
    };
    if key_id.is_empty() || key_id.len() > 128 || key_id.chars().any(|char| char.is_control()) {
        return Err("Plugin signature key_id is invalid.".to_owned());
    }
    for (field, expected_length) in [("public_key", 32usize), ("value", 64usize)] {
        let Some(encoded) = object.get(field).and_then(Value::as_str) else {
            return Err(format!("Plugin signature {field} must be base64 text."));
        };
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| format!("Plugin signature {field} is not valid base64."))?;
        if decoded.len() != expected_length {
            return Err(format!("Plugin signature {field} has an invalid length."));
        }
    }
    Ok(())
}

fn verify_signature(manifest: &Value, file_bytes: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let verified = verify_manifest(manifest, file_bytes);
    if verified.get("signed") != Some(&json!(true)) {
        let error = verified.get("error").and_then(Value::as_str).unwrap_or("");
        return Err(if error.is_empty() {
            "Plugin signature verifier returned an invalid result.".to_owned()
        } else {
            error.to_owned()
        });
    }
    if verified.get("valid") != Some(&json!(true)) {
        let error = verified.get("error").and_then(Value::as_str).unwrap_or("");
        return Err(if error.is_empty() {
            "Plugin signature verification failed.".to_owned()
        } else {
            format!("Plugin signature verification failed: {error}")
        });
    }
    Ok(())
}

// ── чтение архива ───────────────────────────────────────────────────

type ArchiveContents = (Value, Value, BTreeMap<String, Vec<u8>>);

fn read_archive(content: &[u8]) -> Result<ArchiveContents, String> {
    let invalid = || "Plugin archive is invalid or contains unreadable files.".to_owned();
    let mut archive = zip::ZipArchive::new(Cursor::new(content)).map_err(|_| invalid())?;
    let members_len = archive.len();
    if members_len == 0 || members_len > PLUGIN_MAX_FILES {
        return Err("Plugin archive has no files or contains too many entries.".to_owned());
    }
    struct Member {
        name: String,
        is_dir: bool,
        size: u64,
        mode: u32,
        encrypted: bool,
    }
    let mut members: Vec<Member> = Vec::with_capacity(members_len);
    for index in 0..members_len {
        let file = archive.by_index(index).map_err(|_| invalid())?;
        members.push(Member {
            name: file.name().map(|v| v.to_string()).unwrap_or_default(),
            is_dir: file.is_dir(),
            size: file.size(),
            mode: file.unix_mode().unwrap_or(0),
            encrypted: file.encrypted(),
        });
    }
    let mut seen: HashSet<String> = HashSet::new();
    let mut file_bytes: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut total_size: u64 = 0;
    for (index, member) in members.iter().enumerate() {
        let name = normalize_archive_path(&member.name, member.is_dir)?;
        if !seen.insert(name.clone()) {
            return Err("Plugin archive contains duplicate paths.".to_owned());
        }
        let mode = member.mode & 0o170000;
        if mode == 0o120000 {
            return Err("Plugin archive contains a symbolic link.".to_owned());
        }
        if member.is_dir {
            if mode != 0 && mode != 0o040000 {
                return Err("Plugin archive contains a non-directory entry.".to_owned());
            }
            continue;
        }
        if mode != 0 && mode != 0o100000 {
            return Err("Plugin archive contains a non-regular file.".to_owned());
        }
        if member.encrypted {
            return Err("Encrypted plugin archive entries are not allowed.".to_owned());
        }
        let remaining = PLUGIN_MAX_UNPACKED_BYTES.saturating_sub(total_size as usize);
        let per_file_limit = if name == PLUGIN_MANIFEST {
            PLUGIN_MAX_MANIFEST_BYTES
        } else {
            PLUGIN_MAX_UNPACKED_BYTES
        };
        let limit = remaining.min(per_file_limit);
        if member.size > limit as u64 {
            return Err("Plugin archive exceeds the unpacked size limit.".to_owned());
        }
        let mut source = archive.by_index(index).map_err(|_| invalid())?;
        let mut data = Vec::with_capacity(member.size.min(limit as u64 + 1) as usize);
        source
            .by_ref()
            .take(limit as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|_| invalid())?;
        if data.len() > limit || data.len() as u64 != member.size {
            return Err("Plugin archive expands beyond the declared size limit.".to_owned());
        }
        total_size += data.len() as u64;
        file_bytes.insert(name, data);
    }

    for name in file_bytes.keys() {
        let parts: Vec<&str> = name.split('/').collect();
        for index in 1..parts.len() {
            if file_bytes.contains_key(&parts[..index].join("/")) {
                return Err("Plugin archive contains a file path used as a directory.".to_owned());
            }
        }
    }
    let manifest_bytes = file_bytes
        .remove(PLUGIN_MANIFEST)
        .ok_or_else(|| "Plugin archive must contain a root plugin.json manifest.".to_owned())?;
    let manifest = parse_manifest_bytes(&manifest_bytes)?;
    let validated = validate_author_manifest(&manifest)?;
    let entrypoint = validated
        .get("entrypoint")
        .or_else(|| manifest.get("entrypoint"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if entrypoint.is_empty() || !file_bytes.contains_key(&normalize_archive_path(entrypoint, false)?) {
        return Err("Plugin archive does not contain the manifest entrypoint.".to_owned());
    }
    Ok((manifest, validated, file_bytes))
}

// ── линтер исходников ───────────────────────────────────────────────

fn lint_sources(files: &ProjectFiles) -> Result<Vec<String>, String> {
    let mut warnings: Vec<String> = Vec::new();
    for (name, content) in files {
        let extension = Path::new(name)
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_lowercase)
            .unwrap_or_default();
        if !matches!(extension.as_str(), "js" | "mjs" | "cjs") {
            continue;
        }
        let source = std::str::from_utf8(content)
            .map_err(|_| format!("JavaScript source is not UTF-8: {name}"))?;
        let tokens = javascript_tokens(source)?;
        if let Some(line) = unsupported_import_line(&tokens) {
            return Err(format!(
                "Unsupported static ESM/CommonJS import in {name}:{line}; \
                 bundle dependencies into a single file and remove imports."
            ));
        }
        let identifiers: HashSet<&str> = tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Identifier)
            .map(|token| token.value.as_str())
            .collect();
        let mut network: Vec<&str> = identifiers.iter().copied().filter(|id| NETWORK_APIS.contains(id)).collect();
        network.sort_unstable();
        let mut filesystem: Vec<&str> = identifiers
            .iter()
            .copied()
            .filter(|id| FILESYSTEM_APIS.contains(id))
            .collect();
        filesystem.sort_unstable();
        if !network.is_empty() {
            warnings.push(format!(
                "{name}: direct network API(s) detected ({}); prefer permission-scoped network APIs. \
                 This check is advisory; QuickJs is the security boundary.",
                network.join(", ")
            ));
        }
        if !filesystem.is_empty() {
            warnings.push(format!(
                "{name}: direct filesystem API(s) detected ({}); prefer the plugin files API. \
                 This check is advisory; QuickJs is the security boundary.",
                filesystem.join(", ")
            ));
        }
    }
    Ok(warnings)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Str,
    Template,
    Regex,
    Identifier,
    Punctuation,
}

struct Token {
    kind: TokenKind,
    value: String,
    line: usize,
}

fn javascript_tokens(source: &str) -> Result<Vec<Token>, String> {
    let characters: Vec<char> = source.chars().collect();
    let mut tokens: Vec<Token> = Vec::new();
    let mut index = 0usize;
    let mut line = 1usize;
    while index < characters.len() {
        let char = characters[index];
        if char.is_whitespace() {
            if char == '\n' {
                line += 1;
            }
            index += 1;
            continue;
        }
        if char == '/' && characters.get(index + 1) == Some(&'/') {
            let Some(newline) = characters[index + 2..].iter().position(|value| *value == '\n') else {
                break;
            };
            index = index + 2 + newline;
            continue;
        }
        if char == '/' && characters.get(index + 1) == Some(&'*') {
            let Some(end) = find_substring(&characters, "*/", index + 2) else {
                return Err("JavaScript source contains an unterminated block comment.".to_owned());
            };
            line += characters[index..end + 2].iter().filter(|value| **value == '\n').count();
            index = end + 2;
            continue;
        }
        if char == '\'' || char == '"' {
            index = consume_quoted(&characters, index, char, &mut line)
                .ok_or_else(|| "JavaScript source contains an unterminated string.".to_owned())?;
            tokens.push(Token { kind: TokenKind::Str, value: String::new(), line });
            continue;
        }
        if char == '`' {
            index = consume_quoted(&characters, index, char, &mut line)
                .ok_or_else(|| "JavaScript source contains an unterminated template string.".to_owned())?;
            tokens.push(Token { kind: TokenKind::Template, value: String::new(), line });
            continue;
        }
        if char == '/' && regex_can_start(&tokens) {
            let start_line = line;
            index += 1;
            let mut in_character_class = false;
            while index < characters.len() {
                let current = characters[index];
                if current == '\\' {
                    index += 2;
                    continue;
                }
                if current == '[' {
                    in_character_class = true;
                } else if current == ']' {
                    in_character_class = false;
                } else if current == '/' && !in_character_class {
                    index += 1;
                    while index < characters.len() && characters[index].is_alphabetic() {
                        index += 1;
                    }
                    break;
                } else if current == '\n' {
                    break;
                }
                index += 1;
            }
            tokens.push(Token { kind: TokenKind::Regex, value: String::new(), line: start_line });
            continue;
        }
        if char.is_alphabetic() || char == '_' || char == '$' {
            let start = index;
            index += 1;
            while index < characters.len()
                && (characters[index].is_alphanumeric() || characters[index] == '_' || characters[index] == '$')
            {
                index += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Identifier,
                value: characters[start..index].iter().collect(),
                line,
            });
            continue;
        }
        tokens.push(Token { kind: TokenKind::Punctuation, value: char.to_string(), line });
        index += 1;
    }
    Ok(tokens)
}

/// строка/шаблон: возвращает индекс за закрывающей кавычкой или None,
/// если кавычка не закрыта до конца файла
fn consume_quoted(characters: &[char], mut index: usize, quote: char, line: &mut usize) -> Option<usize> {
    index += 1;
    while index < characters.len() {
        if characters[index] == '\\' {
            if characters.get(index + 1) == Some(&'\n') {
                *line += 1;
            }
            index += 2;
            continue;
        }
        if characters[index] == quote {
            return Some(index + 1);
        }
        if characters[index] == '\n' {
            *line += 1;
        }
        index += 1;
    }
    None
}

fn find_substring(characters: &[char], needle: &str, from: usize) -> Option<usize> {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() {
        return Some(from);
    }
    (from..=characters.len().saturating_sub(needle.len()))
        .find(|start| characters[*start..*start + needle.len()] == needle[..])
}

fn regex_can_start(tokens: &[Token]) -> bool {
    let Some(last) = tokens.last() else {
        return true;
    };
    matches!(
        last.value.as_str(),
        "=" | "(" | "[" | "{" | "," | ":" | ";" | "!" | "?" | "&" | "|" | "return" | "case" | "throw" | "=>"
    )
}

fn unsupported_import_line(tokens: &[Token]) -> Option<usize> {
    for index in 0..tokens.len() {
        let token = &tokens[index];
        if token.kind != TokenKind::Identifier {
            continue;
        }
        let previous = if index > 0 { tokens[index - 1].value.as_str() } else { "" };
        let following = tokens
            .get(index + 1)
            .map(|next| next.value.as_str())
            .unwrap_or_default();
        if token.value == "import" && previous != "." {
            if following == "("
                || (following == "."
                    && tokens.get(index + 2).map(|next| next.value.as_str()) == Some("meta"))
            {
                continue;
            }
            return Some(token.line);
        }
        if token.value == "require" && previous != "." && following == "("
            && tokens.get(index + 2).map(|next| next.kind) == Some(TokenKind::Str)
        {
            return Some(token.line);
        }
        if token.value == "export" {
            let mut cursor = index + 1;
            while cursor + 1 < tokens.len() && tokens[cursor].value != ";" {
                if tokens[cursor].kind == TokenKind::Identifier
                    && tokens[cursor].value == "from"
                    && tokens[cursor + 1].kind == TokenKind::Str
                {
                    return Some(token.line);
                }
                cursor += 1;
            }
        }
    }
    None
}

// ── пути и файловые примитивы ───────────────────────────────────────

fn plugin_id_from_directory(name: &str) -> String {
    let lowered = name.to_lowercase();
    let mut plugin_id = String::new();
    let mut previous_dash = false;
    for char in lowered.chars() {
        let keep = char.is_ascii_lowercase() || char.is_ascii_digit() || char == '.' || char == '_' || char == '-';
        if keep {
            plugin_id.push(char);
            previous_dash = false;
        } else if !previous_dash {
            plugin_id.push('-');
            previous_dash = true;
        }
    }
    let trimmed = plugin_id.trim_matches(|char| char == '.' || char == '_' || char == '-');
    let mut plugin_id = trimmed.to_owned();
    if plugin_id.is_empty() || !plugin_id.starts_with(|char: char| char.is_ascii_lowercase()) {
        plugin_id = format!("plugin-{plugin_id}");
    }
    if plugin_id.chars().count() < 2 {
        plugin_id = format!("{plugin_id}-plugin");
    }
    plugin_id.chars().take(64).collect()
}

pub(crate) fn normalize_archive_path(name: &str, is_directory: bool) -> Result<String, String> {
    if name.is_empty() || name.contains('\0') || name.contains('\\') {
        return Err("Plugin package contains an invalid path.".to_owned());
    }
    let normalized = if is_directory && name.ends_with('/') {
        &name[..name.len() - 1]
    } else {
        name
    };
    let absolute_drive = normalized
        .char_indices()
        .nth(1)
        .map(|(offset, char)| char == ':' && offset == 1)
        .unwrap_or(false);
    if normalized.starts_with('/') || absolute_drive {
        return Err("Plugin package contains an absolute path.".to_owned());
    }
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == ".." || part.contains(':'))
    {
        return Err("Plugin package contains an unsafe path.".to_owned());
    }
    if parts
        .iter()
        .any(|part| part.chars().any(|char| (char as u32) < 32 || (char as u32) == 127))
    {
        return Err("Plugin package contains a path with control characters.".to_owned());
    }
    if normalized.len() > 1024 || parts.iter().any(|part| part.len() > 255) {
        return Err("Plugin package contains an excessively long path.".to_owned());
    }
    Ok(normalized.to_owned())
}

fn read_private_key(path: &Path) -> Result<Vec<u8>, String> {
    let key_path = expand_home(path);
    let metadata =
        fs::symlink_metadata(&key_path).map_err(|_| "Private key must be a regular PEM file, not a symbolic link.".to_owned())?;
    if metadata.is_symlink() || !metadata.is_file() {
        return Err("Private key must be a regular PEM file, not a symbolic link.".to_owned());
    }
    let mut content = Vec::new();
    File::open(&key_path)
        .map_err(|error| format!("Could not read private key: {error}"))?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut content)
        .map_err(|error| format!("Could not read private key: {error}"))?;
    if content.is_empty() || content.len() > 1024 * 1024 {
        return Err("Private key file is empty or too large.".to_owned());
    }
    Ok(content)
}

fn key_output_paths(private_path: &Path, public_path: &Path) -> Result<(PathBuf, PathBuf), String> {
    let paths = [expand_home(private_path), expand_home(public_path)];
    if resolve_non_strict(&paths[0]) == resolve_non_strict(&paths[1]) {
        return Err("Private and public key paths must be different.".to_owned());
    }
    let mut normalized: Vec<PathBuf> = Vec::new();
    for path in &paths {
        let metadata = fs::symlink_metadata(path);
        if metadata.as_ref().map(|meta| meta.is_symlink()).unwrap_or(false) || path.exists() {
            return Err(format!(
                "Key output already exists or is a symbolic link: {}",
                path.display()
            ));
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        if !parent.is_dir() {
            return Err(format!("Key output directory does not exist: {}", parent.display()));
        }
        let parent = canonicalize_strict(parent, "Could not resolve key output directory")?;
        normalized.push(parent.join(path.file_name().unwrap_or_default()));
    }
    if normalized[0] == normalized[1] {
        return Err("Private and public key paths must be different.".to_owned());
    }
    Ok((normalized[0].clone(), normalized[1].clone()))
}

fn write_new_file(path: &Path, content: &[u8], mode: u32) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    let mut handle = options
        .open(path)
        .map_err(|error| format!("Could not write Ed25519 key files: {error}"))?;
    if let Err(error) = handle.write_all(content).and_then(|_| handle.sync_all()) {
        let _ = fs::remove_file(path);
        return Err(format!("Could not write Ed25519 key files: {error}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }
    Ok(())
}

fn pem_wrap(label: &str, der: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let mut output = format!("-----BEGIN {label}-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        output.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        output.push('\n');
    }
    output.push_str(&format!("-----END {label}-----\n"));
    output
}

/// разворачивает ведущий `~` через `$HOME`
fn expand_home(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if text == "~" || text.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            if text == "~" {
                return home;
            }
            return home.join(&text[2..]);
        }
    }
    path.to_path_buf()
}

fn symlink_metadata(path: &Path) -> Option<fs::Metadata> {
    fs::symlink_metadata(path).ok()
}

fn canonicalize_strict(path: &Path, label: &str) -> Result<PathBuf, String> {
    fs::canonicalize(path).map_err(|error| format!("{label}: {error}"))
}

/// канонизирует существующую часть пути и сохраняет отсутствующий хвост
fn resolve_non_strict(path: &Path) -> PathBuf {
    if let Ok(resolved) = fs::canonicalize(path) {
        return resolved;
    }
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();
    while let (Some(file_name), Some(parent)) = (
        current.file_name().map(|name| name.to_owned()),
        current.parent().map(Path::to_path_buf),
    ) {
        tail.push(file_name);
        if let Ok(resolved) = fs::canonicalize(&parent) {
            let mut result = resolved;
            for component in tail.iter().rev() {
                result.push(component);
            }
            return result;
        }
        if parent == current {
            break;
        }
        current = parent;
    }
    path.to_path_buf()
}

fn is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

/// сортирует ключи, сохраняет Unicode и добавляет перевод строки
fn json_bytes(value: &Value) -> Result<Vec<u8>, String> {
    let sorted = sorted_value(value);
    let text = serde_json::to_string(&sorted)
        .map_err(|_| "Plugin manifest contains values that cannot be encoded as JSON.".to_owned())?;
    Ok(format!("{text}\n").into_bytes())
}

fn sorted_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), sorted_value(&map[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted_value).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nivora-plugin-dev-{tag}-{}-{}",
            std::process::id(),
            super::super::mods::random_hex(4)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_project(root: &Path, manifest: &Value, entrypoint: &str) {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join(PLUGIN_MANIFEST), json_bytes(manifest).unwrap()).unwrap();
        fs::write(root.join("main.js"), entrypoint).unwrap();
    }

    fn sample_manifest() -> Value {
        json!({
            "id": "sample-plugin",
            "name": "Sample Plugin",
            "version": "0.1.0",
            "api_version": "3",
            "author": "Plugin author",
            "description": "A Nivora Launcher plugin.",
            "entrypoint": "main.js",
            "permissions": [],
            "sandbox_profile": "strict",
        })
    }

    #[test]
    fn init_creates_starter_and_refuses_overwrite() {
        let root = temp_root("init");
        let project = root.join("My Cool.Plugin");
        let created = init_plugin(&project).unwrap();
        assert!(created.join(PLUGIN_MANIFEST).is_file());
        assert!(created.join("main.js").is_file());
        assert!(created.join("README.md").is_file());
        let manifest: Value =
            serde_json::from_slice(&fs::read(created.join(PLUGIN_MANIFEST)).unwrap()).unwrap();
        assert_eq!(manifest["id"], json!("my-cool.plugin"));
        assert_eq!(manifest["entrypoint"], json!("main.js"));
        // повторный init не перезаписывает стартовые файлы
        let error = init_plugin(&project).unwrap_err();
        assert_eq!(error, "Plugin starter files already exist; init will not overwrite files.");
    }

    #[test]
    fn plugin_id_normalization_matches_python() {
        assert_eq!(plugin_id_from_directory("My Cool.Plugin"), "my-cool.plugin");
        assert_eq!(plugin_id_from_directory("1st plugin"), "plugin-1st-plugin");
        assert_eq!(plugin_id_from_directory("a"), "a-plugin");
        assert_eq!(plugin_id_from_directory("..."), "plugin-");
    }

    #[test]
    fn lint_reports_advisory_apis_and_rejects_imports() {
        let root = temp_root("lint");
        write_project(
            &root,
            &sample_manifest(),
            "const ws = new WebSocket('ws://localhost');\nexport default {};\n",
        );
        let warnings = lint_plugin(&root).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("main.js: direct network API(s) detected (WebSocket)"));

        let root = temp_root("lint-import");
        write_project(&root, &sample_manifest(), "import x from './x.js';\n");
        let error = lint_plugin(&root).unwrap_err();
        assert!(error.starts_with("Unsupported static ESM/CommonJS import in main.js:1"));
    }

    #[test]
    fn tokenizer_flags_line_of_require() {
        let tokens = javascript_tokens("const a = 1;\nconst b = require('fs');\n").unwrap();
        assert_eq!(unsupported_import_line(&tokens), Some(2));
        // разрешены динамический import и `import.meta`
        let tokens = javascript_tokens("const a = import('./x.js');\nconst b = import.meta.url;\n").unwrap();
        assert_eq!(unsupported_import_line(&tokens), None);
        // export ... from '...'
        let tokens = javascript_tokens("export { x } from './x.js';\n").unwrap();
        assert_eq!(unsupported_import_line(&tokens), Some(1));
        // property access .require не считается CommonJS
        let tokens = javascript_tokens("const a = mod.require('fs');\n").unwrap();
        assert_eq!(unsupported_import_line(&tokens), None);
    }

    #[test]
    fn tokenizer_survives_regex_and_comments() {
        let source = "// import x from 'y'\nconst re = /import \\/ x[/];/g;\n/* export z\n from 'q' */ let s = 'a`b';\n";
        let tokens = javascript_tokens(source).unwrap();
        assert_eq!(unsupported_import_line(&tokens), None);
        assert!(javascript_tokens("const s = 'unterminated").is_err());
        assert!(javascript_tokens("/* forever").is_err());
    }

    #[test]
    fn pack_and_verify_roundtrip_unsigned() {
        let root = temp_root("pack");
        let project = root.join("sample-plugin");
        write_project(&project, &sample_manifest(), "export default {};\n");
        fs::create_dir_all(project.join("assets")).unwrap();
        fs::write(project.join("assets/icon.png"), b"png").unwrap();

        let output = root.join("out.nivora-plugin.zip");
        let archive = pack_plugin(&project, &output, None).unwrap();
        assert!(archive.is_file());

        let result = verify_plugin(&archive).unwrap();
        assert_eq!(result["id"], json!("sample-plugin"));
        assert_eq!(result["version"], json!("0.1.0"));
        assert_eq!(result["signed"], json!(false));

        // повторная упаковка должна давать те же байты
        let output2 = root.join("out2.nivora-plugin.zip");
        pack_plugin(&project, &output2, None).unwrap();
        assert_eq!(fs::read(&output).unwrap(), fs::read(&output2).unwrap());

        // упаковка внутрь каталога проекта запрещена
        let error = pack_plugin(&project, &project.join("inside.zip"), None).unwrap_err();
        assert_eq!(error, "Plugin archive output must be outside the plugin directory.");
    }

    #[test]
    fn signed_roundtrip_verifies_with_public_key() {
        let root = temp_root("sign");
        let (private_pem, public_pem) = generate_keypair(&root.join("signing.pem"), &root.join("signing.pub")).unwrap();
        assert!(private_pem.is_file() && public_pem.is_file());
        let private_text = fs::read_to_string(&private_pem).unwrap();
        assert!(private_text.starts_with("-----BEGIN PRIVATE KEY-----\n"));

        let project = root.join("sample-plugin");
        write_project(&project, &sample_manifest(), "export default {};\n");
        let output = root.join("signed.nivora-plugin.zip");
        pack_plugin(&project, &output, Some(&private_pem)).unwrap();

        let result = verify_plugin(&output).unwrap();
        assert_eq!(result["signed"], json!(true));
        assert_eq!(result["id"], json!("sample-plugin"));

        // повреждение содержимого entrypoint ломает целостность: перепаковываем
        // архив с изменённым main.js (в deflate-потоке байты не найти)
        let mut source = zip::ZipArchive::new(File::open(&output).unwrap()).unwrap();
        let broken_path = root.join("broken.zip");
        let file = File::create(&broken_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        for index in 0..source.len() {
            let mut entry = source.by_index(index).unwrap();
            let name = entry.name().map(|v| v.to_string()).unwrap_or_default();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            if name == "main.js" && !data.is_empty() {
                data[0] = b'X';
            }
            writer
                .start_file(name.as_str(), zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&data).unwrap();
        }
        writer.finish().unwrap();
        let error = verify_plugin(&broken_path).unwrap_err();
        assert!(
            error.contains("integrity") || error.contains("signature"),
            "неожиданная ошибка: {error}"
        );
    }

    #[test]
    fn verify_rejects_archives_without_manifest() {
        let root = temp_root("verify");
        let archive = root.join("empty.zip");
        let file = File::create(&archive).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("readme.txt", options).unwrap();
        writer.write_all(b"no manifest here").unwrap();
        writer.finish().unwrap();
        let error = verify_plugin(&archive).unwrap_err();
        assert_eq!(error, "Plugin archive must contain a root plugin.json manifest.");
    }

    #[test]
    fn normalize_paths_rejects_traversal_and_absolute() {
        assert!(normalize_archive_path("a/b.js", false).is_ok());
        assert!(normalize_archive_path("dir/", true).is_ok());
        assert_eq!(
            normalize_archive_path("../escape.js", false).unwrap_err(),
            "Plugin package contains an unsafe path."
        );
        assert_eq!(
            normalize_archive_path("/etc/passwd", false).unwrap_err(),
            "Plugin package contains an absolute path."
        );
        assert_eq!(
            normalize_archive_path("C:\\evil.js", false).unwrap_err(),
            "Plugin package contains an invalid path."
        );
    }

    #[test]
    fn keypair_rejects_existing_and_equal_paths() {
        let root = temp_root("keygen");
        let error = generate_keypair(&root.join("same.pem"), &root.join("same.pem")).unwrap_err();
        assert_eq!(error, "Private and public key paths must be different.");
        let existing = root.join("exists.pem");
        fs::write(&existing, b"x").unwrap();
        let error = generate_keypair(&existing, &root.join("other.pub")).unwrap_err();
        assert!(error.starts_with("Key output already exists or is a symbolic link"));
    }

    #[test]
    fn json_bytes_sorts_keys_compactly_with_newline() {
        let value = json!({"b": 1, "a": {"d": [1, 2], "c": true}});
        let bytes = json_bytes(&value).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "{\"a\":{\"c\":true,\"d\":[1,2]},\"b\":1}\n"
        );
    }
}
