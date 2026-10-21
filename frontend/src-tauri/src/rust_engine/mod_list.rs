//! Экспорт списка модов экземпляра: Markdown / CSV / текст.
//!
//! Контракт операции `instance_mod_list_export`:
//! `{name, format: "markdown"|"csv"|"text", include_disabled: bool}` →
//! `{content, format, count, file_name}`. Данные те же, что показывает
//! вкладка модов: jar-файлы каталога `mods` (включая `.disabled`),
//! метаданные из `fabric.mod.json` / `META-INF/mods.toml` читает
//! nivora-turbo scan-jars; без него остаётся имя файла.
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::state;

const MAX_JARS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Csv,
    Text,
}

impl Format {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim).unwrap_or("markdown") {
            "" | "markdown" | "md" => Ok(Self::Markdown),
            "csv" => Ok(Self::Csv),
            "text" | "txt" => Ok(Self::Text),
            other => Err(format!("Неизвестный формат списка модов: {other}")),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::Csv => "csv",
            Self::Text => "text",
        }
    }
}

pub fn instance_mod_list_export(request: &Value) -> Result<Value, String> {
    let name = request
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Поле «name» обязательно для этой операции".to_owned())?;
    let format = Format::parse(request.get("format").and_then(Value::as_str))?;
    let include_disabled = request.get("include_disabled").and_then(Value::as_bool).unwrap_or(true);
    let roots = state::launcher_roots();
    let game_dir = state::settings_game_dir(&roots.config_home, &roots.data_home);
    let path = state::resolve_instance_path(&game_dir, name).ok_or_else(|| format!("Экземпляр не найден: {name}"))?;
    let metadata = state::instance_metadata(&path);
    let display_name = metadata
        .get("display_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(name)
        .to_owned();
    let mods_dir = path.join("mods");
    let entries = collect_mods(&mods_dir, include_disabled)?;
    // метаданные модов — из jar-манифестов (nivora-turbo scan-jars), по пути.
    let mut manifests: std::collections::HashMap<PathBuf, super::global_search::JarManifest> = std::collections::HashMap::new();
    if let Some(binary) = super::turbo::turbo_binary() {
        for manifest in super::global_search::scan_jars(&binary, &entries) {
            manifests.insert(manifest.path.clone(), manifest);
        }
    }
    let rows: Vec<Row> = entries
        .iter()
        .map(|file| {
            let manifest = manifests.get(file).cloned();
            let (name, version, mod_id, loader) = manifest
                .as_ref()
                .map(|value| {
                    (
                        (!value.name.is_empty()).then(|| json!(value.name)),
                        (!value.version.is_empty()).then(|| json!(value.version)),
                        (!value.mod_id.is_empty()).then(|| json!(value.mod_id)),
                        (!value.loader.is_empty()).then(|| json!(value.loader)),
                    )
                })
                .unwrap_or((None, None, None, None));
            Row {
                file: file.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                name,
                version,
                mod_id,
                loader,
                disabled: file
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_none_or(|value| value.eq_ignore_ascii_case("disabled")),
            }
        })
        .collect();
    let content = render(format, &display_name, name, &rows);
    let count = rows.len();
    let file_name = sanitize_file_name(&display_name, format);
    Ok(json!({ "content": content, "format": format.name(), "count": count, "file_name": file_name }))
}

struct Row {
    file: String,
    name: Option<Value>,
    version: Option<Value>,
    mod_id: Option<Value>,
    loader: Option<Value>,
    disabled: bool,
}

impl Row {
    fn title(&self) -> String {
        self.name
            .as_ref()
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| self.file.trim_end_matches(".jar").trim_end_matches(".disabled").to_owned())
    }

    fn cell(&self, value: &Option<Value>) -> String {
        value.as_ref().and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).unwrap_or("-").to_owned()
    }
}

fn collect_mods(mods_dir: &Path, include_disabled: bool) -> Result<Vec<PathBuf>, String> {
    let is_symlink = |path: &Path| fs::symlink_metadata(path).map(|meta| meta.is_symlink()).unwrap_or(false);
    if is_symlink(mods_dir) || !mods_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = fs::read_dir(mods_dir)
        .map_err(|error| format!("Не удалось прочитать каталог модов: {error}"))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            if is_symlink(path) || !path.is_file() {
                return false;
            }
            let name = path.file_name().and_then(|value| value.to_str()).unwrap_or_default().to_ascii_lowercase();
            let stem = name.trim_end_matches(".disabled");
            if !stem.ends_with(".jar") {
                return false;
            }
            include_disabled || !name.ends_with(".disabled")
        })
        .collect();
    files.sort_by_key(|path| path.file_name().map(|value| value.to_string_lossy().to_lowercase()).unwrap_or_default());
    files.truncate(MAX_JARS);
    Ok(files)
}

fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_owned()
    }
}

fn render(format: Format, display_name: &str, name: &str, rows: &[Row]) -> String {
    match format {
        Format::Markdown => {
            let mut out = format!("# Моды: {display_name}\n\nЭкземпляр: `{name}` · модов: {}\n\n", rows.len());
            out.push_str("| Мод | Версия | ID | Загрузчик | Файл | Состояние |\n|---|---|---|---|---|---|\n");
            for row in rows {
                out.push_str(&format!(
                    "| {} | {} | {} | {} | `{}` | {} |\n",
                    row.title().replace('|', "\\|"),
                    row.cell(&row.version),
                    row.cell(&row.mod_id),
                    row.cell(&row.loader),
                    row.file.replace('|', "\\|"),
                    if row.disabled { "выключен" } else { "включен" },
                ));
            }
            out
        }
        Format::Csv => {
            let mut out = String::from("mod,version,mod_id,loader,file,enabled\n");
            for row in rows {
                out.push_str(&format!(
                    "{},{},{},{},{},{}\n",
                    csv_field(&row.title()),
                    csv_field(&row.cell(&row.version)),
                    csv_field(&row.cell(&row.mod_id)),
                    csv_field(&row.cell(&row.loader)),
                    csv_field(&row.file),
                    !row.disabled,
                ));
            }
            out
        }
        Format::Text => {
            let mut out = format!("Моды: {display_name} (экземпляр {name}), всего {}\n", rows.len());
            for row in rows {
                out.push_str(&format!("- {} {}\n", row.title(), row.cell(&row.version)));
            }
            out
        }
    }
}

fn sanitize_file_name(display_name: &str, format: Format) -> String {
    let stem: String = display_name
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\') { ' ' } else { c })
        .collect();
    let trimmed = stem.trim().chars().take(60).collect::<String>();
    let base = if trimmed.is_empty() { "mods".to_owned() } else { trimmed };
    format!("{}.{}", base, format.extension())
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Csv => "csv",
            Self::Text => "txt",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sandbox(PathBuf);

    impl Sandbox {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!("nivora-modlist-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn export_formats_cover_enabled_and_disabled_mods() {
        let sandbox = Sandbox::new("formats");
        let instance = sandbox.0.join("instance");
        let mods = instance.join("mods");
        fs::create_dir_all(&mods).unwrap();
        fs::write(mods.join("sodium.jar"), b"jar").unwrap();
        fs::write(mods.join("Old Mod.disabled"), b"jar").unwrap();
        fs::write(mods.join("notes.txt"), b"text").unwrap();
        let result = instance_mod_list_export(&json!({ "name": "x", "format": "markdown" })).is_err();
        assert!(result, "несуществующий экземпляр должен давать ошибку");
        let _ = result;
        // Через прямой вызов рендера: содержимое стабильного формата.
        let rows = vec![
            Row {
                file: "sodium.jar".into(),
                name: Some(json!("Sodium")),
                version: Some(json!("0.6.0")),
                mod_id: Some(json!("sodium")),
                loader: Some(json!("fabric")),
                disabled: false,
            },
            Row {
                file: "Old, Mod.disabled".into(),
                name: None,
                version: None,
                mod_id: None,
                loader: None,
                disabled: true,
            },
        ];
        let markdown = render(Format::Markdown, "Выживание, 1.21", "survival", &rows);
        assert!(markdown.starts_with("# Моды: Выживание, 1.21"));
        assert!(markdown.contains("| Sodium | 0.6.0 | sodium | fabric | `sodium.jar` | включен |"));
        assert!(markdown.contains("Old, Mod.disabled"));
        let csv = render(Format::Csv, "X", "x", &rows);
        assert!(csv.starts_with("mod,version,mod_id,loader,file,enabled\n"));
        assert!(csv.contains("\"Old, Mod.disabled\",false\n"), "{csv}");
        let text = render(Format::Text, "X", "x", &rows);
        assert!(text.contains("- Sodium 0.6.0"));
        assert_eq!(sanitize_file_name("My: World", Format::Csv), "My: World.csv");
        assert_eq!(Format::parse(Some("md")).unwrap(), Format::Markdown);
        assert!(Format::parse(Some("pdf")).is_err());
    }
}
