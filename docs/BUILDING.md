# Сборка Nivora Launcher

Все платформы собираются скриптами из `scripts/`. `scripts/build_all.sh`
запускает цели для текущей ОС и проверяет наличие ожидаемых артефактов.

| Цель | Скрипт | Артефакты (`dist/`) |
|---|---|---|
| `windows` | `build_windows_exe.sh` | `windows/*-setup.exe`, `windows/nivora-launcher.exe`, `windows/sha256sums.txt` |
| `deb` | `build_deb.sh` | `deb/*.deb` |
| `rpm` | `build_rpm.sh` | `rpm/*.rpm` |
| `arch` | `build_arch_package.sh` | `arch/*.pkg.tar.zst` |
| `macos` | `build_macos_app.sh` | `macos/Nivora-Launcher_<версия>_<arch>.dmg`, `.app.zip`, `SHA256SUMS` |

## Windows

```bash
bash scripts/build_windows_exe.sh        # на Windows (Git Bash) или Linux (нативная кросс-сборка через cargo-xwin)
```

- Нативные хелперы (`nivora-turbo.exe`, `nivora-modcat.exe`,
  `ModScan.java`) собираются скриптом и вкладываются ДО упаковки: их
  получают и установщик, и портативный exe.
- Подсистема PE проверяется `objdump` (Windows GUI): лаунчер не должен
  открывать консоль.
- Подпись Authenticode — опционально (`WINDOWS_CERT_FILE`,
  `osslsigncode`). Без неё SmartScreen предупреждает при первом запуске —
  это нормально для неподписанных сборок.

## macOS

```bash
bash scripts/build_macos_app.sh                  # архитектура хоста
MACOS_ARCH=arm64  bash scripts/build_macos_app.sh
MACOS_ARCH=x86_64 bash scripts/build_macos_app.sh
MACOS_ARCH=both   bash scripts/build_all.sh macos # обе архитектуры
```

Скрипт проверяет зависимости, собирает хелперы под целевую архитектуру,
собирает `.app` + `.dmg` и валидирует: наличие артефактов, версию в
Info.plist, архитектуру бинарников (`lipo`), ad-hoc подпись (`codesign
--verify --deep --strict`) и монтирование DMG. `universal2` не собирается:
хелперы и движок компонуются отдельно под каждую архитектуру.

### Подпись и Gatekeeper

Без Apple Developer ID приложение подписывается **ad-hoc** и не проходит notarization:

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

`.github/workflows/ci.yml` использует те же скрипты для сборки трёх
платформ и запускает `--smoke-test` на упакованных приложениях.
`release.yml` публикует артефакты для тегов `v*`; `build_windows_exe.sh`
создаёт `latest.json`, если задан `TAURI_SIGNING_PRIVATE_KEY`.
