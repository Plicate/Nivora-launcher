# Выпуск релиза

## Версия и проверка

Перед выпуском синхронизируйте версию в `frontend/package.json`, `frontend/src-tauri/Cargo.toml`, `frontend/src-tauri/tauri.conf.json`, `frontend/src/version.ts` и `frontend/src-tauri/src/rust_engine/constants.rs`. Метка сборки в `frontend/src/services/bridge.ts` и Rust-константе `BUILD_STAMP` тоже должна совпадать.

Запустите фронтенд-тесты и production-сборку. Платформенные команды описаны в [инструкции по сборке](BUILDING.md).

## Публикация

`release.yml` запускается тегом `v*`. Workflow собирает и проверяет Windows-, macOS- и Linux-пакеты, затем публикует артефакты в GitHub Release. Linux-сборка в CI выпускает `deb` и `rpm`; macOS-сборки — отдельные `arm64` и `x86_64` DMG.

Для Windows-обновлений задайте секреты репозитория `TAURI_SIGNING_PRIVATE_KEY` и `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Скрипт `scripts/build_windows_exe.sh` подписывает установщик и создаёт `latest.json`; без ключа пакет выйдет, а лаунчер предложит скачать его со страницы релиза вместо автоматической установки. Подпись Windows Authenticode (`WINDOWS_CERT_FILE`, `WINDOWS_CERT_PASSWORD`) необязательна.

macOS-сборка без `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` и `APPLE_SIGNING_IDENTITY` подписывается ad-hoc и не проходит notarization. Linux-пакеты `deb` и `rpm` обновляются ручной переустановкой.

## Запуск вручную

После синхронизации версии и проверок запустите соответствующий платформенный скрипт из `scripts/` либо создайте тег:

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

Workflow запускается только тегом и публикует проверенные артефакты; перед отправкой тега убедитесь, что секреты подписи настроены для нужной платформы.
