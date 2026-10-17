from __future__ import annotations

import contextlib
import io
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from nivora_launcher import windows_sandbox
from nivora_launcher.errors import LauncherError
from nivora_launcher.security import prepare_process, sandbox_status


def _windows(**extra):
    """Патчи, имитирующие Windows с доступным AppContainer на любой ОС."""
    return (
        patch("nivora_launcher.security.platform.system", return_value="Windows"),
        patch("nivora_launcher.security.windows_sandbox.appcontainer_supported", return_value=extra.get("supported", True)),
    )


class WindowsSandboxPolicyTests(unittest.TestCase):
    def _prepare(self, instance: Path, permissions: list[str], *, profile: str = "strict", command: list[str] | None = None, supported: bool = True):
        system, support = _windows(supported=supported)
        environment = {"PATH": "C:/Windows", "TEMP": "C:/Users/u/AppData/Local/Temp", "LOCALAPPDATA": "C:/Users/u/AppData/Local"}
        with system, support:
            return prepare_process(
                command or [sys.executable, "-version"],
                instance,
                environment,
                {"security_profile": profile, "security_permissions": permissions},
            )

    def _policy(self, command: list[str]) -> dict:
        separator = command.index("--")
        return windows_sandbox.decode_policy(command[separator - 1])

    def test_strict_wraps_java_in_appcontainer_helper(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory) / "instance"
            instance.mkdir()
            command, cwd, environment, warning = self._prepare(instance, ["files:instance", "desktop:display", "desktop:audio"])

            self.assertEqual(cwd, instance)
            separator = command.index("--")
            self.assertEqual(command[:separator - 1], windows_sandbox.helper_command())
            self.assertEqual(command[separator + 1:], [sys.executable, "-version"])
            policy = self._policy(command)
            self.assertEqual(policy["profile_name"], windows_sandbox.appcontainer_name(instance))
            self.assertEqual(policy["instance_access"], "modify")
            self.assertFalse(policy["network"])
            self.assertTrue(policy["display"])
            self.assertIn(str(instance / ".trial-launcher.json"), policy["deny_write_files"])
            self.assertIn("отключил сеть", warning or "")

    def test_cell_environment_reaches_java_but_not_helper(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory) / "instance"
            instance.mkdir()
            _command, _cwd, environment, _warning = self._prepare(instance, ["files:instance", "desktop:display"])

            # helper (и распаковка PyInstaller) работают с настоящими TEMP/LOCALAPPDATA
            self.assertEqual(environment["TEMP"], "C:/Users/u/AppData/Local/Temp")
            self.assertEqual(environment["LOCALAPPDATA"], "C:/Users/u/AppData/Local")
            self.assertNotIn("HOME", environment)
            # а Java получает каталоги cell
            child = windows_sandbox.child_environment(environment)
            cell = instance.resolve() / ".trial-cell"
            self.assertEqual(child["TEMP"], str(cell / "tmp"))
            self.assertEqual(child["TMP"], str(cell / "tmp"))
            self.assertEqual(child["HOME"], str(cell / "home"))
            self.assertEqual(child["APPDATA"], str(cell / "config"))
            self.assertEqual(child["LOCALAPPDATA"], str(cell / "cache"))
            self.assertFalse(any(key.startswith(windows_sandbox.SANDBOX_ENV_PREFIX) for key in child))

    def test_network_grant_adds_capabilities_and_loopback_warning(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory) / "instance"
            instance.mkdir()
            command, _cwd, _environment, warning = self._prepare(instance, ["network:game", "files:instance", "desktop:display", "desktop:audio"])

            self.assertTrue(self._policy(command)["network"])
            self.assertIn("localhost", warning or "")
            self.assertEqual(
                windows_sandbox.network_capabilities(True),
                ["S-1-15-3-1", "S-1-15-3-2", "S-1-15-3-3"],
            )
            self.assertEqual(windows_sandbox.network_capabilities(False), [])

    def test_read_only_instance_keeps_cell_tmp_writable(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory) / "instance"
            instance.mkdir()
            command, *_ = self._prepare(instance, ["desktop:display"])
            policy = self._policy(command)
            self.assertEqual(policy["instance_access"], "read")
            self.assertEqual(policy["writable_dirs"], [str(instance.resolve() / ".trial-cell" / "tmp")])

    def test_argument_file_and_java_root_are_readable_from_container(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            instance = root / "instance"
            instance.mkdir()
            java_home = root / "runtime" / "java-21"
            (java_home / "bin").mkdir(parents=True)
            (java_home / "lib").mkdir()
            java = java_home / "bin" / "java.exe"
            java.write_text("", encoding="utf-8")
            argument_file = root / "app-data" / ".java-args-x"
            argument_file.parent.mkdir()
            argument_file.write_text("--accessToken secret\n", encoding="utf-8")

            command, *_ = self._prepare(instance, ["files:instance", "desktop:display"], command=[str(java), f"@{argument_file}"])
            policy = self._policy(command)
            self.assertEqual(policy["read_dirs"], [str(java_home.resolve())])
            self.assertEqual(policy["read_files"], [str(argument_file)])
            self.assertNotIn("secret", " ".join(command))

    def test_display_and_audio_limits_are_reported_honestly(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory) / "instance"
            instance.mkdir()
            command, _cwd, _environment, warning = self._prepare(instance, ["files:instance"])
            self.assertFalse(self._policy(command)["display"])
            self.assertIn("буфер обмена", warning or "")
            self.assertIn("desktop:audio не обеспечивается", warning or "")
        restricted = windows_sandbox.ui_restrictions(False)
        allowed = windows_sandbox.ui_restrictions(True)
        self.assertTrue(restricted & windows_sandbox.UILIMIT_READCLIPBOARD)
        self.assertTrue(restricted & windows_sandbox.UILIMIT_HANDLES)
        self.assertFalse(allowed & windows_sandbox.UILIMIT_READCLIPBOARD)
        self.assertTrue(allowed & windows_sandbox.UILIMIT_DESKTOP)

    def test_host_file_permissions_are_not_silently_widened(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory) / "instance"
            instance.mkdir()
            with self.assertRaisesRegex(LauncherError, "host-read"):
                self._prepare(instance, ["files:instance", "files:host-read"])
            with self.assertRaisesRegex(LauncherError, "запись в файлы компьютера"):
                self._prepare(instance, ["files:instance", "files:host-write"])
            command, _cwd, _environment, warning = self._prepare(instance, ["files:instance", "files:host-read"], profile="manual")
            self.assertNotIn("--", command)
            self.assertIn("не обеспечиваются", warning or "")

    def test_strict_requires_appcontainer_and_manual_degrades_with_warning(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            instance = Path(directory) / "instance"
            instance.mkdir()
            with self.assertRaisesRegex(LauncherError, "AppContainer"):
                self._prepare(instance, ["files:instance"], supported=False)
            command, _cwd, _environment, warning = self._prepare(instance, ["files:instance"], profile="manual", supported=False)
            self.assertEqual(command, [sys.executable, "-version"])
            self.assertIn("AppContainer недоступен", warning or "")
            command, *_ = self._prepare(instance, ["files:instance", "desktop:display"], profile="manual")
            self.assertIn("--", command)

    def test_policy_is_validated_before_use(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            good = {
                "version": 1,
                "profile_name": windows_sandbox.appcontainer_name(Path(directory)),
                "instance_dir": str(Path(directory).resolve()),
                "instance_access": "modify",
                "writable_dirs": [],
                "read_dirs": [],
                "read_files": [],
                "deny_write_files": [],
                "network": False,
                "display": True,
                "audio": True,
            }
            self.assertEqual(windows_sandbox.decode_policy(windows_sandbox.encode_policy(good)), good)
            for key, value in (
                ("profile_name", "Other.App"),
                ("instance_access", "full"),
                ("read_dirs", ["relative/path"]),
                ("network", "yes"),
                ("version", 2),
            ):
                with self.subTest(key=key), self.assertRaises(windows_sandbox.WindowsSandboxError):
                    windows_sandbox.validate_policy({**good, key: value})
            with self.assertRaises(windows_sandbox.WindowsSandboxError):
                windows_sandbox.decode_policy("not-base64!!")

    def test_profile_name_is_stable_and_within_windows_limit(self) -> None:
        first = windows_sandbox.appcontainer_name(Path("/games/instances/a"))
        self.assertEqual(first, windows_sandbox.appcontainer_name(Path("/games/instances/a")))
        self.assertNotEqual(first, windows_sandbox.appcontainer_name(Path("/games/instances/b")))
        self.assertLessEqual(len(first), 64)

    def test_frozen_backend_reenters_itself_as_helper(self) -> None:
        with (
            patch("nivora_launcher.windows_sandbox.sys.frozen", True, create=True),
            patch("nivora_launcher.windows_sandbox.sys.executable", "C:/Nivora/backend.exe"),
        ):
            self.assertEqual(windows_sandbox.helper_command(), ["C:/Nivora/backend.exe", "--win-sandbox"])

    def test_helper_strips_pyinstaller_variables_and_builds_block(self) -> None:
        block = windows_sandbox.environment_block({"_MEIPASS2": "x", "_PYI_ARCHIVE": "y", "PATH": "C:/w", "A": "1"})
        self.assertEqual(block, "A=1\0PATH=C:/w\0\0")

    def test_helper_rejects_malformed_arguments(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(windows_sandbox.main(["only-policy"]), 2)
            self.assertEqual(windows_sandbox.main(["broken", "--", "java"]), 1)

    def test_acl_grants_are_applied_once_and_reapplied_after_changes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            instance = root / "instance"
            instance.mkdir()
            service = instance / ".trial-launcher.json"
            service.write_text("{}", encoding="utf-8")
            argument_file = root / "args"
            argument_file.write_text("", encoding="utf-8")
            policy = {
                "version": 1,
                "profile_name": windows_sandbox.appcontainer_name(instance),
                "instance_dir": str(instance),
                "instance_access": "modify",
                "writable_dirs": [],
                "read_dirs": [],
                "read_files": [str(argument_file)],
                "deny_write_files": [str(service)],
                "network": False,
                "display": True,
                "audio": True,
            }
            calls: list[tuple[str, ...]] = []
            with (
                patch.dict("os.environ", {"LOCALAPPDATA": str(root / "local")}),
                patch("nivora_launcher.windows_sandbox._icacls", side_effect=lambda *args: calls.append(args)),
            ):
                windows_sandbox.apply_acl(policy, "S-1-15-2-1")
                first = list(calls)
                calls.clear()
                windows_sandbox.apply_acl(policy, "S-1-15-2-1")
                second = list(calls)
                calls.clear()
                windows_sandbox.apply_acl({**policy, "instance_access": "read"}, "S-1-15-2-1")
                third = list(calls)

            self.assertEqual(first[0], (str(instance), "/remove:g", "*S-1-15-2-1"))
            self.assertIn((str(instance), "/grant", "*S-1-15-2-1:(OI)(CI)(IO)(M)"), first)
            # корень экземпляра без права DELETE
            root_grant = next(call[2] for call in first if call[1] == "/grant" and "(IO)" not in call[2] and call[0] == str(instance))
            self.assertNotIn("DE", root_grant.split(":", 1)[1].strip("()").split(","))
            # метка совпала: тяжёлые ACL не трогаются, а файлы запуска — каждый раз
            self.assertEqual(
                second,
                [(str(argument_file), "/grant", "*S-1-15-2-1:(R)"), (str(service), "/deny", f"*S-1-15-2-1:{windows_sandbox.DENY_WRITE_RIGHTS}")],
            )
            self.assertEqual(third[0], (str(instance), "/remove:g", "*S-1-15-2-1"))
            self.assertIn((str(instance), "/grant", "*S-1-15-2-1:(OI)(CI)(RX)"), third)

    def test_status_reports_appcontainer_on_windows(self) -> None:
        with (
            patch("nivora_launcher.security.platform.system", return_value="Windows"),
            patch("nivora_launcher.security.windows_sandbox.probe", return_value=(True, "")),
        ):
            status = sandbox_status()
        self.assertEqual(status["platform"], "windows")
        self.assertEqual(status["sandbox"], "appcontainer")
        self.assertTrue(status["strict_available"])


if __name__ == "__main__":
    unittest.main()
