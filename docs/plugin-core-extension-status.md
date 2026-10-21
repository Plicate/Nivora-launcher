# Plugin core extensions — implementation status

The declarative UI/runtime core is in place. The ten proposed extension areas are not all covered — see the bottom of this page for the gaps.

## Implemented

1. Declarative `form`, `tabs`, `accordion`, `table`, `chart` and guest-resolved `include` nodes. Both package manifest validation and the running worker/host boundary validate the extended vocabulary. The Vue renderer uses native elements/VNodes, never plugin HTML.
2. UI v2 node set: `text`, `badge`, `button`, `input`, `textarea`, `number`, `slider`, `color`, `datetime`, `select`, `checkbox`, `switch`, `progress`, `metric`, `alert`, `code`, `image`, `list`, `link`, `divider`, `group`, `card`, `grid`, `form`, `tabs`, `accordion`, `table`, `chart`, `include`. The five mirrors — `plugins/api.ts`, worker `compose` whitelist, host `isUiNode`, manifest `_validate_v2_ui_node` and the Vue renderer — accept exactly this vocabulary. New nodes obey the same limits: `color` values must be `#rrggbb`, `slider` requires `min <= max`, `grid` takes 1–4 `columns`, `switch`/`metric`/`alert`/`card` accept the standard tones. `form` collects values from `input`, `textarea`, `number`, `slider`, `color`, `datetime`, `select`, `checkbox` and `switch` fields. Image/link URLs are re-checked against the HTTP(S) allowlist at render time.
3. Runtime view slots mount natively: `sidebar_tab` views appear as real launcher tabs next to static plugin tabs, `home_widget` views render on the instances page, `settings_section` views render inside the settings page, and `instance_tab` views remain available through the plugin center panel. All use the same declarative renderer; plugin code still never runs outside QuickJS.
4. `ui.registerFragment`, `ui.updateFragment` and `ui.bindData`. A fragment belongs to one guest runtime. Includes are resolved before crossing the host boundary. Missing references, cycles and excessive expansion are rejected. Updates to a used fragment refresh its views.
5. Static/event/poll loaders run inside QuickJS; JavaScript functions are not serialized over RPC. Dynamic values map to list items, table rows, chart values, text, checkbox/switch state or input/textarea/number/slider/color/datetime/select/progress/metric values. A target ID must be unique. Poll intervals are 2–60 seconds, default 5 seconds; overlapping loads are suppressed. Unsubscription, view removal and runtime termination clean up bindings. Results from removed bindings are ignored.
6. `onChange` is a declarative **action ID string**, not DOM JavaScript. Existing `config_key` change notifications remain. Published changes to control values update the renderer without retaining obsolete local values.
7. `events.listListeners()` returns this guest's event names, listener counts and listener IDs. It cannot inspect another plugin. Canonical event names and generated listener IDs are also recorded in the host audit.
8. Host-call audit plus native plugin-operation audit: time, plugin identity, operation, argument **names**, outcome and duration. Argument values are deliberately omitted; URLs, request bodies, chat content, files, cookies, headers and credentials must not be copied to the audit. Pure local helpers such as `ui.compose` are not host operations. Worker-side validation rejections do not reach the native audit.
9. Trusted runtime limit: 180 logical host calls per rolling minute, 8 MiB request/response data, 32 concurrent host calls. Native backend independently enforces 360 plugin RPCs and 8 MiB request/response data per minute. Audit transport has a separate bounded budget. These layers are enforced, not just reported. Reloading a worker does not clear the native backend budget. Existing storage quotas remain.
10. UI limits: depth 10; 512 logical nodes and 512 aggregate collection entries per view; 256 KiB serialized view; 32 views, 128 fragment registrations, 16 data bindings per guest runtime. Tables have at most 32 columns. QuickJS memory/stack/instruction limits remain. These are protocol-node bounds, not a promise that the number of DOM elements equals 512.
11. `nivora-launcher plugin reload <id>` requests a reload in an open desktop launcher. Developer mode is required. The desktop polls installed plugin manifest/entrypoint metadata every 2 seconds and serializes runtime replacement with ordinary plugin synchronization. This is a bounded **polling file watcher**, not an OS-wide watcher.

## Reload and trust

Reload requests validate the currently installed package. They do not enable a disabled plugin, grant permissions, recompute signatures or silently approve modified unsigned code. Edited packages that fail integrity/trust checks are stopped/rejected. Repack/reinstall and explicitly approve changed unsigned packages through the existing plugin management flow before running them. The watcher examines single-file entry bundles and manifests, not arbitrary imports or recursive folders.

The audit lives below launcher data home at `audit/plugin-operations.jsonl`, with one `plugin-operations.previous.jsonl` rotation after approximately 4 MiB. On POSIX the directory is owner-only and files are mode 0600. Linked/non-regular destinations are rejected. This is a local rotating investigation log, not an externally anchored, tamper-proof ledger.

`runtime.getQuotaUsage()` retains storage telemetry and adds `operations` for native enforcement and `hostCalls` for the running worker's budget. Native RPC counts differ from logical SDK calls because authorization and audit transport are separate requests.

## Example

The manifest must request `ui:compose`; event sources also need an allowed event declared in `events` and the corresponding launcher-event grant. `onChange` names an action registered by the plugin.

```js
export default async (api) => {
  const stops = [];
  stops.push(api.actions.register({ id: 'name-changed', label: 'Name changed' }, async (value) => {
    await api.config.set('name', value);
  }));
  stops.push(api.ui.registerFragment('status', {
    type: 'text', id: 'status-text', text: 'Waiting',
  }));
  stops.push(api.ui.registerView({
    id: 'overview', slot: 'home_widget', title: 'Plugin overview',
    root: { type: 'group', children: [
      { type: 'input', id: 'name', label: 'Name', config_key: 'name', onChange: 'name-changed' },
      { type: 'include', id: 'status-ref', fragment: 'status' },
    ] },
  }));
  stops.push(api.ui.bindData('overview', 'status-text', {
    type: 'poll', intervalMs: 5000,
    loader: async () => String(await api.config.get('name', 'Ready')),
  }));
  return async () => { for (const stop of stops.reverse()) await stop(); };
};
```

## Not implemented

- Proxy session/tunnel transport, per-instance routing grants and plugin-facing tunnel traffic metrics. Existing proxy policy CRUD is not a VPN.
- The additional process/network/account/mod/snapshot/path events and `api.files.watch`. The developer entrypoint watcher above is **not** the requested plugin filesystem API.
- OS file/reveal/executable launch authorization and isolated browser sessions.
- Plugin secret store, general OAuth sessions, WebSocket transport and provider/model-consented AI broker/streaming.

The existing permission names for those areas must not be interpreted as executable capabilities. No success stubs for these modules were added.

## Verification

Guest facade behavior is tested using the actual facade builder in a JavaScript VM. Trusted runtime transport is tested with a synthetic worker. Backend quota/audit/reload/package validation tests run offline. These are not a full QuickJS/WASM, Vue browser rendering or target desktop build validation. The available environment lacks `vue-tsc` and Rust build tools. Live router/provider/account tests were not performed.

---

<!-- Русская версия / Russian version -->

# Расширения ядра плагинов — статус реализации

Декларативное ядро UI/рантайма работает. Десять предложенных направлений расширения закрыты не полностью — список пробелов в конце страницы.

## Реализовано

1. Декларативные узлы `form`, `tabs`, `accordion`, `table`, `chart` и резолвящийся на стороне гостя `include`. Расширенный словарь проверяют и валидация манифеста пакета, и граница worker/host в рантайме. Vue-рендерер использует нативные элементы/VNodes и никогда — HTML плагина.
2. Набор узлов UI v2: `text`, `badge`, `button`, `input`, `textarea`, `number`, `slider`, `color`, `datetime`, `select`, `checkbox`, `switch`, `progress`, `metric`, `alert`, `code`, `image`, `list`, `link`, `divider`, `group`, `card`, `grid`, `form`, `tabs`, `accordion`, `table`, `chart`, `include`. Пять зеркал — `plugins/api.ts`, whitelist `compose` в worker, `isUiNode` на хосте, `_validate_v2_ui_node` манифеста и Vue-рендерер — принимают ровно этот словарь. Новые узлы подчиняются тем же лимитам: значения `color` обязаны быть `#rrggbb`, `slider` требует `min <= max`, `grid` принимает 1–4 `columns`, `switch`/`metric`/`alert`/`card` принимают стандартные тона. `form` собирает значения полей `input`, `textarea`, `number`, `slider`, `color`, `datetime`, `select`, `checkbox` и `switch`. URL карт и ссылок перепроверяются по HTTP(S)-allowlist в момент рендера.
3. Слоты рантайм-вью монтируются нативно: вью `sidebar_tab` появляются как настоящие вкладки лаунчера рядом со статичными вкладками плагинов, вью `home_widget` рендерятся на странице экземпляров, вью `settings_section` — внутри страницы настроек, а вью `instance_tab` остаются доступны через панель центра плагинов. Все используют один декларативный рендерер; код плагина по-прежнему никогда не выполняется вне QuickJS.
4. `ui.registerFragment`, `ui.updateFragment` и `ui.bindData`. Фрагмент принадлежит одному гостевому рантайму. Include-ы резолвятся до пересечения границы хоста. Отсутствующие ссылки, циклы и чрезмерное раскрытие отклоняются. Обновление используемого фрагмента обновляет его вью.
5. Статические загрузчики и загрузчики событий/поллинга выполняются внутри QuickJS; JavaScript-функции через RPC не сериализуются. Динамические значения маппятся на элементы списков, строки таблиц, значения графиков, текст, состояние checkbox/switch или значения input/textarea/number/slider/color/datetime/select/progress/metric. Целевой ID обязан быть уникальным. Интервалы поллинга — 2–60 секунд, по умолчанию 5; пересекающиеся загрузки подавляются. Отписка, удаление вью и завершение рантайма очищают биндинги. Результаты удалённых биндингов игнорируются.
6. `onChange` — декларативная **строка-ID действия**, а не DOM-JavaScript. Существующие уведомления об изменении `config_key` сохранены. Публикуемые изменения значений контролов обновляют рендерер, не удерживая устаревшие локальные значения.
7. `events.listListeners()` возвращает имена событий, счётчики слушателей и их ID только этого гостя; другой плагин инспектировать нельзя. Канонические имена событий и сгенерированные ID слушателей также пишутся в аудит хоста.
8. Аудит host-вызовов плюс нативный аудит плагинных операций: время, идентичность плагина, операция, **имена** аргументов, результат и длительность. Значения аргументов намеренно опущены; URL, тела запросов, содержимое чата, файлы, cookies, заголовки и учётные данные не должны копироваться в аудит. Чисто локальные хелперы вроде `ui.compose` — не host-операции. Отказы валидации на стороне worker до нативного аудита не доходят.
9. Лимиты доверенного рантайма: 180 логических host-вызовов в скользящую минуту, 8 MiB на запрос/ответ, 32 одновременных host-вызова. Нативный бэкенд независимо контролирует 360 plugin RPC и 8 MiB запроса/ответа в минуту. Транспорт аудита имеет отдельный ограниченный бюджет. Эти уровни принуждаются, а не только отчётатся. Перезагрузка worker не сбрасывает бюджет нативного бэкенда. Существующие квоты хранения остаются.
10. Лимиты UI: глубина 10; 512 логических узлов и 512 суммарных элементов коллекций на вью; 256 KiB сериализованного вью; 32 вью, 128 регистраций фрагментов, 16 биндингов данных на гостевой рантайм. Таблицы — не более 32 колонок. Лимиты памяти/стека/инструкций QuickJS остаются. Это границы протокольных узлов, а не обещание, что DOM-элементов будет ровно 512.
11. `nivora-launcher plugin reload <id>` запрашивает перезагрузку в открытом desktop-лаунчере. Требуется режим разработчика. Desktop опрашивает метаданные манифеста/entrypoint установленных плагинов каждые 2 секунды и согласовывает замену рантайма с обычной синхронизацией плагинов. Это ограниченный **поллинг-вотчер файлов**, а не вотчер всей ОС.

## Перезагрузка и доверие

Запрос перезагрузки валидирует текущий установленный пакет. Он не включает выключенный плагин, не выдаёт права, не пересчитывает подписи и не одобряет молча изменённый unsigned-код. Изменённые пакеты, не прошедшие проверки целостности/доверия, останавливаются/отклоняются. Перепакуйте/переустановите и явно одобрите изменённые unsigned-пакеты через существующий процесс управления плагинами перед запуском. Вотчер смотрит однофайловые entry-бандлы и манифесты, а не произвольные импорты и не рекурсивные папки.

Аудит лежит ниже data home лаунчера в `audit/plugin-operations.jsonl`, с одной ротацией в `plugin-operations.previous.jsonl` примерно после 4 MiB. На POSIX каталог доступен только владельцу, файлы имеют режим 0600. Symlink и нестандартные файлы как назначение отклоняются. Это локальный ротируемый журнал для расследований, а не привязанный извне и защищённый от подделки реестр.

`runtime.getQuotaUsage()` сохраняет телеметрию хранения и добавляет `operations` для нативного контроля и `hostCalls` для бюджета работающего worker. Нативные счётчики RPC отличаются от логических вызовов SDK: авторизация и транспорт аудита — отдельные запросы.

## Пример

Манифест обязан запросить `ui:compose`; источникам событий также нужны разрешённое событие из `events` и соответствующий launcher-event grant. `onChange` ссылается на действие, зарегистрированное самим плагином.

```js
export default async (api) => {
  const stops = [];
  stops.push(api.actions.register({ id: 'name-changed', label: 'Name changed' }, async (value) => {
    await api.config.set('name', value);
  }));
  stops.push(api.ui.registerFragment('status', {
    type: 'text', id: 'status-text', text: 'Waiting',
  }));
  stops.push(api.ui.registerView({
    id: 'overview', slot: 'home_widget', title: 'Plugin overview',
    root: { type: 'group', children: [
      { type: 'input', id: 'name', label: 'Name', config_key: 'name', onChange: 'name-changed' },
      { type: 'include', id: 'status-ref', fragment: 'status' },
    ] },
  }));
  stops.push(api.ui.bindData('overview', 'status-text', {
    type: 'poll', intervalMs: 5000,
    loader: async () => String(await api.config.get('name', 'Ready')),
  }));
  return async () => { for (const stop of stops.reverse()) await stop(); };
};
```

## Не реализовано

- Транспорт прокси-сессий/туннелей, per-instance routing grants и метрики туннельного трафика для плагинов. Существующий CRUD proxy-политик — не VPN.
- Дополнительные события process/network/account/mod/snapshot/path и `api.files.watch`. Описанный выше девелоперский вотчер entrypoint — **не** запрошенный плагинный API файловой системы.
- Авторизация запуска файлов/показа/исполнения в ОС и изолированные сессии браузера.
- Хранилище секретов плагинов, общие OAuth-сессии, WebSocket-транспорт и согласованный с провайдерами и моделями AI-брокер/стриминг.

Существующие имена разрешений для этих областей нельзя трактовать как исполняемые возможности. Успешных заглушек для этих модулей не добавлено.

## Проверка

Поведение гостевого фасада тестируется реальным билдером фасада в JavaScript-VM, транспорт доверенного рантайма — синтетическим worker, квоты/аудит/перезагрузка/валидация пакетов — офлайн-тестами бэкенда. Полной валидации QuickJS/WASM, рендеринга Vue в браузере и целевой desktop-сборки эти тесты не заменяют: живые прогоны роутера/провайдеров/аккаунтов нужно делать вручную на реальной сборке.
