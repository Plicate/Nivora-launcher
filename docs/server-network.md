# Публикация сервера через белый IPv4

## Интерфейс

Откройте сервер → **Управление → Сеть и белый IP**. На Dashboard также есть кнопка перехода в этот раздел. Это публикация игрового Java TCP-порта, а не LAN-комната, VPN или relay.

1. Сохраните внутренний игровой порт в настройках сервера (обычно `25565`).
2. Для обычного Minecraft-ядра остановите сервер и нажмите **Подготовить server.properties**. После подтверждения лаунчер выставит сохранённый `server-port` и очистит `server-ip`, чтобы сервер принимал подключения на сетевых интерфейсах. Остальные свойства, в том числе `online-mode` и `white-list`, не меняются.
3. Запустите сервер обычной кнопкой и дождитесь готовности ядра.
4. Нажмите **Проверить сеть**. Проверка определяет локальный адрес, внешний IPv4 через `https://api.ipify.org`, UPnP IGD и WAN-адрес роутера. Проверяется локальный игровой TCP listener. Открытие вкладки само по себе не вызывает IP-сервис и не создаёт проброс.
5. Выберите внешний TCP-порт. Например, `25570 → локальный_IP:25565`; сохранять одинаковые порты необязательно.
6. При белом WAN и доступном UPnP нажмите **Открыть через UPnP** и подтвердите публикацию. Лаунчер создаёт временное правило и показывает адрес `IPv4:внешний_порт` для копирования.
7. Проверьте подключение с другого интернет-канала: например, попросите друга подключиться или используйте мобильный интернет. С того же компьютера нельзя надёжно подтвердить доступность извне из-за NAT loopback.

У Velocity/Waterfall listener настраивается в файлах прокси, а не в `server.properties`. Сохранённый внутренний порт лаунчера должен совпадать с портом listener. TCP-проверка использует этот порт; подготовка `server.properties` для прокси заблокирована.

## Тип подключения

- **Белый IP на компьютере:** проброс на роутере не нужен. Сервер должен принимать подключения на этом интерфейсе; разрешите входящий игровой TCP-порт в firewall.
- **Белый WAN роутера:** автоматический UPnP или ручной Port Forwarding / Virtual Server.
- **Серый WAN / второй роутер:** проброс только на внутреннем роутере недостаточен. Настройте оба роутера или bridge на внешнем.
- **CGNAT (`100.64.0.0/10`):** закажите публичный IPv4 у провайдера. Лаунчер не обходит NAT провайдера.
- **WAN не совпадает с адресом IP-сервиса:** возможен другой NAT, VPN или иной выход в интернет. Автопубликация блокируется, а автоадрес для копирования скрывается, пока топология не будет понятна.
- **UPnP не найден:** включите UPnP IGD в роутере или выполните ручной проброс. Наличие внешнего IP в отчёте само по себе не доказывает, что этот IP назначен вашему роутеру.
- **Не удалось определить IP:** можно вручную указать известный белый IPv4 для адреса подключения. Это не включает проброс и не меняет результат проверки CGNAT; используйте ручной адрес только если знаете, что внешний маршрутизатор настроен на этот порт.

Поддержка в этом модуле: IPv4, Java TCP, UPnP IGD v1/v2 (WANIPConnection v1/v2, WANPPPConnection v1). IPv6, Bedrock UDP, NAT-PMP/PCP, VPN, туннели и внешняя инфраструктура проверки портов не реализованы.

## Правила UPnP и остановка

- Только игровой TCP-порт; RCON, query UDP и интерфейс лаунчера не публикуются автоматически.
- Перед созданием проверяется существующее правило. Чужое правило или порт другого сервера не перезаписывается.
- Запрашивается аренда **600 секунд**. Роутер должен подтвердить конечную аренду от 60 до 600 секунд; постоянные/неподтверждённые правила отклоняются с попыткой удаления.
- Правило продлевается не реже половины подтверждённой аренды, обычно раз в 240 секунд. Фоновый монитор проверяет состояние каждые 15 секунд.
- Кнопка **Закрыть UPnP-проброс** удаляет только правило с ожидаемым описанием, внутренним IP и портом. Правило, заменённое вне лаунчера, не удаляется.
- Обычная остановка сервера через лаунчер пытается удалить проброс сразу. При завершении через консоль или аварии серверного процесса монитор удаляет его после обнаружения остановки.
- При недоступном роутере продление отключается, остаётся ошибка и возможность повторить удаление. Подтверждённая конечная аренда истекает без продления.
- При принудительном завершении backend/лаунчера гарантировать удаление невозможно; у поддерживаемого роутера с корректной арендой правило истечёт максимум через десять минут. На проблемном роутере проверяйте таблицу правил вручную.
- Переход на другую вкладку не закрывает работающий сервер. Автооткрытие при новом запуске не включается: публикация требует явного действия.
- Ручной проброс и правила firewall лаунчер не удаляет; их надо отменить вручную.

## Firewall и безопасность

Панель показывает команды UFW, firewalld и Windows PowerShell для выбранного **внутреннего** порта, включая команды отмены. Лаунчер их не выполняет, не запрашивает права администратора и не отключает firewall. Используйте команду только для firewall, который действительно управляет вашей системой; команды firewalld в панели действуют до перезагрузки firewall.

Перед публикацией рекомендуется:

- `online-mode=true` для обычного standalone Minecraft-сервера;
- `white-list=true` и `whitelist add <ник>` для доверенных игроков;
- актуальное доверенное ядро и проверенные плагины;
- резервная копия мира;
- закрытый RCON и отсутствие лишних ручных пробросов.

На прокси-сети настройки аутентификации и forwarding имеют свою специфику; backend-серверы не должны быть доступны напрямую из интернета. Лаунчер не переписывает такую конфигурацию автоматически.

Диагностика не является внешним аудитом безопасности. Статус **«UPnP-проброс создан»** означает подтверждённое правило роутера, не подтверждённое подключение из интернета.

## Границы реализации

UPnP использует отдельный ограниченный LAN-клиент, без изменения политик публичного HTTP-клиента. Description/control URLs должны содержать RFC1918 IPv4, совпадающий с источником SSDP-ответа. DNS-имена, loopback, link-local, сторонние адреса, URL credentials и redirects запрещены. XML ограничен по размеру и разбирается через defusedxml; DTD/entities не разрешены. Запросы не содержат учётных данных Microsoft или Minecraft.

Проверки выполняются unit/integration-тестами с поддельным роутером: аренда и продление, удаление после остановки, конфликты, неподдерживаемые постоянные правила, CGNAT, подтверждение публикации, сохранение server.properties, XML/URL ограничения, HTTP redirects, мост frontend → desktop → backend. Подключение к настоящему роутеру и вход из внешнего интернета проверяются на целевой системе.

---

<!-- English version / Английская версия -->

# Publishing a server over a white IPv4

## Interface

Open the server → **Manage → Network and white IP**. The Dashboard also has a button that jumps to this section. This publishes the game's Java TCP port; it is not a LAN room, a VPN or a relay.

1. Save the internal game port in server settings (usually `25565`).
2. For a regular Minecraft core, stop the server and press **Prepare server.properties**. After confirmation the launcher sets the saved `server-port` and clears `server-ip` so the server accepts connections on every network interface. Other properties, including `online-mode` and `white-list`, are untouched.
3. Start the server with the usual button and wait for the core to become ready.
4. Press **Check network**. The check determines the local address, the external IPv4 via `https://api.ipify.org`, UPnP IGD and the router's WAN address. The local game TCP listener is verified. Merely opening the tab does not call the IP service and does not create a forwarding rule.
5. Choose the external TCP port. For example, `25570 → local_IP:25565`; keeping the same port number is not required.
6. With a white WAN and available UPnP press **Open via UPnP** and confirm the publication. The launcher creates a temporary rule and shows the `IPv4:external_port` address for copying.
7. Verify connectivity from a different internet connection: ask a friend to connect or use mobile internet. The same computer cannot reliably confirm external availability because of NAT loopback.

For Velocity/Waterfall the listener is configured in the proxy files, not in `server.properties`. The launcher's saved internal port must match the listener port. The TCP check uses that port; preparing `server.properties` for proxies is blocked.

## Connection type

- **White IP on the computer:** no router forwarding needed. The server must accept connections on that interface; allow the incoming game TCP port in the firewall.
- **Router's white WAN:** automatic UPnP or manual Port Forwarding / Virtual Server.
- **Gray WAN / second router:** forwarding only on the inner router is not enough. Configure both routers or a bridge on the outer one.
- **CGNAT (`100.64.0.0/10`):** order a public IPv4 from your provider. The launcher does not bypass provider-grade NAT.
- **WAN does not match the IP service's address:** another NAT, a VPN or a different internet exit is likely. Auto-publishing is blocked and the copyable auto address is hidden until the topology is clear.
- **UPnP not found:** enable UPnP IGD on the router or forward manually. An external IP in the report alone does not prove that IP is assigned to your router.
- **IP could not be determined:** you can manually enter a known white IPv4 as the connection address. This does not enable forwarding and does not change the CGNAT check result; use a manual address only when you know the outer router forwards that port.

Support in this module: IPv4, Java TCP, UPnP IGD v1/v2 (WANIPConnection v1/v2, WANPPPConnection v1). IPv6, Bedrock UDP, NAT-PMP/PCP, VPN, tunnels and external port-check infrastructure are not implemented.

## UPnP rules and shutdown

- Only the game TCP port; RCON, query UDP and the launcher interface are never published automatically.
- An existing rule is checked before creation. Someone else's rule or another server's port is not overwritten.
- A **600-second** lease is requested. The router must confirm a finite lease of 60 to 600 seconds; permanent or unconfirmed rules are rejected with a deletion attempt.
- The rule is renewed at no less than half the confirmed lease, usually every 240 seconds. The background monitor checks state every 15 seconds.
- The **Close UPnP forwarding** button deletes only the rule with the expected description, internal IP and port. A rule replaced outside the launcher is not deleted.
- A normal server stop through the launcher tries to delete the forwarding immediately. When the server exits via console or crashes, the monitor deletes it after detecting the stop.
- If the router is unreachable, renewal is disabled; the error and a delete retry remain. A confirmed finite lease simply expires unrenewed.
- On forced backend/launcher termination deletion cannot be guaranteed; on a supported router with a correct lease the rule expires within ten minutes at most. On a problematic router check the rule table manually.
- Switching to another tab does not close a running server. Auto-opening on a new launch never happens: publication requires an explicit action.
- Manual forwarding and firewall rules are not deleted by the launcher; cancel them by hand.

## Firewall and security

The panel shows UFW, firewalld and Windows PowerShell commands for the selected **internal** port, including undo commands. The launcher does not execute them, does not request admin rights and does not disable the firewall. Use a command only for the firewall that actually manages your system; the firewalld commands shown in the panel act until the firewall reloads.

Before publishing it is recommended to have:

- `online-mode=true` for a regular standalone Minecraft server;
- `white-list=true` and `whitelist add <nick>` for trusted players;
- an up-to-date trusted core and verified plugins;
- a world backup;
- RCON closed and no extra manual forwarding.

On a proxy network, authentication and forwarding settings have their own specifics; backend servers must not be reachable directly from the internet. The launcher does not rewrite such a configuration automatically.

The diagnostics are not an external security audit. The **"UPnP forwarding created"** status means a confirmed router rule, not a verified connection from the internet.

## Implementation boundaries

UPnP uses a separate restricted LAN client and does not touch the public HTTP client policies. Description/control URLs must contain an RFC1918 IPv4 matching the source of the SSDP response. DNS names, loopback, link-local, third-party addresses, URL credentials and redirects are forbidden. XML is size-limited and parsed through defusedxml; DTD/entities are not allowed. Requests carry no Microsoft or Minecraft credentials.

The checks are covered by unit/integration tests with a fake router: lease and renewal, deletion after stop, conflicts, unsupported permanent rules, CGNAT, publication confirmation, server.properties saving, XML/URL limits, HTTP redirects, and the frontend → desktop → backend bridge. Connecting to a real router and joining from the external internet are verified on the target system.
