from __future__ import annotations

import json
import tempfile
import unittest
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

from nivora_launcher.config import DEFAULT_OVERLAY_SETTINGS, _normalize_overlay_settings
from nivora_launcher.errors import LauncherError

import pytest
from nivora_launcher.gui_service import handle
from nivora_launcher.recorder import GameRecorder, build_recording_args, pick_encoder


class RecordingArgsTests(unittest.TestCase):
    def test_nvenc_prefers_low_latency_hardware_pipeline(self) -> None:
        args = build_recording_args(
            display=":0",
            fps=60,
            quality=23,
            encoder="nvenc",
            container="mp4",
            audio=True,
            output="/tmp/game.mp4",
        )
        self.assertIn("-c:v", args)
        self.assertEqual(args[args.index("-c:v") + 1], "h264_nvenc")
        self.assertIn(("-tune", "ll"), list(zip(args, args[1:])))
        self.assertIn(("-cq", "23"), list(zip(args, args[1:])))
        self.assertIn(("-f", "x11grab"), list(zip(args, args[1:])))
        self.assertIn(("-f", "pulse"), list(zip(args, args[1:])))
        self.assertIn("-movflags", args)
        self.assertEqual(args[-1], "/tmp/game.mp4")

    def test_vaapi_uploads_frames_to_gpu_and_skips_b_frames(self) -> None:
        args = build_recording_args(
            display=":1",
            fps=30,
            quality=25,
            encoder="vaapi",
            container="mkv",
            audio=False,
            output="/tmp/game.mkv",
        )
        self.assertIn(("-vaapi_device", "/dev/dri/renderD128"), list(zip(args, args[1:])))
        self.assertIn("format=nv12,hwupload", args)
        self.assertEqual(args[args.index("-c:v") + 1], "h264_vaapi")
        self.assertIn("-an", args)
        self.assertNotIn("-movflags", args)
        self.assertIn(("-f", "matroska"), list(zip(args, args[1:])))

    def test_x264_fallback_is_fast_and_clamps_extremes(self) -> None:
        args = build_recording_args(
            display=":0",
            fps=500,
            quality=1,
            encoder="x264",
            container="mp4",
            audio=False,
            output="/tmp/g.mp4",
        )
        self.assertEqual(args[args.index("-c:v") + 1], "libx264")
        self.assertIn(("-preset", "veryfast"), list(zip(args, args[1:])))
        self.assertEqual(args[args.index("-framerate") + 1], "120")
        self.assertEqual(args[args.index("-crf") + 1], "15")


class PickEncoderTests(unittest.TestCase):
    def test_auto_prefers_nvenc_then_vaapi(self) -> None:
        self.assertEqual(pick_encoder("auto", {"h264_nvenc", "libx264"}), "nvenc")
        with patch("nivora_launcher.recorder._vaapi_available", return_value=True):
            self.assertEqual(pick_encoder("auto", {"h264_vaapi"}), "vaapi")
            self.assertEqual(pick_encoder("vaapi", {"h264_vaapi"}), "vaapi")
        with patch("nivora_launcher.recorder._vaapi_available", return_value=False):
            self.assertEqual(pick_encoder("auto", {"h264_vaapi"}), "x264")
            self.assertEqual(pick_encoder("vaapi", {"h264_vaapi"}), "x264")

    def test_forced_encoder_falls_back_when_unavailable(self) -> None:
        self.assertEqual(pick_encoder("nvenc", set()), "x264")
        self.assertEqual(pick_encoder("x264", {"h264_nvenc"}), "x264")


class GameRecorderLifecycleTests(unittest.TestCase):
    def _config(self, tmp: Path) -> SimpleNamespace:
        return SimpleNamespace(
            game_dir=str(tmp),
            settings=SimpleNamespace(overlay_settings=dict(DEFAULT_OVERLAY_SETTINGS)),
        )

    def test_start_builds_process_and_stop_flushes_file(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            process = Mock()
            process.poll.return_value = None
            process.wait.return_value = 0
            process.stdin = Mock(closed=False)
            recorder = GameRecorder()
            with patch("nivora_launcher.recorder.shutil.which", return_value="/usr/bin/ffmpeg"), \
                    patch.dict("os.environ", {"DISPLAY": ":0"}), \
                    patch("nivora_launcher.recorder.detect_encoders", return_value={"h264_nvenc", "libx264"}), \
                    patch("nivora_launcher.recorder.subprocess.Popen", return_value=process) as popen:
                status = recorder.start(self._config(tmp), "Wild Forest")
                self.assertTrue(status["active"])
                self.assertEqual(status["encoder"], "h264_nvenc")
                self.assertIn("Wild Forest", status["file"])
                launched = popen.call_args[0][0]
                self.assertEqual(launched[0], "/usr/bin/ffmpeg")
                self.assertIn("h264_nvenc", launched)

                stopped = recorder.stop()
            self.assertFalse(stopped["active"])
            process.stdin.write.assert_called_once_with("q")
            process.wait.assert_called()

    def test_start_requires_display_and_ffmpeg(self) -> None:
        recorder = GameRecorder()
        config = self._config(Path("/tmp"))
        with patch("nivora_launcher.recorder.shutil.which", return_value=None):
            self.assertRaisesRegex(LauncherError, "ffmpeg", recorder.start, config)
        with patch("nivora_launcher.recorder.shutil.which", return_value="/usr/bin/ffmpeg"), \
                patch.dict("os.environ", {"DISPLAY": ""}, clear=True):
            self.assertRaisesRegex(LauncherError, "X11", recorder.start, config)

    def test_double_start_is_rejected(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory() as tmp_name:
            tmp = Path(tmp_name)
            recorder = GameRecorder()
            process = Mock()
            process.poll.return_value = None
            process.stdin = Mock()
            with patch("nivora_launcher.recorder.shutil.which", return_value="/usr/bin/ffmpeg"), \
                    patch.dict("os.environ", {"DISPLAY": ":0"}), \
                    patch("nivora_launcher.recorder.detect_encoders", return_value=set()), \
                    patch("nivora_launcher.recorder.subprocess.Popen", return_value=process):
                recorder.start(self._config(tmp), "")
                self.assertRaisesRegex(LauncherError, "уже идёт", recorder.start, self._config(tmp), "")
                recorder.stop()


class OverlaySettingsNormalizationTests(unittest.TestCase):
    def test_defaults_survive_roundtrip_and_garbage_is_clamped(self) -> None:
        normalized = _normalize_overlay_settings(
            {
                "enabled": "no",
                "hotkey_overlay": "Ctrl+X; rm -rf",
                "recording_fps": 999,
                "recording_quality": -3,
                "recording_encoder": "divx",
                "recording_container": "avi",
            }
        )
        self.assertFalse(normalized["enabled"])
        self.assertEqual(normalized["hotkey_overlay"], "Shift+Tab")
        self.assertEqual(normalized["recording_fps"], 120)
        self.assertEqual(normalized["recording_quality"], 15)
        self.assertEqual(normalized["recording_encoder"], "auto")
        self.assertEqual(normalized["recording_container"], "mp4")
        self.assertEqual(_normalize_overlay_settings(None), DEFAULT_OVERLAY_SETTINGS)

class OverlayStateRoutingTests(unittest.TestCase):
    """overlay_state обслуживается движком в lib.rs (rust_engine/state.rs),
    до этого роутера. Поведение покрыто cargo test rust_engine::state и
    скриптом scripts/parity_overlay_state.sh; здесь фиксируется, что роутер
    операцию не обслуживает — иначе два исполнителя отвечали бы по-разному.
    """

    def test_overlay_state_is_not_served_by_this_router(self) -> None:
        with pytest.raises(LauncherError):
            handle({"operation": "overlay_state"})


class RecordingStateFileTests(unittest.TestCase):
    """Файл состояния записи — контракт, по которому оверлей читает статус
    записи (см. rust_engine/state.rs::recording_status)."""

    def test_write_and_remove_recording_state(self) -> None:
        from nivora_launcher.recorder import RECORDING_STATE_FILE, remove_recording_state, write_recording_state

        with tempfile.TemporaryDirectory() as temporary:
            output_dir = Path(temporary)
            payload = {"pid": 12345, "file": "/x.mp4", "started_at": 1.5, "encoder": "libx264", "instance": "T"}
            state_path = write_recording_state(output_dir, payload)
            self.assertEqual(state_path.name, RECORDING_STATE_FILE)
            self.assertEqual(json.loads(state_path.read_text(encoding="utf-8")), payload)
            # Повторная запись атомарно заменяет файл, а не оставляет .part.
            write_recording_state(output_dir, {**payload, "pid": 67890})
            self.assertEqual(json.loads(state_path.read_text(encoding="utf-8"))["pid"], 67890)
            self.assertEqual([p.name for p in output_dir.iterdir()], [RECORDING_STATE_FILE])
            remove_recording_state(state_path)
            self.assertFalse(state_path.exists())
            remove_recording_state(state_path)  # повторное удаление молчит


if __name__ == "__main__":
    unittest.main()
