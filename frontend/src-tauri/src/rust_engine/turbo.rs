// пакетный SHA-256 через nivora-turbo; при сбое вызывающая сторона считает хеши на Rust
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const TURBO_TIMEOUT_SECONDS: u64 = 600;
// для меньших пачек не запускаем отдельный процесс
const MIN_BATCH_FILES: usize = 4;

static CACHED_BINARY: OnceLock<Option<PathBuf>> = OnceLock::new();

/// ищет и кеширует путь к `nivora-turbo`
pub(crate) fn turbo_binary() -> Option<PathBuf> {
    CACHED_BINARY.get_or_init(discover_turbo_binary).clone()
}

fn discover_turbo_binary() -> Option<PathBuf> {
    super::platform::helper_path("nivora-turbo", "NIVORA_TURBO_BIN")
        .filter(|path| is_executable(path))
        .or_else(|| find_in_path("nivora-turbo"))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub(crate) fn find_in_path(name: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    let names = [format!("{name}.exe"), name.to_owned()];
    #[cfg(not(windows))]
    let names = [name.to_owned()];
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|directory| {
        names.iter().find_map(|name| {
            let candidate = directory.join(name);
            (candidate.is_file() && is_executable(&candidate)).then_some(candidate)
        })
    })
}

/// хеширует пачку одним запуском; при `None` используется обход на Rust
pub(crate) fn sha256_batch(paths: &[PathBuf]) -> Option<HashMap<PathBuf, (String, u64)>> {
    sha256_batch_with(turbo_binary()?, paths)
}

/// сканирует экземпляр одной командой `instance-manifest`; при сбое возвращает `None`
pub(crate) fn instance_manifest(directory: &Path) -> Option<Vec<serde_json::Value>> {
    let binary = turbo_binary()?;
    let output = super::platform::command(&binary)
        .arg("instance-manifest")
        .arg(directory)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_manifest_entries(&String::from_utf8_lossy(&output.stdout)))
}

/// результат анализа одного JAR
#[derive(Clone, Debug, Default)]
pub(crate) struct ModScan {
    pub(crate) loaders: Vec<String>,
    pub(crate) mod_id: String,
    pub(crate) mixins: Vec<String>,
    pub(crate) packages: Vec<String>,
    pub(crate) corrupted: bool,
}

/// анализирует пачку JAR одним запуском; при сбое возвращает `None`
pub(crate) fn scan_mods(paths: &[PathBuf]) -> Option<Vec<ModScan>> {
    if paths.is_empty() {
        return Some(Vec::new());
    }
    let binary = turbo_binary()?;
    let mut command = super::platform::command(&binary);
    command
        .arg("scan-mods")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    for path in paths {
        command.arg(path);
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::with_capacity(paths.len());
    for (index, line) in stdout.lines().enumerate() {
        if index >= paths.len() {
            break;
        }
        results.push(parse_mod_scan(line));
    }
    (results.len() == paths.len()).then_some(results)
}

fn parse_mod_scan(line: &str) -> ModScan {
    let split_list = |value: &str| -> Vec<String> {
        if value.is_empty() || value == "-" {
            Vec::new()
        } else {
            value.split('+').map(str::to_owned).collect()
        }
    };
    let parts: Vec<&str> = line.split('\t').collect();
    let field = |index: usize| -> String {
        parts
            .get(index)
            .map(|value| if *value == "-" { "" } else { value })
            .unwrap_or_default()
            .to_owned()
    };
    ModScan {
        loaders: split_list(&field(0)),
        mod_id: field(1),
        mixins: split_list(&field(4)),
        packages: split_list(&field(5)),
        corrupted: field(6) == "1",
    }
}

#[cfg(test)]
mod scan_mods_tests {
    use super::*;

    #[test]
    fn mod_scan_line_parses_lists_and_corruption() {
        let scan = parse_mod_scan("fabric+forge\tiris\tIris\t1.7\tiris.mixins.json+a.mixins.json\tnet/irisshaders+com/example\t0\t/mods/iris.jar");
        assert_eq!(scan.loaders, vec!["fabric".to_owned(), "forge".to_owned()]);
        assert_eq!(scan.mod_id, "iris");
        assert_eq!(scan.mixins.len(), 2);
        assert_eq!(scan.packages.len(), 2);
        assert!(!scan.corrupted);
    }

    #[test]
    fn empty_fields_are_dashes() {
        let scan = parse_mod_scan("-\t-\t-\t-\t-\t-\t1\t/mods/broken.jar");
        assert!(scan.loaders.is_empty());
        assert!(scan.mixins.is_empty());
        assert!(scan.packages.is_empty());
        assert!(scan.corrupted);
    }
}

fn parse_manifest_entries(stdout: &str) -> Vec<serde_json::Value> {
    let mut entries = Vec::new();
    for line in stdout.lines() {
        // дефис обозначает пустое строковое поле; число классов и распакованный размер обязательны
        let parts: Vec<&str> = line.split('\t').collect();
        fn dash(value: &str) -> &str {
            if value == "-" { "" } else { value }
        }
        let kind = parts.first().copied().unwrap_or_default();
        let entry = match (kind, parts.len()) {
            ("V", 3) => Some(serde_json::json!({
                "kind": "meta", "key": parts[1], "value": parts[2],
            })),
            ("O", 3) => Some(serde_json::json!({
                "kind": "option", "key": parts[1], "value": parts[2],
            })),
            ("M", 10) => match (parts[8].parse::<i64>(), parts[9].parse::<i64>(), parts[2].parse::<u64>()) {
                (Ok(classes), Ok(unpacked), Ok(size)) => Some(serde_json::json!({
                    "kind": "mod",
                    "path": parts[1],
                    "size": size,
                    "sha256": parts[3],
                    "loader": dash(parts[4]),
                    "mod_id": dash(parts[5]),
                    "name": dash(parts[6]),
                    "version": dash(parts[7]),
                    "classes": classes,
                    "unpacked": unpacked,
                })),
                _ => None,
            },
            ("P", 4) => parts[2].parse::<u64>().ok().map(|size| serde_json::json!({
                "kind": "resourcepack",
                "path": parts[1],
                "size": size,
                "sha256": parts[3],
            })),
            ("S", 6) => match (parts[4].parse::<i64>(), parts[5].parse::<i64>(), parts[2].parse::<u64>()) {
                (Ok(programs), Ok(files), Ok(size)) => Some(serde_json::json!({
                    "kind": "shaderpack",
                    "path": parts[1],
                    "size": size,
                    "sha256": parts[3],
                    "programs": programs,
                    "files": files,
                })),
                _ => None,
            },
            ("C", 4) => parts[2].parse::<u64>().ok().map(|size| serde_json::json!({
                "kind": "config",
                "path": parts[1],
                "size": size,
                "sha256": parts[3],
            })),
            _ => None,
        };
        if let Some(entry) = entry {
            entries.push(entry);
        }
    }
    entries
}

fn sha256_batch_with(
    binary: PathBuf,
    paths: &[PathBuf],
) -> Option<HashMap<PathBuf, (String, u64)>> {
    if paths.len() < MIN_BATCH_FILES {
        return None;
    }
    // убираем повторы, сохраняя порядок путей
    let mut unique: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !unique.contains(path) {
            unique.push(path.clone());
        }
    }
    let mut child = super::platform::command(&binary)
        .arg("sha256")
        .args(&unique)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(TURBO_TIMEOUT_SECONDS);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return None,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
    let mut stdout = String::new();
    child.stdout.take()?.read_to_string(&mut stdout).ok()?;
    let mut digests: HashMap<PathBuf, (String, u64)> = HashMap::new();
    for line in stdout.lines() {
        // формат ответа: хеш, размер и путь через табуляцию
        let mut parts = line.splitn(3, '\t');
        let (digest, size, raw_path) = match (parts.next(), parts.next(), parts.next()) {
            (Some(digest), Some(size), Some(raw_path)) if parts.next().is_none() => {
                (digest, size, raw_path)
            }
            _ => continue,
        };
        if digest.is_empty() || digest == "-" {
            return None;
        }
        let size: u64 = size.parse().ok()?;
        digests.insert(PathBuf::from(raw_path), (digest.to_lowercase(), size));
    }
    if digests.len() != unique.len() {
        return None;
    }
    Some(digests)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nivora-turbo-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_turbo(dir: &Path) -> Option<PathBuf> {
        if find_in_path("sha256sum").is_none() {
            return None; // без `coreutils` тест пропускается
        }
        let script = dir.join("fake-nivora-turbo");
        std::fs::write(
            &script,
            "#!/bin/sh\nshift\nfor f in \"$@\"; do\n  if [ -f \"$f\" ]; then\n    d=$(sha256sum \"$f\" | cut -d' ' -f1)\n    s=$(wc -c < \"$f\" | tr -d ' ')\n    printf '%s\\t%s\\t%s\\n' \"$d\" \"$s\" \"$f\"\n  else\n    printf '%s\\t%s\\t%s\\n' '-' 0 \"$f\"\n  fi\ndone\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        Some(script)
    }

    fn digest_of(content: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(content);
        format!("{:x}", hasher.finalize())
    }

    #[test]
    fn batch_hashes_files_matching_sha256() {
        let root = temp_root("ok");
        let Some(binary) = fake_turbo(&root) else {
            return;
        };
        let paths: Vec<PathBuf> = (0..4)
            .map(|index| {
                let path = root.join(format!("file{index}.bin"));
                std::fs::write(&path, format!("payload-{index}")).unwrap();
                path
            })
            .collect();
        let digests = sha256_batch_with(binary, &paths).expect("пачка хешируется");
        for path in &paths {
            let content = std::fs::read(path).unwrap();
            let expected = digest_of(&content);
            let (digest, size) = digests.get(path).expect("файл в результате");
            assert_eq!(digest, &expected);
            assert_eq!(*size, content.len() as u64);
        }
    }

    #[test]
    fn small_batch_is_rejected_without_spawn() {
        let root = temp_root("small");
        let paths: Vec<PathBuf> = vec![root.join("a"), root.join("b"), root.join("c")];
        assert!(sha256_batch_with(root.join("missing-turbo"), &paths).is_none());
    }

    #[test]
    fn dash_digest_fails_the_whole_batch() {
        let root = temp_root("dash");
        let Some(binary) = fake_turbo(&root) else {
            return;
        };
        let paths: Vec<PathBuf> = (0..4)
            .map(|index| root.join(format!("absent{index}")))
            .collect();
        assert!(sha256_batch_with(binary, &paths).is_none());
    }

    #[test]
    fn missing_files_break_the_count_contract() {
        let root = temp_root("count");
        let Some(binary) = fake_turbo(&root) else {
            return;
        };
        let mut paths: Vec<PathBuf> = Vec::new();
        for index in 0..4 {
            let path = root.join(format!("mix{index}"));
            if index < 3 {
                std::fs::write(&path, "x").unwrap();
            }
            paths.push(path);
        }
        assert!(sha256_batch_with(binary, &paths).is_none());
    }

    /// разбирает протокол `nivora-turbo` (V/M/P/S/C/O); некорректные строки пропускаются
    #[test]
    fn instance_manifest_parses_turbo_protocol() {
        let stdout = concat!(
            "V\t1.21.4-fabric\t1.21.4\n",
            "O\trenderDistance\t24\n",
            "M\tmods/sodium.jar\t1024\tab12\tfabric\tsodium\tSodium\t0.6\t900\t500000\n",
            "M\tmods/broken.jar\t2048\tcd34\tforge\t-\t-\t-\tx\t9\n",
            "P\tresourcepacks/pack.zip\t32\tef56\n",
            "S\tshaderpacks/bsl.zip\t64\t09ab\t42\t60\n",
            "C\tconfig/create.cfg\t16\t1234\n",
            "X\tgarbage\n",
        );
        let entries = parse_manifest_entries(stdout);
        assert_eq!(entries.len(), 6);
        assert_eq!(entries[0]["kind"], serde_json::json!("meta"));
        assert_eq!(entries[0]["value"], serde_json::json!("1.21.4"));
        assert_eq!(entries[1]["kind"], serde_json::json!("option"));
        let mod_entry = &entries[2];
        assert_eq!(mod_entry["path"], serde_json::json!("mods/sodium.jar"));
        assert_eq!(mod_entry["mod_id"], serde_json::json!("sodium"));
        assert_eq!(mod_entry["classes"], serde_json::json!(900));
        assert_eq!(mod_entry["size"], serde_json::json!(1024));
        let shader = &entries[4];
        assert_eq!(shader["kind"], serde_json::json!("shaderpack"));
        assert_eq!(shader["programs"], serde_json::json!(42));
        let config = &entries[5];
        assert_eq!(config["path"], serde_json::json!("config/create.cfg"));
    }

    #[test]
    fn dash_fields_map_to_empty_strings() {
        let stdout = "M\tmods/x.jar\t10\tabcd\t-\t-\t-\t-\t0\t0\n";
        let entries = parse_manifest_entries(stdout);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["mod_id"], serde_json::json!(""));
        assert_eq!(entries[0]["loader"], serde_json::json!(""));
        assert_eq!(entries[0]["classes"], serde_json::json!(0));
    }
}
