// нативный bootstrap: стартовое состояние приложения без Python-воркера.
use serde_json::{json, Value};

use super::accounts;
use super::config::ConfigStore;
use super::constants::BUILD_STAMP;
use super::instances::InstanceStore;
use super::plugin_catalogs::normalize_plugin_catalogs;
use super::plugins::PluginStore;
use super::system;

pub fn bootstrap(_request: &Value) -> Result<Value, String> {
    let config = ConfigStore::load()?;
    let instances: Vec<Value> = InstanceStore::new(&config)?.list().iter().map(|info| info.public()).collect();
    let accounts_list = accounts::public_accounts(&config.paths.config_home)?;
    let plugins = PluginStore::new(&config)?.list()?;
    let cores = system::server_cores(&json!({}))?
        .get("cores")
        .cloned()
        .unwrap_or_else(|| json!([]));
    Ok(json!({
        "build_stamp": BUILD_STAMP,
        "instances": instances,
        "accounts": accounts_list,
        "plugins": plugins,
        "server_cores": cores,
        "plugin_catalogs": normalize_plugin_catalogs(&config.object_setting("plugin_catalogs")),
        "active_plugin_catalog_id": config.str_setting("active_plugin_catalog_id"),
        "settings": config.settings_public(),
        "system": system_block(),
    }))
}

fn system_block() -> Value {
    let platform = if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };
    let machine = std::env::consts::ARCH;
    let which = |name: &str| {
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path).any(|directory| directory.join(name).is_file())
    };
    let security = system::security_status(&json!({})).unwrap_or_else(|_| json!({}));
    let mut security = match security {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    security.insert(
        "sandbox".to_owned(),
        json!(if cfg!(windows) {
            "appcontainer"
        } else if security.get("bubblewrap").and_then(Value::as_bool).unwrap_or(false) {
            "bubblewrap"
        } else {
            "none"
        }),
    );
    json!({
        "platform": platform,
        "machine": machine,
        "python": "rust",
        "session": std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_owned()),
        "gdk_backend": std::env::var("GDK_BACKEND").unwrap_or_default(),
        "display": std::env::var("WAYLAND_DISPLAY")
            .or_else(|_| std::env::var("DISPLAY"))
            .unwrap_or_else(|_| "none".to_owned()),
        "nvidia_smi": which("nvidia-smi"),
        "glxinfo": which("glxinfo"),
        "security": Value::Object(security),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_block_shape_matches_python_contract() {
        let block = system_block();
        for key in ["platform", "machine", "python", "session", "gdk_backend", "display", "nvidia_smi", "glxinfo", "security"] {
            assert!(block.get(key).is_some(), "нет ключа {key}");
        }
        // платформа — те же имена, что даёт platform.system() в Python
        assert!(["Windows", "Darwin", "Linux"].contains(&block["platform"].as_str().unwrap()));
        assert_eq!(block["machine"].as_str().unwrap(), std::env::consts::ARCH);
        let security = block["security"].as_object().unwrap();
        assert!(security.contains_key("sandbox"));
        assert!(["appcontainer", "bubblewrap", "none"].contains(&security["sandbox"].as_str().unwrap()));
    }
}
