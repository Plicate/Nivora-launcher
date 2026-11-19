# Cloud Sync

Nivora Launcher uses WebDAV as a provider-neutral cloud backend. It works with Nextcloud, ownCloud, Yandex Disk and other services that expose a WebDAV endpoint.

## What is synchronized

- The configured game directory and its instances, libraries, versions, resource packs, shaders and worlds.
- Launcher data such as skins and wallpapers. Plugin directories are excluded.
- A filtered `settings.json` backup; execution paths, credentials and endpoint settings are never restored.
- `accounts.json` is never uploaded or restored. Microsoft tokens belong in Secret Service, not in the cloud backup.

Logs and temporary `.part` files are skipped. Sync uploads a backup copy and does not delete remote files automatically. Remote files can be removed individually from the launcher.

## Credentials

The WebDAV password or app token is kept in the desktop libsecret keyring. It is never returned to the frontend or included in normal settings output. HTTP endpoints are rejected. Use a service-specific app token when the provider supports it.

## Restore

Sync writes a version 2 manifest with SHA-256 entries and a MAC made with a local key that is not uploaded. Restore refuses unsigned or changed manifests, shows a preview, and requires confirmation. It writes only allowlisted settings and data paths, excludes plugins and server control JSON, and never deletes local files that are absent from the cloud.

The manifest key is device-local. A backup can currently be verified and restored only on the installation that created it; a different device will reject the MAC. Secure key transfer or passphrase-based cross-device restore is not implemented.

---

<!-- русская версия / Russian version -->

# Облачная синхронизация

Nivora Launcher использует WebDAV как провайдер-нейтральный облачный бэкенд. Работают Nextcloud, ownCloud, Яндекс Диск и другие сервисы с WebDAV-endpoint.

## Что синхронизируется

- Настроенный каталог игры и его экземпляры, библиотеки, версии, ресурспаки, шейдеры и миры.
- Данные лаунчера: скины и обои. Каталоги плагинов исключены.
- Отфильтрованный бэкап `settings.json`; пути исполнения, учётные данные и настройки endpoint никогда не восстанавливаются.
- `accounts.json` никогда не выгружается и не восстанавливается. Microsoft-токены живут в Secret Service, а не в облачной копии.

Логи и временные файлы `.part` пропускаются. Синхронизация выгружает резервную копию и не удаляет удалённые файлы автоматически. Отдельные удалённые файлы можно удалить из лаунчера.

## Учётные данные

Пароль WebDAV или app-токен хранится в desktop keyring libsecret. Он никогда не отдаётся во фронтенд и не попадает в обычный вывод настроек. HTTP-endpoint отклоняются. Если провайдер поддерживает — используйте отдельный app-токен сервиса.

## Восстановление

Синхронизация пишет манифест версии 2 с SHA-256-записями и MAC на локальном ключе, который не выгружается. Восстановление отказывается от неподписанных или изменённых манифестов, показывает предпросмотр и требует подтверждения. Пишутся только разрешённые настройки и пути данных; плагины и управляющие JSON серверов исключены; локальные файлы, отсутствующие в облаке, никогда не удаляются.

Ключ манифеста локален для устройства. Сейчас копию можно проверить и восстановить только на той установке, которая её создала; другое устройство отклонит MAC. Безопасная передача ключа или восстановование между устройствами по парольной фразе не реализованы.
