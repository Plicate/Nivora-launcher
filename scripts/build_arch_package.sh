#!/usr/bin/env bash
# Сборка Arch/CachyOS-пакета Nivora Launcher (pkg.tar.zst) из локального дерева.
# Скрипты сборки независимы: этот собирает только Arch-пакет.
#
# Скрипт создаёт в изолированном временном каталоге архив
# nivora-launcher-<pkgver>.tar.gz с корневой папкой nivora-launcher, вычисляет и
# проверяет его SHA-256, затем запускает makepkg с PKGBUILD из packaging/arch.
# Пакет сохраняется в dist/arch/; после успешной сборки печатается точный путь.
# Установка не выполняется. Системные зависимости не устанавливаются: нужны
# base-devel, nodejs, npm, rust, qt6-base, qt6-svg, python + python-defusedxml,
# python-psutil, libsecret, gnome-keyring, webkit2gtk-4.1, makepkg.
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
PROJECT_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

pkgname="nivora-launcher"
archive_root="$pkgname"
pkgver="$(sed -n 's/^pkgver=//p' packaging/arch/PKGBUILD | tr -d '[:space:]')"
[ -n "$pkgver" ] || { echo "Не удалось прочитать pkgver из packaging/arch/PKGBUILD" >&2; exit 1; }

command -v makepkg >/dev/null 2>&1 || { echo "makepkg не найден: установите pacman-package-group/base-devel" >&2; exit 1; }

log() { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
die() { printf '\033[1;31mОшибка: %s\033[0m\n' "$*" >&2; exit 1; }

BUILD_DIR="$(mktemp -d --tmpdir "${pkgname}-build.XXXXXX")"
trap 'rm -rf "$BUILD_DIR"' EXIT
STAGE_DIR="$BUILD_DIR/$archive_root"
OUTPUT_DIR="$PROJECT_ROOT/dist/arch"
ARCHIVE="$BUILD_DIR/${pkgname}-${pkgver}.tar.gz"

log "Готовлю архив исходников ${pkgname}-${pkgver}.tar.gz"
mkdir -p "$STAGE_DIR"
# Чистое дерево сборки: без node_modules, target и кэшей.
tar -cf - \
  --exclude='node_modules' \
  --exclude='frontend/dist' \
  --exclude='frontend/src-tauri/target' \
  --exclude='frontend/src-tauri/resources/backend' \
  --exclude='__pycache__' \
  --exclude='.build-windows' \
  --exclude='dist' \
  --exclude='.git' \
  --exclude='.venv' \
  --exclude='*.pyc' \
  . | tar -xf - -C "$STAGE_DIR"

tar -czf "$ARCHIVE" -C "$BUILD_DIR" "$archive_root"

log "Проверяю SHA-256 архива"
archive_sha256="$(sha256sum "$ARCHIVE" | awk '{print $1}')"
printf '%s  %s\n' "$archive_sha256" "${ARCHIVE##*/}" > "$BUILD_DIR/${pkgname}-${pkgver}.tar.gz.sha256"
(cd "$BUILD_DIR" && sha256sum -c "${pkgname}-${pkgver}.tar.gz.sha256") || die "Контрольная сумма архива не сошлась"

log "Собираю пакет makepkg'ом (чистое дерево без старого target/)"
mkdir -p "$BUILD_DIR/makepkg"
cp packaging/arch/PKGBUILD "$BUILD_DIR/makepkg/PKGBUILD"
cp "$ARCHIVE" "$BUILD_DIR/makepkg/"
cd "$BUILD_DIR/makepkg"

# makepkg отказывается работать от root: пересобираем под временным
# непривилегированным пользователем с собственным HOME (cargo/npm пишут кэши).
run_makepkg() {
  if [ "$(id -u)" -eq 0 ]; then
    local build_user="${SUDO_USER:-}"
    if [ -z "$build_user" ] || ! id "$build_user" >/dev/null 2>&1; then build_user="nobody"; fi
    mkdir -p "$BUILD_DIR/home"
    chmod -R a+rwX "$BUILD_DIR"
    runuser -u "$build_user" -- \
      env HOME="$BUILD_DIR/home" NIVORA_LAUNCHER_SOURCE_SHA256="$archive_sha256" makepkg -f
  else
    NIVORA_LAUNCHER_SOURCE_SHA256="$archive_sha256" makepkg -f
  fi
}
run_makepkg || die "makepkg завершился с ошибкой — смотрите вывод выше"

package_file="$(ls -1t ${pkgname}-*.pkg.tar.zst 2>/dev/null | head -n 1 || true)"
[ -n "$package_file" ] || die "makepkg не создал пакет в $BUILD_DIR/makepkg"

mkdir -p "$OUTPUT_DIR"
cp -f "$package_file" "$OUTPUT_DIR/"
log "Пакет готов: $OUTPUT_DIR/$package_file"
log "Установка: sudo pacman -U \"$OUTPUT_DIR/$package_file\""
log "ВНИМАНИЕ: ставьте пакет ЦЕЛИКОМ. Подмена файлов Python-бэкенда в"
log "/usr/lib/nivora-launcher/nivora_launcher/ без пересборки пакета рассинхронизирует"
log "бэкенд с интерфейсом внутри binary — приложение покажет баннер предупреждения."
