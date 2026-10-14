# WASM plugin isolation roadmap

This document captures the remaining migration work from the current QuickJS-based plugin runtime to a capability-scoped WASM runtime.

## Goals

1. Untrusted plugins must not execute with ambient filesystem or network access.
2. Every privileged action must flow through an explicit launcher capability grant.
3. Existing UI/plugin contracts should stay stable while the runtime changes underneath.

## Target architecture

- **Primary runtime:** Wasmtime or Wasmer embedded in the launcher backend.
- **Guest ABI:** a narrow host-call surface for:
  - UI rendering
  - plugin storage
  - approved network fetches
  - approved file reads/writes inside plugin-scoped storage
  - launcher events (`instance.beforeLaunch`, `instance.afterExit`, `instance.crash`)
- **Capability model:** manifest-declared permissions translated into host-call allowlists.
- **No direct JS eval for untrusted plugins.** QuickJS remains only as a development compatibility bridge until the WASM path reaches parity.

## Execution phases

### Phase 1 — Compatibility layer

- Define a runtime-neutral plugin host interface in the frontend and backend.
- Move plugin event delivery to host-managed RPC messages.
- Keep QuickJS as the current implementation behind the new interface.

### Phase 2 — WASM sandbox MVP

- Add a backend-managed WASM runtime.
- Load plugins compiled to WebAssembly Component Model or a stable ABI wrapper.
- Expose only these host calls:
  - `ui.registerView`
  - `ui.updateView`
  - `storage.get/set/remove`
  - `events.subscribe`
  - `network.fetch` with hostname allowlists
- Deny all ambient filesystem, process, and socket access.

### Phase 3 — Capability grants

- Map manifest permissions to explicit runtime grants.
- Require user approval for:
  - external network domains
  - server-list mutations
  - file export/import outside plugin storage
- Persist granted capabilities separately from the plugin archive.

### Phase 4 — Migration and deprecation

- Mark QuickJS plugins as legacy.
- Offer a validation tool that reports unsupported APIs before publish.
- Remove QuickJS execution for untrusted third-party plugins after parity is reached.

## Security boundaries

- **Network:** no raw sockets from plugins; all HTTP goes through launcher mediation.
- **Filesystem:** plugin sandbox storage only unless the user grants scoped export/import.
- **Process:** plugins cannot spawn child processes.
- **Native code:** plugins cannot load host libraries.

## Release criteria

The migration should be considered complete only when:

1. third-party plugins run in WASM by default,
2. permissions are enforced by host capability checks,
3. crash telemetry can identify the plugin and denied capability,
4. QuickJS is no longer required for production plugins.

---

<!-- Русская версия / Russian version -->

# Дорожная карта изоляции плагинов на WASM

Документ фиксирует оставшуюся работу по переходу от текущего рантайма плагинов на QuickJS к рантайму на WASM с capability-скоупами.

## Цели

1. Недоверенные плагины не должны исполняться с окружным доступом к файловой системе или сети.
2. Каждое привилегированное действие обязано проходить через явный grant возможности лаунчера.
3. Существующие контракты UI/плагинов должны оставаться стабильными, пока рантайм меняется под ними.

## Целевая архитектура

- **Основной рантайм:** Wasmtime или Wasmer, встроенный в бэкенд лаунчера.
- **Гостевой ABI:** узкая поверхность host-вызовов для:
  - рендеринга UI
  - хранилища плагина
  - одобренных сетевых запросов
  - одобренных чтений/записей файлов внутри хранилища плагина
  - событий лаунчера (`instance.beforeLaunch`, `instance.afterExit`, `instance.crash`)
- **Модель возможностей:** permissions из манифеста транслируются в allowlist host-вызовов.
- **Без прямого eval JS для недоверенных плагинов.** QuickJS остаётся только мостом совместимости для разработки, пока WASM-путь не достигнет паритета.

## Фазы исполнения

### Фаза 1 — слой совместимости

- Определить нейтральный к рантайму интерфейс плагин-хоста во фронтенде и бэкенде.
- Перевести доставку событий плагинам на RPC-сообщения под управлением хоста.
- Оставить QuickJS текущей реализацией за новым интерфейсом.

### Фаза 2 — MVP WASM-песочницы

- Добавить управляемый бэкендом WASM-рантайм.
- Загружать плагины, скомпилированные в WebAssembly Component Model или стабильный ABI-wrapper.
- Открыть только эти host-вызовы:
  - `ui.registerView`
  - `ui.updateView`
  - `storage.get/set/remove`
  - `events.subscribe`
  - `network.fetch` с allowlist имён хостов
- Запретить любой окружной доступ к файловой системе, процессам и сокетам.

### Фаза 3 — выдача возможностей

- Отобразить permissions манифеста в явные рантайм-grants.
- Требовать одобрения пользователя для:
  - внешних сетевых доменов
  - изменений списка серверов
  - экспорта/импорта файлов вне хранилища плагина
- Хранить выданные возможности отдельно от архива плагина.

### Фаза 4 — миграция и вывод из эксплуатации

- Пометить QuickJS-плагины как legacy.
- Предложить инструмент валидации, сообщающий о неподдерживаемых API до публикации.
- Убрать исполнение QuickJS для недоверенных сторонних плагинов после достижения паритета.

## Границы безопасности

- **Сеть:** никаких сырых сокетов от плагинов; весь HTTP идёт через посредничество лаунчера.
- **Файловая система:** только sandbox-хранилище плагина, если пользователь не выдал скоуп экспорта/импорта.
- **Процессы:** плагины не могут порождать дочерние процессы.
- **Нативный код:** плагины не могут загружать библиотеки хоста.

## Критерии завершения

Миграцию можно считать законченной, только когда:

1. сторонние плагины по умолчанию работают в WASM,
2. permissions принуждаются проверками возможностей хоста,
3. краш-телеметрия определяет плагин и отклонённую возможность,
4. QuickJS больше не требуется для продакшн-плагинов.
