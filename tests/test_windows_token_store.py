import unittest
from unittest.mock import patch

from nivora_launcher import accounts


class WindowsTokenStoreCompatibilityTests(unittest.TestCase):
    """Rust-движок пишет токен в цель «trial-launcher-microsoft:<id>» — Python обязан читать именно её."""

    def test_reads_rust_target_first(self) -> None:
        store = {"trial-launcher-microsoft:abc": '{"access_token":"a","refresh_token":"r"}'}
        with patch.object(accounts, "_windows_credential_read", side_effect=lambda target, expected_user=None: store.get(target)):
            self.assertEqual(accounts._windows_load_token_payload("abc"), store["trial-launcher-microsoft:abc"])

    def test_migrates_legacy_keyring_format(self) -> None:
        store = {"abc@trial-launcher-microsoft": '{"access_token":"a","refresh_token":"r"}'}
        writes: list[tuple[str, str, str]] = []
        with (
            patch.object(accounts, "_windows_credential_read", side_effect=lambda target, expected_user=None: store.get(target)),
            patch.object(accounts, "_windows_credential_write", side_effect=lambda *args: writes.append(args) or True),
            patch.object(accounts, "_windows_credential_delete"),
        ):
            payload = accounts._windows_load_token_payload("abc")
        self.assertEqual(payload, store["abc@trial-launcher-microsoft"])
        self.assertEqual(writes, [("trial-launcher-microsoft:abc", "abc", payload)])

    def test_target_matches_rust_engine_format(self) -> None:
        self.assertEqual(accounts._windows_credential_target("abc"), "trial-launcher-microsoft:abc")


class ChunkedCredentialTests(unittest.TestCase):
    def test_long_payload_is_split_and_read_back(self) -> None:
        store: dict[str, str] = {}
        payload = '{"access_token":"' + "a" * 1800 + '","refresh_token":"' + "r" * 700 + '"}'
        with (
            patch.object(accounts, "_windows_credential_read", side_effect=lambda target, expected_user=None: store.get(target)),
            patch.object(accounts, "_windows_credential_write", side_effect=lambda target, _user, value: store.__setitem__(target, value) or True),
            patch.object(accounts, "_windows_credential_delete", side_effect=lambda target: store.pop(target, None)),
        ):
            self.assertTrue(accounts._windows_write_chunked("trial-launcher-microsoft:abc", "abc", payload))
            self.assertTrue(all(len(value.encode("utf-16-le")) <= 2560 for value in store.values()))
            self.assertEqual(store["trial-launcher-microsoft:abc"], "nivora-chunked:3")
            self.assertEqual(accounts._windows_load_token_payload("abc"), payload)
            accounts._windows_delete_chunked("trial-launcher-microsoft:abc")
            self.assertEqual(store, {})


class TokenFileFallbackTests(unittest.TestCase):
    def test_volatile_keyring_falls_back_to_private_file(self) -> None:
        import tempfile
        from pathlib import Path

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            account = accounts.Account(account_id="abc", kind="microsoft", username="Steve", uuid="u", access_token="tok", refresh_token="ref")
            with (
                patch.object(accounts, "_store_keyring_tokens"),
                patch.object(accounts, "_secret_is_persistent", return_value=False),
            ):
                accounts._store_account_tokens(account, root)
            # «Перезапуск»: keyring пуст (session-коллекция исчезла) — токен из файла.
            with patch.object(accounts, "_load_keyring_tokens", return_value={}):
                self.assertEqual(accounts._load_account_tokens("abc", root), {"access_token": "tok", "refresh_token": "ref", "client_token": ""})
            with (
                patch.object(accounts, "_store_keyring_tokens"),
                patch.object(accounts, "_secret_is_persistent", return_value=True),
            ):
                accounts._store_account_tokens(account, root)
            self.assertFalse((root / accounts.TOKEN_FILE).exists())

    def test_session_collection_is_not_persistent(self) -> None:
        from types import SimpleNamespace

        results = iter([SimpleNamespace(returncode=0, stdout="[/org/freedesktop/secrets/collection/session/1]\n")])
        with patch.object(accounts, "os") as fake_os, patch.object(accounts, "_run_secret_tool", side_effect=lambda *_a, **_k: next(results)):
            fake_os.name = "posix"
            self.assertFalse(accounts._secret_is_persistent("abc", "{}"))


if __name__ == "__main__":
    unittest.main()
