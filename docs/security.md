# Instance Security

Каждый игровой экземпляр получает отдельный `.trial-cell/` для `HOME`, временных файлов, конфигурации и кэша. Новый экземпляр использует профиль `strict`.

## Linux Sandbox

- `strict` требует Linux и рабочий `bubblewrap`; при отсутствии sandbox запуск завершается ошибкой. `/usr`, системные бинарники и Java подключаются read-only; вместо полного host `/etc` создаётся отдельный каталог только с небольшим набором файлов конфигурации, нужных libc и Java.
- Домашние и временные каталоги скрыты. Папка экземпляра подключается на запись только с `files:instance`; host-write в `strict` запрещён. `files:host-read` открывает ограниченные host-папки read-only.
- Новые игровые экземпляры по умолчанию получают `files:instance`, `desktop:display` и `desktop:audio`; `network:game` выключен. Его включение разрешает обычный доступ ко всем адресам, включая localhost и LAN; списка разрешённых серверов нет. Без этого разрешения `strict` использует отдельный network namespace. Это не фильтрация адресов и не защита от утечки данных при включённой сети.
- `desktop:display` управляет X11/Wayland сокетами и GPU-устройствами; `desktop:audio` управляет PipeWire/PulseAudio сокетами и `/dev/snd`. Оба разрешения включены по умолчанию для сохранения обычного игрового поведения и отключаются в настройках экземпляра. X11/XWayland может раскрывать окну ввод или содержимое других окон.
- При миграции старых настроек версии 2 сетевой доступ по умолчанию отзывается; включите `network:game` для сетевой игры. Ранее неявный доступ к дисплею и звуку становится явным, чтобы не ломать существующие установки.
- `strict` не устанавливает seccomp syscall filter. Это снижает доступ к файловой системе и устройствам, но не защищает от уязвимости ядра или вредоносного кода, использующего разрешённые каналы.

## Профили и Ограничения

- `compat` сохраняет исходные HOME/XDG/graphics-переменные и не изолирует моды.
- `manual` применяет bubblewrap на Linux, если он доступен и не выдан host-write; иначе возвращает предупреждение, что ограничения network/files/desktop не обеспечиваются.
- `strict` поддерживается только на Linux и не запускается, если bubblewrap недоступен.
- `off` отключает ограничения лаунчера и предназначен только для доверенных сборок.
- `process:spawn`, `chat:read`, `input:capture` и `native:load` нельзя надёжно выдать модам внутри одной JVM в `manual` или `strict`; такие права отклоняются до запуска.
- Java-моды одной сборки работают в одном JVM и разделяют его права. Sandbox применяется ко всему экземпляру, а не к каждому моду отдельно.

## Account Token

Microsoft access token не передаётся Minecraft JVM в OS argv. Launcher формирует Java argument file: на Linux это anonymous `memfd`, а в `strict` bubblewrap подключает его read-only только внутри security cell. В argv остаётся ссылка на argument file; fallback-файл имеет режим `0600` и удаляется после запуска.

Это защищает от случайного раскрытия токена через список аргументов, но не изолирует account от модов. Мод внутри JVM может прочитать состояние запуска и отправить доступные данные по разрешённой сети. Для недоверенных модпаков используйте отдельный Microsoft account и отключайте игровую сеть, если она не нужна.

## Plugins И Поставка

- Исполняемый Plugin API v2/v3 работает в QuickJS worker с permission-проверками на host-вызовах; API v1 остаётся статическим. Эти границы не распространяются на Java-моды. Подробнее: `docs/plugin-api.md`.
- `Cargo.lock` и SHA-пины GitHub Actions зафиксированы. CI создаёт provenance attestation для Windows и Linux пакетов. Это подтверждение происхождения сборки, а не Authenticode/macOS code-signing подпись; ключи подписи установщиков в репозитории не настроены.
- Подписи плагинов и списков отзыва проверяются отдельно по закреплённым Ed25519-ключам. Проверка хеша загружаемого игрового ресурса возможна, когда доверенный источник публикует этот хеш.
- Arch source archive получает SHA-256 при локальной сборке. Полноценная сборка и проверка системной матрицы требуют соответствующих инструментов и выполняются в CI или на целевой системе.

## Проверка модов и плагинов (Mod Audit)

Перед запуском сомнительного `.jar` лаунчер может провести статический аудит: мод или плагин декомпилируется, отчёт показывает оценку времени проверки по структуре файла и скорости ПК, и только после явного подтверждения пользователя начинается анализ. Если подтверждение не получено — декомпилированные исходники уничтожаются без проверки; после проверки они уничтожаются в любом случае, остаётся только отчёт.

Поток: `prepare` (декомпиляция и оценка) → подтверждение пользователя → `accept` (проверка) → отчёт → уничтожение исходников. Кнопка «Проверить безопасность» (иконка щита) доступна для модов во вкладке «Моды» экземпляра и для плагинов сервера. В CLI:

```bash
python3 -m nivora_launcher audit ~/Downloads/SuspiciousMod.jar          # с подтверждением
python3 -m nivora_launcher audit ~/Downloads/SuspiciousMod.jar --yes    # без вопроса
python3 -m nivora_launcher audit ~/Downloads/SuspiciousMod.jar --json   # машинно-читаемый отчёт
```

Как это работает:

- Декомпиляция — это разбор constant pool Java class-файлов в промежуточное представление `.jasm` (чистый Python). Код плагина никогда не исполняется. Вложенные jar-архивы распаковываются и проверяются рекурсивно (глубина 2), нативные библиотеки фиксируются как непроверяемые.
- Проверяются 22 категории сигналов: кейлоггеры (включая native-хуки), съёмка экрана, открытие/закрытие окон, чтение файлов за пределами своей папки (учётные данные, `.minecraft`, лаунчер), сетевые адреса и webhook-каналы утечки, запуск команд ОС, динамическая загрузка кода, обфускация, перехват команд и чата игроков, доступ к токенам аккаунтов, манипуляции с сервером и другие.
- Для снижения ложных срабатываний: известные сервисы (Modrinth, Hangar, SpigotMC, CurseForge, GitHub, Mojang и др.) вынесены в allowlist; голые версии Minecraft не считаются IP-адресами; вызовы с низкой опасностью помечаются как `info` и не влияют на вердикт; правила-комбинации срабатывают только на находках уровня notice и выше.
- Каждая находка имеет severity (`critical`/`warning`/`notice`/`info`), улики (класс и значение) и рекомендацию. Итог — вердикт `clean`/`review`/`suspicious`/`dangerous` и risk score 0–100. Комбинации вроде «кейлоггер + webhook» или «чтение паролей + сеть» помечаются как критические.
- Сессии живут в `data_home/mod-audit/`: подтверждение ждёт 30 минут, готовый отчёт — 6 часов, одновременно активно не более 4 сессий. Уничтожение исходников — перезапись случайными байтами и удаление, не просто unlink.

Ограничения: аудит статический — он не даёт гарантий и не заменяет sandbox. Обфусцированный или нативный код может скрыть поведение; нативные библиотеки и скачанный в рантайме код не проверяются. Используйте аудит вместе с профилем `strict`, а не вместо него.

## LAN Rooms

LAN-комната в авто-режиме слушает все локальные интерфейсы (как и сама игра по LAN), а участникам сообщается конкретный частный IPv4 хоста; если адрес определить не удалось, используется loopback. Публичные IP для входа отклоняются, список альтернативных локальных адресов показывается в интерфейсе комнаты. Комната использует парольную проверку и проксирует только Minecraft TCP-порт через прямое подключение к частному адресу хоста. Она не создаёт виртуальный сетевой адаптер, не передаёт произвольный трафик, не включает шифрование транспорта, NAT traversal или внешний relay и не соединяет устройства за разными сетями. Настройки firewall влияют на доступность комнаты.

## Публичные серверы

Публикация через белый IPv4 — отдельная opt-in функция серверного workspace, не LAN relay. Она создаёт только подтверждённое временное UPnP-правило игрового TCP-порта, не включает RCON, не отключает firewall и не меняет online-mode / whitelist. Чужие UPnP-правила не перезаписываются и не удаляются. Ограниченный LAN HTTP-клиент обращается только к RFC1918 IPv4 источника SSDP-ответа; redirects, credentials, loopback/link-local и сторонние control URLs запрещены. XML ограничен по размеру и разбирается с запретом entities. Публичный HTTP-клиент и его SSRF-политика не ослабляются. Документация: `docs/server-network.md`.

---

<!-- English version / Английская версия -->

# Instance Security

Each game instance gets a separate `.trial-cell/` for `HOME`, temporary files, configuration and cache. A new instance uses the `strict` profile.

## Linux Sandbox

- `strict` requires Linux and a working `bubblewrap`; without a sandbox the launch fails. `/usr`, system binaries and Java are mounted read-only; instead of the full host `/etc` a separate directory is created with only the few configuration files libc and Java need.
- Home and temporary directories are hidden. The instance folder is mounted writable only with `files:instance`; host-write is forbidden in `strict`. `files:host-read` opens limited host folders read-only.
- New game instances default to `files:instance`, `desktop:display` and `desktop:audio`; `network:game` is off. Enabling it allows ordinary access to all addresses, including localhost and LAN; there is no allowed-server list. Without this permission `strict` uses a separate network namespace. This is not address filtering and not leak protection once networking is on.
- `desktop:display` controls X11/Wayland sockets and GPU devices; `desktop:audio` controls PipeWire/PulseAudio sockets and `/dev/snd`. Both permissions are enabled by default to preserve ordinary game behavior and can be disabled in instance settings. X11/XWayland may expose input or the contents of other windows to the game window.
- When old version 2 settings are migrated, network access is revoked by default; enable `network:game` for network play. Previously implicit display and audio access becomes explicit, so existing installations keep working.
- `strict` does not install a seccomp syscall filter. This reduces filesystem and device access but does not protect against a kernel vulnerability or malicious code using permitted channels.

## Profiles and Restrictions

- `compat` keeps the original HOME/XDG/graphics variables and does not isolate mods.
- `manual` applies bubblewrap on Linux if it is available and host-write was not granted; otherwise it returns a warning that network/files/desktop restrictions are not provided.
- `strict` is supported only on Linux and refuses to launch when bubblewrap is unavailable.
- `off` disables launcher restrictions and is intended only for trusted builds.
- `process:spawn`, `chat:read`, `input:capture` and `native:load` cannot be reliably granted to mods inside a single JVM in `manual` or `strict`; such grants are rejected before launch.
- Java mods of one build run in one JVM and share its permissions. The sandbox applies to the whole instance, not to each mod separately.

## Account Token

The Microsoft access token is not passed to the Minecraft JVM in OS argv. The launcher builds a Java argument file: on Linux it is an anonymous `memfd`, and in `strict` bubblewrap mounts it read-only only inside the security cell. A reference to the argument file stays in argv; the fallback file has mode `0600` and is deleted after launch.

This protects against accidental token disclosure through the argument list, but does not isolate the account from mods. A mod inside the JVM can read the launch state and send available data over the permitted network. For untrusted modpacks use a separate Microsoft account and keep game networking off when you do not need it.

## Plugins and Distribution

- The executable Plugin API v2/v3 runs in a QuickJS worker with permission checks on host calls; API v1 remains static. These boundaries do not apply to Java mods. Details: `docs/plugin-api.md`.
- `Cargo.lock` and GitHub Actions SHA pins are fixed. CI creates provenance attestations for the Windows and Linux packages. This is build-origin evidence, not an Authenticode/macOS code-signing signature; installer signing keys are not configured in the repository.
- Plugin signatures and revocation lists are verified separately against pinned Ed25519 keys. Game resource download hash verification is possible when a trusted source publishes that hash.
- The Arch source archive gets a SHA-256 during local build. Full builds and system-matrix verification need the corresponding tools and run in CI or on the target system.

## Mod and plugin audit

Before launching a questionable `.jar` the launcher can run a static audit: the mod or plugin is decompiled, the report shows an estimated check time based on file structure and PC speed, and only after explicit user confirmation does the analysis start. Without confirmation the decompiled sources are destroyed unchecked; after the check they are destroyed in any case — only the report remains.

Flow: `prepare` (decompilation and estimate) → user confirmation → `accept` (analysis) → report → source destruction. The "Check safety" button (shield icon) is available for mods in the instance "Mods" tab and for server plugins. In the CLI:

```bash
python3 -m nivora_launcher audit ~/Downloads/SuspiciousMod.jar          # with confirmation
python3 -m nivora_launcher audit ~/Downloads/SuspiciousMod.jar --yes    # without the question
python3 -m nivora_launcher audit ~/Downloads/SuspiciousMod.jar --json   # machine-readable report
```

How it works:

- Decompilation is a constant-pool parse of Java class files into a `.jasm` intermediate representation (pure Python). Plugin code is never executed. Nested jar archives are unpacked and checked recursively (depth 2); native libraries are recorded as unverifiable.
- 22 signal categories are checked: keyloggers (including native hooks), screen capture, opening/closing windows, reading files outside the mod's own folder (credentials, `.minecraft`, the launcher), network addresses and webhook exfiltration channels, OS command execution, dynamic code loading, obfuscation, command and player-chat interception, account token access, server manipulation and more.
- To reduce false positives: known services (Modrinth, Hangar, SpigotMC, CurseForge, GitHub, Mojang and others) sit on an allowlist; bare Minecraft versions do not count as IP addresses; low-danger calls are marked `info` and do not affect the verdict; combination rules fire only on findings of notice level and above.
- Each finding has a severity (`critical`/`warning`/`notice`/`info`), evidence (class and value) and a recommendation. The result is a verdict `clean`/`review`/`suspicious`/`dangerous` and a risk score of 0–100. Combinations like "keylogger + webhook" or "password reading + network" are marked critical.
- Sessions live in `data_home/mod-audit/`: confirmation waits 30 minutes, a finished report 6 hours, and at most 4 sessions are active at once. Source destruction is an overwrite with random bytes plus deletion, not a plain unlink.

Limitations: the audit is static — it guarantees nothing and does not replace the sandbox. Obfuscated or native code can hide behavior; native libraries and code downloaded at runtime are not checked. Use the audit together with the `strict` profile, not instead of it.

## LAN Rooms

A LAN room in auto mode listens on all local interfaces (like the game itself over LAN), and participants are told the host's specific private IPv4; if the address cannot be determined, loopback is used. Public IPs for joining are rejected; the list of alternative local addresses is shown in the room interface. The room uses password verification and proxies only the Minecraft TCP port over a direct connection to the host's private address. It does not create a virtual network adapter, does not carry arbitrary traffic, does not enable transport encryption, NAT traversal or an external relay, and does not join devices across different networks. Firewall settings affect room availability.

## Public servers

Publishing over a white IPv4 is a separate opt-in feature of the server workspace, not a LAN relay. It creates only a confirmed temporary UPnP rule for the game TCP port, does not enable RCON, does not disable the firewall and does not change online-mode / whitelist. Other people's UPnP rules are not overwritten or deleted. The restricted LAN HTTP client talks only to the RFC1918 IPv4 of the SSDP response source; redirects, credentials, loopback/link-local and third-party control URLs are forbidden. XML is size-limited and parsed with entities disallowed. The public HTTP client and its SSRF policy are not weakened. Documentation: `docs/server-network.md`.
