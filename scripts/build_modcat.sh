#!/usr/bin/env bash
# Сборка nivora-modcat — каталога Modrinth (Go, см. go-modcat/main.go).
# Статический бинарник без зависимостей кладётся рядом с бэкендом;
# движок находит его сам (NIVORA_MODCAT_BIN / соседние каталоги).
# Опционально кладёт и java/ModScan.java — хелпер требуемой версии Java.
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
    echo "nivora-modcat: компилятор Go не найден (поставьте golang >= 1.22)" >&2
    exit 1
fi

mkdir -p "$(dirname "$OUT")"
(cd "$ROOT/go-modcat" && "$GO" build -trimpath -ldflags='-s -w' -o "$OUT" .)

# дымовой тест без сети — только для родной платформы: кросс-бинарник
# здесь не запустить
HOST_OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
TARGET_OS="${GOOS:-$HOST_OS}"
if [ "$TARGET_OS" = "$(echo "$HOST_OS" | sed 's/darwin/darwin/;s/linux/linux/;s/mingw.*/windows/;s/msys.*/windows/;s/cygwin.*/windows/')" ]; then
    SMOKE="$(printf '{"cmd":"bogus"}' | "$OUT")"
    case "$SMOKE" in
        *'"ok":false'*|*'"ok": false'*) ;;
        *) echo "nivora-modcat: дымовой тест не прошёл: $SMOKE" >&2; exit 1 ;;
    esac
fi

# Java-хелпер — просто исходник, кладём рядом: любой JRE 17+ запустит
cp -f "$ROOT/java/ModScan.java" "$(dirname "$OUT")/ModScan.java"

echo "nivora-modcat собран и сверен: $OUT"
