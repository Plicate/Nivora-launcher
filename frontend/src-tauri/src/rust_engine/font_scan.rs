use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const MAX_FONT_FILES: usize = 8192;
const MAX_FONT_FILE_BYTES: u64 = 96 * 1024 * 1024;
const MAX_NAME_TABLE_BYTES: usize = 256 * 1024;

fn font_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(target_os = "windows") {
        if let Some(windir) = std::env::var_os("WINDIR") {
            dirs.push(PathBuf::from(windir).join("Fonts"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"));
        }
    } else if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/System/Library/Fonts"));
        dirs.push(PathBuf::from("/Library/Fonts"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Library").join("Fonts"));
        }
    } else {
        if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
            dirs.push(PathBuf::from(data_home).join("fonts"));
        } else if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            dirs.push(home.join(".local").join("share").join("fonts"));
            dirs.push(home.join(".fonts"));
        }
        dirs.push(PathBuf::from("/usr/share/fonts"));
        dirs.push(PathBuf::from("/usr/local/share/fonts"));
    }
    dirs
}

fn is_font_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()).map(|value| value.to_ascii_lowercase()).as_deref(),
        Some("ttf" | "otf" | "ttc" | "otc")
    )
}

fn collect_font_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 8 || out.len() >= MAX_FONT_FILES {
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(value) => value,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            collect_font_files(&path, out, depth + 1);
            if out.len() >= MAX_FONT_FILES {
                return;
            }
        } else if file_type.is_file() && is_font_file(&path) {
            out.push(path);
            if out.len() >= MAX_FONT_FILES {
                return;
            }
        }
    }
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    let slice = data.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_be_bytes([slice[0], slice[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    let slice = data.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn decode_utf16be(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect();
    String::from_utf16_lossy(&units)
}

fn clean_family_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim_matches(|ch: char| ch <= ' ');
    if trimmed.is_empty() || trimmed.len() > 96 {
        return None;
    }
    if trimmed.chars().any(|ch| ch.is_control()) {
        return None;
    }
    Some(trimmed.to_owned())
}

fn family_from_name_table(data: &[u8]) -> Option<String> {
    if read_u16(data, 0)? != 0 {
        return None;
    }
    let count = read_u16(data, 2)? as usize;
    let storage = read_u16(data, 4)? as usize;
    if count == 0 || count > 4096 || data.len() < 6 + count.checked_mul(12)? {
        return None;
    }
    let mut windows_family: Option<String> = None;
    let mut unicode_family: Option<String> = None;
    let mut mac_family: Option<String> = None;
    let mut typographic: Option<String> = None;
    for index in 0..count {
        let record = 6 + index * 12;
        let platform = read_u16(data, record)?;
        let name_id = read_u16(data, record + 6)?;
        if name_id != 1 && name_id != 16 {
            continue;
        }
        let length = read_u16(data, record + 8)? as usize;
        let offset = read_u16(data, record + 10)? as usize;
        let start = storage.checked_add(offset)?;
        let end = start.checked_add(length)?;
        let bytes = data.get(start..end)?;
        let value = match platform {
            3 => clean_family_name(&decode_utf16be(bytes)),
            0 => clean_family_name(&decode_utf16be(bytes)),
            1 => clean_family_name(&bytes.iter().map(|byte| *byte as char).collect::<String>()),
            _ => None,
        };
        let Some(value) = value else { continue };
        if name_id == 16 {
            if typographic.is_none() {
                typographic = Some(value);
            }
            continue;
        }
        match platform {
            3 => {
                if windows_family.is_none() {
                    windows_family = Some(value);
                }
            }
            0 => {
                if unicode_family.is_none() {
                    unicode_family = Some(value);
                }
            }
            1 => {
                if mac_family.is_none() {
                    mac_family = Some(value);
                }
            }
            _ => {}
        }
    }
    typographic.or(windows_family).or(unicode_family).or(mac_family)
}

fn family_from_sfnt_at(file: &mut fs::File, base: u64) -> Option<String> {
    file.seek(SeekFrom::Start(base)).ok()?;
    let mut header = [0u8; 12];
    file.read_exact(&mut header).ok()?;
    let num_tables = read_u16(&header, 4)? as usize;
    if num_tables == 0 || num_tables > 256 {
        return None;
    }
    let mut table_dir = vec![0u8; num_tables * 16];
    file.read_exact(&mut table_dir).ok()?;
    let mut name_offset = None;
    let mut name_length = 0usize;
    for record in table_dir.chunks_exact(16) {
        if &record[0..4] == b"name" {
            let offset = u32::from_be_bytes([record[4], record[5], record[6], record[7]]) as usize;
            let length = u32::from_be_bytes([record[8], record[9], record[10], record[11]]) as usize;
            if length > 0 && length <= MAX_NAME_TABLE_BYTES {
                name_offset = Some(offset as u64);
                name_length = length;
            }
            break;
        }
    }
    let offset = name_offset?;
    file.seek(SeekFrom::Start(base + offset)).ok()?;
    let mut table = vec![0u8; name_length];
    file.read_exact(&mut table).ok()?;
    family_from_name_table(&table)
}

fn families_from_file(path: &Path) -> Vec<String> {
    let metadata = match fs::metadata(path) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    if metadata.len() == 0 || metadata.len() > MAX_FONT_FILE_BYTES {
        return Vec::new();
    }
    let mut file = match fs::File::open(path) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    let mut tag = [0u8; 4];
    if file.read_exact(&mut tag).is_err() {
        return Vec::new();
    }
    let mut bases = Vec::new();
    if &tag == b"ttcf" {
        let mut collection = [0u8; 12];
        if file.read_exact(&mut collection).is_err() {
            return Vec::new();
        }
        let count = read_u32(&collection, 0).unwrap_or(0) as usize;
        if count == 0 || count > 64 {
            return Vec::new();
        }
        let mut offsets = vec![0u8; count * 4];
        if file.read_exact(&mut offsets).is_err() {
            return Vec::new();
        }
        for index in 0..count.min(16) {
            let offset = u32::from_be_bytes([
                offsets[index * 4],
                offsets[index * 4 + 1],
                offsets[index * 4 + 2],
                offsets[index * 4 + 3],
            ]) as u64;
            bases.push(offset);
        }
    } else {
        bases.push(0);
    }
    let mut families = Vec::new();
    for base in bases {
        if let Some(family) = family_from_sfnt_at(&mut file, base) {
            families.push(family);
        }
    }
    families
}

/// список гарнитур, установленных в системе: сканируем стандартные каталоги шрифтов
/// и читаем имя семейства из таблицы name каждого файла
pub fn font_list(_request: &Value) -> Result<Value, String> {
    let mut files = Vec::new();
    for dir in font_directories() {
        collect_font_files(&dir, &mut files, 0);
    }
    let mut families: BTreeMap<String, String> = BTreeMap::new();
    for path in &files {
        for family in families_from_file(path) {
            let key = family.to_lowercase();
            families.entry(key).or_insert(family);
        }
    }
    let fonts: Vec<String> = families.into_values().collect();
    Ok(json!({ "fonts": fonts }))
}
