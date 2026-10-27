#!/usr/bin/env bash
# собирает nivora-modcat и кладёт ModScan.java рядом с бинарником
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-$ROOT/frontend/src-tauri/resources/backend/nivora-modcat}"
GO="${GO:-go}"
if ! command -v "$GO" >/dev/null 2>&1; then
    for candidate in /usr/local/go/bin/go /usr/lib/go/bin/go "$HOME/.local/go/bin/go"; do
        [ -x "$candidate" ] && GO="$candidate" && break
    done
fi
if ! command -v "$GO" >/dev/null 2>&1 && [ ! -x "$GO" ]; then
    want="$(sed -n 's/^go \([0-9][0-9.]*\)$/\1/p' "$ROOT/go-modcat/go.mod" | head -n 1)"
    echo "nivora-modcat: компилятор Go не найден (нужен golang >= ${want:-1.24}, см. go-modcat/go.mod)" >&2
    echo "  Arch/CachyOS: sudo pacman -S go    Fedora: sudo dnf install golang (проверьте версию)" >&2
    echo "  Иначе: https://go.dev/dl/" >&2
    exit 1
fi
# при GOTOOLCHAIN=auto Go сам скачает минимальную версию из go.mod
want="$(sed -n 's/^go \([0-9][0-9]*\.[0-9][0-9]*\).*/\1/p' "$ROOT/go-modcat/go.mod" | head -n 1)"
have="$("$GO" env GOVERSION 2>/dev/null | sed -n 's/^go\([0-9][0-9]*\.[0-9][0-9]*\).*/\1/p')"
if [ -n "$want" ] && [ -n "$have" ]; then
    if [ "$(printf '%s\n%s\n' "$want" "$have" | sort -V | head -n 1)" != "$want" ]; then
        toolchain="$("$GO" env GOTOOLCHAIN 2>/dev/null || echo auto)"
        case "$toolchain" in
            local|go1.*)
                echo "nivora-modcat: установлен Go $have, нужен >= $want (go-modcat/go.mod)" >&2
                exit 1
                ;;
            *)
                echo "nivora-modcat: установлен Go $have, будет скачан toolchain >= $want (GOTOOLCHAIN=$toolchain)" >&2
                ;;
        esac
    fi
fi

mkdir -p "$(dirname "$OUT")"
# удаляем остатки прошлой сборки: Go ожидает по этому пути файл
rm -rf "$OUT"
(cd "$ROOT/go-modcat" && "$GO" build -trimpath -ldflags='-s -w' -o "$OUT" .)

# локальный smoke test выполняется только для бинарника хостовой платформы
HOST_OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
TARGET_OS="${GOOS:-$HOST_OS}"
if [ "$TARGET_OS" = "$(echo "$HOST_OS" | sed 's/darwin/darwin/;s/linux/linux/;s/mingw.*/windows/;s/msys.*/windows/;s/cygwin.*/windows/')" ]; then
    SMOKE="$(printf '{"cmd":"bogus"}' | "$OUT")"
    case "$SMOKE" in
        *'"ok":false'*|*'"ok": false'*) ;;
        *) echo "nivora-modcat: дымовой тест не прошёл: $SMOKE" >&2; exit 1 ;;
    esac
fi

# modscan запускается через JRE 17+
cp -f "$ROOT/java/ModScan.java" "$(dirname "$OUT")/ModScan.java"

echo "nivora-modcat собран и сверен: $OUT"
