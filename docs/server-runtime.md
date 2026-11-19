# Главный экран сервера, запуск и метрики

## Консоль и запуск

Dashboard сервера содержит консоль с командной строкой, автопрокруткой и кнопкой перехода к последней строке. Отдельный раздел Runtime → Консоль сохраняется для увеличенного просмотра. Вывод обновляется на обоих экранах; параллельные запросы логов не накапливаются. Запуск из общей карточки экземпляра открывает главный экран сервера.

После завершения процесса его вывод не удаляется. Последние строки сохраняются в `.nivora-launcher-server-console.json` и доступны после перезапуска backend. Сохраняются также ошибки до создания процесса: EULA, Java, отсутствующий JAR и профиль изоляции. Код завершения доступен в состоянии сервера. Журнал ограничен размером; это последние строки, не замена полному `logs/latest.log` ядра.

Для стандартных версий Minecraft предварительно проверяется минимальная Java:

- 1.20.5 и новее: Java 21;
- 1.18–1.20.4: Java 17;
- 1.17: Java 16;
- более старые версии: Java 8.

Эти значения — минимальная проверка для стандартного Minecraft, не гарантия совместимости конкретного форка с любой более новой JVM. Если Java явно задана и слишком старая, показывается ошибка до запуска. При отсутствии подходящей системной Java и отсутствии явного выбора используется существующий менеджер runtime Mojang (скачивание требует интернета). Неизвестные/custom версии проверяются самим JAR, а его сообщение остаётся в консоли.

Начальный heap ограничен 512 MiB или меньшим заданным максимумом; `-Xmx` остаётся выбранным пользователем. Это исключает обязательное резервирование всего лимита при старте. Прокси Velocity/Waterfall не получают Minecraft-аргумент `--nogui`.

Максимальный heap `-Xmx` ограничен 75% физической RAM, чтобы оставить системе резерв; если объём RAM определить нельзя, используется предел 2 GiB. Создание сервера и чтение старого конфига автоматически приводят превышение к пределу, а ручное сохранение значения выше него отклоняется. Интерфейс показывает текущий предел и не позволяет выбрать больше.

Профиль безопасности не понижается автоматически. Если strict требует отсутствующий bubblewrap или запрещённые ядром user namespaces, причина появляется в интерфейсе и консоли. Для доверенного ядра пользователь может самостоятельно выбрать другой профиль, понимая потерю изоляции. Сбой конкретного стороннего ядра диагностируется по его выводу; сохранение логов не означает, что любая сборка гарантированно запустится.

Browser preview не запускает Java: вместо имитации успешного запуска показывается требование desktop launcher.

## Рестарт и время работы

Кнопка **Рестарт** расположена рядом с остановкой в шапке работающего сервера. При включённом подтверждении разрушительных действий показывается предупреждение об отключении игроков. Последовательность:

1. закрыть созданный лаунчером UPnP-проброс (при ошибке роутера вывести предупреждение);
2. отправить серверу `stop` и дождаться завершения;
3. при зависании использовать существующую остановку дерева процессов;
4. дождаться финализации логов/метрик и освобождения блокировки экземпляра;
5. запустить сервер заново с сохранёнными настройками.

При неуспешной остановке второй процесс не создаётся. После рестарта публичный UPnP-проброс надо открыть снова в сетевой вкладке. Ручные правила роутера/firewall не изменяются.

Время работы показывается в шапке и отдельной карточке в формате `HH:MM:SS`. Для активного запуска используется монотонное время; при рестарте счётчик и история текущих графиков начинаются заново. Продолжительность прошлого запуска остаётся в последнем снимке.

## RX/TX — источник и ограничения

На Dashboard добавлены отдельные графики входящего **RX** и исходящего **TX**, текущая скорость в B/KiB/MiB в секунду и наблюдаемый объём за запуск.

Источник на Linux — read-only `INET_DIAG` / `TCP_INFO` ядра. Сначала через `/proc` собираются inode TCP-сокетов процесса сервера и его дочерних процессов (включая Java под bubblewrap), затем выбираются только их соединения на сохранённом внутреннем игровом TCP-порту. IPv4 и IPv6 сокеты учитываются. Фильтр ownership не позволяет выдать трафик другого процесса с таким же портом за серверный.

Не используются счётчики сетевой карты всего компьютера, sniffing, root-права, firewall-правила, eBPF или дополнительная программа `ss`. Запросы к ядру ограничены временем и размером; повторные опросы используют краткий кеш. Скорость вычисляется по разнице счётчиков с монотонным интервалом. Socket cookie предотвращает смешивание счётчиков при повторном использовании inode. Исчезновение соединения не уменьшает ранее накопленный наблюдаемый объём.

Это **наблюдаемый TCP payload игрового порта**, а не весь сетевой трафик процесса:

- UDP/query, RCON на другом порту и HTTP-загрузки ядра/плагинов не включены;
- короткое соединение, полностью прошедшее между выборками, может не попасть в статистику;
- сетевые заголовки не являются частью payload;
- пропуски и ограничения доступа не дают точного биллингового объёма;
- при запрете чтения FD или отсутствии необходимых kernel-счётчиков показывается причина и `—`, не фиктивный ноль;
- на Windows/macOS этот источник не реализован: также показывается недоступность, не трафик всего компьютера.

Проверки включают настоящее локальное TCP-соединение с известным количеством переданных байтов, разбор kernel-ответа, ownership дочерних процессов, расчёт скоростей, закрытие соединений, реальные тестовые subprocess с ранней ошибкой, штатной остановкой и рестартом. Полный Minecraft JAR и конкретный desktop build проверяются отдельно на целевой машине.

---

<!-- english version / Английская версия -->

# Server main screen, launch and metrics

## Console and launch

The server Dashboard has a console with a command line, autoscroll and a jump-to-last-line button. A separate Runtime → Console section remains for an enlarged view. Output updates on both screens; parallel log requests do not pile up. Launching from the shared instance card opens the server main screen.

After the process exits its output is not deleted. The last lines are saved to `.nivora-launcher-server-console.json` and survive a backend restart. Errors from before the process existed are saved too: EULA, Java, a missing JAR and the isolation profile. The exit code is available in the server state. The journal is size-limited; it holds the last lines and does not replace the core's full `logs/latest.log`.

For standard Minecraft versions a minimum Java is pre-checked:

- 1.20.5 and newer: Java 21;
- 1.18–1.20.4: Java 17;
- 1.17: Java 16;
- older versions: Java 8.

These values are a minimum check for standard Minecraft, not a guarantee that a specific fork works with any newer JVM. If Java is set explicitly and is too old, an error is shown before launch. When no suitable system Java exists and no explicit choice was made, the existing Mojang runtime manager is used (downloading needs internet). Unknown/custom versions are checked by the JAR itself, and its message stays in the console.

The initial heap is capped at 512 MiB or the smaller configured maximum; `-Xmx` remains the user's choice. This avoids reserving the whole limit up front at start. Velocity/Waterfall proxies do not receive the Minecraft `--nogui` argument.

The maximum heap `-Xmx` is limited to 75% of physical RAM to keep a reserve for the system; when RAM size cannot be determined, a 2 GiB limit applies. Server creation and reading an old config automatically clamp an excess to the limit, and manually saving a value above it is rejected. The interface shows the current limit and prevents choosing more.

The security profile is never downgraded automatically. If strict needs a missing bubblewrap or user namespaces forbidden by the kernel, the reason appears in the interface and console. For a trusted core the user may pick another profile themselves, accepting the loss of isolation. A specific third-party core failure is diagnosed from its output; keeping logs does not mean any build is guaranteed to start.

Browser preview does not launch Java: instead of faking a successful start it shows a desktop-launcher requirement.

## Restart and uptime

The **Restart** button sits next to stop in the running server header. When confirmation of destructive actions is on, a warning about disconnecting players is shown. Sequence:

1. close the launcher-created UPnP forwarding (warn on a router error);
2. send `stop` to the server and wait for it to finish;
3. on a hang use the existing process-tree stop;
4. wait for log/metric finalization and release of the instance lock;
5. start the server again with the saved settings.

On an unsuccessful stop no second process is created. After a restart the public UPnP forwarding must be opened again on the network tab. Manual router/firewall rules are not touched.

Uptime is shown in the header and a separate card as `HH:MM:SS`. Monotonic time is used for the active run; on restart the counter and the current charts' history start over. The previous run's duration stays in the last snapshot.

## RX/TX — source and limits

The Dashboard adds separate incoming **RX** and outgoing **TX** charts, the current rate in B/KiB/MiB per second and the observed volume per run.

On Linux the source is read-only kernel `INET_DIAG` / `TCP_INFO`. First, TCP socket inodes of the server process and its children (including Java under bubblewrap) are collected via `/proc`; then only their connections on the saved internal game TCP port are selected. IPv4 and IPv6 sockets both count. The ownership filter prevents passing off another process's traffic on the same port as the server's.

Whole-machine NIC counters, sniffing, root rights, firewall rules, eBPF or the external `ss` program are not used. Kernel queries are time- and size-limited; repeated polls use a short cache. The rate is computed from counter deltas over a monotonic interval. A socket cookie prevents mixing counters when an inode is reused. A connection disappearing does not reduce the previously accumulated observed volume.

This is the **observed TCP payload of the game port**, not all network traffic of the process:

- UDP/query, RCON on another port and HTTP downloads of cores/plugins are not included;
- a short connection that fully passes between samples may miss the stats;
- network headers are not part of the payload;
- gaps and access limits do not produce exact billable volume;
- when reading FDs is forbidden or the needed kernel counters are missing, the reason and `—` are shown, not a fake zero;
- on Windows/macOS this source is not implemented: unavailability is shown there too, never whole-computer traffic.


