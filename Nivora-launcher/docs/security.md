# Instance Security

Каждый игровой экземпляр получает отдельный `.trial-cell/` для `HOME`, временных файлов, конфигурации и кэша. Новый экземпляр использует профиль `strict`.

## Linux Sandbox

- На Linux `strict` требует рабочий `bubblewrap` (на Windows — AppContainer, см. ниже); при отсутствии sandbox запуск завершается ошибкой. `/usr`, системные бинарники и Java подключаются read-only; вместо полного host `/etc` создаётся отдельный каталог только с небольшим набором файлов конфигурации, нужных libc и Java.
- Домашние и временные каталоги скрыты. Папка экземпляра подключается на запись только с `files:instance`; host-write в `strict` запрещён. `files:host-read` открывает ограниченные host-папки read-only.
- Новые игровые экземпляры по умолчанию получают `files:instance`, `desktop:display` и `desktop:audio`; `network:game` выключен. Его включение разрешает обычный доступ ко всем адресам, включая localhost и LAN; списка разрешённых серверов нет. Без этого разрешения `strict` использует отдельный network namespace. Это не фильтрация адресов и не защита от утечки данных при включённой сети.
- `desktop:display` управляет X11/Wayland сокетами и GPU-устройствами; `desktop:audio` управляет PipeWire/PulseAudio сокетами и `/dev/snd`. Оба разрешения включены по умолчанию для сохранения обычного игрового поведения и отключаются в настройках экземпляра. X11/XWayland может раскрывать окну ввод или содержимое других окон.
- При миграции старых настроек версии 2 сетевой доступ по умолчанию отзывается; включите `network:game` для сетевой игры. Ранее неявный доступ к дисплею и звуку становится явным, чтобы не ломать существующие установки.
- `strict` не устанавливает seccomp syscall filter. Это снижает доступ к файловой системе и устройствам, но не защищает от уязвимости ядра или вредоносного кода, использующего разрешённые каналы.

## Windows Sandbox

На Windows `strict` и `manual` запускают Java в **AppContainer** внутри **Job Object** (Windows 8+, права администратора не нужны). Обёртку выполняет сам бэкенд в режиме `--win-sandbox` (`nivora_launcher/windows_sandbox.py`) — аналог bubblewrap на Linux.

- У каждого экземпляра свой профиль AppContainer (`Nivora.Cell.<hash>`). Его SID получает доступ только к папке экземпляра (запись — с `files:instance`, иначе чтение; временный каталог cell остаётся записываемым), к каталогу Java (чтение) и к Java argument file (чтение). Профиль пользователя, другие диски и данные лаунчера процессу не видны. Служебные файлы `.trial-launcher*.json` закрыты для записи явным deny-ACE — как read-only bind в bubblewrap.
- Метка выданных ACL хранится в `%LOCALAPPDATA%\nivora-launcher\sandbox-acl\`, вне папки экземпляра, чтобы игра не могла её подделать. При переключении записи на чтение прежнее разрешение снимается.
- Без `network:game` у контейнера нет сетевых capabilities — сети нет полностью, включая loopback. С `network:game` выдаются `internetClient`, `internetClientServer` и `privateNetworkClientServer`. Windows не пускает AppContainer к этому же компьютеру (localhost); для игры на локальном сервере используйте `off` или исключение `CheckNetIsolation.exe LoopbackExempt -a -p=<SID>` (нужны права администратора).
- Job Object: игра завершается вместе с обёрткой (`KILL_ON_JOB_CLOSE`, аналог `--die-with-parent`), не может запускать дочерние процессы (лимит — один процесс), не может переключать рабочие столы, менять системные параметры и завершать сеанс. Без `desktop:display` дополнительно закрыты буфер обмена, чужие окна и смена режима экрана.
- Входящие подключения к процессу в контейнере блокируются: хостить Minecraft-сервер из песочницы нельзя — клиенты не смогут подключиться. Серверы из desktop-приложения запускаются нативным движком без песочницы; для CLI-запуска сервера используйте профиль `off`.
- AppContainer работает с низким уровнем целостности (UIPI): нельзя слать сообщения и ставить хуки окнам обычных процессов. Микрофон и камера закрыты всегда — capabilities для них не выдаются.
- `HOME`, `TEMP`/`TMP`, `APPDATA` и `LOCALAPPDATA` игры указывают в cell; сама обёртка работает с настоящими каталогами пользователя, чтобы её код не распаковывался в папку, доступную игре на запись.

Ограничения Windows (честно):

- `desktop:audio` не обеспечивается: Windows не даёт запретить вывод звука в AppContainer. Окно игры без `desktop:display` не скрывается — применяются только UI-ограничения Job Object.
- `files:host-read` в `strict` не поддерживается: потребовалось бы менять ACL на всём профиле пользователя. В `manual` с этим разрешением (или с `files:host-write`) песочница не включается, и лаунчер предупреждает об этом.
- Java из `Program Files` читается через `ALL APPLICATION PACKAGES`; если у установленной Java такого доступа нет, запуск в песочнице завершается ошибкой с подсказкой выбрать Java в папке пользователя или Mojang runtime.
- Часть драйверов OpenGL может вести себя в AppContainer иначе, чем в обычном процессе; при ошибке создания контекста используйте `off` для доверенной сборки.
- Как и на Linux, это изоляция всего экземпляра, а не отдельных модов, и не защита от уязвимостей ядра.
- Серверы из desktop-приложения запускает нативный Rust-движок, который пока не применяет ни bubblewrap, ни AppContainer (на любой ОС).

## Профили и Ограничения

- `compat` сохраняет исходные HOME/XDG/graphics-переменные и не изолирует моды.
- `manual` применяет bubblewrap на Linux (если он доступен и не выдан host-write) и AppContainer на Windows (если не выданы host-read/host-write); иначе возвращает предупреждение, что ограничения network/files/desktop не обеспечиваются.
- `strict` поддерживается на Linux (bubblewrap) и Windows (AppContainer) и не запускается, если песочница недоступна. На других ОС `strict` отклоняется.
- `off` отключает ограничения лаунчера и предназначен только для доверенных сборок.
- `process:spawn`, `chat:read`, `input:capture` и `native:load` нельзя надёжно выдать модам внутри одной JVM в `manual` или `strict`; такие права отклоняются до запуска.
- Java-моды одной сборки работают в одном JVM и разделяют его права. Sandbox применяется ко всему экземпляру, а не к каждому моду отдельно.

## Account Token

Microsoft access token не передаётся Minecraft JVM в OS argv. Launcher формирует Java argument file: на Linux это anonymous `memfd`, а в `strict` bubblewrap подключает его read-only только внутри security cell. В argv остаётся ссылка на argument file; fallback-файл имеет режим `0600` и удаляется после запуска. На Windows файл лежит в каталоге данных лаунчера, его ACL сужается до текущего пользователя (наследование отключается), а в `strict`/`manual` чтение дополнительно выдаётся только SID песочницы этого экземпляра.

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

## LAN-комнаты

Встроенные LAN-комнаты удалены. Лаунчер больше не открывает сетевой порт для комнат и не проксирует игровой трафик; операции `lan_session_*` отклоняются desktop bridge и Python-бэкендом.

## Публичные серверы

Публикация через белый IPv4 — отдельная opt-in функция серверного workspace, не LAN relay. Она создаёт только подтверждённое временное UPnP-правило игрового TCP-порта, не включает RCON, не отключает firewall и не меняет online-mode / whitelist. Чужие UPnP-правила не перезаписываются и не удаляются. Ограниченный LAN HTTP-клиент обращается только к RFC1918 IPv4 источника SSDP-ответа; redirects, credentials, loopback/link-local и сторонние control URLs запрещены. XML ограничен по размеру и разбирается с запретом entities. Публичный HTTP-клиент и его SSRF-политика не ослабляются. Документация: `docs/server-network.md`.

---

<!-- English version / Английская версия -->

# Instance Security

Each game instance gets a separate `.trial-cell/` for `HOME`, temporary files, configuration and cache. A new instance uses the `strict` profile.

## Linux Sandbox

- On Linux `strict` requires a working `bubblewrap` (on Windows — AppContainer, see below); without a sandbox the launch fails. `/usr`, system binaries and Java are mounted read-only; instead of the full host `/etc` a separate directory is created with only the few configuration files libc and Java need.
- Home and temporary directories are hidden. The instance folder is mounted writable only with `files:instance`; host-write is forbidden in `strict`. `files:host-read` opens limited host folders read-only.
- New game instances default to `files:instance`, `desktop:display` and `desktop:audio`; `network:game` is off. Enabling it allows ordinary access to all addresses, including localhost and LAN; there is no allowed-server list. Without this permission `strict` uses a separate network namespace. This is not address filtering and not leak protection once networking is on.
- `desktop:display` controls X11/Wayland sockets and GPU devices; `desktop:audio` controls PipeWire/PulseAudio sockets and `/dev/snd`. Both permissions are enabled by default to preserve ordinary game behavior and can be disabled in instance settings. X11/XWayland may expose input or the contents of other windows to the game window.
- When old version 2 settings are migrated, network access is revoked by default; enable `network:game` for network play. Previously implicit display and audio access becomes explicit, so existing installations keep working.
- `strict` does not install a seccomp syscall filter. This reduces filesystem and device access but does not protect against a kernel vulnerability or malicious code using permitted channels.

## Windows Sandbox

On Windows `strict` and `manual` run Java in an **AppContainer** inside a **Job Object** (Windows 8+, no administrator rights needed). The wrapper is the backend itself in `--win-sandbox` mode (`nivora_launcher/windows_sandbox.py`) — the counterpart of bubblewrap on Linux.

- Each instance has its own AppContainer profile (`Nivora.Cell.<hash>`). Its SID is granted access only to the instance folder (write with `files:instance`, otherwise read; the cell temp directory stays writable), the Java directory (read) and the Java argument file (read). The user profile, other drives and launcher data are invisible to the process. The `.trial-launcher*.json` service files are write-protected with an explicit deny ACE — like the read-only bind in bubblewrap.
- The record of granted ACLs is kept in `%LOCALAPPDATA%\nivora-launcher\sandbox-acl\`, outside the instance folder, so the game cannot forge it. Switching from write to read removes the previous grant.
- Without `network:game` the container has no network capabilities — no network at all, loopback included. With `network:game` it gets `internetClient`, `internetClientServer` and `privateNetworkClientServer`. Windows blocks AppContainers from reaching the same computer (localhost); to play on a local server use `off` or the `CheckNetIsolation.exe LoopbackExempt -a -p=<SID>` exemption (administrator rights required).
- Job Object: the game exits together with the wrapper (`KILL_ON_JOB_CLOSE`, like `--die-with-parent`), cannot start child processes (limit of one process), cannot switch desktops, change system parameters or log off. Without `desktop:display` the clipboard, other windows and display mode changes are also closed.
- Inbound connections to a process in the container are blocked: you cannot host a Minecraft server from the sandbox — clients will not be able to connect. Servers started from the desktop app run through the native engine without a sandbox; use the `off` profile for CLI server launches.
- AppContainer runs at low integrity (UIPI): it cannot send messages to or hook windows of normal processes. The microphone and camera are always closed — no capabilities are granted for them.
- The game's `HOME`, `TEMP`/`TMP`, `APPDATA` and `LOCALAPPDATA` point into the cell; the wrapper itself uses the real user directories so its code is never unpacked into a folder the game can write.

Windows limitations (honestly):

- `desktop:audio` is not enforced: Windows cannot forbid audio output in an AppContainer. Without `desktop:display` the game window is not hidden — only the Job Object UI restrictions apply.
- `files:host-read` is not supported in `strict`: it would require changing ACLs on the whole user profile. In `manual` with this permission (or `files:host-write`) the sandbox is not enabled, and the launcher warns about it.
- Java under `Program Files` is readable through `ALL APPLICATION PACKAGES`; if an installed Java lacks that access, the sandboxed launch fails with a hint to pick a Java in the user folder or the Mojang runtime.
- Some OpenGL drivers may behave differently in an AppContainer than in a normal process; if context creation fails, use `off` for a trusted build.
- As on Linux, this isolates the whole instance, not individual mods, and does not protect against kernel vulnerabilities.
- Servers in the desktop app are started by the native Rust engine, which does not apply bubblewrap or AppContainer yet (on any OS).

## Profiles and Restrictions

- `compat` keeps the original HOME/XDG/graphics variables and does not isolate mods.
- `manual` applies bubblewrap on Linux (if available and host-write was not granted) and AppContainer on Windows (if host-read/host-write were not granted); otherwise it returns a warning that network/files/desktop restrictions are not provided.
- `strict` is supported on Linux (bubblewrap) and Windows (AppContainer) and refuses to launch when the sandbox is unavailable. On other systems `strict` is rejected.
- `off` disables launcher restrictions and is intended only for trusted builds.
- `process:spawn`, `chat:read`, `input:capture` and `native:load` cannot be reliably granted to mods inside a single JVM in `manual` or `strict`; such grants are rejected before launch.
- Java mods of one build run in one JVM and share its permissions. The sandbox applies to the whole instance, not to each mod separately.

## Account Token

The Microsoft access token is not passed to the Minecraft JVM in OS argv. The launcher builds a Java argument file: on Linux it is an anonymous `memfd`, and in `strict` bubblewrap mounts it read-only only inside the security cell. A reference to the argument file stays in argv; the fallback file has mode `0600` and is deleted after launch. On Windows the file lives in the launcher data directory, its ACL is narrowed to the current user (inheritance disabled), and in `strict`/`manual` read access is additionally granted only to this instance's sandbox SID.

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

## LAN rooms

The built-in LAN rooms have been removed. The launcher no longer opens a network port for rooms and no longer proxies game traffic; `lan_session_*` operations are rejected by the desktop bridge and the Python backend.

## Public servers

Publishing over a white IPv4 is a separate opt-in feature of the server workspace, not a LAN relay. It creates only a confirmed temporary UPnP rule for the game TCP port, does not enable RCON, does not disable the firewall and does not change online-mode / whitelist. Other people's UPnP rules are not overwritten or deleted. The restricted LAN HTTP client talks only to the RFC1918 IPv4 of the SSDP response source; redirects, credentials, loopback/link-local and third-party control URLs are forbidden. XML is size-limited and parsed with entities disallowed. The public HTTP client and its SSRF policy are not weakened. Documentation: `docs/server-network.md`.
