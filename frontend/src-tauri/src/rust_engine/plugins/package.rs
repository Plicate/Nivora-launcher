use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use super::{
    parse_v2_manifest_bytes, path_exists, path_is_dir, path_is_symlink, sha256_hex,
    validate_plugin_json_manifest, HEX64_RE, PLUGIN_MANIFEST, PLUGIN_MAX_FILES,
    PLUGIN_MAX_MANIFEST_BYTES, PLUGIN_MAX_UNPACKED_BYTES, PLUGIN_V2_MANIFEST,
    V2_DATA_DIRECTORIES, WINDOWS_DRIVE_RE,
};

pub(super) struct V2Package {
    pub(super) manifest: Value,
    pub(super) files: BTreeMap<String, Vec<u8>>,
    pub(super) signed: bool,
    pub(super) key_id: Option<String>,
    pub(super) public_key: Option<String>,
    pub(super) fingerprint: Option<String>,
    pub(super) package_digest: String,
}

pub(super) fn read_v2_package(root: &Path, expected_id: Option<&str>) -> Result<V2Package, String> {
    if path_is_symlink(root) || !path_is_dir(root) {
        return Err("Plugin package root must be a real directory".to_owned());
    }
    if path_exists(&root.join(PLUGIN_MANIFEST)) || path_is_symlink(&root.join(PLUGIN_MANIFEST)) {
        return Err("Plugin package cannot mix plugin.yml and plugin.json".to_owned());
    }
    let (file_bytes, manifest_bytes) = collect_v2_files(root)?;
    let raw_manifest = parse_v2_manifest_bytes(&manifest_bytes)?;
    let manifest = validate_plugin_json_manifest(&raw_manifest)?;
    if let Some(expected) = expected_id {
        if manifest.get("id").and_then(Value::as_str) != Some(expected) {
            return Err("plugin.json id does not match its plugin directory".to_owned());
        }
    }
    let entrypoint = manifest.get("entrypoint").and_then(Value::as_str).unwrap_or_default().to_owned();
    if !file_bytes.contains_key(&entrypoint) {
        return Err("Plugin package is missing its entrypoint".to_owned());
    }

    let signed = raw_manifest.as_object().map(|fields| fields.contains_key("signature")).unwrap_or(false);
    let (key_id, public_key, fingerprint): (Option<String>, Option<String>, Option<String>);
    if signed {
        let verification = super::super::plugin_security::verify_manifest(&raw_manifest, &file_bytes);
        let verified = verification.get("signed") == Some(&json!(true))
            && verification.get("valid") == Some(&json!(true));
        if !verified {
            let reason = verification
                .get("error")
                .and_then(Value::as_str)
                .filter(|error| !error.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| "invalid package".to_owned());
            return Err(format!("Plugin signature or integrity verification failed: {reason}"));
        }
        key_id = verification.get("key_id").and_then(Value::as_str).map(str::to_owned);
        public_key = verification.get("public_key").and_then(Value::as_str).map(str::to_owned);
        fingerprint = verification.get("fingerprint").and_then(Value::as_str).map(str::to_owned);
    } else {
        verify_unsigned_v2_integrity(&raw_manifest, &file_bytes)?;
        key_id = None;
        public_key = None;
        fingerprint = None;
    }

    let canonical = super::super::plugin_security::canonical_json_bytes(&raw_manifest, "plugin.json")
        .map_err(|_| "plugin.json cannot be encoded as canonical JSON".to_owned())?;
    Ok(V2Package {
        manifest,
        files: file_bytes,
        signed,
        key_id,
        public_key,
        fingerprint,
        package_digest: sha256_hex(&canonical),
    })
}

fn collect_v2_files(root: &Path) -> Result<(BTreeMap<String, Vec<u8>>, Vec<u8>), String> {
    let manifest_path = root.join(PLUGIN_V2_MANIFEST);
    let metadata = fs::symlink_metadata(&manifest_path)
        .map_err(|_| "Plugin package must contain a root plugin.json".to_owned())?;
    if !metadata.is_file() || metadata.len() > PLUGIN_MAX_MANIFEST_BYTES as u64 {
        return Err("plugin.json must be a bounded regular file".to_owned());
    }
    let manifest_bytes = read_regular_package_file(&manifest_path, PLUGIN_MAX_MANIFEST_BYTES)?;

    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut total_size = manifest_bytes.len() as u64;
    let mut file_count: usize = 1;
    walk_v2_package(root, root, &mut files, &mut total_size, &mut file_count)?;
    Ok((files, manifest_bytes))
}

fn relative_posix(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().to_string())
}

fn walk_v2_package(
    directory: &Path,
    root: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
    total_size: &mut u64,
    file_count: &mut usize,
) -> Result<(), String> {
    let inspect = |error: std::io::Error| format!("Could not inspect plugin package files: {error}");
    let mut entries: Vec<(std::ffi::OsString, PathBuf)> = Vec::new();
    for entry in fs::read_dir(directory).map_err(inspect)? {
        let entry = entry.map_err(inspect)?;
        entries.push((entry.file_name(), entry.path()));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut kept_directories: Vec<PathBuf> = Vec::new();
    for (name, path) in &entries {
        let follows_dir = fs::metadata(path).map(|metadata| metadata.is_dir()).unwrap_or(false);
        if !follows_dir {
            continue;
        }
        let lstat = fs::symlink_metadata(path).map_err(inspect)?;
        if lstat.is_symlink() {
            return Err(format!("Plugin package contains a symbolic link: {}", relative_posix(path, root)));
        }
        if !lstat.is_dir() {
            return Err(format!("Plugin package contains a non-directory path: {}", relative_posix(path, root)));
        }
        let relative = v2_package_path(&relative_posix(path, root))?;
        let name_text = name.to_string_lossy();
        if relative.split('/').count() == 1 && V2_DATA_DIRECTORIES.contains(&name_text.as_ref()) {
            continue;
        }
        if relative == ".enabled" {
            return Err("plugin.json package uses a reserved runtime path".to_owned());
        }
        kept_directories.push(path.clone());
    }

    for (name, path) in &entries {
        let _ = name;
        let follows_dir = fs::metadata(path).map(|metadata| metadata.is_dir()).unwrap_or(false);
        if follows_dir {
            continue;
        }
        let lstat = fs::symlink_metadata(path).map_err(inspect)?;
        let relative = v2_package_path(&relative_posix(path, root))?;
        if lstat.is_symlink() {
            return Err(format!("Plugin package contains a symbolic link: {relative}"));
        }
        if !lstat.is_file() {
            return Err(format!("Plugin package contains a non-regular file: {relative}"));
        }
        if relative == PLUGIN_V2_MANIFEST {
            continue;
        }
        if relative == ".enabled" {
            if lstat.len() != 0 {
                return Err("Plugin enabled marker is malformed".to_owned());
            }
            continue;
        }
        if relative == PLUGIN_MANIFEST {
            return Err("Plugin package cannot mix plugin.yml and plugin.json".to_owned());
        }
        let remaining = (PLUGIN_MAX_UNPACKED_BYTES as u64).saturating_sub(*total_size);
        if lstat.len() > remaining {
            return Err("Plugin package exceeds the unpacked size limit".to_owned());
        }
        let content = read_regular_package_file(path, remaining as usize)?;
        *total_size += content.len() as u64;
        *file_count += 1;
        if *file_count > PLUGIN_MAX_FILES {
            return Err("Plugin package contains too many files".to_owned());
        }
        files.insert(relative, content);
    }

    for path in kept_directories {
        walk_v2_package(&path, root, files, total_size, file_count)?;
    }
    Ok(())
}

pub(super) fn read_regular_package_file(path: &Path, size_limit: usize) -> Result<Vec<u8>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::FromRawFd;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "Could not securely read plugin package file".to_owned())?;
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err("Could not securely read plugin package file".to_owned());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        // проверяем тип и размер файла до чтения
        let metadata = file
            .metadata()
            .map_err(|_| "Could not securely read plugin package file".to_owned())?;
        if !metadata.is_file() || metadata.len() > size_limit as u64 {
            return Err("Plugin package file is not a bounded regular file".to_owned());
        }
        let mut buffer = Vec::new();
        if file
            .take(size_limit as u64 + 1)
            .read_to_end(&mut buffer)
            .is_err()
        {
            return Err("Could not securely read plugin package file".to_owned());
        }
        if buffer.len() > size_limit || buffer.len() as u64 != metadata.len() {
            return Err("Plugin package file changed while it was being read".to_owned());
        }
        Ok(buffer)
    }
    #[cfg(not(unix))]
    {
        let metadata = fs::symlink_metadata(path).map_err(|_| "Could not securely read plugin package file".to_owned())?;
        if metadata.is_symlink() || !metadata.is_file() || metadata.len() > size_limit as u64 {
            return Err("Plugin package file is not a bounded regular file".to_owned());
        }
        fs::read(path).map_err(|_| "Could not securely read plugin package file".to_owned())
    }
}

pub(super) fn v2_package_path(value: &str) -> Result<String, String> {
    if value.is_empty()
        || value.contains('\\')
        || value.contains('\0')
        || value.starts_with('/')
        || WINDOWS_DRIVE_RE.is_match(value)
    {
        return Err("Plugin package contains an unsafe path".to_owned());
    }
    if value.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
        return Err("Plugin package contains an unsafe path".to_owned());
    }
    Ok(value.to_owned())
}

pub(super) fn reject_v2_runtime_paths(root: &Path) -> Result<(), String> {
    for name in ["storage", "files", "config", "database", ".enabled"] {
        let child = root.join(name);
        if path_exists(&child) || path_is_symlink(&child) {
            return Err(format!("Plugin package uses a reserved runtime path: {name}"));
        }
    }
    Ok(())
}

pub(crate) fn verify_unsigned_v2_integrity(manifest: &Value, file_bytes: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let integrity = manifest.get("integrity");
    let valid_shape = integrity
        .and_then(Value::as_object)
        .map(|fields| fields.len() == 1 && fields.contains_key("files"))
        .unwrap_or(false)
        && integrity
            .and_then(|value| value.get("files"))
            .map(Value::is_object)
            .unwrap_or(false);
    if !valid_shape {
        return Err("Unsigned plugin.json must include integrity.files".to_owned());
    }
    let Some(expected) = integrity.and_then(|value| value.get("files")).and_then(Value::as_object) else {
        return Err("Unsigned plugin.json must include integrity.files".to_owned());
    };
    if expected.len() != file_bytes.len() || file_bytes.keys().any(|path| !expected.contains_key(path)) {
        return Err("Plugin integrity hashes must exactly cover every package file except plugin.json".to_owned());
    }
    for (path, content) in file_bytes {
        let Some(digest) = expected.get(path).and_then(Value::as_str) else {
            return Err(format!("Plugin integrity hash is invalid for {path}"));
        };
        if !HEX64_RE.is_match(digest) {
            return Err(format!("Plugin integrity hash is invalid for {path}"));
        }
        if digest != sha256_hex(content) {
            return Err(format!("Plugin file hash mismatch: {path}"));
        }
    }
    Ok(())
}
