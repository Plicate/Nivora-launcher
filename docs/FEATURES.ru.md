# Nivora Launcher

Лаунчер Minecraft с современным Tauri/Vue-интерфейсом и CLI для диагностики. Desktop-пакеты собираются для Windows 10/11, Debian/Ubuntu и Fedora; исходный пакет CachyOS/Arch сохраняется.

## Что уже есть

- vanilla Minecraft из официального Mojang Piston manifest;
- версии из официального Mojang manifest от `1.0` и выше с современными и legacy arguments;
- библиотеки, client JAR, assets, logging config и Linux natives;
- SHA-1/SHA-512/size проверка и атомарные загрузки;
- автоматический Mojang Java runtime по `javaVersion.component`;
- Fabric и Quilt через официальные launcher profiles;
- Forge и NeoForge через официальные installer JAR;
- Microsoft device-code OAuth, Xbox Live, XSTS и Minecraft Services profile/ownership (нужен собственный Azure public-client ID);
- бессрочный offline debug-аккаунт для локального тестирования;
- поиск и установка модов, ресурспаков, шейдеров и датапаков с Modrinth;
- сортировка каталогов и карточки подробностей; проверка обновлений ресурсов и серверных плагинов с выбором версии перед заменой;
- каталоги серверных плагинов Modrinth, Hangar, Spigot и совместимых API с проверкой версий ядра и обязательных зависимостей;
- отдельная страница LAN-комнат: пароль, список участников и TCP-прокси одного игрового порта для устройств, уже доступных друг другу по локальной сети; это не VPN и не соединяет разные сети;
- поиск плагинов из публичных GitHub releases по теме `nivora-launcher-plugin`; архив проверяется и сохраняется выключенным, `entrypoint` не запускается;
- включённые плагины могут добавлять ограниченную статичную sidebar-вкладку через manifest `sidebar_tab.label`; код плагина по-прежнему не исполняется;
- отдельные instances, открытие папки игры, память JVM, логи и команда `doctor`;
- Minecraft Bedrock на всех ОС: Linux — профили через встроенный BedrockOnLinux 2.2.9 (игра загружается из Microsoft Store под лицензией пользователя), Windows — ОФИЦИАЛЬНЫЙ «Minecraft for Windows» из Microsoft Store (winget-автоустановка с фолбэком на страницу Store, запуск по протоколу `minecraft://`, контент — прямо в `com.mojang` пакета); вся логика Bedrock работает в Rust-движке без Python-воркера;
- клиентский workspace с главной панелью и метриками, быстрым запуском миров/серверов, live-консолью и архивами логов, снапшотами ресурсов, галереей скриншотов и файловым менеджером; текстовые файлы до 2 MiB можно редактировать после остановки игры, загрузка ограничена 32 MiB на файл, предпросмотр изображений — 8 MiB;
- Tauri/Vue/TypeScript GUI с sidebar-страницами, выбором версии/loader/аккаунта, установкой, запуском, модами, скинами и activity-панелью;
- выбор реальных версий Fabric, Quilt, Forge и NeoForge для выбранной версии Minecraft;
- управление экземплярами через CLI и GUI: список, дублирование, удаление, ZIP-экспорт и shortcut для desktop-систем;
- пользовательские профили экземпляров: название, описание, иконка, banner и накопленное время игры; группы доступны для клиентов и серверов с фильтрами, быстрым переносом и закреплением до трёх групп; удаляются только пустые группы;
- локальные alias-имена аккаунтов, порядок аккаунтов и PNG-библиотека скинов;
- установка skin/model, управление capes Microsoft и встроенная панель скинов внутри профиля аккаунта;
- включение и отключение `.jar`-модов без удаления файлов, автопереход в консоль после аварийного запуска;
- server workspaces с официальными каталогами версий и загрузкой ядер Spigot, Paper, Purpur, Folia, Pufferfish, Patina и Leaf;
- гибкая персонализация GUI в отдельной вкладке «Центр настройки тем» в стиле Blender/Unity: слева — разделы (тема, цвета, формы, обои, навигация, движение, страницы), в центре — интерактивный вьюпорт с живым предпросмотром, повторяющим реальный макет лаунчера (страницы, вкладки фильтров, строки экземпляров; клик по элементу открывает редактор этого элемента, смена страницы проигрывает её анимацию), справа — контекстный инспектор; system/light/dark/glass темы, логотипы для светлого и тёмного фона, обои с opacity/blur, accent color, полный редактор color tokens, закруглённость панелей/кнопок/полей ввода (применяется ко всему интерфейсу), шрифты, sidebar слева/справа/сверху/снизу, compact mode, анимации, плотность, масштаб и per-page layout/theme;
- настраиваемые сторонние источники: login/catalog/data provider и API URLs;
- локальный набор Lucide SVG-иконок и bundled официальные marks Fabric, Forge, NeoForge и Quilt для offline UI;
- резервная синхронизация launcher data через WebDAV с подписанным локальным манифестом, лимитами и подтверждением восстановления;
- шаблон `PKGBUILD` для CachyOS/Arch и CI-сборка Windows, `.deb` и `.rpm` пакетов;
- профили настроек лаунчера: сохранение текущих настроек под именем, применение, перезапись, удаление, экспорт/импорт JSON-файлом;
- HTTP/SOCKS5-прокси для загрузок лаунчера; SOCKS5 реализован на чистом stdlib с remote-DNS (`socks5h`);
- JVM-настройки на экземпляр: свой `java_path`, память, свои JVM/game arguments и пресеты аргументов — независимо от launch-профиля;
- плановые бэкапы миров: глобальное расписание и переопределение на экземпляр (интервал, число копий, локально или на WebDAV), список бэкапов с восстановлением и удалением;
- теги и заметки экземпляров: теги отображаются в списке, фильтрация по тегу одним кликом, заметки видны прямо в строке экземпляра;
- краш-отчёты: анализатор лога после аварийного выхода игры и сервера называет вероятную причину, даёт советы и предлагает удалить конкретные моды, если лог указывает на них;
- дерево зависимостей модов: локальный разбор `fabric.mod.json`/`mods.toml`/manifest JAR показывает связи модов и помечает, что сломается при удалении выбранного мода;
- экспорт экземпляра как modpack `.mrpack` (Modrinth-формат): ненулевые профили, каталогизированные моды и overrides;
- системные метрики лаунчера в стиле Afterburner: CPU по ядрам, RAM, GPU, диски и температуры — в топбаре и в оверлее.

## Безопасность запуска

В Linux новые экземпляры и серверы запускаются через bubblewrap strict profile. На Windows этот sandbox недоступен: `strict` завершается явной ошибкой, а профиль `manual` предупреждает, что изоляция ОС не предоставляется. Manual/off на Linux также дают меньше изоляции и требуют осознанного выбора в настройках.

Offline-аккаунт предназначен только для локального дебага и не имеет ограничения по времени. Он не даёт доступа к официальным online-серверам, Realms или обходу покупки Minecraft.

### Microsoft client ID

Nivora Launcher должен использовать собственное Azure public-client приложение проекта с device-code flow. Его публичный ID должен входить в сборку лаунчера; конечным пользователям не нужно вводить его вручную. Нельзя повторно использовать ID Mojang или другого лаунчера.

Только для локальной разработки после регистрации приложения проекта передайте ID до запуска:

```bash
export NIVORA_LAUNCHER_CLIENT_ID="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
```

Распространяемые сборки должны содержать ID в `DEFAULT_MSA_CLIENT_ID`. Если он не настроен, Microsoft-вход недоступен; остаются локальные/offline профили.

Нестандартные login/data endpoint доступны только после включения режима разработчика и не должны использоваться с рабочими токенами.

Microsoft access и refresh tokens не записываются в `accounts.json`: в Linux они хранятся в Secret Service через `secret-tool` (нужен запущенный `gnome-keyring`), а в Windows — в Windows Credential Manager.

## Быстрый старт из исходников

Для CLI нужен Python 3.10 или новее. Для desktop GUI при сборке пакета нужны `base-devel`, Node.js/npm и Rust; Qt6 остаётся fallback для сборок без Tauri toolchain.

Windows `.exe`/`.msi`, Debian `.deb` и Fedora `.rpm` собираются CI и доступны как workflow artifacts в GitHub Actions. Эти артефакты пока не являются подписанными или опубликованными GitHub Releases.

```bash
cd nivora-launcher
python3 -m nivora_launcher doctor
```

Для сборки desktop GUI используйте Arch-пакет из раздела ниже. Если `npm` и `cargo` доступны, собирается Tauri/Vue frontend; без них используется Qt6 fallback. После установки GUI открывается командой `nivora-launcher`. CLI-команды доступны отдельно:

```bash
python3 -m nivora_launcher doctor
python3 -m nivora_launcher account debug Tester
python3 -m nivora_launcher versions --releases-only --limit 20
```

Показать настройки:

```bash
python3 -m nivora_launcher config show
```

Операции с экземплярами:

```bash
python3 -m nivora_launcher instances list
python3 -m nivora_launcher instances duplicate vanilla-1.20.1 copy
python3 -m nivora_launcher instances export copy ~/Desktop/copy.zip
python3 -m nivora_launcher instances shortcut copy
```

Установить vanilla-версию без скачивания asset objects:

```bash
python3 -m nivora_launcher install 1.20.1 --no-assets
```

Перед первым реальным запуском лучше не использовать `--no-assets`:

```bash
python3 -m nivora_launcher install 1.20.1
python3 -m nivora_launcher launch 1.20.1 --account offline
```

## Графика в Linux

В режиме `auto` лаунчер опрашивает `nvidia-smi`; если команда видит видеокарту NVIDIA, включается PRIME offload. Иначе выбор драйвера остаётся системным. Лог запуска показывает выбранный режим, GLFW backend и включён ли PRIME offload. «Проверить GLX» выполняет `glxinfo -B` с тем же окружением. Это подтверждает выбранный маршрут, но не доказывает, что драйвер создал OpenGL-контекст.

Для современного Minecraft на Wayland в режиме NVIDIA native Wayland GLFW v1.1.0 запускается сразу, до GLX-попытки. Библиотека загружается в cache выбранного экземпляра только после проверки размера и закреплённого SHA-256; если она недоступна, лаунчер сообщает причину и пробует обычный GLX. В native-режиме включаются `GLFW_FORCE_WAYLAND=1`, Sodium workaround и PRIME offload; strict sandbox открывает `/sys` только для чтения, чтобы драйвер мог обнаружить GPU. Если NVIDIA-режим сообщает Mesa/llvmpipe или не создаёт контекст, запуск завершается вместо незаметного переключения на программный рендеринг. В остальных графических режимах сохраняется прежний fallback. Библиотека — сторонний prebuilt release, она не включена в архив проекта.

Сухой запуск печатает сформированную Java-команду, но не запускает игру:

```bash
python3 -m nivora_launcher launch 1.20.1 --account offline --dry-run --no-assets
```

## Microsoft-вход

Microsoft device-code OAuth в исходном/разработческом запуске требует ID проекта в `NIVORA_LAUNCHER_CLIENT_ID`. Готовая сборка должна предоставлять его автоматически и не запрашивать у пользователей:

```bash
python3 -m nivora_launcher login microsoft
```

Поток использует системный браузер и device code. Основной OAuth endpoint закреплён за Microsoft; совместимый endpoint можно изменить только в режиме разработчика. Секретные ключи и токены в frontend не встраиваются.

## Minecraft Bedrock (Rust-движок, все ОС)

Вся логика Bedrock живёт в Rust-движке (`frontend/src-tauri/src/rust_engine/bedrock.rs`) и обслуживается нативно, без Python-воркера: статус среды, выбор и установка версий, вход в Microsoft Store, запуск игры, импорт/удаление контента и фоновые задания с отменой. Python остался только лёгким слоем путей для вкладки «Файлы» и метрик (`nivora_launcher/bedrock.py`).

**Linux** — Bedrock-профили отделены от Java-экземпляров. Сборки Nivora содержат официальный BedrockOnLinux 2.2.9 (MIT, SHA-256 закреплён); вручную устанавливать его отдельно не требуется — при первом использовании он сам загружает игру и компоненты Wine/GDK-Proton из официальных источников (если окружение не успело доустановиться, Nivora завершает setup сам перед запуском). Nivora не включает игровые файлы и не обходит проверку лицензии: подключите Microsoft Store-аккаунт, которому принадлежит Minecraft для Windows. Этот вход отдельный от Java device-code OAuth; пароль и токены Microsoft не проходят через Nivora (строки с секретами вырезаются из журналов). Редакция (release/preview) и версия выбираются прямо в диалоге создания экземпляра и устанавливаются сразу после создания профиля (preview-редакции передаются с флагом `--beta`); центр экземпляра устроен как у Java-версий — вкладки «Обзор», «Версия», «Контент» и «Файлы». В workspace можно импортировать `.mcpack`, `.mcaddon`, `.mcworld`, `.mctemplate` и `.mcskin` и управлять установленным контентом. См. `nivora_launcher/vendor/LICENSE-BedrockOnLinux.txt`.

**Windows** — только ОФИЦИАЛЬНЫЙ «Minecraft for Windows» из Microsoft Store: BedrockOnLinux и Wine не используются. Установка пробует winget (`msstore`) и, если продукт лицензируется только самим Store (платные/Xbox-игры), открывает официальную страницу продукта (`ms-windowsstore://pdp/?ProductId=9NBLGGH2JHXJ`, preview — `9P5X4QVLC2XR`) — все зависимости устанавливает Store. Запуск идёт по протоколу `minecraft://` (preview — `minecraft-preview://`) с фолбэком на `shell:AppsFolder`; вход в аккаунт Microsoft/Xbox выполняется в самой игре, лаунчер ничего не хранит. Контент импортируется напрямую в `com.mojang` пакета (`%LOCALAPPDATA%\Packages\<семейство>\LocalState\games\com.mojang`).

**macOS и другие ОС** — официальный Bedrock не выпускается: Nivora честно сообщает об этом и не предлагает обходных путей.

Системные уведомления об ошибках отправляются только когда окно лаунчера свёрнуто или не в фокусе: BedrockOnLinux не вызывает `notify-send` напрямую (его уведомления перенаправляются в журнал активности), а сам лаунчер дублирует сбой в системное уведомление через ту же `notify-send`-цепочку только для свёрнутого окна.

## Loaders

```bash
python3 -m nivora_launcher install 1.20.1 --loader fabric
python3 -m nivora_launcher launch 1.20.1 --loader quilt --account offline
python3 -m nivora_launcher install 1.20.1 --loader forge
python3 -m nivora_launcher install 1.21.1 --loader neoforge
```

Конкретную версию loader можно передать через `--loader-version`. Для Forge/NeoForge installer загружается только с официального Maven host, проверяется по доступному SHA-512/SHA-256 (legacy fallback: SHA-1), запускается внутри strict bubblewrap с сетью игры и записью только в instance, а результат проверяется в `instance/versions/`.

## Моды

Modrinth работает без ключа:

```bash
python3 -m nivora_launcher mods search modrinth sodium --game-version 1.20.1 --loader fabric
python3 -m nivora_launcher mods install modrinth sodium --instance fabric-loader-0.15.11-1.20.1 --game-version 1.20.1 --loader fabric
```

Датапаки устанавливаются в выбранный мир через GUI или CLI. Для GUI выберите
экземпляр, тип «Датапаки», проект Modrinth и затем целевой мир. Мод можно
временно отключить во вкладке «Моды», не удаляя его из экземпляра.

Сомнительные моды и плагины можно проверить перед запуском: кнопка щита во
вкладке «Моды» (или в плагинах сервера) декомпилирует `.jar`, покажет оценку
времени и запустит статический анализ по 22 категориям (кейлоггеры, окна,
файлы, сеть, запуск команд и другие). Декомпилированные исходники уничтожаются
без проверки при отказе и уничтожаются после неё; код плагина не исполняется.
Подробнее: `docs/security.md`.

```bash
python3 -m nivora_launcher audit ~/Downloads/SuspiciousMod.jar
```

Версии загрузчика для выбранной версии Minecraft:

```bash
python3 -m nivora_launcher loaders choices fabric 1.20.1
python3 -m nivora_launcher loaders choices forge 1.20.1
```

Локальные скины и Microsoft profile:

```bash
python3 -m nivora_launcher skins import ~/Pictures/skin.png --model slim
python3 -m nivora_launcher skins list
python3 -m nivora_launcher skins profile microsoft
python3 -m nivora_launcher skins apply microsoft SKIN_ID --model slim
python3 -m nivora_launcher skins cape-apply microsoft CAPE_ID
python3 -m nivora_launcher skins cape-clear microsoft
```

## Пути

По умолчанию используются XDG-каталоги:

- настройки: `~/.config/nivora-launcher/`;
- instances и логи: `~/.local/share/nivora-launcher/`;
- загрузки и кэш: `~/.cache/nivora-launcher/`.

Конфигурация не меняет существующий `~/.minecraft`. Путь можно изменить:

```bash
python3 -m nivora_launcher config set game_dir /path/to/nivora-launcher-game
python3 -m nivora_launcher open-folder
```

## Облачная синхронизация

В разделе «Настройки» можно подключить WebDAV endpoint Nextcloud, ownCloud, Yandex Disk или совместимого сервиса. В облако выгружаются только разрешённые данные; плагины, серверные служебные JSON, `.trial-cell` и `accounts.json` не синхронизируются.

Пароль или app token хранится в libsecret и не возвращается в интерфейс. Перед восстановлением показывается список изменений; файлы проверяются по локально аутентифицированному SHA-256 манифесту. Ключ манифеста хранится только на этом устройстве, поэтому восстановление на другом устройстве пока не поддерживается. Подробности находятся в `docs/cloud-sync.md`.

## Главный экран и запуск сервера

На Dashboard сервера расположены консоль с отправкой команд, CPU/RAM и отдельные графики RX/TX игрового TCP-порта, время работы и кнопка рестарта. Ошибки и последние строки консоли сохраняются после остановки. Рестарт ждёт завершения и освобождения блокировки перед новым запуском. На Linux трафик измеряется по kernel-счётчикам сокетов серверного процесса (inet-diag), на Windows — через TCP ESTATS `GetPerTcpConnectionEstats` без прав администратора; в обоих случаях это трафик только этого сервера, а не всей сетевой карты, и неподдерживаемые платформы/ограничения доступа показываются явно.

Вкладка **Доступ и свойства** — структурные редакторы `whitelist.json`, `ops.json` и `server.properties`: пока сервер работает, изменения применяются живыми командами консоли (`whitelist add/remove/reload`, `op`/`deop`), на остановленном — правкой файлов с сохранением порядка ключей и комментариев. В настройках сервера настраивается ежедневное расписание перезапуска с `say`-предупреждением за настраиваемое число минут. При аварийном завершении сервера на Dashboard появляется баннер краш-отчёта: вероятная причина, советы и, если лог указывает на конкретные моды, кнопки их удаления.

Подробно: [Запуск, консоль и метрики](docs/server-runtime.md).

## Сервер в интернете · белый IPv4

В серверном workspace откройте **Управление → Сеть и белый IP**: подготовка `server.properties`, диагностика внешнего IPv4 / WAN / CGNAT, временный UPnP-проброс игрового TCP-порта, адрес для друзей и инструкции по ручному пробросу и firewall. Публикация требует подтверждения; чужие правила не заменяются. UPnP-правило продлевается только во время работы сервера, снимается при остановке и имеет конечную аренду до 10 минут. Без белого IPv4 UPnP не обходит NAT провайдера. Внешнюю доступность проверяйте с другого интернет-канала.

Подробно: [Публикация сервера](docs/server-network.md).

## LAN-комнаты

Откройте отдельную страницу «LAN-комнаты» в боковом меню. Устройства должны находиться в одной локальной сети. В режиме «auto» комната слушает все интерфейсы (`0.0.0.0`), а участникам передаётся конкретный LAN IPv4 этого компьютера; если у машины несколько сетевых adapters, на странице комнаты показываются запасные адреса `host:порт`, которые можно ввести в Minecraft вручную. Хост передаёт друзьям LAN-адрес и порт комнаты; пароль передаётся отдельно. Страница помогает сверить состав участников, но не запускает Minecraft-сервер и не пересылает игровой трафик. Для игры сам сервер Minecraft должен быть доступен по показанному LAN-адресу и порту; при необходимости разрешите порты в локальном firewall.

## Игровой оверлей и запись

Пока экземпляр запущен, лаунчер держит глобальные клавиши и прозрачное окно поверх игры:

- **Shift+Tab** — открыть/закрыть оверлей (Esc тоже закрывает). Сверху по центру — часы и счётчики сессии/всего наигранного времени (этот блок можно скрыть в настройках, но не передвинуть). Слева — виджет модов с поиском (Java-экземпляры), справа — виджет метрик процесса CPU/RAM/диск с пиковыми значениями и панель записи.
- **Системные метрики в стиле MSI Afterburner** — отдельная настраиваемая панель: суммарная загрузка CPU с полосками по каждому ядру, RAM и swap, каждая GPU (загрузка, видеопамять, температура), разделы дисков и температуры CPU/платы. Источники — psutil и `nvidia-smi`; чего нет в системе, то просто не показывается.
- **Быстрый чат сервера** — для серверных экземпляров: хвост консоли под оверлеем и поле отправки команд (`say Привет!`, `/whitelist add Steve` …) без сворачивания игры.
- **F7** — начать/остановить запись экрана. Запись идёт внешним `ffmpeg`: захват через x11grab, кодирование аппаратное — NVENC на NVIDIA, VAAPI на Intel/AMD, иначе быстрый пресет libx264. Файлы кладутся в `<каталог игры>/recordings`, лог последнего запуска — рядом (`last-recording.log`). Звук пишется через PulseAudio, если включён в настройках.
- Настройки — в разделе «Настройки → Игровой оверлей и запись»: включение оверлея, клавиши, виджеты, кодировщик (авто/NVENC/VAAPI/x264), контейнер (MP4/MKV), FPS, качество (CRF/CQ), запись звука, а также вид системных метрик: позиция (четыре угла), масштаб, прозрачность подложки, компактный режим, часы и состав блоков метрик (CPU/RAM/GPU/диски/температуры/чат) — каждый отдельно. В топбаре лаунчера живут компактные системные метрики, обновляющиеся каждые 2 секунды.

Клавиши регистрируются только на время игровой сессии (Java и Bedrock) и освобождаются после выхода из игры; запись при этом корректно останавливается. Оверлей работает в X11/XWayland-сессиях: глобальные клавиши и захват экрана используют X11. Если клавиша перехвачена другой программой, замените комбинацию в настройках. В эксклюзивном полноэкранном режиме игра прячет все окна поверх — переключите её в оконный режим (F11 в Minecraft), чтобы оверлей был виден.

## Каталог плагинов GitHub

В каталоге можно искать публичные GitHub Releases из репозиториев с topic `nivora-launcher-plugin` или импортировать ZIP с устройства. Перед сохранением launcher проверяет manifest и архив, вычисляет SHA-256 и оставляет плагин выключенным. Эта проверка не является подписью релиза: просмотрите источник и разрешения самостоятельно. Сторонний код пока не запускается.

Публикация из лаунчера не поддерживается. Чтобы добавить плагин в каталог сообщества, создайте публичный репозиторий с topic `nivora-launcher-plugin` и приложите ZIP к GitHub Release; требования описаны в `docs/plugin-api.md`.

## Arch/CachyOS package

При наличии `base-devel`, `nodejs`, `npm`, `rust`, `qt6-base`, `qt6-svg`, `python`, `python-defusedxml`, `python-psutil`, `libsecret`, `gnome-keyring`, `libappindicator` и `makepkg`:

```bash
./scripts/build_arch_package.sh &&
sudo pacman -U "$(ls -1t dist/arch/nivora-launcher-*.pkg.tar.zst | head -n 1)"
```

Скрипт создаёт в изолированном временном каталоге архив `nivora-launcher-<pkgver>.tar.gz` с корневой папкой `nivora-launcher`, вычисляет и проверяет его SHA-256, затем запускает `makepkg`. Arch-пакет запускает Python backend через системный Python, поэтому PyInstaller из AUR не нужен. Пакет сохраняется в `dist/arch/`; после успешной сборки скрипт печатает точный путь. Установка запускается только если сборка завершилась успешно. Скрипт не устанавливает системные зависимости. Сборке Tauri нужен доступ к сети для загрузки npm-зависимостей.

Создание сервера не заменяет существующий экземпляр: выберите другое имя либо отдельно сохраните и удалите старый сервер перед повторным созданием. Это предотвращает потерю мира, если загрузка ядра завершится ошибкой.

Для Arch/CachyOS сборка Tauri использует локальный профиль GCC/BFD из `scripts/arch_native_build.py`: нативный C/C++ LTO и Rust linker-plugin LTO отключаются, включая target-specific переменные компилятора. Это предотвращает использование несовместимых LTO-объектов при упаковке нативных библиотек в Rust rlib. Системные `makepkg.conf` и Cargo-настройки не изменяются. После изменения профиля запускайте `scripts/build_arch_package.sh` заново: он создаёт чистое дерево сборки без старого `target/`.

Arch-профиль собирает только бинарник Tauri с `--no-bundle` и `tauri.arch.conf.json`. DEB/RPM/AppImage и подписанные updater-артефакты здесь не создаются: итоговый `.pkg.tar.zst` формирует `makepkg`, поэтому `TAURI_SIGNING_PRIVATE_KEY` для локальной Arch-сборки не требуется. Публичный ключ и проверка подписей обновлений не отключаются; настройки подписанных релизов других платформ остаются прежними.

По умолчанию пакет запускает графический интерфейс:

```bash
nivora-launcher
```

Для диагностики используйте CLI-команды:

```bash
nivora-launcher doctor
```

## Windows .exe (отдельный скрипт)

Сборка exe полностью автоматизирована и не связана с Arch-пакетом:

```bash
./scripts/build_windows_exe.sh              # бэкенд + фронтенд + установщик NSIS
./scripts/build_windows_exe.sh --only-backend  # только nivora-launcher-backend.exe
```

Один прогон делает всё: PyInstaller собирает Python backend в `nivora-launcher-backend.exe` (точка входа `scripts/backend_entry.py`: без аргументов — JSON-воркер Tauri, с `--cli` — CLI; пакетируется вместе с `vendor/*.pyz`), exe кладётся в `frontend/src-tauri/resources/backend/`, где его ждёт `tauri.windows.conf.json`, затем собирается фронтенд и `tauri build` создаёт NSIS-установщик `*-setup.exe` и автономный `nivora-launcher.exe`. Артефакты копируются в `dist/windows/`, скрипт печатает точные пути. Если `TAURI_SIGNING_PRIVATE_KEY` не задан, подписанные updater-артефакты для локальной сборки отключаются.

**Portable-запуск без установки — один файл.** `nivora-launcher.exe` уже содержит Python-бэкенд внутри себя: при первом запуске бэкенд распаковывается в `%LOCALAPPDATA%\nivora-launcher\backend\` и запускается оттуда. Достаточно одного `nivora-launcher.exe` — ничего докладывать рядом не нужно. Если рядом с exe всё же лежит `nivora-launcher-backend.exe` (или в подпапке `backend/`) — используется он; NSIS-установщик тоже продолжает собираться для тех, кому нужна классическая установка в систему. Бэкенд работает в windowed-режиме (`--noconsole`): окна консоли не появляются ни при запуске лаунчера, ни при записи (ffmpeg), ни при `--cli`-запуске игры. Системный Python на Windows не вызывается вовсе — на чистой системе это заглушка Store («Python was not found»).

**Почему wine на Linux.** PyInstaller не умеет кросс-компиляцию: exe бэкенда может собрать только Windows-версия Python, и на Linux она запускается под wine. Rust/Tauri-оболочка кросс-компилируется нативно и wine не использует — wine нужен ровно для одного шага. Настраивать его руками не нужно: если Windows Python не найден, скрипт сам скачивает официальный установщик с python.org и молча (`/quiet`) ставит его в изолированный префикс `.build-windows/wineprefix` — `~/.wine` не затрагивается, повторные запуски переиспользуют готовую установку. Зависимости проекта (`.[windows]`) и PyInstaller скрипт тоже ставит сам в этот Python. Вариант без wine вообще — запускать скрипт на Windows (Git Bash/MSYS2): там используется нативный Python.

Где запускать: на Windows (Git Bash/MSYS2) нужны Python 3.10+, Node.js/npm и Rust (msvc); на Linux — только `wine` и `curl` (для бэкенда; установке Python под wine нужен работающий `DISPLAY` или `xvfb-run`). Всё остальное скрипт ставит сам: `rustup` с таргетом `x86_64-pc-windows-msvc` и `cargo-xwin` — в `$HOME` (системный Rust не затрагивается), системные пакеты `clang`/`lld`/`llvm`/`binutils` — через pacman/apt (нужен root/sudo). `tauri build` запускает cargo через `--runner cargo-xwin`: обёртка подставляет clang-cl, lld-link и скачивает Windows SDK, поэтому msvc-таргет линкуется без Windows. Отдельный случай — NSIS-установщик: tauri-bundler вызывает системный `makensis`, а в Arch пакета `nsis` в официальных репозиториях нет (только AUR), поэтому скрипт скачивает готовый `makensis` из Debian-пакета в `.build-windows/nsis/` (зависимости — базовые glibc/libstdc++/zlib) и подкладывает данные симлинком `/usr/share/nsis`; Stubs/плагины tauri докачивает сам. Шаблон установщика tauri подключает `Win\RestartManager.nsh` — файл из NSIS 3.10+, которого нет ни в Debian stable, ни в пакете 3.08, поэтому скрипт докидывает и его из `nsis-common` 3.11. Сборка идёт с `--bundles nsis` (MSI/WiX на Linux-хосте не собирается). Первая кросс-сборка долгая — качается Windows SDK и компилируются все Rust-зависимости.

## Linux .deb / .rpm и сборка всего сразу

Скрипты устроены так же, как виндовый: бэкенд PyInstaller (onefile, с keyring внутри), фронтенд, `tauri build --bundles deb|rpm`:

```bash
./scripts/build_deb.sh            # Debian/Ubuntu → dist/deb/*.deb
./scripts/build_rpm.sh            # Fedora/RHEL/openSUSE → dist/rpm/*.rpm
./scripts/build_all.sh            # всё: windows + arch + deb + rpm
./scripts/build_all.sh deb rpm    # только выбранные цели
```

Системные зависимости: `build-essential`/`gcc-c++`, `pkg-config`, `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`, `libssl-dev`, `python3-venv`, Node.js/npm и Rust — скрипты проверяют их и печатают точную команду установки, которой не хватает. Кодовая подпись опциональна: `DEB_SIGN_KEY=<gpg-id>` (dpkg-sig) для deb и `RPM_SIGN_KEY=<gpg-id>` (rpmsign) для rpm — GPG-ключ должен быть в локальной связке. Без ключа пакет собирается, но менеджер пакетов предупредит о неподписанном источнике.

## Windows предупреждает при запуске

Установщик и exe лаунчера **не подписаны кодовой подписью Authenticode**, поэтому при первом запуске Windows SmartScreen может показать синее окно «Система защитила ваш компьютер», а браузер при скачивании — «файл нечасто скачивается». Это стандартное поведение для любых неподписанных файлов (у файла просто нет «репутации»), а не признак вредоносного кода: предупреждение одинаково показывается и на самодельных сборках, и на индии-проектах без сертификата.

Что делать пользователю:

1. Нажать **«Подробнее»** → **«Выполнить в любом случае»** (Run anyway). Для установщика — то же самое в окне UAC («Издатель: неизвестен»).
2. При желании сверить файл: рядом с exe публикуется `sha256sums.txt`; проверка — `sha256sum -c sha256sums.txt` (или `certutil -hashfile файл SHA256` на Windows).
3. Если конкретный антивирус (не SmartScreen) сообщил именно «троян» — это почти наверняка ложное срабатывание эвристики на PyInstaller-onefile; проверить файл на virustotal.com и, если детект один-два движка из семидесяти, спокойно отправить отчёт о ложном срабатывании: https://www.microsoft.com/wdsi/filesubmission.

Меры, уже встроенные в сборку: метаданные версии/издателя в exe (`--version-file`, `bundle.publisher`), контрольные суммы и опциональная подпись — если задать `WINDOWS_CERT_FILE` (PKCS#12, например Certum Open Source Code Signing) и при необходимости `WINDOWS_CERT_PASSWORD`, скрипт подпишет все exe через `osslsigncode` (с таймстампом), и SmartScreen перестанет показывать «неизвестный издатель».

## Следующие задачи

1. Протестировать vanilla на чистом CachyOS с Java 8/17/21/25.
2. Проверить installer matrix для поддерживаемых Forge/NeoForge версий.
3. Реализовать безопасное восстановление WebDAV-копии на другом устройстве и подписывать release-артефакты.
4. Расширить plugin sandbox: больше host-сервисов (сеть с scopes, фоновые задачи) и визуальный редактор UI-узлов.
5. Проверить Tauri/Vue UI и реальный запуск на чистом CachyOS, включая GLX/software fallback.

## Лицензия

Nivora Launcher распространяется под GNU General Public License version 3. Текст лицензии находится в `LICENSE`.


### Plugin core extensions

Declarative forms/tabs/accordion/tables/charts, guest-owned fragments and data bindings, enforced call/data quotas, redacted host audit, listener diagnostics and developer runtime reload are implemented. See [implementation status](docs/plugin-core-extension-status.md) for limits and examples. This update does not implement the proposed VPN sessions, new filesystem/process callbacks, OS executable/browser sessions or secret/OAuth/WebSocket/AI services; existing permission names do not establish implementation.

---

<!-- English version / Английская версия -->

# Nivora Launcher

A Minecraft launcher with a modern Tauri/Vue interface and a diagnostics CLI. Desktop packages are built for Windows 10/11, Debian/Ubuntu and Fedora; the CachyOS/Arch source package is kept.

## What is already there

- vanilla Minecraft from the official Mojang Piston manifest;
- versions from the official Mojang manifest from `1.0` upward with modern and legacy arguments;
- libraries, the client JAR, assets, logging config and Linux natives;
- SHA-1/SHA-512/size verification and atomic downloads;
- the automatic Mojang Java runtime per `javaVersion.component`;
- Fabric and Quilt through official launcher profiles;
- Forge and NeoForge through the official installer JARs;
- Microsoft device-code OAuth, Xbox Live, XSTS and Minecraft Services profile/ownership (requires the project's own Azure public-client ID);
- a permanent offline debug account for local testing;
- search and install of mods, resource packs, shaders and data packs from Modrinth;
- catalog sorting and detail cards; resource and server-plugin update checks with version choice before replacing;
- Modrinth, Hangar, Spigot and compatible-API server plugin catalogs with core version checks and required dependencies;
- a separate LAN rooms page: password, participant list and a TCP proxy for the single game port for devices already reachable on the same LAN; this is not a VPN and does not join different networks;
- plugin search over public GitHub releases with the topic `nivora-launcher-plugin`; the archive is verified and saved disabled, the `entrypoint` is never executed;
- enabled plugins may add a limited static sidebar tab via the manifest's `sidebar_tab.label`; plugin code still never runs;
- separate instances, opening the game folder, JVM memory, logs and the `doctor` command;
- Minecraft Bedrock on every OS: Linux — profiles via the bundled BedrockOnLinux 2.2.9 (the game is downloaded from the Microsoft Store under the user's license), Windows — the OFFICIAL "Minecraft for Windows" from the Microsoft Store (winget auto-install with a Store-page fallback, `minecraft://` protocol launch, content lands straight in the package's `com.mojang`); all Bedrock logic runs in the Rust engine with no Python worker;
- a client workspace with a home panel and metrics, quick world/server launch, a live console and log archives, resource snapshots, a screenshot gallery and a file manager; text files up to 2 MiB are editable after the game stops, uploads are capped at 32 MiB per file and image previews at 8 MiB;
- a Tauri/Vue/TypeScript GUI with sidebar pages, version/loader/account selection, install, launch, mods, skins and an activity panel;
- selecting real Fabric, Quilt, Forge and NeoForge versions for the chosen Minecraft version;
- instance management via CLI and GUI: list, duplicate, delete, ZIP export and a desktop shortcut;
- user instance profiles: name, description, icon, banner and accumulated play time; groups work for clients and servers with filters, quick moving and pinning up to three groups; only empty groups can be deleted;
- local account aliases, account ordering and a PNG skin library;
- skin/model assignment, Microsoft cape management and a built-in skins panel inside the account profile;
- enabling and disabling `.jar` mods without deleting files; automatic jump to the console after a crash launch;
- server workspaces with official version catalogs and downloads of the Spigot, Paper, Purpur, Folia, Pufferfish, Patina and Leaf cores;
- deep GUI personalization in a separate "Theme center" tab in the Blender/Unity spirit: sections on the left (theme, colors, shapes, wallpaper, navigation, motion, pages), an interactive viewport with a live preview replicating the real launcher layout in the middle (pages, filter tabs, instance rows; clicking an element opens its editor, switching pages plays that page's animation), and a contextual inspector on the right; system/light/dark/glass themes, logos for light and dark backgrounds, wallpaper with opacity/blur, accent color, a full color-token editor, panel/button/input corner radii (applied to the whole interface), fonts, sidebar left/right/top/bottom, compact mode, animations, density, scale and per-page layout/theme;
- configurable third-party sources: login/catalog/data providers and API URLs;
- a local set of Lucide SVG icons and the bundled official Fabric, Forge, NeoForge and Quilt marks for the offline UI;
- backup sync of launcher data over WebDAV with a signed local manifest, limits and restore confirmation;
- a `PKGBUILD` template for CachyOS/Arch and CI builds of Windows, `.deb` and `.rpm` packages;
- launcher settings profiles: save the current settings under a name, apply, overwrite, delete, export/import as a JSON file;
- an HTTP/SOCKS5 proxy for launcher downloads; SOCKS5 is implemented in pure stdlib with remote DNS (`socks5h`);
- per-instance JVM settings: custom `java_path`, memory, custom JVM/game arguments and argument presets — independent of the launch profile;
- scheduled world backups: a global schedule and per-instance overrides (interval, copy count, local or WebDAV), a backup list with restore and delete;
- instance tags and notes: tags show in the list, one-click filtering by tag, notes visible right in the instance row;
- crash reports: after a game or server crash a log analyzer names the likely cause, gives advice and offers to remove specific mods when the log points at them;
- a mod dependency tree: local parsing of `fabric.mod.json`/`mods.toml`/JAR manifests shows mod relationships and marks what breaks when the selected mod is removed;
- instance export as a `.mrpack` modpack (the Modrinth format): non-default profiles, cataloged mods and overrides;
- Afterburner-style system metrics in the launcher: per-core CPU, RAM, GPU, disks and temperatures — in the top bar and in the overlay.

## Launch security

On Linux, new instances and servers launch through the bubblewrap strict profile. On Windows this sandbox is unavailable: `strict` fails with an explicit error, and the `manual` profile warns that OS isolation is not provided. Manual/off on Linux also provide less isolation and require a deliberate choice in settings.

The offline account is intended for local debugging only and has no time limit. It does not grant access to official online servers, Realms or any bypass of the Minecraft purchase.

### Microsoft client ID

Nivora Launcher must use the project's own Azure public-client application with the device-code flow. Its public ID must be built into the launcher; end users must not enter it manually. Reusing the ID of Mojang or another launcher is forbidden.

For local development only, after registering the project's application, provide the ID before launching:

```bash
export NIVORA_LAUNCHER_CLIENT_ID="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
```

Distributed builds must contain the ID in `DEFAULT_MSA_CLIENT_ID`. When it is not configured, Microsoft sign-in is unavailable; local/offline profiles remain.

Non-default login/data endpoints are available only with developer mode enabled and must not be used with production tokens.

Microsoft access and refresh tokens are never written to `accounts.json`: on Linux they live in the Secret Service via `secret-tool` (a running `gnome-keyring` is required), on Windows — in the Windows Credential Manager.

## Quick start from sources

The CLI needs Python 3.10 or newer. Building the desktop GUI package requires `base-devel`, Node.js/npm and Rust; Qt6 remains the fallback for builds without the Tauri toolchain.

Windows `.exe`/`.msi`, Debian `.deb` and Fedora `.rpm` are built by CI and published as workflow artifacts in GitHub Actions. These artifacts are not yet signed or published GitHub Releases.

```bash
cd nivora-launcher
python3 -m nivora_launcher doctor
```

For the desktop GUI use the Arch package from the section below. When `npm` and `cargo` are available, the Tauri/Vue frontend is built; without them the Qt6 fallback is used. After installation the GUI opens with the `nivora-launcher` command. CLI commands work separately:

```bash
python3 -m nivora_launcher doctor
python3 -m nivora_launcher account debug Tester
python3 -m nivora_launcher versions --releases-only --limit 20
```

Show settings:

```bash
python3 -m nivora_launcher config show
```

Instance operations:

```bash
python3 -m nivora_launcher instances list
python3 -m nivora_launcher instances duplicate vanilla-1.20.1 copy
python3 -m nivora_launcher instances export copy ~/Desktop/copy.zip
python3 -m nivora_launcher instances shortcut copy
```

Install a vanilla version without downloading asset objects:

```bash
python3 -m nivora_launcher install 1.20.1 --no-assets
```

Avoid `--no-assets` before the first real launch:

```bash
python3 -m nivora_launcher install 1.20.1
python3 -m nivora_launcher launch 1.20.1 --account offline
```

## Graphics on Linux

In `auto` mode the launcher polls `nvidia-smi`; when the command sees an NVIDIA card, PRIME offload is enabled. Otherwise driver selection stays with the system. The launch log shows the chosen mode, the GLFW backend and whether PRIME offload is on. "Check GLX" runs `glxinfo -B` with the same environment. This confirms the chosen route but does not prove the driver created an OpenGL context.

For modern Minecraft on Wayland in NVIDIA native Wayland mode, GLFW v1.1.0 launches immediately, before the GLX attempt. The library is placed into the chosen instance's cache only after a size check and a pinned SHA-256; when unavailable, the launcher reports the reason and tries plain GLX. In native mode `GLFW_FORCE_WAYLAND=1`, the Sodium workaround and PRIME offload are enabled; the strict sandbox opens `/sys` read-only so the driver can detect the GPU. If the NVIDIA mode reports Mesa/llvmpipe or fails to create a context, the launch aborts instead of silently switching to software rendering. Other graphics modes keep the previous fallback. The library is a third-party prebuilt release and is not included in the project archive.

A dry run prints the composed Java command without starting the game:

```bash
python3 -m nivora_launcher launch 1.20.1 --account offline --dry-run --no-assets
```

## Microsoft sign-in

Microsoft device-code OAuth from a source/development run requires the project ID in `NIVORA_LAUNCHER_CLIENT_ID`. A finished build must provide it automatically and never ask users:

```bash
python3 -m nivora_launcher login microsoft
```

The flow uses the system browser and a device code. The primary OAuth endpoint is pinned to Microsoft; a compatible endpoint can be changed only in developer mode. No secrets or tokens are embedded in the frontend.

## Minecraft Bedrock (Rust engine, all OSes)

All Bedrock logic lives in the Rust engine (`frontend/src-tauri/src/rust_engine/bedrock.rs`) and is served natively, without the Python worker: runtime status, version selection and setup, Microsoft Store sign-in, game launch, content import/removal and cancellable background jobs. Python keeps only a light path-helper layer for the "Files" tab and metrics (`nivora_launcher/bedrock.py`).

**Linux** — Bedrock profiles are separate from Java instances. Nivora bundles the official BedrockOnLinux 2.2.9 (MIT, SHA-256 pinned); installing it manually is not required — on first use it downloads the game and the Wine/GDK-Proton components from official sources by itself (if the environment did not finish installing, Nivora completes the setup right before launch). Nivora includes no game files and does not bypass license checks: sign in with the Microsoft Store account that owns Minecraft for Windows. This sign-in is separate from the Java device-code OAuth; Microsoft passwords and tokens never pass through Nivora (secret-bearing lines are redacted from the journals). The Bedrock edition (release/preview) and version are chosen right in the instance-creation dialog and installed immediately after the profile is created (preview editions pass the `--beta` flag as BedrockOnLinux requires); the instance hub mirrors the Java layout — "Overview", "Version", "Content" and "Files" tabs. The workspace imports `.mcpack`, `.mcaddon`, `.mcworld`, `.mctemplate` and `.mcskin`, and manages installed content. See `nivora_launcher/vendor/LICENSE-BedrockOnLinux.txt`.

**Windows** — only the OFFICIAL "Minecraft for Windows" from the Microsoft Store: BedrockOnLinux and Wine are not used. Setup first tries winget (`msstore`) and, for products that only the Store itself can license (paid/Xbox games), opens the official product page (`ms-windowsstore://pdp/?ProductId=9NBLGGH2JHXJ`, preview — `9P5X4QVLC2XR`) — the Store installs every dependency. The game launches via the `minecraft://` protocol (preview — `minecraft-preview://`) with a `shell:AppsFolder` fallback; Microsoft/Xbox sign-in happens inside the game and the launcher stores nothing. Content is imported straight into the package's `com.mojang` (`%LOCALAPPDATA%\Packages\<family>\LocalState\games\com.mojang`).

**macOS and other OSes** — no official Bedrock exists there: Nivora says so honestly and offers no workarounds.

System error notifications fire only when the launcher window is minimized or unfocused: BedrockOnLinux does not call `notify-send` directly (its notifications are redirected into the activity journal), while the launcher itself mirrors a failure into a system notification through the same `notify-send` chain only for a minimized window.

## Loaders

```bash
python3 -m nivora_launcher install 1.20.1 --loader fabric
python3 -m nivora_launcher launch 1.20.1 --loader quilt --account offline
python3 -m nivora_launcher install 1.20.1 --loader forge
python3 -m nivora_launcher install 1.21.1 --loader neoforge
```

A specific loader version is passed via `--loader-version`. For Forge/NeoForge the installer is downloaded only from the official Maven host, verified by the available SHA-512/SHA-256 (legacy fallback: SHA-1), run inside the strict bubblewrap with game network and writes limited to the instance, and the result is verified in `instance/versions/`.

## Mods

Modrinth works without a key:

```bash
python3 -m nivora_launcher mods search modrinth sodium --game-version 1.20.1 --loader fabric
python3 -m nivora_launcher mods install modrinth sodium --instance fabric-loader-0.15.11-1.20.1 --game-version 1.20.1 --loader fabric
```

Data packs install into a chosen world via the GUI or CLI. In the GUI pick the instance, the "Data packs" type, a Modrinth project and then the target world. A mod can be temporarily disabled in the "Mods" tab without removing it from the instance.

Questionable mods and plugins can be checked before launch: the shield button in the "Mods" tab (or among server plugins) decompiles the `.jar`, shows a time estimate and runs a static analysis across 22 categories (keyloggers, windows, files, network, command execution and more). The decompiled sources are destroyed unchecked when you decline and destroyed after the check either way; plugin code never executes. Details: `docs/security.md`.

```bash
python3 -m nivora_launcher audit ~/Downloads/SuspiciousMod.jar
```

Loader versions for the chosen Minecraft version:

```bash
python3 -m nivora_launcher loaders choices fabric 1.20.1
python3 -m nivora_launcher loaders choices forge 1.20.1
```

Local skins and the Microsoft profile:

```bash
python3 -m nivora_launcher skins import ~/Pictures/skin.png --model slim
python3 -m nivora_launcher skins list
python3 -m nivora_launcher skins profile microsoft
python3 -m nivora_launcher skins apply microsoft SKIN_ID --model slim
python3 -m nivora_launcher skins cape-apply microsoft CAPE_ID
python3 -m nivora_launcher skins cape-clear microsoft
```

## Paths

XDG directories are used by default:

- settings: `~/.config/nivora-launcher/`;
- instances and logs: `~/.local/share/nivora-launcher/`;
- downloads and cache: `~/.cache/nivora-launcher/`.

The configuration does not touch the existing `~/.minecraft`. The path can be changed:

```bash
python3 -m nivora_launcher config set game_dir /path/to/nivora-launcher-game
python3 -m nivora_launcher open-folder
```

## Cloud sync

In the "Settings" section you can connect a WebDAV endpoint of Nextcloud, ownCloud, Yandex Disk or a compatible service. Only permitted data is uploaded; plugins, server control JSON, `.trial-cell` and `accounts.json` are not synced.

The password or app token is kept in libsecret and never returned to the interface. Before a restore, a change list is shown; files are verified against the locally authenticated SHA-256 manifest. The manifest key lives only on this device, so restoring on another device is not supported yet. Details live in `docs/cloud-sync.md`.

## Server main screen and launch

The server Dashboard holds a console with command sending, CPU/RAM and separate RX/TX charts of the game TCP port, uptime and a restart button. Errors and the last console lines persist after a stop. A restart waits for completion and lock release before starting again. On Linux, traffic is measured from kernel socket counters of the server process (inet-diag); on Windows via TCP ESTATS `GetPerTcpConnectionEstats` without admin rights; in both cases it is this server's traffic only, not the whole NIC, and unsupported platforms or access limits are shown explicitly.

The **Access and properties** tab is a structural editor for `whitelist.json`, `ops.json` and `server.properties`: while the server runs, changes apply through live console commands (`whitelist add/remove/reload`, `op`/`deop`); when stopped, by editing the files while preserving key order and comments. Server settings include a daily restart schedule with a `say` warning a configurable number of minutes ahead. After a server crash the Dashboard shows a crash-report banner: likely cause, advice and, when the log points at specific mods, buttons to remove them.

In detail: [Launch, console and metrics](docs/server-runtime.md).

## Server on the internet · white IPv4

In the server workspace open **Manage → Network and white IP**: `server.properties` preparation, external IPv4 / WAN / CGNAT diagnostics, a temporary UPnP forwarding of the game TCP port, an address for friends and manual forwarding and firewall instructions. Publishing requires confirmation; other people's rules are never replaced. The UPnP rule renews only while the server runs, is removed on stop and carries a finite lease of up to ten minutes. Without a white IPv4, UPnP does not bypass provider NAT. Verify external availability from a different internet connection.

In detail: [Publishing a server](docs/server-network.md).

## LAN rooms

Open the separate "LAN rooms" page in the side menu. Devices must share one local network. In "auto" mode the room listens on all interfaces (`0.0.0.0`) and participants are given this computer's specific LAN IPv4; when the machine has several network adapters, the room page lists backup `host:port` addresses to enter in Minecraft manually. The host shares the LAN address and room port with friends; the password is shared separately. The page helps track the participant list but does not start a Minecraft server and does not forward game traffic. For play, the Minecraft server itself must be reachable at the shown LAN address and port; allow the ports in the local firewall if needed.

## Game overlay and recording

While an instance runs, the launcher holds global hotkeys and a transparent window over the game:

- **Shift+Tab** — open/close the overlay (Esc closes too). Top center: a clock and session/total playtime counters (this block can be hidden in settings but not moved). Left: a mods widget with search (Java instances); right: a process metrics widget CPU/RAM/disk with peak values and the recording panel.
- **MSI Afterburner-style system metrics** — a separate configurable panel: total CPU load with per-core bars, RAM and swap, every GPU (load, video memory, temperature), disk partitions and CPU/board temperatures. Sources are psutil and `nvidia-smi`; whatever is absent in the system is simply not shown.
- **Quick server chat** — for server instances: a console tail under the overlay and a command field (`say Hi!`, `/whitelist add Steve` …) without minimizing the game.
- **F7** — start/stop screen recording. Recording runs through external `ffmpeg`: capture via x11grab, hardware encoding — NVENC on NVIDIA, VAAPI on Intel/AMD, otherwise a fast libx264 preset. Files land in `<game dir>/recordings`, the last run's log next to them (`last-recording.log`). Audio is recorded via PulseAudio when enabled in settings.
- Settings live in "Settings → Game overlay and recording": overlay toggle, keys, widgets, encoder (auto/NVENC/VAAPI/x264), container (MP4/MKV), FPS, quality (CRF/CQ), audio recording, and the system-metrics view: position (four corners), scale, backdrop transparency, compact mode, the clock and the set of metric blocks (CPU/RAM/GPU/disks/temperatures/chat) — each one separately. Compact system metrics live in the launcher top bar, refreshing every 2 seconds.

Hotkeys are registered only for the game session (Java and Bedrock) and released when the game exits; recording stops cleanly at the same time. The overlay works in X11/XWayland sessions: global hotkeys and screen capture use X11. When another program grabs a key, change the combination in settings. In exclusive fullscreen the game hides all overlay windows — switch it to windowed mode (F11 in Minecraft) so the overlay is visible.

## GitHub plugin catalog

The catalog can search public GitHub Releases from repositories with the topic `nivora-launcher-plugin`, or import a ZIP from the device. Before saving, the launcher validates the manifest and the archive, computes the SHA-256 and keeps the plugin disabled. This check is not a release signature: review the source and the permissions yourself. Third-party code does not run yet.

Publishing from the launcher is not supported. To add a plugin to the community catalog, create a public repository with the topic `nivora-launcher-plugin` and attach a ZIP to a GitHub Release; requirements are described in `docs/plugin-api.md`.

## Arch/CachyOS package

With `base-devel`, `nodejs`, `npm`, `rust`, `qt6-base`, `qt6-svg`, `python`, `python-defusedxml`, `python-psutil`, `libsecret`, `gnome-keyring`, `libappindicator` and `makepkg` available:

```bash
./scripts/build_arch_package.sh &&
sudo pacman -U "$(ls -1t dist/arch/nivora-launcher-*.pkg.tar.zst | head -n 1)"
```

The script creates an isolated temporary directory with a `nivora-launcher-<pkgver>.tar.gz` archive whose root folder is `nivora-launcher`, computes and verifies its SHA-256, then runs `makepkg`. The Arch package runs the Python backend through the system Python, so PyInstaller from the AUR is not needed. The package lands in `dist/arch/`; after a successful build the script prints the exact path. Installation runs only when the build succeeded. The script does not install system dependencies. The Tauri build needs network access for npm dependencies.

Creating a server does not replace an existing instance: pick another name, or save and delete the old server separately before creating again. This prevents world loss when a core download fails.

For Arch/CachyOS the Tauri build uses the local GCC/BFD profile from `scripts/arch_native_build.py`: native C/C++ LTO and Rust linker-plugin LTO are disabled, including target-specific compiler variables. This prevents incompatible LTO objects when native libraries are packed into a Rust rlib. The system `makepkg.conf` and Cargo settings are not modified. After changing the profile, run `scripts/build_arch_package.sh` again: it creates a clean build tree without the old `target/`.

The Arch profile builds only the Tauri binary with `--no-bundle` and `tauri.arch.conf.json`. No DEB/RPM/AppImage or signed updater artifacts are produced here: the final `.pkg.tar.zst` comes from `makepkg`, so `TAURI_SIGNING_PRIVATE_KEY` is not required for a local Arch build. The public key and update-signature verification are not disabled; signed-release settings for other platforms remain as before.

By default the package starts the graphical interface:

```bash
nivora-launcher
```

For diagnostics use the CLI commands:

```bash
nivora-launcher doctor
```

## Windows .exe (separate script)

The exe build is fully automated and independent of the Arch package:

```bash
./scripts/build_windows_exe.sh              # backend + frontend + NSIS installer
./scripts/build_windows_exe.sh --only-backend  # only nivora-launcher-backend.exe
```

A single run does everything: PyInstaller packs the Python backend into `nivora-launcher-backend.exe` (entry point `scripts/backend_entry.py`: without arguments — the Tauri JSON worker, with `--cli` — the CLI; packed together with `vendor/*.pyz`), the exe is placed into `frontend/src-tauri/resources/backend/` where `tauri.windows.conf.json` expects it, then the frontend is built and `tauri build` produces the NSIS installer `*-setup.exe` and the standalone `nivora-launcher.exe`. Artifacts are copied to `dist/windows/` and the script prints the exact paths. When `TAURI_SIGNING_PRIVATE_KEY` is not set, signed updater artifacts are disabled for the local build.

**Portable run without installation — one file.** `nivora-launcher.exe` already contains the Python backend inside: on first run the backend unpacks into `%LOCALAPPDATA%\nivora-launcher\backend\` and runs from there. The single `nivora-launcher.exe` is enough — nothing needs to be placed next to it. If a `nivora-launcher-backend.exe` does sit next to the exe (or in a `backend/` subfolder), it is used; the NSIS installer keeps building too, for those who want a classic system installation. The backend runs in windowed mode (`--noconsole`): no console windows appear on launcher start, recording (ffmpeg) or `--cli` game runs. The system Python on Windows is never invoked — on a clean system that is the Store stub ("Python was not found").

**Why wine on Linux.** PyInstaller cannot cross-compile: the backend exe can only be built by a Windows Python, which on Linux runs under wine. The Rust/Tauri shell cross-compiles natively and does not use wine — wine is needed for exactly one step. You do not configure it by hand: when a Windows Python is not found, the script downloads the official installer from python.org and quietly (`/quiet`) installs it into the isolated prefix `.build-windows/wineprefix` — `~/.wine` is untouched, and repeated runs reuse the finished installation. The project dependencies (`.[windows]`) and PyInstaller are installed into that Python by the script as well. The no-wine option at all is running the script on Windows (Git Bash/MSYS2): native Python is used there.

Where to run: on Windows (Git Bash/MSYS2) you need Python 3.10+, Node.js/npm and Rust (msvc); on Linux — only `wine` and `curl` (for the backend; installing Python under wine needs a working `DISPLAY` or `xvfb-run`). Everything else the script installs itself: `rustup` with the `x86_64-pc-windows-msvc` target and `cargo-xwin` — into `$HOME` (the system Rust is untouched), the system packages `clang`/`lld`/`llvm`/`binutils` — via pacman/apt (root/sudo needed). `tauri build` runs cargo through `--runner cargo-xwin`: the wrapper supplies clang-cl, lld-link and downloads the Windows SDK, so the msvc target links without Windows. A separate case is the NSIS installer: tauri-bundler calls the system `makensis`, and Arch has no `nsis` package in the official repositories (AUR only), so the script downloads a ready `makensis` from a Debian package into `.build-windows/nsis/` (dependencies — basic glibc/libstdc++/zlib) and feeds the data through the `/usr/share/nsis` symlink; tauri downloads its own Stubs/plugins. The tauri installer template includes `Win\RestartManager.nsh` — a file from NSIS 3.10+ that exists neither in Debian stable nor in the 3.08 package, so the script adds it from `nsis-common` 3.11. The build runs with `--bundles nsis` (MSI/WiX does not build on a Linux host). The first cross-build is long — the Windows SDK is downloaded and all Rust dependencies compile.

## Linux .deb / .rpm and building everything at once

The scripts work like the Windows one: a PyInstaller backend (onefile, with keyring inside), the frontend, `tauri build --bundles deb|rpm`:

```bash
./scripts/build_deb.sh            # Debian/Ubuntu → dist/deb/*.deb
./scripts/build_rpm.sh            # Fedora/RHEL/openSUSE → dist/rpm/*.rpm
./scripts/build_all.sh            # everything: windows + arch + deb + rpm
./scripts/build_all.sh deb rpm    # only the selected targets
```

System dependencies: `build-essential`/`gcc-c++`, `pkg-config`, `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`, `libssl-dev`, `python3-venv`, Node.js/npm and Rust — the scripts check them and print the exact missing install command. Code signing is optional: `DEB_SIGN_KEY=<gpg-id>` (dpkg-sig) for deb and `RPM_SIGN_KEY=<gpg-id>` (rpmsign) for rpm — the GPG key must be in the local keyring. Without a key the package still builds; the package manager will warn about the unsigned source.

## Windows warns on launch

The launcher's installer and exe are **not Authenticode code-signed**, so on first run Windows SmartScreen may show the blue "Windows protected your PC" window, and the browser on download — "file is not commonly downloaded". This is standard behavior for any unsigned file (the file simply has no "reputation"), not a sign of malware: the warning appears equally on home builds and on indie projects without a certificate.

What the user should do:

1. Click **"More info"** → **"Run anyway"**. For the installer — the same in the UAC window ("Publisher: unknown").
2. Optionally verify the file: `sha256sums.txt` is published next to the exe; check with `sha256sum -c sha256sums.txt` (or `certutil -hashfile file SHA256` on Windows).
3. When a specific antivirus (not SmartScreen) reports a "trojan" — it is almost certainly a heuristic false positive on a PyInstaller-onefile file; check the file on virustotal.com and, when only one or two engines out of seventy flag it, calmly file a false-positive report: https://www.microsoft.com/wdsi/filesubmission.

Measures already built in: version/publisher metadata in the exe (`--version-file`, `bundle.publisher`), checksums and optional signing — set `WINDOWS_CERT_FILE` (PKCS#12, for example Certum Open Source Code Signing) and, if needed, `WINDOWS_CERT_PASSWORD`, and the script signs every exe via `osslsigncode` (with a timestamp), after which SmartScreen stops showing an "unknown publisher".

## Next tasks

1. Test vanilla on a clean CachyOS with Java 8/17/21/25.
2. Verify the installer matrix for the supported Forge/NeoForge versions.
3. Implement safe WebDAV restore on another device and sign release artifacts.
4. Extend the plugin sandbox: more host services (scoped network, background tasks) and a visual UI-node editor.
5. Verify the Tauri/Vue UI and a real launch on clean CachyOS, including the GLX/software fallback.

## License

Nivora Launcher is distributed under the GNU General Public License version 3. The license text is in `LICENSE`.

### Plugin core extensions

Declarative forms/tabs/accordion/tables/charts, guest-owned fragments and data bindings, enforced call/data quotas, redacted host audit, listener diagnostics and developer runtime reload are implemented. See [implementation status](docs/plugin-core-extension-status.md) for limits and examples. This update does not implement the proposed VPN sessions, new filesystem/process callbacks, OS executable/browser sessions or secret/OAuth/WebSocket/AI services; existing permission names do not establish implementation.
