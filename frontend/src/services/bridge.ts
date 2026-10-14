import { convertFileSrc, invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type {
  AccountInfo,
  AnimationSettings,
  AppState,
  BedrockContentItem,
  BedrockScreenshotItem,
  BedrockVersionsResult,
  BedrockSetupStatus,
  BedrockRuntimeStatus,
  BackendEvent,
  InstanceDetails,
  InstanceFile,
  InstanceFileReadResult,
  InstanceLog,
  InstanceLogArchive,
  InstanceLogArchiveReadResult,
  InstanceInfo,
  InstanceSettings,
  LoaderChoice,
  LoaderId,
  ModAuditPrepareResult,
  ModAuditReport,
  ModResult,
  ModpackDependencyRequirements,
  OverlayState,
  PluginManifest,
  ProjectType,
  SettingsInfo,
  CloudFile,
  CloudStatus,
  CatalogSort,
  RecordingStatus,
  ExternalInstanceCandidate,
  SecurityStatus,
  SkinProfile,
  SkinInfo,
  VersionInfo,
  PluginCatalog,
  ServerConfig,
  ServerCore,
  ServerDetails,
  ServerFile,
  ServerMetrics,
  SettingsProfile,
  WorldBackup,
  BackupSchedule,
  SystemMetricsSnapshot,
  ModDependencyGraph,
  CrashReport,
  ResourceUpdate,
  LanRoomCredentials,
  LanRoomStatus,
  GitHubPluginDetails,
  GitHubPluginRepository,
  GitHubThemeRepository,
  LauncherThemePreset,
  ThemeArchiveMetadata,
  ServerPluginUpdate,
  ServerPluginResult,
  ServerSnapshot,
  ServerStatus,
  ServerNetworkMapping,
  ServerNetworkReport,
  PluginPermission,
} from '../types'
import type { PluginFileInfo, PluginNetworkRequest } from '../../../plugins/api'
import type {
  HttpRequest,
  HttpResponse,
  PermissionsSummary,
  ProcessMetricsResponse,
  ProxyRoutePolicy,
  RuntimeInfo,
} from '../../../plugins/api-v3'
import type { PluginServer, PluginServerInput } from '../plugins/api'
import type { RuntimePluginManifest } from '../plugins/runtimeManifest'

interface BackendEnvelope<T> {
  ok: boolean
  data?: T
  error?: string
}

const fallbackSettings: SettingsInfo = {
  game_dir: '~/.local/share/nivora-launcher/game',
  java_path: '',
  security_profile: 'strict',
  security_permissions: ['files:instance', 'desktop:display', 'desktop:audio'],
  memory: '8GiB',
  memory_kib: 8 * 1024 * 1024,
  memory_total_kib: 16 * 1024 * 1024,
  memory_recommended_kib: 8 * 1024 * 1024,
  max_downloads: 8,
  startup_view: 'instances',
  last_view: 'instances',
  remember_last_view: true,
  default_version_filter: 'release',
  default_loader: 'fabric',
  default_account_id: 'auto',
  open_last_instance: true,
  launch_after_install: false,
  auto_refresh_metadata: true,
  metadata_refresh_hours: 6,
  download_timeout_seconds: 30,
  download_retries: 3,
  confirm_destructive_actions: true,
  show_activity_panel: true,
  show_toasts: true,
  minimize_on_launch: false,
  language: null as string | null,
  font_family: 'system',
  keep_logs_days: 14,
  telemetry: false,
  telemetry_endpoint: '',
  keyboard_shortcuts: { open_settings: 'Ctrl+,', refresh: 'Ctrl+R', new_instance: 'Ctrl+N', focus_search: 'Ctrl+K', toggle_activity: 'Ctrl+J' },
  graphics_mode: 'auto',
  launcher_profiles: [{ id: 'default', name: 'По умолчанию', memory: 'auto', java_path: '', graphics_mode: 'auto', account_id: 'auto', window_width: 1280, window_height: 720, fullscreen: false, quick_play_world: '', quick_play_server: '', jvm_arguments: [], game_arguments: [] }],
  active_launcher_profile_id: 'default',
  http_proxy: '',
  has_client_id: false,
  developer_mode: false,
  theme: 'dark',
  active_theme_id: 'midnight-signal',
  wallpaper_path: '',
  wallpaper_data: '',
  wallpaper_blur: 12,
  wallpaper_opacity: 34,
  accent_color: '#8b653f',
  sidebar_position: 'left',
  sidebar_mode: 'adaptive',
  sidebar_width_mode: 'content',
  sidebar_width: 228,
  sidebar_floating: false,
  sidebar_visibility: 'always',
  animations: 'full',
  animation_settings: {
    page_style: 'fade',
    page_duration_ms: 140,
    surface_duration_ms: 140,
    surface_stagger_ms: 0,
    page_styles: { instances: 'fade', 'instance-settings': 'fade', accounts: 'fade', mods: 'fade', modpacks: 'fade', plugins: 'fade', 'lan-rooms': 'fade', themes: 'fade', settings: 'fade' },
    page_durations_ms: { instances: 140, 'instance-settings': 140, accounts: 140, mods: 140, modpacks: 140, plugins: 140, 'lan-rooms': 140, themes: 140, settings: 140 },
  } as AnimationSettings,
  overlay_settings: {
    enabled: true,
    hotkey_overlay: 'Shift+Tab',
    hotkey_recording: 'F7',
    show_mods: true,
    show_metrics: true,
    recording_fps: 60,
    recording_quality: 23,
    recording_encoder: 'auto',
    recording_container: 'mp4',
    recording_audio: true,
    position: 'top-left',
    scale: 100,
    opacity: 96,
    compact: false,
    show_clock: true,
    show_system_metrics: true,
    metrics_cpu: true,
    metrics_ram: true,
    metrics_gpu: true,
    metrics_disk: true,
    metrics_temperatures: true,
    system_metrics_display: 'overlay',
    show_chat: true,
  },
  backup_schedule: { enabled: false, interval_hours: 24, keep: 5, destination: 'local' },
  density: 'comfortable',
  ui_scale: 100,
  radius_tokens: { card: 7, button: 8, control: 10 },
    page_themes: { instances: 'inherit', 'instance-settings': 'inherit', accounts: 'inherit', mods: 'inherit', modpacks: 'inherit', plugins: 'inherit', 'lan-rooms': 'inherit', themes: 'inherit', settings: 'inherit' },
    page_layouts: { instances: 'balanced', 'instance-settings': 'balanced', accounts: 'balanced', mods: 'wide', modpacks: 'wide', plugins: 'wide', 'lan-rooms': 'balanced', themes: 'wide', settings: 'balanced' },
  widget_visibility: { 'instances.workspace': true, 'accounts.status': true, 'mods.context': true },
  color_tokens: {
    accent: '#8b653f', accent_strong: '#c5a274', background: '#191a17', panel: '#20221e', panel_raised: '#282a25', panel_soft: '#30322c',
    text: '#e9e8de', muted: '#a2a395', quiet: '#c7c7bb', border: '#77796b', danger: '#d58a7d', success: '#9bac78', warning: '#d2ad72', banner_shade: '#191a17',
  },
  instance_groups: [],
  pinned_instance_groups: [],
  login_source: 'microsoft',
  login_api_url: 'https://login.microsoftonline.com/consumers/oauth2/v2.0',
  catalog_source: 'modrinth',
  catalog_api_url: 'https://api.modrinth.com/v2',
  data_source: 'mojang',
  data_api_url: 'https://piston-meta.mojang.com/mc/game/version_manifest_v2.json',
  resource_catalogs: [
    { id: 'modrinth', name: 'Modrinth', kind: 'modrinth', api_url: 'https://api.modrinth.com/v2' },
    { id: 'ftb', name: 'Feed The Beast', kind: 'ftb', api_url: 'https://api.feed-the-beast.com/v1/modpacks/public' },
    { id: 'technic', name: 'Technic', kind: 'technic', api_url: 'https://api.technicpack.net' },
  ],
  active_catalog_id: 'modrinth',
  plugin_catalogs: [
    { id: 'modrinth-plugins', name: 'Modrinth', kind: 'modrinth', api_url: 'https://api.modrinth.com/v2', enabled: true },
    { id: 'spigot', name: 'Spigot', kind: 'spigot', api_url: 'https://api.spiget.org/v2', enabled: true },
    { id: 'hangar', name: 'Hangar', kind: 'hangar', api_url: 'https://hangar.papermc.io/api/v1', enabled: true },
  ],
  active_plugin_catalog_id: 'modrinth-plugins',
  cloud_url: '',
  cloud_username: '',
  cloud_remote_path: 'nivora-launcher',
  cloud_include_accounts: false,
  cloud_last_sync: 0,
  cloud_configured: false,
}

const fallbackState: AppState = {
  instances: [],
  accounts: [],
  plugins: [],
  server_cores: [
    { id: 'paper', name: 'Paper', family: 'PaperMC', description: 'Быстрое стабильное ядро для Paper API.' },
    { id: 'purpur', name: 'Purpur', family: 'PaperMC', description: 'Paper-совместимое ядро с расширенными настройками.' },
    { id: 'folia', name: 'Folia', family: 'PaperMC', description: 'Региональный параллельный server runtime.' },
    { id: 'spigot', name: 'Spigot', family: 'Bukkit', description: 'Классическое Bukkit-совместимое ядро.' },
    { id: 'leaf', name: 'Leaf', family: 'Paper', description: 'Paper fork с оптимизациями.' },
    { id: 'pufferfish', name: 'Pufferfish', family: 'Paper', description: 'Оптимизированное Paper-совместимое ядро.' },
    { id: 'patina', name: 'Patina', family: 'Paper', description: 'Производительный Paper-based дистрибутив.' },
    { id: 'velocity', name: 'Velocity', family: 'Proxy', description: 'Прокси-ядро для сети серверов.' },
    { id: 'waterfall', name: 'Waterfall', family: 'Proxy', description: 'BungeeCord-совместимое прокси-ядро.' },
    { id: 'custom', name: 'Своё ядро', family: 'Custom', description: 'Загрузите любой server.jar.' },
  ],
  plugin_catalogs: fallbackSettings.plugin_catalogs,
  active_plugin_catalog_id: fallbackSettings.active_plugin_catalog_id,
  settings: fallbackSettings,
  system: { platform: 'Linux', machine: 'x86_64', python: '3.13', session: 'wayland', display: 'preview', nvidia_smi: true },
}

const mockInstanceSettings = new Map<string, InstanceSettings>()

function defaultMockInstanceSettings(): InstanceSettings {
  return {
    memory: 'auto',
    java_path: '',
    graphics_mode: 'auto',
    window_width: 1280,
    window_height: 720,
    fullscreen: false,
    quick_play_world: '',
    quick_play_server: '',
    jvm_arguments: [],
    game_arguments: [],
    jvm_preset: 'none',
    console_colors: { error: '#ff657a', info: '#e6edf7', warning: '#f5c451' },
    security_profile: 'strict',
    security_permissions: ['files:instance', 'desktop:display', 'desktop:audio'],
  }
}

export function inTauri(): boolean {
  return typeof window !== 'undefined' && Boolean(window.__TAURI_INTERNALS__)
}

// Метка сборки интерфейса; бэкенд присылает свою в bootstrap (build_stamp).
// Расхождение = в приложение подменили только Python-бэкенд (или наоборот),
// и половина операций больше не согласована — показываем баннер, а не гадаем.
export const FRONTEND_BUILD_STAMP = 'r38'

function requestTimeout(operation: string): number | undefined {
  // bootstrap на холодном старте сканирует каталоги экземпляров: на
  // медленных дисках и больших библиотеках 10 секунд не хватало, и
  // пользователь видел «Backend timeout: bootstrap» на ровном месте
  if (operation === 'bootstrap') return 30000
  if (operation === 'bedrock_runtime_status') return 60000
  if (operation === 'bedrock_versions') return 120000
  // Установка Bedrock и вход в Microsoft идут в фоновой нити бэкенда:
  // сами операции стартуют мгновенно, состояние опрашивается отдельно
  if (operation === 'bedrock_setup_version' || operation === 'bedrock_store_login') return 30000
  if (operation === 'bedrock_setup_status' || operation === 'bedrock_store_login_status' || operation === 'bedrock_setup_cancel') return 15000
  if (operation === 'bedrock_create_instance') return 120000
  if (operation === 'bedrock_content_import') return 10 * 60 * 1000
  if (operation === 'server_restart') return 180000
  if (operation === 'skin_profile' || operation === 'server_network_diagnose' || operation === 'server_network_open') return 120000
  if (operation === 'server_network_close' || operation === 'server_network_prepare') return 30000
  if (operation === 'versions' || operation === 'loader_choices' || operation === 'skins_list' || operation === 'java_options' || operation === 'java_validate' || operation === 'microsoft_login_status') return 30000
  if (operation === 'instance_details' || operation === 'instance_log' || operation === 'instance_file_list' || operation === 'instance_file_read' || operation === 'instance_file_write' || operation === 'instance_file_mkdir' || operation === 'instance_file_delete' || operation === 'instance_log_archives' || operation === 'instance_log_archive_read' || operation === 'graphics_diagnostics' || operation === 'server_details' || operation === 'server_log') return 15000
  if (operation === 'external_instances_scan') return 30000
  if (operation === 'external_instance_import') return 2 * 60 * 1000
  if (operation === 'server_plugin_search' || operation === 'server_plugin_details' || operation === 'server_plugin_updates' || operation === 'server_core_versions' || operation === 'mods_details' || operation === 'mods_check_updates') return 30000
  // Catalog search must always resolve: the backend worker is serial, and without a
  // deadline a stuck request would leave the search spinner spinning forever.
  if (operation === 'mods_search') return 60000
  if (operation === 'login_microsoft') return 15 * 60 * 1000
  if (operation === 'instance_file_upload') return 30000
  if (operation === 'overlay_state') return 20000
  if (operation === 'recording_start' || operation === 'recording_stop') return 45000
  if (operation === 'recording_status') return 15000
  if (operation === 'server_plugin_install' || operation === 'server_plugin_update' || operation === 'server_file_upload' || operation === 'server_upload_core' || operation === 'server_core_download' || operation === 'mods_update') return 10 * 60 * 1000
  if (operation === 'mod_audit_prepare') return 5 * 60 * 1000
  if (operation === 'mod_audit_accept') return 30 * 60 * 1000
  return undefined
}

const activeOperationIds = new Set<string>()

function operationId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') return crypto.randomUUID()
  return `operation-${Date.now()}-${Math.random().toString(16).slice(2)}`
}

function abortError(): Error {
  const error = new Error('Backend request aborted')
  error.name = 'AbortError'
  return error
}

async function invokeBackend<T>(body: Record<string, unknown>, timeout: number | undefined, signal?: AbortSignal): Promise<T> {
  if (signal?.aborted) throw abortError()
  const pending = invoke<T>('backend_call', { request: body })
  let timer: number | undefined
  let abortListener: (() => void) | undefined
  const requests: Promise<T>[] = [pending]
  if (timeout) {
    requests.push(new Promise<never>((_, reject) => {
      timer = window.setTimeout(() => {
        const id = typeof body.operation_id === 'string' ? body.operation_id : ''
        if (id && inTauri()) void invoke('cancel_operation', { operationId: id }).catch(() => undefined)
        reject(new Error(`Backend timeout: ${String(body.operation)}`))
      }, timeout)
    }))
  }
  if (signal) {
    requests.push(new Promise<never>((_, reject) => {
      abortListener = () => {
        const id = typeof body.operation_id === 'string' ? body.operation_id : ''
        if (id && inTauri()) void invoke('cancel_operation', { operationId: id }).catch(() => undefined)
        reject(abortError())
      }
      signal.addEventListener('abort', abortListener, { once: true })
    }))
  }
  try {
    return await Promise.race(requests)
  } finally {
    if (timer) window.clearTimeout(timer)
    if (abortListener) signal?.removeEventListener('abort', abortListener)
  }
}

async function request<T>(operation: string, payload: Record<string, unknown> = {}, signal?: AbortSignal): Promise<T> {
  if (signal?.aborted) throw abortError()
  const id = operationId()
  activeOperationIds.add(id)
  const body = { operation, operation_id: id, ...payload }
  try {
    if (inTauri()) {
      let response: BackendEnvelope<T>
      try {
        response = await invokeBackend<BackendEnvelope<T>>(body, requestTimeout(operation), signal)
      } catch (error) {
        const message = error instanceof Error ? error.message : typeof error === 'string' ? error : JSON.stringify(error)
        throw new Error(message || 'Backend request failed')
      }
      if (!response.ok) {
        throw new Error(response.error || 'Backend request failed')
      }
      return response.data as T
    }
    return mockRequest<T>(operation, payload)
  } finally {
    activeOperationIds.delete(id)
  }
}

function fallbackServerConfig(server?: InstanceInfo): ServerConfig {
  return {
    core: server?.server_core || 'paper',
    version: server?.server_core_version || server?.minecraft_version || '1.21.1',
    build: '',
    port: server?.server_port || 25565,
    max_players: 20,
    motd: server?.display_name || server?.name || 'Demo server',
    memory: '4GiB',
    java_path: '',
    security_profile: 'strict',
    security_permissions: ['network:game', 'files:instance'],
    eula_accepted: Boolean(server?.server_eula),
    auto_restart: false,
    restart_schedule: { enabled: false, time: '04:00', warn_minutes: 1, last_run: '' },
    snapshot_retention_hours: 5,
    snapshot_max_count: 1,
  }
}

function fallbackServerStatus(name: string, running: boolean): ServerStatus {
  const server = fallbackState.instances.find((item) => item.name === name)
  const config = fallbackServerConfig(server)
  const now = Date.now() / 1000
  const metrics: ServerMetrics = {
    timestamp: now,
    pid: 0,
    running,
    uptime_seconds: 0,
    ram_bytes: 0,
    cpu_percent: 0,
    storage_bytes: 0,
    players: 0,
    max_players: config.max_players,
    tps: running ? 20 : 0,
  }
  return { running, pid: 0, started_at: running ? now : 0, players: 0, max_players: config.max_players, tps: metrics.tps, uptime_seconds: 0, metrics, last_metrics: {} }
}

async function mockRequest<T>(operation: string, payload: Record<string, unknown>): Promise<T> {
  await new Promise((resolve) => window.setTimeout(resolve, 220))
  if (operation === 'bootstrap') return structuredClone(fallbackState) as T
  if (operation === 'create_offline_account') {
    const username = String(payload.username || 'Player').trim().replace(/[^\p{L}\p{N}_-]/gu, '').slice(0, 16)
    if (!username) throw new Error('Имя debug-аккаунта пустое')
    const previous = fallbackState.accounts.find((account) => account.account_id === 'offline-debug')
    const account: AccountInfo = {
      account_id: 'offline-debug',
      kind: 'offline',
      username,
      alias: '',
      display_name: username,
      expires_at: 0,
      limit_seconds: 0,
      used_seconds: 0,
      is_default: previous?.is_default ?? fallbackState.accounts.length === 0,
    }
    fallbackState.accounts = [...fallbackState.accounts.filter((value) => value.account_id !== account.account_id), account]
    return { account } as T
  }
  if (operation === 'plugin_install' || operation === 'plugin_set_enabled' || operation === 'plugin_uninstall') {
    throw new Error('Управление плагинами доступно только в desktop launcher')
  }
  if (operation.startsWith('lan_session_') || operation.startsWith('github_plugin_')) {
    throw new Error('Эта функция доступна только в установленном desktop launcher')
  }
  if (operation === 'plugin_network_dns') return { host: String(payload.host || ''), port: payload.port ?? null, addresses: [] } as T
  if (operation === 'plugin_network_ping' || operation === 'plugin_network_port_check') {
    return { host: String(payload.host || ''), port: Number(payload.port || 25565), reachable: false, latency_ms: 0, error: 'Диагностика доступна только в desktop launcher' } as T
  }
  if (operation === 'plugin_server_list_get' || operation === 'plugin_server_list_save') return { servers: [] } as T
  if (operation === 'plugin_server_list_add') return { server: payload.server, servers: [payload.server] } as T
  if (operation === 'plugin_server_list_remove') return { removed: false, servers: [] } as T
  if (operation === 'save_settings') {
    const values = JSON.parse(JSON.stringify(payload.values || {})) as Record<string, unknown>
    Object.assign(fallbackState.settings, values)
    return { settings: structuredClone(fallbackState.settings) } as T
  }
  if (operation === 'cloud_status') return { status: { provider: 'webdav', configured: false, endpoint: '', username: '', remote_path: 'nivora-launcher', include_accounts: false, last_sync: 0 } } as T
  if (operation === 'cloud_configure') {
    const values = payload as { url?: string; username?: string; remote_path?: string; include_accounts?: boolean }
    Object.assign(fallbackState.settings, {
      cloud_url: values.url || '',
      cloud_username: values.username || '',
      cloud_remote_path: values.remote_path || 'nivora-launcher',
      cloud_include_accounts: Boolean(values.include_accounts),
    })
    return { status: { provider: 'webdav', configured: false, endpoint: fallbackState.settings.cloud_url, username: fallbackState.settings.cloud_username, remote_path: fallbackState.settings.cloud_remote_path, include_accounts: fallbackState.settings.cloud_include_accounts, last_sync: 0 }, settings: structuredClone(fallbackState.settings) } as T
  }
  if (operation === 'cloud_test' || operation === 'cloud_list' || operation === 'cloud_sync' || operation === 'cloud_restore' || operation === 'cloud_delete') {
    return { status: { provider: 'webdav', configured: false, endpoint: fallbackState.settings.cloud_url, username: fallbackState.settings.cloud_username, remote_path: fallbackState.settings.cloud_remote_path, include_accounts: fallbackState.settings.cloud_include_accounts, last_sync: fallbackState.settings.cloud_last_sync }, files: [] } as T
  }
  if (operation === 'cloud_disconnect') {
    Object.assign(fallbackState.settings, { cloud_url: '', cloud_username: '', cloud_remote_path: 'nivora-launcher', cloud_include_accounts: false, cloud_last_sync: 0 })
    return { status: { provider: 'webdav', configured: false, endpoint: '', username: '', remote_path: 'nivora-launcher', include_accounts: false, last_sync: 0 }, settings: structuredClone(fallbackState.settings) } as T
  }
  if (operation === 'instance_groups') return { groups: [...fallbackState.settings.instance_groups] } as T
  if (operation === 'external_instances_scan') return { candidates: [] } as T
  if (operation === 'external_instance_import') throw new Error('Импорт экземпляров доступен только в desktop launcher')
  if (operation === 'bedrock_runtime_status') return { status: { available: false, supported: false, version: '', message: 'Minecraft Bedrock доступен только в desktop launcher.' } } as T
  if (operation === 'bedrock_versions') return { versions: [], current_edition: '', current_version: '' } as T
  if (operation === 'bedrock_content_list') return { items: [] as BedrockContentItem[] } as T
  if (operation.startsWith('bedrock_')) throw new Error('Minecraft Bedrock доступен только в desktop launcher')
  if (operation === 'create_instance_group') {
    const name = String(payload.name || '').trim().slice(0, 40)
    if (!name) throw new Error('Название группы не может быть пустым')
    if (!fallbackState.settings.instance_groups.includes(name)) fallbackState.settings.instance_groups.push(name)
    return { groups: [...fallbackState.settings.instance_groups] } as T
  }
  if (operation === 'delete_instance_group') {
    const name = String(payload.name || '').trim()
    if (fallbackState.instances.some((instance) => instance.group === name)) {
      throw new Error('Нельзя удалить группу, пока в ней есть экземпляры')
    }
    fallbackState.settings.instance_groups = fallbackState.settings.instance_groups.filter((group) => group !== name)
    return { groups: [...fallbackState.settings.instance_groups] } as T
  }
  if (operation === 'update_instance_profile') {
    const name = String(payload.name || '')
    const instance = fallbackState.instances.find((item) => item.name === name)
    if (!instance) throw new Error(`Экземпляр не найден: ${name}`)
    const group = String(payload.group || '').trim()
    if (group && !fallbackState.settings.instance_groups.includes(group)) {
      throw new Error('Сначала создайте выбранную группу')
    }
    instance.display_name = String(payload.display_name || instance.name).trim() || instance.name
    instance.description = String(payload.description || '').trim()
    instance.group = group
    return { instance, groups: [...fallbackState.settings.instance_groups] } as T
  }
  if (operation === 'instance_details') {
    const name = String(payload.name || '')
    const instance = fallbackState.instances.find((item) => item.name === name)
    if (!instance) throw new Error(`Экземпляр не найден: ${name}`)
    const settings = mockInstanceSettings.get(name) || defaultMockInstanceSettings()
    return {
      instance,
      settings: structuredClone(settings),
      assets: { mods: [], resourcepacks: [], shaders: [] },
      worlds: [],
      servers: [],
      snapshots: { mods: [], resourcepacks: [], shaders: [], saves: [] },
    } as T
  }
  if (operation === 'save_instance_settings') {
    const name = String(payload.name || '')
    const current = mockInstanceSettings.get(name) || defaultMockInstanceSettings()
    const values = (payload.values || {}) as Partial<InstanceSettings>
    const settings: InstanceSettings = {
      ...current,
      ...values,
      console_colors: {
        ...current.console_colors,
        ...values.console_colors,
      },
    }
    mockInstanceSettings.set(name, settings)
    return { settings: structuredClone(settings) } as T
  }
  if (operation === 'versions') {
    return {
      versions: ['1.21.4', '1.21.1', '24w14a', '1.20.1', 'b1.8.1', 'a1.2.6', '1.16.5'].map((id) => ({
        id,
        type: id.startsWith('a') ? 'old_alpha' : id.startsWith('b') ? 'old_beta' : id.includes('w') ? 'snapshot' : 'release',
        release_time: '',
      })),
    } as T
  }
  if (operation === 'java_options') {
    return {
      options: [{ value: 'auto', label: 'Автоматически', meta: 'Mojang runtime → системная Java' }],
    } as T
  }
  if (operation === 'security_status') {
    return {
      platform: 'linux',
      bubblewrap: false,
      bubblewrap_path: '',
      strict_available: false,
      profiles: ['compat', 'off', 'manual', 'strict'],
      permissions: ['network:game', 'files:instance', 'files:host-read', 'files:host-write', 'process:spawn', 'chat:read', 'input:capture', 'native:load'],
    } as T
  }
  if (operation === 'java_validate') return { path: String(payload.path || ''), version: 21 } as T
  if (operation === 'loader_choices') {
    return {
      choices: [
        { loader: payload.loader as LoaderId, version: 'latest', installer_url: '' },
      ],
    } as T
  }
  if (operation === 'install') {
    const version = String(payload.version || '1.21.1')
    const requestedLoader = String(payload.loader || 'vanilla')
    const loader = (['vanilla', 'fabric', 'forge', 'neoforge', 'quilt'].includes(requestedLoader) ? requestedLoader : 'vanilla') as LoaderId
    const loaderVersion = String(payload.loader_version || 'latest')
    const name = String(payload.instance_name || `${loader}-${version}`)
    const versionId = loader === 'vanilla' ? version : `${loader}-${loaderVersion}-${version}`
    const instance: InstanceInfo = {
      name,
      display_name: name,
      description: '',
      group: '',
      icon: '',
      banner: '',
      played_seconds: 0,
      version_id: versionId,
      minecraft_version: version,
      loader,
      path: `~/.local/share/nivora-launcher/game/instances/${name}`,
      kind: 'client',
    }
    fallbackState.instances = [...fallbackState.instances.filter((item) => item.name !== name), instance]
    return { version_id: versionId, instance_dir: instance.path, instances: structuredClone(fallbackState.instances) } as T
  }
  if (operation === 'skins_list') return { skins: [] } as T
  if (operation === 'instance_log') return { lines: [], running: false, metrics: { timestamp: Date.now() / 1000, pid: 0, running: false, uptime_seconds: 0, ram_bytes: 0, cpu_percent: 0, storage_bytes: 0, players: 0, max_players: 0, tps: 0 } } as T
  if (operation === 'instance_file_list') return { files: [] as InstanceFile[] } as T
  if (operation === 'instance_file_read') return { content: '', encoding: payload.encoding === 'base64' ? 'base64' : 'text' } as InstanceFileReadResult as T
  if (operation === 'instance_file_write') return { saved: true } as T
  if (operation === 'instance_file_mkdir') return { created: true } as T
  if (operation === 'instance_file_upload') return { uploaded: true } as T
  if (operation === 'instance_file_delete') return { removed: true } as T
  if (operation === 'instance_log_archives') return { logs: [] as InstanceLogArchive[], next_offset: null } as T
  if (operation === 'instance_log_archive_read') return { content: '', next_offset: Number(payload.offset || 0), eof: true } as InstanceLogArchiveReadResult as T
  if (operation === 'microsoft_login_status') return { status: 'idle' } as T
  if (operation === 'mods_search') return { results: [] } as T
  if (operation === 'modpack_import') throw new Error('Импорт модпака доступен только в desktop launcher')
  if (operation === 'server_cores') return { cores: fallbackState.server_cores || [] } as T
  if (operation === 'server_core_versions') return { core: String(payload.core || 'custom'), versions: [] } as T
  if (operation === 'server_plugin_catalogs') return { catalogs: fallbackState.settings.plugin_catalogs, active_catalog_id: fallbackState.settings.active_plugin_catalog_id } as T
  if (operation === 'save_server_plugin_catalogs') return { catalogs: fallbackState.settings.plugin_catalogs, active_catalog_id: fallbackState.settings.active_plugin_catalog_id } as T
  if (operation === 'server_create') {
    const name = String(payload.name || 'demo-server')
    if (fallbackState.instances.some((item) => item.name === name)) {
      throw new Error('Это имя уже занято. Создание сервера не заменяет существующие экземпляры.')
    }
    const server: InstanceInfo = { name, display_name: String(payload.display_name || name), description: 'Серверный экземпляр', group: '', icon: '', banner: '', played_seconds: 0, version_id: '', minecraft_version: String(payload.version || '1.21.1'), loader: 'vanilla', path: `~/.local/share/nivora-launcher/game/instances/${name}`, kind: 'server', server_core: String(payload.core || 'paper'), server_core_version: String(payload.version || '1.21.1'), server_port: Number(payload.port || 25565), server_eula: Boolean(payload.eula_accepted) }
    fallbackState.instances = [...fallbackState.instances, server]
    return { server, instances: fallbackState.instances } as T
  }
  if (operation === 'server_save_config') {
    const server = fallbackState.instances.find((item) => item.name === payload.name)
    if (server) {
      const values = (payload.values || {}) as Partial<ServerConfig>
      Object.assign(server, { server_core: values.core || server.server_core, server_core_version: values.version || server.server_core_version, server_port: values.port || server.server_port, server_eula: values.eula_accepted ?? server.server_eula })
      return { config: { ...fallbackServerConfig(server), ...values }, server } as T
    }
  }
  if (operation.startsWith('server_network_')) throw new Error('Открытие сервера в интернет доступно только в desktop launcher')
  if (['server_start', 'server_stop', 'server_restart', 'server_command'].includes(operation)) throw new Error('Запуск и управление процессом сервера доступны только в desktop launcher; браузерный preview не запускает Java')
  if (operation === 'server_status') return fallbackServerStatus(String(payload.name || ''), false) as T
  if (operation === 'server_log') return { lines: [], running: false } as T
  if (operation === 'server_details') {
    const server = payload.name ? fallbackState.instances.find((item) => item.name === payload.name) : undefined
    return { server, config: fallbackServerConfig(server), memory_total_kib: 16 * 1024 * 1024, memory_limit_kib: 12 * 1024 * 1024, status: fallbackServerStatus(String(payload.name || ''), false), files: [], plugin_files: [], logs: { lines: [], running: false }, snapshots: { mods: [], plugins: [] } } as T
  }
  if (operation === 'server_file_list') return { files: [] } as T
  if (operation === 'server_file_read') return { content: '' } as T
  if (operation === 'server_core_download') return { download: { core: String(payload.core || 'paper'), version: String(payload.version || '1.21.1'), build: 'latest', path: 'server.jar', url: '' }, server: fallbackState.instances.find((item) => item.name === payload.name) } as T
  if (operation === 'server_snapshot_list' || operation === 'server_snapshot_create' || operation === 'server_snapshot_restore' || operation === 'server_snapshot_delete') return { snapshots: [] } as T
  if (operation === 'server_plugin_search') return { results: [] } as T
  if (operation === 'server_plugin_details') return { details: {}, catalog_id: String(payload.catalog_id || '') } as T
  if (operation === 'server_plugin_updates') return { updates: [], files: [] } as T
  if (operation === 'server_plugin_update') return { plugin: {}, files: [], catalog_id: String(payload.catalog_id || '') } as T
  if (operation === 'mods_details') return { details: {}, compatible_versions: [], catalog_id: String(payload.catalog_id || '') } as T
  if (operation === 'mods_check_updates') return { updates: [], project_type: String(payload.project_type || 'mod'), instance_name: String(payload.instance_name || ''), catalog_id: String(payload.catalog_id || '') } as T
  if (operation === 'mods_update') return { project_id: String(payload.project_id || ''), name: '', version: '', version_id: '', file_name: '', dependencies: [], details: {}, instance_name: String(payload.instance_name || '') } as T
  return {} as T
}

export const bridge = {
  pluginDevChanges: () => request<{ enabled: boolean; generation: string; requests: Record<string, number> }>('plugin_dev_changes'),
  pluginOperationTrace: (pluginId: string, method: string, args: Record<string, unknown>, signal?: AbortSignal, outcome: 'begin' | 'ok' | 'denied' = 'begin', elapsedMs = 0) => request('plugin_api_operation_trace', { plugin_id: pluginId, method, outcome, elapsed_ms: elapsedMs, event: typeof args.event === 'string' ? args.event : '', listener_id: typeof args.listener_id === 'string' ? args.listener_id : '', arguments: Object.fromEntries(Object.keys(args).slice(0, 64).map((key) => [key, '[redacted]'])) }, signal),
  cancelActiveOperations: async (): Promise<void> => {
    if (!inTauri()) return
    await Promise.all([...activeOperationIds].map((operation_id) => invoke('cancel_operation', { operationId: operation_id })))
  },
  subscribeEvents: async (onEvent: (event: BackendEvent) => void): Promise<UnlistenFn | undefined> => {
    if (!inTauri()) return undefined
    return listen<BackendEvent>('backend-event', (event) => onEvent(event.payload))
  },
  bootstrap: () => request<AppState>('bootstrap'),
  versions: (includeSnapshots = true) =>
    request<{ versions: VersionInfo[] }>('versions', { include_snapshots: includeSnapshots }),
  javaOptions: () => request<{ options: { value: string; label: string; meta?: string }[] }>('java_options'),
  javaValidate: (path: string) => request<{ path: string; version: number }>('java_validate', { path }),
  securityStatus: () => request<SecurityStatus>('security_status'),
  loaderChoices: (loader: LoaderId, minecraftVersion: string) =>
    request<{ choices: LoaderChoice[] }>('loader_choices', {
      loader,
      minecraft_version: minecraftVersion,
    }),
  saveSettings: (values: Partial<SettingsInfo> & Record<string, unknown>) =>
    request<{ settings: SettingsInfo }>('save_settings', { values }),
  cloudStatus: () => request<{ status: CloudStatus }>('cloud_status'),
  cloudConfigure: (values: { url: string; username: string; remote_path: string; include_accounts: boolean; password?: string }) =>
    request<{ status: CloudStatus; settings: SettingsInfo }>('cloud_configure', values),
  cloudTest: () => request<{ status: CloudStatus; files: CloudFile[] }>('cloud_test'),
  cloudList: () => request<{ status: CloudStatus; files: CloudFile[] }>('cloud_list'),
  cloudSync: () => request<{ result: { uploaded: number; bytes: number }; status: CloudStatus; files: CloudFile[] }>('cloud_sync'),
  cloudRestorePreview: () => request<{ result: { preview: true; manifest_id: string; changes: { key: string; action: string; size?: number; keys?: string[] }[]; skipped: number; files: number; bytes: number; settings: Record<string, unknown> }; status: CloudStatus }>('cloud_restore'),
  cloudRestore: (manifest_id: string) => request<{ result: { restored: number; bytes: number; skipped: number; manifest_id: string }; status: CloudStatus }>('cloud_restore', { confirmed: true, manifest_id }),
  cloudDelete: (key: string) => request<{ status: CloudStatus; files: CloudFile[] }>('cloud_delete', { key }),
  cloudDisconnect: () => request<{ status: CloudStatus; settings: SettingsInfo }>('cloud_disconnect'),
  createOfflineAccount: (username: string) =>
    request<{ account: AccountInfo }>('create_offline_account', { username }),
  loginMicrosoft: () => request<{ account: AccountInfo; messages: string[] }>('login_microsoft'),
  microsoftLoginStatus: () => request<{ status: string; code?: string; verification_uri?: string }>('microsoft_login_status'),
  deleteAccount: (accountId: string) => request<{ removed: boolean }>('delete_account', { account_id: accountId }),
  setDefaultAccount: (accountId: string) => request<{ account: AccountInfo; accounts: AccountInfo[] }>('set_default_account', { account_id: accountId }),
  moveAccount: (accountId: string, direction: 'up' | 'down') => request<{ accounts: AccountInfo[] }>('move_account', { account_id: accountId, direction }),
  deleteInstance: (name: string) => request<{ instances: InstanceInfo[] }>('delete_instance', { name }),
  duplicateInstance: (source: string, target: string) =>
    request<{ instance: InstanceInfo }>('duplicate_instance', { source, target }),
  externalInstances: () => request<{ candidates: ExternalInstanceCandidate[] }>('external_instances_scan'),

  importExternalInstance: (sourcePath: string, targetName?: string) =>
    request<{ instance: InstanceInfo; instances: InstanceInfo[] }>('external_instance_import', { source_path: sourcePath, target_name: targetName }),

  // ── Optimization ───────────────────────────────────────────────
  jvmPresetsList: () =>
    request<{ presets: import('../types').JvmPreset[] }>('jvm_presets_list'),
  optimizationAnalyze: (name?: string) =>
    request<{ report: import('../types').OptimizationReport }>('optimization_analyze', name ? { name } : {}),
  optimizationApply: (opts: {
    name?: string
    preset_id: string
    memory_mib?: number
    graphics_mode?: string
  }) => request<{ ok: boolean; preset_id: string }>('optimization_apply', opts),
  launchHistory: (name: string) =>
    request<{ history: import('../types').LaunchHistoryEntry[] }>('launch_history', { name }),
  cacheStats: () => request<{ cache: import('../types').CacheStats }>('cache_stats'),
  cachePrune: (maxAgeHours?: number) =>
    request<{ deleted_files: number }>('cache_prune', { max_age_hours: maxAgeHours ?? 48 }),
  instanceDetails: (name: string) => request<InstanceDetails>('instance_details', { name }),
  bedrockRuntimeStatus: (name = '') => request<{ status: BedrockRuntimeStatus }>('bedrock_runtime_status', { name }),
  bedrockVersions: (name: string, refresh = false) =>
    request<BedrockVersionsResult>('bedrock_versions', { name, refresh }),
  bedrockSetupVersion: (name: string, edition: 'release' | 'preview', version: string) =>
    request<{ started: boolean; already_running?: boolean }>('bedrock_setup_version', { name, edition, version }),
  bedrockSetupStatus: (name: string) =>
    request<BedrockSetupStatus>('bedrock_setup_status', { name }),
  bedrockSetupCancel: (name: string) =>
    request<{ stopped: boolean }>('bedrock_setup_cancel', { name }),
  createBedrockInstance: (name: string, displayName = '') =>
    request<{ instance: InstanceInfo; instances: InstanceInfo[] }>('bedrock_create_instance', { name, display_name: displayName }),
  bedrockContentList: (name: string) => request<{ items: BedrockContentItem[] }>('bedrock_content_list', { name }),
  bedrockContentImport: (name: string, fileName: string, data: string) =>
    request<{ items: BedrockContentItem[]; logs: string[] }>('bedrock_content_import', { name, file_name: fileName, data }),
  bedrockContentRemove: (name: string, scope: string, directory: string, itemName: string) =>
    request<{ items: BedrockContentItem[] }>('bedrock_content_remove', { name, scope, directory, item_name: itemName }),
  bedrockStoreLogin: (name: string) => request<{ started: boolean; already_running?: boolean }>('bedrock_store_login', { name }),
  bedrockStoreLoginStatus: (name: string) => request<BedrockSetupStatus>('bedrock_store_login_status', { name }),
  bedrockAccountStatus: (name: string) => request<{ signed_in: boolean; platform: string }>('bedrock_account_status', { name }),
  systemNotify: (title: string, body: string) => request<{ sent: boolean }>('system_notify', { title, body }),
  overlayState: () => request<OverlayState>('overlay_state'),
  recordingStart: (instanceName: string) => request<RecordingStatus>('recording_start', { instance_name: instanceName }),
  recordingStop: () => request<RecordingStatus>('recording_stop'),
  recordingStatus: () => request<RecordingStatus>('recording_status'),
  bedrockCancelStoreLogin: (name: string) => request<{ stopped: boolean }>('bedrock_store_login_cancel', { name }),
  bedrockStop: (name: string) => request<{ stopped: boolean }>('bedrock_stop', { name }),
  instanceGroups: () => request<{ groups: string[] }>('instance_groups'),
  settingsProfilesList: () => request<{ profiles: SettingsProfile[] }>('settings_profiles_list'),
  settingsProfileSave: (name: string) => request<{ profiles: SettingsProfile[] }>('settings_profile_save', { name }),
  settingsProfileApply: (name: string) => request<{ settings: SettingsInfo; applied: string[] }>('settings_profile_apply', { name }),
  settingsProfileDelete: (name: string) => request<{ profiles: SettingsProfile[] }>('settings_profile_delete', { name }),
  settingsProfileExport: (name: string) => request<{ name: string; file_name: string; json: string }>('settings_profile_export', { name }),
  settingsProfileImport: (json: string, name?: string) => request<{ profiles: SettingsProfile[] }>('settings_profile_import', { json, name }),
  worldBackupList: (name: string) => request<{ backups: WorldBackup[]; schedule: BackupSchedule }>('world_backup_list', { name }),
  worldBackupCreate: (name: string, destination = 'local') => request<{ backup: { backup_id: string; size_bytes: number; files: number }; schedule: BackupSchedule }>('world_backup_create', { name, destination }),
  worldBackupRestore: (name: string, backupId: string) => request<{ restored: string[]; backups: WorldBackup[] }>('world_backup_restore', { name, backup_id: backupId }),
  worldBackupDelete: (name: string, backupId: string) => request<{ backups: WorldBackup[] }>('world_backup_delete', { name, backup_id: backupId }),
  worldBackupScheduleGet: (name: string) => request<{ schedule: BackupSchedule }>('world_backup_schedule_get', { name }),
  worldBackupScheduleSet: (name: string, values: Partial<BackupSchedule>, override?: boolean) => request<{ schedule: BackupSchedule }>('world_backup_schedule_set', { name, values, override }),
  systemMetrics: (fresh = false, gameRoot = '') => request<SystemMetricsSnapshot>('system_metrics', { fresh, game_root: gameRoot }),
  modDependencyGraph: (name: string) => request<ModDependencyGraph>('mod_dependency_graph', { name }),
  modRemovalImpact: (name: string, modId: string) => request<{ mod_id: string; breaks: string[] }>('mod_removal_impact', { name, mod_id: modId }),
  exportModpack: (name: string, displayName = '', versionLabel = '') => request<{ path: string; file_name: string; size_bytes: number; indexed_files: number; override_files: number }>('export_modpack', { name, display_name: displayName, version_label: versionLabel }),
  createInstanceGroup: (name: string) => request<{ groups: string[] }>('create_instance_group', { name }),
  renameInstanceGroup: (oldName: string, newName: string) => request<{ groups: string[] }>('rename_instance_group', { old_name: oldName, new_name: newName }),
  bedrockScreenshotsList: (name: string) => request<{ items: BedrockScreenshotItem[] }>('bedrock_screenshots_list', { name }),
  deleteInstanceGroup: (name: string) => request<{ groups: string[] }>('delete_instance_group', { name }),
  updateInstanceProfile: (payload: {
    name: string
    display_name: string
    description: string
    group: string
    icon_file_name?: string
    icon_data?: string
    banner_file_name?: string
    banner_data?: string
    clear_icon?: boolean
    clear_banner?: boolean
    notes?: string
    tags?: string[]
  }) => request<{ instance: InstanceInfo; groups: string[] }>('update_instance_profile', payload),
  instanceLog: (name: string) => request<InstanceLog>('instance_log', { name }),
  instanceFileList: (name: string, directory?: string) =>
    request<{ files: InstanceFile[] }>('instance_file_list', { name, directory }),
  instanceFileRead: (name: string, path: string, encoding?: 'text' | 'base64') =>
    request<InstanceFileReadResult>('instance_file_read', { name, path, encoding }),
  instanceFileWrite: (name: string, path: string, content: string) =>
    request<{ saved: boolean }>('instance_file_write', { name, path, content }),
  instanceFileMkdir: (name: string, path: string) =>
    request<{ created: boolean }>('instance_file_mkdir', { name, path }),
  instanceFileUpload: (name: string, path: string, data: string) =>
    request<{ uploaded: boolean }>('instance_file_upload', { name, path, data }),
  instanceFileDelete: (name: string, path: string) =>
    request<{ removed: boolean }>('instance_file_delete', { name, path }),
  instanceLogArchives: (name: string, offset?: number, limit?: number) =>
    request<{ logs: InstanceLogArchive[]; next_offset: number | null }>('instance_log_archives', { name, offset, limit }),
  instanceLogArchiveRead: (name: string, logId: string, offset?: number, limit?: number) =>
    request<InstanceLogArchiveReadResult>('instance_log_archive_read', { name, log_id: logId, offset, limit }),
  instanceSnapshotList: (name: string, category: string) => request<{ snapshots: ServerSnapshot[]; category: string }>('instance_snapshot_list', { name, category }),
  instanceSnapshotCreate: (name: string, category: string) => request<{ snapshot: ServerSnapshot; snapshots: ServerSnapshot[]; details: InstanceDetails }>('instance_snapshot_create', { name, category }),
  instanceSnapshotRestore: (name: string, category: string, snapshotId: string) => request<{ restored: ServerSnapshot; snapshots: ServerSnapshot[]; details: InstanceDetails }>('instance_snapshot_restore', { name, category, snapshot_id: snapshotId }),
  instanceSnapshotDelete: (name: string, category: string, snapshotId: string) => request<{ removed: boolean; snapshots: ServerSnapshot[]; details: InstanceDetails }>('instance_snapshot_delete', { name, category, snapshot_id: snapshotId }),
  saveInstanceSettings: (name: string, values: Partial<InstanceSettings>) =>
    request<{ settings: InstanceSettings }>('save_instance_settings', { name, values }),
  serverCores: () => request<{ cores: ServerCore[] }>('server_cores'),
  serverCoreVersions: (core: string) => request<{ core: string; versions: string[] }>('server_core_versions', { core }),
  serverPluginCatalogs: () => request<{ catalogs: PluginCatalog[]; active_catalog_id: string }>('server_plugin_catalogs'),
  saveServerPluginCatalogs: (catalogs: PluginCatalog[], activeCatalogId: string) =>
    request<{ catalogs: PluginCatalog[]; active_catalog_id: string }>('save_server_plugin_catalogs', { catalogs, active_catalog_id: activeCatalogId }),
  createServer: (payload: {
    name: string
    display_name: string
    core: string
    version: string
    port: number
    memory: string
    eula_accepted: boolean
    custom_jar_name?: string
    custom_jar_data?: string
  }) => request<{ server: InstanceInfo; instances: InstanceInfo[] }>('server_create', payload),
  serverDetails: (name: string, directory = '') => request<ServerDetails>('server_details', { name, directory }),
  saveServerConfig: (name: string, values: Partial<ServerConfig>) => request<{ config: ServerConfig; server: InstanceInfo }>('server_save_config', { name, values }),
  uploadServerCore: (name: string, fileName: string, data: string) => request<{ path: string; server: InstanceInfo }>('server_upload_core', { name, file_name: fileName, data }),
  downloadServerCore: (name: string, core?: string, version?: string, build?: string, confirmUnverified = false) => request<{ download: { core: string; version: string; build: string; path: string; url: string; sha256: string; trusted_hash: boolean }; server: InstanceInfo }>('server_core_download', { name, core, version, build, confirm_unverified: confirmUnverified }),
  startServer: (name: string) => request<ServerStatus>('server_start', { name }),
  restartServer: (name: string) => request<ServerStatus>('server_restart', { name }),
  stopServer: (name: string) => request<ServerStatus>('server_stop', { name }),
  serverCommand: (name: string, command: string) => request<ServerStatus>('server_command', { name, command }),
  serverStatus: (name: string) => request<ServerStatus>('server_status', { name }),
  serverNetworkDiagnose: (name: string, externalPort: number) => request<ServerNetworkReport>('server_network_diagnose', { name, external_port: externalPort }),
  serverNetworkStatus: (name: string) => request<{ mapping: ServerNetworkMapping }>('server_network_status', { name }),
  serverNetworkPrepare: (name: string) => request<{ prepared: boolean; port: number }>('server_network_prepare', { name, confirmed: true }),
  serverNetworkOpen: (name: string, externalPort: number, observedIp: string) => request<{ mapping: ServerNetworkMapping }>('server_network_open', { name, external_port: externalPort, observed_ip: observedIp, confirmed: true }),
  serverNetworkClose: (name: string) => request<{ mapping: ServerNetworkMapping }>('server_network_close', { name }),
  serverLog: (name: string, since = 0) => request<{ lines: string[]; running: boolean; cursor?: number; reset?: boolean }>('server_log', { name, since }),
  serverFileList: (name: string, directory = '') => request<{ files: ServerFile[] }>('server_file_list', { name, directory }),
  serverFileRead: (name: string, path: string) => request<{ content: string }>('server_file_read', { name, path }),
  serverFileWrite: (name: string, path: string, content: string) => request<{ saved: boolean }>('server_file_write', { name, path, content }),
  serverFileMkdir: (name: string, path: string) => request<{ created: boolean }>('server_file_mkdir', { name, path }),
  serverFileUpload: (name: string, path: string, data: string) => request<{ uploaded: boolean }>('server_file_upload', { name, path, data }),
  serverFileDelete: (name: string, path: string) => request<{ removed: boolean }>('server_file_delete', { name, path }),
  serverPluginSearch: (name: string, query: string, catalogId?: string, sortBy: CatalogSort = 'relevance') => request<{ results: ServerPluginResult[]; catalog_id: string; core: string; version: string }>('server_plugin_search', { name, query, catalog_id: catalogId, sort_by: sortBy }),
  serverPluginDetails: (name: string, projectId: string, catalogId?: string) => request<{ details: ServerPluginResult & Record<string, unknown>; catalog_id: string; core: string; version: string }>('server_plugin_details', { name, project_id: projectId, catalog_id: catalogId }),
  serverPluginUpdates: (name: string) => request<{ updates: ServerPluginUpdate[]; files: ServerFile[] }>('server_plugin_updates', { name }),
  serverPluginUpdate: (name: string, fileName: string, catalogId: string, confirmUnverified = false) => request<{ plugin: { file_name: string; version: string; version_id: string; sha256: string; trusted_hash: boolean; updated?: boolean }; files: ServerFile[]; catalog_id: string }>('server_plugin_update', { name, file_name: fileName, catalog_id: catalogId, confirm_unverified: confirmUnverified }),
  serverPluginInstall: (name: string, projectId: string, catalogId?: string, confirmUnverified = false) => request<{ plugin: { file_name: string; path: string; version: string; sha256: string; trusted_hash: boolean }; files: ServerFile[]; catalog_id: string }>('server_plugin_install', { name, project_id: projectId, catalog_id: catalogId, confirm_unverified: confirmUnverified }),
  serverPluginRemove: (name: string, fileName: string) => request<{ removed: boolean; files: ServerFile[] }>('server_plugin_remove', { name, file_name: fileName }),
  lanSessionCreate: (payload: LanRoomCredentials) => request<LanRoomStatus>('lan_session_create', { ...payload }),
  lanSessionJoin: (payload: LanRoomCredentials) => request<LanRoomStatus>('lan_session_join', { ...payload }),
  lanSessionStatus: (payload: LanRoomCredentials) => request<LanRoomStatus>('lan_session_status', { ...payload }),
  lanSessionLeave: (payload: LanRoomCredentials) => request<{ left: boolean; room_name: string }>('lan_session_leave', { ...payload }),
  githubPluginSearch: (query = '', page = 1) => request<{ results: GitHubPluginRepository[]; page: number }>('github_plugin_search', { query, page }),
  githubPluginDetails: (owner: string, repo: string) => request<GitHubPluginDetails>('github_plugin_details', { owner, repo }),
  githubPluginInstall: (owner: string, repo: string, replace = false) => request<{ plugin: PluginManifest; plugins: PluginManifest[]; metadata: { full_name: string; version: string; size: number; sha256: string } }>('github_plugin_install', { owner, repo, replace }),
  themeList: () => request<{ themes: LauncherThemePreset[] }>('theme_list'),
  themePreview: (data: string) => request<{ theme: LauncherThemePreset }>('theme_preview', { data }),
  themeSave: (
    theme: Omit<LauncherThemePreset, 'source' | 'version' | 'creators' | 'source_url'>,
    metadata: ThemeArchiveMetadata,
    replace = false,
  ) => request<{ theme: LauncherThemePreset; themes: LauncherThemePreset[] }>('theme_save', { theme, metadata, replace }),
  themeImport: (data: string, replace = false) => request<{ theme: LauncherThemePreset; themes: LauncherThemePreset[] }>('theme_import', { data, replace }),
  themeDelete: (themeId: string) => request<{ removed: boolean; themes: LauncherThemePreset[] }>('theme_delete', { theme_id: themeId }),
  themeExport: (themeId: string) => request<{ file_name: string; data: string }>('theme_export', { theme_id: themeId }),
  githubThemeSearch: (query = '', page = 1) => request<{ results: GitHubThemeRepository[]; page: number }>('github_theme_search', { query, page }),
  githubThemeInstall: (owner: string, repo: string, replace = false) => request<{
    theme: LauncherThemePreset
    themes: LauncherThemePreset[]
    metadata: { full_name: string; version: string; size: number; sha256: string; url: string }
  }>('github_theme_install', { owner, repo, replace }),
  serverSnapshotList: (name: string, category: 'mods' | 'plugins') => request<{ snapshots: ServerSnapshot[]; category: string }>('server_snapshot_list', { name, category }),
  serverSnapshotCreate: (name: string, category: 'mods' | 'plugins') => request<{ snapshot: ServerSnapshot; snapshots: ServerSnapshot[] }>('server_snapshot_create', { name, category }),
  serverSnapshotRestore: (name: string, category: 'mods' | 'plugins', snapshotId: string) => request<{ restored: ServerSnapshot; snapshots: ServerSnapshot[]; files: ServerFile[] }>('server_snapshot_restore', { name, category, snapshot_id: snapshotId }),
  serverSnapshotDelete: (name: string, category: 'mods' | 'plugins', snapshotId: string) => request<{ removed: boolean; snapshots: ServerSnapshot[] }>('server_snapshot_delete', { name, category, snapshot_id: snapshotId }),
  pluginList: () => request<{ plugins: PluginManifest[] }>('plugin_list'),
  pluginInstall: (data: string, enable = false, replace = false) =>
    request<{ plugin: PluginManifest; plugins: PluginManifest[] }>('plugin_install', { data, enable, replace }),
  pluginSetEnabled: (pluginId: string, enabled: boolean) =>
    request<{ plugin: PluginManifest; plugins: PluginManifest[] }>('plugin_set_enabled', { plugin_id: pluginId, enabled }),
  pluginUninstall: (pluginId: string) =>
    request<{ removed: boolean; plugins: PluginManifest[] }>('plugin_uninstall', { plugin_id: pluginId }),
  pluginNetworkFetch: (pluginId: string, url: string, requestOptions?: PluginNetworkRequest, signal?: AbortSignal) =>
    request<{ value: unknown }>('plugin_network_fetch', { plugin_id: pluginId, url, request: requestOptions }, signal),
  pluginNetworkDns: (pluginId: string, host: string, port?: number, signal?: AbortSignal) =>
    request<{ host: string; port?: number | null; addresses: Array<{ family: string; address: string }> }>('plugin_network_dns', { plugin_id: pluginId, host, port }, signal),
  pluginNetworkPing: (pluginId: string, host: string, port = 25565, timeoutMs = 3000, signal?: AbortSignal) =>
    request<{ host: string; port: number; reachable: boolean; latency_ms: number; error: string }>('plugin_network_ping', { plugin_id: pluginId, host, port, timeout_ms: timeoutMs }, signal),
  pluginNetworkPortCheck: (pluginId: string, host: string, port = 25565, timeoutMs = 3000, signal?: AbortSignal) =>
    request<{ host: string; port: number; reachable: boolean; latency_ms: number; error: string }>('plugin_network_port_check', { plugin_id: pluginId, host, port, timeout_ms: timeoutMs }, signal),
  pluginServerListGet: (pluginId: string, signal?: AbortSignal) => request<{ servers: PluginServer[] }>('plugin_server_list_get', { plugin_id: pluginId }, signal),
  pluginServerListSave: (pluginId: string, servers: readonly PluginServerInput[], signal?: AbortSignal) => request<{ servers: PluginServer[] }>('plugin_server_list_save', { plugin_id: pluginId, servers }, signal),
  pluginServerListAdd: (pluginId: string, server: PluginServerInput, signal?: AbortSignal) => request<{ server: PluginServer; servers: PluginServer[] }>('plugin_server_list_add', { plugin_id: pluginId, server }, signal),
  pluginServerListRemove: (pluginId: string, serverId: string, signal?: AbortSignal) => request<{ removed: boolean; servers: PluginServer[] }>('plugin_server_list_remove', { plugin_id: pluginId, server_id: serverId }, signal),
  pluginStorageGet: (pluginId: string, key: string, signal?: AbortSignal) => request<{ value?: unknown }>('plugin_storage_get', { plugin_id: pluginId, key }, signal),
  pluginStorageSet: (pluginId: string, key: string, value: unknown, signal?: AbortSignal) => request<{ saved: boolean }>('plugin_storage_set', { plugin_id: pluginId, key, value }, signal),
  pluginStorageRemove: (pluginId: string, key: string, signal?: AbortSignal) => request<{ removed: boolean }>('plugin_storage_remove', { plugin_id: pluginId, key }, signal),
  pluginConfigGet: (pluginId: string, key: string, signal?: AbortSignal) => request<{ value?: unknown }>('plugin_config_get', { plugin_id: pluginId, key }, signal),
  pluginConfigSet: (pluginId: string, key: string, value: unknown, signal?: AbortSignal) => request<{ saved: boolean }>('plugin_config_set', { plugin_id: pluginId, key, value }, signal),
  pluginConfigRemove: (pluginId: string, key: string, signal?: AbortSignal) => request<{ removed: boolean }>('plugin_config_remove', { plugin_id: pluginId, key }, signal),
  pluginFileList: (pluginId: string, directory = '') => request<{ files: string[] }>('plugin_file_list', { plugin_id: pluginId, directory }),
  pluginFileRead: (pluginId: string, path: string) => request<{ content: string }>('plugin_file_read', { plugin_id: pluginId, path }),
  pluginFileWrite: (pluginId: string, path: string, content: string) => request<{ saved: boolean }>('plugin_file_write', { plugin_id: pluginId, path, content }),
  pluginFileRemove: (pluginId: string, path: string) => request<{ removed: boolean }>('plugin_file_remove', { plugin_id: pluginId, path }),
  pluginDatabaseQuery: (pluginId: string, sql: string, parameters: readonly unknown[] = [], signal?: AbortSignal) => request<{ rows: Record<string, unknown>[] }>('plugin_database_query', { plugin_id: pluginId, sql, parameters }, signal),
  pluginDatabaseExecute: (pluginId: string, sql: string, parameters: readonly unknown[] = [], signal?: AbortSignal) => request<{ changes: number }>('plugin_database_execute', { plugin_id: pluginId, sql, parameters }, signal),
  pluginSecurityStatus: () => request<{ plugins: PluginManifest[]; publishers: Array<Record<string, unknown>> }>('plugin_security_status'),
  pluginPermissionGrantsSet: (pluginId: string, permissions: readonly PluginPermission[]) => request<{ plugin: PluginManifest; plugins: PluginManifest[] }>('plugin_permission_grants_set', { plugin_id: pluginId, permissions }),
  pluginLocalUntrustedExecution: (pluginId: string, allowed: boolean) => request<{ plugin: PluginManifest; plugins: PluginManifest[] }>('plugin_local_untrusted_execution', { plugin_id: pluginId, allowed }),
  pluginPublisherPin: (keyId: string, publicKey: string) => request<{ plugins: PluginManifest[]; publishers: Array<Record<string, unknown>>; publisher: Record<string, unknown> }>('plugin_publisher_pin', { key_id: keyId, public_key: publicKey }),
  pluginPublisherUnpin: (keyId: string) => request<{ plugins: PluginManifest[]; publishers: Array<Record<string, unknown>>; key_id: string; unpinned: boolean }>('plugin_publisher_unpin', { key_id: keyId }),
  pluginPublisherRevocationAuthoritySet: (keyId: string, enabled: boolean) => request<{ plugins: PluginManifest[]; publishers: Array<Record<string, unknown>> }>('plugin_publisher_revocation_authority_set', { key_id: keyId, enabled }),
  pluginRevocationsImport: (documentJson: string) => request<{
    imported: { sequence: number; authority_key_id: string; revoked_plugins: string[]; revoked_publishers: string[] }
    plugins: PluginManifest[]
    publishers: Array<Record<string, unknown>>
  }>('plugin_revocations_import', { document_json: documentJson }),
  pluginLocalRevoke: (pluginId: string, reason = '') => request<{ plugin: PluginManifest; plugins: PluginManifest[] }>('plugin_local_revoke', { plugin_id: pluginId, reason }),
  pluginLocalUnrevoke: (pluginId: string) => request<{ plugin: PluginManifest; plugins: PluginManifest[] }>('plugin_local_unrevoke', { plugin_id: pluginId }),
  pluginApiSource: (pluginId: string, signal?: AbortSignal) => request<{ manifest: RuntimePluginManifest; entrypoint: string; source: string; package_sha256: string }>('plugin_api_source', { plugin_id: pluginId }, signal),
  pluginApiRuntimeInfo: (pluginId: string, signal?: AbortSignal) => request<RuntimeInfo>('plugin_api_runtime_info', { plugin_id: pluginId }, signal),
  pluginApiCapabilities: (pluginId: string, signal?: AbortSignal) => request<{ plugin_id: string; api_version: string; capabilities: string[]; sandbox_profile: string }>('plugin_api_capabilities', { plugin_id: pluginId }, signal),
  pluginApiQuotaUsage: (pluginId: string, signal?: AbortSignal) => request<{ plugin_id: string; totals: { bytes: number; files: number }; directories: Record<string, { bytes: number; files: number }> }>('plugin_api_quota_usage', { plugin_id: pluginId }, signal),
  pluginApiPermissionsGet: (pluginId: string, signal?: AbortSignal) => request<PermissionsSummary>('plugin_api_permissions_get', { plugin_id: pluginId }, signal),
  pluginApiPermissionsSet: (pluginId: string, permissions: readonly string[], signal?: AbortSignal) => request<{ plugin: PluginManifest; permissions: PermissionsSummary }>('plugin_api_permissions_set', { plugin_id: pluginId, permissions }, signal),
  pluginApiAuthorize: (pluginId: string, permission: PluginPermission, signal?: AbortSignal) => request<{ authorized: boolean }>('plugin_api_authorize', { plugin_id: pluginId, permission }, signal),
  pluginApiOpenExternal: (pluginId: string, url: string, signal?: AbortSignal) => request<{ opened: boolean }>('plugin_api_open_external', { plugin_id: pluginId, url }, signal),
  openExternal: (url: string, signal?: AbortSignal) => request<{ opened: boolean }>('open_external', { url }, signal),
  pluginApiFilesList: (pluginId: string, scope: 'plugin' | 'instance' | 'launcher-root-readonly', directory = '', instanceName = '', signal?: AbortSignal) =>
    request<{ files: PluginFileInfo[] }>('plugin_api_files_list', { plugin_id: pluginId, scope, directory, instance_name: instanceName }, signal),
  pluginApiFilesRead: (pluginId: string, scope: 'plugin' | 'instance' | 'launcher-root-readonly', path: string, instanceName = '', signal?: AbortSignal) =>
    request<{ content: string }>('plugin_api_files_read', { plugin_id: pluginId, scope, path, instance_name: instanceName }, signal),
  pluginApiFilesWrite: (pluginId: string, scope: 'plugin' | 'instance', path: string, content: string, instanceName = '', signal?: AbortSignal) =>
    request<{ saved: boolean }>('plugin_api_files_write', { plugin_id: pluginId, scope, path, content, instance_name: instanceName }, signal),
  pluginApiFilesRemove: (pluginId: string, scope: 'plugin' | 'instance', path: string, instanceName = '', signal?: AbortSignal) =>
    request<{ removed: boolean }>('plugin_api_files_remove', { plugin_id: pluginId, scope, path, instance_name: instanceName }, signal),
  pluginApiConfigProjectedRead: <T = Record<string, unknown>>(pluginId: string, schemaId: string, signal?: AbortSignal) =>
    request<{ schema_id: string; data: T; removed_fields: string[] }>('plugin_api_config_projected_read', { plugin_id: pluginId, schema_id: schemaId }, signal),
  pluginHttpRequest: (pluginId: string, requestPayload: HttpRequest, signal?: AbortSignal) =>
    request<HttpResponse>('plugin_http_request', { plugin_id: pluginId, request: requestPayload }, signal),
  pluginApiInstancesList: (pluginId: string, signal?: AbortSignal) =>
    request<{ instances: ProcessMetricsResponse['instance'][] }>('plugin_api_instances_list', { plugin_id: pluginId }, signal),
  pluginApiProcessMetrics: (pluginId: string, instanceName: string, signal?: AbortSignal) =>
    request<ProcessMetricsResponse>('plugin_api_process_metrics', { plugin_id: pluginId, instance_name: instanceName }, signal),
  pluginApiProxyRoutesGet: (pluginId: string, signal?: AbortSignal) =>
    request<{ routes: ProxyRoutePolicy[] }>('plugin_api_proxy_routes_get', { plugin_id: pluginId }, signal),
  pluginApiProxyRoutesSave: (pluginId: string, routes: ProxyRoutePolicy[], signal?: AbortSignal) =>
    request<{ routes: ProxyRoutePolicy[] }>('plugin_api_proxy_routes_save', { plugin_id: pluginId, routes }, signal),
  pluginApiProxyRoutesAdd: (pluginId: string, route: Omit<ProxyRoutePolicy, 'id'> & { id?: string }, signal?: AbortSignal) =>
    request<{ route: ProxyRoutePolicy; routes: ProxyRoutePolicy[] }>('plugin_api_proxy_routes_add', { plugin_id: pluginId, route }, signal),
  pluginApiProxyRoutesRemove: (pluginId: string, routeId: string, signal?: AbortSignal) =>
    request<{ removed: boolean; routes: ProxyRoutePolicy[] }>('plugin_api_proxy_routes_remove', { plugin_id: pluginId, route_id: routeId }, signal),
  removeInstanceAsset: (name: string, kind: string, path: string) =>
    request<InstanceDetails>('remove_instance_asset', { name, kind, path }),
  toggleMod: (name: string, path: string, enabled: boolean) => request<InstanceDetails>('toggle_mod', { name, path, enabled }),
  modAuditPrepare: (instanceName: string, path: string, signal?: AbortSignal) =>
    request<ModAuditPrepareResult>('mod_audit_prepare', { instance_name: instanceName, path }, signal),
  modAuditAccept: (sessionId: string, signal?: AbortSignal) =>
    request<{ report: ModAuditReport }>('mod_audit_accept', { session_id: sessionId }, signal),
  modAuditCancel: (sessionId: string) =>
    request<{ session_id: string; cancelled: boolean; sources_wiped: boolean }>('mod_audit_cancel', { session_id: sessionId }),
  modAuditStatus: (sessionId: string) =>
    request<{ session_id: string; state: string; sources_wiped: boolean; report?: ModAuditReport }>('mod_audit_status', { session_id: sessionId }),
  importAsset: (name: string, kind: string, fileName: string, data: string) =>
    request<InstanceDetails>('import_asset', { name, kind, file_name: fileName, data }),
  install: (payload: {
    version: string
    loader: LoaderId
    loader_version?: string
    instance_name?: string
    memory?: string
    java_path?: string
    no_assets?: boolean
    replace_existing?: boolean
    fail_if_exists?: boolean
  }) => request<{ version_id: string; instance_dir: string; instances: InstanceInfo[] }>('install', payload),
  launch: (payload: {
    version: string
    loader: LoaderId
    loader_version?: string
    instance_name?: string
    memory?: string
    java_path?: string
    account?: string
    profile_id?: string
    no_assets?: boolean
  }) => request<{ exit_code: number; logs: string[]; version_id: string; crash?: CrashReport }>('launch', payload),
  launchInstance: (name: string, options: { world?: string; server?: string; account?: string; profile_id?: string } = {}) =>
    request<{ exit_code: number; logs: string[]; version_id: string; crash?: CrashReport }>('launch_instance', { name, ...options }),
  openFolder: (path?: string) => request<{ opened: boolean }>('open_folder', { path }),
  modsSearch: (payload: { query: string; instance_name?: string; game_version?: string; loader?: string; server_core?: string; project_type?: ProjectType; catalog_id?: string; sort_by?: CatalogSort }) =>
    request<{ results: ModResult[]; instance_name: string; game_version: string; loader: string; server_core?: string; catalog_id?: string }>('mods_search', payload),
  modsDetails: (payload: { project_id: string; instance_name?: string; game_version?: string; loader?: string; server_core?: string; project_type?: ProjectType; catalog_id?: string }) =>
    request<{ details: Record<string, unknown>; compatible_versions: { id: string; version_number: string; name: string; date_published: string; game_versions: string[]; loaders: string[] }[]; catalog_id: string }>('mods_details', payload),
  modsCheckUpdates: (instanceName: string, projectType: Exclude<ProjectType, 'plugin' | 'modpack'>, catalogId?: string) =>
    request<{ updates: ResourceUpdate[]; errors?: string[]; project_type: ProjectType; instance_name: string; catalog_id: string }>('mods_check_updates', { instance_name: instanceName, project_type: projectType, catalog_id: catalogId }),
  modsUpdate: (payload: { project_id: string; version_id: string; instance_name: string; project_type?: Exclude<ProjectType, 'plugin' | 'modpack'>; world_name?: string; catalog_id?: string; confirm_unverified?: boolean }) =>
    request<{ project_id: string; name: string; version: string; version_id: string; file_name: string; dependencies: Array<Record<string, unknown>>; details: InstanceDetails; instance_name: string }>('mods_update', payload),
  modsInstall: (payload: {
    project_id: string
    instance_name: string
    project_type?: ProjectType
    world_name?: string
    catalog_id?: string
    version?: string
    version_id?: string
    display_name?: string
    confirm_unverified?: boolean
    preflight_only?: boolean
    game_version?: string
    loader?: string
    loader_version?: string
  }) => request<{ file_name?: string; path?: string; details?: InstanceDetails; trusted_hash?: boolean; preflight?: boolean; version_id?: string; dependencies?: Array<Record<string, unknown>> | ModpackDependencyRequirements }>('mods_install', payload),
  modsRemove: (payload: {
    project_id: string
    instance_name: string
    project_type?: ProjectType
    catalog_id?: string
  }) => request<{ removed: number; instance_name: string; details: InstanceDetails }>('mods_remove', payload),
  importModpack: (payload: {
    instance_name: string
    file_name: string
    data: string
    confirm_unverified?: boolean
    preflight_only?: boolean
    game_version?: string
    loader?: string
    loader_version?: string
  }) =>
    request<{ file_name?: string; path?: string; details?: InstanceDetails; files?: string[]; preflight?: boolean; dependencies?: ModpackDependencyRequirements }>('modpack_import', payload),
  graphicsDiagnostics: () => request<Record<string, unknown>>('graphics_diagnostics'),
  skinsList: () => request<{ skins: SkinInfo[] }>('skins_list'),
  skinsImport: (fileName: string, data: string, model: 'classic' | 'slim') => request<{ skin: SkinInfo }>('skins_import', { file_name: fileName, data, model }),
  skinProfile: (accountId: string) => request<SkinProfile>('skin_profile', { account_id: accountId }),
  applySkin: (accountId: string, skinId: string, model: 'classic' | 'slim') => request<{ ok: boolean }>('apply_skin', { account_id: accountId, skin_id: skinId, model }),
  clearSkin: (accountId: string) => request<{ ok: boolean }>('clear_skin', { account_id: accountId }),
  applyCape: (accountId: string, capeId: string) => request<{ ok: boolean }>('apply_cape', { account_id: accountId, cape_id: capeId }),
  clearCape: (accountId: string) => request<{ ok: boolean }>('clear_cape', { account_id: accountId }),
  openSkinsFolder: () => request<{ opened: boolean }>('open_skins_folder'),
  importWallpaper: (fileName: string, data: string) => request<{ settings: SettingsInfo }>('wallpaper_import', { file_name: fileName, data }),
  clearWallpaper: () => request<{ settings: SettingsInfo }>('wallpaper_clear'),
  fileUrl: (path: string) => (inTauri() ? convertFileSrc(path) : path),
}
