# Nivora Launcher Plugin API

API v2/v3 (`plugin.json`) запускает entrypoint в отдельном QuickJS worker. До запуска проверяются целостность пакета и доверие к издателю или явное согласие на локальный запуск unsigned-пакета. Каждый host-вызов дополнительно проверяет grant соответствующего permission. API v1 — статический: описывается тем же `plugin.json` с `"api_version": 1` и не запускает код. Прежний `plugin.yml` читается только для совместимости со старыми пакетами.

## `plugin.json` (статический, API v1)

```json
{
  "id": "server-notes",
  "name": "Server Notes",
  "version": "1.0.0",
  "api_version": "1",
  "entrypoint": "dist/index.js",
  "description": "Notes scoped to the current instance",
  "permissions": ["storage:workspace", "storage:database", "files:plugin", "ui:compose"]
}
```

`entrypoint` обязан быть относительным `.js`-файлом. Запрещены абсолютные пути и `..`. Поле проверяется, но файл не запускается.

Для тестирования sidebar host поддерживает ограниченную декларацию `sidebar_tab`:

```json
{
  "permissions": ["ui:compose"],
  "sidebar_tab": { "label": "Test Tab" }
}
```

Поле `sidebar_tab` может содержать только `label` длиной 1–40 печатных символов и требует `ui:compose`. Вкладка видна только пока плагин установлен и включён. Host выводит label как текст и показывает статичную страницу; HTML, JavaScript и plugin UI-фрагменты не загружаются. Отключение или удаление плагина убирает вкладку.

### Декларативный UI v2 (API v2/v3)

Рантайм-плагины регистрируют `ui.views` с корнем из безопасных узлов: контейнеры `form`, `tabs`, `accordion`, `group`, `card` (блок с tone и заголовком), `grid` (1–4 колонки), `include`; контролы `input`, `textarea`, `number`, `slider`, `color` (#rrggbb), `datetime`, `select`, `checkbox`, `switch`; отображение `text`, `badge`, `button`, `progress`, `metric` (крупное значение), `alert` (заметка с tone), `code`, `image` (только http/https, проверяется на рендере), `list`, `link`, `divider`, `table`, `chart`. Один и тот же словарь проверяют пять зеркал: типы `plugins/api.ts`, whitelist `compose` в worker, `isUiNode` на host-стороне, валидатор manifest и Vue-рендерер — лишние ключи и неизвестные типы отбрасываются везде.

Слоты монтируются нативно: `sidebar_tab` — настоящая вкладка рядом со статичными вкладками плагинов, `home_widget` — на странице «Экземпляры», `settings_section` — внутри настроек, `instance_tab` — в панели плагина. Управления с `config_key` пишут в config плагина, `onChange` — ID declarative-действия, не JavaScript. Динамические данные подключаются через `ui.bindData` (static/event/poll 2–60 c) к list/table/chart/text/чекбоксам и значениям контролов.

Архив проверяется до записи: launcher ограничивает его размер 64 MiB, распакованный размер 128 MiB и 4096 файлов; символьные ссылки и выход за staging-каталог отклоняются. Архив сохраняется выключенным. Включение и выключение меняют только локальный marker и не запускают код.

В API v1 поля permissions остаются метаданными и не открывают plugin-коду доступ к host API. Launcher не загружает манифест как код.

## Встроенные LAN-комнаты

`network:listen` не входит в набор permissions API v2/v3, поэтому сторонний плагин не может получить доступ к общему менеджеру LAN-комнат. Встроенная страница «LAN-комнаты» продолжает работать через внутренний интерфейс лаунчера.

LAN-сервис принимает участников по прямому частному IP-адресу хоста и проксирует для участника один Minecraft TCP-порт через локальный loopback. Это не Hamachi/Radmin-подобная виртуальная сеть: нет виртуального адаптера, произвольной маршрутизации, шифрования транспорта, NAT traversal или внешнего relay. Устройства должны уже иметь прямой сетевой маршрут друг к другу; игровые UDP и LAN-discovery не пересылаются.

## Доверие и разрешения API v2/v3

Подписанный плагин запускается только после явного закрепления Ed25519-ключа издателя. Для неподписанного пакета пользователь отдельно включает локальный запуск; согласие привязано к SHA-256 конкретной версии. Пакеты по умолчанию выключены. Замена пакета сохраняет grants только для разрешений, которые объявлены и до, и после замены; новые разрешения нужно выдать отдельно.

V2 предоставляет `launcher:servers` для списка серверов, изолированного в namespace конкретного плагина. Доступны `api.launcher.servers.list()`, `save()`, `add()` и `remove()`; хост ограничивает список 200 записями и проверяет permission при каждом вызове. `network:fetch`, `network:dns` и `network:ping` требуют соответствующих grants и точного host из `network_hosts`. Доступ к LAN-комнатам через `network:listen` не предоставляется.

V3 добавляет расширенные host-секции `runtime`, `permissions`, `config.readProjected`, `http`, `instances`, `os` и `proxy`. Уже реализованы следующие операции:

- `api.runtime.getInfo()`, `getCapabilities()`, `getQuotaUsage()`, `assertCapability()`;
- `api.permissions.get()` и `set()`;
- `api.config.readProjected(schemaId)` для схем `launcher-public-config` и `network-diagnostics`;
- `api.http.request(...)` для host-mediated HTTP(S) запросов с allowlist-проверкой домена;
- `api.instances.list()` и `api.instances.getProcessMetrics(instanceName)`;
- `api.os.openUrl(url)`;
- `api.proxy.listRoutes()`, `saveRoutes()`, `addRoute()`, `removeRoute()`.

V3 file scopes расширены до `plugin`, `instance` и `launcher-root-readonly`. Scope `launcher-root-readonly` разрешает только list/read-операции и требует permission `files:launcher-root-read`.

Обычный вызов plugin runtime ограничен пятью секундами. Каждый пользовательский диалог (`alert`, `confirm`, `prompt`, `choose`) может ожидать ответ до пяти минут; это исключение не увеличивает срок остальных вызовов.

### Импорт списков отзыва

Закреплённому ключу можно отдельно выдать полномочие подписывать списки отзыва. Пользователь импортирует JSON-файл вручную; файл ограничен 1 MiB. Документ должен иметь схему v1, подпись Ed25519 назначенным ключом и `sequence`, больше предыдущего импортированного значения. Подписанное содержимое включает оба массива целиком; более новый список заменяет предыдущий набор импортированных отзывов.

```json
{
  "schema_version": 1,
  "sequence": 1,
  "revoked_plugins": [{ "id": "unsafe-plugin", "reason": "Compromised release" }],
  "revoked_publishers": [{ "key_id": "publisher-key", "reason": "Key compromised" }],
  "signature": {
    "algorithm": "ed25519",
    "key_id": "revocation-authority",
    "public_key": "<base64 public key>",
    "value": "<base64 signature>"
  }
}
```

Подпись вычисляется по каноническому UTF-8 JSON без поля `signature` (ключи отсортированы, разделители компактные). Ключ должен быть закреплён, отдельно назначен полномочием и не отозван. Локальные блокировки пользователя хранятся отдельно и не снимаются импортом.

## Публичный каталог GitHub

Community-каталог ищет только публичные репозитории с topic `nivora-launcher-plugin` и предлагает последний подходящий release. Архив, manifest, пути и integrity-подписи проверяются до сохранения. Для подписанного API v2/v3 отдельно проверяется Ed25519-подпись; публикация в GitHub сама по себе не закрепляет издателя, не доказывает provenance и не даёт permissions. Пакет остаётся выключенным до пользовательского решения о доверии и grants.

Поиск и установка публичных архивов не требуют авторизации. Встроенной формы публикации или GitHub OAuth в лаунчере нет.

### Ручная публикация через сайт GitHub

1. Создайте публичный репозиторий и добавьте ему topic `nivora-launcher-plugin`.
2. Создайте обычный опубликованный Release и приложите ZIP-архив.
3. В корне ZIP должен быть `plugin.yml` для статического API v1 либо `plugin.json` для исполняемого API v2/v3, непустой `LICENSE` и указанный в manifest файл `entrypoint`. ZIP ограничен 64 MiB; каталог проверяет архив до сохранения. Runtime entrypoint запускается только после прохождения проверки доверия и выдачи permissions.

После этого репозиторий появится в поиске сообщества без входа в GitHub из лаунчера. Публикация и обновление Release выполняются на сайте GitHub.

## TypeScript API v2/v3

Типы API находятся в `plugins/api.ts`, `plugins/api-v3.ts` и в пакете SDK. Корневой export `@nivora-launcher/plugin-sdk` теперь указывает на v3-контракт; legacy v2-контракт доступен через `@nivora-launcher/plugin-sdk/v2`.

```ts
import type { PluginApi, TrialPlugin } from "@nivora-launcher/plugin-sdk";

const plugin: TrialPlugin = {
  activate(api: PluginApi) {
    return api.events.on("instance.afterExit", ({ instance, exit_code }) => {
      api.notifications.show({
        title: instance.name,
        message: `Game exited with ${exit_code ?? "unknown status"}.`,
      });
    });
  },
};

export default plugin;
```

Соберите TypeScript в один ESM entrypoint без статических `import`/`export` зависимостей и без `require`; перед упаковкой включите зависимости в файл. Runtime не открывает `window`, `document`, Node.js-модули, файловую систему или сеть напрямую. Файлы, хранилище и база данных ограничены namespace плагина; `files:instance` требует отдельного permission.

`api.network.fetchJson(url)` доступен только с `network:fetch`, использует HTTPS, проверяет точный hostname из `network_hosts` и ограничивает JSON-ответ 4 MiB. Сетевой запрос выполняется host-ом, а не напрямую из plugin-кода.

Для сетевых инструментов доступны отдельные permissions:

- `network:dns` — DNS resolve только разрешённых доменов;
- `network:ping` — TCP latency/port probe без запуска shell-команд;
- `launcher:servers` — изолированный список серверов в namespace текущего плагина (`api.launcher.servers`);
- `launcher:dialogs` — пользовательские диалоги и уведомления.

DNS и TCP probe не обходят allowlist: `host` обязан точно присутствовать в `network_hosts`. Server list хранит только `id`, `name`, `address`, `port`, `description` и `tags`; максимум 200 записей.

Список permissions v2 задан типом `PluginPermission` в `plugins/api.ts`; высокорисковые полномочия вроде `files:host-write`, `process:spawn`, `input:capture` и `native:load` не поддерживаются.


## Extended UI, quotas, audit and developer reload

The implementation status, exact limits, executable UI/data-binding example and remaining unsupported services are documented in [plugin-core-extension-status.md](plugin-core-extension-status.md).

Developer mode enables a bounded manifest/entrypoint polling watcher. To explicitly request a runtime replacement in an open desktop launcher:

```sh
nivora-launcher plugin reload <id>
```

Reload preserves signature/integrity checks and user permission grants; it is not implicit approval to execute edited unsigned code. Existing `config_key` notifications remain, while `onChange` names a registered action. `events.listListeners()` reports only the calling plugin's listeners.

---

<!-- English version / Английская версия -->

# Nivora Launcher Plugin API

API v2/v3 (`plugin.json`) runs the entrypoint in a separate QuickJS worker. Before launch, package integrity and publisher trust — or explicit consent to run an unsigned package locally — are verified. Every host call additionally checks the grant of the corresponding permission. API v1 is static: it is described by the same `plugin.json` with `"api_version": 1` and runs no code. The legacy `plugin.yml` is read only for compatibility with old packages.

## `plugin.json` (static, API v1)

```json
{
  "id": "server-notes",
  "name": "Server Notes",
  "version": "1.0.0",
  "api_version": "1",
  "entrypoint": "dist/index.js",
  "description": "Notes scoped to the current instance",
  "permissions": ["storage:workspace", "storage:database", "files:plugin", "ui:compose"]
}
```

`entrypoint` must be a relative `.js` file. Absolute paths and `..` are forbidden. The field is validated, but the file is never executed.

For testing, the sidebar host supports a limited `sidebar_tab` declaration:

```json
{
  "permissions": ["ui:compose"],
  "sidebar_tab": { "label": "Test Tab" }
}
```

The `sidebar_tab` field may contain only a `label` of 1–40 printable characters and requires `ui:compose`. The tab is visible only while the plugin is installed and enabled. The host renders the label as text and shows a static page; no HTML, JavaScript or plugin UI fragments are loaded. Disabling or removing the plugin removes the tab.

### Declarative UI v2 (API v2/v3)

Runtime plugins register `ui.views` whose root is built from safe nodes: containers `form`, `tabs`, `accordion`, `group`, `card` (a block with tone and heading), `grid` (1–4 columns), `include`; controls `input`, `textarea`, `number`, `slider`, `color` (#rrggbb), `datetime`, `select`, `checkbox`, `switch`; display `text`, `badge`, `button`, `progress`, `metric` (a large value), `alert` (a note with tone), `code`, `image` (http/https only, re-checked at render), `list`, `link`, `divider`, `table`, `chart`. The same vocabulary is checked by five mirrors: the types in `plugins/api.ts`, the worker `compose` whitelist, host-side `isUiNode`, the manifest validator and the Vue renderer — extra keys and unknown types are dropped everywhere.

Slots mount natively: `sidebar_tab` becomes a real tab next to static plugin tabs, `home_widget` renders on the "Instances" page, `settings_section` inside settings, `instance_tab` in the plugin panel. Controls with a `config_key` write to the plugin config; `onChange` is a declarative action ID, not JavaScript. Dynamic data is attached via `ui.bindData` (static/event/poll 2–60 s) to list/table/chart/text/checkboxes and control values.

The archive is validated before saving: the launcher caps it at 64 MiB, the unpacked size at 128 MiB and 4096 files; symbolic links and escapes from the staging directory are rejected. The archive is saved disabled. Enabling and disabling only flip a local marker and run no code.

In API v1 the permissions fields remain metadata and do not give plugin code access to the host API. The launcher never loads the manifest as code.

## Built-in LAN rooms

`network:listen` is not part of the API v2/v3 permission set, so a third-party plugin cannot reach the shared LAN-room manager. The built-in "LAN rooms" page keeps working through the launcher's internal interface.

The LAN service accepts participants over the host's direct private IP address and proxies a single Minecraft TCP port for the participant over a local loopback. This is not a Hamachi/Radmin-style virtual network: there is no virtual adapter, arbitrary routing, transport encryption, NAT traversal or external relay. Devices must already have a direct network route to each other; game UDP and LAN discovery are not forwarded.

## Trust and permissions, API v2/v3

A signed plugin runs only after the publisher's Ed25519 key is explicitly pinned. For an unsigned package the user separately enables local execution; the consent is bound to the SHA-256 of that exact version. Packages are disabled by default. Replacing a package keeps grants only for permissions declared both before and after the replacement; new permissions must be granted again.

V2 provides `launcher:servers` for a server list namespaced to the plugin. Available are `api.launcher.servers.list()`, `save()`, `add()` and `remove()`; the host caps the list at 200 entries and checks the permission on every call. `network:fetch`, `network:dns` and `network:ping` require the matching grants and an exact host from `network_hosts`. Access to LAN rooms via `network:listen` is not granted.

V3 adds the extended host sections `runtime`, `permissions`, `config.readProjected`, `http`, `instances`, `os` and `proxy`. Implemented operations:

- `api.runtime.getInfo()`, `getCapabilities()`, `getQuotaUsage()`, `assertCapability()`;
- `api.permissions.get()` and `set()`;
- `api.config.readProjected(schemaId)` for the `launcher-public-config` and `network-diagnostics` schemas;
- `api.http.request(...)` for host-mediated HTTP(S) requests with allowlist checking of the domain;
- `api.instances.list()` and `api.instances.getProcessMetrics(instanceName)`;
- `api.os.openUrl(url)`;
- `api.proxy.listRoutes()`, `saveRoutes()`, `addRoute()`, `removeRoute()`.

V3 file scopes extend to `plugin`, `instance` and `launcher-root-readonly`. The `launcher-root-readonly` scope allows only list/read operations and requires the `files:launcher-root-read` permission.

An ordinary plugin runtime call is limited to five seconds. Each user dialog (`alert`, `confirm`, `prompt`, `choose`) may wait up to five minutes for an answer; this exception does not extend the other calls.

### Revocation list import

A pinned key can be separately granted the authority to sign revocation lists. The user imports a JSON file by hand; the file is capped at 1 MiB. The document must use schema v1, carry an Ed25519 signature by the designated key and a `sequence` greater than the previously imported value. The signed content covers both arrays in full; a newer list replaces the previously imported set of revocations.

```json
{
  "schema_version": 1,
  "sequence": 1,
  "revoked_plugins": [{ "id": "unsafe-plugin", "reason": "Compromised release" }],
  "revoked_publishers": [{ "key_id": "publisher-key", "reason": "Key compromised" }],
  "signature": {
    "algorithm": "ed25519",
    "key_id": "revocation-authority",
    "public_key": "<base64 public key>",
    "value": "<base64 signature>"
  }
}
```

The signature is computed over canonical UTF-8 JSON without the `signature` field (keys sorted, compact separators). The key must be pinned, separately designated by the authority grant, and not revoked. The user's local blocks are stored separately and are not lifted by an import.

## Public GitHub catalog

The community catalog searches only public repositories with the topic `nivora-launcher-plugin` and offers the latest suitable release. The archive, manifest, paths and integrity signatures are verified before saving. For a signed API v2/v3 the Ed25519 signature is verified separately; publishing on GitHub by itself neither pins the publisher, proves provenance, nor grants permissions. The package stays disabled until the user decides on trust and grants.

Searching and installing public archives requires no authorization. There is no built-in publishing form or GitHub OAuth in the launcher.

### Manual publishing via the GitHub website

1. Create a public repository and give it the topic `nivora-launcher-plugin`.
2. Create a regular published Release and attach a ZIP archive.
3. The ZIP root must contain `plugin.yml` for static API v1 or `plugin.json` for executable API v2/v3, a non-empty `LICENSE`, and the `entrypoint` file named in the manifest. The ZIP is capped at 64 MiB; the catalog verifies the archive before saving. A runtime entrypoint runs only after passing the trust checks and receiving permissions.

After that the repository shows up in community search without signing into GitHub from the launcher. Publishing and updating the Release happen on the GitHub website.

## TypeScript API v2/v3

The API types live in `plugins/api.ts`, `plugins/api-v3.ts` and in the SDK package. The root export `@nivora-launcher/plugin-sdk` now points to the v3 contract; the legacy v2 contract is available as `@nivora-launcher/plugin-sdk/v2`.

```ts
import type { PluginApi, TrialPlugin } from "@nivora-launcher/plugin-sdk";

const plugin: TrialPlugin = {
  activate(api: PluginApi) {
    return api.events.on("instance.afterExit", ({ instance, exit_code }) => {
      api.notifications.show({
        title: instance.name,
        message: `Game exited with ${exit_code ?? "unknown status"}.`,
      });
    });
  },
};

export default plugin;
```

Compile the TypeScript into a single ESM entrypoint with no static `import`/`export` dependencies and no `require`; bundle dependencies into the file before packaging. The runtime exposes no `window`, `document`, Node.js modules, filesystem or direct network access. Files, storage and the database are scoped to the plugin namespace; `files:instance` requires a separate permission.

`api.network.fetchJson(url)` is available only with `network:fetch`; it uses HTTPS, checks the exact hostname from `network_hosts` and caps the JSON response at 4 MiB. The network request is performed by the host, not by plugin code directly.

Separate permissions exist for network tools:

- `network:dns` — DNS resolution of allowed domains only;
- `network:ping` — TCP latency/port probe without spawning shell commands;
- `launcher:servers` — a server list namespaced to the current plugin (`api.launcher.servers`);
- `launcher:dialogs` — user dialogs and notifications.

DNS and TCP probes do not bypass the allowlist: the `host` must appear exactly in `network_hosts`. The server list stores only `id`, `name`, `address`, `port`, `description` and `tags`; at most 200 entries.

The v2 permission list is defined by the `PluginPermission` type in `plugins/api.ts`; high-risk authorities such as `files:host-write`, `process:spawn`, `input:capture` and `native:load` are not supported.

## Extended UI, quotas, audit and developer reload

Implementation status, exact limits, an executable UI/data-binding example and the remaining unsupported services are documented in [plugin-core-extension-status.md](plugin-core-extension-status.md).

Developer mode enables a bounded manifest/entrypoint polling watcher. To explicitly request a runtime replacement in an open desktop launcher:

```sh
nivora-launcher plugin reload <id>
```

Reload preserves signature/integrity checks and user permission grants; it is not implicit approval to execute edited unsigned code. Existing `config_key` notifications remain, while `onChange` names a registered action. `events.listListeners()` reports only the calling plugin's listeners.
