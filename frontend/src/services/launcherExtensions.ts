import type { DensityMode, LauncherThemePreset, PluginManifest, PluginPermission, PluginUiFragment, SidebarMode, ThemeMode, ThemeSource } from '../types'

export const permissionLabels: Record<PluginPermission, string> = {
  'links:open': 'Открывать ссылки',
  'network:fetch': 'Получать данные по сети',
  'network:dns': 'Разрешать DNS-имена',
  'network:ping': 'Проверять доступность портов',
  'network:listen': 'Слушать локальную сеть',
  'launcher:servers': 'Хранить список серверов',
  'launcher:dialogs': 'Показывать диалоги и уведомления',
  'network:game': 'Использовать сеть игры',
  'storage:workspace': 'Хранить данные плагина',
  'storage:database': 'Хранить данные в базе плагина',
  'files:plugin': 'Работать с файлами плагина',
  'files:instance': 'Работать с файлами экземпляра',
  'files:host-read': 'Читать файлы компьютера',
  'files:host-write': 'Записывать файлы компьютера',
  'process:spawn': 'Запускать процессы',
  'chat:read': 'Читать чат и команды',
  'input:capture': 'Перехватывать ввод',
  'native:load': 'Загружать нативный код',
  'ui:compose': 'Регистрировать интерфейс, команды и сочетания клавиш',
  'events:launcher': 'Слушать события лаунчера',
  'runtime:observe': 'Читать runtime-метаданные',
  'lifecycle:observe': 'Наблюдать lifecycle лаунчера',
  'process:observe': 'Читать метрики процессов',
  'files:launcher-root-read': 'Читать файлы launcher-root',
  'files:launcher-config-projected': 'Читать projected config лаунчера',
  'network:http': 'Делать HTTP-запросы',
  'network:https': 'Делать HTTPS-запросы',
  'network:websocket': 'Открывать WebSocket-сессии',
  'network:tcp-client': 'Открывать TCP client-сокеты',
  'network:udp-client': 'Открывать UDP client-сокеты',
  'network:proxy-control': 'Управлять proxy route policy',
  'network:traffic-observe': 'Наблюдать сетевой трафик',
  'network:traffic-intercept': 'Перехватывать сетевой трафик',
  'network:tunnel': 'Управлять туннелями',
  'os:open-url': 'Открывать URL через ОС',
  'os:open-file': 'Открывать файлы через ОС',
  'os:reveal-in-file-manager': 'Показывать файлы в проводнике',
  'os:clipboard-read': 'Читать буфер обмена',
  'os:clipboard-write': 'Записывать в буфер обмена',
  'os:spawn-external': 'Запускать внешние приложения',
  'os:browser-session': 'Управлять browser session',
  'auth:oauth': 'Запускать OAuth-потоки',
  'auth:token-use': 'Использовать токены доступа',
  'secrets:store': 'Хранить секреты',
  'secrets:use': 'Использовать секреты',
  'ai:invoke-cloud': 'Вызывать cloud AI',
  'ai:invoke-local': 'Вызывать local AI',
  'downloads:manage': 'Управлять загрузками',
  'updates:manage': 'Управлять обновлениями',
  'dependencies:resolve': 'Разрешать зависимости',
  'integrity:scan': 'Проверять целостность',
  'metrics:emit': 'Публиковать метрики',
  'logs:write': 'Записывать логи',
  'presence:discord': 'Управлять Discord presence',
  'scheduler:jobs': 'Запускать фоновые jobs',
  'commands:register': 'Регистрировать команды',
  'shortcuts:register': 'Регистрировать shortcuts',
  'instances:read': 'Читать список экземпляров',
  'instances:mutate': 'Изменять экземпляры',
  'mods:read': 'Читать моды и ресурсы',
  'mods:install': 'Устанавливать моды и ресурсы',
  'packs:manage': 'Управлять паками и сборками',
  'accounts:projected-read': 'Читать projected account data',
  'backup:export': 'Экспортировать резервные копии',
  'backup:restore': 'Восстанавливать резервные копии',
  'sync:cloud': 'Синхронизировать с облаком',
  'ipc:sidecar': 'Обмениваться с sidecar-процессами',
  'policy:read': 'Читать policy-конфигурацию',
  'crypto:sign': 'Подписывать данные',
  'crypto:verify': 'Проверять подписи',
}

export const permissionDescriptions: Record<PluginPermission, string> = {
  'links:open': 'Только http(s)-ссылки через системный браузер.',
  'network:fetch': 'Запросы к разрешённым доменам из манифеста.',
  'network:dns': 'DNS-резолв только разрешённых доменов из манифеста.',
  'network:ping': 'TCP latency и проверка порта только разрешённых доменов.',
  'network:listen': 'Зарезервировано для встроенных LAN-комнат; runtime plugin API не предоставляет это разрешение.',
  'launcher:servers': 'Изолированный список игровых серверов внутри namespace плагина.',
  'launcher:dialogs': 'Показывать диалоги и короткие уведомления лаунчера.',
  'network:game': 'Сетевые соединения текущего игрового процесса.',
  'storage:workspace': 'Изолированное хранилище с namespace плагина.',
  'storage:database': 'SQLite-база внутри каталога плагина.',
  'files:plugin': 'Только файлы внутри каталога текущего плагина.',
  'files:instance': 'Файлы текущего экземпляра после отдельного согласия.',
  'files:host-read': 'Чтение файлов компьютера; высокорисковое разрешение.',
  'files:host-write': 'Запись файлов компьютера; высокорисковое разрешение.',
  'process:spawn': 'Запуск процессов; высокорисковое разрешение.',
  'chat:read': 'Чтение игрового чата; высокорисковое разрешение.',
  'input:capture': 'Перехват клавиатуры/мыши; всегда требует отдельного согласия.',
  'native:load': 'Загрузка JNI/нативного кода; высокорисковое разрешение.',
  'ui:compose': 'Декларативный UI без HTML, команды, сочетания клавиш и собственные theme tokens.',
  'events:launcher': 'Запуск, экземпляры, аккаунты и состояние окна.',
  'runtime:observe': 'Доступ к runtime info, capability negotiation и quota telemetry.',
  'lifecycle:observe': 'Наблюдение за lifecycle host/runtime без права на изменение.',
  'process:observe': 'Получение process metrics выбранного экземпляра.',
  'files:launcher-root-read': 'Read-only доступ к файлам launcher-root через отдельный scope.',
  'files:launcher-config-projected': 'Доступ только к очищенным projected schema лаунчера.',
  'network:http': 'Host-mediated HTTP без raw socket доступа.',
  'network:https': 'Host-mediated HTTPS с allowlist-проверкой доменов.',
  'network:websocket': 'Управление WebSocket-соединениями через host policy.',
  'network:tcp-client': 'Только клиентские TCP-соединения, без listen-сокетов.',
  'network:udp-client': 'Только клиентские UDP-соединения, без произвольного bind/listen.',
  'network:proxy-control': 'Чтение и изменение proxy route policies в namespace плагина.',
  'network:traffic-observe': 'Telemetry/наблюдение за трафиком без модификации пакетов.',
  'network:traffic-intercept': 'Высокорисковый interception-трафика, требует явного grant.',
  'network:tunnel': 'Управление tunnel-сессиями под контролем лаунчера.',
  'os:open-url': 'Открытие URL через системный обработчик без прямого shell доступа.',
  'os:open-file': 'Открытие локального файла системным приложением.',
  'os:reveal-in-file-manager': 'Показ локального пути в файловом менеджере.',
  'os:clipboard-read': 'Чтение буфера обмена; чувствительное разрешение.',
  'os:clipboard-write': 'Запись в буфер обмена пользователя.',
  'os:spawn-external': 'Запуск внешних приложений; высокорисковое разрешение.',
  'os:browser-session': 'Управление выделенной browser session через host bridge.',
  'auth:oauth': 'OAuth redirect/login flows под контролем host policy.',
  'auth:token-use': 'Использование уже выданных access tokens без прямого раскрытия секретов.',
  'secrets:store': 'Сохранение секретов во внутреннем secure store.',
  'secrets:use': 'Использование сохранённых секретов без раскрытия plugin storage.',
  'ai:invoke-cloud': 'Вызовы cloud AI-провайдеров через host mediation.',
  'ai:invoke-local': 'Вызовы локальных AI backends, если они доступны.',
  'downloads:manage': 'Управление job-ами скачивания и очередями.',
  'updates:manage': 'Управление обновлением лаунчера, пакетов и зависимостей.',
  'dependencies:resolve': 'Разрешение graph зависимостей и advisory-проверок.',
  'integrity:scan': 'Вычисление и проверка integrity/hash metadata.',
  'metrics:emit': 'Публикация metrics в host telemetry pipeline.',
  'logs:write': 'Запись структурированных логов от имени плагина.',
  'presence:discord': 'Обновление Discord Rich Presence через host adapter.',
  'scheduler:jobs': 'Регистрация периодических и отложенных job-ов.',
  'commands:register': 'Регистрация host-level commands из декларативного контракта.',
  'shortcuts:register': 'Регистрация keyboard shortcuts в разрешённой области UI.',
  'instances:read': 'Чтение списка экземпляров и их публичных свойств.',
  'instances:mutate': 'Изменение экземпляров; потенциально опасное разрешение.',
  'mods:read': 'Чтение установленных модов, паков и metadata.',
  'mods:install': 'Установка/обновление модов и сопутствующих ресурсов.',
  'packs:manage': 'Управление modpack/workspace пакетами.',
  'accounts:projected-read': 'Доступ только к projected account fields без секретов.',
  'backup:export': 'Экспорт резервных копий во внешние артефакты.',
  'backup:restore': 'Восстановление резервных копий; операция с риском потери данных.',
  'sync:cloud': 'Синхронизация артефактов и state с cloud backend.',
  'ipc:sidecar': 'Связь с sidecar-процессами через ограниченный IPC bridge.',
  'policy:read': 'Чтение host policy, ограничений и capability matrix.',
  'crypto:sign': 'Подписание данных host-managed ключами.',
  'crypto:verify': 'Проверка цифровых подписей и attestations.',
}

export const THEME_SCHEMA_VERSION = 1 as const

const themeModes: ThemeMode[] = ['system', 'light', 'dark', 'glass']
const themeDensities: DensityMode[] = ['compact', 'comfortable', 'spacious']
const themeSidebarModes: SidebarMode[] = ['expanded', 'adaptive', 'compact']
const themeIdPattern = /^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$/
const themeColorPattern = /^#[0-9a-f]{6}$/i

function themeText(value: unknown, field: string, maxLength: number): string {
  const text = String(value ?? '').trim()
  if (!text || text.length > maxLength) throw new Error(`Некорректное поле темы: ${field}`)
  return text
}

function themeColor(value: unknown, field: string): string {
  const color = String(value ?? '').trim().toLowerCase()
  if (!themeColorPattern.test(color)) throw new Error(`Цвет темы ${field} должен иметь формат #rrggbb`)
  return color
}

function themeEnum<T extends string>(value: unknown, field: string, values: readonly T[]): T {
  const normalized = String(value ?? '').trim().toLowerCase() as T
  if (!values.includes(normalized)) throw new Error(`Некорректное поле темы: ${field}`)
  return normalized
}

export function normalizeThemePreset(value: unknown, source: ThemeSource = 'shared'): LauncherThemePreset {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Пакет темы должен содержать объект')
  const raw = value as Record<string, unknown>
  if (Number(raw.schema_version ?? THEME_SCHEMA_VERSION) !== THEME_SCHEMA_VERSION) throw new Error('Неподдерживаемая версия схемы темы')
  const id = themeText(raw.id, 'id', 48).toLowerCase()
  if (!themeIdPattern.test(id)) throw new Error('id темы должен быть kebab-case')
  return {
    schema_version: THEME_SCHEMA_VERSION,
    id,
    name: themeText(raw.name, 'name', 80),
    author: themeText(raw.author, 'author', 120),
    source,
    description: themeText(raw.description, 'description', 500),
    mode: themeEnum(raw.mode, 'mode', themeModes),
    accent: themeColor(raw.accent, 'accent'),
    accent_strong: themeColor(raw.accent_strong, 'accent_strong'),
    background: themeColor(raw.background, 'background'),
    panel: themeColor(raw.panel, 'panel'),
    panel_soft: themeColor(raw.panel_soft, 'panel_soft'),
    density: themeEnum(raw.density, 'density', themeDensities),
    sidebar_mode: themeEnum(raw.sidebar_mode, 'sidebar_mode', themeSidebarModes),
    featured: raw.featured === true,
  }
}

const builtinThemeDefinitions = [
  {
    id: 'midnight-signal',
    schema_version: 1,
    name: 'Basalt & Brass',
    author: 'Nivora Launcher',
    source: 'builtin',
    description: 'Матовые графитовые поверхности и сдержанный латунный акцент.',
    mode: 'dark',
    accent: '#8b653f',
    accent_strong: '#c5a274',
    background: '#191a17',
    panel: '#20221e',
    panel_soft: '#30322c',
    density: 'comfortable',
    sidebar_mode: 'adaptive',
    featured: true,
  },
  {
    id: 'aurora-mint',
    schema_version: 1,
    name: 'Pine Shade',
    author: 'Nivora Launcher',
    source: 'builtin',
    description: 'Тёмная хвоя, каменные поверхности и мягкий шалфейный акцент.',
    mode: 'dark',
    accent: '#65734d',
    accent_strong: '#a7b18a',
    background: '#1b201c',
    panel: '#242a25',
    panel_soft: '#303830',
    density: 'comfortable',
    sidebar_mode: 'adaptive',
  },
  {
    id: 'paper-light',
    schema_version: 1,
    name: 'Limestone',
    author: 'Nivora Launcher',
    source: 'builtin',
    description: 'Тёплая светлая тема с каменными оттенками и спокойным зелёным акцентом.',
    mode: 'light',
    accent: '#65764b',
    accent_strong: '#4e6037',
    background: '#e9e8df',
    panel: '#f2f1e9',
    panel_soft: '#e2e1d7',
    density: 'comfortable',
    sidebar_mode: 'adaptive',
  },
  {
    id: 'glasshouse',
    schema_version: 1,
    name: 'Coal Slate',
    author: 'Nivora Launcher',
    source: 'builtin',
    description: 'Нейтральная тёмная тема с холодным серым и медным акцентом.',
    mode: 'dark',
    accent: '#7d684f',
    accent_strong: '#baa083',
    background: '#20211f',
    panel: '#292b28',
    panel_soft: '#353733',
    density: 'comfortable',
    sidebar_mode: 'adaptive',
  },
] as const

export const launcherThemes: LauncherThemePreset[] = builtinThemeDefinitions.map((theme) => normalizeThemePreset(theme, 'builtin'))

export const safeSyscalls: Array<{ name: string; description: string; permission: PluginPermission }> = [
  { name: 'launcher.openExternal', description: 'Открывает подтверждённую http(s)-ссылку вне лаунчера.', permission: 'links:open' },
  { name: 'network.fetch', description: 'Возвращает JSON только с разрешённого домена плагина.', permission: 'network:fetch' },
  { name: 'network.resolve', description: 'Разрешает DNS только для доменов из allowlist.', permission: 'network:dns' },
  { name: 'network.ping', description: 'Проверяет TCP-порт и latency без запуска shell-команд.', permission: 'network:ping' },
  { name: 'launcher.servers', description: 'Хранит server list только в namespace текущего плагина.', permission: 'launcher:servers' },
  { name: 'storage.namespace', description: 'Читает и пишет только пространство текущего плагина.', permission: 'storage:workspace' },
  { name: 'ui.mount', description: 'Рисует безопасные декларативные фрагменты без выполнения HTML/JS.', permission: 'ui:compose' },
  { name: 'launcher.subscribe', description: 'Подписывает плагин на белый список событий лаунчера.', permission: 'events:launcher' },
]

function assertPermission(manifest: PluginManifest, permission: PluginPermission) {
  if (!manifest.permissions.includes(permission)) throw new Error(`Плагину ${manifest.name} не выдано разрешение ${permission}`)
}

function safeUrl(value: string): URL {
  const url = new URL(value)
  if (!['http:', 'https:'].includes(url.protocol)) throw new Error('Разрешены только http(s)-ссылки')
  return url
}

function safeNetworkUrl(value: string): URL {
  const url = safeUrl(value)
  if (url.protocol !== 'https:' || url.username || url.password || url.port) {
    throw new Error('Сетевые запросы плагинам разрешены только по HTTPS без credentials и нестандартного порта')
  }
  return url
}

function sanitizeFragment(fragment: PluginUiFragment): PluginUiFragment {
  if (!fragment || typeof fragment !== 'object' || !['text', 'input', 'button', 'form', 'list'].includes(fragment.kind)) {
    throw new Error('Неподдерживаемый UI-фрагмент плагина')
  }
  return Object.freeze({ ...fragment })
}

export function createSafePluginHost(manifest: PluginManifest, allowedHosts: string[] = []) {
  const storagePrefix = `nivora-launcher:plugin:${manifest.id}:`
  const hosts = new Set((allowedHosts.length ? allowedHosts : manifest.network_hosts || []).map((host) => host.toLowerCase().replace(/\.$/, '')))
  return {
    openExternal(value: string) {
      assertPermission(manifest, 'links:open')
      window.open(safeUrl(value).toString(), '_blank', 'noopener,noreferrer')
    },
    async fetchJson<T>(value: string, init?: RequestInit): Promise<T> {
      assertPermission(manifest, 'network:fetch')
      const url = safeNetworkUrl(value)
      const host = url.hostname.toLowerCase().replace(/\.$/, '')
      if (!hosts.has(host)) throw new Error('Домен не входит в allowlist плагина')
      const response = await fetch(url, { ...init, headers: { Accept: 'application/json', ...(init?.headers || {}) } })
      if (!response.ok) throw new Error(`Сервис вернул ${response.status}`)
      return response.json() as Promise<T>
    },
    storage: {
      get<T>(key: string): T | undefined {
        assertPermission(manifest, 'storage:workspace')
        const raw = localStorage.getItem(`${storagePrefix}${key}`)
        return raw ? (JSON.parse(raw) as T) : undefined
      },
      set<T>(key: string, value: T) {
        assertPermission(manifest, 'storage:workspace')
        localStorage.setItem(`${storagePrefix}${key}`, JSON.stringify(value))
      },
      remove(key: string) {
        assertPermission(manifest, 'storage:workspace')
        localStorage.removeItem(`${storagePrefix}${key}`)
      },
    },
    compose(fragment: PluginUiFragment): Readonly<PluginUiFragment> {
      assertPermission(manifest, 'ui:compose')
      return sanitizeFragment(fragment)
    },
  }
}
