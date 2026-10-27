// доверяем только ключам, явно закреплённым пользователем; встроенных ключей нет
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::mods::random_hex;

pub(crate) const MAX_PACKAGE_FILES: usize = 4096;
const STORE_FILENAME: &str = "plugin-trust.json";
const STORE_SCHEMA_VERSION: i64 = 1;
const MAX_STORE_BYTES: usize = 16 * 1024 * 1024;
const MAX_REVOCATION_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_REASON_LENGTH: usize = 1024;

pub(crate) fn identifier_valid(value: &str) -> bool {
    let mut characters = value.chars();
    let Some(first) = characters.next() else { return false };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    let rest: usize = characters.count();
    if rest == 0 || rest > 127 {
        return false;
    }
    value
        .chars()
        .skip(1)
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | ':' | '-'))
}

fn reason_valid(value: &str) -> bool {
    value.chars().count() <= MAX_REASON_LENGTH && !value.contains('\0')
}

/// путь пакета: непустой, относительный, нормализованный, без "."/".."
fn package_path_valid(value: &str) -> Result<(), String> {
    if value.is_empty() || value.contains('\0') {
        return Err("Plugin package paths must be non-empty relative paths".to_owned());
    }
    if value.contains('\\') {
        return Err("Plugin package paths must use forward slashes, not backslashes".to_owned());
    }
    let mut characters = value.chars();
    let first = characters.next().unwrap_or('\0');
    let windows_drive = first.is_ascii_alphabetic() && characters.next() == Some(':');
    if value.starts_with('/') || windows_drive {
        return Err("Plugin package paths must not be absolute".to_owned());
    }
    let parts: Vec<&str> = value.split('/').collect();
    if parts.iter().any(|part| part.is_empty() || *part == "." || *part == "..") {
        return Err("Plugin package paths must be normalized and cannot contain '.' or '..'".to_owned());
    }
    if value == "plugin.json" {
        return Err("plugin.json is excluded from the plugin file hash map".to_owned());
    }
    Ok(())
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn fingerprint(public_key: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(public_key))
}

fn decode_base64(value: &str, expected_length: usize, label: &str) -> Result<Vec<u8>, String> {
    let expected_encoded_length = 4 * ((expected_length + 2) / 3);
    if value.len() != expected_encoded_length {
        return Err(format!("{label} has an invalid encoded length"));
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value.as_bytes())
        .map_err(|_| format!("{label} is not valid base64"))?;
    let reencoded = base64::engine::general_purpose::STANDARD.encode(&decoded);
    if reencoded != value {
        return Err(format!("{label} must use canonical base64 encoding"));
    }
    if decoded.len() != expected_length {
        return Err(format!("{label} must decode to exactly {expected_length} bytes"));
    }
    Ok(decoded)
}

/// принимает публичный ключ длиной 32 байта или в каноническом Base64
pub(crate) fn coerce_public_key(value: &Value, label: &str) -> Result<Vec<u8>, String> {
    match value {
        Value::String(text) => decode_base64(text, 32, label),
        other => {
            let _ = other;
            Err(format!("{label} must be 32 raw Ed25519 bytes or their base64 encoding"))
        }
    }
}

fn sorted_deep(value: &Value, depth: usize, label: &str) -> Result<Value, String> {
    if depth > 128 {
        return Err(format!("{label} is nested too deeply"));
    }
    match value {
        Value::Number(number) => {
            if let Some(float) = number.as_f64() {
                if !float.is_finite() {
                    return Err(format!("{label} contains a non-finite number"));
                }
            }
            Ok(value.clone())
        }
        Value::String(_) | Value::Bool(_) | Value::Null => Ok(value.clone()),
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(sorted_deep(item, depth + 1, label)?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(fields) => {
            let mut keys: Vec<&String> = fields.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for key in keys {
                out.insert(key.clone(), sorted_deep(&fields[key], depth + 1, label)?);
            }
            Ok(Value::Object(out))
        }
    }
}

/// сериализует JSON без пробелов, с отсортированными ключами и без экранирования Unicode
pub(crate) fn canonical_json_bytes(value: &Value, label: &str) -> Result<Vec<u8>, String> {
    let sorted = sorted_deep(value, 0, label)?;
    serde_json::to_vec(&sorted).map_err(|_| format!("{label} cannot be encoded as canonical JSON"))
}

/// проверяет, что объект можно привести к каноническому JSON и разобрать обратно
pub(crate) fn copy_json_object(value: &Value, label: &str) -> Result<Map<String, Value>, String> {
    let Some(fields) = value.as_object() else {
        return Err(format!("{label} must be a JSON object"));
    };
    let copied: Value = Value::Object(fields.clone());
    let encoded = canonical_json_bytes(&copied, label)?;
    let parsed: Value = serde_json::from_slice(&encoded)
        .map_err(|_| format!("{label} is not valid JSON data"))?;
    match parsed {
        Value::Object(map) => Ok(map),
        _ => Err(format!("{label} must be a JSON object")),
    }
}

fn parse_ed25519_private_pem(pem: &[u8]) -> Result<[u8; 32], String> {
    let text = std::str::from_utf8(pem).map_err(|_| "private_key_pem must be an unencrypted Ed25519 PEM".to_owned())?;
    let mut base64_body = String::new();
    let mut in_body = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("-----BEGIN") {
            in_body = true;
            continue;
        }
        if line.starts_with("-----END") {
            break;
        }
        if in_body {
            base64_body.push_str(line);
        }
    }
    if base64_body.is_empty() {
        return Err("private_key_pem must be an unencrypted Ed25519 private key".to_owned());
    }
    let der = base64::engine::general_purpose::STANDARD
        .decode(base64_body.as_bytes())
        .map_err(|_| "private_key_pem must be an unencrypted Ed25519 private key".to_owned())?;
    // формат PKCS8 Ed25519: SEQUENCE { version 0, AlgId ed25519, OCTET STRING { OCTET STRING seed } }
    const PREFIX: &[u8] = &[
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
    ];
    if der.len() != PREFIX.len() + 32 || der[..PREFIX.len()] != *PREFIX {
        return Err("private_key_pem must contain an Ed25519 private key".to_owned());
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&der[PREFIX.len()..]);
    Ok(seed)
}

fn verify_ed25519(public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), String> {
    let mut key_bytes = [0u8; 32];
    key_bytes.copy_from_slice(public_key);
    let verifying_key = VerifyingKey::from_bytes(&key_bytes)
        .map_err(|_| "Plugin signature is invalid".to_owned())?;
    let mut signature_bytes = [0u8; 64];
    signature_bytes.copy_from_slice(signature);
    let signature = Signature::from_bytes(&signature_bytes);
    verifying_key
        .verify(message, &signature)
        .map_err(|_| "Plugin signature is invalid".to_owned())
}

fn normalize_file_bytes(files: &std::collections::BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    if files.len() > MAX_PACKAGE_FILES {
        return Err(format!("Plugin packages may contain at most {MAX_PACKAGE_FILES} files"));
    }
    for path in files.keys() {
        package_path_valid(path)?;
    }
    Ok(())
}

fn validate_integrity(manifest: &Map<String, Value>, files: &std::collections::BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let Some(integrity) = manifest.get("integrity") else {
        return Err("Plugin manifest integrity must contain exactly a files object".to_owned());
    };
    let Some(fields) = integrity.as_object() else {
        return Err("Plugin manifest integrity must contain exactly a files object".to_owned());
    };
    if fields.len() != 1 || !fields.contains_key("files") {
        return Err("Plugin manifest integrity must contain exactly a files object".to_owned());
    }
    let Some(expected_raw) = fields.get("files").and_then(Value::as_object) else {
        return Err("Plugin manifest integrity.files must be an object".to_owned());
    };
    if expected_raw.len() > MAX_PACKAGE_FILES {
        return Err(format!("Plugin packages may contain at most {MAX_PACKAGE_FILES} files"));
    }
    let mut expected: Vec<(String, String)> = Vec::new();
    for (path, digest) in expected_raw {
        package_path_valid(path)?;
        let Some(digest) = digest.as_str() else {
            return Err(format!("Plugin SHA-256 hashes must be 64 lowercase hexadecimal characters: {path}"));
        };
        if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)) {
            return Err(format!("Plugin SHA-256 hashes must be 64 lowercase hexadecimal characters: {path}"));
        }
        expected.push((path.clone(), digest.to_owned()));
    }

    if files.len() > MAX_PACKAGE_FILES {
        return Err(format!("Plugin packages may contain at most {MAX_PACKAGE_FILES} files"));
    }
    let mut actual_hashes: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (path, content) in files {
        package_path_valid(path)?;
        actual_hashes.insert(path.clone(), sha256_hex(content));
    }
    let expected_paths: HashSet<&String> = expected.iter().map(|(path, _)| path).collect();
    let actual_paths: HashSet<&String> = actual_hashes.keys().collect();
    if expected_paths != actual_paths {
        return Err("Plugin integrity.files must exactly cover all package files except plugin.json".to_owned());
    }
    for (path, content) in actual_hashes {
        let digest = expected.iter().find(|(candidate, _)| *candidate == path).map(|(_, digest)| digest.clone());
        if digest.as_deref() != Some(content.as_str()) {
            return Err(format!("Plugin file hash mismatch: {path}"));
        }
    }
    Ok(())
}

fn parse_signature(signature: &Value, label: &str) -> Result<(String, Vec<u8>, Vec<u8>), String> {
    let Some(fields) = signature.as_object() else {
        return Err(format!("{label} signature must contain exactly algorithm, key_id, public_key, and value"));
    };
    if fields.len() != 4
        || !fields.contains_key("algorithm")
        || !fields.contains_key("key_id")
        || !fields.contains_key("public_key")
        || !fields.contains_key("value")
    {
        return Err(format!("{label} signature must contain exactly algorithm, key_id, public_key, and value"));
    }
    if fields.get("algorithm").and_then(Value::as_str) != Some("ed25519") {
        return Err(format!("{label} signature algorithm must be ed25519"));
    }
    let key_id = fields
        .get("key_id")
        .and_then(Value::as_str)
        .filter(|value| identifier_valid(value))
        .ok_or_else(|| format!("{label} signature key_id must be a non-empty ASCII identifier"))?
        .to_owned();
    let public_key = decode_base64(
        fields.get("public_key").and_then(Value::as_str).unwrap_or(""),
        32,
        &format!("{label} public_key"),
    )?;
    let signature_value = decode_base64(
        fields.get("value").and_then(Value::as_str).unwrap_or(""),
        64,
        &format!("{label} signature value"),
    )?;
    Ok((key_id, public_key, signature_value))
}

pub(crate) fn sign_manifest(
    manifest: &Value,
    files: &std::collections::BTreeMap<String, Vec<u8>>,
    private_key_pem: &[u8],
) -> Result<Value, String> {
    let Some(source) = manifest.as_object() else {
        return Err("Plugin manifest must be a JSON object".to_owned());
    };
    let key_id_hint = source
        .get("signature")
        .and_then(Value::as_object)
        .and_then(|fields| fields.get("key_id"))
        .and_then(Value::as_str)
        .filter(|value| identifier_valid(value))
        .map(str::to_owned);

    let mut unsigned = source.clone();
    unsigned.remove("signature");
    let mut unsigned_map = copy_json_object(&Value::Object(unsigned), "Plugin manifest")?;
    normalize_file_bytes(files)?;
    let mut hashes: Map<String, Value> = Map::new();
    for (path, content) in files {
        hashes.insert(path.clone(), json!(sha256_hex(content)));
    }
    unsigned_map.insert("integrity".to_owned(), json!({ "files": hashes }));

    if private_key_pem.is_empty() || private_key_pem.len() > 1024 * 1024 {
        return Err("private_key_pem must be an unencrypted Ed25519 PEM".to_owned());
    }
    let seed = parse_ed25519_private_pem(private_key_pem)?;
    let signing_key = SigningKey::from_bytes(&seed);
    let public_key = signing_key.verifying_key().to_bytes();
    let key_id = key_id_hint.unwrap_or_else(|| fingerprint(&public_key));
    let canonical = canonical_json_bytes(&Value::Object(unsigned_map.clone()), "Plugin manifest")?;
    let signature_value = signing_key.sign(&canonical).to_bytes();
    unsigned_map.insert(
        "signature".to_owned(),
        json!({
            "algorithm": "ed25519",
            "key_id": key_id,
            "public_key": base64::engine::general_purpose::STANDARD.encode(public_key),
            "value": base64::engine::general_purpose::STANDARD.encode(signature_value),
        }),
    );
    Ok(Value::Object(unsigned_map))
}

pub(crate) fn verify_manifest(manifest: &Value, files: &std::collections::BTreeMap<String, Vec<u8>>) -> Value {
    let signed = manifest.as_object().map(|fields| fields.contains_key("signature")).unwrap_or(false);
    let mut result = json!({
        "signed": signed,
        "valid": false,
        "key_id": null,
        "public_key": null,
        "fingerprint": null,
        "error": null,
    });
    let outcome = (|| -> Result<(), String> {
        let manifest_copy = copy_json_object(manifest, "Plugin manifest")?;
        if !manifest_copy.contains_key("signature") {
            return Err("Plugin manifest is unsigned".to_owned());
        }
        let (key_id, public_key, signature_value) =
            parse_signature(manifest_copy.get("signature").unwrap(), "Plugin manifest")?;
        result["key_id"] = json!(key_id);
        result["public_key"] = json!(base64::engine::general_purpose::STANDARD.encode(&public_key));
        result["fingerprint"] = json!(fingerprint(&public_key));
        validate_integrity(&manifest_copy, files)?;
        let mut unsigned = manifest_copy;
        unsigned.remove("signature");
        let canonical = canonical_json_bytes(&Value::Object(unsigned), "Plugin manifest")?;
        verify_ed25519(&public_key, &canonical, &signature_value)?;
        result["valid"] = json!(true);
        Ok(())
    })();
    if let Err(message) = outcome {
        result["error"] = json!(message);
    }
    result
}

fn scan_json_no_duplicate_keys(text: &str) -> Result<(), String> {
    let mut scanner = JsonScanner { bytes: text.as_bytes(), position: 0 };
    scanner.skip_whitespace();
    scanner.value(0)?;
    scanner.skip_whitespace();
    if scanner.position != scanner.bytes.len() {
        return Err("trailing data".to_owned());
    }
    Ok(())
}

struct JsonScanner<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> JsonScanner<'a> {
    fn skip_whitespace(&mut self) {
        while self.position < self.bytes.len() && matches!(self.bytes[self.position], b' ' | b'\t' | b'\n' | b'\r') {
            self.position += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn value(&mut self, depth: usize) -> Result<(), String> {
        if depth > 512 {
            return Err("nested too deeply".to_owned());
        }
        match self.peek().ok_or("unexpected end of JSON")? {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => self.string().map(|_| ()),
            b't' => self.literal("true"),
            b'f' => self.literal("false"),
            b'n' => self.literal("null"),
            b'-' | b'0'..=b'9' => self.number(),
            other => Err(format!("unexpected byte {other}")),
        }
    }

    fn literal(&mut self, expected: &str) -> Result<(), String> {
        if self.bytes[self.position..].starts_with(expected.as_bytes()) {
            self.position += expected.len();
            Ok(())
        } else {
            Err("bad literal".to_owned())
        }
    }

    fn number(&mut self) -> Result<(), String> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        while matches!(self.peek(), Some(b'0'..=b'9') | Some(b'.') | Some(b'e') | Some(b'E') | Some(b'+') | Some(b'-')) {
            self.position += 1;
        }
        if self.position == start {
            return Err("bad number".to_owned());
        }
        Ok(())
    }

    fn string(&mut self) -> Result<String, String> {
        self.position += 1; // открывающая кавычка
        let mut out = String::new();
        loop {
            let byte = self.peek().ok_or("unterminated string")?;
            self.position += 1;
            match byte {
                b'"' => return Ok(out),
                b'\\' => {
                    let escape = self.peek().ok_or("unterminated escape")?;
                    self.position += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = self
                                .bytes
                                .get(self.position..self.position + 4)
                                .ok_or("bad unicode escape")?;
                            let text = std::str::from_utf8(hex).map_err(|_| "bad unicode escape")?;
                            let code = u16::from_str_radix(text, 16).map_err(|_| "bad unicode escape")?;
                            self.position += 4;
                            out.push(char::from_u32(code as u32).unwrap_or('\u{FFFD}'));
                        }
                        other => return Err(format!("bad escape {other}")),
                    }
                }
                other if other < 0x20 => return Err("control character in string".to_owned()),
                other => {
                    // многобайтовый UTF-8 копируем целиком
                    let width = utf8_width(other);
                    let start = self.position - 1;
                    self.position = start + width;
                    let chunk = self.bytes.get(start..self.position).ok_or("bad utf-8")?;
                    out.push_str(std::str::from_utf8(chunk).map_err(|_| "bad utf-8")?);
                }
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<(), String> {
        self.position += 1; // '{'
        let mut keys: HashSet<String> = HashSet::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(());
        }
        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return Err("expected object key".to_owned());
            }
            let key = self.string()?;
            if !keys.insert(key) {
                return Err("duplicate JSON key".to_owned());
            }
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return Err("expected ':'".to_owned());
            }
            self.position += 1;
            self.skip_whitespace();
            self.value(depth + 1)?;
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b'}') => {
                    self.position += 1;
                    return Ok(());
                }
                _ => return Err("expected ',' or '}'".to_owned()),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<(), String> {
        self.position += 1; // '['
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(());
        }
        loop {
            self.skip_whitespace();
            self.value(depth + 1)?;
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b']') => {
                    self.position += 1;
                    return Ok(());
                }
                _ => return Err("expected ',' or ']'".to_owned()),
            }
        }
    }
}

fn utf8_width(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if byte >> 5 == 0b110 {
        2
    } else if byte >> 4 == 0b1110 {
        3
    } else if byte >> 3 == 0b11110 {
        4
    } else {
        1
    }
}

/// отклоняет повторяющиеся ключи в JSON
pub(crate) fn parse_strict_json(raw: &[u8], label: &str) -> Result<Value, String> {
    let text = std::str::from_utf8(raw).map_err(|_| format!("{label} contains invalid JSON"))?;
    scan_json_no_duplicate_keys(text).map_err(|_| format!("{label} contains invalid JSON"))?;
    serde_json::from_str(text).map_err(|_| format!("{label} contains invalid JSON"))
}

fn initial_store_state() -> Value {
    json!({
        "schema_version": STORE_SCHEMA_VERSION,
        "publishers": {},
        "local_revoked_plugins": {},
        "local_revoked_publishers": {},
        "imported_revocations": {
            "sequence": -1,
            "authority_key_id": "",
            "revoked_plugins": {},
            "revoked_publishers": {},
        },
    })
}

fn validate_reason_map(value: &Value, label: &str, publisher_ids: bool) -> Result<(), String> {
    let Some(fields) = value.as_object() else {
        return Err(format!("Plugin trust store {label} must be an object"));
    };
    for (identifier, reason) in fields {
        let id_label = if publisher_ids { "Publisher key ID" } else { "Plugin ID" };
        if !identifier_valid(identifier) {
            return Err(format!("{id_label} must be a non-empty ASCII identifier"));
        }
        let Some(reason) = reason.as_str() else {
            return Err(format!("Plugin trust store {label} reason must be text of at most {MAX_REASON_LENGTH} characters"));
        };
        if !reason_valid(reason) {
            return Err(format!("Plugin trust store {label} reason must be text of at most {MAX_REASON_LENGTH} characters"));
        }
    }
    Ok(())
}

fn validate_store_state(value: &Value) -> Result<(), String> {
    let Some(fields) = value.as_object() else {
        return Err("Plugin trust store has an unsupported or invalid format".to_owned());
    };
    let required = ["schema_version", "publishers", "local_revoked_plugins", "local_revoked_publishers", "imported_revocations"];
    if fields.len() != required.len() || required.iter().any(|key| !fields.contains_key(*key)) {
        return Err("Plugin trust store has an unsupported or invalid format".to_owned());
    }
    if fields.get("schema_version").and_then(Value::as_i64) != Some(STORE_SCHEMA_VERSION) {
        return Err("Plugin trust store has an unsupported schema version".to_owned());
    }
    let Some(publishers) = fields.get("publishers").and_then(Value::as_object) else {
        return Err("Plugin trust store publishers must be an object".to_owned());
    };
    let mut seen_public_keys: HashSet<Vec<u8>> = HashSet::new();
    for (key_id, publisher) in publishers {
        if !identifier_valid(key_id) {
            return Err("Publisher key ID must be a non-empty ASCII identifier".to_owned());
        }
        let Some(record) = publisher.as_object() else {
            return Err("Plugin trust store contains an invalid publisher record".to_owned());
        };
        if record.len() != 2 || !record.contains_key("public_key") || !record.contains_key("revocation_authority") {
            return Err("Plugin trust store contains an invalid publisher record".to_owned());
        }
        let public_key = decode_base64(
            record.get("public_key").and_then(Value::as_str).unwrap_or(""),
            32,
            "Pinned public key",
        )?;
        if record.get("revocation_authority").and_then(Value::as_bool).is_none() {
            return Err("Plugin trust store revocation_authority flag must be boolean".to_owned());
        }
        if seen_public_keys.contains(&public_key) {
            return Err("Plugin trust store pins one public key under multiple key IDs".to_owned());
        }
        seen_public_keys.insert(public_key);
    }

    validate_reason_map(fields.get("local_revoked_plugins").unwrap_or(&Value::Null), "local_revoked_plugins", false)?;
    validate_reason_map(fields.get("local_revoked_publishers").unwrap_or(&Value::Null), "local_revoked_publishers", true)?;
    let Some(imported) = fields.get("imported_revocations").and_then(Value::as_object) else {
        return Err("Plugin trust store imported revocations are invalid".to_owned());
    };
    let imported_keys = ["sequence", "authority_key_id", "revoked_plugins", "revoked_publishers"];
    if imported.len() != imported_keys.len() || imported_keys.iter().any(|key| !imported.contains_key(*key)) {
        return Err("Plugin trust store imported revocations are invalid".to_owned());
    }
    let sequence = imported.get("sequence").and_then(Value::as_i64);
    let sequence_valid = match sequence {
        Some(value) if value >= -1 => true,
        _ => false,
    };
    if !sequence_valid {
        return Err("Plugin trust store revocation sequence is invalid".to_owned());
    }
    let authority_key_id = imported.get("authority_key_id").and_then(Value::as_str).unwrap_or("");
    if sequence == Some(-1) {
        if !authority_key_id.is_empty() {
            return Err("Plugin trust store has an authority without imported revocations".to_owned());
        }
    } else if !identifier_valid(authority_key_id) {
        return Err("Revocation authority key ID must be a non-empty ASCII identifier".to_owned());
    }
    validate_reason_map(imported.get("revoked_plugins").unwrap_or(&Value::Null), "imported revoked_plugins", false)?;
    validate_reason_map(imported.get("revoked_publishers").unwrap_or(&Value::Null), "imported revoked_publishers", true)?;
    if sequence == Some(-1)
        && (imported
            .get("revoked_plugins")
            .and_then(Value::as_object)
            .map(|fields| !fields.is_empty())
            .unwrap_or(false)
            || imported
                .get("revoked_publishers")
                .and_then(Value::as_object)
                .map(|fields| !fields.is_empty())
                .unwrap_or(false))
    {
        return Err("Plugin trust store has revocations without an imported sequence".to_owned());
    }
    Ok(())
}

pub(crate) struct PluginTrustStore {
    root: PathBuf,
    path: PathBuf,
    state: Mutex<Value>,
}

impl PluginTrustStore {
    pub(crate) fn new(root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root)
            .and_then(|_| {
                let metadata = fs::symlink_metadata(root)?;
                if !metadata.is_dir() {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a directory"));
                }
                Ok(())
            })
            .map_err(|_| "Could not create a private plugin trust store directory".to_owned())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(root, fs::Permissions::from_mode(0o700));
        }
        let path = root.join(STORE_FILENAME);
        let state = load_store_state(&path)?;
        Ok(Self {
            root: root.to_path_buf(),
            path,
            state: Mutex::new(state),
        })
    }

    fn write_state(&self, state: &Value) -> Result<(), String> {
        validate_store_state(state)?;
        let sorted = sorted_deep(state, 0, "Plugin trust store").map_err(|_| "Plugin trust store data cannot be serialized".to_owned())?;
        let pretty = serde_json::to_string_pretty(&sorted)
            .map_err(|_| "Plugin trust store data cannot be serialized".to_owned())?;
        let mut payload = pretty.into_bytes();
        payload.push(b'\n');
        if payload.len() > MAX_STORE_BYTES {
            return Err("Plugin trust store would exceed its maximum size".to_owned());
        }

        let mut temporary: Option<PathBuf> = None;
        for _ in 0..8 {
            let candidate = self.root.join(format!(".{STORE_FILENAME}.{}.tmp", random_hex(8)));
            match create_private_file(&candidate) {
                Ok(mut file) => {
                    if let Err(error) = file.write_all(&payload).and_then(|_| file.sync_all()) {
                        let _ = fs::remove_file(&candidate);
                        return Err(format!("Could not atomically save the plugin trust store: {error}"));
                    }
                    temporary = Some(candidate);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err("Could not create a private temporary plugin trust store".to_owned()),
            }
        }
        let Some(temporary) = temporary else {
            return Err("Could not allocate a temporary plugin trust store file".to_owned());
        };
        if fs::rename(&temporary, &self.path).is_err() {
            let _ = fs::remove_file(&temporary);
            return Err("Could not atomically save the plugin trust store".to_owned());
        }
        sync_directory(&self.root);
        Ok(())
    }

    fn commit(&self, state: Value) -> Result<(), String> {
        let mut current = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = current.clone();
        *current = state;
        if let Err(message) = self.write_state(&current) {
            *current = previous;
            return Err(message);
        }
        Ok(())
    }

    fn state_snapshot(&self) -> Value {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    pub(crate) fn pin_publisher_key(
        &self,
        key_id: &str,
        public_key: &Value,
        revocation_authority: bool,
    ) -> Result<(), String> {
        if !identifier_valid(key_id) {
            return Err("Publisher key ID must be a non-empty ASCII identifier".to_owned());
        }
        let raw_key = coerce_public_key(public_key, "Public key")?;
        let encoded_key = base64::engine::general_purpose::STANDARD.encode(&raw_key);
        let state = self.state_snapshot();
        let publishers = state.get("publishers").and_then(Value::as_object).cloned().unwrap_or_default();
        if let Some(current) = publishers.get(key_id) {
            let existing_key = decode_base64(
                current.get("public_key").and_then(Value::as_str).unwrap_or(""),
                32,
                "Pinned public key",
            )?;
            if existing_key != raw_key {
                return Err("Publisher key ID is already pinned to another public key; unpin it first".to_owned());
            }
            if revocation_authority && current.get("revocation_authority").and_then(Value::as_bool) != Some(true) {
                let mut updated = state.clone();
                updated["publishers"][key_id]["revocation_authority"] = json!(true);
                return self.commit(updated);
            }
            return Ok(());
        }
        for record in publishers.values() {
            let pinned = decode_base64(record.get("public_key").and_then(Value::as_str).unwrap_or(""), 32, "Pinned public key")?;
            if pinned == raw_key {
                return Err("Publisher public key is already pinned under a different key ID".to_owned());
            }
        }
        let mut updated = state.clone();
        updated["publishers"][key_id] = json!({
            "public_key": encoded_key,
            "revocation_authority": revocation_authority,
        });
        self.commit(updated)
    }

    pub(crate) fn unpin_publisher_key(&self, key_id: &str) -> Result<bool, String> {
        if !identifier_valid(key_id) {
            return Err("Publisher key ID must be a non-empty ASCII identifier".to_owned());
        }
        let state = self.state_snapshot();
        if !state.get("publishers").and_then(Value::as_object).map(|fields| fields.contains_key(key_id)).unwrap_or(false) {
            return Ok(false);
        }
        let mut updated = state.clone();
        updated["publishers"].as_object_mut().unwrap().remove(key_id);
        self.commit(updated)?;
        Ok(true)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_publisher_pinned(&self, key_id: &str, public_key: Option<&Value>) -> Result<bool, String> {
        if !identifier_valid(key_id) {
            return Err("Publisher key ID must be a non-empty ASCII identifier".to_owned());
        }
        let raw_key = match public_key {
            Some(value) => Some(coerce_public_key(value, "Public key")?),
            None => None,
        };
        let state = self.state_snapshot();
        let record = state.get("publishers").and_then(Value::as_object).and_then(|fields| fields.get(key_id));
        let Some(record) = record else { return Ok(false) };
        match raw_key {
            None => Ok(true),
            Some(raw_key) => {
                let stored = decode_base64(record.get("public_key").and_then(Value::as_str).unwrap_or(""), 32, "Pinned public key")?;
                Ok(stored == raw_key)
            }
        }
    }

    pub(crate) fn publisher_status(&self, key_id: &str, public_key: Option<&Value>) -> Result<Value, String> {
        if !identifier_valid(key_id) {
            return Err("Publisher key ID must be a non-empty ASCII identifier".to_owned());
        }
        let raw_key = match public_key {
            Some(value) => Some(coerce_public_key(value, "Public key")?),
            None => None,
        };
        let state = self.state_snapshot();
        let record = state.get("publishers").and_then(Value::as_object).and_then(|fields| fields.get(key_id)).cloned();
        let mut pinned = record.is_some();
        if let (Some(record), Some(raw_key)) = (record.as_ref(), raw_key.as_ref()) {
            let stored = decode_base64(record.get("public_key").and_then(Value::as_str).unwrap_or(""), 32, "Pinned public key")?;
            pinned = stored == *raw_key;
        }
        let revocation = self.publisher_revocation_status(&state, key_id);
        Ok(json!({
            "key_id": key_id,
            "pinned": pinned,
            "trusted": pinned && !revocation["revoked"].as_bool().unwrap_or(false),
            "revoked": revocation["revoked"],
            "reason": revocation["reason"],
            "revocation_authority": record
                .as_ref()
                .and_then(|record| record.get("revocation_authority"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }))
    }

    pub(crate) fn set_revocation_authority(&self, key_id: &str, enabled: bool) -> Result<(), String> {
        if !identifier_valid(key_id) {
            return Err("Publisher key ID must be a non-empty ASCII identifier".to_owned());
        }
        let state = self.state_snapshot();
        let record = state.get("publishers").and_then(Value::as_object).and_then(|fields| fields.get(key_id));
        let Some(record) = record else {
            return Err("A publisher key must be pinned before it can be a revocation authority".to_owned());
        };
        if record.get("revocation_authority").and_then(Value::as_bool) == Some(enabled) {
            return Ok(());
        }
        let mut updated = state.clone();
        updated["publishers"][key_id]["revocation_authority"] = json!(enabled);
        self.commit(updated)
    }

    pub(crate) fn revoke_plugin(&self, plugin_id: &str, reason: &str) -> Result<(), String> {
        if !identifier_valid(plugin_id) {
            return Err("Plugin ID must be a non-empty ASCII identifier".to_owned());
        }
        if !reason_valid(reason) {
            return Err(format!("Revocation reason must be text of at most {MAX_REASON_LENGTH} characters"));
        }
        let state = self.state_snapshot();
        if state.get("local_revoked_plugins").and_then(Value::as_object).and_then(|fields| fields.get(plugin_id)).and_then(Value::as_str) == Some(reason) {
            return Ok(());
        }
        let mut updated = state.clone();
        updated["local_revoked_plugins"][plugin_id] = json!(reason);
        self.commit(updated)
    }

    pub(crate) fn unrevoke_plugin(&self, plugin_id: &str) -> Result<bool, String> {
        if !identifier_valid(plugin_id) {
            return Err("Plugin ID must be a non-empty ASCII identifier".to_owned());
        }
        let state = self.state_snapshot();
        if !state
            .get("local_revoked_plugins")
            .and_then(Value::as_object)
            .map(|fields| fields.contains_key(plugin_id))
            .unwrap_or(false)
        {
            return Ok(false);
        }
        let mut updated = state.clone();
        updated["local_revoked_plugins"].as_object_mut().unwrap().remove(plugin_id);
        self.commit(updated)?;
        Ok(true)
    }

    pub(crate) fn is_plugin_revoked(&self, plugin_id: &str) -> Result<bool, String> {
        if !identifier_valid(plugin_id) {
            return Err("Plugin ID must be a non-empty ASCII identifier".to_owned());
        }
        let state = self.state_snapshot();
        Ok(self.plugin_revocation_status(&state, plugin_id)["revoked"].as_bool().unwrap_or(false))
    }

    pub(crate) fn plugin_revocation_reason(&self, plugin_id: &str) -> Result<Option<String>, String> {
        if !identifier_valid(plugin_id) {
            return Err("Plugin ID must be a non-empty ASCII identifier".to_owned());
        }
        let state = self.state_snapshot();
        let reason = self.plugin_revocation_status(&state, plugin_id)["reason"].clone();
        Ok(reason.as_str().map(str::to_owned))
    }

    pub(crate) fn is_trusted_publisher(&self, key_id: &str, public_key: Option<&Value>) -> Result<bool, String> {
        if !identifier_valid(key_id) {
            return Err("Publisher key ID must be a non-empty ASCII identifier".to_owned());
        }
        let state = self.state_snapshot();
        let record = state
            .get("publishers")
            .and_then(Value::as_object)
            .and_then(|fields| fields.get(key_id));
        let Some(record) = record else { return Ok(false) };
        if let Some(value) = public_key {
            let raw_key = coerce_public_key(value, "Public key")?;
            let stored = decode_base64(record.get("public_key").and_then(Value::as_str).unwrap_or(""), 32, "Pinned public key")?;
            if stored != raw_key {
                return Ok(false);
            }
        }
        Ok(!self.publisher_revocation_status(&state, key_id)["revoked"].as_bool().unwrap_or(false))
    }

    fn plugin_revocation_status(&self, state: &Value, plugin_id: &str) -> Value {
        revocation_status(
            state.pointer("/local_revoked_plugins"),
            state.pointer("/imported_revocations/revoked_plugins"),
            plugin_id,
        )
    }

    fn publisher_revocation_status(&self, state: &Value, key_id: &str) -> Value {
        revocation_status(
            state.pointer("/local_revoked_publishers"),
            state.pointer("/imported_revocations/revoked_publishers"),
            key_id,
        )
    }

    pub(crate) fn import_revocations_json(&self, content: &str) -> Result<Value, String> {
        if content.len() > MAX_REVOCATION_DOCUMENT_BYTES {
            return Err("Revocation document exceeds the 1 MiB limit".to_owned());
        }
        let document = parse_strict_json(content.as_bytes(), "Revocation document")
            .map_err(|message| format!("Revocation document is not valid JSON: {message}"))?;
        if !document.is_object() {
            return Err("Revocation document must be a JSON object".to_owned());
        }
        self.import_revocations(&document)
    }

    pub(crate) fn import_revocations(&self, document: &Value) -> Result<Value, String> {
        let copied = copy_json_object(document, "Revocation document")?;
        let required = ["schema_version", "sequence", "revoked_plugins", "revoked_publishers", "signature"];
        if copied.len() != required.len() || required.iter().any(|key| !copied.contains_key(*key)) {
            return Err("Revocation document must contain exactly schema_version, sequence, revoked_plugins, revoked_publishers, and signature".to_owned());
        }
        if copied.get("schema_version").and_then(Value::as_i64) != Some(1) {
            return Err("Revocation document schema_version must be 1".to_owned());
        }
        let Some(sequence) = copied.get("sequence").and_then(Value::as_i64).filter(|value| *value >= 0) else {
            return Err("Revocation document sequence must be a non-negative integer".to_owned());
        };
        let revoked_plugins = parse_revocation_entries(copied.get("revoked_plugins"), "revoked_plugins", "id", false)?;
        let revoked_publishers =
            parse_revocation_entries(copied.get("revoked_publishers"), "revoked_publishers", "key_id", true)?;
        let (authority_key_id, document_public_key, signature_value) =
            parse_signature(copied.get("signature").unwrap(), "Revocation document")?;
        let mut unsigned = copied.clone();
        unsigned.remove("signature");
        let canonical = canonical_json_bytes(&Value::Object(unsigned), "Revocation document")?;

        let state = self.state_snapshot();
        let authority = state
            .get("publishers")
            .and_then(Value::as_object)
            .and_then(|fields| fields.get(&authority_key_id))
            .cloned();
        let Some(authority) = authority else {
            return Err("Revocation document was signed by an unknown authority key".to_owned());
        };
        if authority.get("revocation_authority").and_then(Value::as_bool) != Some(true) {
            return Err("Revocation document signer is not pinned as a revocation authority".to_owned());
        }
        if self.publisher_revocation_status(&state, &authority_key_id)["revoked"].as_bool().unwrap_or(false) {
            return Err("Revocation document signer is revoked".to_owned());
        }
        let pinned_key = decode_base64(
            authority.get("public_key").and_then(Value::as_str).unwrap_or(""),
            32,
            "Pinned authority public key",
        )?;
        if pinned_key != document_public_key {
            return Err("Revocation document public key does not match the pinned authority".to_owned());
        }
        verify_ed25519(&pinned_key, &canonical, &signature_value)
            .map_err(|_| "Revocation document signature is invalid".to_owned())?;

        let previous_sequence = state
            .pointer("/imported_revocations/sequence")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        if sequence <= previous_sequence {
            return Err("Revocation document sequence must be greater than the last imported sequence".to_owned());
        }
        let mut updated = state.clone();
        updated["imported_revocations"] = json!({
            "sequence": sequence,
            "authority_key_id": authority_key_id,
            "revoked_plugins": revoked_plugins,
            "revoked_publishers": revoked_publishers,
        });
        self.commit(updated)?;

        let mut sorted_plugins: Vec<String> = revoked_plugins.keys().cloned().collect();
        sorted_plugins.sort();
        let mut sorted_publishers: Vec<String> = revoked_publishers.keys().cloned().collect();
        sorted_publishers.sort();
        Ok(json!({
            "sequence": sequence,
            "authority_key_id": authority_key_id,
            "revoked_plugins": sorted_plugins,
            "revoked_publishers": sorted_publishers,
        }))
    }
}

fn revocation_status(local: Option<&Value>, imported: Option<&Value>, identifier: &str) -> Value {
    let local_fields = local.and_then(Value::as_object);
    let imported_fields = imported.and_then(Value::as_object);
    let local_reason = local_fields.and_then(|fields| fields.get(identifier)).and_then(Value::as_str);
    let imported_reason = imported_fields.and_then(|fields| fields.get(identifier)).and_then(Value::as_str);
    let local_revoked = local_reason.is_some();
    let external_revoked = imported_reason.is_some();
    let reason = local_reason.or(imported_reason);
    json!({
        "revoked": local_revoked || external_revoked,
        "reason": reason,
        "local": local_revoked,
        "external": external_revoked,
    })
}

fn parse_revocation_entries(
    value: Option<&Value>,
    label: &str,
    id_field: &str,
    publisher_ids: bool,
) -> Result<Map<String, Value>, String> {
    let Some(entries) = value.and_then(Value::as_array) else {
        return Err(format!("Revocation document {label} must be an array"));
    };
    let mut result = Map::new();
    for entry in entries {
        let Some(fields) = entry.as_object() else {
            return Err(format!("Each {label} entry must contain exactly {id_field} and reason"));
        };
        if fields.len() != 2 || !fields.contains_key(id_field) || !fields.contains_key("reason") {
            return Err(format!("Each {label} entry must contain exactly {id_field} and reason"));
        }
        let id_label = if publisher_ids { "Publisher key ID" } else { "Plugin ID" };
        let identifier = fields
            .get(id_field)
            .and_then(Value::as_str)
            .filter(|value| identifier_valid(value))
            .ok_or_else(|| format!("{id_label} must be a non-empty ASCII identifier"))?
            .to_owned();
        if result.contains_key(&identifier) {
            return Err(format!("Revocation document contains a duplicate {id_field}"));
        }
        let reason = fields
            .get("reason")
            .and_then(Value::as_str)
            .filter(|value| reason_valid(value))
            .ok_or_else(|| {
                format!("Revocation reason must be text of at most {MAX_REASON_LENGTH} characters")
            })?
            .to_owned();
        result.insert(identifier, json!(reason));
    }
    Ok(result)
}

fn load_store_state(path: &Path) -> Result<Value, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(initial_store_state()),
        Err(_) => return Err("Could not inspect the plugin trust store file".to_owned()),
    };
    if !metadata.is_file() {
        return Err("Plugin trust store must be a regular file, not a symlink".to_owned());
    }
    if metadata.len() as usize > MAX_STORE_BYTES {
        return Err("Plugin trust store file is too large".to_owned());
    }
    let raw = read_private_file(path, MAX_STORE_BYTES)?;
    let raw = match raw {
        Some(raw) => raw,
        None => return Ok(initial_store_state()),
    };
    if raw.len() > MAX_STORE_BYTES {
        return Err("Plugin trust store file is too large".to_owned());
    }
    let state = parse_strict_json(&raw, "Plugin trust store").map_err(|_| "Plugin trust store contains invalid JSON".to_owned())?;
    validate_store_state(&state)?;
    Ok(state)
}

/// чтение с O_NOFOLLOW на unix: symlink не откроется даже при гонке
fn read_private_file(path: &Path, max_bytes: usize) -> Result<Option<Vec<u8>>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::io::FromRawFd;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "Could not securely read the plugin trust store".to_owned())?;
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let fd = unsafe { libc::open(c_path.as_ptr(), flags) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            return match error.kind() {
                std::io::ErrorKind::NotFound => Ok(None),
                _ => Err("Could not securely read the plugin trust store".to_owned()),
            };
        }
        if unsafe { libc::fchmod(fd, 0o600) } != 0 {
            unsafe { libc::close(fd) };
            return Err("Could not securely read the plugin trust store".to_owned());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let mut buffer = Vec::new();
        let outcome = file.take(max_bytes as u64 + 1).read_to_end(&mut buffer);
        match outcome {
            Ok(_) if buffer.len() <= max_bytes => Ok(Some(buffer)),
            Ok(_) => Err("Plugin trust store file is too large".to_owned()),
            Err(_) => Err("Could not securely read the plugin trust store".to_owned()),
        }
    }
    #[cfg(not(unix))]
    {
        if fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(true) {
            return Err("Could not securely read the plugin trust store".to_owned());
        }
        match fs::read(path) {
            Ok(buffer) if buffer.len() <= max_bytes => Ok(Some(buffer)),
            Ok(_) => Err("Plugin trust store file is too large".to_owned()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("Could not securely read the plugin trust store".to_owned()),
        }
    }
}

fn create_private_file(path: &Path) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        fs::OpenOptions::new().write(true).create_new(true).open(path)
    }
}

fn sync_directory(root: &Path) {
    #[cfg(unix)]
    unsafe {
        use std::os::unix::ffi::OsStrExt;
        if let Ok(c_path) = std::ffi::CString::new(root.as_os_str().as_bytes()) {
            let fd = libc::open(c_path.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY);
            if fd >= 0 {
                libc::fsync(fd);
                libc::close(fd);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = root;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-plugin-security-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("создать временный каталог");
        dir
    }

    #[test]
    fn canonical_json_sorts_and_compacts() {
        let value = json!({"b": 1, "a": {"d": [3, 2], "c": "текст"}});
        let encoded = canonical_json_bytes(&value, "test").unwrap();
        assert_eq!(
            String::from_utf8(encoded).unwrap(),
            r#"{"a":{"c":"текст","d":[3,2]},"b":1}"#
        );
    }

    #[test]
    fn duplicate_keys_are_rejected_by_scanner() {
        assert!(scan_json_no_duplicate_keys(r#"{"a":1,"b":2}"#).is_ok());
        assert!(scan_json_no_duplicate_keys(r#"{"a":1,"a":2}"#).is_err());
        assert!(scan_json_no_duplicate_keys(r#"[{"x":[{"y":1}]}]"#).is_ok());
        assert!(scan_json_no_duplicate_keys(r#"{"у":1,"у":2}"#).is_err());
        assert!(scan_json_no_duplicate_keys(r#""просто строка""#).is_ok());
        assert!(scan_json_no_duplicate_keys(r#"{"a":}{"#).is_err());
    }

    #[test]
    fn package_paths_are_strict() {
        assert!(package_path_valid("assets/icon.png").is_ok());
        assert!(package_path_valid("plugin.json").is_err());
        assert!(package_path_valid("../escape").is_err());
        assert!(package_path_valid("a//b").is_err());
        assert!(package_path_valid("C:/x").is_err());
        assert!(package_path_valid("/abs").is_err());
        assert!(package_path_valid("a\\b").is_err());
    }

    #[test]
    fn base64_must_be_canonical() {
        let encoded = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        assert_eq!(decode_base64(&encoded, 32, "key").unwrap(), vec![7u8; 32]);
        assert!(decode_base64(&format!("{encoded} "), 32, "key").is_err());
        assert!(decode_base64(&encoded[..encoded.len() - 1], 32, "key").is_err());
    }

    #[test]
    fn sign_and_verify_manifest_roundtrip() {
        let seed: [u8; 32] = core::array::from_fn(|index| index as u8);
        let signing_key = SigningKey::from_bytes(&seed);
        let pem_public = signing_key.verifying_key().to_bytes();
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
            base64::engine::general_purpose::STANDARD.encode([
                0x30u8, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
            ].iter().chain(seed.iter()).cloned().collect::<Vec<u8>>())
        );
        let mut files = std::collections::BTreeMap::new();
        files.insert("main.js".to_owned(), b"console.log('hi')".to_vec());
        let manifest = json!({"id": "demo-plugin", "name": "Demo", "api_version": "3"});
        let signed = sign_manifest(&manifest, &files, pem.as_bytes()).unwrap();
        assert_eq!(signed["signature"]["public_key"], json!(base64::engine::general_purpose::STANDARD.encode(pem_public)));

        let verified = verify_manifest(&signed, &files);
        assert_eq!(verified["valid"], json!(true));
        assert_eq!(verified["error"], json!(null));

        // подмененный файл ломает хеш, но не подпись манифеста
        let mut tampered = files.clone();
        tampered.insert("main.js".to_owned(), b"console.log('evil')".to_vec());
        let broken = verify_manifest(&signed, &tampered);
        assert_eq!(broken["valid"], json!(false));
        assert!(broken["error"].as_str().unwrap().contains("hash mismatch"));

        let unsigned = verify_manifest(&manifest, &files);
        assert_eq!(unsigned["signed"], json!(false));
        assert_eq!(unsigned["error"], json!("Plugin manifest is unsigned"));
    }

    #[test]
    fn trust_store_pins_and_revokes() {
        let root = temp_root("store");
        let store = PluginTrustStore::new(&root).unwrap();
        let seed: [u8; 32] = core::array::from_fn(|index| index as u8);
        let signing_key = SigningKey::from_bytes(&seed);
        let public_key = base64::engine::general_purpose::STANDARD.encode(signing_key.verifying_key().to_bytes());

        store.pin_publisher_key("author", &json!(public_key), false).unwrap();
        assert!(store.is_publisher_pinned("author", None).unwrap());
        assert!(!store.is_publisher_pinned("author", Some(&json!(json!("aGk=")))).is_ok_and(|value| value));

        // повторная фиксация того же ключа не меняет состояние
        store.pin_publisher_key("author", &json!(public_key), true).unwrap();
        let status = store.publisher_status("author", None).unwrap();
        assert_eq!(status["revocation_authority"], json!(true));

        assert!(store.unpin_publisher_key("author").unwrap());
        assert!(!store.unpin_publisher_key("author").unwrap());

        store.revoke_plugin("bad-plugin", "вредоносный").unwrap();
        assert!(store.is_plugin_revoked("bad-plugin").unwrap());
        assert_eq!(store.plugin_revocation_reason("bad-plugin").unwrap().as_deref(), Some("вредоносный"));
        assert!(store.unrevoke_plugin("bad-plugin").unwrap());
        assert!(!store.is_plugin_revoked("bad-plugin").unwrap());

        // перезагрузка с диска сохраняет состояние
        drop(store);
        let reopened = PluginTrustStore::new(&root).unwrap();
        assert!(reopened.unrevoke_plugin("persisted").is_ok());
        reopened.revoke_plugin("persisted", "").unwrap();
        drop(reopened);
        let reopened = PluginTrustStore::new(&root).unwrap();
        assert!(reopened.is_plugin_revoked("persisted").unwrap());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn revocation_import_requires_pinned_authority_signature() {
        let root = temp_root("revocations");
        let store = PluginTrustStore::new(&root).unwrap();
        let authority: SigningKey = SigningKey::from_bytes(&{
            let mut seed = [0u8; 32];
            seed[0] = 9;
            seed
        });
        let public_key = base64::engine::general_purpose::STANDARD.encode(authority.verifying_key().to_bytes());
        store.pin_publisher_key("authority", &json!(public_key), true).unwrap();

        let mut document = json!({
            "schema_version": 1,
            "sequence": 7,
            "revoked_plugins": [{"id": "evil", "reason": "stolen key"}],
            "revoked_publishers": [],
        });
        let mut unsigned = document.clone();
        let canonical = canonical_json_bytes(&unsigned, "doc").unwrap();
        let signature = authority.sign(&canonical).to_bytes();
        document["signature"] = json!({
            "algorithm": "ed25519",
            "key_id": "authority",
            "public_key": public_key,
            "value": base64::engine::general_purpose::STANDARD.encode(signature),
        });
        let _ = &mut unsigned;

        let imported = store.import_revocations_json(&document.to_string()).unwrap();
        assert_eq!(imported["sequence"], json!(7));
        assert!(store.is_plugin_revoked("evil").unwrap());

        // повторная вставка той же последовательности отклоняется
        assert!(store.import_revocations_json(&document.to_string()).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
