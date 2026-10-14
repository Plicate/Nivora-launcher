# Nivora Launcher updater setup

The updater is **disabled** in shipped builds: `tauri.conf.json` carries no `plugins.updater` block and `bundle.createUpdaterArtifacts` is `false`. The in-app "Self-update" card honestly reports that updates are not configured instead of pretending to check a placeholder endpoint — an updater pointed at `updates.example.invalid` would be broken or unsafe.

To enable real updates before a release:

1. Generate a signing keypair with the Tauri CLI.
2. Add a `plugins.updater` block to `tauri.conf.json` with the **public** key.
3. Point `endpoints` at your signed `latest.json` host.
4. Export `TAURI_SIGNING_PRIVATE_KEY` (and optionally `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) before running the release build.
5. Set `bundle.createUpdaterArtifacts` back to `true` so Tauri emits signed updater artifacts next to the normal bundles.

---

<!-- Русская версия / Russian version -->

# Настройка самообновления Nivora Launcher

В распространяемых сборках апдейтер **выключен**: в `tauri.conf.json` нет блока `plugins.updater`, а `bundle.createUpdaterArtifacts` равен `false`. Карточка «Самообновление» в интерфейсе честно сообщает, что обновления не настроены, вместо видимости проверки несуществующего endpoint — апдейтер, указывающий на `updates.example.invalid`, был бы сломан или небезопасен.

Чтобы включить настоящие обновления перед релизом:

1. Сгенерируйте пару ключей подписи через Tauri CLI.
2. Добавьте в `tauri.conf.json` блок `plugins.updater` с **публичным** ключом.
3. Укажите в `endpoints` хост подписанного `latest.json`.
4. Экспортируйте `TAURI_SIGNING_PRIVATE_KEY` (и при необходимости `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) перед релизной сборкой.
5. Верните `bundle.createUpdaterArtifacts` в `true`, чтобы Tauri создавал подписанные updater-артефакты рядом с обычными пакетами.
