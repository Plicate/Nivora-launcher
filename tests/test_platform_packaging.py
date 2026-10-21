from __future__ import annotations

import json
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from nivora_launcher.accounts import _windows_keyring
from nivora_launcher.errors import AuthenticationError, LauncherError
from nivora_launcher.installer import _safe_extract_windows
from nivora_launcher.instances import _launcher_cli_command
from nivora_launcher.launcher import _create_launcher_java_argument_file
from nivora_launcher.portable_fs import safe_path
from nivora_launcher.snapshots import _extract_archive_windows, _list_windows


class PlatformPackagingTests(unittest.TestCase):
    def test_arch_package_uses_system_python_instead_of_aur_pyinstaller(self) -> None:
        project_root = Path(__file__).resolve().parents[1]
        pkgbuild = (project_root / "packaging/arch/PKGBUILD").read_text(encoding="utf-8")
        build_script = (project_root / "scripts/build_arch_package.sh").read_text(encoding="utf-8")
        backend_wrapper = (project_root / "packaging/arch/nivora-launcher-backend").read_text(encoding="utf-8")

        self.assertNotIn("python-pyinstaller", pkgbuild)
        self.assertIn("packaging/arch/nivora-launcher-backend", pkgbuild)
        self.assertIn('cd "${srcdir}/${pkgname}"', pkgbuild)
        self.assertIn('archive_root="$pkgname"', build_script)
        self.assertIn("exec /usr/bin/python3 -m nivora_launcher.gui_service", backend_wrapper)

    def test_windows_exe_build_is_a_separate_auto_script(self) -> None:
        project_root = Path(__file__).resolve().parents[1]
        windows_script = (project_root / "scripts/build_windows_exe.sh").read_text(encoding="utf-8")
        arch_script = (project_root / "scripts/build_arch_package.sh").read_text(encoding="utf-8")
        backend_entry = (project_root / "nivora_launcher/backend_entrypoint.py").read_text(encoding="utf-8")
        windows_conf = (project_root / "frontend/src-tauri/tauri.windows.conf.json").read_text(encoding="utf-8")

        # Один прогон собирает всё: PyInstaller-бэкенд, фронтенд и tauri build,
        # а exe кладётся ровно туда, где его ждёт tauri.windows.conf.json.
        self.assertIn("-m PyInstaller", windows_script)
        self.assertIn("scripts/backend_entry.py", windows_script)
        # Бэкенд — onedir-сборка (меньше ложных срабатываний антивирусов,
        # чем onefile), вкладывается zip-архивом и кладётся ресурсом app/.
        self.assertIn("resources/backend/app", windows_script)
        self.assertIn("nivora-launcher-backend.zip", windows_script)
        self.assertIn("npm run tauri -- build", windows_script)
        self.assertIn("resources/backend/app", windows_conf)
        self.assertIn("--onedir", windows_script)
        self.assertIn("--noupx", windows_script)
        # PyInstaller не кросс-компилирует: на Linux скрипт сам ставит
        # Windows Python в изолированный wine-префикс (официальный установщик).
        self.assertIn("www.python.org", windows_script)
        self.assertIn("wineprefix", windows_script)
        # Кросс-сборка Tauri тоже автоматическая: тулчейн ставится сам,
        # а cargo запускается через cargo-xwin (clang-cl + lld-link + Windows SDK).
        self.assertIn("rustup target add x86_64-pc-windows-msvc", windows_script)
        self.assertIn("cargo install cargo-xwin", windows_script)
        self.assertIn("--runner cargo-xwin", windows_script)
        self.assertIn("clang lld llvm", windows_script)
        # NSIS: tauri-bundler зовёт системный makensis, которого нет в репо Arch —
        # скрипт берёт его из Debian-пакета и подкладывает данные симлинком;
        # RestartManager.nsh (NSIS 3.10+, нужен шаблону tauri) — из nsis-common 3.11.
        self.assertIn("deb.debian.org", windows_script)
        self.assertIn("/usr/share/nsis", windows_script)
        self.assertIn("RestartManager.nsh", windows_script)
        self.assertIn("--bundles nsis", windows_script)
        # Доверие к exe: метаданные версии, суммы SHA256 и опциональная
        # Authenticode-подпись (osslsigncode) вместо «безликого» бинарника.
        self.assertIn("--version-file", windows_script)
        # PyInstaller ≥6 резолвит --version-file от --specpath (= .build-windows),
        # поэтому аргумент — голое имя файла; путь от корня удваивался и валил
        # сборку (FileNotFoundError: .build-windows/.build-windows/...).
        self.assertIn("--version-file backend-version.txt", windows_script)
        self.assertIn("sha256sums.txt", windows_script)
        self.assertIn("osslsigncode", windows_script)
        # Без окон консоли и без «ошибки Python» на Windows: бэкенд собирается
        # windowed, а portable-режим — ОДИН файл: бэкенд встроен в сам
        # trial-launcher.exe (include_bytes!) и при первом запуске распаковывается
        # в %LOCALAPPDATA%. Системный python на Windows не вызывается вовсе.
        self.assertIn("--noconsole", windows_script)
        lib_rs = (project_root / "frontend/src-tauri/src/lib.rs").read_text(encoding="utf-8")
        self.assertIn("current_exe", lib_rs)
        self.assertIn("include_bytes!", lib_rs)
        self.assertIn("EMBEDDED_BACKEND_ARCHIVE", lib_rs)
        self.assertIn("extract_backend_archive", lib_rs)
        self.assertIn("LOCALAPPDATA", lib_rs)
        recorder = (project_root / "nivora_launcher/recorder.py").read_text(encoding="utf-8")
        self.assertIn("CREATE_NO_WINDOW", recorder)
        gui_service = (project_root / "nivora_launcher/gui_service.py").read_text(encoding="utf-8")
        self.assertIn("sys.stdin is None", gui_service)
        readme = (project_root / "README.md").read_text(encoding="utf-8")
        self.assertIn("SmartScreen", readme)
        readme_ru = (project_root / "README.ru.md").read_text(encoding="utf-8")
        self.assertIn("Windows предупреждает при запуске", readme_ru)
        # Точка входа: без аргументов — JSON-воркер для Tauri, с --cli — CLI-режим.
        self.assertIn("from nivora_launcher.gui_service import main", backend_entry)
        self.assertIn('"--cli"', backend_entry)
        # exe-сборка — ОТДЕЛЬНЫЙ скрипт: Arch-пакет её не запускает и не дублирует.
        self.assertNotIn("build_windows_exe", arch_script)

    def test_windows_bundle_has_valid_ico_icon(self) -> None:
        project_root = Path(__file__).resolve().parents[1]
        icon_path = project_root / "frontend/src-tauri/icons/icon.ico"
        config = json.loads(
            (project_root / "frontend/src-tauri/tauri.conf.json").read_text(encoding="utf-8")
        )

        # tauri-build при Windows-таргете требует icons/icon.ico (Windows Resource);
        # валидный ICO начинается с magic 00 00 01 00 и содержит >= 1 запись.
        raw = icon_path.read_bytes()
        self.assertEqual(raw[:4], b"\x00\x00\x01\x00")
        self.assertGreaterEqual(int.from_bytes(raw[4:6], "little"), 1)
        self.assertIn("icons/icon.ico", config["bundle"]["icon"])

    def test_windows_java_argument_file_uses_private_app_data_directory(self) -> None:
        instance_dir = Path("instance")
        data_home = Path("app-data")
        argument_file = object()

        with (
            patch("nivora_launcher.launcher.os.name", "nt"),
            patch("nivora_launcher.launcher._create_java_argument_file", return_value=argument_file) as create,
        ):
            result = _create_launcher_java_argument_file(["-cp", "minecraft.jar"], instance_dir, data_home)

        self.assertIs(result, argument_file)
        create.assert_called_once_with(
            ["-cp", "minecraft.jar"],
            instance_dir,
            private_directory=data_home,
        )

    def test_non_windows_java_argument_file_keeps_instance_directory_behavior(self) -> None:
        instance_dir = Path("instance")
        data_home = Path("app-data")
        argument_file = object()

        with (
            patch("nivora_launcher.launcher.os.name", "posix"),
            patch("nivora_launcher.launcher._create_java_argument_file", return_value=argument_file) as create,
        ):
            result = _create_launcher_java_argument_file(["-cp", "minecraft.jar"], instance_dir, data_home)

        self.assertIs(result, argument_file)
        create.assert_called_once_with(["-cp", "minecraft.jar"], instance_dir)

    def test_windows_launch_command_keeps_microsoft_token_out_of_argv(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            instance_dir = root / "instance"
            data_home = root / "app-data"
            instance_dir.mkdir()
            with (
                patch("nivora_launcher.launcher.os.name", "nt"),
                patch("nivora_launcher.launcher.os.memfd_create", None, create=True),
            ):
                argument_file = _create_launcher_java_argument_file(["--accessToken", "access-secret"], instance_dir, data_home)
            try:
                launch_command = ["javaw.exe", argument_file.argument]
                self.assertNotIn("access-secret", launch_command)
                stored_files = list(data_home.iterdir())
                self.assertEqual(len(stored_files), 1)
                self.assertIn("access-secret", stored_files[0].read_text(encoding="utf-8"))
                argument_file.path = stored_files[0]
            finally:
                argument_file.close()

    def test_source_shortcuts_use_installed_cli(self) -> None:
        with patch("nivora_launcher.instances.sys.frozen", False, create=True):
            self.assertEqual(_launcher_cli_command(), ["nivora-launcher"])

    def test_packaged_shortcuts_reenter_backend_executable_in_cli_mode(self) -> None:
        with (
            patch("nivora_launcher.instances.sys.executable", "C:/Nivora Launcher/backend.exe"),
            patch("nivora_launcher.instances.sys.frozen", True, create=True),
        ):
            self.assertEqual(
                _launcher_cli_command(),
                ["C:/Nivora Launcher/backend.exe", "--cli"],
            )

    def test_windows_native_extractor_writes_files_and_rejects_traversal(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive_path = root / "native.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("bin/native.dll", b"native")

            destination = root / "natives"
            _safe_extract_windows(archive_path, destination, set())
            self.assertEqual((destination / "bin" / "native.dll").read_bytes(), b"native")

            malicious_path = root / "malicious.zip"
            with zipfile.ZipFile(malicious_path, "w") as archive:
                archive.writestr("../outside.dll", b"escape")
            with self.assertRaises(LauncherError):
                _safe_extract_windows(malicious_path, root / "malicious", set())
            self.assertFalse((root / "outside.dll").exists())

    def test_windows_snapshot_extract_and_listing_are_path_based(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive_path = root / "mods-snapshot.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                archive.writestr("mod.jar", b"mod")

            extracted = root / "extracted"
            extracted.mkdir()
            _extract_archive_windows(archive_path, extracted)
            self.assertEqual((extracted / "mod.jar").read_bytes(), b"mod")

            snapshot_dir = root / "snapshots"
            snapshot_dir.mkdir()
            snapshot_id = "1700000000-deadbeef"
            (snapshot_dir / f"mods-{snapshot_id}.zip").write_bytes(archive_path.read_bytes())
            (snapshot_dir / f"mods-{snapshot_id}.json").write_text(
                json.dumps({"id": snapshot_id, "category": "mods", "archive": f"mods-{snapshot_id}.zip"}),
                encoding="utf-8",
            )
            self.assertEqual(len(_list_windows(snapshot_dir, root / "mods", "mods")), 1)

            malicious_path = root / "traversal.zip"
            with zipfile.ZipFile(malicious_path, "w") as archive:
                archive.writestr("../outside.jar", b"escape")
            with self.assertRaises(LauncherError):
                _extract_archive_windows(malicious_path, root / "malicious-snapshot")
            self.assertFalse((root / "outside.jar").exists())

    def test_windows_keyring_requires_a_usable_backend(self) -> None:
        keyring = Mock()
        keyring.get_keyring.return_value = SimpleNamespace(priority=1)
        with patch.dict(sys.modules, {"keyring": keyring}):
            self.assertIs(_windows_keyring(), keyring)

        keyring.get_keyring.return_value = SimpleNamespace(priority=0)
        with patch.dict(sys.modules, {"keyring": keyring}):
            with self.assertRaises(AuthenticationError):
                _windows_keyring()

    def test_windows_safe_paths_reject_reserved_device_names(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaises(LauncherError):
                safe_path(root, ("NUL.txt",))


if __name__ == "__main__":
    unittest.main()
