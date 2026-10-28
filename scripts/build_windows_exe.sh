#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
PROJECT_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

WORK_DIR="$PROJECT_ROOT/.build-windows"
OUTPUT_DIR="$PROJECT_ROOT/dist/windows"
HELPERS_RESOURCE="$PROJECT_ROOT/frontend/src-tauri/resources/backend"
# на Arch используем makensis из Debian-пакета: NSIS есть только в AUR
NSIS_DEB_VERSION="${NSIS_DEB_VERSION:-3.08-3+deb12u1}"
NSIS_DEB_BASE="${NSIS_DEB_BASE:-http://deb.debian.org/debian/pool/main/n/nsis}"
# файл RestartManager.nsh появился в NSIS 3.10, поэтому берём его из отдельного пакета
NSIS_RM_DEB_VERSION="${NSIS_RM_DEB_VERSION:-3.11-1}"
mkdir -p "$WORK_DIR" "$OUTPUT_DIR"

log() { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
die() { printf '\033[1;31mОшибка: %s\033[0m\n' "$*" >&2; exit 1; }

for argument in "$@"; do
  case "$argument" in
    *) die "Неизвестный аргумент: $argument (аргументов нет)" ;;
  esac
done

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) HOST_WINDOWS=1 ;;
  *) HOST_WINDOWS=0 ;;
esac

log "Собираю nivora-turbo.exe (C++, статически с zlib)"
TURBO_TARGET=windows bash "$SCRIPT_DIR/build_turbo.sh" "$HELPERS_RESOURCE/nivora-turbo.exe" \
  || die "nivora-turbo.exe не собрался (нужен mingw-w64 g++ с posix-потоками: Arch — mingw-w64-gcc, Debian — g++-mingw-w64-x86-64-posix, либо MSYS2)."
GO_BIN="$(command -v go || true)"
[ -n "$GO_BIN" ] || { [ -x "$HOME/.local/go/bin/go" ] && GO_BIN="$HOME/.local/go/bin/go"; } || true
[ -n "$GO_BIN" ] || die "Go не найден: nivora-modcat.exe обязателен (https://go.dev/dl/)."
(cd "$PROJECT_ROOT/go-modcat" && GOOS=windows GOARCH=amd64 CGO_ENABLED=0 "$GO_BIN" build \
    -trimpath -ldflags='-s -w' -o "$HELPERS_RESOURCE/nivora-modcat.exe" .) \
  || die "nivora-modcat.exe не собрался."
cp -f "$PROJECT_ROOT/java/ModScan.java" "$HELPERS_RESOURCE/ModScan.java" || die "ModScan.java не скопирован."

log "Собираю фронтенд (npm)"
cd frontend
npm install --no-audit --no-fund
# npm 11 может блокировать postinstall esbuild
if npm install-scripts ls >/dev/null 2>&1; then
  npm install-scripts approve esbuild || true
fi
npm rebuild esbuild --no-audit --no-fund
npm run build
cd "$PROJECT_ROOT"

# cargo-xwin подставляет clang-cl, lld-link и Windows SDK для MSVC-кросс-сборки
SUDO=()
if [ "$(id -u)" -ne 0 ]; then
  command -v sudo >/dev/null 2>&1 || die "Для установки системных пакетов нужен root или sudo."
  SUDO=(sudo)
fi

extract_deb_data() {
  # извлекаем data.tar.* из пакета Debian
  local deb="$1" dest="$2" data_tar
  mkdir -p "$dest"
  (
    cd "$dest"
    ar x "$deb"
    data_tar="$(find . -maxdepth 1 -name 'data.tar.*' | head -n 1)"
    [ -n "$data_tar" ] || exit 1
    tar -xf "$data_tar"
    rm -f "$data_tar" control.tar.* debian-binary _gpgorigin 2>/dev/null || true
  ) || die "Не удалось распаковать $(basename "$deb")."
}

ensure_nsis_restart_manager_nsh() {
  # для сборки нужен RestartManager.nsh из NSIS 3.10+; в Debian stable 3.08 его нет
  local target
  [ -f "/usr/share/nsis/Include/Win/RestartManager.nsh" ] && return 0

  log "Докидываю Include/Win/RestartManager.nsh (файл из NSIS 3.10+, нужен шаблону tauri)"
  command -v curl >/dev/null 2>&1 || die "curl не найден: нужен для скачивания NSIS (pacman -S curl)."
  command -v ar >/dev/null 2>&1 || die "ar не найден: нужен для распаковки .deb (pacman -S binutils)."

  local rm_dir="$WORK_DIR/nsis-restartmanager"
  if [ ! -f "$rm_dir/usr/share/nsis/Include/Win/RestartManager.nsh" ]; then
    curl -fsSL -o "$WORK_DIR/nsis-rm.deb" "$NSIS_DEB_BASE/nsis-common_${NSIS_RM_DEB_VERSION}_all.deb" \
      || die "Не удалось скачать nsis-common_$NSIS_RM_DEB_VERSION (задайте NSIS_DEB_BASE/NSIS_RM_DEB_VERSION или поставьте nsis >=3.10 из AUR)."
    rm -rf "$rm_dir"
    extract_deb_data "$WORK_DIR/nsis-rm.deb" "$rm_dir"
  fi
  [ -f "$rm_dir/usr/share/nsis/Include/Win/RestartManager.nsh" ] \
    || die "RestartManager.nsh не найден внутри пакета — поставьте nsis >=3.10 из AUR."

  # для симлинка /usr/share/nsis записываем в цель; системный каталог требует sudo
  if [ -L /usr/share/nsis ]; then
    target="$(cd /usr/share/nsis 2>/dev/null && pwd -P || echo /usr/share/nsis)"
  else
    target="/usr/share/nsis"
  fi
  if [ ! -d "$target/Include/Win" ]; then
    mkdir -p "$target/Include/Win" 2>/dev/null || "${SUDO[@]}" mkdir -p "$target/Include/Win"
  fi
  if ! cp "$rm_dir/usr/share/nsis/Include/Win/RestartManager.nsh" "$target/Include/Win/" 2>/dev/null; then
    "${SUDO[@]}" cp "$rm_dir/usr/share/nsis/Include/Win/RestartManager.nsh" "$target/Include/Win/" \
      || die "Не удалось записать $target/Include/Win/RestartManager.nsh — запустите от root или с sudo."
  fi
  [ -f "/usr/share/nsis/Include/Win/RestartManager.nsh" ] \
    || die "RestartManager.nsh по-прежнему недоступен в /usr/share/nsis — проверьте права."
}

ensure_nsis_linux() {
  # на Arch NSIS есть только в AUR; при необходимости используем Debian-пакет
  if ! command -v makensis >/dev/null 2>&1; then
    local nsis_root="$WORK_DIR/nsis"
    if [ ! -x "$nsis_root/pkg/usr/bin/makensis" ]; then
      log "makensis не найден (в Arch его нет в официальных репо) — беру NSIS $NSIS_DEB_VERSION из Debian-пакета"
      command -v curl >/dev/null 2>&1 || die "curl не найден: нужен для скачивания NSIS (pacman -S curl)."
      command -v ar >/dev/null 2>&1 || die "ar не найден: нужен для распаковки .deb (pacman -S binutils)."
      curl -fsSL -o "$WORK_DIR/nsis.deb" "$NSIS_DEB_BASE/nsis_${NSIS_DEB_VERSION}_amd64.deb" \
        || die "Не удалось скачать nsis_$NSIS_DEB_VERSION (архив мог переехать: задайте NSIS_DEB_BASE/NSIS_DEB_VERSION или поставьте nsis из AUR)."
      curl -fsSL -o "$WORK_DIR/nsis-common.deb" "$NSIS_DEB_BASE/nsis-common_${NSIS_DEB_VERSION}_all.deb" \
        || die "Не удалось скачать nsis-common_$NSIS_DEB_VERSION (задайте NSIS_DEB_BASE/NSIS_DEB_VERSION или поставьте nsis из AUR)."
      rm -rf "$nsis_root"
      extract_deb_data "$WORK_DIR/nsis.deb" "$nsis_root/pkg"
      extract_deb_data "$WORK_DIR/nsis-common.deb" "$nsis_root/common"
    fi

    # удаляем висячую ссылку от предыдущей сборки
    if [ -L /usr/share/nsis ] && [ ! -e /usr/share/nsis ]; then
      log "Убираю висячий симлинк /usr/share/nsis (цель из прошлой сборки удалена)"
      "${SUDO[@]}" rm -f /usr/share/nsis \
        || die "Не удалось убрать висячий симлинк /usr/share/nsis — запустите от root или с sudo."
    fi
    if [ ! -e /usr/share/nsis ]; then
      log "Создаю симлинк /usr/share/nsis (makensis ищет данные по этому пути)"
      "${SUDO[@]}" ln -s "$nsis_root/common/usr/share/nsis" /usr/share/nsis \
        || die "Не удалось создать симлинк /usr/share/nsis — запустите от root или с sudo."
    else
      log "/usr/share/nsis уже существует — использую его данные как есть"
    fi
    export PATH="$nsis_root/pkg/usr/bin:$PATH"
    command -v makensis >/dev/null 2>&1 || die "makensis недоступен после установки NSIS."
  fi
  ensure_nsis_restart_manager_nsh
}

ensure_linux_cross_toolchain() {
  local missing=() binary toolchain_cargo
  # clang-cl компилирует C-код, lld-link линкует MSVC-таргет, llvm-objcopy обрабатывает ресурсы
  for binary in clang-cl lld-link llvm-objcopy; do
    command -v "$binary" >/dev/null 2>&1 || missing+=("$binary")
  done
  if [ "${#missing[@]}" -gt 0 ]; then
    log "Ставлю системные пакеты для кросс-сборки (не хватало: ${missing[*]})"
    if command -v pacman >/dev/null 2>&1; then
      # nsis сюда НЕ включаем: в Arch его нет в официальных репо (см. ensure_nsis_linux)
      "${SUDO[@]}" pacman -S --needed --noconfirm clang lld llvm binutils xz \
        || die "Не удалось поставить clang/lld/llvm/binutils/xz через pacman — установите вручную и перезапустите."
    elif command -v apt-get >/dev/null 2>&1; then
      "${SUDO[@]}" apt-get update && "${SUDO[@]}" apt-get install -y clang lld llvm nsis binutils xz-utils \
        || die "Не удалось поставить clang/lld/llvm/nsis/binutils/xz-utils через apt — установите вручную и перезапустите."
    else
      die "Не найден ни pacman, ни apt. Установите вручную clang, lld, llvm, binutils и xz (не хватало: ${missing[*]}) и перезапустите."
    fi
  fi
  ensure_nsis_linux

  # для MSVC-таргета нужен стандартный Rust через rustup
  if ! command -v rustup >/dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/rustup" ]; then
    log "rustup не найден — устанавливаю (нужен только ради таргета x86_64-pc-windows-msvc)"
    if command -v pacman >/dev/null 2>&1; then
      "${SUDO[@]}" pacman -S --needed --noconfirm rustup \
        || die "Не удалось установить rustup через pacman: sudo pacman -S rustup"
    elif command -v curl >/dev/null 2>&1; then
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --profile minimal --default-toolchain stable \
        || die "Не удалось установить rustup через rustup.rs — установите вручную: https://rustup.rs"
    else
      die "rustup и curl не найдены — установите rustup вручную (https://rustup.rs) и перезапустите."
    fi
  fi
  export PATH="$HOME/.cargo/bin:$PATH"
  command -v rustup >/dev/null 2>&1 || die "rustup не найден после установки."

  # устанавливаем stable, если rustup ещё не настроен
  if ! rustup which cargo >/dev/null 2>&1; then
    log "Ставлю Rust-тулчейн stable (первый запуск долгий)"
    rustup toolchain install stable --profile minimal
    rustup default stable
  fi
  # используем cargo из rustup, а не системный
  toolchain_cargo="$(rustup which cargo)"
  export PATH="$(dirname "$toolchain_cargo"):$HOME/.cargo/bin:$PATH"

  if ! rustup target list --installed | grep -q '^x86_64-pc-windows-msvc$'; then
    log "Добавляю Rust-таргет x86_64-pc-windows-msvc"
    rustup target add x86_64-pc-windows-msvc
  fi

  if ! command -v cargo-xwin >/dev/null 2>&1; then
    log "Ставлю cargo-xwin (компиляция займёт несколько минут)"
    cargo install cargo-xwin --locked \
      || die "Не удалось установить cargo-xwin: cargo install cargo-xwin"
  fi
}

if [ "$HOST_WINDOWS" -eq 1 ]; then
  TAURI_TARGET_ARGS=()
  RELEASE_DIR="frontend/src-tauri/target/release"
else
  ensure_linux_cross_toolchain
  # на Linux собираем только NSIS: MSI/WiX требуют Windows
  TAURI_TARGET_ARGS=(--target x86_64-pc-windows-msvc --runner cargo-xwin --bundles nsis)
  RELEASE_DIR="frontend/src-tauri/target/x86_64-pc-windows-msvc/release"
fi

# конфиг Windows передаём явно: при Linux cross-build Tauri выбирает конфиг по ОС хоста
# updater-артефакты создаются только при наличии ключа подписи
TAURI_CONFIG_ARGS=(--config src-tauri/tauri.windows.conf.json)
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  TAURI_CONFIG_ARGS+=(--config '{"bundle":{"createUpdaterArtifacts":false}}')
fi

# установщик включает нативные helper-файлы из backend/
for helper in nivora-turbo.exe nivora-modcat.exe ModScan.java; do
  [ -f "$HELPERS_RESOURCE/$helper" ] \
    || die "Не найден frontend/src-tauri/resources/backend/$helper — шаг сборки хелперов не выполнен."
done

log "Собираю Tauri (Windows exe; первая кросс-сборка качает Windows SDK и компилирует все зависимости — это надолго)"
cd frontend
npm run tauri -- build "${TAURI_TARGET_ARGS[@]}" "${TAURI_CONFIG_ARGS[@]}"
cd "$PROJECT_ROOT"

setup_exe="$(ls -1t "$RELEASE_DIR/bundle/nsis/"*-setup.exe 2>/dev/null | head -n 1 || true)"
nsis_script="$RELEASE_DIR/nsis/x64/installer.nsi"
if [ -f "$nsis_script" ] && ! grep -q "NivoraPurgeData" "$nsis_script"; then
  die "Установщик собран из стандартного шаблона Tauri, а не из frontend/src-tauri/windows/installer.nsi — проверьте bundle.windows.nsis.template."
fi
raw_exe="$RELEASE_DIR/nivora-launcher.exe"

[ -n "$setup_exe" ] || die "NSIS-установщик не найден в $RELEASE_DIR/bundle/nsis/ — смотрите вывод tauri build."
[ -f "$raw_exe" ] || die "Автономный exe не найден: $raw_exe"
# проверяем GUI-подсистему PE: поле WORD находится по смещению PE+0x5C
if command -v objdump >/dev/null 2>&1; then
  # Не используем grep -q в конвейере под pipefail: ранний выход grep может
  # завершить objdump с SIGPIPE и дать ложный отказ для корректного GUI-exe.
  pe_headers="$(objdump -p "$raw_exe" 2>/dev/null)" \
    || die "Не удалось прочитать PE-заголовок nivora-launcher.exe."
  [[ "$pe_headers" =~ Subsystem[[:space:]]*00000002[[:space:]]*\(Windows\ GUI\) ]] \
    || die "nivora-launcher.exe собран с консольной подсистемой — в production консоли быть не должно."
fi

cp -f "$setup_exe" "$OUTPUT_DIR/"
cp -f "$raw_exe" "$OUTPUT_DIR/"
log "Установщик: $OUTPUT_DIR/$(basename "$setup_exe")"
log "Автономный exe: $OUTPUT_DIR/nivora-launcher.exe"

# переменная WINDOWS_CERT_FILE включает необязательную Authenticode-подпись
if [ -n "${WINDOWS_CERT_FILE:-}" ]; then
  log "Подписываю exe (osslsigncode)"
  command -v osslsigncode >/dev/null 2>&1 \
    || die "osslsigncode не найден, подпись невозможна: Arch — pacman -S osslsigncode, Debian — apt install osslsigncode."
  WINDOWS_CERT_PASSWORD="${WINDOWS_CERT_PASSWORD:-}"
  WINDOWS_TIMESTAMP_URL="${WINDOWS_TIMESTAMP_URL:-http://timestamp.digicert.com}"
  for target in "$OUTPUT_DIR"/*.exe; do
    [ -f "$target" ] || continue
    tmp="$target.signed"
    osslsigncode sign -pkcs12 "$WINDOWS_CERT_FILE" -pass "$WINDOWS_CERT_PASSWORD" \
      -h sha256 -t "$WINDOWS_TIMESTAMP_URL" -n "Nivora Launcher" \
      -in "$target" -out "$tmp" \
      || die "Не удалось подписать $(basename "$target")."
    mv -f "$tmp" "$target"
  done
  log "Подпись: OK ($(ls -1 "$OUTPUT_DIR"/*.exe | wc -l) exe)"
else
  log "Внимание: exe НЕ подписан (нет WINDOWS_CERT_FILE). Windows SmartScreen покажет предупреждение — это нормально для файлов без кодовой подписи, см. README «Windows предупреждает при запуске»."
fi

# latest.json нужен встроенному updater; подписываем финальный установщик после Authenticode
APP_VERSION_TAURI="$(grep -m1 '"version"' frontend/src-tauri/tauri.conf.json | cut -d'"' -f4)"
[ -n "$APP_VERSION_TAURI" ] || die "Не удалось прочитать версию из tauri.conf.json"
RELEASE_TAG="${RELEASE_TAG:-$APP_VERSION_TAURI}"
GITHUB_RELEASE_REPO="${GITHUB_RELEASE_REPO:-Plicate/Nivora-launcher}"
setup_name="$(basename "$setup_exe")"
# пробелы в именах release assets заменяются точками при загрузке в GitHub
update_asset="${setup_name// /.}"
if [ "$update_asset" != "$setup_name" ]; then
  mv -f "$OUTPUT_DIR/$setup_name" "$OUTPUT_DIR/$update_asset"
fi
if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  log "Подписываю установщик для апдейтера и собираю latest.json (тег $RELEASE_TAG)"
  rm -f "$OUTPUT_DIR/$update_asset.sig"
  ( cd frontend && npm run tauri -- signer sign "$OUTPUT_DIR/$update_asset" >/dev/null ) \
    || die "Не удалось подписать установщик ключом апдейтера (проверьте TAURI_SIGNING_PRIVATE_KEY и TAURI_SIGNING_PRIVATE_KEY_PASSWORD)."
  [ -f "$OUTPUT_DIR/$update_asset.sig" ] || die "tauri signer не создал $update_asset.sig"
  signature="$(tr -d '\r\n' < "$OUTPUT_DIR/$update_asset.sig")"
  pub_date="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  rm -f "$OUTPUT_DIR/latest.json"
  node "$SCRIPT_DIR/write_updater_manifest.mjs" \
    "$OUTPUT_DIR/latest.json" "$APP_VERSION_TAURI" "$GITHUB_RELEASE_REPO" "$RELEASE_TAG" \
    "$update_asset" "$signature" "$pub_date" \
    || die "Не удалось создать валидный latest.json для Tauri updater."
  log "latest.json: $OUTPUT_DIR/latest.json — загрузите его в релиз $RELEASE_TAG вместе с $update_asset"
  log "Например: gh release create $RELEASE_TAG \"$OUTPUT_DIR/$update_asset\" \"$OUTPUT_DIR/latest.json\" --title \"Nivora Launcher $APP_VERSION_TAURI\""
else
  log "Внимание: нет TAURI_SIGNING_PRIVATE_KEY — latest.json НЕ создан. Встроенное самообновление этот релиз не увидит; лаунчер 1.1+ всё равно найдёт его через GitHub API и предложит скачать вручную."
fi

( cd "$OUTPUT_DIR" && sha256sum ./*.exe > sha256sums.txt )
log "Суммы SHA256: $OUTPUT_DIR/sha256sums.txt"

log "Готово. Артефакты: $OUTPUT_DIR"
log "Главный файл — nivora-launcher.exe (ОДИН файл: весь движок — нативный Rust, без Python). Установщик кладёт нативные хелперы рядом с приложением."
( cd "$OUTPUT_DIR" && for artifact in *; do
    [ -f "$artifact" ] || continue
    printf '    %8s  %s\n' "$(du -h "$artifact" | cut -f1)" "$artifact"
  done )
