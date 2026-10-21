from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from nivora_launcher.launcher import GameLauncher, _looks_like_non_nvidia_renderer


class _FakeProcess:
    def __init__(self, output: str, return_code: int, pid: int) -> None:
        self.stdout = iter(f"{line}\n" for line in output.splitlines() or [""])
        self.return_code = return_code
        self.pid = pid
        self.running = True
        self.terminate_called = False

    def poll(self) -> int | None:
        return None if self.running else self.return_code

    def terminate(self) -> None:
        self.terminate_called = True
        self.running = False
        self.return_code = -15

    def wait(self, timeout: float | None = None) -> int:
        self.running = False
        return self.return_code


class _FakeArgumentFile:
    def __init__(self, index: int) -> None:
        self.argument = f"@argument-file-{index}"
        self.fd = None
        self.path = None
        self.pass_fds = ()
        self.closed = False

    def close(self) -> None:
        self.closed = True


class GlfwFallbackTests(unittest.TestCase):
    def test_nvidia_renderer_detection_distinguishes_integrated_and_nvidia_vendors(self) -> None:
        self.assertTrue(_looks_like_non_nvidia_renderer("[Render thread/INFO]: OpenGL Vendor: Intel Corporation"))
        self.assertTrue(_looks_like_non_nvidia_renderer("OpenGL Vendor: Mesa/X.org"))
        self.assertTrue(_looks_like_non_nvidia_renderer("OpenGL Renderer: llvmpipe (LLVM 18.1)"))
        self.assertFalse(_looks_like_non_nvidia_renderer("[Render thread/INFO]: OpenGL Vendor: NVIDIA Corporation"))

    def test_nvidia_wayland_starts_with_native_glfw(self) -> None:
        processes = [_FakeProcess("OpenGL Vendor: NVIDIA Corporation\nMinecraft started", 0, 201)]
        process_calls: list[tuple[list[str], dict[str, str]]] = []
        argument_files: list[_FakeArgumentFile] = []
        argument_lists: list[list[str]] = []
        messages: list[str] = []
        launch_log = ""
        launch_log_path: Path | None = None
        unpersisted_lines: list[str] = []

        def capture_output(line: str) -> None:
            nonlocal launch_log_path
            messages.append(line)
            if line.startswith("Лог запуска: "):
                launch_log_path = Path(line.removeprefix("Лог запуска: "))
            if launch_log_path and line not in launch_log_path.read_text(encoding="utf-8").splitlines():
                unpersisted_lines.append(line)

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            instance_dir = root / "instance"
            instance_dir.mkdir()
            config = SimpleNamespace(
                paths=SimpleNamespace(logs_dir=root / "logs", data_home=root / "data"),
                settings=SimpleNamespace(graphics_mode="nvidia"),
            )
            launcher = object.__new__(GameLauncher)
            launcher.config = config
            launcher.process = None
            launcher.cancellation = None
            launcher.build_command = lambda *_args: ["java", "-cp", "minecraft.jar", "net.minecraft.client.main.Main"]
            library = instance_dir / ".trial-cell" / "cache" / "wayland-glfw-v1.1.0" / "libglfw.so"
            launcher._wayland_glfw_library = lambda _instance: library

            initial_environment = {
                "PATH": "/usr/bin",
                "HOME": str(root),
                "XDG_SESSION_TYPE": "wayland",
                "WAYLAND_DISPLAY": "wayland-0",
                "MESA_LOADER_DRIVER_OVERRIDE": "llvmpipe",
                "GALLIUM_DRIVER": "llvmpipe",
                "LIBGL_ALWAYS_SOFTWARE": "1",
                "__VK_LAYER_NV_optimus": "NVIDIA_only",
            }

            def prepare(command, cwd, environment, _settings, **_kwargs):
                return command, cwd, dict(environment), None

            def create_argument_file(arguments, _instance_dir):
                argument_lists.append(list(arguments))
                argument_file = _FakeArgumentFile(len(argument_files) + 1)
                argument_files.append(argument_file)
                return argument_file

            def popen(command, **options):
                process_calls.append((list(command), dict(options["env"])))
                return processes[len(process_calls) - 1]

            account = SimpleNamespace(kind="microsoft", access_token="test-token", username="Player")
            install = SimpleNamespace(
                version_id="1.21.11",
                instance_dir=instance_dir,
                version_json={
                    "mainClass": "net.minecraft.client.main.Main",
                    "javaVersion": {"majorVersion": 21},
                },
            )
            with (
                patch("nivora_launcher.launcher.safe_process_environment", side_effect=lambda **_kwargs: dict(initial_environment)),
                patch("nivora_launcher.launcher.java_version", return_value=27),
                patch("nivora_launcher.launcher.prepare_process", side_effect=prepare),
                patch("nivora_launcher.launcher.java_bitness", return_value=64),
                patch("nivora_launcher.launcher.java_probe", return_value=(21, 64)),
                patch("nivora_launcher.launcher._create_java_argument_file", side_effect=create_argument_file),
                patch("nivora_launcher.launcher.subprocess.Popen", side_effect=popen),
                patch("nivora_launcher.launcher.os.killpg", side_effect=ProcessLookupError),
                patch("nivora_launcher.launcher.InstanceProcessLock") as process_lock,
                patch("nivora_launcher.launcher.InstanceStore") as instance_store,
            ):
                process_lock.return_value.acquire.return_value = None
                process_lock.return_value.release.return_value = None
                instance_store.return_value.add_played_seconds.return_value = None
                result = launcher.run(
                    install,
                    account,
                    output=capture_output,
                    settings={"security_profile": "compat", "graphics_mode": "nvidia"},
                )
                launch_log = next((root / "logs").glob("*.log")).read_text(encoding="utf-8")

        self.assertEqual(result, 0)
        self.assertEqual(unpersisted_lines, [])
        self.assertTrue(any("Java: 27; Minecraft metadata: 21" in line for line in messages))
        self.assertTrue(any("совместимость не гарантирована" in line for line in messages))
        self.assertIn("GLFW=native-wayland-glfw-v1.1.0", launch_log)
        self.assertIn("NVIDIA + Wayland: использую native Wayland GLFW с первой попытки", launch_log)
        self.assertNotIn("GLX не создал контекст", launch_log)
        self.assertIn("Minecraft started", launch_log)
        self.assertEqual(len(process_calls), 1)
        self.assertEqual(len(argument_lists), 1)
        self.assertIn(f"-Dorg.lwjgl.glfw.libname={library}", argument_lists[0])
        self.assertIn("-Dsodium.checks.issue2561=false", argument_lists[0])
        self.assertFalse(processes[0].terminate_called)
        self.assertTrue(argument_files[0].closed)
        native_environment = process_calls[0][1]
        self.assertEqual(native_environment["GLFW_FORCE_WAYLAND"], "1")
        self.assertEqual(native_environment["TRIAL_LAUNCHER_GRAPHICS_BACKEND"], "native-wayland-glfw-v1.1.0")
        self.assertNotIn("__GLX_VENDOR_LIBRARY_NAME", native_environment)
        self.assertEqual(native_environment["__NV_PRIME_RENDER_OFFLOAD"], "1")
        self.assertNotIn("__VK_LAYER_NV_optimus", native_environment)
        self.assertNotIn("MESA_LOADER_DRIVER_OVERRIDE", native_environment)
        self.assertNotIn("GALLIUM_DRIVER", native_environment)
        self.assertNotIn("LIBGL_ALWAYS_SOFTWARE", native_environment)

    def test_wayland_library_failure_keeps_glx_launch_and_holds_instance_lock(self) -> None:
        process = _FakeProcess("Minecraft started", 0, 251)
        process_environments: list[dict[str, str]] = []
        lock_counts: list[int] = []
        messages: list[str] = []

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            instance_dir = root / "instance"
            instance_dir.mkdir()
            launcher = object.__new__(GameLauncher)
            launcher.config = SimpleNamespace(
                paths=SimpleNamespace(logs_dir=root / "logs", data_home=root / "data"),
                settings=SimpleNamespace(graphics_mode="nvidia"),
            )
            launcher.process = None
            launcher.cancellation = None
            launcher.build_command = lambda *_args: ["java", "Main"]

            def unavailable_library(_instance_dir):
                lock_counts.append(process_lock.return_value.acquire.call_count)
                raise OSError("network unavailable")

            def prepare(command, cwd, environment, _settings, **_kwargs):
                return command, cwd, dict(environment), None

            def popen(_command, **options):
                process_environments.append(dict(options["env"]))
                return process

            account = SimpleNamespace(kind="microsoft", access_token="", username="Player")
            install = SimpleNamespace(
                version_id="1.21.11",
                instance_dir=instance_dir,
                version_json={"mainClass": "Main"},
            )
            with (
                patch(
                    "nivora_launcher.launcher.safe_process_environment",
                    return_value={"PATH": "/usr/bin", "XDG_SESSION_TYPE": "wayland", "WAYLAND_DISPLAY": "wayland-0"},
                ),
                patch("nivora_launcher.launcher.prepare_process", side_effect=prepare),
                patch("nivora_launcher.launcher.java_bitness", return_value=64),
                patch("nivora_launcher.launcher.java_probe", return_value=(21, 64)),
                patch("nivora_launcher.launcher.subprocess.Popen", side_effect=popen),
                patch("nivora_launcher.launcher.os.killpg", side_effect=ProcessLookupError),
                patch("nivora_launcher.launcher.InstanceProcessLock") as process_lock,
                patch("nivora_launcher.launcher.InstanceStore") as instance_store,
            ):
                process_lock.return_value.acquire.return_value = None
                process_lock.return_value.release.return_value = None
                instance_store.return_value.add_played_seconds.return_value = None
                launcher._wayland_glfw_library = unavailable_library
                result = launcher.run(
                    install,
                    account,
                    output=messages.append,
                    settings={"security_profile": "compat", "graphics_mode": "nvidia"},
                )
                launch_log = next((root / "logs").glob("*.log")).read_text(encoding="utf-8")

        self.assertEqual(result, 0)
        self.assertEqual(lock_counts, [1])
        self.assertEqual(len(process_environments), 1)
        self.assertNotIn("TRIAL_LAUNCHER_GRAPHICS_BACKEND", process_environments[0])
        self.assertIn("GLFW=bundled", launch_log)
        self.assertIn("пробую обычный запуск через GLX", launch_log)

    def test_nvidia_wayland_stops_instead_of_accepting_llvmpipe(self) -> None:
        process = _FakeProcess("OpenGL Vendor: Mesa\nOpenGL Renderer: llvmpipe", 0, 301)
        process_calls: list[tuple[list[str], dict[str, str]]] = []
        messages: list[str] = []

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            instance_dir = root / "instance"
            instance_dir.mkdir()
            launcher = object.__new__(GameLauncher)
            launcher.config = SimpleNamespace(
                paths=SimpleNamespace(logs_dir=root / "logs", data_home=root / "data"),
                settings=SimpleNamespace(graphics_mode="nvidia"),
            )
            launcher.process = None
            launcher.cancellation = None
            launcher.build_command = lambda *_args: ["java", "-cp", "minecraft.jar", "Main"]
            launcher._wayland_glfw_library = lambda _instance: instance_dir / "libglfw.so"

            def prepare(command, cwd, environment, _settings, **_kwargs):
                return command, cwd, dict(environment), None

            def popen(command, **options):
                process_calls.append((list(command), dict(options["env"])))
                return process

            account = SimpleNamespace(kind="microsoft", access_token="", username="Player")
            install = SimpleNamespace(
                version_id="1.21.11",
                instance_dir=instance_dir,
                version_json={"mainClass": "Main"},
            )
            with (
                patch(
                    "nivora_launcher.launcher.safe_process_environment",
                    return_value={"PATH": "/usr/bin", "XDG_SESSION_TYPE": "wayland", "WAYLAND_DISPLAY": "wayland-0"},
                ),
                patch("nivora_launcher.launcher.prepare_process", side_effect=prepare),
                patch("nivora_launcher.launcher.java_bitness", return_value=64),
                patch("nivora_launcher.launcher.java_probe", return_value=(21, 64)),
                patch("nivora_launcher.launcher.subprocess.Popen", side_effect=popen),
                patch("nivora_launcher.launcher.os.killpg", side_effect=ProcessLookupError),
                patch("nivora_launcher.launcher.InstanceProcessLock") as process_lock,
                patch("nivora_launcher.launcher.InstanceStore") as instance_store,
            ):
                process_lock.return_value.acquire.return_value = None
                process_lock.return_value.release.return_value = None
                instance_store.return_value.add_played_seconds.return_value = None
                result = launcher.run(
                    install,
                    account,
                    output=messages.append,
                    settings={"security_profile": "compat", "graphics_mode": "nvidia"},
                )

        self.assertEqual(result, -15)
        self.assertTrue(process.terminate_called)
        self.assertEqual(len(process_calls), 1)
        self.assertEqual(process_calls[0][1]["TRIAL_LAUNCHER_GRAPHICS_BACKEND"], "native-wayland-glfw-v1.1.0")
        self.assertFalse(process_calls[0][1].get("LIBGL_ALWAYS_SOFTWARE"))
        self.assertTrue(any("остановлен: NVIDIA была выбрана" in line for line in messages))

    def test_wayland_glfw_download_is_pinned_and_cached_inside_security_cell(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            instance_dir = Path(directory) / "instance"
            instance_dir.mkdir()
            launcher = object.__new__(GameLauncher)
            launcher.installer = SimpleNamespace(http=Mock())

            with patch("nivora_launcher.launcher.os.uname", return_value=SimpleNamespace(machine="x86_64")):
                library = launcher._wayland_glfw_library(instance_dir)

        self.assertEqual(library.name, "libglfw.so")
        self.assertIn(".trial-cell/cache/wayland-glfw-v1.1.0", str(library))
        launcher.installer.http.download.assert_called_once_with(
            "https://github.com/jdkeke142/glfw-wayland-minecraft/releases/download/v1.1.0/libglfw.so",
            library,
            sha256="1fa075ea4f595bd6917a7f304f2fe5d583ec82d8f1ac0f0350bd38b32a20a46e",
            size=662440,
        )

    def test_auto_detects_nvidia_and_enables_prime_render_offload(self) -> None:
        launcher = object.__new__(GameLauncher)
        launcher.config = SimpleNamespace(settings=SimpleNamespace(graphics_mode="auto"))
        clean_environment = {"PATH": "/usr/bin", "DISPLAY": ":0", "XDG_SESSION_TYPE": "wayland"}
        nvidia_smi_result = SimpleNamespace(returncode=0, stdout="NVIDIA Test GPU\n")

        with (
            patch("nivora_launcher.launcher.shutil.which", return_value="/usr/bin/nvidia-smi"),
            patch("nivora_launcher.launcher.subprocess.run", return_value=nvidia_smi_result),
            patch("nivora_launcher.launcher.safe_process_environment", return_value=clean_environment),
        ):
            environment = launcher._launch_environment({"security_profile": "strict", "graphics_mode": "auto"})

        self.assertEqual(environment["TRIAL_LAUNCHER_GRAPHICS_MODE"], "nvidia")
        self.assertEqual(environment["__NV_PRIME_RENDER_OFFLOAD"], "1")
        self.assertEqual(environment["__GLX_VENDOR_LIBRARY_NAME"], "nvidia")
        self.assertNotIn("__VK_LAYER_NV_optimus", environment)
        self.assertNotIn("__GL_THREADED_OPTIMIZATIONS", environment)

    def test_explicit_nvidia_mode_restores_prime_after_environment_sanitizing(self) -> None:
        launcher = object.__new__(GameLauncher)
        launcher.config = SimpleNamespace(settings=SimpleNamespace(graphics_mode="auto"))

        with (
            patch("nivora_launcher.launcher.shutil.which") as locate_nvidia_smi,
            patch("nivora_launcher.launcher.safe_process_environment", return_value={"PATH": "/usr/bin"}),
        ):
            environment = launcher._launch_environment({"security_profile": "strict", "graphics_mode": "nvidia"})

        locate_nvidia_smi.assert_not_called()
        self.assertEqual(environment["TRIAL_LAUNCHER_GRAPHICS_MODE"], "nvidia")
        self.assertEqual(environment["__NV_PRIME_RENDER_OFFLOAD"], "1")
        self.assertEqual(environment["__GLX_VENDOR_LIBRARY_NAME"], "nvidia")
        self.assertNotIn("__VK_LAYER_NV_optimus", environment)
        self.assertNotIn("__GL_THREADED_OPTIMIZATIONS", environment)

    def test_auto_keeps_system_glx_when_nvidia_is_not_detected(self) -> None:
        launcher = object.__new__(GameLauncher)
        launcher.config = SimpleNamespace(settings=SimpleNamespace(graphics_mode="auto"))

        with (
            patch("nivora_launcher.launcher.shutil.which", return_value=None),
            patch("nivora_launcher.launcher.safe_process_environment", return_value={"PATH": "/usr/bin"}),
        ):
            environment = launcher._launch_environment({"security_profile": "strict", "graphics_mode": "auto"})

        self.assertEqual(environment["TRIAL_LAUNCHER_GRAPHICS_MODE"], "auto")
        self.assertNotIn("__NV_PRIME_RENDER_OFFLOAD", environment)
        self.assertNotIn("__GLX_VENDOR_LIBRARY_NAME", environment)
        self.assertNotIn("__GL_THREADED_OPTIMIZATIONS", environment)

    def test_glx_bad_fbconfig_retries_from_auto_and_explicit_driver_modes(self) -> None:
        for graphics_mode in ("auto", "mesa"):
            with self.subTest(graphics_mode=graphics_mode):
                self._assert_glx_fallback(graphics_mode)

    def test_nvidia_glx_failure_does_not_fall_back_to_llvmpipe(self) -> None:
        self._assert_nvidia_glx_failure("nvidia")
        self._assert_nvidia_glx_failure("auto", configured_graphics_mode="nvidia")

    def _assert_nvidia_glx_failure(self, graphics_mode: str, configured_graphics_mode: str = "auto") -> None:
        error = "GLFW error 65543: GLX: Failed to create context: GLXBadFBConfig"
        process = _FakeProcess(error, 1, 401)
        process_calls: list[dict[str, str]] = []

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            instance_dir = root / "instance"
            instance_dir.mkdir()
            launcher = object.__new__(GameLauncher)
            launcher.config = SimpleNamespace(
                paths=SimpleNamespace(logs_dir=root / "logs", data_home=root / "data"),
                settings=SimpleNamespace(graphics_mode=configured_graphics_mode),
            )
            launcher.process = None
            launcher.cancellation = None
            launcher.build_command = lambda *_args: ["java", "Main"]

            def prepare(command, cwd, environment, _settings, **_kwargs):
                return command, cwd, dict(environment), None

            def popen(_command, **options):
                process_calls.append(dict(options["env"]))
                return process

            account = SimpleNamespace(kind="microsoft", access_token="", username="Player")
            install = SimpleNamespace(version_id="1.12.2", instance_dir=instance_dir)
            with (
                patch(
                    "nivora_launcher.launcher.safe_process_environment",
                    return_value={"PATH": "/usr/bin", "XDG_SESSION_TYPE": "x11"},
                ),
                patch("nivora_launcher.launcher.prepare_process", side_effect=prepare),
                patch("nivora_launcher.launcher.java_bitness", return_value=64),
                patch("nivora_launcher.launcher.java_probe", return_value=(21, 64)),
                patch("nivora_launcher.launcher.subprocess.Popen", side_effect=popen),
                patch("nivora_launcher.launcher.shutil.which", return_value=None),
                patch("nivora_launcher.launcher.os.killpg", side_effect=ProcessLookupError),
                patch("nivora_launcher.launcher.InstanceProcessLock") as process_lock,
                patch("nivora_launcher.launcher.InstanceStore") as instance_store,
            ):
                process_lock.return_value.acquire.return_value = None
                process_lock.return_value.release.return_value = None
                instance_store.return_value.add_played_seconds.return_value = None
                result = launcher.run(
                    install,
                    account,
                    output=lambda _line: None,
                    settings={"security_profile": "compat", "graphics_mode": graphics_mode},
                )

        self.assertEqual(result, -15)
        self.assertEqual(len(process_calls), 1)
        self.assertTrue(process.terminate_called)

    def _assert_glx_fallback(self, graphics_mode: str, configured_graphics_mode: str = "auto") -> None:
        error = "GLFW error 65543: GLX: Failed to create context: GLXBadFBConfig"
        processes = [_FakeProcess(error, 1, 101), _FakeProcess("Minecraft started", 0, 102)]
        launch_environments: list[dict[str, str]] = []
        process_calls: list[dict[str, str]] = []

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            instance_dir = root / "instance"
            instance_dir.mkdir()
            config = SimpleNamespace(
                paths=SimpleNamespace(logs_dir=root / "logs", data_home=root / "data"),
                settings=SimpleNamespace(graphics_mode=configured_graphics_mode),
            )
            launcher = object.__new__(GameLauncher)
            launcher.config = config
            launcher.process = None
            launcher.cancellation = None
            launcher.build_command = lambda *_args: ["java", "Main"]

            initial_environment = {
                "PATH": "/usr/bin",
                "HOME": str(root),
                "XDG_SESSION_TYPE": "wayland",
                "WAYLAND_DISPLAY": "wayland-0",
                "__GLX_VENDOR_LIBRARY_NAME": "nvidia",
                "MESA_LOADER_DRIVER_OVERRIDE": "llvmpipe",
                "__NV_PRIME_RENDER_OFFLOAD": "1",
                "__VK_LAYER_NV_optimus": "NVIDIA_only",
            }

            def prepare(command, cwd, environment, _settings, **_kwargs):
                launch_environments.append(dict(environment))
                return command, cwd, dict(environment), None

            def popen(_command, **options):
                process_calls.append(dict(options["env"]))
                return processes[len(process_calls) - 1]

            account = SimpleNamespace(kind="microsoft", access_token="", username="Player")
            install = SimpleNamespace(version_id="1.12.2", instance_dir=instance_dir)
            with (
                patch(
                    "nivora_launcher.launcher.safe_process_environment",
                    side_effect=lambda **_kwargs: dict(initial_environment),
                ),
                 patch("nivora_launcher.launcher.prepare_process", side_effect=prepare),
                patch("nivora_launcher.launcher.java_bitness", return_value=64),
                patch("nivora_launcher.launcher.java_probe", return_value=(21, 64)),
                  patch("nivora_launcher.launcher.subprocess.Popen", side_effect=popen),
                  patch("nivora_launcher.launcher.shutil.which", return_value=None),
                  patch("nivora_launcher.launcher.os.killpg", side_effect=ProcessLookupError),
                 patch("nivora_launcher.launcher.InstanceProcessLock") as process_lock,
                 patch("nivora_launcher.launcher.InstanceStore") as instance_store,
            ):
                process_lock.return_value.acquire.return_value = None
                process_lock.return_value.release.return_value = None
                instance_store.return_value.add_played_seconds.return_value = None
                result = launcher.run(
                    install,
                    account,
                    output=lambda _line: None,
                    settings={"security_profile": "compat", "graphics_mode": graphics_mode},
                )

        self.assertEqual(result, 0)
        self.assertEqual(len(process_calls), 2)
        self.assertEqual(len(launch_environments), 2)
        self.assertTrue(processes[0].terminate_called)
        self.assertFalse(processes[1].terminate_called)

        first_attempt = process_calls[0]
        expected_mode = configured_graphics_mode if graphics_mode == "auto" else graphics_mode
        self.assertEqual(first_attempt["TRIAL_LAUNCHER_GRAPHICS_MODE"], expected_mode)
        self.assertNotIn("LIBGL_ALWAYS_SOFTWARE", first_attempt)
        self.assertEqual(first_attempt["MESA_LOADER_DRIVER_OVERRIDE"], "llvmpipe")
        expected_vendor = "mesa" if expected_mode == "mesa" else "nvidia"
        self.assertEqual(first_attempt["__GLX_VENDOR_LIBRARY_NAME"], expected_vendor)
        if expected_mode == "mesa":
            self.assertNotIn("__NV_PRIME_RENDER_OFFLOAD", first_attempt)
            self.assertNotIn("__VK_LAYER_NV_optimus", first_attempt)
        else:
            self.assertEqual(first_attempt["__NV_PRIME_RENDER_OFFLOAD"], "1")
            self.assertEqual(first_attempt["__VK_LAYER_NV_optimus"], "NVIDIA_only")

        retry = process_calls[1]
        self.assertEqual(retry["TRIAL_LAUNCHER_GRAPHICS_MODE"], "software")
        self.assertEqual(retry["__GLX_VENDOR_LIBRARY_NAME"], "mesa")
        self.assertEqual(retry["LIBGL_ALWAYS_SOFTWARE"], "1")
        self.assertEqual(retry["GALLIUM_DRIVER"], "llvmpipe")
        self.assertNotIn("MESA_LOADER_DRIVER_OVERRIDE", retry)
        self.assertNotIn("__NV_PRIME_RENDER_OFFLOAD", retry)
        self.assertNotIn("__VK_LAYER_NV_optimus", retry)


if __name__ == "__main__":
    unittest.main()
