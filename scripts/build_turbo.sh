#!/usr/bin/env bash
# Сборка nivora-turbo — пакетного SHA-256 и скана jar-манифестов
# (cpp-turbo/nivora-turbo.cpp). Совместим с /bin/bash 3.2 (macOS).
#
#   scripts/build_turbo.sh [OUT]
#
# Переменные:
#   CXX              компилятор (по умолчанию g++, на macOS c++);
#   CXXFLAGS_EXTRA   доп. флаги, например «-arch x86_64» для macOS;
#   TURBO_TARGET     linux | darwin | windows (по умолчанию — по OUT/хосту).
# Для windows zlib собирается из исходников статически (скачивается
# zlib 1.3.1 со сверкой SHA-256, либо берётся из TURBO_ZLIB_SRC).
# Любая ошибка компиляции или самопроверки — ненулевой код выхода.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-$ROOT/frontend/src-tauri/resources/backend/nivora-turbo}"
HOST_OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
case "$HOST_OS" in mingw*|msys*|cygwin*) HOST_OS=windows ;; esac

TARGET="${TURBO_TARGET:-}"
if [ -z "$TARGET" ]; then
  case "$OUT" in
    *.exe) TARGET=windows ;;
    *) TARGET="$HOST_OS" ;;
  esac
fi

if [ -z "${CXX:-}" ]; then
  case "$TARGET" in
    darwin) CXX=c++ ;;
    windows) if [ "$HOST_OS" = windows ]; then CXX=g++; else CXX=x86_64-w64-mingw32-g++-posix; fi ;;
    *) CXX=g++ ;;
  esac
fi
command -v "$CXX" >/dev/null 2>&1 || { echo "nivora-turbo: компилятор $CXX не найден" >&2; exit 1; }

EXTRA=()
if [ -n "${CXXFLAGS_EXTRA:-}" ]; then
  # shellcheck disable=SC2206
  EXTRA=(${CXXFLAGS_EXTRA})
fi
FLAGS=(-O2 -std=c++17 -pthread -Wall -Wextra)
[ "${#EXTRA[@]}" -gt 0 ] && FLAGS+=("${EXTRA[@]}")

# SHA-NI: интринсики включаются, только если ЦЕЛЕВАЯ архитектура x86_64
# (диспетчер CPUID в коде сам решит при старте). Для arm64 — обычный путь.
predefined="$(echo | "$CXX" "${FLAGS[@]}" -dM -E -x c++ - 2>/dev/null || true)"
if printf '%s\n' "$predefined" | grep '__x86_64__' >/dev/null; then
  FLAGS+=(-msha -msse4.1 -mssse3)
fi

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
LINK=(-lz)
SOURCES=("$ROOT/cpp-turbo/nivora-turbo.cpp")

if [ "$TARGET" = windows ]; then
  ZLIB_VERSION=1.3.1
  ZLIB_SHA256=9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23
  ZLIB_SRC="${TURBO_ZLIB_SRC:-}"
  if [ -z "$ZLIB_SRC" ]; then
    cache="$ROOT/.build-windows/zlib-$ZLIB_VERSION"
    if [ ! -f "$cache/inflate.c" ]; then
      mkdir -p "$ROOT/.build-windows"
      archive="$ROOT/.build-windows/zlib-$ZLIB_VERSION.tar.gz"
      if [ ! -f "$archive" ]; then
        curl -fsSL -o "$archive" "https://zlib.net/fossils/zlib-$ZLIB_VERSION.tar.gz" \
          || { echo "nivora-turbo: не удалось скачать zlib (задайте TURBO_ZLIB_SRC)" >&2; exit 1; }
      fi
      actual="$( (sha256sum "$archive" 2>/dev/null || shasum -a 256 "$archive") | cut -d' ' -f1)"
      [ "$actual" = "$ZLIB_SHA256" ] || { echo "nivora-turbo: контрольная сумма zlib не совпала ($actual)" >&2; exit 1; }
      tar -xzf "$archive" -C "$ROOT/.build-windows"
    fi
    ZLIB_SRC="$cache"
  fi
  CC_WIN="${CC:-${CXX%g++*}gcc${CXX##*g++}}"
  command -v "$CC_WIN" >/dev/null 2>&1 || CC_WIN=gcc
  objects=()
  for unit in adler32 crc32 inflate inffast inftrees zutil; do
    "$CC_WIN" -O2 -c "$ZLIB_SRC/$unit.c" -I"$ZLIB_SRC" -o "$tmpdir/$unit.o"
    objects+=("$tmpdir/$unit.o")
  done
  FLAGS+=(-I"$ZLIB_SRC")
  LINK=("${objects[@]}" -static)
fi

mkdir -p "$(dirname "$OUT")"
"$CXX" "${FLAGS[@]}" "${SOURCES[@]}" -o "$OUT" "${LINK[@]}"

# Самопроверка возможна только для бинарника родной ОС.
if [ "$TARGET" != "$HOST_OS" ]; then
  echo "nivora-turbo собран (кросс, $TARGET): $OUT"
  exit 0
fi
case "${CXXFLAGS_EXTRA:-}" in
  *"-arch "*)
    want="$(printf '%s' "$CXXFLAGS_EXTRA" | sed -n 's/.*-arch \([a-z0-9_]*\).*/\1/p')"
    if [ "$want" != "$(uname -m)" ] && ! { [ "$want" = x86_64 ] && arch -x86_64 /usr/bin/true 2>/dev/null; }; then
      echo "nivora-turbo собран ($want, без запуска): $OUT"
      exit 0
    fi
    ;;
esac

# Эталонные дайджесты «abc» и пустого файла (FIPS 180-4).
ABC_EXPECTED="ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
EMPTY_EXPECTED="e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
printf 'abc' > "$tmpdir/abc.bin"
: > "$tmpdir/empty.bin"
"$OUT" sha256 "$tmpdir/abc.bin" "$tmpdir/empty.bin" > "$tmpdir/out.txt"
grep -q "^$ABC_EXPECTED" "$tmpdir/out.txt" || { echo "turbo: дайджест abc не совпал" >&2; exit 1; }
grep -q "^$EMPTY_EXPECTED" "$tmpdir/out.txt" || { echo "turbo: дайджест пустого файла не совпал" >&2; exit 1; }

# scan-jars: fabric- и forge-jar с deflate-манифестами.
PY="$(command -v python3 || command -v python || true)"
if [ -n "$PY" ]; then
  "$PY" - "$tmpdir" <<'PY'
import os, sys, zipfile
os.chdir(sys.argv[1])
with zipfile.ZipFile("mod.jar", "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("fabric.mod.json", '{"schemaVersion":1,"id":"sodiumextra","version":"0.5.4","name":"Sodium Extra"}')
    z.writestr("assets/dummy.txt", "x")
with zipfile.ZipFile("forge.jar", "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("META-INF/mods.toml", 'modLoader="javafml"\n[[mods]]\nmodId="coolmod"\nversion="1.2.3"\ndisplayName="Cool Mod"\n')
PY
  "$OUT" scan-jars "$tmpdir/mod.jar" "$tmpdir/forge.jar" > "$tmpdir/scan.txt"
  awk -F '\t' '$1=="fabric" && $2=="sodiumextra" && $3=="Sodium Extra" && $4=="0.5.4" {found=1} END {exit !found}' "$tmpdir/scan.txt" \
    || { echo "turbo: fabric-манифест не прочитан" >&2; exit 1; }
  awk -F '\t' '$1=="forge" && $2=="coolmod" && $3=="Cool Mod" && $4=="1.2.3" {found=1} END {exit !found}' "$tmpdir/scan.txt" \
    || { echo "turbo: forge-манифест не прочитан" >&2; exit 1; }
fi

echo "nivora-turbo собран и сверен: $OUT"
