//! обновление ресурсов со снапшотами для отката

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

use super::net::atomic_json_write;

pub(crate) const SAFE_UPDATE_FILE: &str = ".trial-launcher-safe-updates.json";
pub(crate) const MAX_SAFE_UPDATE_BYTES: u64 = 256 * 1024;
pub(crate) const MAX_SESSIONS: usize = 10;
/// типы ресурсов для безопасного обновления
pub(crate) const PROJECT_TYPES: &[&str] = &["mod", "resourcepack", "shader"];
const SNAPSHOT_CATEGORY: &[(&str, &str)] = &[("mod", "mods"), ("resourcepack", "resourcepacks"), ("shader", "shaders")];

fn sessions_path(instance_root: &Path) -> PathBuf {
    instance_root.join(SAFE_UPDATE_FILE)
}

fn snapshot_category(project_type: &str) -> &'static str {
    SNAPSHOT_CATEGORY
        .iter()
        .find(|(kind, _)| *kind == project_type)
        .map(|(_, category)| *category)
        .unwrap_or("mods")
}

pub(crate) fn list_sessions(instance_root: &Path) -> Vec<Map<String, Value>> {
    let path = sessions_path(instance_root);
    let Ok(metadata) = std::fs::symlink_metadata(&path) else {
        return Vec::new();
    };
    if !metadata.is_file() || metadata.len() > MAX_SAFE_UPDATE_BYTES {
        return Vec::new();
    }
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let Some(rows) = value.as_array() else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(Value::as_object)
        .take(MAX_SESSIONS)
        .cloned()
        .collect()
}

fn write_sessions(instance_root: &Path, sessions: &[Map<String, Value>]) -> Result<(), String> {
    let trimmed: Vec<Value> = sessions.iter().take(MAX_SESSIONS).cloned().map(Value::Object).collect();
    atomic_json_write(&sessions_path(instance_root), &Value::Array(trimmed), true)
}

#[derive(Debug)]
pub(crate) struct SafeUpdateOutcome {
    pub(crate) session: Map<String, Value>,
    pub(crate) applied: Vec<Value>,
    pub(crate) failed: Vec<Value>,
    pub(crate) snapshots: Vec<(String, String)>,
}

/// сохраняет снапшоты перед обновлением; отдельная ошибка не останавливает остальные
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_safe_update<F, S>(
    instance_root: &Path,
    updates: &[Map<String, Value>],
    mut apply_update: F,
    mut create_snapshot: S,
    emit: impl Fn(&str, &str),
) -> Result<SafeUpdateOutcome, String>
where
    F: FnMut(&Map<String, Value>) -> Result<Map<String, Value>, String>,
    S: FnMut(&str) -> Result<Map<String, Value>, String>,
{
    if updates.is_empty() {
        return Err("Нет доступных обновлений".to_owned());
    }
    let mut touched: Vec<&str> = updates
        .iter()
        .map(|item| snapshot_category(item.get("project_type").and_then(Value::as_str).unwrap_or("mod")))
        .collect();
    touched.sort_unstable();
    touched.dedup();
    let mut snapshots: Vec<(String, String)> = Vec::new();
    for category in touched {
        let snapshot = create_snapshot(category)?;
        let id = snapshot.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
        snapshots.push((category.to_owned(), id));
        emit(
            "safe-update-progress",
            &format!("Снапшот {category} создан перед обновлением"),
        );
    }
    let session_id = format!(
        "{}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs())
            .unwrap_or_default(),
        super::mods::random_hex(4)
    );
    let mut applied: Vec<Value> = Vec::new();
    let mut failed: Vec<Value> = Vec::new();
    for update in updates {
        let label = update
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .or_else(|| update.get("project_id").and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned();
        match apply_update(update) {
            Ok(result) => {
                applied.push(json!({
                    "project_id": result.get("project_id").and_then(Value::as_str)
                        .or_else(|| update.get("project_id").and_then(Value::as_str)).unwrap_or_default(),
                    "name": result.get("name").and_then(Value::as_str).unwrap_or(&label),
                    "version": result.get("version").and_then(Value::as_str).unwrap_or_default(),
                    "version_id": result.get("version_id").and_then(Value::as_str).unwrap_or_default(),
                }));
                emit("safe-update-progress", &format!("Обновлён {label}"));
            }
            Err(error) => {
                failed.push(json!({ "name": label, "error": error }));
                emit("safe-update-progress", &format!("Не обновился {label}: {error}"));
            }
        }
    }
    let mut session = Map::new();
    session.insert("id".to_owned(), json!(session_id));
    session.insert(
        "created_at".to_owned(),
        json!(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_secs_f64())
            .unwrap_or_default()),
    );
    session.insert(
        "snapshots".to_owned(),
        json!(snapshots
            .iter()
            .map(|(category, id)| json!({ category.clone(): id.clone() }))
            .fold(Map::new(), |mut acc, value| {
                if let Some(source) = value.as_object() {
                    for (key, item) in source {
                        acc.insert(key.clone(), item.clone());
                    }
                }
                acc
            })),
    );
    session.insert("applied".to_owned(), Value::Array(applied.clone()));
    session.insert("failed".to_owned(), Value::Array(failed.clone()));
    session.insert("rolled_back".to_owned(), json!(false));
    let mut sessions = vec![session.clone()];
    sessions.extend(list_sessions(instance_root));
    write_sessions(instance_root, &sessions)?;
    Ok(SafeUpdateOutcome { session, applied, failed, snapshots })
}

#[derive(Debug)]
pub(crate) struct RollbackOutcome {
    pub(crate) session_id: String,
    pub(crate) restored: Vec<String>,
    pub(crate) errors: Vec<Value>,
}

/// восстанавливает снапшоты сессии обновления
pub(crate) fn rollback<F>(instance_root: &Path, session_id: &str, mut restore_snapshot: F) -> Result<RollbackOutcome, String>
where
    F: FnMut(&str, &str) -> Result<Map<String, Value>, String>,
{
    let mut sessions = list_sessions(instance_root);
    let position = sessions
        .iter()
        .position(|item| item.get("id").and_then(Value::as_str).unwrap_or_default() == session_id)
        .ok_or_else(|| "Сессия безопасного обновления не найдена".to_owned())?;
    if sessions[position].get("rolled_back").and_then(Value::as_bool).unwrap_or(false) {
        return Err("Эта сессия уже откачена".to_owned());
    }
    let mut pairs: Vec<(String, String)> = sessions[position]
        .get("snapshots")
        .and_then(Value::as_object)
        .map(|snapshots| {
            snapshots
                .iter()
                .map(|(category, id)| (category.clone(), id.as_str().unwrap_or_default().to_owned()))
                .collect()
        })
        .unwrap_or_default();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let mut restored: Vec<String> = Vec::new();
    let mut errors: Vec<Value> = Vec::new();
    for (category, snapshot_id) in pairs {
        match restore_snapshot(&category, &snapshot_id) {
            Ok(_) => restored.push(category),
            Err(error) => errors.push(json!({ "category": category, "error": error })),
        }
    }
    if restored.is_empty() && !errors.is_empty() {
        return Err(format!(
            "Откат не удался: {}",
            errors[0].get("error").and_then(Value::as_str).unwrap_or_default()
        ));
    }
    for item in sessions.iter_mut() {
        if item.get("id").and_then(Value::as_str).unwrap_or_default() == session_id {
            item.insert("rolled_back".to_owned(), json!(errors.is_empty()));
            if errors.is_empty() {
                item.insert(
                    "rolled_back_at".to_owned(),
                    json!(std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|value| value.as_secs_f64())
                        .unwrap_or_default()),
                );
            } else {
                item.remove("rolled_back_at");
            }
        }
    }
    write_sessions(instance_root, &sessions)?;
    Ok(RollbackOutcome { session_id: session_id.to_owned(), restored, errors })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nivora-safe-update-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn session_with(id: &str, rolled_back: bool) -> Map<String, Value> {
        let mut session = Map::new();
        session.insert("id".to_owned(), json!(id));
        session.insert("snapshots".to_owned(), json!({ "mods": "abc" }));
        session.insert("rolled_back".to_owned(), json!(rolled_back));
        session
    }

    #[test]
    fn run_creates_session_and_rolls_failures_into_failed() {
        let root = tmpdir("run");
        let updates = vec![
            {
                let mut item = Map::new();
                item.insert("project_id".to_owned(), json!("a"));
                item.insert("project_type".to_owned(), json!("mod"));
                item.insert("name".to_owned(), json!("Alpha"));
                item
            },
            {
                let mut item = Map::new();
                item.insert("project_id".to_owned(), json!("b"));
                item.insert("project_type".to_owned(), json!("shader"));
                item
            },
        ];
        let outcome = run_safe_update(
            &root,
            &updates,
            |update| {
                if update.get("project_id").and_then(Value::as_str) == Some("a") {
                    let mut result = Map::new();
                    result.insert("project_id".to_owned(), json!("a"));
                    result.insert("name".to_owned(), json!("Alpha"));
                    result.insert("version".to_owned(), json!("2"));
                    result.insert("version_id".to_owned(), json!("v2"));
                    Ok(result)
                } else {
                    Err("нет файла".to_owned())
                }
            },
            |category| {
                let mut snapshot = Map::new();
                snapshot.insert("id".to_owned(), json!(format!("snap-{category}")));
                Ok(snapshot)
            },
            |_, _| {},
        )
        .unwrap();
        assert_eq!(outcome.applied.len(), 1);
        assert_eq!(outcome.failed.len(), 1);
        assert!(outcome.snapshots.iter().any(|(category, _)| category == "shaders"));
        let sessions = list_sessions(&root);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].get("applied").and_then(Value::as_array).map(Vec::len), Some(1));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rollback_restores_and_marks_session() {
        let root = tmpdir("rollback");
        write_sessions(&root, &[session_with("s1", false), session_with("s2", true)]).unwrap();
        let outcome = rollback(&root, "s1", |category, id| {
            assert_eq!(category, "mods");
            assert_eq!(id, "abc");
            let mut restored = Map::new();
            restored.insert("id".to_owned(), json!(id));
            Ok(restored)
        })
        .unwrap();
        assert_eq!(outcome.restored, vec!["mods".to_owned()]);
        let sessions = list_sessions(&root);
        assert!(sessions[0].get("rolled_back").and_then(Value::as_bool).unwrap());
        let error = rollback(&root, "s1", |_, _| Ok(Map::new())).unwrap_err();
        assert_eq!(error, "Эта сессия уже откачена");
        let error = rollback(&root, "missing", |_, _| Ok(Map::new())).unwrap_err();
        assert_eq!(error, "Сессия безопасного обновления не найдена");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn partial_rollback_remains_retryable_until_every_category_restores() {
        let root = tmpdir("rollback-partial");
        let mut session = session_with("s1", false);
        session.insert("snapshots".to_owned(), json!({ "mods": "mods-snap", "shaders": "shaders-snap" }));
        write_sessions(&root, &[session]).unwrap();

        let partial = rollback(&root, "s1", |category, _| {
            if category == "shaders" {
                Err("snapshot unavailable".to_owned())
            } else {
                Ok(Map::new())
            }
        })
        .unwrap();
        assert_eq!(partial.restored, vec!["mods".to_owned()]);
        assert_eq!(partial.errors.len(), 1);
        assert_eq!(partial.errors[0]["category"], json!("shaders"));
        let sessions = list_sessions(&root);
        assert_eq!(sessions[0].get("rolled_back").and_then(Value::as_bool), Some(false));
        assert!(sessions[0].get("rolled_back_at").is_none());

        let retried = rollback(&root, "s1", |_, _| Ok(Map::new())).unwrap();
        assert_eq!(retried.restored, vec!["mods".to_owned(), "shaders".to_owned()]);
        assert!(retried.errors.is_empty());
        let sessions = list_sessions(&root);
        assert_eq!(sessions[0].get("rolled_back").and_then(Value::as_bool), Some(true));
        assert!(sessions[0].get("rolled_back_at").is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_updates_rejected() {
        let root = tmpdir("empty");
        let error = run_safe_update(&root, &[], |_| Ok(Map::new()), |_| Ok(Map::new()), |_, _| {}).unwrap_err();
        assert_eq!(error, "Нет доступных обновлений");
        let _ = std::fs::remove_dir_all(&root);
    }
}
