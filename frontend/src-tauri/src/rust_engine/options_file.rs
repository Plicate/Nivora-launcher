use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

pub(crate) struct OptionsFile {
    /// файл существовал и прочитан (иначе правки не применимы)
    present: bool,
    lines: Vec<String>,
    slots: BTreeMap<String, usize>,
}

/// ключ заканчивается перед первым двоеточием и не может быть пустым
fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(':')?;
    if key.is_empty() {
        return None;
    }
    Some((key, value))
}

impl OptionsFile {
    /// отсутствующий файл/симлинк/ошибка чтения → `present == false`
    pub(crate) fn read(path: &Path) -> OptionsFile {
        let mut file = OptionsFile {
            present: false,
            lines: Vec::new(),
            slots: BTreeMap::new(),
        };
        let Ok(metadata) = fs::symlink_metadata(path) else {
            return file;
        };
        if metadata.is_symlink() || !metadata.is_file() {
            return file;
        }
        let Ok(text) = fs::read_to_string(path) else {
            return file;
        };
        file.present = true;
        for line in text.lines() {
            if let Some((key, _)) = split_key_value(line) {
                file.slots.entry(key.to_owned()).or_insert(file.lines.len());
            }
            file.lines.push(line.to_owned());
        }
        file
    }

    pub(crate) fn present(&self) -> bool {
        self.present
    }

    /// значение возвращается без `trim`, как записала игра
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.slots
            .get(key)
            .and_then(|index| split_key_value(&self.lines[*index]))
            .map(|(_, value)| value)
    }

    /// заменяет строку ключа на месте (порядок строк сохраняется) или
    /// дописывает `key:value` в конец
    pub(crate) fn set(&mut self, key: &str, value: &str) {
        let line = format!("{key}:{value}");
        match self.slots.get(key) {
            Some(index) => self.lines[*index] = line,
            None => {
                self.slots.insert(key.to_owned(), self.lines.len());
                self.lines.push(line);
            }
        }
    }

    /// содержимое файла: строки + завершающий перевод (как пишет игра)
    pub(crate) fn render(&self) -> String {
        if self.lines.is_empty() {
            return String::new();
        }
        let mut output = self.lines.join("\n");
        output.push('\n');
        output
    }

    /// применяет правки к `options.txt` в каталоге экземпляра: атомарная
    /// замена, симлинки и отсутствующие файлы не трогаем
    pub(crate) fn apply(instance_dir: &Path, changes: &BTreeMap<String, String>) -> Result<(), String> {
        if changes.is_empty() {
            return Ok(());
        }
        let path = instance_dir.join("options.txt");
        let mut file = OptionsFile::read(&path);
        if !file.present() {
            return Err("options.txt не найден или не читается — настройки Minecraft не изменены".to_owned());
        }
        for (key, value) in changes {
            file.set(key, value);
        }
        let content = file.render();
        super::portable_fs::atomic_write(instance_dir, &["options.txt"], content.as_bytes())
            .map(|_| ())
            .map_err(|error| format!("Не удалось обновить options.txt: {error}"))
    }
}

/// исходное содержимое файла (backup в бенчмарке шейдеров): отсутствующий
/// файл/симлинк → пустая строка
pub(crate) fn read_raw(path: &Path) -> String {
    if path.is_symlink() || !path.is_file() {
        return String::new();
    }
    fs::read_to_string(path).unwrap_or_default()
}

/// записывает содержимое без изменений и удаляет файл при пустом значении
pub(crate) fn write_raw(path: &Path, content: &str) -> Result<(), String> {
    let write = || -> std::io::Result<()> {
        if !content.is_empty() {
            fs::write(path, content)
        } else if path.exists() {
            fs::remove_file(path)
        } else {
            Ok(())
        }
    };
    write().map_err(|error| format!("Не удалось вернуть options.txt: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nivora-options-file-{tag}-{}-{}",
            std::process::id(),
            super::super::mods::random_hex(4)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn read_parses_keys_and_keeps_line_order() {
        let root = temp_root("read");
        let path = root.join("options.txt");
        fs::write(&path, "version:3935\nrenderDistance:32\nfullscreen:false\n").unwrap();
        let file = OptionsFile::read(&path);
        assert!(file.present());
        assert_eq!(file.get("renderDistance"), Some("32"));
        assert_eq!(file.get("fullscreen"), Some("false"));
        assert_eq!(file.get("version"), Some("3935"));
        assert_eq!(file.get("missing"), None);
        assert_eq!(file.lines.len(), 3);
    }

    #[test]
    fn set_replaces_in_place_and_appends_new_keys() {
        let root = temp_root("set");
        let path = root.join("options.txt");
        fs::write(&path, "renderDistance:32\nmaxFps:120\n").unwrap();
        let mut file = OptionsFile::read(&path);
        file.set("renderDistance", "16");
        file.set("simulationDistance", "10");
        assert_eq!(
            file.render(),
            "renderDistance:16\nmaxFps:120\nsimulationDistance:10\n"
        );
        // порядок исходных строк сохранён, новый ключ в конце
        assert_eq!(file.lines[0], "renderDistance:16");
        assert_eq!(file.lines[1], "maxFps:120");
        assert_eq!(file.lines[2], "simulationDistance:10");
    }

    #[test]
    fn values_may_contain_colons_and_json() {
        // 1.21.4+ хранит некоторые значения JSON-строками с двоеточиями
        let root = temp_root("json");
        let path = root.join("options.txt");
        fs::write(&path, "shaderPack:\"{\\\"id\\\":\\\"nivora\\\":true}\"\n").unwrap();
        let file = OptionsFile::read(&path);
        assert_eq!(
            file.get("shaderPack"),
            Some("\"{\\\"id\\\":\\\"nivora\\\":true}\"")
        );
    }

    #[test]
    fn apply_writes_changes_atomically_and_preserves_the_rest() {
        let root = temp_root("apply");
        fs::write(root.join("options.txt"), "renderDistance:32\nsoundCategory_master:0.5\n").unwrap();
        let mut changes = BTreeMap::new();
        changes.insert("renderDistance".to_owned(), "16".to_owned());
        OptionsFile::apply(&root, &changes).unwrap();
        let text = fs::read_to_string(root.join("options.txt")).unwrap();
        assert_eq!(text, "renderDistance:16\nsoundCategory_master:0.5\n");
        // повторное применение того же значения ничего не ломает
        OptionsFile::apply(&root, &changes).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("options.txt")).unwrap(),
            "renderDistance:16\nsoundCategory_master:0.5\n"
        );
        assert!(OptionsFile::apply(&root, &BTreeMap::new()).is_ok());
    }

    #[test]
    fn apply_refuses_missing_or_symlinked_file() {
        let root = temp_root("missing");
        let mut changes = BTreeMap::new();
        changes.insert("renderDistance".to_owned(), "16".to_owned());
        let error = OptionsFile::apply(&root, &changes).unwrap_err();
        assert_eq!(error, "options.txt не найден или не читается — настройки Minecraft не изменены");

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let target = root.join("elsewhere.txt");
            fs::write(&target, "renderDistance:8\n").unwrap();
            let link = root.join("options.txt");
            symlink(&target, &link).unwrap();
            let error = OptionsFile::apply(&root, &changes).unwrap_err();
            assert!(error.contains("не найден или не читается"), "симлинк: {error}");
        }
    }

    #[test]
    fn raw_read_write_roundtrip_for_benchmark() {
        let root = temp_root("raw");
        let path = root.join("options.txt");
        fs::write(&path, "renderDistance:32\n").unwrap();
        let original = read_raw(&path);
        assert_eq!(original, "renderDistance:32\n");
        write_raw(&path, &original).unwrap();
        assert_eq!(read_raw(&path), original);
        // пустой restore удаляет файл (состояние "файла не было")
        write_raw(&path, "").unwrap();
        assert!(!path.exists());
        assert_eq!(read_raw(&path), "");
    }
}

#[cfg(test)]
mod autotune_safety_tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nivora-options-safe-{tag}-{}-{}",
            std::process::id(),
            super::super::mods::random_hex(4)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// реальный options.txt 1.20.1: сотни строк, CRLF, ключи без значения
    fn realistic_options(crlf: bool) -> String {
        let newline = if crlf { "\r\n" } else { "\n" };
        let mut lines = vec![
            "version:3465",
            "renderDistance:32",
            "simulationDistance:20",
            "maxFps:260",
            "mipmapLevels:0",
            "graphicsMode:fancy",
            "ao:true",
            "biomeBlendRadius:5",
            "entityDistanceScaling:2.0",
            "entityShadows:true",
            "particles:2",
            "cloudStatus:true",
            "vsync:false",
            "pauseOnLostFocus:true",
            "screenEffectScale:1.0",
            "chatOpacity:0.5",
            "textBackgroundOpacity:0.5",
            "guiScale:1",
            "soundCategory_master:1.0",
            "resourcePacks:[\"vanilla\"]",
            "incompatibleResourcePacks:[]",
            "lang:ru_ru",
            "key_key.forward:key.keyboard.w",
            // ключ без значения и мусорная строка — так бывает после сбоя записи
            "fullscreen:",
            "битая строка без двоеточия",
            "lastServer:",
        ];
        // добиваем объёмом, как в настоящем файле
        for index in 0..200 {
            lines.push(Box::leak(format!("key_test{index}:key.keyboard.unknown").into_boxed_str()));
        }
        lines.join(newline) + newline
    }

    /// применяет план автонастройки к реальному файлу и проверяет, что
    /// игра получит валидный options.txt
    #[test]
    fn autotune_keeps_options_file_parseable() {
        for crlf in [false, true] {
            let root = temp_root(if crlf { "crlf" } else { "lf" });
            let path = root.join("options.txt");
            fs::write(&path, realistic_options(crlf)).unwrap();
            let before = OptionsFile::read(&path);
            let options: serde_json::Map<String, serde_json::Value> = before
                .lines
                .iter()
                .filter_map(|line| {
                    line.split_once(':')
                        .map(|(key, value)| (key.to_owned(), serde_json::json!(value)))
                })
                .collect();

            let hardware = crate::rust_engine::hardware::profile_cached();
            for mode in [
                crate::rust_engine::autotune::TuneMode::Performance,
                crate::rust_engine::autotune::TuneMode::Balanced,
                crate::rust_engine::autotune::TuneMode::Quality,
            ] {
                let changes = crate::rust_engine::autotune::recommended_minecraft_options(
                    &options, hardware, 0, 144, "1.20.1", mode,
                );
                OptionsFile::apply(&root, &changes).unwrap();

                let text = fs::read_to_string(&path).unwrap();
                let after = OptionsFile::read(&path);
                assert!(after.present(), "файл должен остаться читаемым");
                // ни одной строки не потеряно и не задвоено
                assert_eq!(after.lines.len(), before.lines.len(), "число строк изменилось");
                // автонастройка не добавляет "битых" строк: их число как было
                let broken_before = before.lines.iter().filter(|line| !line.contains(':')).count();
                let broken_after = after.lines.iter().filter(|line| !line.contains(':')).count();
                assert_eq!(broken_after, broken_before, "появились строки без ключа");
                // служебные ключи не тронуты
                assert_eq!(after.get("version"), Some("3465"));
                assert_eq!(after.get("resourcePacks"), Some("[\"vanilla\"]"));
                assert_eq!(after.get("lang"), Some("ru_ru"));
                assert_eq!(after.get("fullscreen"), Some(""));
                // повторное применение ничего не меняет (идемпотентность)
                let again = crate::rust_engine::autotune::recommended_minecraft_options(
                    &options, hardware, 0, 144, "1.20.1", mode,
                );
                OptionsFile::apply(&root, &again).unwrap();
                assert_eq!(fs::read_to_string(&path).unwrap(), text, "повторная запись изменила файл");
            }
        }
    }

    /// после записи новое чтение даёт те же значения — автонастройка не борется сама с собой
    #[test]
    fn autotune_is_stable_on_second_pass() {
        let root = temp_root("stable");
        let path = root.join("options.txt");
        fs::write(&path, realistic_options(false)).unwrap();
        let hardware = crate::rust_engine::hardware::profile_cached();
        let first = OptionsFile::read(&path);
        let options: serde_json::Map<String, serde_json::Value> = first
            .lines
            .iter()
            .filter_map(|line| {
                line.split_once(':')
                    .map(|(key, value)| (key.to_owned(), serde_json::json!(value)))
            })
            .collect();
        let changes = crate::rust_engine::autotune::recommended_minecraft_options(
            &options,
            hardware,
            0,
            144,
            "1.20.1",
            crate::rust_engine::autotune::TuneMode::Balanced,
        );
        OptionsFile::apply(&root, &changes).unwrap();

        // второй проход видит уже применённые значения и не должен ничего менять
        let second = OptionsFile::read(&path);
        let applied: serde_json::Map<String, serde_json::Value> = second
            .lines
            .iter()
            .filter_map(|line| {
                line.split_once(':')
                    .map(|(key, value)| (key.to_owned(), serde_json::json!(value)))
            })
            .collect();
        let again = crate::rust_engine::autotune::recommended_minecraft_options(
            &applied,
            hardware,
            0,
            144,
            "1.20.1",
            crate::rust_engine::autotune::TuneMode::Balanced,
        );
        assert!(again.is_empty(), "второй проход снова что-то меняет: {again:?}");
    }
}
