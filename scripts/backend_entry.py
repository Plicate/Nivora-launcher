"""Точка входа PyInstaller-сборки Python-бэкенда Nivora Launcher (Windows).

Desktop-оболочка (Tauri) запускает ``nivora-launcher-backend.exe`` без аргументов
и общается строками JSON через stdin/stdout — это цикл ``gui_service.main``.
С аргументом ``--cli`` тот же exe работает как CLI (см. ``_launcher_cli_command``):
``nivora-launcher-backend.exe --cli launch 1.21.1 --instance demo``.
"""

from __future__ import annotations

import sys


def _force_utf8_streams() -> None:
    """Принудительный UTF-8 для stdio.

    На Windows Python пишет в пайпы/файлы в ANSI-кодировке (cp1251), из-за
    чего русские строки в JSON-протоколе с Tauri ломаются, а backend.log
    превращается в кракозябры. PYTHONUTF8=1 задаёт оболочка Tauri; здесь —
    страховка для --cli-запусков из ярлыков и прямого запуска exe.
    """
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is None:  # windowed-режим: NullWriter
            continue
        try:
            reconfigure(encoding="utf-8")
        except (OSError, ValueError):
            pass


def main() -> int:
    _force_utf8_streams()
    argv = sys.argv[1:]
    if argv and argv[0] == "--cli":
        from nivora_launcher.cli import main as cli_main

        return cli_main(argv[1:])
    from nivora_launcher.gui_service import main as worker_main

    return worker_main()


if __name__ == "__main__":
    raise SystemExit(main())
