#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
PROJECT_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

OUTPUT_DIR="$PROJECT_ROOT/dist/rpm"
RELEASE_BUNDLE="frontend/src-tauri/target/release/bundle"

log() { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
die() { printf '\033[1;31mОШИБКА:%s %s\033[0m\n' '' "$*" >&2; exit 1; }

log "Собираю nivora-turbo (пакетное SHA-256)"
bash "$SCRIPT_DIR/build_turbo.sh" || die "nivora-turbo не собрался"

log "Собираю nivora-modcat (каталог Modrinth)"
bash "$SCRIPT_DIR/build_modcat.sh" || die "nivora-modcat не собрался"

log "Собираю фронтенд (npm)"
cd frontend
npm install --no-audit --no-fund
# npm 11 может блокировать postinstall esbuild, поэтому разрешаем его и пересобираем
if npm install-scripts ls >/dev/null 2>&1; then
  npm install-scripts approve esbuild || true
fi
npm rebuild esbuild --no-audit --no-fund

# rpm-бандлеру нужны PNG-иконки всех размеров; генерируем из логотипа
if [ ! -f src-tauri/icons/32x32.png ]; then
  log "Генерирую иконки (tauri icon)"
  npm run tauri -- icon src/assets/uni.png --output src-tauri/icons
fi

log "Собираю Tauri (rpm; первая сборка компилирует все зависимости — это надолго)"
# без ключа отключаем updater-артефакты: rpm-сборка их не использует
if [ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  TAURI_CONFIG_ARGS=()
else
  TAURI_CONFIG_ARGS=(--config '{"bundle":{"createUpdaterArtifacts":false}}')
fi
npm run tauri -- build --bundles rpm "${TAURI_CONFIG_ARGS[@]}"
cd "$PROJECT_ROOT"

rpm_file="$(ls -1t "$RELEASE_BUNDLE/rpm/"*.rpm 2>/dev/null | head -n 1 || true)"
[ -n "$rpm_file" ] || die "rpm не найден в $RELEASE_BUNDLE/rpm/ — смотрите вывод tauri build."

mkdir -p "$OUTPUT_DIR"
cp -f "$rpm_file" "$OUTPUT_DIR/"
log "Пакет готов: $OUTPUT_DIR/$(basename "$rpm_file")"
log "Установка: sudo dnf install \"$OUTPUT_DIR/$(basename "$rpm_file")\""

# переменная RPM_SIGN_KEY задаёт GPG-ключ для необязательной подписи пакета
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
