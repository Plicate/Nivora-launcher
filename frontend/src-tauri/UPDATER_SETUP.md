# Nivora Launcher updater setup

The updater is **enabled** in `tauri.conf.json`:

- `plugins.updater.endpoints` points at `https://github.com/vedrooou0/Nivora-launcher/releases/latest/download/latest.json`
- `plugins.updater.pubkey` verifies artifact signatures (minisign key `A0FF285547EE928D`)
- `bundle.createUpdaterArtifacts` is `true`, so bundling signs updater artifacts
- `capabilities/default.json` grants `updater:default` and `process:default` to the main window, so the UI can check, download, install and relaunch

## How a release must be published for updates to work

`release.yml` is the only correct path — it runs `tauri-action`, which uploads the
installers **plus `latest.json` and `.sig` files** to the GitHub Release:

1. Bump `version` in `tauri.conf.json` (and `frontend/src-tauri/Cargo.toml`) — the updater
   compares versions; same version = "no update".
2. Push a tag **matching `v*`** (e.g. `v1.0.1`). The tag name is what triggers `release.yml`.
3. The workflow publishes a **full release** (not a pre-release!). `releases/latest/download/…`
   resolves only to full releases; a pre-release never becomes "latest".
4. Repository secrets must be set: `TAURI_SIGNING_PRIVATE_KEY` (contents of the private key
   file) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (may be empty).

## Local builds

With `createUpdaterArtifacts: true` and a pubkey in the config, `tauri build` **fails**
without the matching private key:

> A public key has been found, but no private key. Make sure to set `TAURI_SIGNING_PRIVATE_KEY` environment variable.

Export before building locally:

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/nivora-launcher.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""   # if the key has no password
```

If the private key is lost, generate a new pair (`npm run tauri -- signer generate -w ~/.tauri/nivora-launcher.key`),
replace `pubkey` in `tauri.conf.json`, and set the new key as the repo secret. Builds installed
with the old key will reject updates signed with the new one, so this is only safe while the
user base is empty.

## Platform notes

- **Windows**: updates via NSIS/MSI artifacts — supported.
- **Linux**: the v2 updater only self-updates **AppImage** builds. `deb`/`rpm`/arch packages
  are published for manual installation; `latest.json` points at the AppImage. The local
  `scripts/build_all.sh` builds `arch/deb/rpm` only — those builds never self-update.

## Manual update with the Windows installer (already built-in)

The NSIS setup.exe shipped by Tauri supports command-line switches:

| Switch | Effect |
|---|---|
| `/UPDATE` | Update mode: skips the «Already Installed» page, overwrites files in place, keeps shortcuts and the autostart entry |
| `/S` | Fully silent install (combine: `setup.exe /UPDATE /S`) |
| `/P` | Passive mode: shows progress bar only, no questions |
| `/NS` | Do not (re)create shortcuts |
| `/D=<path>` | Install into `<path>` (last argument, no quotes) |

So to refresh an installed copy without uninstalling, run:

```bat
Nivora.Launcher_1.0.1_x64-setup.exe /UPDATE
```

Without `/UPDATE`, running a NEWER setup.exe over an old install shows a choice
«Uninstall before installing / Do not uninstall» — «Do not uninstall» performs the same
in-place update. The **MSI** package upgrades automatically when the version is higher
(`upgradeCode` is fixed in `tauri.conf.json`); running the same-version MSI only offers
repair/remove — that is standard Windows Installer behavior.

---

<!-- Русская версия / Russian version -->

# Настройка самообновления Nivora Launcher

Самообновление **включено** в `tauri.conf.json`:

- `plugins.updater.endpoints` указывает на `https://github.com/vedrooou0/Nivora-launcher/releases/latest/download/latest.json`
- `plugins.updater.pubkey` проверяет подписи артефактов (minisign-ключ `A0FF285547EE928D`)
- `bundle.createUpdaterArtifacts` = `true` — сборка подписывает updater-артефакты
- `capabilities/default.json` выдаёт главному окну `updater:default` и `process:default`,
  поэтому интерфейс умеет проверять, скачивать, ставить обновления и перезапускаться

## Как нужно публиковать релиз, чтобы обновления работали

`release.yml` — единственный правильный путь: он запускает `tauri-action`, который кладёт в
GitHub Release установщики **плюс `latest.json` и `.sig`-файлы**:

1. Поднимите `version` в `tauri.conf.json` (и в `frontend/src-tauri/Cargo.toml`) — апдейтер
   сравнивает версии; та же версия = «обновлений нет».
2. Запушьте тег, **начинающийся с `v`** (например, `v1.0.1`) — именно он запускает `release.yml`.
3. Воркфлоу публикует **полноценный релиз** (не pre-release!). Ссылка `releases/latest/download/…`
   ведёт только на полноценные релизы; pre-release никогда не становится «latest».
4. В секретах репозитория должны быть: `TAURI_SIGNING_PRIVATE_KEY` (содержимое файла приватного
   ключа) и `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (может быть пустым).

## Локальные сборки

С `createUpdaterArtifacts: true` и pubkey в конфиге `tauri build` **падает** без приватного ключа:

> A public key has been found, but no private key. Make sure to set `TAURI_SIGNING_PRIVATE_KEY` environment variable.

Экспортируйте перед локальной сборкой:

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/nivora-launcher.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""   # если ключ без пароля
```

Если приватный ключ утерян — сгенерируйте новую пару (`npm run tauri -- signer generate -w ~/.tauri/nivora-launcher.key`),
замените `pubkey` в `tauri.conf.json` и положите новый ключ в секреты репозитория. Сборки,
установленные со старым ключом, отвергнут обновления, подписанные новым, — менять ключ можно,
пока пользователей нет.

## Особенности платформ

- **Windows**: обновления через NSIS/MSI-артефакты — поддерживается.
- **Linux**: v2-апдейтер самообновляет только **AppImage**. Пакеты `deb`/`rpm`/arch публикуются
  для ручной установки; `latest.json` ссылается на AppImage. Локальный `scripts/build_all.sh`
  собирает только `arch/deb/rpm` — такие сборки сами не обновляются.

## Ручное обновление установщиком Windows (уже встроено)

NSIS-setup.exe, который собирает Tauri, понимает параметры командной строки:

| Параметр | Что делает |
|---|---|
| `/UPDATE` | Режим обновления: пропускает страницу «Already Installed», перезаписывает файлы на месте, сохраняет ярлыки и автозапуск |
| `/S` | Полностью тихая установка (комбинируется: `setup.exe /UPDATE /S`) |
| `/P` | Пассивный режим: только прогресс-бар, без вопросов |
| `/NS` | Не пересоздавать ярлыки |
| `/D=<путь>` | Установить в `<путь>` (последний аргумент, без кавычек) |

Обновить установленную копию без удаления:

```bat
Nivora.Launcher_1.0.1_x64-setup.exe /UPDATE
```

Без `/UPDATE` запуск НОВОГО setup.exe поверх старой версии предлагает выбор
«Удалить перед установкой / Не удалять» — «Не удалять» делает то же обновление на месте.
**MSI**-пакет при более высокой версии обновляется автоматически (`upgradeCode` зафиксирован
в `tauri.conf.json`); повторный запуск той же версии MSI предлагает только восстановление/
удаление — это стандартное поведение Windows Installer.

## Custom Windows installer (NSIS template)

`windows/installer.nsi` (wired in `tauri.windows.conf.json`) replaces the stock Tauri template:

- If the launcher is already installed, the setup shows a maintenance page:
  **Update** (replace program files only, keep data and shortcuts),
  **Uninstall** (program removed, launcher folder kept) and
  **Completely remove** (program + `%APPDATA%\nivora-launcher`, `%LOCALAPPDATA%\nivora-launcher`,
  `%LOCALAPPDATA%\Cache\nivora-launcher`, legacy `trial-launcher` folders, WebView2 data and saved
  Microsoft sign-ins in Windows Credential Manager).
- `uninstall.exe` asks the same keep/full question; `/MODE=keep`, `/MODE=full` (`/FULL`) skip it.
- `/UPDATE`, `/P`, `/S`, `/NS`, `/R` keep working, so the built-in self-update is unaffected.
- A game folder moved to a custom location in settings is never deleted automatically.
