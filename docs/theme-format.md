# Формат тем

Пакет темы — ZIP с обязательными корневыми файлами `theme.json` и `theme.yml`. Импортер сохраняет совместимость со схемой `1`; новые темы используют `schema_version: 2`. Размер ZIP не должен превышать 8 MiB, суммарный распакованный размер — 32 MiB, число файлов — 128. `theme.json` ограничен 128 KiB, `theme.yml` — 16 KiB; изображения — 512 KiB, звук — 1 MiB, исходник функции — 256 KiB (всего исходников — 2 MiB).

## Базовые поля

`id` использует kebab-case. Все цвета палитры — шестизначные hexadecimal-значения. Допустимые значения `mode`: `system`, `light`, `dark`, `glass`; `density`: `compact`, `comfortable`, `spacious`; `sidebar_mode`: `expanded`, `adaptive`, `compact`.

Обязательный `theme.yml` содержит метаданные:

```yaml
author: "Имя автора"
description: "Описание темы"
version: "1.0.0"
creators:
  - "Первый создатель"
```

Он принимается в ограниченном YAML-формате: без тегов, anchors, блоков и табуляции. Требуются `author`, `description`, `version` и список от 1 до 16 `creators`.

## Схема v2

Поля палитры сохраняют формат v1. Дополнительно поддерживаются `locale` (`ru` или `en`), `button_customization`, `resources` и декларативный объект `plugin`:

```json
{
  "schema_version": 2,
  "id": "midnight-signal",
  "name": "Midnight Signal",
  "mode": "dark",
  "accent": "#7aa2ff",
  "accent_strong": "#b7c9ff",
  "background": "#080d16",
  "panel": "#101a29",
  "panel_soft": "#182941",
  "density": "comfortable",
  "sidebar_mode": "adaptive",
  "locale": "ru",
  "button_customization": {
    "width": 180,
    "height": 44,
    "fill": true,
    "border": true,
    "font_family": "inter",
    "font_size": 14,
    "outline": { "enabled": true, "width": 1, "color": "#7aa2ff" },
    "texture_fit": "cover",
    "crop_enabled": false,
    "crop": { "x": 0, "y": 0, "width": 100, "height": 100 },
    "labels": {
      "instances": "Экземпляры",
      "accounts": "Аккаунты",
      "mods": "Моды",
      "modpacks": "Сборки",
      "themes": "Темы",
      "settings": "Настройки"
    },
    "overrides": {
      "hero-play": { "label": "Запустить", "width": 220 }
    }
  },
  "resources": {
    "texture": "resources/button-texture.webp",
    "icon": "resources/button-icon.png",
    "sound": "resources/button-click.ogg"
  },
  "plugin": {
    "id": "midnight-effects",
    "name": "Midnight effects",
    "version": "1.0.0",
    "api_version": "3",
    "entrypoint": "functions/index.js",
    "sources": ["functions/index.js"]
  }
}
```

Все поля `button_customization` необязательны. Ограничения: `width` 120–280, `height` 28–64, `font_size` 9–20; `fill`, `border`, `crop_enabled` — boolean; `font_family` — `system`, `inter`, `manrope` или `mono`; `texture_fit` — `cover` или `contain`. `outline` принимает boolean `enabled`, числовой `width` 0–3 и цвет `#rrggbb`. `crop` содержит `x`/`y` в диапазоне 0–100 и `width`/`height` в диапазоне 5–100; прямоугольник должен оставаться внутри 100×100. `labels` содержит только ключи `instances`, `accounts`, `mods`, `modpacks`, `themes`, `settings`; каждая строка ограничена 16 символами.

`overrides` — необязательная карта индивидуальных настроек для `topbar-button`, `card-button`, `hero-play` и `nav-item`. Она принимает те же стилистические поля, что и глобальные настройки, а также `label` (до 16 символов). Отсутствующие поля наследуются из глобальной настройки; отдельный `label` меняет только подпись выбранной цели.

`resources` связывает логические ключи `texture`, `icon`, `sound` и `<target>-texture`, `<target>-icon`, `<target>-sound` с путями ресурсов. Допустимые форматы — растровые PNG/JPEG/WebP (GIF только для `texture`) и OGG/MP3/WAV/WebM для звука; содержимое проверяется на соответствие типу. Каждый объявленный ресурс обязан присутствовать, повторные пути и незаявленные файлы запрещены. Для API сохранения двоичные данные передаются отдельно в `resource_data` (ключ -> base64); base64 никогда не записывается в `theme.json`.

`plugin` — только декларативные метаданные: `id`, `name`, `version`, `api_version: "3"`, `entrypoint` и список `sources`. Пути исходников должны находиться под `functions/`; entrypoint должен входить в `sources`. Разрешены UTF-8 файлы `.js`, `.mjs`, `.json` и `.txt` в заданных пределах. Исходники сохраняются как текст и не исполняются импортером. Объект plugin/manifest не предоставляет разрешений и не запускает код; выполнение возможно только отдельно, в явно согласованной песочнице.

## Структура ZIP и доверие

```text
theme.json
theme.yml
resources/button-texture.webp
resources/button-icon.png
resources/button-click.ogg
functions/index.js
```

Файлы ресурсов и функций необязательны только когда на них нет ссылок. Разрешены лишь `theme.json`, `theme.yml`, заявленные файлы внутри `resources/` и заявленные исходники внутри `functions/`. Каталоги в ZIP не нужны. Абсолютные пути, `..`, точки-компоненты, обратные косые черты, символьные ссылки, специальные файлы, дубликаты, незаявленные файлы и неизвестные пути отклоняются. Архив проверяется до сохранения и экспорта; импортер ничего не извлекает по путям из архива и не исполняет вложенный код.

Схема v1 остаётся доступной для старых архивов: в ней достаточно исходных полей палитры и двух обязательных файлов в корне ZIP; ресурсы и встроенный plugin не обязательны. Встроенные темы получают `source: "builtin"`; локальные импорты и темы GitHub хранятся отдельно от настроек приложения.

Для каталога GitHub репозиторий должен быть публичным и иметь topic `trial-launcher-theme` (идентификатор совместимости прежнего имени лаунчера). Последний стабильный Release должен содержать ZIP; `id` темы должен совпадать с именем репозитория, а версия в `theme.yml` — с тегом Release без необязательного начального `v`. Подпись автора не проверяется; лаунчер вычисляет SHA-256 загруженного архива.

---

<!-- english version / Английская версия -->

# Theme format

A theme package is a ZIP with required root files `theme.json` and `theme.yml`. The importer remains compatible with schema `1`; new themes use `schema_version: 2`. The ZIP is limited to 8 MiB, total uncompressed data to 32 MiB, and the file count to 128. `theme.json` is limited to 128 KiB, `theme.yml` to 16 KiB; images are limited to 512 KiB, audio to 1 MiB, and each function source to 256 KiB (2 MiB total for sources).

## Base fields

`id` uses kebab-case. Palette colors are six-digit hexadecimal values. Allowed `mode` values are `system`, `light`, `dark`, `glass`; `density`: `compact`, `comfortable`, `spacious`; `sidebar_mode`: `expanded`, `adaptive`, `compact`.

The required `theme.yml` contains metadata:

```yaml
author: "Author name"
description: "Theme description"
version: "1.0.0"
creators:
  - "First creator"
```

It uses a restricted YAML subset without tags, anchors, blocks or tabs. `author`, `description`, `version` and a list of 1–16 `creators` are required.

## Schema v2

Palette fields keep the v1 format. Optional additions are `locale` (`ru` or `en`), `button_customization`, `resources` and declarative `plugin`; see the JSON example above.

All `button_customization` fields are optional. Limits: `width` 120–280, `height` 28–64, `font_size` 9–20; `fill`, `border`, `crop_enabled` are booleans; `font_family` is `system`, `inter`, `manrope` or `mono`; `texture_fit` is `cover` or `contain`. `outline` accepts boolean `enabled`, numeric `width` 0–3 and a `#rrggbb` color. `crop` has `x`/`y` in 0–100 and `width`/`height` in 5–100; the rectangle must remain within 100×100. `labels` accepts only `instances`, `accounts`, `mods`, `modpacks`, `themes` and `settings`, each at most 16 characters.

`resources` maps logical keys `texture`, `icon` and `sound` to resource paths. Supported types are raster PNG/JPEG/WebP (GIF only for `texture`) and OGG/MP3/WAV/WebM for sound; file signatures are checked. Every declared resource must exist; duplicate paths and unreferenced files are rejected. The save API accepts binary payloads separately in `resource_data` (key -> base64); base64 is never written into `theme.json`.

`plugin` contains only declarative metadata: `id`, `name`, `version`, `api_version: "3"`, `entrypoint` and a `sources` list. Source paths must stay under `functions/`, and the entrypoint must be listed in `sources`. UTF-8 `.js`, `.mjs`, `.json` and `.txt` files are supported within the stated limits. Sources are stored as text and are never executed by the importer. A plugin/manifest object grants no permissions and does not run code; execution requires separate explicit consent and a sandbox.

## ZIP layout and trust

```text
theme.json
theme.yml
resources/button-texture.webp
resources/button-icon.png
resources/button-click.ogg
functions/index.js
```

Resource and function files are optional only when nothing references them. Only `theme.json`, `theme.yml`, declared files under `resources/` and declared sources under `functions/` are allowed. ZIP directory entries are unnecessary. Absolute paths, `..`, dot components, backslashes, symlinks, special files, duplicates, unreferenced files and unexpected paths are rejected. The archive is validated before saving/export; the importer never extracts using archive-controlled paths or executes bundled code.

Schema v1 remains available for old archives: it needs the original palette fields and the two required root files; assets and embedded plugins are not required. Built-in themes use `source: "builtin"`; local imports and GitHub themes are stored separately from application settings.

For the GitHub catalog, a repository must be public and have the `trial-launcher-theme` topic (a compatibility identifier from the launcher's former name). The latest stable Release must contain the ZIP; the theme `id` must match the repository name, and the `theme.yml` version must match the Release tag without an optional leading `v`. Author signatures are not verified; the launcher computes the downloaded archive's SHA-256.
