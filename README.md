<div align="center">

# Nivora Launcher

**Универсальный лаунчер Minecraft: Java, Bedrock и серверы — в одном окне**

[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-blue)]()
[![License: GPL-3.0](https://img.shields.io/badge/license-GPL--3.0-green)]()
[![Tauri 2](https://img.shields.io/badge/Tauri-2-orange)]()
[![Version](https://img.shields.io/badge/version-1.0.0-purple)]()

Современный интерфейс на Vue 3, производительное ядро на Rust, честное
отношение к лицензиям и данным. macOS не поддерживается — без фанатизма.

</div>

---

## О лаунчере

Nivora запускает **Java-версию Minecraft** (vanilla, Fabric, Quilt, Forge,
NeoForge), **серверные ядра** (Paper, Purpur, Pufferfish, Folia, Spigot,
Leaf, Patina) и **Minecraft Bedrock** — на Linux через встроенный
BedrockOnLinux, на Windows через официальный «Minecraft for Windows» из
Microsoft Store. Никаких переупаковок игры: Bedrock всегда ставится из
официальных источников под лицензией вашего аккаунта.

- **Экземпляры вместо одной установки** — изолированные папки, группы, теги,
  иконки, время игры, дублирование, ZIP-экспорт.
- **Каталоги** — моды, ресурспаки, шейдеры, датапаки и модпаки с Modrinth,
  CurseForge, FTB, Technic; серверные плагины с Modrinth, Hangar, Spiget и
  GitHub Releases.
- **Microsoft-аккаунты** — device-code вход, Xbox Live/XSTS, скины и плащи,
  несколько аккаунтов с alias-именами. Пароли не хранятся никогда.
- **Игровой оверлей** (`Shift+Tab`) — моды, время сессии, системные метрики,
  запись видео (`F7`) поверх игры.
- **Рабочее пространство** — live-консоль, снапшоты, галерея скриншотов,
  файловый менеджер, метрики CPU/RAM/диск.
- **Персонализация** — центр тем в стиле Blender/Unity: цвета, формы,
  обои, шрифты, компоновка, анимации; темы из GitHub-каталога.
- **Прочее** — WebDAV-синхронизация данных, HTTP/SOCKS5-прокси, LAN-комнаты,
  профили настроек, ограничения плагинов без исполнения кода, CLI и `doctor`
  для диагностики.

Полный список возможностей — в [docs/FEATURES.ru.md](docs/FEATURES.ru.md).

## Скриншоты

> Поместите скриншоты в `docs/screenshots/` и раскомментируйте:
>
> | Библиотека | Оверлей | Центр тем |
> |:---:|:---:|:---:|
> | ![Библиотека](docs/screenshots/library.png) | ![Оверлей](docs/screenshots/overlay.png) | ![Темы](docs/screenshots/themes.png) |

## Установка

| ОС | Способ |
|---|---|
| **Windows 10/11** | Установщик `.exe` (NSIS). Требуется WebView2 (ставится автоматически). Bedrock — официальный «Minecraft for Windows» из Microsoft Store: лаунчер сам предложит установку через winget или страницу Store. |
| **Debian/Ubuntu** | Пакет `.deb` (`python3` ставится как зависимость). |
| **Fedora/RHEL** | Пакет `.rpm`. |
| **Arch/CachyOS** | Сборка пакета скриптом `scripts/build_arch_package.sh`. |

Готовые пакеты публикуются на странице Releases.

> **Linux + Wayland:** отдельное окно оверлея поверх чужой игры Wayland не
> позволяет — оверлей открывается внутри окна лаунчера (кнопка в верхней
> панели, назначенная клавиша или пункт в трее). На X11 работает поверх игры.

## Сборка из исходников

```bash
git clone <repo> nivora-launcher && cd nivora-launcher

# Windows (из WSL/Linux-окружения с кросс-инструментами или из Windows)
bash scripts/build_windows_exe.sh

# Linux
cd frontend && npm install && npm run tauri build   # .deb/.rpm/AppImage

# Разработка
cd frontend && npm install && npm run tauri dev
```

Для Python-бэкенда нужен Python 3.10+. Проверка перед релизом:

```bash
cargo test --locked                                # Rust-ядро
python3 -m unittest discover -s tests -q           # Python-часть
cd frontend && npm run build                       # vue-tsc + vite
```

## Minecraft Bedrock

- **Windows** — только официальный «Minecraft for Windows» из Microsoft Store.
  Установка через winget с фолбэком на страницу Store, запуск по протоколу
  `minecraft://`, контент — в `com.mojang` пакета.
- **Linux** — встроенный BedrockOnLinux **2.2.9** (SHA-256 закреплён в
  исходниках, целостность проверяется при каждом запуске). Игра загружается
  с CDN Microsoft Store **под вашей лицензией**: аккаунт, который входит в
  Nivora, обязан владеть Minecraft.
- Вход Microsoft **один на все экземпляры** Bedrock (общее хранилище лицензии
  xodus) — повторные регистрации устройства не создаются.
- Без входа игра не стартует: её файлы зашифрованы лицензией аккаунта-владельца.
  Карточка «Лицензия Microsoft Store» в воркспейсе всегда показывает статус.

## Архитектура

```
frontend/          Vue 3 + TypeScript + Tauri 2 (GUI)
  src-tauri/       Rust-ядро: Bedrock, оверлей, загрузки, каталоги, аккаунты
nivora_launcher/   Python-воркер: лёгкие операции и утилиты
scripts/           Сборка пакетов (exe/deb/rpm/arch), диагностика
docs/              Документация по подсистемам
```

Критичное для запуска и производительности живёт в Rust; Python отвечает за
административные функции. Обе части помечены меткой сборки — при рассинхроне
лаунчер показывает предупреждение вместо непредсказуемого поведения.

## Данные и приватность

- Каталоги данных: `nivora-launcher` в `%APPDATA%`/`%LOCALAPPDATA%` (Windows)
  или XDG-каталогах (Linux). Данные старого `trial-launcher` переносятся
  автоматически при первом запуске.
- Учётные данные — в системном хранилище (Credential Manager / keyring);
  пароли Microsoft вводятся на страницах Microsoft и в лаунчер не попадают.
- Телеметрии нет. Все запросы — только к официальным API (Mojang, Microsoft,
  каталоги модов) и вашему прокси/облаку, если вы их настроили.

## Горячие клавиши

| Действие | По умолчанию |
|---|---|
| Игровой оверлей | `Shift+Tab` (настраивается) |
| Запись видео оверлея | `F7` (настраивается) |
| Поиск / Настройки / Обновить | `Ctrl+K` / `Ctrl+,` / `Ctrl+R` |
| Новая activity-панель | `Ctrl+J` |

## Диагностика

- «Диагностика оверлея» в настройках — пошаговый самотест (среда, окно,
  показ, always-on-top, глобальные клавиши).
- Windows: `scripts/collect_crash_report.bat` собирает логи в один файл
  (`%LOCALAPPDATA%\nivora-launcher\`).
- Сбои старта показываются диалогом с текстом ошибки, а не молча.

## Участие

PR приветствуются: описывайте проблему и сценарий, держите тесты зелёными
(`cargo test`, Python-сьют, `npm run build`). Новые crate-зависимости
обсуждаются отдельно. UI-строки — на русском.

## Лицензия

GPL-3.0. См. [LICENSE](LICENSE).

**Minecraft** — торговая марка Mojang Studios / Microsoft. Лаунчер не связан
с ними, не перепродаёт игру и требует купленной копии: Java-аккаунт Microsoft
с лицензией, для Bedrock — аккаунт-владелец «Minecraft for Windows».
BedrockOnLinux — отдельный проект [Wyze3306/BedrockOnLinux](https://github.com/Wyze3306/BedrockOnLinux).
