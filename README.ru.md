# Nivora Launcher

Настольный лаунчер для Minecraft Java Edition, серверов и Bedrock на поддерживаемых платформах. Интерфейс построен на Vue 3 и Tauri 2, движок приложения — на Rust.

[English](README.md) | **Русский**

## Возможности

- Отдельные экземпляры игры, аккаунты, версии и загрузчики.
- Установка модов, ресурспаков, шейдеров, датапаков и модпаков из настроенных каталогов.
- Создание и управление Minecraft-серверами.
- Игровой оверлей, скриншоты, журнал активности и рабочие области экземпляров.
- Настройка интерфейса и синхронизация выбранных данных через WebDAV.
- Bedrock из официальных источников Microsoft Store на Windows и через BedrockOnLinux на Linux. Для Bedrock на Linux может потребоваться Python 3 или исполняемый файл `bedrock-on-linux`.

Для Bedrock нужна действующая лицензия Microsoft. Nivora не распространяет файлы игры.

## Установка

Используйте пакет для своей платформы: установщик Windows, пакет Linux или образ macOS. Bedrock поддерживается только на Windows и Linux.

## Сборка и тесты

Установите платформенные зависимости Tauri, затем запустите:

```bash
cd frontend
npm ci
npm run tauri dev
```

Проверки фронтенда:

```bash
cd frontend
npm test
npm run build
```

Тесты Rust:

```bash
cd frontend/src-tauri
cargo test --locked
```

Скрипты сборки платформ лежат в `scripts/`; зависимости и цели описаны в [инструкции по сборке](docs/BUILDING.md).

## Документация

- [Модель безопасности](docs/security.md)
- [API плагинов](docs/plugin-api.md)
- [Формат тем](docs/theme-format.md)
- [Серверный движок](docs/server-runtime.md) и [сеть серверов](docs/server-network.md)
- [Синхронизация с облаком](docs/cloud-sync.md)
- [Выпуск релизов](docs/RELEASING.md)

## Лицензия

GPL-3.0. См. [LICENSE](LICENSE).

Minecraft — торговая марка Mojang Studios и Microsoft. Nivora Launcher не связан с ними. BedrockOnLinux — отдельный проект: [Wyze3306/BedrockOnLinux](https://github.com/Wyze3306/BedrockOnLinux).
