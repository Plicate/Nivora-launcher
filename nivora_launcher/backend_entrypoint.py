from __future__ import annotations

import sys


if __name__ == "__main__":
    if sys.argv[1:2] == ["--win-sandbox"]:
        # Windows-песочница экземпляра: короткий helper без GUI-модулей.
        from nivora_launcher.windows_sandbox import main as sandbox_main

        raise SystemExit(sandbox_main(sys.argv[2:]))
    if sys.argv[1:2] == ["--cli"]:
        from nivora_launcher.cli import main as cli_main

        raise SystemExit(cli_main(sys.argv[2:]))
    from nivora_launcher.gui_service import main as gui_main

    raise SystemExit(gui_main())
