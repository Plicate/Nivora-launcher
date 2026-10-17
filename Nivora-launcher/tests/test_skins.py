from __future__ import annotations

import json
import unittest

from nivora_launcher.accounts import Account
from nivora_launcher.errors import LauncherError
from nivora_launcher.skins import PROFILE_URL, apply_cape, profile


def _skin_png(width: int = 64, height: int = 64) -> bytes:
    return b"\x89PNG\r\n\x1a\n" + b"\x00\x00\x00\rIHDR" + width.to_bytes(4, "big") + height.to_bytes(4, "big")


class FakeHttp:
    def __init__(self, profile_data: dict) -> None:
        self.profile_data = profile_data
        self.calls: list[tuple] = []

    def get_json(self, url: str, *, headers: dict[str, str] | None = None):
        self.calls.append(("get_json", url, headers))
        return json.loads(json.dumps(self.profile_data))

    def request(
        self,
        url: str,
        method: str = "GET",
        data: bytes | None = None,
        headers: dict[str, str] | None = None,
        max_bytes: int | None = None,
    ) -> bytes:
        self.calls.append(("request", url, method, data, headers, max_bytes))
        return _skin_png() if "skin.png" in url else b"\x89PNG\r\n\x1a\nCape texture"


class UnauthorizedOnceHttp(FakeHttp):
    def __init__(self, profile_data: dict) -> None:
        super().__init__(profile_data)
        self.profile_reads = 0

    def get_json(self, url: str, *, headers: dict[str, str] | None = None):
        self.calls.append(("get_json", url, headers))
        self.profile_reads += 1
        if self.profile_reads == 1:
            raise LauncherError(f"HTTP 401 при запросе {url}")
        return json.loads(json.dumps(self.profile_data))


def _account() -> Account:
    return Account("microsoft-test", "microsoft", "Player", "uuid", "access-token")


class SkinProfileTests(unittest.TestCase):
    def test_profile_returns_mojang_active_skin_and_cape_images(self) -> None:
        http = FakeHttp(
            {
                "name": "Player",
                "skins": [
                    {"id": "active-skin", "state": "ACTIVE", "variant": "SLIM", "url": "https://textures.example/skin.png"}
                ],
                "capes": [
                    {"id": "owned-cape", "state": "ACTIVE", "alias": "Migrator", "url": "https://textures.example/cape.png"}
                ],
            }
        )

        result = profile(http, _account())

        self.assertEqual(result["skins"][0]["state"], "ACTIVE")
        self.assertEqual(result["skins"][0]["variant"], "SLIM")
        self.assertTrue(result["skins"][0]["url"].startswith("data:image/png;base64,"))
        self.assertEqual(result["capes"][0]["id"], "owned-cape")
        self.assertTrue(result["capes"][0]["url"].startswith("data:image/png;base64,"))
        self.assertEqual(http.calls[0][1], PROFILE_URL)
        self.assertEqual(http.calls[0][2], {"Authorization": "Bearer access-token"})

    def test_apply_cape_accepts_an_owned_inactive_cape(self) -> None:
        http = FakeHttp({"capes": [{"id": "owned-cape", "state": "INACTIVE"}]})

        apply_cape(http, _account(), "owned-cape")

        put = next(call for call in http.calls if call[0] == "request" and call[2] == "PUT")
        self.assertEqual(put[1], "https://api.minecraftservices.com/minecraft/profile/capes/active")
        self.assertEqual(json.loads(put[3]), {"capeId": "owned-cape"})

    def test_apply_cape_rejects_a_cape_not_owned_by_the_profile(self) -> None:
        http = FakeHttp({"capes": [{"id": "owned-cape", "state": "INACTIVE"}]})

        with self.assertRaisesRegex(LauncherError, "профиле Minecraft"):
            apply_cape(http, _account(), "custom-cape")

        self.assertFalse(any(call[0] == "request" and call[2] == "PUT" for call in http.calls))


    def test_profile_marks_first_skin_and_cape_active_when_api_omits_state(self) -> None:
        http = FakeHttp(
            {
                "name": "Player",
                "skins": [{"id": "skin-1", "variant": "CLASSIC", "url": "https://textures.example/skin.png"}],
                "capes": [{"alias": "Migrator", "url": "https://textures.example/cape.png"}],
            }
        )

        result = profile(http, _account())

        self.assertEqual(result["skins"][0]["state"], "ACTIVE")
        self.assertEqual(result["capes"][0]["state"], "ACTIVE")
        self.assertEqual(result["capes"][0]["name"], "Migrator")

    def test_profile_retries_once_with_refreshed_token_after_unauthorized_response(self) -> None:
        http = UnauthorizedOnceHttp(
            {
                "name": "Player",
                "skins": [{"id": "active-skin", "state": "ACTIVE", "url": "https://textures.example/skin.png"}],
                "capes": [{"id": "owned-cape", "state": "ACTIVE", "url": "https://textures.example/cape.png"}],
            }
        )
        refreshed = Account("microsoft-test", "microsoft", "Player", "uuid", "fresh-token")
        refresh_calls: list[bool] = []

        result = profile(http, _account(), refresh_account=lambda: (refresh_calls.append(True) or refreshed))

        self.assertEqual(refresh_calls, [True])
        self.assertEqual(http.profile_reads, 2)
        profile_calls = [call for call in http.calls if call[0] == "get_json"]
        self.assertEqual(profile_calls[0][2], {"Authorization": "Bearer access-token"})
        self.assertEqual(profile_calls[1][2], {"Authorization": "Bearer fresh-token"})
        self.assertEqual(result["skins"][0]["state"], "ACTIVE")
        self.assertEqual(result["capes"][0]["id"], "owned-cape")


if __name__ == "__main__":
    unittest.main()
