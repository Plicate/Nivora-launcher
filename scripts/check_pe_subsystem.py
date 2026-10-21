"""Проверка подсистемы PE-файлов: --gui требует WINDOWS_GUI (2), --console — 3.

Сборочный glue: production-exe не должен открывать окно консоли.
"""

import struct
import sys

NAMES = {2: "GUI", 3: "CONSOLE"}


def subsystem(path: str) -> int:
    with open(path, "rb") as handle:
        header = handle.read(4096)
    if header[:2] != b"MZ":
        raise ValueError("не PE-файл")
    offset = struct.unpack_from("<I", header, 0x3C)[0]
    if header[offset:offset + 4] != b"PE\0\0":
        raise ValueError("нет сигнатуры PE")
    return struct.unpack_from("<H", header, offset + 24 + 68)[0]


def main(argv: list[str]) -> int:
    if len(argv) < 2 or argv[0] not in ("--gui", "--console"):
        print("использование: check_pe_subsystem.py --gui|--console FILE...", file=sys.stderr)
        return 2
    expected = 2 if argv[0] == "--gui" else 3
    failed = 0
    for path in argv[1:]:
        try:
            value = subsystem(path)
        except (OSError, ValueError, struct.error) as error:
            print(f"{path}: {error}", file=sys.stderr)
            failed += 1
            continue
        status = "OK" if value == expected else "ОШИБКА"
        print(f"{status}: {path}: подсистема {NAMES.get(value, value)}")
        failed += value != expected
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
