from __future__ import annotations

try:
    from defusedxml import ElementTree as ET
except ImportError:  # pragma: no cover - the package is a release dependency
    import xml.etree.ElementTree as ET
from dataclasses import dataclass
from typing import Any
from urllib.parse import quote

from .constants import FABRIC_META_URL, QUILT_META_URL
from .errors import LauncherError
from .net import HttpClient


@dataclass
class LoaderChoice:
    loader: str
    version: str
    installer_url: str = ""


class LoaderRepository:
    def __init__(self, http: HttpClient | None = None) -> None:
        self.http = http or HttpClient()

    def profile(self, loader: str, minecraft_version: str, loader_version: str | None = None) -> dict[str, Any]:
        if loader == "fabric":
            return self._fabric_profile(minecraft_version, loader_version)
        if loader == "quilt":
            return self._quilt_profile(minecraft_version, loader_version)
        raise LauncherError(f"JSON-профиль для loader пока не описан: {loader}")

    def choices(self, loader: str, minecraft_version: str) -> list[LoaderChoice]:
        if loader == "fabric":
            value = self.http.get_json(f"{FABRIC_META_URL}/v2/versions/loader/{quote(minecraft_version, safe='')}")
            choices = []
            for item in value if isinstance(value, list) else []:
                loader_data = item.get("loader", item) if isinstance(item, dict) else {}
                version = loader_data.get("version")
                if version:
                    choices.append(LoaderChoice("fabric", str(version)))
            return choices
        if loader == "quilt":
            value = self.http.get_json(f"{QUILT_META_URL}/v3/versions/loader/{quote(minecraft_version, safe='')}")
            choices = []
            for item in value if isinstance(value, list) else []:
                loader_data = item.get("loader", item) if isinstance(item, dict) else {}
                version = loader_data.get("version") if isinstance(loader_data, dict) else None
                if version:
                    choices.append(LoaderChoice("quilt", str(version)))
            return choices
        if loader == "forge":
            return self._forge_choices(minecraft_version)
        if loader == "neoforge":
            return self._neoforge_choices(minecraft_version)
        raise LauncherError(f"Неизвестный loader: {loader}")

    def installer_url(self, loader: str, minecraft_version: str, loader_version: str) -> str:
        if loader == "forge":
            coordinate = loader_version if loader_version.startswith(f"{minecraft_version}-") else f"{minecraft_version}-{loader_version}"
            return (
                "https://maven.minecraftforge.net/net/minecraftforge/forge/"
                f"{coordinate}/forge-{coordinate}-installer.jar"
            )
        if loader == "neoforge":
            return (
                "https://maven.neoforged.net/releases/net/neoforged/neoforge/"
                f"{loader_version}/neoforge-{loader_version}-installer.jar"
            )
        raise LauncherError(f"У loader {loader} нет installer URL")

    def _fabric_profile(self, minecraft_version: str, loader_version: str | None) -> dict[str, Any]:
        if not loader_version:
            choices = self.choices("fabric", minecraft_version)
            if not choices:
                raise LauncherError(f"Fabric не поддерживает Minecraft {minecraft_version}")
            loader_version = choices[0].version
        url = (
            f"{FABRIC_META_URL}/v2/versions/loader/{quote(minecraft_version, safe='')}/"
            f"{quote(loader_version, safe='')}/profile/json"
        )
        return self._validate_profile(self.http.get_json(url), "fabric")

    def _quilt_profile(self, minecraft_version: str, loader_version: str | None) -> dict[str, Any]:
        if not loader_version:
            choices = self.choices("quilt", minecraft_version)
            if not choices:
                raise LauncherError(f"Quilt не поддерживает Minecraft {minecraft_version}")
            loader_version = choices[0].version
        url = (
            f"{QUILT_META_URL}/v3/versions/loader/{quote(minecraft_version, safe='')}/"
            f"{quote(loader_version, safe='')}/profile/json"
        )
        return self._validate_profile(self.http.get_json(url), "quilt")

    def _validate_profile(self, profile: Any, loader: str) -> dict[str, Any]:
        if not isinstance(profile, dict) or not profile.get("id") or not profile.get("inheritsFrom"):
            raise LauncherError(f"{loader} вернул некорректный launcher profile")
        return profile

    def _forge_choices(self, minecraft_version: str) -> list[LoaderChoice]:
        url = "https://files.minecraftforge.net/net/minecraftforge/forge/maven-metadata.json"
        value = self.http.get_json(url)
        versions: list[str] = []
        if isinstance(value, dict):
            raw = value.get(minecraft_version, [])
            if isinstance(raw, list):
                versions = [str(item) for item in raw]
        return [LoaderChoice("forge", item) for item in versions]

    def _neoforge_choices(self, minecraft_version: str) -> list[LoaderChoice]:
        url = "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml"
        raw = self.http.request(url).decode("utf-8", errors="replace")
        if "<!DOCTYPE" in raw.upper() or "<!ENTITY" in raw.upper():
            raise LauncherError("NeoForge metadata содержит запрещённые XML-декларации")
        try:
            root = ET.fromstring(raw)
        except ET.ParseError as exc:
            raise LauncherError("NeoForge metadata имеет некорректный XML") from exc
        values = [node.text for node in root.findall(".//version") if node.text]
        # NeoForge's version scheme encodes the complete MC minor line:
        # 1.20.1 -> 20.1.x, 1.21.1 -> 21.1.x, and 1.21 -> 21.0.x.
        parts = minecraft_version.split(".")
        if len(parts) < 2:
            return []
        minor_line = parts[2] if len(parts) > 2 else "0"
        prefix = f"{parts[1]}.{minor_line}."
        return [LoaderChoice("neoforge", item) for item in values if item.startswith(prefix)]
