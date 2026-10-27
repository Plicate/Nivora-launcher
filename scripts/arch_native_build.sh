#!/usr/bin/env bash
# сборка x86_64 для Arch использует GCC/BFD без C/C++ и Rust linker-plugin LTO
# системные makepkg.conf и настройки Cargo не меняются
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
PROJECT_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
TARGET="x86_64_unknown_linux_gnu"

[ "$(uname -s)" = "Linux" ] && { [ "$(uname -m)" = "x86_64" ] || [ "$(uname -m)" = "amd64" ]; } \
  || { echo "Профиль совместимости поддерживает только Arch Linux x86_64." >&2; exit 1; }
for tool in gcc g++ ar ranlib ld.bfd cargo npm; do
  command -v "$tool" >/dev/null 2>&1 || { echo "Нет инструмента сборки: $tool" >&2; exit 1; }
done

# убираем унаследованные LTO-флаги и используем BFD для линковки
native_flags() {
  local linking="$2" flag
  local filtered=()
  for flag in $1; do
    case "$flag" in
      -flto*|-fuse-ld=*|-fno-lto|-fuse-linker-plugin|-fno-fat-lto-objects|-ffat-lto-objects|-fwhole-program)
        ;;
      *) filtered+=("$flag") ;;
    esac
  done
  filtered+=(-fno-lto)
  [ "$linking" = "yes" ] && filtered+=(-fuse-ld=bfd)
  printf '%s ' "${filtered[@]}"
}

# cc-rs проверяет таргет-специфичные и HOST_/TARGET_-переменные раньше глобальных
# нормализуем все написания, включая тройки с дефисами
for name in $(compgen -v 2>/dev/null | grep -E '^[A-Za-z_][A-Za-z0-9_]*$' || true); do
  case "$name" in
    ARFLAGS|HOST_ARFLAGS|TARGET_ARFLAGS|ARFLAGS_*)
      unset "$name" || true
      continue
      ;;
  esac
  case "$name" in
    CC|HOST_CC|TARGET_CC|CC_*) export "$name=gcc" ;;
    CXX|HOST_CXX|TARGET_CXX|CXX_*) export "$name=g++" ;;
    RANLIB|HOST_RANLIB|TARGET_RANLIB|RANLIB_*) export "$name=ranlib" ;;
    AR|HOST_AR|TARGET_AR|AR_*) export "$name=ar" ;;
  esac
done
export CC=gcc CXX=g++ AR=ar RANLIB=ranlib
export "CC_$TARGET=gcc" "CXX_$TARGET=g++" "AR_$TARGET=ar" "RANLIB_$TARGET=ranlib"

export CFLAGS="$(native_flags "${CFLAGS:-}" no)"
export CXXFLAGS="$(native_flags "${CXXFLAGS:-}" no)"
export CPPFLAGS="$(native_flags "${CPPFLAGS:-}" no)"
export LDFLAGS="$(native_flags "${LDFLAGS:-}" yes)"

# сохраняем прочие RUSTFLAGS, заменяя только linker/LTO-настройки
rustflags_filtered=()
if [ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]; then
  IFS=$'\x1f' read -r -d '' -a rustflags_raw < <(printf '%s\x00' "$CARGO_ENCODED_RUSTFLAGS") || true
else
  read -r -a rustflags_raw <<< "${RUSTFLAGS:-}"
fi
skip_next=0
for flag in "${rustflags_raw[@]:-}"; do
  [ "$skip_next" = 1 ] && { skip_next=0; continue; }
  case "$flag" in
    -C|--codegen) skip_next=1 ;;
    -Clinker*|-Clinker-flavor*|-Clinker-features*|-Clto*|-Clinker-plugin-lto*|--codegen=linker*|--codegen=lto*) ;;
    -Clink-arg=-fuse-ld=*|-Clink-arg=-flto*) ;;
    *) rustflags_filtered+=("$flag") ;;
  esac
done
rustflags_filtered+=(
  -Clinker=gcc
  -Clinker-flavor=gcc
  "-Clink-arg=-fuse-ld=bfd"
  -Clto=off
  -Clinker-plugin-lto=no
)
joined="$(printf '%s\x1f' "${rustflags_filtered[@]}")"
export RUSTFLAGS="${joined%$'\x1f'}"
export CARGO_ENCODED_RUSTFLAGS="$RUSTFLAGS"
for name in $(compgen -v 2>/dev/null | grep -E '^CARGO_TARGET_.*_RUSTFLAGS$' || true); do
  unset "$name"
done
export "CARGO_TARGET_${TARGET^^}_LINKER"=gcc
export CARGO_PROFILE_RELEASE_LTO=false

echo "Arch desktop build: GCC/BFD, нативный LTO выключен, Rust linker-plugin LTO выключен."
cd "$PROJECT_ROOT/frontend"
export CARGO_TARGET_DIR="$PWD/src-tauri/target"
# makepkg сам пакует бинарник; подписанные updater-артефакты здесь не нужны
exec npm run desktop:build -- --no-bundle --config src-tauri/tauri.arch.conf.json
