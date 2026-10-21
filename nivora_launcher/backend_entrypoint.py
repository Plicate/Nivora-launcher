"""Точка входа упакованного Python-бэкенда Nivora Launcher (PyInstaller).

Desktop-оболочка (Tauri) запускает ``nivora-launcher-backend`` без аргументов
и общается строками JSON через stdin/stdout — это цикл ``gui_service.main``.
``--win-sandbox`` запускает Java в AppContainer (песочница экземпляра),
``--cli`` — режим командной строки.
"""

from __future__ import annotations

import os
import sys

_CREATE_NEW_CONSOLE = 0x00000010
_DETACHED_PROCESS = 0x00000008
_CREATE_NO_WINDOW = 0x08000000


def _force_utf8_streams() -> None:
    """UTF-8 для stdio: на Windows пайпы по умолчанию в ANSI (cp1251)."""
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is None:
            continue
        try:
            reconfigure(encoding="utf-8")
        except (OSError, ValueError):
            pass


def _hide_child_consoles() -> None:
    """Windows: дочерние консольные процессы без собственного окна.

    Бэкенд собран без консоли (windowed), поэтому каждый консольный дочерний
    процесс — java, powershell, taskkill, winget, ffmpeg — получал бы новое
    видимое окно. Флаг ставится всем ``subprocess.Popen`` (а значит и
    ``run``/``check_output``), кроме явно запросивших свою консоль.
    """
    if os.name != "nt":
        return
    import subprocess

    original = subprocess.Popen.__init__
    if getattr(original, "_nivora_no_window", False):
        return

    def __init__(self, *args, **kwargs):  # type: ignore[no-untyped-def]
        flags = int(kwargs.get("creationflags") or 0)
        if not flags & (_CREATE_NEW_CONSOLE | _DETACHED_PROCESS):
            kwargs["creationflags"] = flags | _CREATE_NO_WINDOW
        original(self, *args, **kwargs)

    __init__._nivora_no_window = True  # type: ignore[attr-defined]
    subprocess.Popen.__init__ = __init__  # type: ignore[method-assign]


def main(argv: list[str] | None = None) -> int:
    _force_utf8_streams()
    arguments = list(sys.argv[1:] if argv is None else argv)
    if arguments[:1] != ["--cli"]:
        # CLI запускают из терминала — там дочерним процессам окно не нужно
        # подавлять: они пишут в ту же консоль.
        _hide_child_consoles()
    if arguments[:1] == ["--win-sandbox"]:
        from nivora_launcher.windows_sandbox import main as sandbox_main

        return sandbox_main(arguments[1:])
    if arguments[:1] == ["--cli"]:
        from nivora_launcher.cli import main as cli_main

        return cli_main(arguments[1:])
    from nivora_launcher.gui_service import main as worker_main

    return worker_main()


if __name__ == "__main__":
    raise SystemExit(main())
