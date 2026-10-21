from __future__ import annotations

import re

APP_ID = "nivora-launcher"
APP_NAME = "Nivora Launcher"
APP_VERSION = "1.1.0"
# репозиторий проекта: фолбэк-проверка обновлений ходит в GitHub Releases API,
# когда файл latest.json в релизе ещё не выложен (ручная загрузка пакетов).
GITHUB_OWNER = "vedrooou0"
GITHUB_REPO = "Nivora-launcher"
GITHUB_RELEASES_API_URL = f"https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}/releases/latest"
GITHUB_RELEASES_PAGE_URL = f"https://github.com/{GITHUB_OWNER}/{GITHUB_REPO}/releases/latest"
# метка сборки: фронтенд (FRONTEND_BUILD_STAMP в bridge.ts) сверяет её с bootstrap-ответом.
# рассинхрон = в приложение подменили часть файлов; молчать об этом нельзя.
BUILD_STAMP = "r40"
# зарегистрированный Azure public-client ID лаунчера; на чужие (Mojang) ID не падаем никогда.
DEFAULT_MSA_CLIENT_ID = "ce777973-8920-4993-8b21-4a06dab23ba6"
LEGACY_MSA_CLIENT_IDS = frozenset({"00000000402b5328", "c36a9fb6-4f2a-41ff-90bd-ae7cc92031eb"})
MSA_LOGIN_API_URL = "https://login.microsoftonline.com/consumers/oauth2/v2.0"

VERSION_MANIFEST_URL = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
JAVA_RUNTIME_MANIFEST_URL = (
    "https://piston-meta.mojang.com/v1/products/java-runtime/"
    "2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json"
)
MINECRAFT_ASSET_BASE_URL = "https://resources.download.minecraft.net"
MINECRAFT_LIBRARY_BASE_URL = "https://libraries.minecraft.net"
FABRIC_META_URL = "https://meta.fabricmc.net"
QUILT_META_URL = "https://meta.quiltmc.org"
MODRINTH_API_URL = "https://api.modrinth.com/v2"
FTB_MODPACK_API_URL = "https://api.feed-the-beast.com/v1/modpacks/public"
TECHNIC_API_URL = "https://api.technicpack.net"
MAX_INSTANCE_VERSION_JSON_BYTES = 8 * 1024 * 1024
MAX_INSTANCE_PROFILE_JSON_BYTES = 256 * 1024
MAX_INSTANCE_LIVE_STATE_JSON_BYTES = 64 * 1024
MAX_SERVER_CONFIG_JSON_BYTES = 256 * 1024

MINECRAFT_RELEASE_RE = re.compile(
    r"^(?P<base>[0-9]+(?:\.[0-9]+){1,2})(?:-(?:snapshot|rc|pre)(?:-?[0-9]+)?)?$"
)
MINECRAFT_WEEKLY_RE = re.compile(r"^(?P<year>[0-9]{2})w[0-9]{2}[a-z]$")
MINECRAFT_ALPHA_BETA_RE = re.compile(r"^[ab][0-9]+\.[0-9]+(?:\.[0-9]+)?(?:_[0-9]+)?$")

def version_key(version: str) -> tuple[int, ...]:
    """Return a numeric key suitable for the supported version range."""
    match = MINECRAFT_RELEASE_RE.fullmatch(version)
    if match:
        return tuple(int(part) for part in match.group("base").split("."))
    weekly = MINECRAFT_WEEKLY_RE.fullmatch(version)
    if weekly:
        return (int(weekly.group("year")), 0)
    return ()

def is_supported_version(version: str) -> bool:
    if MINECRAFT_ALPHA_BETA_RE.fullmatch(version):
        return True
    key = version_key(version)
    # the official manifest is the source of truth; a baked-in upper bound would
    # silently hide every future release until the launcher itself is rebuilt.
    return bool(key) and key >= version_key("1.0")
