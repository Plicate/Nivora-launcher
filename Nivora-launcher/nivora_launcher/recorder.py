"""Запись игрового процесса для оверлея Nivora (F7).

Захват выполняется внешним ffmpeg: x11grab для картинки, pulse для звука и
аппаратный кодирователь, когда он доступен (NVENC на NVIDIA, VAAPI на
Intel/AMD), иначе быстрый пресет libx264. Аргументы собирает чистая функция
`build_recording_args` — она покрыта юнит-тестами и не трогает процессы.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import time
from pathlib import Path
from typing import Any

from .config import ConfigStore
from .errors import LauncherError

_ENCODER_NVENC = "h264_nvenc"

RECORDING_STATE_FILE = "recording-state.json"


def write_recording_state(output_dir: Path, payload: dict[str, Any]) -> Path:
    """Атомарно записать файл состояния записи (см. GameRecorder._state_file)."""
    import json
    import tempfile

    output_dir.mkdir(parents=True, exist_ok=True)
    state_path = output_dir / RECORDING_STATE_FILE
    fd, temporary_name = tempfile.mkstemp(prefix=f".{RECORDING_STATE_FILE}.", suffix=".part", dir=output_dir)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(payload, handle, ensure_ascii=False)
        os.replace(temporary_name, state_path)
    except OSError:
        try:
            os.unlink(temporary_name)
        except OSError:
            pass
        raise
    return state_path


def remove_recording_state(state_path: Path) -> None:
    try:
        state_path.unlink()
    except OSError:
        pass
_ENCODER_VAAPI = "h264_vaapi"
_ENCODER_X264 = "libx264"
_DEFAULT_VAAPI_DEVICE = "/dev/dri/renderD128"


def build_recording_args(
    *,
    display: str,
    fps: int,
    quality: int,
    encoder: str,
    container: str,
    audio: bool,
    output: str,
    vaapi_device: str = _DEFAULT_VAAPI_DEVICE,
) -> list[str]:
    """Собрать аргументы ffmpeg для записи экрана с минимальной нагрузкой.

    Порядок важен: глобальные опции, входы, кодеки, выход. Качество `quality`
    — это CRF/CQ: меньше — лучше.
    """
    fps = max(15, min(120, int(fps)))
    quality = max(15, min(35, int(quality)))
    # stdin остаётся открытым: остановка записи — это буква «q» в stdin ffmpeg.
    args = ["-hide_banner", "-loglevel", "warning"]
    if encoder == "vaapi":
        args += ["-vaapi_device", vaapi_device]

    args += ["-f", "x11grab", "-draw_mouse", "1", "-framerate", str(fps), "-i", display]
    if audio:
        args += ["-f", "pulse", "-fragment_size", "1024", "-i", "default"]

    if encoder == "nvenc":
        args += [
            "-c:v", _ENCODER_NVENC,
            "-preset", "p4",
            "-tune", "ll",
            "-rc", "vbr",
            "-cq", str(quality),
            "-b:v", "0",
            "-pix_fmt", "yuv420p",
        ]
    elif encoder == "vaapi":
        args += [
            "-vf", "format=nv12,hwupload",
            "-c:v", _ENCODER_VAAPI,
            "-bf", "0",
            "-qp", str(quality),
        ]
    else:
        args += [
            "-c:v", _ENCODER_X264,
            "-preset", "veryfast",
            "-crf", str(quality),
            "-pix_fmt", "yuv420p",
        ]

    if audio:
        args += ["-c:a", "aac", "-b:a", "160k"]
    else:
        args += ["-an"]
    if container == "mkv":
        args += ["-f", "matroska"]
    else:
        args += ["-movflags", "+faststart", "-f", "mp4"]
    args.append(output)
    return args


def detect_encoders(ffmpeg: str) -> set[str]:
    """Названия кодировщиков, которые собирает этот ffmpeg."""
    known = {_ENCODER_NVENC, _ENCODER_VAAPI, _ENCODER_X264}
    try:
        completed = subprocess.run(
            [ffmpeg, "-hide_banner", "-encoders"],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            stdin=subprocess.DEVNULL,
            text=True,
            timeout=10,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return set()
    text = completed.stdout or ""
    return {name for name in known if name in text}


def _vaapi_available() -> bool:
    return Path(_DEFAULT_VAAPI_DEVICE).exists()


def pick_encoder(requested: str, encoders: set[str]) -> str:
    """Выбрать тип кодировщика («nvenc»/«vaapi»/«x264»): приоритет
    аппаратному, форс можно в настройках."""
    if requested == "nvenc":
        return "nvenc" if _ENCODER_NVENC in encoders else "x264"
    if requested == "vaapi":
        if _ENCODER_VAAPI in encoders and _vaapi_available():
            return "vaapi"
        return "x264"
    if requested == "x264":
        return "x264"
    if _ENCODER_NVENC in encoders:
        return "nvenc"
    if _ENCODER_VAAPI in encoders and _vaapi_available():
        return "vaapi"
    return "x264"


class GameRecorder:
    """Один активный процесс ffmpeg на весь лаунчер (воркер сериален)."""

    def __init__(self) -> None:
        self._process: subprocess.Popen[str] | None = None
        self._log_handle = None
        self._file = ""
        self._encoder = ""
        self._started_at = 0.0
        self._instance = ""
        self._encoders_cache: set[str] | None = None
        # Статус записи читается с диска (recordings/recording-state.json),
        # а не из памяти этого процесса: живость ffmpeg проверяется по pid.
        self._state_file: Path | None = None

    # --- Состояние ---------------------------------------------------------
    def status(self) -> dict[str, Any]:
        active = self._process is not None and self._process.poll() is None
        file_size = 0
        if self._file:
            try:
                file_size = Path(self._file).stat().st_size
            except OSError:
                file_size = 0
        return {
            "active": bool(active),
            "file": self._file,
            "size_bytes": file_size,
            "started_at": self._started_at,
            "elapsed_seconds": int(max(0.0, time.time() - self._started_at)) if active else 0,
            "encoder": self._encoder,
            "instance": self._instance,
        }

    # --- Управление --------------------------------------------------------
    def start(self, config: ConfigStore, instance_name: str = "") -> dict[str, Any]:
        if self._process is not None and self._process.poll() is None:
            raise LauncherError("Запись уже идёт — сначала остановите текущую")
        ffmpeg = shutil.which("ffmpeg")
        if not ffmpeg:
            raise LauncherError("ffmpeg не найден. Установите ffmpeg, чтобы записывать игры")
        display = os.environ.get("DISPLAY") or ""
        if not display:
            raise LauncherError("Запись доступна в X11/XWayland-сессии: переменная DISPLAY не задана")

        settings = config.settings.overlay_settings
        if self._encoders_cache is None:
            self._encoders_cache = detect_encoders(ffmpeg)
        encoder_kind = pick_encoder(str(settings.get("recording_encoder", "auto")), self._encoders_cache)
        encoder_name = {"nvenc": _ENCODER_NVENC, "vaapi": _ENCODER_VAAPI}.get(encoder_kind, _ENCODER_X264)
        container = str(settings.get("recording_container", "mp4"))
        safe_instance = "".join(character if character.isalnum() or character in "-_ " else "-" for character in instance_name.strip())[:48].strip() or "game"
        output_dir = Path(config.game_dir).expanduser() / "recordings"
        output_dir.mkdir(parents=True, exist_ok=True)
        output = str(output_dir / f"{time.strftime('%Y-%m-%d_%H-%M-%S')} {safe_instance}.{container}")
        log_path = output_dir / "last-recording.log"

        args = [ffmpeg, *build_recording_args(
            display=display,
            fps=int(settings.get("recording_fps", 60)),
            quality=int(settings.get("recording_quality", 23)),
            encoder=encoder_kind,
            container=container,
            audio=bool(settings.get("recording_audio", True)),
            output=output,
        )]
        self._log_handle = log_path.open("w", encoding="utf-8")
        try:
            self._process = subprocess.Popen(
                args,
                stdin=subprocess.PIPE,
                stdout=subprocess.DEVNULL,
                stderr=self._log_handle,
                text=True,
                # Бэкенд работает без консоли (--noconsole), поэтому ffmpeg
                # без этого флага получил бы собственное окно консоли.
                **(
                    {"creationflags": getattr(subprocess, "CREATE_NO_WINDOW", 0)}
                    if os.name == "nt"
                    else {}
                ),
            )
        except (OSError, subprocess.SubprocessError) as exc:
            self._log_handle.close()
            self._log_handle = None
            raise LauncherError(f"Не удалось запустить ffmpeg: {exc}") from exc
        self._file = output
        self._encoder = encoder_name
        self._started_at = time.time()
        self._instance = instance_name
        # Ошибка записи файла состояния не должна ломать саму запись:
        # состояние вторично, ffmpeg уже работает.
        try:
            try:
                pid_value = int(self._process.pid)
            except (TypeError, ValueError):
                pid_value = 0
            self._state_file = write_recording_state(
                output_dir,
                {
                    "pid": pid_value,
                    "file": self._file,
                    "started_at": self._started_at,
                    "encoder": self._encoder,
                    "instance": self._instance,
                },
            )
        except OSError:
            self._state_file = None
        return self.status()

    def stop(self) -> dict[str, Any]:
        process = self._process
        if process is None or process.poll() is not None:
            summary = self.status()
            self._cleanup()
            summary["active"] = False
            return summary
        try:
            if process.stdin and not process.stdin.closed:
                process.stdin.write("q")
                process.stdin.flush()
        except (OSError, ValueError):
            pass
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        summary = self.status()
        summary["active"] = False
        self._cleanup()
        return summary

    def _cleanup(self) -> None:
        self._process = None
        if self._log_handle is not None:
            try:
                self._log_handle.close()
            except OSError:
                pass
            self._log_handle = None
        self._started_at = 0.0
        if self._state_file is not None:
            remove_recording_state(self._state_file)
            self._state_file = None


recorder = GameRecorder()
