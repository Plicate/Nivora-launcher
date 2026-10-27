use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use super::{
    atomic_json_write, parse_strict_json, path_is_symlink, HEX64_RE, MAX_PERMISSION_STATE_BYTES,
    PLUGIN_ID_RE, PLUGIN_PERMISSION_NAMES, PLUGIN_PERMISSION_SCHEMA,
};

pub(super) fn read_permission_state(path: &Path) -> Result<Value, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"schema_version": PLUGIN_PERMISSION_SCHEMA, "plugins": {}}));
        }
        Err(_) => return Err("Could not inspect plugin permission grants".to_owned()),
    };
    if !metadata.is_file() || metadata.len() > MAX_PERMISSION_STATE_BYTES as u64 {
        return Err("Plugin permission grants must be a bounded regular file".to_owned());
    }
    let raw = read_private_file(path, MAX_PERMISSION_STATE_BYTES)
        .map_err(|_| "Could not securely read plugin permission grants".to_owned())?;
    let Some(raw) = raw else {
        return Ok(json!({"schema_version": PLUGIN_PERMISSION_SCHEMA, "plugins": {}}));
    };
    if raw.len() > MAX_PERMISSION_STATE_BYTES {
        return Err("Plugin permission grants file is too large".to_owned());
    }
    let state = parse_strict_json(&raw, "Plugin permission grants")
        .map_err(|_| "Plugin permission grants contain invalid JSON".to_owned())?;
    validate_permission_state(&state)?;
    Ok(state)
}

pub(super) fn validate_permission_state(value: &Value) -> Result<(), String> {
    let Some(fields) = value.as_object() else {
        return Err("Plugin permission grants have an unsupported format".to_owned());
    };
    if fields.len() != 2 || !fields.contains_key("schema_version") || !fields.contains_key("plugins") {
        return Err("Plugin permission grants have an unsupported format".to_owned());
    }
    if fields.get("schema_version").and_then(Value::as_i64) != Some(PLUGIN_PERMISSION_SCHEMA) {
        return Err("Plugin permission grants have an unsupported schema version".to_owned());
    }
    let Some(plugins) = fields.get("plugins").and_then(Value::as_object) else {
        return Err("Plugin permission grants plugins must be an object".to_owned());
    };
    for (plugin_id, record) in plugins {
        if !PLUGIN_ID_RE.is_match(plugin_id) {
            return Err("Plugin permission grants contain an unsafe plugin id".to_owned());
        }
        let Some(record) = record.as_object() else {
            return Err("Plugin permission grants contain an invalid record".to_owned());
        };
        if record.len() != 2 || !record.contains_key("grants") || !record.contains_key("untrusted_execution_hash") {
            return Err("Plugin permission grants contain an invalid record".to_owned());
        }
        let grants = record.get("grants").cloned().unwrap_or(Value::Null);
        let Some(grants) = grants.as_array() else {
            return Err("Plugin permission grants contain an invalid permission".to_owned());
        };
        for grant in grants {
            let Some(name) = grant.as_str() else {
                return Err("Plugin permission grants contain an invalid permission".to_owned());
            };
            if !PLUGIN_PERMISSION_NAMES.contains(name) {
                return Err("Plugin permission grants contain an invalid permission".to_owned());
            }
        }
        let unique: HashSet<&str> = grants.iter().filter_map(Value::as_str).collect();
        if unique.len() != grants.len() {
            return Err("Plugin permission grants contain duplicate permissions".to_owned());
        }
        let package_hash = record.get("untrusted_execution_hash").cloned().unwrap_or(Value::Null);
        let Some(package_hash) = package_hash.as_str() else {
            return Err("Plugin permission grants contain an invalid local execution hash".to_owned());
        };
        if !package_hash.is_empty() && !HEX64_RE.is_match(package_hash) {
            return Err("Plugin permission grants contain an invalid local execution hash".to_owned());
        }
    }
    Ok(())
}

pub(super) fn write_permission_state(path: &Path, value: &Value) -> Result<(), String> {
    validate_permission_state(value)?;
    if path_is_symlink(path) {
        return Err("Plugin permission grants must not be a symbolic link".to_owned());
    }
    atomic_json_write(path, value, true).map_err(|_| "Could not save plugin permission grants".to_owned())
}

/// на Unix открывает файл без перехода по ссылкам и задаёт права 0600
fn read_private_file(path: &Path, max_bytes: usize) -> Result<Option<Vec<u8>>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::FromRawFd;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "Could not securely read plugin permission grants".to_owned())?;
        let fd = unsafe { libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            return match error.kind() {
                std::io::ErrorKind::NotFound => Ok(None),
                _ => Err("Could not securely read plugin permission grants".to_owned()),
            };
        }
        if unsafe { libc::fchmod(fd, 0o600) } != 0 {
            unsafe { libc::close(fd) };
            return Err("Could not securely read plugin permission grants".to_owned());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let mut buffer = Vec::new();
        let outcome = file.take(max_bytes as u64 + 1).read_to_end(&mut buffer);
        match outcome {
            Ok(_) if buffer.len() <= max_bytes => Ok(Some(buffer)),
            Ok(_) => Err("Plugin permission grants file is too large".to_owned()),
            Err(_) => Err("Could not securely read plugin permission grants".to_owned()),
        }
    }
    #[cfg(not(unix))]
    {
        if fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(true) {
            return Err("Could not securely read plugin permission grants".to_owned());
        }
        match fs::read(path) {
            Ok(buffer) if buffer.len() <= max_bytes => Ok(Some(buffer)),
            Ok(_) => Err("Plugin permission grants file is too large".to_owned()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("Could not securely read plugin permission grants".to_owned()),
        }
    }
}
