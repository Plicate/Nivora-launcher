from __future__ import annotations

import io
import json
import os
import tempfile
import time
import unittest
from contextlib import contextmanager, redirect_stdout
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from nivora_launcher import cli
from nivora_launcher.accounts import Account, AccountStore, MicrosoftAuthenticator
from nivora_launcher.config import ConfigStore, Paths
from nivora_launcher.errors import AuthenticationError
from nivora_launcher.gui_service import _microsoft_account, _select_account as gui_select_account


@contextmanager
def _configured_config():
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        with patch.dict(
            os.environ,
            {
                "XDG_CONFIG_HOME": str(root / "config"),
                "XDG_DATA_HOME": str(root / "data"),
                "XDG_CACHE_HOME": str(root / "cache"),
            },
            clear=False,
        ):
            yield ConfigStore(Paths())


@contextmanager
def _secret_service():
    items: dict[str, dict[str, str]] = {}

    def run(command, **kwargs):
        operation = command[1]
        arguments = command[2:]
        account_id = arguments[arguments.index("account-id") + 1]
        if operation == "clear":
            existed = items.pop(account_id, None) is not None
            return SimpleNamespace(returncode=0 if existed else 1, stdout="", stderr="")
        if operation == "store":
            items[account_id] = json.loads(kwargs["input"])
            return SimpleNamespace(returncode=0, stdout="", stderr="")
        if operation == "lookup":
            if account_id not in items:
                return SimpleNamespace(returncode=1, stdout="", stderr="")
            return SimpleNamespace(returncode=0, stdout=json.dumps(items[account_id]), stderr="")
        raise AssertionError(command)

    with patch(
        "nivora_launcher.accounts.shutil.which",
        side_effect=lambda name: "/usr/bin/secret-tool" if name == "secret-tool" else None,
    ), patch("nivora_launcher.accounts.subprocess.run", side_effect=run):
        yield items


def _account(
    account_id: str,
    username: str,
    *,
    access_token: str | None = None,
    refresh_token: str | None = None,
    expires_at: float | None = None,
    alias: str = "",
    is_default: bool = False,
    profile_id: str | None = None,
) -> Account:
    return Account(
        account_id=account_id,
        kind="microsoft",
        username=username,
        uuid=profile_id or account_id,
        access_token=access_token or f"access-{account_id}",
        expires_at=time.time() + 3600 if expires_at is None else expires_at,
        refresh_token=refresh_token or f"refresh-{account_id}",
        alias=alias,
        is_default=is_default,
    )


class MultipleMicrosoftAccountTests(unittest.TestCase):
    def test_device_login_without_client_id_fails_before_network(self) -> None:
        with _configured_config() as config:
            config.settings.client_id = ""
            requests: list[str] = []

            class FakeHttp:
                def post_form(self, url, _data):
                    requests.append(url)
                    raise AssertionError("OAuth must not start without a client ID")

            authenticator = MicrosoftAuthenticator(config, AccountStore(config), FakeHttp())
            with self.assertRaisesRegex(AuthenticationError, "public client ID"):
                authenticator.login_device_code()

        self.assertEqual(requests, [])

    def test_minecraft_profile_id_is_the_new_account_id(self) -> None:
        with _configured_config() as config:
            class FakeHttp:
                def post_json(self, url, _payload, headers=None):
                    if url.endswith("/user/authenticate"):
                        return {"Token": "xbl-token"}
                    if url.endswith("/xsts/authorize"):
                        return {"Token": "xsts-token", "DisplayClaims": {"xui": [{"uhs": "user-hash"}]}}
                    return {"access_token": "minecraft-token", "expires_in": 3600, "xuid": "xuid"}

            authenticator = MicrosoftAuthenticator(config, AccountStore(config), FakeHttp())
            authenticator._get_profile = lambda _token: {"id": "AABBCCDDEEFF00112233445566778899", "name": "ProfileA"}
            authenticator._check_ownership = lambda _token: None

            account = authenticator._finish_microsoft_login({"access_token": "msa-token"}, "client-id")

        self.assertEqual(account.account_id, "aabbccddeeff00112233445566778899")
        self.assertEqual(account.uuid, "AABBCCDDEEFF00112233445566778899")

    def test_profiles_keep_tokens_in_separate_secret_service_entries(self) -> None:
        with _configured_config() as config, _secret_service() as secrets:
            store = AccountStore(config)
            store.save(_account("profile-a", "PlayerA", access_token="access-a", refresh_token="refresh-a"))
            store.save(_account("profile-b", "PlayerB", access_token="access-b", refresh_token="refresh-b"))

            accounts = {account.account_id: account for account in store.all()}
            records = json.loads(config.paths.accounts_file.read_text(encoding="utf-8"))

        self.assertEqual(set(accounts), {"profile-a", "profile-b"})
        self.assertEqual(accounts["profile-a"].access_token, "access-a")
        self.assertEqual(accounts["profile-b"].access_token, "access-b")
        self.assertEqual(secrets["profile-a"]["refresh_token"], "refresh-a")
        self.assertEqual(secrets["profile-b"]["refresh_token"], "refresh-b")
        self.assertTrue(records[0]["is_default"])
        self.assertFalse(records[1]["is_default"])
        self.assertTrue(all("access_token" not in record and "refresh_token" not in record for record in records))

    def test_refresh_targets_requested_and_configured_default_profiles(self) -> None:
        with _configured_config() as config, _secret_service():
            store = AccountStore(config)
            store.save(_account("profile-a", "PlayerA", alias="Alpha", is_default=False, expires_at=0))
            store.save(_account("profile-b", "PlayerB", alias="Beta", is_default=True, expires_at=0))
            store.set_default("profile-b")
            authenticator = MicrosoftAuthenticator(config, store, object())
            refreshed_tokens: list[str] = []

            def refresh(refresh_token):
                refreshed_tokens.append(refresh_token)
                return {"refresh_token": f"new-{refresh_token}", "profile": refresh_token}

            def finish(msa, _client_id):
                profile = msa["profile"]
                return _account(
                    f"new-{profile}",
                    f"Refreshed-{profile}",
                    access_token=f"new-access-{profile}",
                    refresh_token=msa["refresh_token"],
                    profile_id=profile,
                )

            authenticator._refresh_msa = refresh
            authenticator._finish_microsoft_login = finish

            requested = authenticator.get_valid("profile-a")
            default = authenticator.get_valid()
            stored_a = store.get(account_id="profile-a")
            stored_b = store.get(account_id="profile-b")

        self.assertEqual(refreshed_tokens, ["refresh-profile-a", "refresh-profile-b"])
        self.assertEqual(requested.account_id, "profile-a")
        self.assertEqual(requested.alias, "Alpha")
        self.assertFalse(requested.is_default)
        self.assertEqual(default.account_id, "profile-b")
        self.assertEqual(default.alias, "Beta")
        self.assertTrue(default.is_default)
        self.assertEqual(stored_a.access_token, "new-access-refresh-profile-a")
        self.assertEqual(stored_b.access_token, "new-access-refresh-profile-b")

    def test_device_login_preserves_existing_alias_and_default(self) -> None:
        with _configured_config() as config, _secret_service():
            store = AccountStore(config)
            store.save(_account("profile-a", "OldName", alias="My profile", is_default=True))
            config.set_value("client_id", "00000000-0000-4000-8000-000000000001")

            class FakeHttp:
                def post_form(self, _url, _data):
                    return {
                        "verification_uri": "https://login.example.invalid/device",
                        "user_code": "ABCD-EFGH",
                        "device_code": "device-code",
                        "expires_in": 900,
                        "interval": 5,
                    }

            authenticator = MicrosoftAuthenticator(config, store, FakeHttp(), notify=lambda _message: None)
            authenticator._poll_msa_token = lambda *_args: {"access_token": "msa-access", "refresh_token": "msa-refresh"}
            authenticator._finish_microsoft_login = lambda *_args: _account(
                "profile-a",
                "NewName",
                access_token="new-access",
                refresh_token="new-refresh",
                profile_id="profile-a",
            )

            with patch("nivora_launcher.accounts.webbrowser.open"):
                account = authenticator.login_device_code()

        self.assertEqual(account.account_id, "profile-a")
        self.assertEqual(account.alias, "My profile")
        self.assertTrue(account.is_default)

    def test_legacy_microsoft_account_id_remains_refreshable(self) -> None:
        with _configured_config() as config, _secret_service():
            store = AccountStore(config)
            store.save(
                _account(
                    "microsoft",
                    "LegacyPlayer",
                    alias="Legacy alias",
                    is_default=True,
                    expires_at=0,
                    profile_id="legacy-profile-id",
                )
            )
            authenticator = MicrosoftAuthenticator(config, store, object())
            authenticator._refresh_msa = lambda _token: {"access_token": "msa-access", "refresh_token": "msa-refresh-new"}
            authenticator._finish_microsoft_login = lambda *_args: _account(
                "legacy-profile-id",
                "LegacyPlayerUpdated",
                access_token="minecraft-access-new",
                refresh_token="msa-refresh-new",
                profile_id="legacy-profile-id",
            )

            account = authenticator.get_valid("microsoft")
            stored = store.get(account_id="microsoft")

        self.assertEqual(account.account_id, "microsoft")
        self.assertEqual(account.alias, "Legacy alias")
        self.assertTrue(account.is_default)
        self.assertEqual(stored.access_token, "minecraft-access-new")

    def test_cli_default_command_and_launch_selection_use_selected_profile(self) -> None:
        with _configured_config() as config, _secret_service():
            store = AccountStore(config)
            store.save(_account("profile-a", "PlayerA"))
            store.save(_account("profile-b", "PlayerB"))
            args = cli.build_parser().parse_args(["account", "set-default", "profile-b"])
            with redirect_stdout(io.StringIO()):
                cli._account_command(args, config, store)

            auto = cli._select_account("auto", config, store)
            microsoft = cli._select_account("microsoft", config, store)
            default_id = store.default().account_id

        self.assertEqual(config.settings.default_account_id, "profile-b")
        self.assertEqual(default_id, "profile-b")
        self.assertEqual(auto.account_id, "profile-b")
        self.assertEqual(microsoft.account_id, "profile-b")

    def test_gui_specific_operations_refresh_the_requested_profile(self) -> None:
        with _configured_config() as config, _secret_service():
            store = AccountStore(config)
            store.save(_account("profile-a", "PlayerA"))
            store.save(_account("profile-b", "PlayerB", is_default=True))
            config.set_value("default_account_id", "profile-b")

            selected = gui_select_account("profile-a", config, store)
            default = gui_select_account("auto", config, store)
            skin_account = _microsoft_account("profile-a", config, store, object())

        self.assertEqual(selected.account_id, "profile-a")
        self.assertEqual(default.account_id, "profile-b")
        self.assertEqual(skin_account.account_id, "profile-a")


if __name__ == "__main__":
    unittest.main()
