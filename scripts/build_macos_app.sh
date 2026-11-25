#!/usr/bin/env bash
# собирает .app и DMG для macOS arm64/x86_64; совместим с Bash 3.2
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
PROJECT_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

log() { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
die() { printf '\033[1;31mОШИБКА: %s\033[0m\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "Этот скрипт запускается только на macOS (uname: $(uname -s))."

HOST_ARCH="$(uname -m)"
ARCH="${MACOS_ARCH:-$HOST_ARCH}"
case "$ARCH" in
  arm64|aarch64) ARCH=arm64; RUST_TARGET=aarch64-apple-darwin; GOARCH_VALUE=arm64 ;;
  x86_64|amd64|intel) ARCH=x86_64; RUST_TARGET=x86_64-apple-darwin; GOARCH_VALUE=amd64 ;;
  *) die "MACOS_ARCH=$ARCH не поддерживается (arm64 или x86_64)." ;;
esac

missing=""
need() { command -v "$1" >/dev/null 2>&1 || missing="$missing\n  - $1: $2"; }
xcode-select -p >/dev/null 2>&1 || missing="$missing\n  - Xcode Command Line Tools: xcode-select --install"
need node "Node.js 20+ (brew install node)"
need npm "Node.js 20+ (brew install node)"
need c++ "Xcode Command Line Tools: xcode-select --install"
need hdiutil "входит в macOS"
need codesign "Xcode Command Line Tools"
need shasum "входит в macOS"
need ditto "входит в macOS"
need lipo "Xcode Command Line Tools"
if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
  export PATH="$HOME/.cargo/bin:$PATH"
fi
need cargo "Rust через rustup: https://rustup.rs"
need rustup "Rust через rustup: https://rustup.rs"
GO_BIN="${GO:-}"
if [ -z "$GO_BIN" ]; then
  GO_BIN="$(command -v go || true)"
  [ -n "$GO_BIN" ] || { [ -x /usr/local/go/bin/go ] && GO_BIN=/usr/local/go/bin/go; } || true
  [ -n "$GO_BIN" ] || { [ -x /opt/homebrew/bin/go ] && GO_BIN=/opt/homebrew/bin/go; } || true
fi
[ -n "$GO_BIN" ] || missing="$missing\n  - go: Go 1.22+ (brew install go)"
if [ -n "$missing" ]; then
  printf '\033[1;31mОШИБКА: не хватает зависимостей для сборки macOS:%b\033[0m\n' "$missing" >&2
  exit 1
fi

rustup target list --installed | grep -qx "$RUST_TARGET" || {
  log "Добавляю Rust-таргет $RUST_TARGET"
  rustup target add "$RUST_TARGET" || die "rustup target add $RUST_TARGET не удался."
}

VERSION="$(grep -m1 '"version"' frontend/src-tauri/tauri.conf.json | cut -d'"' -f4)"
[ -n "$VERSION" ] || die "Не удалось прочитать версию из tauri.conf.json"
PRODUCT="Nivora Launcher"
ARTIFACT_BASE="Nivora-Launcher_${VERSION}_${ARCH}"
WORK_DIR="$PROJECT_ROOT/.build-macos/$ARCH"
OUTPUT_DIR="$PROJECT_ROOT/dist/macos"
HELPERS_RESOURCE="$PROJECT_ROOT/frontend/src-tauri/resources/backend"
BUNDLE_DIR="$PROJECT_ROOT/frontend/src-tauri/target/$RUST_TARGET/release/bundle"
log "Сборка $PRODUCT $VERSION для macOS $ARCH (хост $HOST_ARCH)"

log "Собираю nivora-turbo (C++, $ARCH)"
CXX="${CXX:-c++}" CXXFLAGS_EXTRA="-arch $ARCH -mmacosx-version-min=10.15" \
  bash "$SCRIPT_DIR/build_turbo.sh" "$HELPERS_RESOURCE/nivora-turbo" \
  || die "nivora-turbo не собрался — сборка остановлена."
log "Собираю nivora-modcat (Go, darwin/$GOARCH_VALUE)"
GOOS=darwin GOARCH="$GOARCH_VALUE" GO="$GO_BIN" bash "$SCRIPT_DIR/build_modcat.sh" "$HELPERS_RESOURCE/nivora-modcat" \
  || die "nivora-modcat не собрался — сборка остановлена."
cp -f "$PROJECT_ROOT/java/ModScan.java" "$HELPERS_RESOURCE/ModScan.java" \
  || die "ModScan.java не скопирован в ресурсы."
for helper in nivora-turbo nivora-modcat; do
  lipo -archs "$HELPERS_RESOURCE/$helper" | grep -qw "$ARCH" \
    || die "$helper собран не под $ARCH: $(lipo -archs "$HELPERS_RESOURCE/$helper")"
done

log "Собираю фронтенд и Tauri (app + dmg, $RUST_TARGET)"
cd frontend
npm ci --no-audit --no-fund || die "npm ci не удался."
if [ ! -f src-tauri/icons/icon.icns ]; then
  log "Генерирую иконки (tauri icon)"
  npm run tauri -- icon src/assets/uni.png --output src-tauri/icons
fi
rm -rf "$BUNDLE_DIR/macos" "$BUNDLE_DIR/dmg"
# updater-артефакты отключены: macOS-обновления требуют Developer ID
npm run tauri -- build --target "$RUST_TARGET" --config src-tauri/tauri.macos.conf.json --bundles app,dmg \
  || die "tauri build завершился с ошибкой."
cd "$PROJECT_ROOT"

app_path="$BUNDLE_DIR/macos/$PRODUCT.app"
[ -d "$app_path" ] || die ".app не найден: $app_path"
dmg_file="$(ls -1 "$BUNDLE_DIR/dmg/"*.dmg 2>/dev/null | head -n 1 || true)"
[ -n "$dmg_file" ] && [ -s "$dmg_file" ] || die "DMG не создан в $BUNDLE_DIR/dmg — сборка считается проваленной."

plist_version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app_path/Contents/Info.plist")"
[ "$plist_version" = "$VERSION" ] || die "Версия в Info.plist ($plist_version) не совпадает с tauri.conf.json ($VERSION)."
main_binary="$app_path/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$app_path/Contents/Info.plist")"
lipo -archs "$main_binary" | grep -qw "$ARCH" || die "Главный бинарник не под $ARCH: $(lipo -archs "$main_binary")"
for helper in nivora-launcher-backend nivora-turbo nivora-modcat ModScan.java; do
  [ -e "$app_path/Contents/Resources/backend/$helper" ] || die "В .app нет Resources/backend/$helper."
done
codesign --verify --deep --strict --verbose=2 "$app_path" || die "codesign --verify не прошёл."

mount_point="$(mktemp -d /tmp/nivora-dmg.XXXXXX)"
hdiutil attach -nobrowse -readonly -mountpoint "$mount_point" "$dmg_file" >/dev/null \
  || die "DMG не монтируется: $dmg_file"
[ -d "$mount_point/$PRODUCT.app" ] || { hdiutil detach "$mount_point" >/dev/null || true; die "В DMG нет $PRODUCT.app"; }
hdiutil detach "$mount_point" >/dev/null || true
rmdir "$mount_point" 2>/dev/null || true

mkdir -p "$OUTPUT_DIR"
cp -f "$dmg_file" "$OUTPUT_DIR/$ARTIFACT_BASE.dmg"
rm -f "$OUTPUT_DIR/$ARTIFACT_BASE.app.zip"
ditto -c -k --sequesterRsrc --keepParent "$app_path" "$OUTPUT_DIR/$ARTIFACT_BASE.app.zip"
(
  cd "$OUTPUT_DIR"
  shasum -a 256 Nivora-Launcher_*.dmg Nivora-Launcher_*.app.zip > SHA256SUMS
)
log "DMG:  $OUTPUT_DIR/$ARTIFACT_BASE.dmg"
log "ZIP:  $OUTPUT_DIR/$ARTIFACT_BASE.app.zip"
log "SHA256SUMS: $OUTPUT_DIR/SHA256SUMS"
log "Сборка не нотаризована (ad-hoc подпись). Первый запуск: ПКМ → Открыть, либо xattr -dr com.apple.quarantine \"/Applications/$PRODUCT.app\"."
