#!/usr/bin/env bash
# Автоматическая сборка Windows-версии Nivora Launcher (.exe).
#
# Это ОТДЕЛЬНЫЙ скрипт: Arch/CachyOS-пакет собирает scripts/build_arch_package.sh,
# здесь только Windows-артефакты. Один прогон делает всё:
#   1) PyInstaller-сборка Python-бэкенда в nivora-launcher-backend.exe
#      (венв + зависимости pyproject [+windows] + pyinstaller);
#   2) установка exe в frontend/src-tauri/resources/backend/ — tauri build
#      упаковывает его В САМ nivora-launcher.exe (include_bytes! в lib.rs),
#      поэтому портативная версия — один файл; NSIS-установщик тоже
#      кладёт его ресурсом рядом с установленным приложением;
#   3) сборка фронтенда (npm);
#   4) tauri build → NSIS-установщик (*-setup.exe) и автономный nivora-launcher.exe;
#   5) артефакты копируются в dist/windows/, печатается точный путь.
#
# Почему wine на Linux: PyInstaller не кросс-компилирует, exe бэкенда собирает
# только Windows-версия Python. Если её нет — скрипт САМ скачивает официальный
# установщик python.org и молча ставит его в изолированный префикс
# .build-windows/wineprefix (~/.wine не затрагивается; повторные запуски
# переиспользуют уже установленный Python). Tauri-оболочка кросс-компилируется
# нативно и wine не использует.
#
# Где запускать:
#   * Windows (Git Bash / MSYS2): нужен Python 3.10+, Node.js + npm, Rust (msvc).
#   * Linux: wine для бэкенда. Для кросс-сборки Tauri нужен rustup с target
#     x86_64-pc-windows-msvc, cargo-xwin, clang/lld/llvm и makensis — всё это
#     скрипт ставит сам: системные пакеты через pacman/apt (нужен root/sudo),
#     rustup, cargo-xwin и Windows SDK — в $HOME, системный Rust не затрагивается.
#     Отдельный случай: в Arch нет пакета nsis (только AUR), поэтому makensis
#     скрипт берёт из Debian-пакета в .build-windows/ и подкладывает данные
#     симлинком /usr/share/nsis.
#
# Windows Python тоже ставится автоматически (внутрь .build-windows/), если
# готовый не найден. Единственное, что скрипт заведомо не ставит, — сам wine.
#
# Использование:
#   ./scripts/build_windows_exe.sh               # полная сборка
#   ./scripts/build_windows_exe.sh --only-backend # только nivora-launcher-backend.exe
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
PROJECT_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

BACKEND_NAME="nivora-launcher-backend.exe"
WORK_DIR="$PROJECT_ROOT/.build-windows"
# Ровно этот путь ждёт tauri.windows.conf.json (bundle.resources).
BACKEND_RESOURCE="$PROJECT_ROOT/frontend/src-tauri/resources/backend/nivora-launcher-backend.exe"
OUTPUT_DIR="$PROJECT_ROOT/dist/windows"
ONLY_BACKEND=0
# Windows Python для авто-установки под wine (pyproject требует >=3.10).
PYTHON_WINDOWS_VERSION="${PYTHON_WINDOWS_VERSION:-3.12.10}"
# NSIS для Arch: пакета nsis в официальных репо нет, берём готовый makensis из
# Debian-пакета (stable/bookworm; нужны только glibc/libstdc++/zlib).
NSIS_DEB_VERSION="${NSIS_DEB_VERSION:-3.08-3+deb12u1}"
NSIS_DEB_BASE="${NSIS_DEB_BASE:-http://deb.debian.org/debian/pool/main/n/nsis}"
# RestartManager.nsh появился только в NSIS 3.10 — тянем его из отдельного пакета.
NSIS_RM_DEB_VERSION="${NSIS_RM_DEB_VERSION:-3.11-1}"
mkdir -p "$WORK_DIR"

log() { printf '\033[1;34m==> %s\033[0m\n' "$*"; }
die() { printf '\033[1;31mОшибка: %s\033[0m\n' "$*" >&2; exit 1; }

for argument in "$@"; do
  case "$argument" in
    --only-backend) ONLY_BACKEND=1 ;;
    *) die "Неизвестный аргумент: $argument (доступен --only-backend)" ;;
  esac
done

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) HOST_WINDOWS=1 ;;
  *) HOST_WINDOWS=0 ;;
esac

# --- Выбор Python для сборки бэкенда ----------------------------------------
# Windows: системный python (или py -3). Linux: Windows Python под wine.
if [ "$HOST_WINDOWS" -eq 1 ]; then
  if command -v python >/dev/null 2>&1; then
    PYTHON_CMD=(python)
  elif command -v py >/dev/null 2>&1; then
    PYTHON_CMD=(py -3)
  else
    die "Python не найден. Установите Python 3.10+ (python.org) и перезапустите."
  fi
  PYINSTALLER_RUN=("${PYTHON_CMD[@]}")
else
  # PyInstaller не умеет кросс-компиляцию: exe бэкенда собирает только
  # Windows-версия Python, и на Linux она запускается через wine.
  # Tauri-оболочка кросс-компилируется нативно и wine не использует.
  command -v wine >/dev/null 2>&1 || die "wine не найден. Без него на Linux не собрать exe бэкенда: PyInstaller не кросс-компилирует, а нужен именно Windows-Python (Tauri-часть wine не использует). Установите: sudo pacman -S wine (или apt install wine), либо запустите скрипт на Windows."

  PROJECT_WINEPREFIX="$WORK_DIR/wineprefix"
  WINE_PYTHON="${WINE_PYTHON:-}"
  if [ -z "$WINE_PYTHON" ]; then
    # 1) изолированный префикс проекта — сюда ставит авто-установка ниже;
    if [ -f "$PROJECT_WINEPREFIX/drive_c/nivora-launcher-python/python.exe" ]; then
      WINE_PYTHON="$PROJECT_WINEPREFIX/drive_c/nivora-launcher-python/python.exe"
    fi
    # 2) уже установленный пользователем Python в ~/.wine.
    if [ -z "$WINE_PYTHON" ]; then
      for candidate in "$HOME"/.wine/drive_c/Python*/python.exe; do
        if [ -f "$candidate" ]; then WINE_PYTHON="$candidate"; break; fi
      done
    fi
  fi

  if [ -z "$WINE_PYTHON" ]; then
    # Авто-установка: официальный установщик Python ставится молча (/quiet)
    # в префикс проекта. ~/.wine не затрагивается.
    log "Windows Python не найден — устанавливаю автоматически в изолированный префикс $PROJECT_WINEPREFIX"
    command -v curl >/dev/null 2>&1 || die "curl не найден: он нужен для скачивания установщика Python (pacman -S curl)."
    mkdir -p "$PROJECT_WINEPREFIX"
    export WINEPREFIX="$PROJECT_WINEPREFIX"
    export WINEDLLOVERRIDES="mscoree,mshtml="  # без диалогов mono/gecko
    WINEDEBUG=-all wineboot -u >/dev/null 2>&1 || true
    installer="$WORK_DIR/python-$PYTHON_WINDOWS_VERSION-amd64.exe"
    if [ ! -f "$installer" ]; then
      log "Скачиваю https://www.python.org/ftp/python/$PYTHON_WINDOWS_VERSION/python-$PYTHON_WINDOWS_VERSION-amd64.exe"
      curl -fsSL -o "$installer" "https://www.python.org/ftp/python/$PYTHON_WINDOWS_VERSION/python-$PYTHON_WINDOWS_VERSION-amd64.exe" \
        || die "Не удалось скачать установщик Python. Поставьте Windows Python вручную и задайте WINE_PYTHON=/путь/к/python.exe"
    fi
    wine "$installer" /quiet InstallAllUsers=1 TargetDir='C:\nivora-launcher-python' Include_pip=1 Include_launcher=0 Include_test=0 Shortcuts=0 AssociateFiles=0 \
      || die "Установка Windows Python под wine не удалась (нужен работающий DISPLAY, либо xvfb-run). Поставьте вручную и задайте WINE_PYTHON=/путь/к/python.exe"
    WINE_PYTHON="$PROJECT_WINEPREFIX/drive_c/nivora-launcher-python/python.exe"
  elif [ "${WINE_PYTHON#"$PROJECT_WINEPREFIX"}" != "$WINE_PYTHON" ]; then
    export WINEPREFIX="$PROJECT_WINEPREFIX"
  fi

  [ -f "$WINE_PYTHON" ] || die "Windows Python не найден по пути $WINE_PYTHON. Задайте WINE_PYTHON=/путь/к/python.exe"
  PYINSTALLER_RUN=(wine "$WINE_PYTHON")
fi

# --- 1. Сборка бэкенда PyInstaller ------------------------------------------
# Onedir, а не onefile: onefile-бутлоадер PyInstaller — лидер по ложным
# срабатываниям антивирусов (распаковка во временную папку + запуск оттуда
# эвристики считают дроппером). Onedir-сборки детектируются на порядок реже.
# В nivora-launcher.exe бэкед вкладывается zip-архивом onedir-каталога.
log "Собираю $BACKEND_NAME (PyInstaller, onedir)"
mkdir -p "$WORK_DIR"

# Метаданные (версия/компания/описание) внутри exe: «безликие» бинарники
# сильнее цепляют эвристики антивирусов и выглядят подозрительно в свойствах.
VERSION_FILE_ARGS=()
if python3 - <<'PY' > "$WORK_DIR/backend-version.txt"
import re, pathlib, tomllib

version = "0.1.0"
try:
    version = tomllib.loads(pathlib.Path("pyproject.toml").read_text())["project"]["version"]
except Exception:
    pass
match = re.match(r"(\d+)\.(\d+)\.(\d+)", version)
quad = tuple(int(x) for x in match.groups()) if match else (0, 1, 0)
quad = quad + (min(int(re.sub(r"\D", "", version[match.end():]) or "0"), 65535),)

print("# UTF-8")
print("# Сгенерировано scripts/build_windows_exe.sh — метаданные бэкенд-exe.")
print(f"VSVersionInfo(")
print(f"  ffi=FixedFileInfo(filevers={quad}, prodvers={quad}, mask=0x3f, flags=0x0,")
print(f"                     OS=0x40004, fileType=0x1, subtype=0x0, date=(0, 0)),")
print(f"  kids=[")
print(f"    StringFileInfo([")
print(f"      StringTable('040904B0', [")
print(f"        StringStruct('CompanyName', 'Nivora'),")
print(f"        StringStruct('FileDescription', 'Nivora Launcher backend service'),")
print(f"        StringStruct('FileVersion', '{version}'),")
print(f"        StringStruct('InternalName', 'nivora-launcher-backend'),")
print(f"        StringStruct('LegalCopyright', '(c) Nivora Launcher contributors'),")
print(f"        StringStruct('OriginalFilename', 'nivora-launcher-backend.exe'),")
print(f"        StringStruct('ProductName', 'Nivora Launcher'),")
print(f"        StringStruct('ProductVersion', '{version}'),")
print(f"      ])")
print(f"    ]),")
print(f"    VarFileInfo([VarStruct('Translation', [1033, 1200])])")
print(f"  ]")
print(f")")
PY
then
  # ВАЖНО: PyInstaller резолвит --version-file относительно --specpath
  # (= .build-windows), а не относительно cwd проекта. Абсолютный/корневой
  # относительный путь здесь удваивался («.build-windows/.build-windows/…»)
  # и валил сборку в самом конце. Файл лежит в .build-windows → передаём
  # голое имя.
  VERSION_FILE_ARGS=(--version-file backend-version.txt)
fi

if [ "$HOST_WINDOWS" -eq 1 ]; then
  if [ ! -x "$WORK_DIR/venv/Scripts/python.exe" ]; then
    "${PYTHON_CMD[@]}" -m venv "$WORK_DIR/venv"
  fi
  VENV_PY="$WORK_DIR/venv/Scripts/python.exe"
  "$VENV_PY" -m pip install --upgrade pip --quiet
  "$VENV_PY" -m pip install ".[windows]" pyinstaller
  PYINSTALLER_RUN=("$VENV_PY")
else
  # wine-Python: зависимости проекта и PyInstaller ставятся прямо в него.
  # Windows-Python в префиксе изолирован от системы, так что это безопасно;
  # повторные запуски пропускают уже установленные пакеты.
  log "Ставлю зависимости проекта и PyInstaller в Windows Python (первый запуск долгий)"
  "${PYINSTALLER_RUN[@]}" -m pip install --upgrade pip --quiet
  "${PYINSTALLER_RUN[@]}" -m pip install ".[windows]" pyinstaller
fi

# PyInstaller запускается с относительными путями: под wine cwd отображается
# в Z:\, и относительные пути работают без конвертации winepath.
"${PYINSTALLER_RUN[@]}" -m PyInstaller \
  --noconfirm --clean --onedir --noconsole --noupx \
  --name nivora-launcher-backend \
  --distpath .build-windows/dist \
  --workpath .build-windows/work \
  --specpath .build-windows \
  --collect-submodules nivora_launcher \
  --collect-data nivora_launcher \
  --collect-all keyring \
  --hidden-import keyring.backends.Windows \
  --hidden-import truststore \
  "${VERSION_FILE_ARGS[@]}" \
  scripts/backend_entry.py

BACKEND_DIR="$WORK_DIR/dist/nivora-launcher-backend"
BACKEND_EXE="$BACKEND_DIR/$BACKEND_NAME"
[ -f "$BACKEND_EXE" ] || die "PyInstaller не создал $BACKEND_EXE — смотрите вывод выше."

# --- 1b. Нативные хелперы для Windows (обязательны) ---------------------------
# Кладутся в onedir-каталог ДО упаковки zip: тогда их получают и установщик,
# и portable-exe (раньше zip собирался раньше, и в portable-версии хелперов
# не было). Сбой любого хелпера — ошибка сборки.
log "Собираю nivora-turbo.exe (C++, статически с zlib)"
TURBO_TARGET=windows bash "$SCRIPT_DIR/build_turbo.sh" "$BACKEND_DIR/nivora-turbo.exe" \
  || die "nivora-turbo.exe не собрался (нужен mingw-w64 g++ с posix-потоками или MSYS2 g++)."
GO_BIN="$(command -v go || true)"
[ -n "$GO_BIN" ] || { [ -x "$HOME/.local/go/bin/go" ] && GO_BIN="$HOME/.local/go/bin/go"; } || true
[ -n "$GO_BIN" ] || die "Go не найден: nivora-modcat.exe обязателен (https://go.dev/dl/)."
(cd "$PROJECT_ROOT/go-modcat" && GOOS=windows GOARCH=amd64 CGO_ENABLED=0 "$GO_BIN" build \
    -trimpath -ldflags='-s -w' -o "$BACKEND_DIR/nivora-modcat.exe" .) \
  || die "nivora-modcat.exe не собрался."
cp -f "$PROJECT_ROOT/java/ModScan.java" "$BACKEND_DIR/ModScan.java" || die "ModScan.java не скопирован."

# Ни один бинарник сборки не должен открывать консоль сам по себе.
python3 "$SCRIPT_DIR/check_pe_subsystem.py" --gui "$BACKEND_EXE" \
  || die "Бэкенд собран как консольное приложение (нужен --noconsole)."

# Zip для встраивания в nivora-launcher.exe (include_bytes!) и как
# переносимый отладочный артефакт. Питоном — zip в окружении может не быть.
log "Упаковываю onedir-каталог бэкенда в zip"
BACKEND_ZIP="$WORK_DIR/nivora-launcher-backend.zip"
python3 - "$BACKEND_DIR" "$BACKEND_ZIP" <<'PY'
import os, sys, zipfile

source, target = sys.argv[1], sys.argv[2]
root = os.path.basename(source)
with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
    for base, _dirs, files in os.walk(source):
        for name in files:
            path = os.path.join(base, name)
            archive.write(path, os.path.join(root, os.path.relpath(path, source)))
PY
[ -s "$BACKEND_ZIP" ] || die "не удалось собрать $BACKEND_ZIP"

mkdir -p "$(dirname "$BACKEND_RESOURCE")" "$OUTPUT_DIR"
# Для встраивания: include_bytes! ждёт zip рядом (см. lib.rs).
BACKEND_ZIP_RESOURCE="$PROJECT_ROOT/frontend/src-tauri/resources/backend/nivora-launcher-backend.zip"
cp -f "$BACKEND_ZIP" "$BACKEND_ZIP_RESOURCE"
# Для NSIS-установщика: распакованный onedir кладём ресурсом — установленная
# копия запускает бэкенд напрямую, без распаковки при первом старте.
BACKEND_APP_RESOURCE="$PROJECT_ROOT/frontend/src-tauri/resources/backend/app"
rm -rf "$BACKEND_APP_RESOURCE"
mkdir -p "$BACKEND_APP_RESOURCE"
cp -R "$BACKEND_DIR/." "$BACKEND_APP_RESOURCE/"

log "Бэкенд готов: $BACKEND_ZIP_RESOURCE (+ $BACKEND_APP_RESOURCE для установщика)"

if [ "$ONLY_BACKEND" -eq 1 ]; then
  cp -f "$BACKEND_ZIP" "$OUTPUT_DIR/$BACKEND_NAME.zip"
  log "Готово (--only-backend): $OUTPUT_DIR/$BACKEND_NAME.zip"
  exit 0
fi

# --- 2. Фронтенд --------------------------------------------------------------
log "Собираю фронтенд (npm)"
cd frontend
npm install --no-audit --no-fund
# npm 11 может блокировать postinstall esbuild — разрешаем и пересобираем, как в Arch-сборке.
if npm install-scripts ls >/dev/null 2>&1; then
  npm install-scripts approve esbuild || true
fi
npm rebuild esbuild --no-audit --no-fund
npm run build
cd "$PROJECT_ROOT"

# --- 2.5 Хостовый тулчейн для кросс-сборки Tauri (Linux -> Windows) ------------
# cargo-xwin подставляет clang-cl, lld-link и скачанный Windows SDK (CRT+UCRT),
# поэтому msvc-таргет линкуется без Windows. Всё недостающее ставим сами:
# системные пакеты — pacman/apt (нужен root/sudo), rustup и cargo-xwin — в $HOME
# (дистрибутивный Rust и ~/.wine не затрагиваются).
SUDO=()
if [ "$(id -u)" -ne 0 ]; then
  command -v sudo >/dev/null 2>&1 || die "Для установки системных пакетов нужен root или sudo."
  SUDO=(sudo)
fi

extract_deb_data() {
  # Распаковка data.tar.* из .deb (deb — это ar-архив; ar и tar есть везде).
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
  # Шаблон tauri-установщика подключает !include "Win\RestartManager.nsh" — файл
  # появился только в NSIS 3.10, а в Debian-данных 3.08 (и системном nsis Debian
  # stable) его нет. Докидываем его из nsis-common 3.11, иначе makensis падает
  # на include. Нужен и при системном makensis, и при нашем из deb.
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

  # Если /usr/share/nsis — наш симлинк в .build-windows, пишем напрямую;
  # в настоящий системный каталог — через sudo. pwd -P резолвит симлинк.
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
  # tauri-bundler запускает системный makensis из PATH (Stubs/Plugins он качает
  # сам). В Arch nsis есть только в AUR, поэтому при отсутствии makensis берём
  # готовый бинарь из Debian-пакета: ему нужны лишь glibc/libstdc++/zlib, которые
  # есть и в Arch. Данные makensis ищет по зашитому пути /usr/share/nsis —
  # подкладываем их симлинком (если системного nsis нет).
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

    # Висячий симлинк с прошлого прогона: цель жила в .build-windows внутри
    # дерева проекта, и после пересоздания/перепаковки каталога проверка -e
    # даёт «пути нет», но ln отказывается ставить новую ссылку («Файл
    # существует»). Чистим ссылку и ставим заново.
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
  # clang-cl — C-код, lld-link — линковка msvc, llvm-objcopy — ресурсы бандла.
  for binary in clang-cl lld-link llvm-objcopy; do
    command -v "$binary" >/dev/null 2>&1 || missing+=("$binary")
  done
  if [ "${#missing[@]}" -gt 0 ]; then
    log "Ставлю системные пакеты для кросс-сборки (не хватало: ${missing[*]})"
    if command -v pacman >/dev/null 2>&1; then
      # nsis сюда НЕ включаем: в Arch его нет в официальных репо (см. ensure_nsis_linux).
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

  # rustup: у дистрибутивного Rust нет std для msvc-таргета, нужен rustup-тулчейн.
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

  # Активный тулчейн: если default не настроен, ставим fresh stable.
  if ! rustup which cargo >/dev/null 2>&1; then
    log "Ставлю Rust-тулчейн stable (первый запуск долгий)"
    rustup toolchain install stable --profile minimal
    rustup default stable
  fi
  # cargo должен быть из тулчейна rustup (в PATH может быть дистрибутивный).
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

# --- 3. Tauri build -----------------------------------------------------------
if [ "$HOST_WINDOWS" -eq 1 ]; then
  TAURI_TARGET_ARGS=()
  RELEASE_DIR="frontend/src-tauri/target/release"
else
  ensure_linux_cross_toolchain
  # tauri CLI запускает cargo через cargo-xwin (--runner): он подставляет
  # clang-cl, lld-link и Windows SDK, поэтому линковка msvc идёт без Windows.
  # --bundles nsis: MSI/WiX на Linux-хосте не собирается (и не нужен).
  TAURI_TARGET_ARGS=(--target x86_64-pc-windows-msvc --runner cargo-xwin --bundles nsis)
  RELEASE_DIR="frontend/src-tauri/target/x86_64-pc-windows-msvc/release"
fi

# Подписанные updater-артефакты создаём только при наличии ключа,
# локальной сборке exe они не нужны (см. tauri.arch.conf.json для Arch).
# tauri.windows.conf.json (свой NSIS-шаблон установщика, ресурсы бэкенда)
# tauri CLI подхватывает по ОС ХОСТА: при кросс-сборке с Linux он молча
# игнорировался, и получался стандартный установщик Tauri. Передаём явно.
TAURI_CONFIG_ARGS=(--config src-tauri/tauri.windows.conf.json)
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
  TAURI_CONFIG_ARGS+=(--config '{"bundle":{"createUpdaterArtifacts":false}}')
fi

# include_bytes! в lib.rs упаковывает zip бэкеда В САМ nivora-launcher.exe —
# на момент tauri build архив обязан существовать, иначе cargo выдаст
# малопонятную ошибку. Здесь — явная проверка с понятным сообщением.
if [ ! -f "$BACKEND_ZIP_RESOURCE" ]; then
  die "Не найден $BACKEND_ZIP_RESOURCE. tauri build не сможет встроить бэкенд в nivora-launcher.exe — сначала соберите его (PyInstaller-шаг выше)."
fi

log "Собираю Tauri (Windows exe; первая кросс-сборка качает Windows SDK и компилирует все зависимости — это надолго)"
cd frontend
NIVORA_REQUIRE_EMBEDDED_BACKEND=1 npm run tauri -- build "${TAURI_TARGET_ARGS[@]}" "${TAURI_CONFIG_ARGS[@]}"
cd "$PROJECT_ROOT"

# --- 4. Артефакты -------------------------------------------------------------
setup_exe="$(ls -1t "$RELEASE_DIR/bundle/nsis/"*-setup.exe 2>/dev/null | head -n 1 || true)"
nsis_script="$RELEASE_DIR/nsis/x64/installer.nsi"
if [ -f "$nsis_script" ] && ! grep -q "NivoraPurgeData" "$nsis_script"; then
  die "Установщик собран из стандартного шаблона Tauri, а не из frontend/src-tauri/windows/installer.nsi — проверьте bundle.windows.nsis.template."
fi
raw_exe="$RELEASE_DIR/nivora-launcher.exe"

[ -n "$setup_exe" ] || die "NSIS-установщик не найден в $RELEASE_DIR/bundle/nsis/ — смотрите вывод tauri build."
[ -f "$raw_exe" ] || die "Автономный exe не найден: $raw_exe"
python3 "$SCRIPT_DIR/check_pe_subsystem.py" --gui "$raw_exe" \
  || die "nivora-launcher.exe собран с консольной подсистемой — в production консоли быть не должно."

cp -f "$setup_exe" "$OUTPUT_DIR/"
cp -f "$raw_exe" "$OUTPUT_DIR/"
log "Установщик: $OUTPUT_DIR/$(basename "$setup_exe")"
log "Автономный exe: $OUTPUT_DIR/nivora-launcher.exe"

# --- 4.1 Кодовая подпись (опционально) ----------------------------------------
# Authenticode-подпись убирает «Неизвестный издатель» в SmartScreen/UAC.
# Включается заданием WINDOWS_CERT_FILE (PKCS#12) [+ WINDOWS_CERT_PASSWORD].
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

# --- 4.1.1 Фид самообновления (latest.json) ---------------------------------------
# Встроенный апдейтер читает releases/latest/download/latest.json. Без этого
# файла в релизе лаунчер пишет «Could not fetch a valid release JSON from the
# remote» и не видит новых версий. Подписываем ФИНАЛЬНЫЙ установщик (уже после
# osslsigncode — Authenticode меняет байты, и подпись tauri build устаревает).
APP_VERSION_TAURI="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' frontend/src-tauri/tauri.conf.json)"
RELEASE_TAG="${RELEASE_TAG:-$APP_VERSION_TAURI}"
GITHUB_RELEASE_REPO="${GITHUB_RELEASE_REPO:-vedrooou0/Nivora-launcher}"
setup_name="$(basename "$setup_exe")"
# GitHub заменяет пробелы в именах ассетов точками — сразу называем так же,
# чтобы ссылка в latest.json совпала с реальным файлом релиза.
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
  python3 - "$OUTPUT_DIR" "$update_asset" "$APP_VERSION_TAURI" "$RELEASE_TAG" "$GITHUB_RELEASE_REPO" <<'PY'
import datetime, json, sys, urllib.parse
from pathlib import Path
out, asset, version, tag, repo = sys.argv[1:6]
signature = (Path(out) / f"{asset}.sig").read_text(encoding="utf-8").strip()
url = f"https://github.com/{repo}/releases/download/{urllib.parse.quote(tag)}/{urllib.parse.quote(asset)}"
entry = {"signature": signature, "url": url}
feed = {
    "version": version,
    "notes": f"Nivora Launcher {version}",
    "pub_date": datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
    "platforms": {"windows-x86_64": entry, "windows-x86_64-nsis": entry},
}
(Path(out) / "latest.json").write_text(json.dumps(feed, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
PY
  log "latest.json: $OUTPUT_DIR/latest.json — загрузите его в релиз $RELEASE_TAG вместе с $update_asset"
  log "Например: gh release create $RELEASE_TAG \"$OUTPUT_DIR/$update_asset\" \"$OUTPUT_DIR/latest.json\" --title \"Nivora Launcher $APP_VERSION_TAURI\""
else
  log "Внимание: нет TAURI_SIGNING_PRIVATE_KEY — latest.json НЕ создан. Встроенное самообновление этот релиз не увидит; лаунчер 1.1+ всё равно найдёт его через GitHub API и предложит скачать вручную."
fi

# --- 4.2 Контрольные суммы -----------------------------------------------------
( cd "$OUTPUT_DIR" && sha256sum ./*.exe > sha256sums.txt )
log "Суммы SHA256: $OUTPUT_DIR/sha256sums.txt"

log "Готово. Артефакты: $OUTPUT_DIR"
log "Главный файл — nivora-launcher.exe (ОДИН файл: бэкенд встроен zip-архивом и при первом запуске распаковывается в %LOCALAPPDATA%\\nivora-launcher\\backend). Установщик кладёт бэкенд рядом с приложением — без распаковки."
( cd "$OUTPUT_DIR" && for artifact in *; do
    [ -f "$artifact" ] || continue
    printf '    %8s  %s\n' "$(du -h "$artifact" | cut -f1)" "$artifact"
  done )
