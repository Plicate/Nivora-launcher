#!/usr/bin/env bash
# Универсальный сборщик Nivora Launcher: прогоняет платформенные скрипты
# по списку целей. Каждый скрипт независим и умеет собирать сам — этот
# только оркестрирует, чтобы собрать всё одной командой.
#
#   scripts/build_all.sh                # все цели: windows arch deb rpm
#   scripts/build_all.sh deb rpm        # только перечисленные
#
# Цели и что на выходе (в dist/):
#   windows — build_windows_exe.sh → dist/windows/*-setup.exe (+ подпись
#             при WINDOWS_CERT_FILE/WINDOWS_CERT_PASSWORD)
#   arch    — build_arch_package.sh → dist/arch/*.pkg.tar.zst
#   deb     — build_deb.sh          → dist/deb/*.deb (+ подпись при DEB_SIGN_KEY)
#   rpm     — build_rpm.sh          → dist/rpm/*.rpm (+ подпись при RPM_SIGN_KEY)
#
# Сборка идёт до первого «тяжёлого» шага каждой платформы; падение одной
# цели не останавливает остальные — в конце сводка.
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
PROJECT_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

log() { printf '\033[1;34m==> %s\033[0m\n' "$*"; }

ALL_TARGETS=(windows arch deb rpm)
declare -A SCRIPT_OF=(
  [windows]="build_windows_exe.sh"
  [arch]="build_arch_package.sh"
  [deb]="build_deb.sh"
  [rpm]="build_rpm.sh"
)

targets=()
for arg in "$@"; do
  case "$arg" in
    all) targets=("${ALL_TARGETS[@]}") ;;
    windows|arch|deb|rpm) targets+=("$arg") ;;
    *)
      printf 'Неизвестная цель: %s\nДоступны: all %s\n' "$arg" "${ALL_TARGETS[*]}" >&2
      exit 2
      ;;
  esac
done
[ "${#targets[@]}" -gt 0 ] || targets=("${ALL_TARGETS[@]}")

# Уникальные цели с сохранением порядка.
seen=()
for t in "${targets[@]}"; do
  skip=0
  for s in "${seen[@]:-}"; do [ "$s" = "$t" ] && skip=1; done
  [ "$skip" -eq 0 ] && seen+=("$t")
done

declare -A STATUS=()
failed=0
for target in "${seen[@]}"; do
  script="$SCRIPT_DIR/${SCRIPT_OF[$target]}"
  log "=== Цель: $target ($(basename "$script")) ==="
  if bash "$script"; then
    STATUS[$target]="OK"
  else
    STATUS[$target]="FAIL"
    failed=$((failed + 1))
  fi
done

log "=== Сводка сборки ==="
for target in "${seen[@]}"; do
  printf '  %-8s %s\n' "$target" "${STATUS[$target]}"
done
[ "$failed" -eq 0 ] || { printf 'Целей с ошибками: %d\n' "$failed" >&2; exit 1; }
log "Все цели собраны. Артефакты: $PROJECT_ROOT/dist/"
