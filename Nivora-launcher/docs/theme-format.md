# Формат тем

Тема Nivora Launcher версии `1` является переносимым ZIP-пакетом настроек интерфейса. Она не содержит JavaScript, HTML, CSS, сетевых разрешений или игровых файлов. В корне ZIP должны находиться ровно два файла: `theme.json` и `theme.yml`.

Минимальный объект:

```json
{
  "schema_version": 1,
  "id": "midnight-signal",
  "name": "Midnight Signal",
  "author": "Community",
  "description": "Темная тема с синим акцентом.",
  "mode": "dark",
  "accent": "#7aa2ff",
  "accent_strong": "#b7c9ff",
  "background": "#080d16",
  "panel": "#101a29",
  "panel_soft": "#182941",
  "density": "comfortable",
  "sidebar_mode": "adaptive"
}
```

`id` использует kebab-case. Все цвета должны быть шестизначными hexadecimal-значениями. Допустимые значения `mode`: `system`, `light`, `dark`, `glass`; `density`: `compact`, `comfortable`, `spacious`; `sidebar_mode`: `expanded`, `adaptive`, `compact`.

Обязательный `theme.yml` содержит метаданные темы:

```yaml
author: "Имя автора"
description: "Описание темы"
version: "1.0.0"
creators:
  - "Первый создатель"
  - "Второй создатель"
```

`theme.yml` принимается в ограниченном YAML-формате без тегов, anchors и блоков. Тема проверяется до сохранения; размер ZIP ограничен 8 MiB. Все значения ограничены по длине, CSS и исполняемый код не выполняются.

Для GitHub-каталога репозиторий должен быть публичным и иметь topic `trial-launcher-theme` (постоянный идентификатор совместимости, унаследованный от прежнего имени лаунчера). Последний стабильный Release должен содержать ZIP темы; `id` в `theme.json` обязан совпадать с именем репозитория, а версия в `theme.yml` — с тегом Release без необязательного начального `v`. Подпись автора не проверяется; лаунчер вычисляет SHA-256 загруженного архива.

Встроенные темы получают `source: "builtin"`; локальные импорты и темы из GitHub хранятся в пользовательском каталоге данных отдельно от настроек приложения.

---

<!-- English version / Английская версия -->

# Theme format

A Nivora Launcher theme, version `1`, is a portable ZIP package of interface settings. It contains no JavaScript, HTML, CSS, network permissions or game files. The ZIP root must contain exactly two files: `theme.json` and `theme.yml`.

Minimal object:

```json
{
  "schema_version": 1,
  "id": "midnight-signal",
  "name": "Midnight Signal",
  "author": "Community",
  "description": "A dark theme with a blue accent.",
  "mode": "dark",
  "accent": "#7aa2ff",
  "accent_strong": "#b7c9ff",
  "background": "#080d16",
  "panel": "#101a29",
  "panel_soft": "#182941",
  "density": "comfortable",
  "sidebar_mode": "adaptive"
}
```

`id` uses kebab-case. All colors must be six-digit hexadecimal values. Allowed `mode` values: `system`, `light`, `dark`, `glass`; `density`: `compact`, `comfortable`, `spacious`; `sidebar_mode`: `expanded`, `adaptive`, `compact`.

The required `theme.yml` carries the theme metadata:

```yaml
author: "Author name"
description: "Theme description"
version: "1.0.0"
creators:
  - "First creator"
  - "Second creator"
```

`theme.yml` is accepted in a restricted YAML format without tags, anchors or blocks. The theme is validated before saving; the ZIP size is limited to 8 MiB. All values are length-limited; CSS and executable code are never executed.

For the GitHub catalog the repository must be public and carry the topic `trial-launcher-theme` (a permanent compatibility identifier inherited from the launcher’s earlier name). The latest stable Release must contain the theme ZIP; the `id` in `theme.json` must match the repository name, and the version in `theme.yml` must match the Release tag without the optional leading `v`. The author signature is not verified; the launcher computes the SHA-256 of the downloaded archive.

Built-in themes get `source: "builtin"`; local imports and GitHub themes live in the user data directory, separate from application settings.
