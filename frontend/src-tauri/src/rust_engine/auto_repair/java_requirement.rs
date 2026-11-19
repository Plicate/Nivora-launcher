//! Требования Java из локальных метаданных профиля и родительской версии.
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Debug, PartialEq)]
pub(super) struct JavaRequirement {
    pub major: i64,
    pub component: Option<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id.len() <= 200
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '+'))
}

pub(super) fn from_metadata(
    instance: &Path,
    version_id: &str,
    game_version: &str,
) -> Option<JavaRequirement> {
    let versions = instance.join("versions");
    if fs::symlink_metadata(&versions)
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    let mut visited = HashSet::new();
    for start in [version_id, game_version] {
        let mut current = start.trim().to_owned();
        for _ in 0..8 {
            if !valid_id(&current) || !visited.insert(current.clone()) {
                break;
            }
            let dir = versions.join(&current);
            let path = dir.join(format!("{current}.json"));
            if fs::symlink_metadata(&dir)
                .ok()
                .is_none_or(|m| m.file_type().is_symlink() || !m.is_dir())
            {
                break;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else {
                break;
            };
            if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > 4 * 1024 * 1024 {
                break;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                break;
            };
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                break;
            };
            if let Some(java) = value.get("javaVersion") {
                if let Some(major) = java
                    .get("majorVersion")
                    .and_then(Value::as_i64)
                    .filter(|v| (8..=100).contains(v))
                {
                    let component = java
                        .get("component")
                        .and_then(Value::as_str)
                        .filter(|s| valid_id(s))
                        .map(str::to_owned);
                    return Some(JavaRequirement { major, component });
                }
            }
            current = value
                .get("inheritsFrom")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_owned();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn follows_parent_and_rejects_unsafe_or_cyclic_ids() {
        let root = std::env::temp_dir().join(format!("nivora-java-meta-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for id in ["forge", "vanilla", "cycle"] {
            fs::create_dir_all(root.join("versions").join(id)).unwrap();
        }
        fs::write(
            root.join("versions/forge/forge.json"),
            r#"{"inheritsFrom":"vanilla"}"#,
        )
        .unwrap();
        fs::write(
            root.join("versions/vanilla/vanilla.json"),
            r#"{"javaVersion":{"majorVersion":21,"component":"java-runtime-delta"}}"#,
        )
        .unwrap();
        fs::write(
            root.join("versions/cycle/cycle.json"),
            r#"{"inheritsFrom":"cycle"}"#,
        )
        .unwrap();
        assert_eq!(from_metadata(&root, "forge", "").unwrap().major, 21);
        assert_eq!(
            from_metadata(&root, "missing", "vanilla")
                .unwrap()
                .component
                .as_deref(),
            Some("java-runtime-delta")
        );
        assert!(from_metadata(&root, "../vanilla", "").is_none());
        assert!(from_metadata(&root, "cycle", "").is_none());
        let _ = fs::remove_dir_all(root);
    }
}
