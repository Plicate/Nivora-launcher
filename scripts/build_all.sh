#!/usr/bin/env bash
# Универсальный сборщик Nivora Launcher: прогоняет платформенные скрипты по
# списку целей. Совместим с /bin/bash 3.2 (macOS): без ассоциативных массивов.
#
#   scripts/build_all.sh                 # цели по умолчанию для этой ОС
#   scripts/build_all.sh deb rpm         # только перечисленные
#   MACOS_ARCH=both scripts/build_all.sh macos   # arm64 и x86_64
#
# Цели и артефакты (dist/):
#   windows — build_windows_exe.sh → dist/windows/*-setup.exe, nivora-launcher.exe
#   arch    — build_arch_package.sh → dist/arch/*.pkg.tar.zst
#   deb     — build_deb.sh          → dist/deb/*.deb
#   rpm     — build_rpm.sh          → dist/rpm/*.rpm
#   macos   — build_macos_app.sh    → dist/macos/Nivora-Launcher_<ver>_<arch>.dmg
#
# Каждая цель сама собирает свои нативные хелперы (nivora-turbo, nivora-modcat)
# под свою платформу и падает, если хелпер не собрался. После цели проверяется,
# что артефакт действительно появился: «успех» без артефакта невозможен.
# Падение одной цели не останавливает остальные; итоговый код — ненулевой.
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
PROJECT_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

log() { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
err() { printf '\033[1;31m%s\033[0m\n' "$*" >&2; }

script_of() {
  case "$1" in
    windows) echo build_windows_exe.sh ;;
    arch) echo build_arch_package.sh ;;
    deb) echo build_deb.sh ;;
    rpm) echo build_rpm.sh ;;
    macos) echo build_macos_app.sh ;;
  esac
}

artifact_glob() {
  case "$1" in
    windows) echo "dist/windows/*-setup.exe" ;;
    arch) echo "dist/arch/*.pkg.tar.zst" ;;
    deb) echo "dist/deb/*.deb" ;;
    rpm) echo "dist/rpm/*.rpm" ;;
    macos) echo "dist/macos/Nivora-Launcher_*_$2.dmg" ;;
  esac
}

case "$(uname -s)" in
  Darwin) DEFAULT_TARGETS="macos" ;;
  MINGW*|MSYS*|CYGWIN*) DEFAULT_TARGETS="windows" ;;
  *) DEFAULT_TARGETS="windows arch deb rpm" ;;
esac

targets=""
for arg in "$@"; do
  case "$arg" in
    all) targets="windows arch deb rpm macos" ;;
    windows|arch|deb|rpm|macos)
      case " $targets " in *" $arg "*) ;; *) targets="$targets $arg" ;; esac
      ;;
    *)
      err "Неизвестная цель: $arg. Доступны: all windows arch deb rpm macos"
      exit 2
      ;;
  esac
done
[ -n "$targets" ] || targets="$DEFAULT_TARGETS"

summary=""
failed=0
record() { summary="$summary\n  $(printf '%-16s %s' "$1" "$2")"; }

run_target() {
  local target="$1" arch="${2:-}" label="$1" script pattern found
  [ -n "$arch" ] && label="$target/$arch"
  script="$SCRIPT_DIR/$(script_of "$target")"
  log "=== Цель: $label ($(basename "$script")) ==="
  if [ -n "$arch" ]; then
    MACOS_ARCH="$arch" bash "$script" || { record "$label" "FAIL"; failed=$((failed + 1)); return; }
  else
    bash "$script" || { record "$label" "FAIL"; failed=$((failed + 1)); return; }
  fi
  pattern="$(artifact_glob "$target" "$arch")"
  found="$(cd "$PROJECT_ROOT" && ls -1 $pattern 2>/dev/null | head -n 1 || true)"
  if [ -z "$found" ]; then
    err "Цель $label завершилась без артефакта ($pattern)"
    record "$label" "FAIL (нет артефакта)"
    failed=$((failed + 1))
    return
  fi
  record "$label" "OK  $found"
}

for target in $targets; do
  if [ "$target" = macos ]; then
    case "${MACOS_ARCH:-native}" in
      both|all) run_target macos arm64; run_target macos x86_64 ;;
      native) run_target macos "$( [ "$(uname -m)" = arm64 ] && echo arm64 || echo x86_64)" ;;
      *) run_target macos "$MACOS_ARCH" ;;
    esac
  else
    run_target "$target"
  fi
done

log "=== Сводка сборки ==="
printf '%b\n' "$summary"
if [ "$failed" -ne 0 ]; then
  err "Целей с ошибками: $failed"
  exit 1
fi
log "Все цели собраны. Артефакты: $PROJECT_ROOT/dist/"
