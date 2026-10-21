# Сборка Nivora Launcher

Все платформы собираются платформенными скриптами из `scripts/`;
`scripts/build_all.sh` прогоняет цели для текущей ОС и проверяет, что
после каждой цели артефакт действительно появился («успех без артефакта»
невозможен).

| Цель | Скрипт | Артефакты (`dist/`) |
|---|---|---|
| `windows` | `build_windows_exe.sh` | `windows/*-setup.exe`, `windows/nivora-launcher.exe`, `windows/sha256sums.txt` |
| `deb` | `build_deb.sh` | `deb/*.deb` |
| `rpm` | `build_rpm.sh` | `rpm/*.rpm` |
| `arch` | `build_arch_package.sh` | `arch/*.pkg.tar.zst` |
| `macos` | `build_macos_app.sh` | `macos/Nivora-Launcher_<версия>_<arch>.dmg`, `.app.zip`, `SHA256SUMS` |

## Windows

```bash
bash scripts/build_windows_exe.sh        # на Windows (Git Bash) или Linux (кросс через wine + cargo-xwin)
```

- Бэкенд собирается PyInstaller в режиме **onedir + noconsole** и
  вкладывается в `nivora-launcher.exe` zip-архивом; при первом старте
  распаковывается в `%LOCALAPPDATA%\nivora-launcher\backend`. NSIS-копия
  кладёт бэкенд рядом с приложением — без распаковки.
- Нативные хелперы (`nivora-turbo.exe`, `nivora-modcat.exe`,
  `ModScan.java`) собираются скриптом и вкладываются ДО упаковки: их
  получают и установщик, и портативный exe.
- `scripts/check_pe_subsystem.py` проверяет подсистему PE: ни бэкенд, ни
  лаунчер не должны открывать консоль.
- Подпись Authenticode — опционально (`WINDOWS_CERT_FILE`,
  `osslsigncode`). Без неё SmartScreen предупреждает при первом запуске —
  это нормально для неподписанных сборок.

## macOS

```bash
bash scripts/build_macos_app.sh                  # архитектура хоста
MACOS_ARCH=arm64  bash scripts/build_macos_app.sh
MACOS_ARCH=x86_64 bash scripts/build_macos_app.sh   # на arm64 нужен x86_64-Python
MACOS_ARCH=both   bash scripts/build_all.sh macos # обе архитектуры
```

Скрипт проверяет зависимости, собирает бэкенд (PyInstaller onedir в
`Resources/backend/app`), хелперы под целевую архитектуру, собирает
`.app` + `.dmg` и валидирует: наличие артефактов, версию в Info.plist,
архитектуру бинарников (`lipo`), ad-hoc подпись (`codesign --verify
--deep --strict`) и монтирование DMG. `universal2` не собирается: у
`psutil` нет universal2-колёс.

### Подпись и Gatekeeper

Без Apple Developer ID приложение подписывается **ad-hoc** и **не
нотаризуется** — это осознанное ограничение сборки, а не ошибка:

- при первом запуске скачанной копии Gatekeeper спросит подтверждение:
  **правый клик по «Nivora Launcher.app» → «Открыть» → «Открыть»**, либо
  `xattr -dr com.apple.quarantine "Nivora Launcher.app"`;
- встроенное самообновление на macOS отключено (фид апдейтера требует
  подписи Developer ID) — обновляйтесь скачиванием нового `.dmg`;
- для подписанных релизов задайте секреты `APPLE_CERTIFICATE`,
  `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` —
  `scripts/build_macos_app.sh` и релизный воркфлоу подхватят их.

## Linux

```bash
bash scripts/build_all.sh deb rpm    # или build_deb.sh / build_rpm.sh / build_arch_package.sh
```

Хелперы собираются под нативную архитектуру и включаются в пакет;
ресурсы BedrockOnLinux лежат в `frontend/src-tauri/resources/vendor/`.

## Дымовой прогон упакованного приложения

Каждая сборка умеет проверить саму себя (`--smoke-test`): операции идут
через тот же мост, что использует UI, включая 20 параллельных нативных и
воркерных вызовов; отчёт — JSON в диагностическом каталоге, код выхода
`0` — успех, `3` — провал шага, `4` — превышен бюджет времени.

```bash
./nivora-launcher.exe --smoke-test                                    # Windows
"/Applications/Nivora Launcher.app/Contents/MacOS/nivora-launcher" --smoke-test
nivora-launcher --smoke-test                                          # Linux (под xvfb-run в headless)
```

## CI

`.github/workflows/ci.yml` собирает все три платформы этими же скриптами
и запускает `--smoke-test` против упакованных приложений (Windows:
портативный exe и тихо установленная NSIS-копия; macOS: `.app` из
сборки; Linux: deb устанавливается в раннер, прогон под `xvfb-run`).
Релизный воркфлоу (`release.yml`, тег `v*`) публикует артефакты в GitHub
Release; `latest.json` для встроенного апдейтера собирает
`build_windows_exe.sh` при заданном `TAURI_SIGNING_PRIVATE_KEY`.
