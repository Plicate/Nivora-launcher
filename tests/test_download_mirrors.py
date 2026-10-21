import unittest
from unittest.mock import patch

from nivora_launcher.errors import LauncherError
from nivora_launcher.net import HttpClient, _SourceUnavailable, mirror_url


class MirrorUrlTests(unittest.TestCase):
    def test_known_hosts_map_to_bmclapi(self) -> None:
        cases = {
            "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json": "https://bmclapi2.bangbang93.com/mc/game/version_manifest_v2.json",
            "https://piston-data.mojang.com/v1/objects/abc/client.jar": "https://bmclapi2.bangbang93.com/v1/objects/abc/client.jar",
            "https://resources.download.minecraft.net/ab/abcdef": "https://bmclapi2.bangbang93.com/assets/ab/abcdef",
            "https://libraries.minecraft.net/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar": "https://bmclapi2.bangbang93.com/maven/com/mojang/brigadier/1.0.18/brigadier-1.0.18.jar",
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/21.1.77/x.jar": "https://bmclapi2.bangbang93.com/maven/net/neoforged/neoforge/21.1.77/x.jar",
            "https://meta.fabricmc.net/v2/versions/loader/1.20.1?x=1": "https://bmclapi2.bangbang93.com/fabric-meta/v2/versions/loader/1.20.1?x=1",
        }
        for source, expected in cases.items():
            with self.subTest(source=source):
                self.assertEqual(mirror_url(source), expected)

    def test_unknown_hosts_and_prefix_tricks_are_not_mirrored(self) -> None:
        self.assertIsNone(mirror_url("https://api.modrinth.com/v2/search"))
        self.assertIsNone(mirror_url("https://maven.neoforged.net/releasesX/a.jar"))
        self.assertIsNone(mirror_url("http://piston-meta.mojang.com/a.json"))
        self.assertIsNone(mirror_url("https://piston-meta.mojang.com:8443/a.json"))


class MirrorFallbackTests(unittest.TestCase):
    def test_auto_falls_back_to_mirror_after_network_failure(self) -> None:
        client = HttpClient(retries=1)
        calls: list[str] = []

        def fake(url, **_kwargs):
            calls.append(url)
            if "mojang" in url:
                raise LauncherError("Сетевая ошибка")
            return b"ok"

        with patch.object(client, "_request_single", side_effect=fake):
            self.assertEqual(client.request("https://piston-meta.mojang.com/x.json"), b"ok")
        self.assertEqual(calls, ["https://piston-meta.mojang.com/x.json", "https://bmclapi2.bangbang93.com/x.json"])

    def test_official_404_is_final(self) -> None:
        client = HttpClient(retries=1)
        with patch.object(client, "_request_single", side_effect=_SourceUnavailable("HTTP 404")) as single:
            with self.assertRaises(_SourceUnavailable):
                client.request("https://piston-meta.mojang.com/missing.json")
        self.assertEqual(single.call_count, 1)

    def test_mirror_mode_tries_mirror_first_and_official_mode_never(self) -> None:
        mirror_first = HttpClient(retries=1, download_source="mirror")
        self.assertEqual(
            [url for url, _ in mirror_first._candidates("https://libraries.minecraft.net/a.jar")],
            ["https://bmclapi2.bangbang93.com/maven/a.jar", "https://libraries.minecraft.net/a.jar"],
        )
        official = HttpClient(retries=1, download_source="official")
        self.assertEqual(len(official._candidates("https://libraries.minecraft.net/a.jar")), 1)
        self.assertEqual(len(mirror_first._candidates("https://libraries.minecraft.net/a.jar", "POST")), 1)


if __name__ == "__main__":
    unittest.main()
