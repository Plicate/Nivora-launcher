from __future__ import annotations

import sys

from nivora_launcher.cli import main as cli_main
from nivora_launcher.gui_service import main as gui_main


if __name__ == "__main__":
    if sys.argv[1:2] == ["--cli"]:
        raise SystemExit(cli_main(sys.argv[2:]))
    raise SystemExit(gui_main())
