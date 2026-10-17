#!/usr/bin/env bash
# Сборка .rpm Nivora Launcher из локального дерева (Fedora/RHEL/openSUSE).
# Скрипты сборки независимы: этот собирает только rpm.
#
# Шаги:
#   1) PyInstaller-сборка Linux-бэкенда в nivora-launcher-backend (onefile);
#   2) npm install + генерация иконок (если нет) + tauri build --bundles rpm;
#   3) перенос пакета в dist/rpm/;
#   4) кодовая подпись (опционально): RPM_SIGN_KEY=<gpg-id> — rpmsign.
#
# Системные зависимости (Fedora): gcc-c++, pkgconf-pkg-config,
# webkit2gtk4.1-devel, gtk3-devel, libayatana-appindicator3-devel,
# librsvg2-devel, openssl-devel, python3-devel, nodejs, npm, cargo (rustup).
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
PROJECT_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

WORK_DIR="$PROJECT_ROOT/.build-linux"
OUTPUT_DIR="$PROJECT_ROOT/dist/rpm"
BACKEND_NAME="nivora-launcher-backend"
BACKEND_RESOURCE="frontend/src-tauri/resources/backend/$BACKEND_NAME"
RELEASE_BUNDLE="frontend/src-tauri/target/release/bundle"

log() { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
die() { printf '\033[1;31mОШИБКА:%s %s\033[0m\n' '' "$*" >&2; exit 1; }

command -v python3 >/dev/null 2>&1 || die "python3 не найден. Fedora: sudo dnf install python3"
command -v npm >/dev/null 2>&1 || die "npm не найден. Fedora: sudo dnf install nodejs npm"
if ! command -v cargo >/dev/null 2>&1; then
  [ -x "$HOME/.cargo/bin/cargo" ] && export PATH="$HOME/.cargo/bin:$PATH" \
    || die "cargo не найден. Установите rustup: https://rustup.rs"
fi
command -v pkg-config >/dev/null 2>&1 || die "pkg-config не найден: sudo dnf install pkgconf-pkg-config"
pkg-config --exists webkit2gtk-4.1 \
  || die "webkit2gtk-4.1 не найден: sudo dnf install webkit2gtk4.1-devel"
pkg-config --exists gtk+-3.0 || die "gtk+-3.0 не найден: sudo dnf install gtk3-devel"

# --- 1. Бэкенд (PyInstaller, onefile, нативный Linux) -------------------------
log "Собираю $BACKEND_NAME (PyInstaller, onefile)"
mkdir -p "$WORK_DIR" "$(dirname "$BACKEND_RESOURCE")"
if [ ! -x "$WORK_DIR/venv/bin/python" ]; then
  python3 -m venv "$WORK_DIR/venv"
fi
"$WORK_DIR/venv/bin/python" -m pip install --upgrade pip --quiet
# keyring не в основных зависимостях (на Arch его ставит pacman), но в rpm
# бэкенд-бинарник один на всё — libsecret-бэкенд keyring обязан быть внутри.
"$WORK_DIR/venv/bin/python" -m pip install --quiet . keyring pyinstaller

"$WORK_DIR/venv/bin/python" -m PyInstaller \
  --noconfirm --clean --onefile \
  --name "$BACKEND_NAME" \
  --distpath "$WORK_DIR/dist" \
  --workpath "$WORK_DIR/work" \
  --specpath "$WORK_DIR" \
  --collect-submodules nivora_launcher \
  --collect-data nivora_launcher \
  scripts/backend_entry.py

BACKEND_BIN="$WORK_DIR/dist/$BACKEND_NAME"
[ -f "$BACKEND_BIN" ] || die "PyInstaller не создал $BACKEND_BIN — смотрите вывод выше."
chmod +x "$BACKEND_BIN"
cp -f "$BACKEND_BIN" "$BACKEND_RESOURCE"
log "Бэкенд готов: $BACKEND_RESOURCE"

# --- 2. Фронтенд и Tauri rpm ---------------------------------------------------
log "Собираю фронтенд (npm)"
cd frontend
npm install --no-audit --no-fund
# npm 11 может блокировать postinstall esbuild — разрешаем и пересобираем.
if npm install-scripts ls >/dev/null 2>&1; then
  npm install-scripts approve esbuild || true
fi
npm rebuild esbuild --no-audit --no-fund

# rpm-бандлеру нужны PNG-иконки всех размеров; генерируем из логотипа.
if [ ! -f src-tauri/icons/32x32.png ]; then
  log "Генерирую иконки (tauri icon)"
  npm run tauri -- icon src/assets/uni.png --output src-tauri/icons
fi

log "Собираю Tauri (rpm; первая сборка компилирует все зависимости — это надолго)"
# Подписанные updater-артефакты создаём только при наличии ключа
# (как в build_windows_exe.sh): без TAURI_SIGNING_PRIVATE_KEY tauri build
# падает на «A public key has been found, but no private key», потому что в
# tauri.conf.json есть pubkey апдейтера. Локальной rpm-сборке артефакты
# не нужны — самообновление на Linux работает только через AppImage.
if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  TAURI_CONFIG_ARGS=()
else
  TAURI_CONFIG_ARGS=(--config '{"bundle":{"createUpdaterArtifacts":false}}')
fi
npm run tauri -- build --bundles rpm "${TAURI_CONFIG_ARGS[@]}"
cd "$PROJECT_ROOT"

# --- 3. Артефакт ---------------------------------------------------------------
rpm_file="$(ls -1t "$RELEASE_BUNDLE/rpm/"*.rpm 2>/dev/null | head -n 1 || true)"
[ -n "$rpm_file" ] || die "rpm не найден в $RELEASE_BUNDLE/rpm/ — смотрите вывод tauri build."

mkdir -p "$OUTPUT_DIR"
cp -f "$rpm_file" "$OUTPUT_DIR/"
log "Пакет готов: $OUTPUT_DIR/$(basename "$rpm_file")"
log "Установка: sudo dnf install \"$OUTPUT_DIR/$(basename "$rpm_file")\""

# --- 4. Кодовая подпись (опционально) ------------------------------------------
# Подпись пакета снимает вопросы dnf «неизвестного происхождения».
# Включается заданием RPM_SIGN_KEY=<gpg-id>; секретный ключ должен быть в GPG.
if [ -n "${RPM_SIGN_KEY:-}" ]; then
  log "Подписываю rpm (rpmsign, ключ $RPM_SIGN_KEY)"
  command -v rpmsign >/dev/null 2>&1 \
    || die "rpmsign не найден: sudo dnf install rpm-sign"
  rpmsign --define "_gpg_name $RPM_SIGN_KEY" \
    --addsign "$OUTPUT_DIR/$(basename "$rpm_file")" \
    || die "Не удалось подписать rpm (проверьте gpg --list-secret-keys)."
  log "Подпись: OK"
else
  log "Внимание: rpm НЕ подписан (нет RPM_SIGN_KEY). dnf предупредит о неподписанном источнике."
fi
