# Nivora Launcher frontend

The desktop UI is a Vue 3 + TypeScript application packaged with Tauri 2. The
Python launcher remains the source of truth for installs, accounts, metadata,
mods and game launch. Tauri invokes the Python service through a small JSON
request/response boundary. Windows, Debian and Fedora builds bundle the
service as a self-contained executable; the Arch package also bundles it and
keeps the installed Python module as a fallback.

## Development

```bash
cd frontend
npm install
npm run dev
```

For the desktop shell, generate its platform icons once and then use
`npm run desktop:dev`. During development the application expects the Python
package to be available from the project root.

```bash
npm run tauri -- icon src/assets/uni.png --output src-tauri/icons
```

## Build

```bash
npm run desktop:build
```

Packaged desktop builds require the backend resource to be staged at
`src-tauri/resources/backend/nivora-launcher-backend[.exe]` before Tauri runs.
The CI workflow builds this with PyInstaller for Windows and Linux. The Arch
PKGBUILD stages a small wrapper that starts the backend with system Python.

The Vite public directory points at `cpp-ui/assets`, so the existing official
Fabric, Forge, NeoForge and Quilt marks are bundled into the frontend without
an additional network dependency at runtime.

---

<!-- Русская версия / Russian version -->

# Фронтенд Nivora Launcher

Desktop-интерфейс — приложение Vue 3 + TypeScript в обёртке Tauri 2. Источником истины для установки, аккаунтов, metadata, модов и запуска игры остаётся Python-лаунчер. Tauri обращается к Python-сервису через узкую JSON-границу «запрос/ответ». Сборки Windows, Debian и Fedora упаковывают сервис как самодостаточный исполняемый файл; Arch-пакет тоже его упаковывает и дополнительно держит установленный Python-модуль как fallback.

## Разработка

```bash
cd frontend
npm install
npm run dev
```

Для desktop-оболочки один раз сгенерируйте платформенные иконки, затем используйте `npm run desktop:dev`. Во время разработки приложение ожидает доступности Python-пакета из корня проекта.

```bash
npm run tauri -- icon src/assets/uni.png --output src-tauri/icons
```

## Сборка

```bash
npm run desktop:build
```

Пакетируемые desktop-сборки требуют, чтобы ресурс бэкенда лежал в `src-tauri/resources/backend/nivora-launcher-backend[.exe]` до запуска Tauri. CI-воркфлоу собирает его PyInstaller'ом для Windows и Linux. Arch-PKGBUILD кладёт небольшой wrapper, запускающий бэкенд системным Python.

Публичный каталог Vite указывает на `cpp-ui/assets`, поэтому существующие официальные марки Fabric, Forge, NeoForge и Quilt попадают во фронтенд без дополнительной сетевой зависимости в рантайме.
