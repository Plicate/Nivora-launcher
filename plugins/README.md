# Nivora Launcher Plugin SDK

`@nivora-launcher/plugin-sdk` is a type-only npm package for Nivora Launcher plugins. It contains no runtime, browser shims, Node.js declarations, or host implementation. Import its exports with `import type` while authoring and bundle plugin code into one JavaScript entrypoint.

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

The package root export now targets the v3 contract in `api-v3.ts`. Legacy v2 types remain available through `@nivora-launcher/plugin-sdk/v2`, and the explicit v3 path is `@nivora-launcher/plugin-sdk/v3`.

The runtime `plugin.json` manifest carries metadata, permissions, and declarative capabilities. The entrypoint is a single `.js` file; static ESM imports, re-exports, and literal CommonJS `require()` imports are unsupported. Bundle dependencies before packing.

V2/v3 entrypoints run in QuickJS only after integrity validation, publisher-key trust (or explicit hash-bound local consent for unsigned packages), and user grants for requested permissions. Packages start disabled. A package replacement keeps only grants still requested by both versions; removed grants do not return automatically.

```json
{
  "id": "sample-widget",
  "name": "Sample Widget",
  "version": "1.0.0",
  "api_version": "3",
  "entrypoint": "main.js",
  "permissions": ["runtime:observe", "network:https", "network:proxy-control"],
  "network_hosts": ["example.com"],
  "sandbox_profile": "strict",
  "compatibility_matrix": { "min_launcher_version": "0.1.0" }
}
```

## V3 Host Sections

The current v3 SDK surface includes:

- `api.runtime.getInfo()`, `getCapabilities()`, `getQuotaUsage()`, `assertCapability()`
- `api.permissions.get()` and `set()`; `set()` can only revoke existing grants, while new grants require a user action in Plugin Center
- `api.files.list/readText/writeText/remove()` with scopes `plugin`, `instance`, `launcher-root-readonly`
- `api.config.readProjected(schemaId)`
- `api.http.request(...)` for HTTPS to allowlisted hosts; HTTP and configured proxies are blocked
- `api.instances.list()` and `api.instances.getProcessMetrics(instanceName)`
- `api.os.openUrl(url)`
- `api.proxy.listRoutes()/saveRoutes()/addRoute()/removeRoute()`

## Safe Contract

- Metadata and manifest types cover identity, version, entrypoint, permissions, config schema, network host allowlists, UI views, translations, theme contributions, actions, events, commands, shortcuts, integrity hashes, and Ed25519 signatures.
- UI slots are `sidebar_tab`, `instance_tab`, `settings_section`, and `home_widget`. UI nodes are limited to `text`, `badge`, `button`, `input`, `select`, `checkbox`, `progress`, `list`, `link`, `divider`, and `group`; arbitrary HTML and executable UI are not represented.
- Event names are typed, including `instance.beforeLaunch`, `instance.afterExit`, `instance.crash`, `log.line`, `mods.beforeInstall`, and `snapshot.created`.
- Host capabilities are namespaced: storage and config are plugin-scoped; files are limited to the plugin or the explicitly permitted instance; database access uses a plugin-scoped database; network methods are permission- and host-allowlist-scoped.
- Launcher capabilities include confirmation dialogs, external links, selected-instance metadata, notifications, registered commands and shortcuts, plus a `launcher:servers` list isolated to the plugin namespace (maximum 200 entries).
- Each ordinary runtime request is limited to five seconds; user dialogs may wait up to five minutes. `network:listen` is reserved and not available to runtime plugins.
- Registered commands appear on their plugin card; shortcuts must include Ctrl, Alt, or Meta, do not override launcher shortcuts, and do not fire while editing text. Theme contributions are validated and exposed as plugin-namespaced tokens through `api.theme.get()`.
- The contract does not expose `window`, `document`, browser DOM objects, Node.js modules, process access, or filesystem/network globals. The QuickJS runtime is the security boundary; lint heuristics are advisory only.

Publisher revocation is opt-in. A user must first pin a signing key, then separately authorize it to sign revocation lists. Imports accept bounded JSON documents (1 MiB) signed by that pinned, non-revoked authority with a strictly increasing sequence; each document replaces the imported set, while local user blocks remain separate. See `docs/plugin-api.md` for the document shape and details.

## Authoring Commands

```text
nivora-launcher plugin init ./my-plugin
nivora-launcher plugin lint ./my-plugin
nivora-launcher plugin pack ./my-plugin ./my-plugin.zip [--key ./private.pem]
nivora-launcher plugin verify ./my-plugin.zip
nivora-launcher plugin keygen ./private.pem ./public.pem
```

Unsigned packages include SHA-256 integrity hashes. Signing and verification use the launcher's shared Ed25519 plugin-security implementation. Keep the private PEM outside the package and do not commit it.

---

<!-- русская версия / Russian version -->

# SDK плагинов Nivora Launcher

`@nivora-launcher/plugin-sdk` — npm-пакет только с типами для плагинов Nivora Launcher. В нём нет рантайма, браузерных шимов, деклараций Node.js или host-реализации. Импортируйте его экспорты через `import type` при разработке и собирайте код плагина в один JavaScript-entrypoint.

```ts
import type { PluginApi, TrialPlugin } from "@nivora-launcher/plugin-sdk";

const plugin: TrialPlugin = {
  activate(api: PluginApi) {
    return api.events.on("instance.afterExit", ({ instance, exit_code }) => {
      api.notifications.show({
        title: instance.name,
        message: `Игра завершилась с кодом ${exit_code ?? "unknown status"}.`,
      });
    });
  },
};

export default plugin;
```

Корневой экспорт пакета сейчас указывает на v3-контракт из `api-v3.ts`. Legacy-типы v2 доступны через `@nivora-launcher/plugin-sdk/v2`, явный путь v3 — `@nivora-launcher/plugin-sdk/v3`.

Рантайм-манифест `plugin.json` несёт метаданные, permissions и декларативные возможности. Entrypoint — один `.js`-файл; статические ESM-импорты, реэкспорты и литеральные CommonJS `require()` не поддерживаются. Включайте зависимости в файл до упаковки.

V2/v3-entrypoint выполняются в QuickJS только после проверки целостности, доверия ключу издателя (или явного согласия на локальный запуск unsigned-пакета, привязанного к хешу) и выдачи пользователем запрошенных permissions. Пакеты стартуют выключенными. Замена пакета сохраняет только grants, запрошенные обеими версиями; отозванные grants автоматически не возвращаются.

```json
{
  "id": "sample-widget",
  "name": "Sample Widget",
  "version": "1.0.0",
  "api_version": "3",
  "entrypoint": "main.js",
  "permissions": ["runtime:observe", "network:https", "network:proxy-control"],
  "network_hosts": ["example.com"],
  "sandbox_profile": "strict",
  "compatibility_matrix": { "min_launcher_version": "0.1.0" }
}
```

## Host-секции V3

Текущая поверхность v3 SDK:

- `api.runtime.getInfo()`, `getCapabilities()`, `getQuotaUsage()`, `assertCapability()`
- `api.permissions.get()` и `set()`; `set()` может только отзывать grants, новые выдаются пользователем в менеджере плагинов
- `api.files.list/readText/writeText/remove()` со скоупами `plugin`, `instance`, `launcher-root-readonly`
- `api.config.readProjected(schemaId)`
- `api.http.request(...)` для HTTPS к разрешённым хостам; HTTP и настроенные прокси блокируются
- `api.instances.list()` и `api.instances.getProcessMetrics(instanceName)`
- `api.os.openUrl(url)`
- `api.proxy.listRoutes()/saveRoutes()/addRoute()/removeRoute()`

## Безопасный контракт

- Типы метаданных и манифеста покрывают идентичность, версию, entrypoint, permissions, схемы конфигурации, allowlist сетевых хостов, UI-вью, переводы, темы, действия, события, команды, шорткаты, хеши целостности и Ed25519-подписи.
- UI-слоты: `sidebar_tab`, `instance_tab`, `settings_section` и `home_widget`. UI-узлы ограничены `text`, `badge`, `button`, `input`, `select`, `checkbox`, `progress`, `list`, `link`, `divider` и `group`; произвольный HTML и исполняемый UI не представлены.
- Имена событий типизированы, включая `instance.beforeLaunch`, `instance.afterExit`, `instance.crash`, `log.line`, `mods.beforeInstall` и `snapshot.created`.
- Возможности хоста именсованы: storage и config ограничены плагином; файлы — только плагин или явно разрешённый экземпляр; база данных — отдельная на плагин; сетевые методы ограничены permission и allowlist хостов.
- Возможности лаунчера: диалоги подтверждения, внешние ссылки, metadata выбранного экземпляра, уведомления, зарегистрированные команды и шорткаты, а также список `launcher:servers`, изолированный в namespace плагина (максимум 200 записей).
- Обычный рантайм-запрос ограничен пятью секундами; пользовательские диалоги могут ждать до пяти минут. `network:listen` зарезервирован и рантайм-плагинам недоступен.
- Зарегистрированные команды появляются на карточке плагина; шорткаты обязаны содержать Ctrl, Alt или Meta, не перекрывать шорткаты лаунчера и не срабатывать при редактировании текста. Теми валидируются и отдаются как токены в namespace плагина через `api.theme.get()`.
- Контракт не открывает `window`, `document`, объекты DOM браузера, модули Node.js, доступ к процессам или глобалы файловой системы/сети. Граница безопасности — рантайм QuickJS; эвристики линтера имеют только рекомендательный характер.

Отзыв издателя — opt-in. Пользователь сначала закрепляет ключ подписи, затем отдельно уполномочивает его подписывать списки отзыва. Импорт принимает ограниченные JSON-документы (1 MiB), подписанные этим закреплённым и не отозванным ключом со строго возрастающим `sequence`; каждый документ заменяет импортированный набор, локальные блокировки пользователя хранятся отдельно. Формат документа и подробности — в `docs/plugin-api.md`.

## Команды разработки

```text
nivora-launcher plugin init ./my-plugin
nivora-launcher plugin lint ./my-plugin
nivora-launcher plugin pack ./my-plugin ./my-plugin.zip [--key ./private.pem]
nivora-launcher plugin verify ./my-plugin.zip
nivora-launcher plugin keygen ./private.pem ./public.pem
```

Unsigned-пакеты содержат хеши целостности SHA-256. Подпись и проверка используют общую Ed25519-реализацию plugin-security лаунчера. Держите приватный PEM вне пакета и не коммитьте его.

## Theme effects API (plugin themes)

Plugins with the `theme:effects` permission can restyle the launcher far beyond accent colors. Slot replacement and HUD widgets additionally require `ui:compose`. Every operation is scoped to the calling plugin and cleared automatically when the plugin stops or is disabled (`theme.clearPlugin` on the host side).

```ts
import type { PluginApi, TrialPlugin } from "@nivora-launcher/plugin-sdk";

const plugin: TrialPlugin = {
  activate(api: PluginApi) {
    const theme = api.theme;
    void theme.particles.spawn({ preset: "snow" });            // built-in presets: snow, rain, embers, fireflies, sparks, ...
    void theme.particles.spawn({ shape: "star", colors: ["#ffd166", "#ef476f"], count: 120, layer: "above" });
    void theme.particles.burst({ x: 0.5, y: 0.5, count: 60 });  // radial confetti/firework burst
    void theme.wallpaper.set({ image: wallpaperDataUrl, fit: "cover", blur: 12, dim: 0.3 });
    void theme.cursor.set({ image: cursorDataUrl, size: 48, hotspotX: 4, hotspotY: 4 });
    void theme.effects.set({ vignette: 0.5, scanlines: true, shimmer: true, panelGlow: "#3d8bfd" });
    void theme.style.set("sidebar", { "background-color": "#101828", "border-radius": "12px" });
    void theme.slots.set("sidebar.footer", { type: "card", children: [/* plugin UI nodes */] });
    void theme.hud.add({ type: "metric", label: "FPS", value: 120 }, { position: "top-right", width: 260 });
    void theme.transitions.set("cinematic");
    void theme.sounds.register("click", clickDataUrl);
    void theme.sounds.play("click", { volume: 0.6, rate: 1 });
    void theme.fonts.register("Display", fontDataUrl);
    void theme.tokens.set({ "panel.radius": 14 });
    return { dispose: () => { void theme.particles.stopAll(); void theme.effects.reset(); } };
  },
};
```

What the API covers:

- **Particles** — screen-space emitters behind or above the whole interface: shape (circle, square, star, heart, diamond or a data-URL image), color palettes, speed/direction/spread with variance, sway, gravity, wind, opacity and lifetime, spawn zones (full, top, bottom, left, right, center), blend modes (`lighter` for glow effects), respawn, plus one-shot `burst` fireworks. Built-in presets: `snow`, `rain`, `embers`, `fireflies`, `sparks`.
- **Wallpaper** — replace the window background with an image (cover/contain/tile), with blur and dim levels; runs below all launcher content.
- **Effects overlay** — vignette, film-grain noise, CRT scanlines, panel glow/blur, angled gradient washes, and a shimmer highlight; rendered above content but below dialogs.
- **Style overrides** — limited, safe CSS properties (`background-color`, `color`, `border-radius`, `box-shadow`, opacity, transition and similar allow-listed props) applied per scope: window, sidebar, cards, buttons, inputs, header, scrollbars.
- **Slot replacement** — swap three launcher areas (`sidebar.footer`, `instances.top`, `settings.footer`) with declarative plugin UI nodes; `theme.slots.reset()` restores the original.
- **HUD widgets** — floating plugin-driven widgets pinned to six screen positions with bounded width and margin.
- **Sounds** — register short data-URL audio and play it with volume/rate/loop bounds.
- **Custom cursor** — data-URL cursor image with hotspot and size bounds.
- **Transitions** — interface-wide transition presets: `instant`, `swift`, `smooth`, `cinematic`.
- **Fonts and tokens** — register custom fonts (auto-scoped family names) and set safe theme tokens (`accent`, `panel.radius` and friends) without direct CSS injection.

Safety rails: data-URL size caps per asset type, per-plugin caps (8 particle emitters, 8 HUD widgets, 3 slots, 8 fonts, 32 sounds), global launcher caps across plugins, CSS property and color-value allow-lists, and `theme.slots`/`theme.hud` gated behind the same `ui:compose` consent as UI composition.
