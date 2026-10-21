// порт python-модуля turbo.py: пакетное SHA-256 через внешний бинарник
// nivora-turbo (cpp-turbo/). Одна пачка файлов — один запуск процесса;
// при любом сбое возвращаем None и вызывающий считает хеши сам.
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const TURBO_TIMEOUT_SECONDS: u64 = 600;
// меньше этого размера пачка не стоит запуска процесса.
const MIN_BATCH_FILES: usize = 4;

static CACHED_BINARY: OnceLock<Option<PathBuf>> = OnceLock::new();

/// путь к nivora-turbo: env NIVORA_TURBO_BIN, каталог рядом с бинарником
/// лаунчера, resources/backend упакованной сборки, системные пути.
/// Результат кешируется — как `_cached_resolved` в Python.
#[allow(dead_code)] // станет горячим после порта cloud_storage.py
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

/// shutil.which(): первый исполняемый файл в PATH.
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

/// хеширует список файлов одним запуском nivora-turbo. None — бинарника
/// нет, пачка мала или процесс не смог: вызывающий делает fallback.
#[allow(dead_code)] // станет горячим после порта cloud_storage.py
pub(crate) fn sha256_batch(paths: &[PathBuf]) -> Option<HashMap<PathBuf, (String, u64)>> {
    sha256_batch_with(turbo_binary()?, paths)
}

fn sha256_batch_with(
    binary: PathBuf,
    paths: &[PathBuf],
) -> Option<HashMap<PathBuf, (String, u64)>> {
    if paths.len() < MIN_BATCH_FILES {
        return None;
    }
    // dict.fromkeys: уникальные пути с сохранением порядка.
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
        // контракт nivora-turbo: digest \t size \t path
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

    // фиктивный nivora-turbo: печатает «digest \t size \t path» для каждого
    // аргумента; отсутствующему файлу соответствует прочерк (как у боевого).
    fn fake_turbo(dir: &Path) -> Option<PathBuf> {
        if find_in_path("sha256sum").is_none() {
            return None; // фиктиву нужен coreutils; без него тест пропускаем
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
        // «бинарник» — заведомо несуществующий: до него дело не дойдёт.
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
        // фиктивный бинарник печатает «-» для отсутствующих файлов —
        // по контракту это ошибка всей пачки, а не пропуск файла.
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
}
