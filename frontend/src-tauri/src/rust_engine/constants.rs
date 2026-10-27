pub const APP_NAME: &str = "Nivora Launcher";
// версия должна совпадать с frontend/package.json и Cargo.toml
pub const APP_VERSION: &str = "2.0.0";
pub const BUILD_STAMP: &str = "r42";

pub const GITHUB_RELEASES_API_URL: &str = "https://api.github.com/repos/vedrooou0/Nivora-launcher/releases/latest";
pub const GITHUB_RELEASES_PAGE_URL: &str = "https://github.com/vedrooou0/Nivora-launcher/releases/latest";

pub const DEFAULT_MSA_CLIENT_ID: &str = "ce777973-8920-4993-8b21-4a06dab23ba6";
pub const LEGACY_MSA_CLIENT_IDS: &[&str] = &["00000000402b5328", "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb"];
pub const MSA_LOGIN_API_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";

pub const VERSION_MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
pub const JAVA_RUNTIME_MANIFEST_URL: &str =
    "https://piston-meta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
pub const MINECRAFT_ASSET_BASE_URL: &str = "https://resources.download.minecraft.net";
pub const MINECRAFT_LIBRARY_BASE_URL: &str = "https://libraries.minecraft.net";
pub const FABRIC_META_URL: &str = "https://meta.fabricmc.net";
pub const QUILT_META_URL: &str = "https://meta.quiltmc.org";
pub const MODRINTH_API_URL: &str = "https://api.modrinth.com/v2";
pub const FTB_MODPACK_API_URL: &str = "https://api.feed-the-beast.com/v1/modpacks/public";
pub const TECHNIC_API_URL: &str = "https://api.technicpack.net";

pub const MAX_INSTANCE_VERSION_JSON_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_INSTANCE_PROFILE_JSON_BYTES: u64 = 256 * 1024;
pub const MAX_INSTANCE_LIVE_STATE_JSON_BYTES: u64 = 64 * 1024;
pub const MAX_SERVER_CONFIG_JSON_BYTES: u64 = 256 * 1024;

/// числовой ключ версии для сравнения поддерживаемого диапазона
pub fn version_key(version: &str) -> Vec<i64> {
    let release = regex::Regex::new(r"^([0-9]+(?:\.[0-9]+){1,2})(?:-(?:snapshot|rc|pre)(?:-?[0-9]+)?)?\z").unwrap();
    if let Some(captures) = release.captures(version) {
        return captures
            .get(1)
            .unwrap()
            .as_str()
            .split('.')
            .map(|part| part.parse::<i64>().unwrap_or(0))
            .collect();
    }
    let weekly = regex::Regex::new(r"^([0-9]{2})w[0-9]{2}[a-z]\z").unwrap();
    if let Some(captures) = weekly.captures(version) {
        let year: i64 = captures.get(1).unwrap().as_str().parse().unwrap_or(0);
        return vec![year, 0];
    }
    Vec::new()
}

pub fn is_supported_version(version: &str) -> bool {
    let alpha_beta = regex::Regex::new(r"^[ab][0-9]+\.[0-9]+(?:\.[0-9]+)?(?:_[0-9]+)?\z").unwrap();
    if alpha_beta.is_match(version) {
        return true;
    }
    let key = version_key(version);
    // верхняя граница берётся из официального манифеста
    !key.is_empty() && key >= version_key("1.0")
}
