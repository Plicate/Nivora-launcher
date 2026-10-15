<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch, watchEffect } from 'vue'
import ActivityPanel from './components/ActivityPanel.vue'
import AppSidebar from './components/AppSidebar.vue'
import BrandLogo from './components/BrandLogo.vue'
import CatalogBadge from './components/CatalogBadge.vue'
import CatalogDetailsDialog from './components/CatalogDetailsDialog.vue'
import CloudStoragePanel from './components/CloudStoragePanel.vue'
import BedrockWorkspace from './components/BedrockWorkspace.vue'
import CustomSelect from './components/CustomSelect.vue'
import CoreBadge from './components/CoreBadge.vue'
import ColorPicker from './components/ColorPicker.vue'
import Icon from './components/Icon.vue'
import HotkeyInput from './components/HotkeyInput.vue'
import InstanceGroups from './components/InstanceGroups.vue'
import InstanceWorkspace from './components/InstanceWorkspace.vue'
import FirstRunExperience from './components/FirstRunExperience.vue'
import MemoryField from './components/MemoryField.vue'
import NumericStepper from './components/NumericStepper.vue'
import ServerWorkspace from './components/ServerWorkspace.vue'
import LoaderBadge from './components/LoaderBadge.vue'
import PluginCenter from './components/PluginCenter.vue'
import PluginViewRenderer from './components/PluginViewRenderer.vue'
import SkinPreview from './components/SkinPreview.vue'
import ThemeGallery from './components/ThemeGallery.vue'
import ThemeStudio from './components/ThemeStudio.vue'
import { check as checkForAppUpdate } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import { FRONTEND_BUILD_STAMP, bridge } from './services/bridge'
import { setLocale, type Locale } from './i18n'
import { DEFAULT_OVERLAY_SETTINGS, bindGameHotkeys, initOverlayEventBridge, normalizeHotkey, overlayLastError, overlaySelfTest, overlayWaylandMode, overlayWindowRequested, setOverlayOpaqueWindow, setOverlayVisible, setOverlayWaylandMode, toggleOverlay, toggleRecording, unbindGameHotkeys, type OverlaySelfTestStep } from './services/overlay'
import { chooseDialog, confirmDialog, promptDialog } from './services/dialogs'
import { createPluginHostCall } from './services/pluginHost'
import { pluginRuntime } from './services/pluginRuntime'
import { launcherThemes } from './services/launcherExtensions'
import { useModalFocus } from './services/modalFocus'
import { initialFirstRunPhase, readFirstRunExperience, writeFirstRunExperience, type FirstRunExperienceState, type FirstRunPhase } from './services/firstRunExperience'
import { createWallpaperRenderer } from './services/wallpaper'
import DialogHost from './components/DialogHost.vue'
import GameOverlay from './components/GameOverlay.vue'
import type {
  PluginEventName,
  PluginEventPayloadMap,
  PluginJsonValue,
  PluginInstanceRef,
  PluginCommandDefinition,
  PluginShortcutDefinition,
  PluginThemeDefinition,
  PluginThemeContext,
  PluginUiTone,
  PluginUiView,
} from '../../plugins/api'
import type { RuntimePluginManifest } from './plugins/runtimeManifest'
import type {
  AccountInfo,
  ActivityState,
  AnimationSettings,
  AnimationStyle,
  AppearanceSettings,
  AppState,
  BedrockRuntimeStatus,
  CatalogSort,
  ExternalInstanceCandidate,
  InstanceInfo,
  InstanceWorld,
  LauncherProfile,
  LoaderChoice,
  LoaderId,
  ModpackDependencyRequirements,
  ModpackModPreview,
  ModpackModPreviewItem,
  MemoryUnit,
  ModResult,
  LauncherThemePreset,
  OverlaySettings,
  PluginCatalog,
  PluginManifest,
  ProjectType,
  SelectOption,
  SettingsProfile,
  SystemMetricsSnapshot,
  BackupSchedule,
  CrashReport,
  SkinInfo,
  SkinProfile,
  ResourceCatalog,
  ServerCore,
  ServerSnapshot,
  ThemeMode,
  ThemeSaveRequest,
  VersionInfo,
  ViewName,
} from './types'

const views: Record<ViewName, { title: string; kicker: string }> = {
  instances: { title: 'Экземпляры', kicker: 'Библиотека игр' },
  'instance-settings': { title: 'Рабочая область', kicker: 'Экземпляр' },
  accounts: { title: 'Аккаунты', kicker: 'Профили доступа' },
  mods: { title: 'Каталог', kicker: 'Моды и ресурсы' },
  modpacks: { title: 'Модпаки', kicker: 'Готовые сборки' },
  plugins: { title: 'Плагины для лаунчера', kicker: 'Расширения интерфейса' },
  themes: { title: 'Центр настройки тем', kicker: 'Студия оформления' },
  settings: { title: 'Настройки', kicker: 'Параметры лаунчера' },
}
const customizablePages = (Object.entries(views) as Array<[ViewName, { title: string; kicker: string }]>).map(([id, value]) => ({ id, ...value }))
const viewOptions: SelectOption[] = customizablePages.map((view) => ({ value: view.id, label: view.title, meta: view.kicker }))
const persistentViewOptions = viewOptions.filter((option) => option.value !== 'instance-settings')
const shortcutRows = [
  { key: 'open_settings', label: 'Открыть настройки', default: 'Ctrl+,' },
  { key: 'refresh', label: 'Обновить данные', default: 'Ctrl+R' },
  { key: 'new_instance', label: 'Новый экземпляр', default: 'Ctrl+N' },
  { key: 'focus_search', label: 'Фокус на поиск', default: 'Ctrl+K' },
  { key: 'toggle_activity', label: 'Показать активность', default: 'Ctrl+J' },
]

const loaders: Array<{ id: LoaderId; label: string }> = [
  { id: 'vanilla', label: 'Vanilla' },
  { id: 'fabric', label: 'Fabric' },
  { id: 'forge', label: 'Forge' },
  { id: 'neoforge', label: 'NeoForge' },
  { id: 'quilt', label: 'Quilt' },
]
const loaderOptions: SelectOption[] = loaders.map((loader) => ({ value: loader.id, label: loader.label }))

const defaultColorTokens: Record<string, string> = {
  accent: '#8b653f',
  accent_strong: '#c5a274',
  background: '#191a17',
  panel: '#20221e',
  panel_raised: '#282a25',
  panel_soft: '#30322c',
  text: '#e9e8de',
  muted: '#a2a395',
  quiet: '#c7c7bb',
  border: '#77796b',
  danger: '#d58a7d',
  success: '#9bac78',
  warning: '#d2ad72',
  banner_shade: '#191a17',
}

const defaultRadiusTokens: Record<string, number> = {
  card: 7,
  button: 8,
  control: 10,
}

const legacyDefaultColorTokens: Record<string, string> = {
  accent: '#7aa2ff', accent_strong: '#b7c9ff', background: '#080d16', panel: '#101a29',
  panel_raised: '#142236', panel_soft: '#182941', text: '#f1f5ff', muted: '#8595b0',
  quiet: '#aebbd1', border: '#92ace2', danger: '#f47f9a', success: '#62d49a',
  warning: '#f3c76f', banner_shade: '#080d16',
}
function migrateDefaultColors(value?: Record<string, string>): Record<string, string> {
  const colors = { ...defaultColorTokens, ...(value || {}) }
  for (const [key, previous] of Object.entries(legacyDefaultColorTokens)) {
    if (colors[key]?.toLowerCase() === previous) colors[key] = defaultColorTokens[key]
  }
  return colors
}
const lightColorTokens: Record<string, string> = {
  ...defaultColorTokens,
  accent: '#65764b',
  accent_strong: '#4e6037',
  background: '#e9e8df',
  panel: '#f2f1e9',
  panel_raised: '#f8f7f0',
  panel_soft: '#e2e1d7',
  text: '#25271f',
  muted: '#686a5e',
  quiet: '#414338',
  border: '#77796b',
  danger: '#a95349',
  success: '#4f6d3b',
  warning: '#805a2e',
  banner_shade: '#e9e8df',
}

const defaultLauncherProfile: LauncherProfile = {
  id: 'default',
  name: 'По умолчанию',
  memory: 'auto',
  java_path: '',
  graphics_mode: 'auto',
  account_id: 'auto',
  window_width: 1280,
  window_height: 720,
  fullscreen: false,
  quick_play_world: '',
  quick_play_server: '',
  jvm_arguments: [],
  game_arguments: [],
  jvm_preset: 'none',
}

const defaultAnimationSettings: AnimationSettings = {
  // 'slide' + ~260 мс: раньше дефолт был fade на 140 мс — переключение вкладок
  // глазом почти не ловилось и выглядело «мгновенной заменой».
  page_style: 'slide',
  page_duration_ms: 260,
  surface_duration_ms: 190,
  surface_stagger_ms: 24,
  page_styles: { instances: '', 'instance-settings': '', accounts: '', mods: '', modpacks: '', plugins: '', themes: '', settings: '' },
  page_durations_ms: { instances: 260, 'instance-settings': 260, accounts: 260, mods: 260, modpacks: 260, plugins: 260, themes: 260, settings: 260 },
}

const initialState: AppState = {
  instances: [],
  accounts: [],
  settings: {
    game_dir: '',
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
    download_retries: 5,
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
     launcher_profiles: [{ ...defaultLauncherProfile, jvm_arguments: [], game_arguments: [] }],
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
      accent_color: defaultColorTokens.accent,
    sidebar_position: 'left',
    sidebar_mode: 'adaptive',
    sidebar_width_mode: 'content',
    sidebar_width: 228,
    sidebar_floating: false,
      sidebar_visibility: 'always',
      animations: 'full',
      animation_settings: {
        ...defaultAnimationSettings,
        page_styles: { ...defaultAnimationSettings.page_styles },
        page_durations_ms: { ...defaultAnimationSettings.page_durations_ms },
      },
      overlay_settings: { ...DEFAULT_OVERLAY_SETTINGS },
      backup_schedule: { enabled: false, interval_hours: 24, keep: 5, destination: 'local' },
      density: 'comfortable',
     ui_scale: 100,
     radius_tokens: { card: 7, button: 8, control: 10 },
      page_themes: { instances: 'inherit', 'instance-settings': 'inherit', accounts: 'inherit', mods: 'inherit', modpacks: 'inherit', plugins: 'inherit', themes: 'inherit', settings: 'inherit' },
      page_layouts: { instances: 'balanced', 'instance-settings': 'balanced', accounts: 'balanced', mods: 'wide', modpacks: 'wide', plugins: 'wide', themes: 'wide', settings: 'balanced' },
    widget_visibility: { 'instances.workspace': true, 'accounts.status': true, 'mods.context': true },
     color_tokens: { ...defaultColorTokens },
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
  },
   server_cores: [],
   plugin_catalogs: [],
   active_plugin_catalog_id: 'modrinth-plugins',
   system: { platform: 'Linux', machine: 'x86_64', python: '', session: 'unknown', display: 'none', nvidia_smi: false },
}

const state = reactive<AppState>(initialState)
const currentView = ref<ViewName>('instances')
const plugins = ref<PluginManifest[]>([])
type ActivePluginView = { pluginId: string; pluginName: string; view: PluginUiView }
type ActivePluginCommand = { pluginId: string; pluginName: string; definition: PluginCommandDefinition }
type ActivePluginShortcut = { pluginId: string; pluginName: string; definition: PluginShortcutDefinition }
const pluginViews = ref<ActivePluginView[]>([])
const pluginCommands = ref<ActivePluginCommand[]>([])
const pluginShortcuts = ref<ActivePluginShortcut[]>([])
const pluginEventSubscriptions = new Map<string, Map<string, PluginEventName>>()
const pluginLogEventInFlight = new Set<string>()
const pluginLogEventLastSentAt = new Map<string, number>()
const pluginNotificationToastIds = new Map<string, Map<string, number>>()
const pluginThemeContributions = new Map<string, Map<string, PluginThemeDefinition>>()
const runningPluginDigests = new Map<string, string>()
let pluginRuntimeSyncQueue: Promise<void> = Promise.resolve()
const activePluginTabId = ref('')
const enabledPluginTabs = computed(() => {
  const staticTabs = plugins.value
    .filter((plugin) => plugin.installed && plugin.enabled && plugin.sidebar_tab?.label)
    .map((plugin) => ({ id: plugin.id, label: plugin.sidebar_tab!.label, name: plugin.name, description: plugin.description }))
  const runtimeTabs = pluginViews.value
    .filter((entry) => entry.view.slot === 'sidebar_tab')
    .map((entry) => ({ id: runtimeTabId(entry.pluginId, entry.view.id), label: entry.view.title, name: entry.pluginName, description: entry.view.description || '' }))
  return [...staticTabs, ...runtimeTabs]
})
function runtimeTabId(pluginId: string, viewId: string): string {
  return `view:${pluginId}:${viewId}`
}
const activeRuntimeTab = computed(() => {
  if (!activePluginTabId.value.startsWith('view:')) return undefined
  const key = activePluginTabId.value.slice('view:'.length)
  return pluginViews.value.find((entry) => `${entry.pluginId}:${entry.view.id}` === key)
})
const homeWidgetViews = computed(() => pluginViews.value.filter((entry) => entry.view.slot === 'home_widget'))
const settingsSectionViews = computed(() => pluginViews.value.filter((entry) => entry.view.slot === 'settings_section'))
const activePluginTab = computed(() => enabledPluginTabs.value.find((plugin) => plugin.id === activePluginTabId.value))
watch(enabledPluginTabs, (tabs) => {
  if (activePluginTabId.value && !tabs.some((plugin) => plugin.id === activePluginTabId.value)) setView('plugins')
})
watch(plugins, schedulePluginRuntimeSync)
const githubPluginInstallBusy = ref(false)
const localThemes = ref<LauncherThemePreset[]>([])
const themeOperationBusy = ref(false)
const githubThemeInstallBusy = ref(false)
const activeThemeId = ref('midnight-signal')
const createInstanceOpen = ref(false)
const creationKind = ref<'client' | 'bedrock' | 'server'>('client')
const bedrockCreationStatus = ref<BedrockRuntimeStatus | null>(null)
const bedrockCreationEdition = ref<'release' | 'preview'>('release')
const bedrockCreationVersion = ref('')
const bedrockCreationVersions = ref<Array<{ edition: string; version: string; installed: boolean }>>([])
const bedrockCreationVersionsBusy = ref(false)
const bedrockCreationVersionsError = ref('')
const serverCoreFileInput = ref<HTMLInputElement | null>(null)
const serverCoreDragActive = ref(false)
const modpackFileInput = ref<HTMLInputElement | null>(null)
const mobileMenuOpen = ref(false)
const instanceGroupFilter = ref('all')
const initializing = ref(true)
const firstRunReady = ref(false)
const firstRunPhase = ref<FirstRunPhase | null>(null)
const firstRunError = ref('')
const firstRunState = ref<FirstRunExperienceState>({ setupCompleted: false, tourCompleted: false })
const busy = ref(false)
watch(firstRunPhase, (phase) => {
  mobileMenuOpen.value = phase === 'tour' && window.innerWidth <= 760
})
const groupActionBusy = ref(false)
const externalImportBusy = ref(false)
const loadingVersions = ref(false)
const versionError = ref('')
const loadingChoices = ref(false)
const toast = ref('')
// Рассинхрон сборки: интерфейс и Python-бэкенд из разных версий. Из-за
// подмены только бэкенда «ломаются» случайные экраны (например, «код undefined»).
const buildStampMismatch = ref<{ frontend: string; backend: string } | null>(null)
const toastId = ref(0)
const toastToneClass = ref('status-dot--green')
const pageTransitionPulse = ref(true)
const versions = ref<VersionInfo[]>([])
// Порядок манифеста не гарантируется (зеркала отдают по-разному): сортируем
// сами от новых к старым, чтобы «первая версия» всегда была свежей.
function sortVersionsByRelease(list: VersionInfo[]): VersionInfo[] {
  return [...list].sort((a, b) => {
    const at = Date.parse(a.release_time || '') || 0
    const bt = Date.parse(b.release_time || '') || 0
    if (at !== bt) return bt - at
    return String(b.id).localeCompare(String(a.id), undefined, { numeric: true })
  })
}
const versionFilter = ref<'all' | 'release' | 'snapshot' | 'old_beta' | 'old_alpha'>('release')
const javaOptions = ref<SelectOption[]>([])
const loaderChoices = ref<LoaderChoice[]>([])
const selectedVersion = ref('1.20.1')
const selectedLoader = ref<LoaderId>('fabric')
const selectedLoaderVersion = ref('')
const selectedInstanceName = ref('')
const activeCatalogId = ref('modrinth')
const creationName = ref('')
const serverCreation = reactive({
  display_name: '',
  core: 'paper',
  version: '1.21.1',
  port: 25565,
  memory: '4GiB',
  eula_accepted: false,
  install_mode: 'official' as 'official' | 'custom',
  jar_name: '',
  jar_data: '',
})
const serverMemoryValue = ref(4)
const serverMemoryUnit = ref<MemoryUnit>('GiB')
const serverVersionOptions = ref<SelectOption[]>([])
const serverVersionsBusy = ref(false)
let serverVersionRequestId = 0
const selectedAccount = ref('auto')
watch(selectedAccount, (accountId) => {
  void broadcastPluginEvent('account.changed', { account_id: accountId === 'auto' ? null : accountId })
})
const instanceWorkspaceRef = ref<{ openConsoleIfOverview: () => boolean; openSettings: () => void; stop: () => Promise<void>; ensureLoaded?: () => Promise<void> } | null>(null)
const bedrockWorkspaceRef = ref<{ ensureLoaded: () => Promise<void>; applyPendingVersion: (edition: 'release' | 'preview', version: string) => void; refreshAccountStatus?: () => Promise<void> } | null>(null)
const serverWorkspaceRef = ref<{ openSettings: () => void; stop: () => Promise<void> } | null>(null)
const instanceWorkspaceFocus = ref<'overview' | 'settings' | 'console'>('overview')
const catalogServerCore = ref('paper')
const javaSelection = ref('auto')
const memoryValue = ref(8)
const memoryUnit = ref<MemoryUnit>('GiB')
const accountName = ref('Player')
const modQuery = ref('')
const modProjectType = ref<ProjectType>('mod')
const modSortMode = ref<CatalogSort>('relevance')
const modResults = ref<ModResult[]>([])
const modSearchPerformed = ref(false)
const modBusy = ref(false)
const modError = ref('')
const modInstallId = ref('')
const modAction = ref<'install' | 'remove'>('install')
const modInstallProgress = ref(0)
const modInstallDetail = ref('')
const skinResults = ref<SkinInfo[]>([])
const skinPanelOpen = ref(false)
const skinAccountId = ref('')
const skinProfileData = ref<SkinProfile>()
const skinProfileLoading = ref(false)
const skinProfileError = ref('')
const selectedSkinId = ref('')
const skinModel = ref<'classic' | 'slim'>('classic')
const currentSkinModel = ref<'classic' | 'slim'>('classic')
const selectedCapeId = ref('')
const capeActionBusy = ref(false)
const skinFileInput = ref<HTMLInputElement | null>(null)
let skinProfileRequestId = 0
const accountMenuId = ref('')
const pendingDatapack = ref<ModResult>()
const datapackWorlds = ref<InstanceWorld[]>([])
const datapackDialogOpen = ref(false)
const datapackDialogElement = ref<HTMLElement | null>(null)
function closeDatapackDialog() { datapackDialogOpen.value = false }
const handleDatapackDialogKeydown = useModalFocus(datapackDialogOpen, datapackDialogElement, closeDatapackDialog)
const datapackWorldId = ref('')
const catalogDetailsOpen = ref(false)
const catalogDetailsLoading = ref(false)
const catalogDetailsError = ref('')
const catalogDetailsProject = ref<ModResult>()
const catalogDetails = ref<Record<string, unknown>>()
const catalogDetailsType = ref<ProjectType>('mod')
const catalogDetailsVersions = ref<Array<{ id: string; version_number: string; name: string; date_published: string; game_versions: string[]; loaders: string[] }>>([])
let catalogDetailsRequestId = 0
const catalogSortOptions: SelectOption[] = [
  { value: 'relevance', label: 'По релевантности' },
  { value: 'downloads', label: 'Больше загрузок' },
  { value: 'newest', label: 'Сначала новые' },
  { value: 'updated', label: 'Недавно обновлялись' },
]
const activeRemoteSkin = computed(() => skinProfileData.value?.skins?.find((skin) => skin.state?.toUpperCase() === 'ACTIVE') || skinProfileData.value?.skins?.[0])
const availableThemes = computed(() => [...launcherThemes, ...localThemes.value])
const selectedLocalSkin = computed(() => skinResults.value.find((skin) => skin.skin_id === selectedSkinId.value))
const skinPreviewImage = computed(() => selectedLocalSkin.value?.image || activeRemoteSkin.value?.url || '')
const activeCape = computed(() => skinProfileData.value?.capes?.find((cape) => cape.state?.toUpperCase() === 'ACTIVE') || skinProfileData.value?.capes?.[0])
const availableCapes = computed(() => {
  const capes = skinProfileData.value?.capes || []
  const active = capes.find((cape) => cape.id ? cape.id === activeCape.value?.id : cape === activeCape.value)
  return active ? [active, ...capes.filter((cape) => cape !== active)] : capes
})
const selectedCape = computed(() => availableCapes.value.find((cape) => cape.id === selectedCapeId.value) || (!selectedCapeId.value ? activeCape.value : undefined))
const graphicsReport = ref('')
const systemDark = ref(false)
const javaValidation = ref('')
const appShellRef = ref<HTMLElement | null>(null)
const wallpaperPreviewUrl = ref('')
const wallpaperProcessedUrl = ref('')
const wallpaperLoadFailed = ref(false)
const wallpaperRenderBlur = ref(12)
let wallpaperResizeTimer: number | undefined
const sidebarResizeActive = ref(false)
// Дефолтная папка игры для автозаполнения: если бэкенд ещё не отдал settings
// (или недоступен), поле «Папка игры» не должно оставаться пустым. `~`
// впоследствии раскрывается бэкендом (Path.expanduser) — на Windows это
// %USERPROFILE%, на Linux — $HOME.
const defaultGameDirectory = computed(() => {
  const platform = `${state.system.platform} ${typeof navigator !== 'undefined' ? navigator.platform : ''}`.toLowerCase()
  return platform.includes('win')
    ? '~/AppData/Local/nivora-launcher/game'
    : '~/.local/share/nivora-launcher/game'
})
const settingsDraft = reactive({
  game_dir: '',
  java_path: '',
  memory: 'auto',
  max_downloads: 8,
  startup_view: 'instances' as ViewName,
  last_view: 'instances' as ViewName,
  remember_last_view: true,
  default_version_filter: 'release',
  default_loader: 'fabric' as LoaderId,
  default_account_id: 'auto',
  open_last_instance: true,
  launch_after_install: false,
  auto_refresh_metadata: true,
  metadata_refresh_hours: 6,
  download_timeout_seconds: 30,
  download_retries: 5,
  confirm_destructive_actions: true,
  show_activity_panel: true,
  show_toasts: true,
  minimize_on_launch: false,
  language: null as string | null,
    font_family: 'system',
  keep_logs_days: 14,
  telemetry: false,
  telemetry_endpoint: '',
  keyboard_shortcuts: { open_settings: 'Ctrl+,', refresh: 'Ctrl+R', new_instance: 'Ctrl+N', focus_search: 'Ctrl+K', toggle_activity: 'Ctrl+J' } as Record<string, string>,
  graphics_mode: 'auto' as 'auto' | 'nvidia' | 'mesa' | 'software',
  overlay_settings: { ...DEFAULT_OVERLAY_SETTINGS } as OverlaySettings,
  backup_schedule: { enabled: false, interval_hours: 24, keep: 5, destination: 'local' } as BackupSchedule,
  launcher_profiles: [{ ...defaultLauncherProfile, jvm_arguments: [], game_arguments: [] }] as LauncherProfile[],
  active_launcher_profile_id: 'default',
  http_proxy: '',
  developer_mode: false,
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
  ] as ResourceCatalog[],
  active_catalog_id: 'modrinth',
  plugin_catalogs: [
    { id: 'modrinth-plugins', name: 'Modrinth', kind: 'modrinth', api_url: 'https://api.modrinth.com/v2', enabled: true },
    { id: 'spigot', name: 'Spigot', kind: 'spigot', api_url: 'https://api.spiget.org/v2', enabled: true },
    { id: 'hangar', name: 'Hangar', kind: 'hangar', api_url: 'https://hangar.papermc.io/api/v1', enabled: true },
  ] as PluginCatalog[],
  active_plugin_catalog_id: 'modrinth-plugins',
  cloud_url: '',
  cloud_username: '',
  cloud_remote_path: 'nivora-launcher',
  cloud_include_accounts: false,
  cloud_last_sync: 0,
  cloud_configured: false,
})
const settingsSaveState = ref<'saved' | 'dirty' | 'saving' | 'error'>('saved')
const settingsSaveMessage = ref('Изменения сохранены')
const updaterBusy = ref(false)
// Язык выбирается первым шагом первоначальной настройки
// (FirstRunExperience, фаза 'language'), отдельной модалки больше нет.

const languageOptions: SelectOption[] = [
  { value: 'ru', label: 'Русский' },
  { value: 'en', label: 'English' },
]

function setLanguageFromSettings(value: string): void {
  const next: Locale = value === 'en' ? 'en' : 'ru'
  settingsDraft.language = next
  // Применяем сразу — не ждём «Сохранить»: язык видно и без сохранения,
  // но в settings.json попадает только после сохранения настроек.
  setLocale(next)
}
const updaterStatus = ref('Самообновление не проверялось в этой сессии.')
const updaterVersion = ref('')
const updaterNotes = ref('')
const updaterProgressDownloaded = ref(0)
const updaterProgressTotal = ref(0)
let pendingAppUpdate: any = null
const lastLaunchCrash = ref<CrashReport>()
const hydratingSettings = ref(false)
const fontFamilyCss = computed(() => {
  // Windows-первичный стек: без явных 'Segoe UI Variable Text'/'Segoe UI'
  // WebView2 на свежих Windows 11 подставляет системный дефолт, который
  // выглядит заметно беднее фирменной гарнитуры. Inter/Manrope оставлены
  // как выбор пользователя, но с честным фолбэком на Segoe UI.
  const families: Record<string, string> = {
    system: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI Variable Text', 'Segoe UI', Roboto, sans-serif",
    inter: "Inter, 'Segoe UI Variable Text', 'Segoe UI', system-ui, sans-serif",
    manrope: "Manrope, Inter, 'Segoe UI Variable Text', 'Segoe UI', system-ui, sans-serif",
    mono: "'DM Mono', 'Cascadia Mono', 'Cascadia Code', Consolas, ui-monospace, monospace",
  }
  return families[settingsDraft.font_family] || families.system
})
const appearanceDraft = reactive<AppearanceSettings>({
   theme: 'dark',
    active_theme_id: 'midnight-signal',
  wallpaper_path: '',
  wallpaper_data: '',
  wallpaper_blur: 12,
  wallpaper_opacity: 34,
    accent_color: defaultColorTokens.accent,
  sidebar_position: 'left',
  sidebar_mode: 'adaptive',
  sidebar_width_mode: 'content',
  sidebar_width: 228,
  sidebar_floating: false,
   sidebar_visibility: 'always',
   animations: 'full',
   density: 'comfortable',
   ui_scale: 100,
   radius_tokens: { ...defaultRadiusTokens },
   animation_settings: {
     ...defaultAnimationSettings,
     page_styles: { ...defaultAnimationSettings.page_styles },
     page_durations_ms: { ...defaultAnimationSettings.page_durations_ms },
   } as AnimationSettings,
    page_themes: { instances: 'inherit', 'instance-settings': 'inherit', accounts: 'inherit', mods: 'inherit', modpacks: 'inherit', plugins: 'inherit', themes: 'inherit', settings: 'inherit' },
    page_layouts: { instances: 'balanced', 'instance-settings': 'balanced', accounts: 'balanced', mods: 'wide', modpacks: 'wide', plugins: 'wide', themes: 'wide', settings: 'balanced' },
  widget_visibility: { 'instances.workspace': true, 'accounts.status': true, 'mods.context': true },
  color_tokens: { ...defaultColorTokens },
})
const activity = reactive<ActivityState>({
  open: false,
  title: 'Рабочая область',
  detail: 'Готово к запуску.',
  progress: 0,
  busy: false,
  cancelling: false,
  logs: [],
})
const LAST_INSTANCE_STORAGE_KEY = 'nivora-launcher:last-instance'
let lastViewSaveRequest = 0

type SettingsDraftSnapshot = {
  settings: Record<string, unknown>
  appearance: Record<string, unknown>
}
let savedSettingsSnapshot: SettingsDraftSnapshot | undefined

function cloneSettingsValue(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(cloneSettingsValue)
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, cloneSettingsValue(item)]))
  }
  return value
}

function currentSettingsDraftValues(): SettingsDraftSnapshot {
  return {
    settings: { ...settingsDraft, memory: memorySetting.value, java_path: effectiveJavaPath() },
    appearance: appearanceDraft as unknown as Record<string, unknown>,
  }
}

function takeSettingsDraftSnapshot(): SettingsDraftSnapshot {
  return cloneSettingsValue(currentSettingsDraftValues()) as SettingsDraftSnapshot
}

function settingsValuesMatch(left: unknown, right: unknown): boolean {
  if (Object.is(left, right)) return true
  if (!left || !right || typeof left !== 'object' || typeof right !== 'object') return false
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left) && Array.isArray(right) && left.length === right.length && left.every((value, index) => settingsValuesMatch(value, right[index]))
  }
  const leftRecord = left as Record<string, unknown>
  const rightRecord = right as Record<string, unknown>
  const leftKeys = Object.keys(leftRecord)
  const rightKeys = Object.keys(rightRecord)
  return leftKeys.length === rightKeys.length && leftKeys.every((key) => Object.hasOwn(rightRecord, key) && settingsValuesMatch(leftRecord[key], rightRecord[key]))
}

function refreshSettingsSaveState() {
  if (!savedSettingsSnapshot || hydratingSettings.value || settingsSaveState.value === 'saving') return
  const saved = settingsValuesMatch(currentSettingsDraftValues(), savedSettingsSnapshot)
  settingsSaveState.value = saved ? 'saved' : 'dirty'
  settingsSaveMessage.value = saved ? 'Изменения сохранены' : 'Есть несохранённые изменения'
}

let settingsSaveStateTimer: number | undefined
function scheduleSettingsSaveStateRefresh() {
  if (settingsSaveStateTimer !== undefined) window.clearTimeout(settingsSaveStateTimer)
  settingsSaveStateTimer = window.setTimeout(() => {
    settingsSaveStateTimer = undefined
    refreshSettingsSaveState()
  }, 250)
}

function mergeSavedSettingsBaseline(patch: Record<string, unknown>) {
  if (!savedSettingsSnapshot) {
    savedSettingsSnapshot = takeSettingsDraftSnapshot()
  } else {
    savedSettingsSnapshot = {
      ...savedSettingsSnapshot,
      settings: { ...savedSettingsSnapshot.settings, ...(cloneSettingsValue(patch) as Record<string, unknown>) },
    }
  }
  refreshSettingsSaveState()
}

watch([settingsDraft, appearanceDraft, memoryValue, memoryUnit, javaSelection], scheduleSettingsSaveStateRefresh, { deep: true })

const page = computed(() => activePluginTab.value
  ? { title: activePluginTab.value.label, kicker: 'Вкладка плагина' }
  : views[currentView.value])
const resolvedGlobalTheme = computed(() => {
  if (appearanceDraft.theme === 'system') return systemDark.value ? 'dark' : 'light'
  return appearanceDraft.theme
})
const currentTheme = computed(() => {
  const override = appearanceDraft.page_themes[currentView.value]
  if (!override || override === 'inherit') return resolvedGlobalTheme.value
  return override
})
const currentLayout = computed(() => appearanceDraft.page_layouts[currentView.value] || 'balanced')
const effectiveColorTokens = computed<Record<string, string>>(() => {
  const base = currentTheme.value === 'light' ? lightColorTokens : defaultColorTokens
  return Object.fromEntries(
    Object.entries(defaultColorTokens).map(([key, fallback]) => [
      key,
      appearanceDraft.color_tokens[key] === fallback ? base[key] : appearanceDraft.color_tokens[key] || base[key],
    ]),
  )
})
const effectiveAccent = computed(() => effectiveColorTokens.value.accent)
const wallpaperUrl = computed(() => wallpaperPreviewUrl.value || appearanceDraft.wallpaper_data || (appearanceDraft.wallpaper_path ? bridge.fileUrl(appearanceDraft.wallpaper_path) : ''))
const wallpaperDisplayUrl = computed(() => wallpaperProcessedUrl.value || wallpaperUrl.value)
const accentForeground = computed(() => {
  const hex = effectiveAccent.value.replace('#', '')
  const red = Number.parseInt(hex.slice(0, 2), 16) || 0
  const green = Number.parseInt(hex.slice(2, 4), 16) || 0
  const blue = Number.parseInt(hex.slice(4, 6), 16) || 0
  return (red * 299 + green * 587 + blue * 114) / 1000 > 154 ? '#080808' : '#ffffff'
})
const sidebarWidth = computed(() => {
  if (appearanceDraft.sidebar_position === 'top' || appearanceDraft.sidebar_position === 'bottom') return 252
  if (appearanceDraft.sidebar_mode === 'compact') return 72
  if (appearanceDraft.sidebar_width_mode === 'full') return 300
  if (appearanceDraft.sidebar_width_mode === 'manual') return Math.min(360, Math.max(168, appearanceDraft.sidebar_width))
  return 252
})
const sidebarHeight = computed(() => {
  if (appearanceDraft.sidebar_position !== 'top' && appearanceDraft.sidebar_position !== 'bottom') return 0
  return appearanceDraft.sidebar_mode === 'compact' ? 128 : 184
})
const activeLauncherProfile = computed(() => settingsDraft.launcher_profiles.find((profile) => profile.id === settingsDraft.active_launcher_profile_id) || settingsDraft.launcher_profiles[0])
const effectiveAnimationStyle = computed<AnimationStyle>(() => {
  if (appearanceDraft.animations === 'off') return 'none'
  return appearanceDraft.animation_settings.page_styles[currentView.value] || appearanceDraft.animation_settings.page_style
})
const effectiveAnimationDuration = computed(() => appearanceDraft.animation_settings.page_durations_ms[currentView.value] || appearanceDraft.animation_settings.page_duration_ms)
const appStyle = computed<Record<string, string>>(() => ({
  '--bg': effectiveColorTokens.value.background,
  '--panel': effectiveColorTokens.value.panel,
  '--panel-raised': effectiveColorTokens.value.panel_raised,
  '--panel-soft': effectiveColorTokens.value.panel_soft,
  '--text': effectiveColorTokens.value.text,
  '--muted': effectiveColorTokens.value.muted,
  '--quiet': effectiveColorTokens.value.quiet,
  '--blue-bright': effectiveColorTokens.value.accent_strong,
  '--green': effectiveColorTokens.value.success,
  '--danger': effectiveColorTokens.value.danger,
  '--warning': effectiveColorTokens.value.warning,
  '--line': `color-mix(in srgb, ${effectiveColorTokens.value.border} 16%, transparent)`,
  '--line-strong': `color-mix(in srgb, ${effectiveColorTokens.value.border} 30%, transparent)`,
  '--accent': effectiveAccent.value,
  '--accent-foreground': accentForeground.value,
  '--blue': effectiveAccent.value,
  '--brand-color': currentTheme.value === 'light' ? '#111214' : '#f4f7ff',
  '--sidebar-bg': `color-mix(in srgb, ${effectiveColorTokens.value.panel} 92%, ${effectiveColorTokens.value.background})`,
  '--ui-scale': `${appearanceDraft.ui_scale / 100}`,
  '--radius': `${appearanceDraft.radius_tokens.card}px`,
  '--radius-card': `${appearanceDraft.radius_tokens.card}px`,
  '--radius-button': `${appearanceDraft.radius_tokens.button}px`,
  '--radius-control': `${appearanceDraft.radius_tokens.control}px`,
  '--banner-shade': effectiveColorTokens.value.banner_shade,
  '--wallpaper-opacity': `${appearanceDraft.wallpaper_opacity / 100}`,
  '--sidebar-width': `${sidebarWidth.value}px`,
  '--sidebar-height': `${sidebarHeight.value}px`,
  '--font-family': fontFamilyCss.value,
  '--page-duration': `${effectiveAnimationDuration.value}ms`,
  '--surface-duration': `${appearanceDraft.animation_settings.surface_duration_ms}ms`,
  '--surface-stagger': `${appearanceDraft.animation_settings.surface_stagger_ms}ms`,
}))
const toastThemeStyle = computed<Record<string, string>>(() => ({
  '--text': effectiveColorTokens.value.text,
  '--muted': effectiveColorTokens.value.muted,
  '--panel-raised': effectiveColorTokens.value.panel_raised,
  '--panel-soft': effectiveColorTokens.value.panel_soft,
  '--line-strong': `color-mix(in srgb, ${effectiveColorTokens.value.border} 30%, transparent)`,
  '--accent': effectiveAccent.value,
  '--green': effectiveColorTokens.value.success,
  '--danger': effectiveColorTokens.value.danger,
  '--warning': effectiveColorTokens.value.warning,
}))

// Keep the static shell and its large page sections in sync without relying on a deep root patch.
// Reactive inputs are read synchronously (so Vue tracks them), but DOM writes are coalesced into
// a single requestAnimationFrame batch — dragging sliders or colors no longer triggers a full
// style recalculation per input event.
type ShellSnapshot = {
  animations: string
  uiScale: number
  layout: string
  animationStyle: string
  widgetAccountsHidden: boolean
  wallpaperActive: boolean
  theme: string
  sidebarPosition: string
  currentView: string
  sidebarMode: string
  sidebarWidthMode: string
  sidebarFloating: string
  sidebarVisibility: string
  density: string
  style: Record<string, string>
  initializing: boolean
}
let latestShellSnapshot: ShellSnapshot | null = null
let shellFlushScheduled = false

function applyShellSnapshot(snapshot: ShellSnapshot) {
  document.body.dataset.launcherAnimations = snapshot.animations
  document.documentElement.style.setProperty('--ui-scale', String(snapshot.uiScale / 100))
  const element = appShellRef.value
  if (!element) return
  element.className = [
    'app-shell',
    `page-layout--${snapshot.layout}`,
    `animations--${snapshot.animations}`,
    `animation-style--${snapshot.animationStyle}`,
    snapshot.widgetAccountsHidden ? 'widget-account-status-hidden' : '',
    snapshot.wallpaperActive ? 'wallpaper-active' : '',
  ].filter(Boolean).join(' ')
  element.dataset.theme = snapshot.theme
  element.dataset.sidebarPosition = snapshot.sidebarPosition
  element.dataset.currentView = snapshot.currentView
  element.dataset.sidebarMode = snapshot.sidebarMode
  element.dataset.sidebarWidthMode = snapshot.sidebarWidthMode
  element.dataset.sidebarFloating = snapshot.sidebarFloating
  element.dataset.sidebarVisibility = snapshot.sidebarVisibility
  element.dataset.density = snapshot.density
  for (const [property, value] of Object.entries(snapshot.style)) element.style.setProperty(property, value)
  for (const section of element.querySelectorAll<HTMLElement>('[data-page-view]')) {
    section.style.display = section.dataset.pageView === snapshot.currentView ? '' : 'none'
  }
  const bootOverlay = element.querySelector<HTMLElement>('.boot-overlay')
  if (bootOverlay) bootOverlay.style.display = snapshot.initializing ? '' : 'none'
}

function scheduleShellFlush(snapshot: ShellSnapshot) {
  latestShellSnapshot = snapshot
  if (shellFlushScheduled) return
  shellFlushScheduled = true
  window.requestAnimationFrame(() => {
    shellFlushScheduled = false
    if (latestShellSnapshot) applyShellSnapshot(latestShellSnapshot)
  })
}

watchEffect(() => {
  scheduleShellFlush({
    animations: appearanceDraft.animations,
    uiScale: appearanceDraft.ui_scale,
    layout: currentLayout.value,
    animationStyle: effectiveAnimationStyle.value,
    widgetAccountsHidden: appearanceDraft.widget_visibility['accounts.status'] === false,
    wallpaperActive: Boolean(wallpaperUrl.value) && !wallpaperLoadFailed.value,
    theme: currentTheme.value,
    sidebarPosition: appearanceDraft.sidebar_position,
    currentView: currentView.value,
    sidebarMode: appearanceDraft.sidebar_mode,
    sidebarWidthMode: appearanceDraft.sidebar_width_mode,
    sidebarFloating: String(appearanceDraft.sidebar_floating),
    sidebarVisibility: appearanceDraft.sidebar_visibility,
    density: appearanceDraft.density,
    style: appStyle.value,
    initializing: initializing.value,
  })
})
const selectedInstance = computed(() =>
  state.instances.find((value) => value.name === selectedInstanceName.value),
)
const instanceOptions = computed<SelectOption[]>(() => [
  {
    value: '',
    label: 'Поиск без экземпляра',
    meta: 'Только подбор по версии и loader/core; установка недоступна',
  },
  ...state.instances.filter((instance) => modProjectType.value === 'plugin' ? instance.kind === 'server' : instance.kind !== 'server' && instance.game_edition !== 'bedrock').map((instance) => ({
    value: instance.name,
    label: instance.display_name || instance.name,
    meta: `${instance.name} · ${instance.kind === 'server' ? (instance.server_core_version || 'версия ядра не указана') : (instance.minecraft_version || instance.version_id || 'без версии')} · ${instance.kind === 'server' ? (instance.server_core || 'server') : instance.loader}`,
  })),
])
const resourceCatalogOptions = computed<SelectOption[]>(() =>
  settingsDraft.resource_catalogs.map((catalog) => ({
    value: catalog.id,
    label: catalog.name,
    meta: catalog.kind === 'ftb' || catalog.kind === 'technic' ? 'Каталог модпаков' : catalog.kind === 'modrinth' ? 'Modrinth API' : 'Совместимый Modrinth API',
  })),
)
const catalogPickerOptions = computed<SelectOption[]>(() => {
  const catalogs = currentView.value === 'mods' && modProjectType.value === 'plugin'
    ? settingsDraft.plugin_catalogs.filter((catalog) => catalog.enabled !== false)
    : currentView.value === 'modpacks'
      ? settingsDraft.resource_catalogs.filter((catalog) => ['modrinth', 'ftb', 'technic'].includes(catalog.kind))
      : settingsDraft.resource_catalogs.filter((catalog) => !['ftb', 'technic'].includes(catalog.kind))
  return catalogs.map((catalog) => ({
    value: catalog.id,
    label: catalog.name,
    meta: catalog.kind === 'modrinth' ? 'Modrinth API' : catalog.kind === 'custom' ? 'Совместимый API для ресурсов' : catalog.kind === 'spigot' || catalog.kind === 'hangar' ? 'Каталог плагинов' : 'Каталог модпаков',
  }))
})
const activeResourceCatalog = computed(() => settingsDraft.resource_catalogs.find((catalog) => catalog.id === activeCatalogId.value) || settingsDraft.resource_catalogs[0])
const activePluginCatalog = computed(() => settingsDraft.plugin_catalogs.find((catalog) => catalog.id === activeCatalogId.value) || settingsDraft.plugin_catalogs.find((catalog) => catalog.id === settingsDraft.active_plugin_catalog_id) || settingsDraft.plugin_catalogs[0])
const activeCatalog = computed(() => currentView.value === 'mods' && modProjectType.value === 'plugin' ? activePluginCatalog.value : activeResourceCatalog.value)
const serverCoreOptions = computed<SelectOption[]>(() =>
  (state.server_cores || [])
    .filter((core: ServerCore) => core.id !== 'custom' && core.family !== 'Proxy')
    .map((core: ServerCore) => ({ value: core.id, label: core.name, meta: `${core.family} · ${core.description}` })),
)
const catalogServerCoreOptions = computed<SelectOption[]>(() => serverCoreOptions.value.length ? serverCoreOptions.value : [{ value: 'paper', label: 'Paper', meta: 'Ядро по умолчанию' }])
const catalogKindOptions: SelectOption[] = [
  { value: 'modrinth', label: 'Modrinth', meta: 'Официальный каталог' },
  { value: 'ftb', label: 'Feed The Beast', meta: 'Каталог модпаков' },
  { value: 'technic', label: 'Technic', meta: 'Каталог модпаков' },
  { value: 'custom', label: 'Совместимый API', meta: 'Свой каталог ресурсов' },
]
const pluginCatalogKindOptions: SelectOption[] = [
  { value: 'modrinth', label: 'Modrinth', meta: 'project_type:plugin' },
  { value: 'spigot', label: 'Spigot', meta: 'Spiget resources API' },
  { value: 'hangar', label: 'Hangar', meta: 'PaperMC catalog' },
  { value: 'custom', label: 'Custom API', meta: 'Свой HTTPS endpoint' },
]
const modInstanceVersion = computed(() => {
  const instance = selectedInstance.value
  if (!instance) return selectedVersion.value
  return instance.kind === 'server'
    ? instance.server_core_version || instance.minecraft_version || instance.version_id || ''
    : instance.minecraft_version || instance.version_id || ''
})
const microsoftAccount = computed(() => state.accounts.find((value) => value.kind === 'microsoft'))
const defaultAccount = computed(() => state.accounts.find((value) => value.is_default))
const effectiveAccount = computed<AccountInfo | undefined>(() => {
  if (selectedAccount.value === 'auto') return defaultAccount.value || microsoftAccount.value || state.accounts[0]
  return state.accounts.find((value) => value.account_id === selectedAccount.value)
})
const selectedLoaderLabel = computed(
  () => loaders.find((value) => value.id === selectedLoader.value)?.label || selectedLoader.value,
)
const modContextLabel = computed(() => {
  const instance = selectedInstance.value
  if (instance?.kind === 'server') return `${modInstanceVersion.value} · ${instance.server_core || 'server'}`
  if (modProjectType.value === 'plugin') return `${modInstanceVersion.value} · ${catalogServerCore.value}`
  return `${modInstanceVersion.value} · ${selectedLoaderLabel.value}`
})
const filteredVersions = computed(() => {
  if (versionFilter.value === 'all') return versions.value
  return versions.value.filter((version) => version.type === versionFilter.value)
})
const versionOptions = computed<SelectOption[]>(() =>
  filteredVersions.value.length
    ? filteredVersions.value.map((version) => ({ value: version.id, label: version.id, meta: version.type }))
    : [{ value: selectedVersion.value, label: selectedVersion.value, meta: 'локальный fallback' }],
)
async function refreshServerCoreVersions(core: string) {
  const requestId = ++serverVersionRequestId
  serverVersionsBusy.value = true
  try {
    const result = await bridge.serverCoreVersions(core)
    if (requestId !== serverVersionRequestId) return
    const versionsForCore = (result.versions || []).filter(Boolean)
    serverVersionOptions.value = versionsForCore.map((version) => ({ value: version, label: version, meta: 'Официальный каталог ядра' }))
    if (!serverVersionOptions.value.length) serverVersionOptions.value = versionOptions.value
    if (!serverVersionOptions.value.some((option) => option.value === serverCreation.version)) {
      serverCreation.version = serverVersionOptions.value[0]?.value || serverCreation.version
    }
  } catch {
    if (requestId === serverVersionRequestId) serverVersionOptions.value = versionOptions.value
  } finally {
    if (requestId === serverVersionRequestId) serverVersionsBusy.value = false
  }
}
const loaderVersionOptions = computed<SelectOption[]>(() => [
  {
    value: '',
    label: loadingChoices.value ? 'Получаю версии…' : 'Последняя доступная',
    meta: loadingChoices.value ? 'Подключение к metadata API' : 'Автоматический выбор',
  },
  ...loaderChoices.value.map((choice) => ({ value: choice.version, label: choice.version, meta: choice.loader })),
])
const accountOptions = computed<SelectOption[]>(() => [
  { value: 'auto', label: 'Автоматически', meta: effectiveAccount.value?.display_name || 'Offline fallback' },
  ...state.accounts.map((account) => ({ value: account.account_id, label: account.display_name, meta: account.kind })),
])
const javaChoiceOptions = computed<SelectOption[]>(() => [
  ...javaOptions.value,
  { value: 'manual', label: 'Указать вручную', meta: 'Любой executable Java' },
])
const graphicsOptions: SelectOption[] = [
  { value: 'auto', label: 'Автоматически', meta: 'Определить драйвер' },
  { value: 'nvidia', label: 'NVIDIA GLX', meta: 'RTX / proprietary driver' },
  { value: 'mesa', label: 'Mesa GLX', meta: 'Mesa / AMD / Intel' },
  { value: 'software', label: 'Software llvmpipe', meta: 'Fallback при GLXBadFBConfig' },
]
const jvmPresetOptions: SelectOption[] = [
  { value: 'none', label: 'Без пресета', meta: 'Аргументы только из профиля' },
  { value: 'fps', label: 'FPS', meta: 'низкие паузы GC' },
  { value: 'balanced', label: 'Balanced', meta: 'универсальный вариант' },
  { value: 'heavy_modpack', label: 'Heavy Modpack', meta: 'Aikar’s' },
  { value: 'compatibility', label: 'Compatibility', meta: 'старые сборки и моды' },
  { value: 'streaming', label: 'Streaming', meta: 'стабильные кадры записи' },
]
const recordingEncoderOptions: SelectOption[] = [
  { value: 'auto', label: 'Автоматически', meta: 'NVENC → VAAPI → x264' },
  { value: 'nvenc', label: 'NVENC', meta: 'Аппаратно, NVIDIA' },
  { value: 'vaapi', label: 'VAAPI', meta: 'Аппаратно, Intel/AMD' },
  { value: 'x264', label: 'libx264', meta: 'Процессор, очень быстрый пресет' },
]
const recordingContainerOptions: SelectOption[] = [
  { value: 'mp4', label: 'MP4', meta: 'Универсальный' },
  { value: 'mkv', label: 'MKV', meta: 'Устойчив к прерываниям' },
]
const overlayPositionOptions: SelectOption[] = [
  { value: 'top-left', label: 'Слева сверху', meta: 'Как в Afterburner' },
  { value: 'top-right', label: 'Справа сверху', meta: '' },
  { value: 'bottom-left', label: 'Слева снизу', meta: '' },
  { value: 'bottom-right', label: 'Справа снизу', meta: '' },
]
const systemMetricsDisplayOptions: SelectOption[] = [
  { value: 'overlay', label: 'Только оверлей', meta: 'По горячей клавише в игре' },
  { value: 'always', label: 'Постоянно', meta: 'Полоса метрик в верхней панели лаунчера' },
]
const versionFilterOptions: SelectOption[] = [
  { value: 'release', label: 'Релизы', meta: 'Стабильные версии' },
  { value: 'snapshot', label: 'Снапшоты', meta: 'Недельные сборки и pre/rc' },
  { value: 'old_beta', label: 'Бета', meta: 'Исторические beta-версии' },
  { value: 'old_alpha', label: 'Альфа', meta: 'Исторические alpha-версии' },
  { value: 'all', label: 'Все версии', meta: 'Релизы, снапшоты и alpha/beta' },
]
const loginSourceOptions: SelectOption[] = [
  { value: 'microsoft', label: 'Microsoft OAuth', meta: 'Автоматический device-code вход' },
  { value: 'disabled', label: 'Отключён', meta: 'Скрыть внешний вход' },
]
const catalogSourceOptions: SelectOption[] = [
  { value: 'modrinth', label: 'Modrinth API', meta: 'Официальный endpoint по умолчанию' },
  { value: 'custom', label: 'Свой каталог', meta: 'Совместимый API endpoint' },
]
const dataSourceOptions: SelectOption[] = [
  { value: 'mojang', label: 'Mojang metadata', meta: 'Официальный manifest по умолчанию' },
  { value: 'custom', label: 'Свой источник', meta: 'Совместимый version manifest' },
]
const modProjectOptions: SelectOption[] = [
  { value: 'mod', label: 'Моды', meta: 'project_type:mod' },
  { value: 'resourcepack', label: 'Ресурспаки', meta: 'project_type:resourcepack' },
  { value: 'shader', label: 'Шейдеры', meta: 'project_type:shader' },
  { value: 'datapack', label: 'Датапаки', meta: 'project_type:datapack' },
  { value: 'plugin', label: 'Плагины сервера', meta: 'Только server workspace' },
]
const selectedProjectLabel = computed(() => currentView.value === 'modpacks' ? 'Модпаки' : modProjectOptions.find((option) => option.value === modProjectType.value)?.label || 'Каталог')
const memoryUnitOptions: SelectOption[] = [
  { value: 'KiB', label: 'KiB', meta: '1024 байта' },
  { value: 'MiB', label: 'MiB', meta: '1024 KiB' },
  { value: 'GiB', label: 'GiB', meta: '1024 MiB' },
]
const memoryMultipliers: Record<MemoryUnit, number> = { KiB: 1, MiB: 1024, GiB: 1024 * 1024 }
const memoryMultiplier = computed(() => memoryMultipliers[memoryUnit.value])
const memoryMaxValue = computed(() => {
  const total = state.settings.memory_total_kib
  return total > 0 ? Math.max(1, Math.floor(total / memoryMultiplier.value)) : undefined
})
const memoryTotalLabel = computed(() => formatMemoryLabel(state.settings.memory_total_kib))
const memorySetting = computed(() => `${Math.max(1, Math.floor(memoryValue.value || 1))}${memoryUnit.value}`)
const firstRunMemoryGiB = computed(() => Math.max(1, Math.round((Number(memoryValue.value || 1) * memoryMultipliers[memoryUnit.value]) / (1024 * 1024))))
const firstRunRecommendedMemoryGiB = computed(() => Math.max(1, Math.round((state.settings.memory_recommended_kib || 4 * 1024 * 1024) / (1024 * 1024))))
const firstRunMaximumMemoryGiB = computed(() => Math.max(1, Math.floor((state.settings.memory_total_kib || 8 * 1024 * 1024) / (1024 * 1024))))
const serverMemoryLimitKiB = computed(() => {
  const total = Number(state.settings.memory_total_kib || 0)
  return total > 0 ? Math.max(1, Math.floor(total * 0.75)) : 2 * 1024 * 1024
})
const serverMemoryDecimals = computed(() => serverMemoryUnit.value === 'GiB' ? 2 : 0)
const serverMemoryMaxValue = computed(() => {
  const precision = 10 ** serverMemoryDecimals.value
  return Math.max(0.01, Math.floor(serverMemoryLimitKiB.value / memoryMultipliers[serverMemoryUnit.value] * precision) / precision)
})
const serverMemoryMinimum = computed(() => {
  const minimum = serverMemoryUnit.value === 'GiB' ? 0.25 : serverMemoryUnit.value === 'MiB' ? 128 : 128 * 1024
  return Math.min(minimum, serverMemoryMaxValue.value)
})
const serverMemoryStep = computed(() => Math.min(serverMemoryMinimum.value, serverMemoryMaxValue.value))
const serverMemoryLimitLabel = computed(() => formatMemoryLabel(serverMemoryLimitKiB.value))
const serverMemorySetting = computed(() => {
  const minimum = Math.min(serverMemoryMinimum.value, serverMemoryMaxValue.value)
  const value = Math.min(serverMemoryMaxValue.value, Math.max(minimum, Number(serverMemoryValue.value) || minimum))
  return `${Number(value.toFixed(serverMemoryDecimals.value))}${serverMemoryUnit.value}`
})
const toastDurationMs = 2800
let toastTimer: number | undefined
let activityCloseTimer: number | undefined
let loaderChoiceTimer: number | undefined
let modSearchTimer: number | undefined
let modSearchRequestId = 0
let activityLogTimer: number | undefined
let activityLogPollInFlight = false
let microsoftStatusTimer: number | undefined
let microsoftStatusPollInFlight = false
let stopBackendEvents: (() => void) | undefined
let systemThemeMedia: MediaQueryList | undefined
let systemThemeListener: ((event: MediaQueryListEvent) => void) | undefined
const loaderRequestId = ref(0)
let startupViewApplied = false
let userNavigatedDuringBootstrap = false

function clearPluginNotificationToast(toastIdToClear: number) {
  for (const [pluginId, notifications] of pluginNotificationToastIds) {
    for (const [notificationId, notificationToastId] of notifications) {
      if (notificationToastId === toastIdToClear) notifications.delete(notificationId)
    }
    if (notifications.size === 0) pluginNotificationToastIds.delete(pluginId)
  }
}

// Системное уведомление о сбое — только когда окно лаунчера скрыто/свёрнуто.
const launcherHidden = ref(false)

function syncLauncherHidden() {
  launcherHidden.value = document.hidden || !document.hasFocus()
}

function notifySystemIfHidden(message: string) {
  if (!launcherHidden.value) return
  void bridge.systemNotify('Nivora Launcher', message).catch(() => undefined)
}

// --- Игровой оверлей (Shift+Tab) и запись (F7) -------------------------------
// Главное окно следит за запущенной игрой: пока она работает, держит
// глобальные клавиши и окно оверлея, после выхода — прибирает за собой.
const overlayMode = overlayWindowRequested()
let overlayWatchTimer: number | undefined
let overlayAttachedInstance = ''
let overlayRecordingBusy = false
// Сколько подряд «игра не running» терпим, прежде чем снимать горячие
// клавиши (тик вотчера — 3 секунды).
const OVERLAY_DETACH_AFTER_MISSES = 4
let overlayMissedPolls = 0

function overlaySettingsDraft(): OverlaySettings {
  return settingsDraft.overlay_settings
}

function overlayHotkeyLabel(): string {
  return normalizeHotkey(settingsDraft.overlay_settings.hotkey_overlay, 'Shift+Tab')
}

const inAppOverlayOpen = ref(false)

async function handleOverlayToggleHotkey() {
  // Wayland-костыль: отдельное окно поверх игры там не поднять (compositor
  // не даёт чужим окнам topmost), поэтому тот же оверлей открывается
  // внутри окна лаунчера — данные те же, работает всегда.
  if (overlayWaylandMode()) {
    inAppOverlayOpen.value = !inAppOverlayOpen.value
    return
  }
  const visible = await toggleOverlay()
  if (!visible) {
    syncLauncherHidden()
    // Окно не открылось даже на X11 — не оставляем пользователя без
    // оверлея: показываем его внутри лаунчера.
    inAppOverlayOpen.value = true
    if (overlayLastError()) {
      const message = `Отдельное окно оверлея не открылось (${overlayLastError()}) — оверлей показан поверх лаунчера. Подробный разбор — кнопка «Диагностика оверлея» в Настройках.`
      showToast(message, 'danger')
      notifySystemIfHidden(message)
    }
  }
}

// Пошаговая проверка оверлея по кнопке в настройках: показывает, на каком
// именно шаге всё ломается (среда, создание окна, показ, always-on-top,
// глобальные клавиши), вместо «не работает» без объяснений.
const overlaySelfTestReport = ref<OverlaySelfTestStep[]>([])
const overlaySelfTestBusy = ref(false)

async function runOverlaySelfTest() {
  if (overlaySelfTestBusy.value) return
  overlaySelfTestBusy.value = true
  try {
    overlaySelfTestReport.value = await overlaySelfTest(overlaySettingsDraft())
    const failed = overlaySelfTestReport.value.filter((step) => !step.ok).length
    showToast(failed === 0
      ? 'Диагностика оверлея: все шаги в порядке. Если окно не видно поверх игры — игре нужен оконный режим (F11).'
      : `Диагностика оверлея: проблемных шагов — ${failed}. Отчёт ниже в настройках.`, failed === 0 ? 'success' : 'danger')
  } catch (error) {
    showToast(`Диагностика оверлея прервалась: ${errorMessage(error)}`, 'danger')
  } finally {
    overlaySelfTestBusy.value = false
  }
}

async function handleRecordingHotkey() {
  if (overlayRecordingBusy) return
  overlayRecordingBusy = true
  try {
    const message = await toggleRecording(overlayAttachedInstance)
    showToast(message)
    notifySystemIfHidden(message)
  } catch (error) {
    showToast(errorMessage(error), 'danger')
  } finally {
    overlayRecordingBusy = false
  }
}

async function attachOverlay(instanceName: string, options: { silent?: boolean } = {}) {
  overlayAttachedInstance = instanceName
  const failed = await bindGameHotkeys(overlaySettingsDraft(), () => { void handleOverlayToggleHotkey() }, () => { void handleRecordingHotkey() })
  if (failed.length && !options.silent) {
    // Причина «не зарегистрировалась» бывает разной: на нативном Wayland
    // композитор не отдаёт клавиши чужим клиентам вовсе, а на X11/XWayland
    // это скорее всего занятая комбинация. overlayWaylandMode() знает
    // реальный бэкенд (учитывает форсированный GDK_BACKEND=x11).
    const wayland = overlayWaylandMode()
    showToast(wayland
      ? `Глобальные клавиши (${failed.join(', ')}) не работают в нативной Wayland-сессии. Пока окно лаунчера в фокусе, комбинация работает как обычно; иначе открывайте оверлей кнопкой «Оверлей» в шапке или пунктом в меню значка в трее.`
      : `Комбинация не зарегистрировалась (занята другой программой?): ${failed.join(', ')}. Оверлей также открывается кнопкой «Оверлей» в шапке и из меню значка в трее.`, 'danger')
  }
}

// Зовём в момент старта игры: горячие клавиши должны жить уже к моменту,
// когда игрок увидел окно Minecraft. Watcher ниже — страховка для случаев,
// когда игра была запущена не через этот экран.
let overlayHintShown = false
let waylandOverlayHintShown = false

function attachOverlayForLaunch(instanceName: string) {
  if (overlayMode || !inDesktopTauri()) return
  if (!settingsDraft.overlay_settings.enabled) return
  void attachOverlay(instanceName)
  // Эксклюзивный полноэкранный режим прячет любые topmost-окна — лучше
  // предупредить один раз, чем пользователь решит, что оверлей сломан.
  if (!overlayHintShown) {
    overlayHintShown = true
    showToast(`Оверлей готов: ${normalizeHotkey(settingsDraft.overlay_settings.hotkey_overlay, 'Shift+Tab')}. Если игра в полноэкранном режиме — переключите её в оконный (F11 в Minecraft)`)
  }
}

async function detachOverlay() {
  overlayMissedPolls = 0
  if (!overlayAttachedInstance) return
  overlayAttachedInstance = ''
  try {
    const status = await bridge.recordingStatus()
    if (status.active) await bridge.recordingStop()
  } catch {
    // запись могла уже завершиться сама
  }
  // Горячие клавиши остаются назначенными, пока работает лаунчер:
  // снимать их вместе с сессией — значит оставить кнопку «оверлей»,
  // которая внезапно ничего не делает. Снимаем только при выключении
  // настройки оверлея и при выходе из приложения.
  await setOverlayVisible(false)
}

async function pollOverlaySession() {
  if (!settingsDraft.overlay_settings.enabled) {
    overlayMissedPolls = 0
    await detachOverlay()
    await unbindGameHotkeys()
    return
  }
  try {
    const snapshot = await bridge.overlayState()
    if (snapshot.running && snapshot.instance) {
      overlayMissedPolls = 0
      if (!overlayAttachedInstance) await attachOverlay(snapshot.instance.name)
    } else {
      // Один «не работает» — ещё не повод отцеплять горячие клавиши:
      // backend занят долгой операцией, live-state ещё не дописан, игра
      // перезапускается. Отцепляем только после устойчивой серии промахов.
      overlayMissedPolls += 1
      if (overlayMissedPolls >= OVERLAY_DETACH_AFTER_MISSES) await detachOverlay()
    }
  } catch {
    // бэкенд занят долгой операцией — попробуем в следующий тик
    // (состояние горячих клавиш при этом не трогаем)
  }
}

function startOverlayWatcher() {
  if (overlayWatchTimer !== undefined) return
  void initOverlayEventBridge(() => { void handleOverlayToggleHotkey() }, () => { inAppOverlayOpen.value = false })
  // Привязываем горячие клавиши сразу, а не «когда начнётся игра»:
  // назначенная пользователем комбинация обязана работать всегда,
  // иначе выглядит, что оверлей «никак не открывается».
  if (settingsDraft.overlay_settings.enabled) void attachOverlay(overlayAttachedInstance, { silent: true })
  overlayWatchTimer = window.setInterval(() => { void pollOverlaySession() }, 3000)
}

function stopOverlayWatcher() {
  if (overlayWatchTimer !== undefined) {
    window.clearInterval(overlayWatchTimer)
    overlayWatchTimer = undefined
  }
  void unbindGameHotkeys()
  void setOverlayVisible(false)
}

watch(() => settingsDraft.overlay_settings, () => {
  if (overlayMode) return
  // Настройки оверлея поменялись — сразу перерегистрируем клавиши,
  // даже если игра сейчас не запущена (они живут вместе с лаунчером).
  if (settingsDraft.overlay_settings.enabled) {
    void attachOverlay(overlayAttachedInstance, { silent: true })
  } else {
    void detachOverlay()
    void unbindGameHotkeys()
  }
}, { deep: true })

function showToast(message: string, tone: PluginUiTone = 'success') {
  if (tone === 'danger') notifySystemIfHidden(message)
  if (!settingsDraft.show_toasts) return
  const previousToastId = toastId.value
  toastId.value += 1
  clearPluginNotificationToast(previousToastId)
  toast.value = message
  toastToneClass.value = {
    default: '',
    muted: 'status-dot--muted',
    success: 'status-dot--green',
    warning: 'status-dot--warning',
    danger: 'status-dot--danger',
  }[tone]
  const currentToastId = toastId.value
  if (toastTimer) window.clearTimeout(toastTimer)
  toastTimer = window.setTimeout(() => {
    if (toastId.value !== currentToastId) return
    toast.value = ''
    clearPluginNotificationToast(currentToastId)
    toastTimer = undefined
  }, toastDurationMs)
}

async function confirmDestructive(message: string, options: Parameters<typeof confirmDialog>[1] = {}): Promise<boolean> {
  if (!settingsDraft.confirm_destructive_actions) return true
  return confirmDialog(message, { danger: true, confirmLabel: 'Удалить', ...options })
}

function formatMemoryLabel(kib: number): string {
  if (!kib) return 'определяется'
  if (kib % (1024 * 1024) === 0) return `${kib / (1024 * 1024)} GiB`
  if (kib % 1024 === 0) return `${kib / 1024} MiB`
  return `${kib} KiB`
}

function commitWallpaperBlur() {
  const value = Number(appearanceDraft.wallpaper_blur)
  wallpaperRenderBlur.value = Math.min(32, Math.max(0, Number.isFinite(value) ? Math.round(value) : 0))
  appearanceDraft.wallpaper_blur = wallpaperRenderBlur.value
}

// Blob-обои печёт отдельный сервис: нумерация запросов, освобождение старых
// blob-URL и кодирование вне главного потока — см. services/wallpaper.ts.
const wallpaperRenderer = createWallpaperRenderer()

async function renderWallpaper() {
  const next = await wallpaperRenderer.render(wallpaperUrl.value, wallpaperRenderBlur.value)
  // null — параллельный запрос уже отвечает новее, не мешаем ему
  if (next !== null) wallpaperProcessedUrl.value = next
}

function scheduleWallpaperRender() {
  if (wallpaperResizeTimer) window.clearTimeout(wallpaperResizeTimer)
  wallpaperResizeTimer = window.setTimeout(() => {
    wallpaperResizeTimer = undefined
    void renderWallpaper()
  }, 120)
}

function onModIconError(mod: ModResult) {
  mod.icon_url = ''
}

function chooseModpackFile() {
  modpackFileInput.value?.click()
}

// --- Предпросмотр модов при импорте модпака --------------------------------
// До импорта показываем, какие моды окажутся в экземпляре (из архива,
// индекса Modrinth или манифеста CurseForge). Раньше импорт «делал вид»:
// прогресс был, а модов в папке не появлялось.
const modpackPreviewOpen = ref(false)
const modpackPreviewTitle = ref('')
const modpackPreviewItems = ref<ModpackModPreviewItem[]>([])
const modpackPreviewTotal = ref(0)
const modpackPreviewTruncated = ref(false)
const modpackPreviewQuery = ref('')
let modpackPreviewResolver: ((confirmed: boolean) => void) | null = null

const modpackPreviewFiltered = computed(() => {
  const query = modpackPreviewQuery.value.trim().toLowerCase()
  if (!query) return modpackPreviewItems.value
  return modpackPreviewItems.value.filter((item) => item.name.toLowerCase().includes(query))
})

const modpackPreviewSourceLabel = (source: string) => ({
  archive: 'из архива',
  modrinth: 'Modrinth',
  curseforge: 'CurseForge',
  ftb: 'FTB',
} as Record<string, string>)[source] || source

function closeModpackPreview(confirmed: boolean) {
  if (!modpackPreviewOpen.value) return
  modpackPreviewOpen.value = false
  const resolver = modpackPreviewResolver
  modpackPreviewResolver = null
  modpackPreviewQuery.value = ''
  resolver?.(confirmed)
}

async function showModpackPreviewDialog(title: string, preview?: ModpackModPreview): Promise<boolean> {
  const items = preview?.mods || []
  // Пустой список — не блокируем пользователя диалогом без содержания
  if (!items.length && !(preview?.total || 0)) return true
  modpackPreviewTitle.value = title
  modpackPreviewItems.value = items
  modpackPreviewTotal.value = preview?.total || items.length
  modpackPreviewTruncated.value = Boolean(preview?.truncated)
  modpackPreviewOpen.value = true
  return new Promise((resolve) => {
    modpackPreviewResolver = resolve
  })
}

function inDesktopTauri(): boolean {
  return typeof window !== 'undefined' && Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
}

async function checkLauncherUpdates(showToastOnNoUpdate = true) {
  if (!inDesktopTauri()) {
    if (showToastOnNoUpdate) showToast('Проверка обновлений доступна только в desktop launcher')
    return false
  }
  updaterBusy.value = true
  updaterProgressDownloaded.value = 0
  updaterProgressTotal.value = 0
  try {
    const update = await checkForAppUpdate()
    if (!update) {
      pendingAppUpdate = null
      updaterVersion.value = ''
      updaterNotes.value = ''
      updaterStatus.value = 'Установлена последняя доступная версия.'
      if (showToastOnNoUpdate) showToast('Обновлений нет')
      return false
    }
    pendingAppUpdate = update
    updaterVersion.value = String(update.version || '')
    updaterNotes.value = String(update.body || '')
    updaterStatus.value = `Доступна версия ${updaterVersion.value || 'новее текущей'}.`
    showToast(`Найдена версия ${updaterVersion.value || 'новее текущей'}`)
    return true
  } catch (error) {
    // Сборки без настроенного сервера обновлений не должны пугать сырой
    // сетевой ошибкой — говорим прямо, куда смотреть.
    const raw = errorMessage(error)
    if (/example\.invalid|dns|resolve|failed to fetch|endpoint/i.test(raw)) {
      updaterStatus.value = 'Самообновление в этой сборке не настроено — свежие версии выходят на странице проекта.'
      if (showToastOnNoUpdate) showToast('Самообновление недоступно в этой сборке')
    } else {
      updaterStatus.value = raw
      if (showToastOnNoUpdate) showToast(raw)
    }
    return false
  } finally {
    updaterBusy.value = false
  }
}

async function installLauncherUpdate() {
  if (!pendingAppUpdate) {
    const found = await checkLauncherUpdates(false)
    if (!found || !pendingAppUpdate) return
  }
  const version = updaterVersion.value || 'новая версия'
  if (!(await confirmDialog(`Установить обновление ${version}?`, {
    title: 'Самообновление Nivora Launcher',
    detail: updaterNotes.value || 'После установки приложение перезапустится автоматически.',
    confirmLabel: 'Установить',
  }))) return
  updaterBusy.value = true
  updaterStatus.value = `Загружаю ${version}…`
  updaterProgressDownloaded.value = 0
  updaterProgressTotal.value = 0
  try {
    await pendingAppUpdate.downloadAndInstall((event: any) => {
      const phase = String(event?.event || '')
      if (phase === 'Started') {
        updaterProgressTotal.value = Number(event?.data?.contentLength || 0)
        updaterProgressDownloaded.value = 0
        updaterStatus.value = `Загрузка ${version} началась.`
      } else if (phase === 'Progress') {
        updaterProgressDownloaded.value += Number(event?.data?.chunkLength || 0)
        updaterStatus.value = updaterProgressTotal.value > 0
          ? `Скачано ${Math.min(updaterProgressDownloaded.value, updaterProgressTotal.value)} из ${updaterProgressTotal.value} байт.`
          : 'Скачиваю обновление…'
      } else if (phase === 'Finished') {
        updaterStatus.value = 'Пакет обновления скачан, устанавливаю…'
      }
    })
    updaterStatus.value = 'Обновление установлено. Перезапускаю лаунчер…'
    showToast('Обновление установлено, перезапуск…')
    await relaunch()
  } catch (error) {
    updaterStatus.value = errorMessage(error)
    showToast(errorMessage(error))
  } finally {
    updaterBusy.value = false
  }
}

async function withUnverifiedDownloadConfirmation<T>(
  attempt: (confirmUnverified: boolean) => Promise<T>,
  prompt: string,
  cancelMessage: string,
): Promise<{ value: T; confirmed: boolean }> {
  try {
    return { value: await attempt(false), confirmed: false }
  } catch (error) {
    if (!errorMessage(error).includes('[unverified-download]')) throw error
    if (!(await confirmDialog(prompt, { danger: true, confirmLabel: 'Продолжить' }))) throw new Error(cancelMessage)
    return { value: await attempt(true), confirmed: true }
  }
}

async function importModpackWithConfirmation(payload: Parameters<typeof bridge.importModpack>[0]) {
  return (await withUnverifiedDownloadConfirmation(
    (confirmUnverified) => bridge.importModpack({ ...payload, confirm_unverified: confirmUnverified }),
    'В архиве есть внешние файлы без издательского хеша. Размер не подтверждает подлинность. Загрузить их всё равно?',
    'Импорт отменён; файлы экземпляра не изменены.',
  )).value
}

async function preflightArchiveModpack(payload: Parameters<typeof bridge.importModpack>[0]) {
  return withUnverifiedDownloadConfirmation(
    (confirmUnverified) => bridge.importModpack({ ...payload, preflight_only: true, confirm_unverified: confirmUnverified }),
    'В архиве есть внешние файлы без издательского хеша. Размер не подтверждает подлинность. Загрузить их всё равно?',
    'Импорт отменён; файлы экземпляра не изменены.',
  )
}

async function preflightCatalogModpack(payload: Parameters<typeof bridge.modsInstall>[0]) {
  return withUnverifiedDownloadConfirmation(
    (confirmUnverified) => bridge.modsInstall({ ...payload, preflight_only: true, confirm_unverified: confirmUnverified }),
    'Каталог или файлы модпака не предоставили издательский хеш. Размер не подтверждает подлинность. Продолжить загрузку?',
    'Создание отменено; экземпляр не изменён.',
  )
}

async function importModpackFile(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  if (file.size > 512 * 1024 * 1024) {
    showToast('Архив модпака превышает лимит 512 MiB')
    input.value = ''
    return
  }
  if (currentView.value === 'modpacks') {
    try {
      await createModpackInstance(undefined, { fileName: file.name, data: await encodeBlob(file) })
    } finally {
      input.value = ''
    }
    return
  }
  const instance = selectedInstance.value
  if (!instance) return
  const archiveData = await encodeBlob(file)
  modBusy.value = true
  modInstallDetail.value = 'Читаю архив и проверяю файлы…'
  openActivity('Импорт модпака', `${file.name} · ${instance.name}`, false)
  try {
    const preflight = await preflightArchiveModpack({
      instance_name: instance.name,
      file_name: file.name,
      data: archiveData,
      game_version: modInstanceVersion.value,
      loader: instance.loader,
      loader_version: instance.loader_version,
    })
    // Предосмотр модов: подтверждаем содержимое до изменения экземпляра
    if (!(await showModpackPreviewDialog(file.name, modpackPreviewData(preflight.value.dependencies)))) {
      throw new Error('Импорт отменён при просмотре списка модов.')
    }
    const dependencyIssues = modpackDependencyIssues(preflight.value.dependencies)
    if (dependencyIssues.length) {
      const choice = await chooseDialog(
        'Требования модпака не совпадают с текущим экземпляром',
        [{ value: 'separate', label: 'Отдельный экземпляр', description: modpackDependencySummary(preflight.value.dependencies) || 'Создать новый профиль под этот модпак' }],
        {
          title: 'Конфликт модпака',
          detail: dependencyIssues.join(' · '),
          confirmLabel: 'Выбрать',
        },
      )
      if (choice === 'separate') {
        await createModpackInstance(undefined, { fileName: file.name, data: archiveData })
        finishActivity('Импорт перенаправлен в новый экземпляр.')
        return
      }
      throw new Error('Импорт отменён: требования модпака не совпадают с текущим экземпляром.')
    }
    const result = await bridge.importModpack({
      instance_name: instance.name,
      file_name: file.name,
      data: archiveData,
      game_version: modInstanceVersion.value,
      loader: instance.loader,
      loader_version: instance.loader_version,
      confirm_unverified: preflight.confirmed,
    })
    finishActivity('Модпак импортирован.', [`Файлов: ${result.files?.length || 0}`, `Экземпляр: ${instance.name}`])
    showToast('Модпак импортирован в экземпляр')
    await searchMods()
  } catch (error) {
    activity.logs.push(errorMessage(error))
    finishActivity(isCancelled(error) ? 'Импорт остановлен.' : 'Импорт модпака остановлен.')
    if (!isCancelled(error)) showToast(errorMessage(error))
  } finally {
    modBusy.value = false
    modInstallDetail.value = ''
    input.value = ''
  }
}

async function createModpackInstance(mod?: ModResult, archive?: { fileName: string; data: string }) {
  const title = mod?.title || mod?.name || 'Modpack'
  const defaultName = title.replace(/[^\p{L}\p{N}_-]+/gu, '-').replace(/^-+|-+$/g, '').slice(0, 48) || 'Modpack'
  const name = (await promptDialog('Имя нового экземпляра модпака', { initialValue: defaultName, confirmLabel: 'Создать экземпляр' }))?.trim()
  if (!name) return
  if (state.instances.some((value) => value.name === name)) {
    showToast('Это имя уже занято. Выберите новое имя, чтобы сохранить текущий экземпляр.')
    return
  }
  const version = selectedVersion.value
  if (!version) {
    showToast('Не удалось определить версию Minecraft для модпака')
    return
  }
  if (mod?.game_versions?.length && !mod.game_versions.includes(version)) {
    showToast(`Модпак не поддерживает выбранную версию Minecraft ${version}`)
    return
  }
  if (selectedLoader.value !== 'vanilla' && loadingChoices.value) {
    showToast('Дождитесь загрузки версий загрузчика')
    return
  }
  busy.value = true
  openActivity('Создание экземпляра модпака', `${title} · Minecraft ${version}`)
  let created: InstanceInfo | undefined
  let createdInstanceName = ''
  try {
    let confirmUnverified = false
    let manifestRequirements = ''
    let resolvedVersion = version
    let resolvedLoader = selectedLoader.value
    let resolvedLoaderVersion = selectedLoaderVersion.value
    if (archive) {
      const preflight = await preflightArchiveModpack({
        instance_name: name,
        file_name: archive.fileName,
        data: archive.data,
        game_version: version,
        loader: selectedLoader.value,
        loader_version: selectedLoaderVersion.value,
      })
      confirmUnverified = preflight.confirmed
      manifestRequirements = modpackDependencySummary(preflight.value.dependencies)
      if (!(await showModpackPreviewDialog(archive.fileName, modpackPreviewData(preflight.value.dependencies)))) {
        throw new Error('Создание отменено при просмотре списка модов.')
      }
      const resolved = await confirmModpackRequirements(title, preflight.value.dependencies, version, selectedLoader.value, selectedLoaderVersion.value)
      if (!resolved) throw new Error('Создание отменено: требования модпака не применены.')
      resolvedVersion = resolved.version
      resolvedLoader = resolved.loader
      resolvedLoaderVersion = resolved.loaderVersion
    } else if (mod) {
      const preflight = await preflightCatalogModpack({
        project_id: mod.project_id || mod.id || '',
        instance_name: name,
        project_type: 'modpack',
        catalog_id: activeCatalogId.value,
        version: mod.version,
        display_name: title,
        game_version: version,
        loader: selectedLoader.value,
        loader_version: selectedLoaderVersion.value,
      })
      confirmUnverified = preflight.confirmed
      manifestRequirements = modpackDependencySummary(preflight.value.dependencies)
      if (!(await showModpackPreviewDialog(title, modpackPreviewData(preflight.value.dependencies)))) {
        throw new Error('Создание отменено при просмотре списка модов.')
      }
      const resolved = await confirmModpackRequirements(title, preflight.value.dependencies, version, selectedLoader.value, selectedLoaderVersion.value)
      if (!resolved) throw new Error('Создание отменено: требования модпака не применены.')
      resolvedVersion = resolved.version
      resolvedLoader = resolved.loader
      resolvedLoaderVersion = resolved.loaderVersion
    }
    const installed = await bridge.install({
      version: resolvedVersion,
      loader: resolvedLoader,
      loader_version: resolvedLoaderVersion || undefined,
      instance_name: name,
      fail_if_exists: true,
      memory: memorySetting.value,
      java_path: effectiveJavaPath(),
    })
    state.instances = installed.instances
    createdInstanceName = installed.instances.find((value) => value.path === installed.instance_dir)?.name
      || installed.instance_dir.split(/[\\/]/).pop()
      || name
    created = state.instances.find((value) => value.name === createdInstanceName)
    selectedInstanceName.value = createdInstanceName
    if (archive) {
      await bridge.importModpack({
        instance_name: createdInstanceName,
        file_name: archive.fileName,
        data: archive.data,
        game_version: resolvedVersion,
        loader: resolvedLoader,
        loader_version: resolvedLoaderVersion,
        confirm_unverified: confirmUnverified,
      })
    } else if (mod) {
      await bridge.modsInstall({
        project_id: mod.project_id || mod.id || '',
        instance_name: createdInstanceName,
        project_type: 'modpack',
        catalog_id: activeCatalogId.value,
        version: mod.version,
        display_name: title,
        game_version: resolvedVersion,
        loader: resolvedLoader,
        loader_version: resolvedLoaderVersion,
        confirm_unverified: confirmUnverified,
      })
    }
    finishActivity('Экземпляр модпака готов.', [
      `Профиль: ${name}`,
      `Minecraft: ${resolvedVersion}`,
      `Loader: ${resolvedLoader}${resolvedLoaderVersion ? ` ${resolvedLoaderVersion}` : ''}`,
      ...(manifestRequirements ? [`Требования манифеста: ${manifestRequirements}`] : []),
    ])
    showToast('Модпак установлен в отдельный экземпляр')
    setView('instances')
  } catch (error) {
    if (createdInstanceName) {
      try {
        state.instances = (await bridge.deleteInstance(createdInstanceName)).instances
        if (selectedInstanceName.value === createdInstanceName) selectedInstanceName.value = ''
      } catch (cleanupError) {
        activity.logs.push(`Не удалось удалить незавершённый экземпляр: ${errorMessage(cleanupError)}`)
      }
      created = undefined
    }
    activity.logs.push(errorMessage(error))
    finishActivity(isCancelled(error) ? 'Создание остановлено.' : 'Не удалось создать экземпляр модпака.')
    if (!isCancelled(error)) showToast(errorMessage(error))
  } finally {
    busy.value = false
  }
  if (created && settingsDraft.launch_after_install) await launchExisting(created)
}

function modpackDependencySummary(
  dependencies?: ModpackDependencyRequirements | Array<Record<string, unknown>>,
): string {
  if (!dependencies || Array.isArray(dependencies)) return ''
  return [
    dependencies?.minecraft ? `Minecraft ${dependencies.minecraft}` : '',
    dependencies?.loader ? `${dependencies.loader}${dependencies.loader_version ? ` ${dependencies.loader_version}` : ''}` : '',
  ].filter(Boolean).join(' · ')
}

function modpackDependencyIssues(
  dependencies?: ModpackDependencyRequirements | Array<Record<string, unknown>>,
): string[] {
  if (!dependencies || Array.isArray(dependencies) || !Array.isArray(dependencies.issues)) return []
  return dependencies.issues.filter((value) => typeof value === 'string' && value.trim())
}

function modpackPreviewData(dependencies?: ModpackDependencyRequirements | Array<Record<string, unknown>>): ModpackModPreview | undefined {
  if (!dependencies || Array.isArray(dependencies)) return undefined
  const mods = dependencies.mods
  return mods && typeof mods === 'object' && Array.isArray(mods.mods) ? mods : undefined
}

async function confirmModpackRequirements(
  title: string,
  dependencies: ModpackDependencyRequirements | Array<Record<string, unknown>> | undefined,
  fallbackVersion: string,
  fallbackLoader: LoaderId,
  fallbackLoaderVersion: string,
): Promise<{ version: string; loader: LoaderId; loaderVersion: string } | null> {
  if (!dependencies || Array.isArray(dependencies)) {
    return { version: fallbackVersion, loader: fallbackLoader, loaderVersion: fallbackLoaderVersion }
  }
  const version = dependencies.minecraft || fallbackVersion
  const loader = (dependencies.loader || fallbackLoader) as LoaderId
  const loaderVersion = dependencies.loader_version || fallbackLoaderVersion
  const issues = modpackDependencyIssues(dependencies)
  const changed = version !== fallbackVersion || loader !== fallbackLoader || loaderVersion !== fallbackLoaderVersion
  if (!issues.length && !changed) return { version, loader, loaderVersion }
  const choice = await chooseDialog(
    'Применить требования модпака?',
    [{ value: 'apply', label: 'Применить требования', description: modpackDependencySummary(dependencies) || 'Автоматически выбрать совместимый контекст' }],
    {
      title: `Требования ${title}`,
      detail: issues.length ? issues.join(' · ') : 'Модпак просит другой Minecraft или загрузчик. Nivora Launcher может подстроить новый экземпляр автоматически.',
      confirmLabel: 'Применить',
    },
  )
  return choice === 'apply' ? { version, loader, loaderVersion } : null
}

function clampMemoryDraft() {
  const max = memoryMaxValue.value
  memoryValue.value = Math.max(1, Math.floor(Number(memoryValue.value) || 1))
  if (max) memoryValue.value = Math.min(memoryValue.value, max)
  settingsDraft.memory = memorySetting.value
}

function clampServerMemoryDraft() {
  const minimum = Math.min(serverMemoryMinimum.value, serverMemoryMaxValue.value)
  const value = Number(serverMemoryValue.value)
  serverMemoryValue.value = Math.min(serverMemoryMaxValue.value, Math.max(minimum, Number.isFinite(value) && value > 0 ? value : minimum))
  serverCreation.memory = serverMemorySetting.value
}

function setServerMemoryUnit(value: string) {
  if (!Object.prototype.hasOwnProperty.call(memoryMultipliers, value)) return
  const next = value as MemoryUnit
  const previous = serverMemoryUnit.value
  if (next === previous) return
  serverMemoryValue.value = Number(serverMemoryValue.value || serverMemoryMinimum.value) * memoryMultipliers[previous] / memoryMultipliers[next]
  serverMemoryUnit.value = next
  clampServerMemoryDraft()
}

function syncMemoryDraft(raw: string) {
  const value = raw.trim()
  const match = value.match(/^(\d+(?:\.\d+)?)\s*(KiB|MiB|GiB|K|M|G|KB|MB|GB|B)?$/i)
  if (!match || value.toLowerCase() === 'auto') {
    const recommended = state.settings.memory_recommended_kib || state.settings.memory_kib || 1024 * 1024
    if (recommended % (1024 * 1024) === 0) {
      memoryUnit.value = 'GiB'
      memoryValue.value = recommended / (1024 * 1024)
    } else if (recommended % 1024 === 0) {
      memoryUnit.value = 'MiB'
      memoryValue.value = recommended / 1024
    } else {
      memoryUnit.value = 'KiB'
      memoryValue.value = recommended
    }
    clampMemoryDraft()
    return
  }
  const unit = (match[2] || 'MiB').toLowerCase()
  const normalizedUnit: MemoryUnit = unit.startsWith('g') ? 'GiB' : unit.startsWith('k') || unit === 'b' ? 'KiB' : 'MiB'
  memoryUnit.value = normalizedUnit
  memoryValue.value = Number(match[1])
  clampMemoryDraft()
}

function effectiveJavaPath(): string {
  if (javaSelection.value === 'auto') return ''
  if (javaSelection.value === 'manual') return settingsDraft.java_path.trim()
  return javaSelection.value
}

function replacePluginList(next: PluginManifest[]) {
  plugins.value = next.map((plugin) => ({
    ...plugin,
    permissions: [...(plugin.permissions || [])],
    requested_permissions: [...(plugin.requested_permissions || [])],
    granted_permissions: [...(plugin.granted_permissions || [])],
  }))
}

function publishPluginView(pluginId: string, pluginName: string, view: PluginUiView) {
  const key = `${pluginId}:${view.id}`
  pluginViews.value = [
    ...pluginViews.value.filter((entry) => `${entry.pluginId}:${entry.view.id}` !== key),
    { pluginId, pluginName, view },
  ]
}

function registerPluginCommand(pluginId: string, pluginName: string, definition: PluginCommandDefinition) {
  pluginCommands.value = [
    ...pluginCommands.value.filter((entry) => entry.pluginId !== pluginId || entry.definition.id !== definition.id),
    { pluginId, pluginName, definition },
  ]
}

function unregisterPluginCommand(pluginId: string, id: string) {
  pluginCommands.value = pluginCommands.value.filter((entry) => entry.pluginId !== pluginId || entry.definition.id !== id)
}

function registerPluginShortcut(pluginId: string, pluginName: string, definition: PluginShortcutDefinition) {
  if (!isSafePluginShortcutBinding(definition.default_key)) {
    showToast(`${pluginName}: используйте Ctrl, Alt или Meta в сочетании клавиш`, 'warning')
    return
  }
  pluginShortcuts.value = [
    ...pluginShortcuts.value.filter((entry) => entry.pluginId !== pluginId || entry.definition.id !== definition.id),
    { pluginId, pluginName, definition },
  ]
}

function unregisterPluginShortcut(pluginId: string, id: string) {
  pluginShortcuts.value = pluginShortcuts.value.filter((entry) => entry.pluginId !== pluginId || entry.definition.id !== id)
}

function registerPluginTheme(pluginId: string, registrationId: string, definition: PluginThemeDefinition) {
  const contributions = pluginThemeContributions.get(pluginId) || new Map<string, PluginThemeDefinition>()
  contributions.set(registrationId, definition)
  pluginThemeContributions.set(pluginId, contributions)
}

function unregisterPluginTheme(pluginId: string, registrationId: string) {
  const contributions = pluginThemeContributions.get(pluginId)
  contributions?.delete(registrationId)
  if (contributions?.size === 0) pluginThemeContributions.delete(pluginId)
}

function removePluginView(pluginId: string, viewId: string): boolean {
  const previousLength = pluginViews.value.length
  pluginViews.value = pluginViews.value.filter((entry) => entry.pluginId !== pluginId || entry.view.id !== viewId)
  return pluginViews.value.length !== previousLength
}

function clearPluginUiState(pluginId: string) {
  pluginViews.value = pluginViews.value.filter((entry) => entry.pluginId !== pluginId)
  pluginCommands.value = pluginCommands.value.filter((entry) => entry.pluginId !== pluginId)
  pluginShortcuts.value = pluginShortcuts.value.filter((entry) => entry.pluginId !== pluginId)
  pluginThemeContributions.delete(pluginId)
}

function clearPluginRuntimeState(pluginId: string) {
  clearPluginUiState(pluginId)
  pluginEventSubscriptions.delete(pluginId)
  pluginLogEventInFlight.delete(pluginId)
  pluginLogEventLastSentAt.delete(pluginId)
  pluginNotificationToastIds.delete(pluginId)
  runningPluginDigests.delete(pluginId)
}

function pluginInstanceRef(instance = selectedInstance.value): PluginInstanceRef | null {
  if (!instance) return null
  return {
    id: instance.name,
    name: instance.display_name || instance.name,
    version: instance.minecraft_version || instance.version_id || undefined,
    loader: instance.loader,
  }
}

function subscribePluginEvent(pluginId: string, listenerId: string, event: PluginEventName) {
  const subscriptions = pluginEventSubscriptions.get(pluginId) || new Map<string, PluginEventName>()
  subscriptions.set(listenerId, event)
  pluginEventSubscriptions.set(pluginId, subscriptions)
}

function unsubscribePluginEvent(pluginId: string, listenerId: string) {
  const subscriptions = pluginEventSubscriptions.get(pluginId)
  subscriptions?.delete(listenerId)
  if (subscriptions && subscriptions.size === 0) pluginEventSubscriptions.delete(pluginId)
}

async function emitPluginEvent<K extends PluginEventName>(pluginId: string, event: K, payload: PluginEventPayloadMap[K]) {
  const subscriptions = pluginEventSubscriptions.get(pluginId)
  if (!subscriptions || ![...subscriptions.values()].includes(event)) return
  if (event === 'log.line') {
    const now = performance.now()
    if (pluginLogEventInFlight.has(pluginId) || now - (pluginLogEventLastSentAt.get(pluginId) || 0) < 100) return
    pluginLogEventInFlight.add(pluginId)
    pluginLogEventLastSentAt.set(pluginId, now)
  }
  try {
    try {
      await bridge.pluginApiAuthorize(pluginId, 'events:launcher')
    } catch {
      await bridge.pluginApiAuthorize(pluginId, 'lifecycle:observe')
    }
    await pluginRuntime.emit(pluginId, event, payload)
  } catch {
    // Grants can be revoked between subscription and event delivery.
  } finally {
    if (event === 'log.line') pluginLogEventInFlight.delete(pluginId)
  }
}

async function broadcastPluginEvent<K extends PluginEventName>(event: K, payload: PluginEventPayloadMap[K]) {
  await Promise.all([...pluginEventSubscriptions.keys()].map((pluginId) => emitPluginEvent(pluginId, event, payload)))
}

function notifyPluginBeforeLaunch(instance: PluginInstanceRef): Promise<void> {
  return broadcastPluginEvent('instance.beforeLaunch', { instance })
}

// Хук для InstanceWorkspace: плагинам — событие, нам — момент привязать
// горячие клавиши оверлея к стартующему процессу.
async function beforeInstanceLaunch(instance: PluginInstanceRef): Promise<void> {
  attachOverlayForLaunch(String(instance.name || ''))
  await notifyPluginBeforeLaunch(instance)
}

function notifyPluginAfterExit(instance: PluginInstanceRef, exitCode: number | null): Promise<void> {
  return broadcastPluginEvent('instance.afterExit', { instance, exit_code: exitCode })
}

function notifyPluginCrash(instance: PluginInstanceRef, message?: string, exitCode?: number | null): Promise<void> {
  return broadcastPluginEvent('instance.crash', { instance, message, exit_code: exitCode })
}

function notifyPluginSnapshotCreated(instance: PluginInstanceRef, snapshot: ServerSnapshot): Promise<void> {
  return broadcastPluginEvent('snapshot.created', {
    instance,
    snapshot_id: snapshot.id,
    category: snapshot.category,
    created_at: snapshot.created_at,
  })
}

async function reconcilePluginRuntimes() {
  const eligible = plugins.value.filter((plugin) => ['2', '3'].includes(String(plugin.api_version || '')) && plugin.installed && plugin.execution_allowed)
  const eligibleIds = new Set(eligible.map((plugin) => plugin.id))
  for (const pluginId of pluginRuntime.runtimeIds()) {
    if (eligibleIds.has(pluginId)) continue
    await pluginRuntime.stop(pluginId)
    clearPluginRuntimeState(pluginId)
  }

  for (const plugin of eligible) {
    const wasRunning = pluginRuntime.runtimeIds().includes(plugin.id)
    let source: Awaited<ReturnType<typeof bridge.pluginApiSource>>
    try {
      source = await bridge.pluginApiSource(plugin.id)
    } catch (error) {
      if (wasRunning) {
        await pluginRuntime.stop(plugin.id)
        clearPluginRuntimeState(plugin.id)
      }
      showToast(`${plugin.name}: ${errorMessage(error)}`)
      continue
    }

    const current = plugins.value.find((entry) => entry.id === plugin.id)
    if (!current?.execution_allowed) continue
    const isRunning = pluginRuntime.runtimeIds().includes(plugin.id)
    if (isRunning && runningPluginDigests.get(plugin.id) === source.package_sha256) {
      if (!current.granted_permissions?.includes('ui:compose')) {
        clearPluginUiState(plugin.id)
      }
      continue
    }
    if (isRunning) {
      await pluginRuntime.stop(plugin.id)
      clearPluginRuntimeState(plugin.id)
    }

    try {
      const manifest = source.manifest as RuntimePluginManifest
      await pluginRuntime.start(plugin.id, manifest, source.source, {
        hostCall: createPluginHostCall({
          pluginId: plugin.id,
          manifest,
          selectedInstance: () => pluginInstanceRef(),
          theme: (): PluginThemeContext => {
            const currentMode = appearanceDraft.theme === 'system'
              ? (systemDark.value ? 'dark' : 'light')
              : appearanceDraft.theme
            const mode = currentMode === 'glass' ? 'glass' : currentMode === 'light' ? 'light' : 'dark'
            const tokens: Record<string, string | number> = { ...appearanceDraft.color_tokens }
            const contributions = pluginThemeContributions.get(plugin.id)
            if (contributions) {
              for (const [registrationId, contribution] of contributions) {
                if (contribution.name) tokens[`plugin.${plugin.id}.${registrationId}.name`] = contribution.name
                for (const [key, value] of Object.entries(contribution.colors || {})) {
                  tokens[`plugin.${plugin.id}.${registrationId}.color.${key}`] = value
                }
                for (const [key, value] of Object.entries(contribution.tokens || {})) {
                  tokens[`plugin.${plugin.id}.${registrationId}.token.${key}`] = value
                }
              }
            }
            return { mode, accent: appearanceDraft.accent_color, tokens }
          },
          removeView: (viewId) => removePluginView(plugin.id, viewId),
          subscribe: (event, listenerId) => subscribePluginEvent(plugin.id, listenerId, event),
          unsubscribe: (listenerId) => unsubscribePluginEvent(plugin.id, listenerId),
          registerCommand: (definition) => registerPluginCommand(plugin.id, plugin.name, definition),
          unregisterCommand: (id) => unregisterPluginCommand(plugin.id, id),
          registerShortcut: (definition) => registerPluginShortcut(plugin.id, plugin.name, definition),
          unregisterShortcut: (id) => unregisterPluginShortcut(plugin.id, id),
          registerTheme: (id, definition) => registerPluginTheme(plugin.id, id, definition),
          unregisterTheme: (id) => unregisterPluginTheme(plugin.id, id),
          notify: (title, message, tone: PluginUiTone, id) => {
            const notificationId = id || `${plugin.id}-notification-${Date.now()}`
            showToast(`${title}: ${message}`, tone)
            if (settingsDraft.show_toasts) {
              const notifications = pluginNotificationToastIds.get(plugin.id) || new Map<string, number>()
              notifications.set(notificationId, toastId.value)
              pluginNotificationToastIds.set(plugin.id, notifications)
            }
            return notificationId
          },
          dismissNotification: (id) => {
            const notifications = pluginNotificationToastIds.get(plugin.id)
            const notificationToastId = notifications?.get(id)
            if (notificationToastId === undefined) return false
            notifications?.delete(id)
            if (notifications?.size === 0) pluginNotificationToastIds.delete(plugin.id)
            if (notificationToastId === toastId.value) {
              if (toastTimer) window.clearTimeout(toastTimer)
              toastTimer = undefined
              toastId.value += 1
              toast.value = ''
            }
            return true
          },
        }),
        onView: (view) => publishPluginView(plugin.id, plugin.name, view),
        onError: (error) => {
          showToast(`${plugin.name}: ${error.message}`)
          queueMicrotask(() => {
            if (!pluginRuntime.runtimeIds().includes(plugin.id)) clearPluginRuntimeState(plugin.id)
          })
        },
      })
      runningPluginDigests.set(plugin.id, source.package_sha256)
      await emitPluginEvent(plugin.id, 'launcher.ready', { version: '1.0.0' })
      await emitPluginEvent(plugin.id, 'instance.selected', { instance: pluginInstanceRef() })
    } catch (error) {
      clearPluginRuntimeState(plugin.id)
      showToast(`${plugin.name}: ${errorMessage(error)}`)
    }
  }
}

const pluginDevStartedAt = Date.now() * 1000000
let pluginDevGeneration = ''
let pluginDevRequests: Record<string, number> = {}
let pluginDevPollBusy = false
let pluginDevPollTimer: number | undefined
async function pollPluginDevelopment() {
  if (!settingsDraft.developer_mode || pluginDevPollBusy) return
  pluginDevPollBusy = true
  try {
    const next = await bridge.pluginDevChanges()
    if (!next.enabled || next.generation === pluginDevGeneration) return
    const initial = !pluginDevGeneration
    pluginDevGeneration = next.generation
    const changed = Object.keys(next.requests).filter((id) => next.requests[id] !== pluginDevRequests[id])
    pluginDevRequests = next.requests
    if (initial && !changed.some((id) => next.requests[id] >= pluginDevStartedAt)) return
    // Serialize with installs/grant changes; termination clears listeners, views and poll timers.
    pluginRuntimeSyncQueue = pluginRuntimeSyncQueue.then(async () => {
      for (const id of changed) { await pluginRuntime.stop(id); clearPluginRuntimeState(id) }
      await reconcilePluginRuntimes()
    }).catch((error: unknown) => showToast(errorMessage(error)))
  } catch (error) { showToast(errorMessage(error)) }
  finally { pluginDevPollBusy = false }
}

function schedulePluginRuntimeSync() {
  pluginRuntimeSyncQueue = pluginRuntimeSyncQueue
    .then(reconcilePluginRuntimes)
    .catch((error: unknown) => showToast(errorMessage(error)))
}

function runPluginAction(payload: { pluginId: string; action: string; input: PluginJsonValue }) {
  void (async () => {
    try {
      await bridge.pluginApiAuthorize(payload.pluginId, 'ui:compose')
      await pluginRuntime.runAction(payload.pluginId, payload.action, payload.input)
    } catch (error) {
      showToast(errorMessage(error))
    }
  })()
}

async function runPluginCommand(payload: { pluginId: string; commandId: string }) {
  const command = pluginCommands.value.find((entry) => entry.pluginId === payload.pluginId && entry.definition.id === payload.commandId)
  if (!command) return
  try {
    try {
      await bridge.pluginApiAuthorize(payload.pluginId, 'commands:register')
    } catch {
      await bridge.pluginApiAuthorize(payload.pluginId, 'ui:compose')
    }
    const arguments_: Record<string, string> = {}
    for (const argument of command.definition.arguments || []) {
      let value: string | undefined
      if (argument.choices?.length) {
        const choices = argument.choices.map((choice) => ({ value: choice, label: choice }))
        if (!argument.required) choices.unshift({ value: '', label: 'Пропустить' })
        value = await chooseDialog(argument.description || argument.name, choices, { title: command.definition.label })
      } else {
        value = await promptDialog(argument.description || argument.name, { title: command.definition.label })
      }
      if (value === undefined) return
      if (!value && !argument.required) continue
      if (!value) {
        showToast(`Заполните аргумент «${argument.name}» команды «${command.definition.label}»`)
        return
      }
      if (argument.choices?.length && !argument.choices.includes(value)) {
        showToast(`Недопустимое значение аргумента «${argument.name}»`)
        return
      }
      arguments_[argument.name] = value
    }
    await pluginRuntime.runCommand(payload.pluginId, payload.commandId, arguments_)
  } catch (error) {
    showToast(`${command.pluginName}: ${errorMessage(error)}`)
  }
}

async function runPluginShortcut(shortcut: ActivePluginShortcut) {
  try {
    try {
      await bridge.pluginApiAuthorize(shortcut.pluginId, 'shortcuts:register')
    } catch {
      await bridge.pluginApiAuthorize(shortcut.pluginId, 'ui:compose')
    }
    await pluginRuntime.runShortcut(shortcut.pluginId, shortcut.definition.id)
  } catch (error) {
    showToast(`${shortcut.pluginName}: ${errorMessage(error)}`)
  }
}

async function savePluginConfig(payload: { pluginId: string; key: string; value: PluginJsonValue }) {
  try {
    await bridge.pluginConfigSet(payload.pluginId, payload.key, payload.value)
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function openPluginLink(payload: { pluginId: string; url: string }) {
  try {
    await bridge.pluginApiOpenExternal(payload.pluginId, payload.url)
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function togglePlugin(plugin: PluginManifest) {
  try {
    if (plugin.enabled) {
      await pluginRuntime.stop(plugin.id)
      clearPluginRuntimeState(plugin.id)
    }
    const result = await bridge.pluginSetEnabled(plugin.id, !plugin.enabled)
    replacePluginList(result.plugins)
    if (result.plugin.enabled) void broadcastPluginEvent('plugin.enabled', { plugin_id: plugin.id })
    showToast(`${plugin.name}: ${result.plugin.enabled ? 'включён' : 'выключен'}`)
  } catch (error) {
    showToast(errorMessage(error))
    schedulePluginRuntimeSync()
  }
}

async function updatePluginGrants(payload: { pluginId: string; permissions: PluginManifest['permissions'] }) {
  try {
    const result = await bridge.pluginPermissionGrantsSet(payload.pluginId, payload.permissions)
    replacePluginList(result.plugins)
    showToast('Разрешения плагина обновлены')
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function allowLocalPluginExecution(payload: { pluginId: string; allowed: boolean }) {
  if (payload.allowed) {
    const plugin = plugins.value.find((entry) => entry.id === payload.pluginId)
    if (!plugin || !(await confirmDialog(
      `Разрешить выполнение неподписанного кода плагина «${plugin.name}» на этом устройстве?`,
      { danger: true, confirmLabel: 'Разрешить выполнение' },
    ))) return
  }
  try {
    const result = await bridge.pluginLocalUntrustedExecution(payload.pluginId, payload.allowed)
    replacePluginList(result.plugins)
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function pinPluginPublisher(plugin: PluginManifest) {
  if (!plugin.publisher_key_id || !plugin.publisher_public_key) return
  const fingerprint = plugin.publisher_fingerprint || plugin.publisher_key_id
  if (!(await confirmDialog(
    `Закрепить ключ издателя ${plugin.publisher_key_id}? Этот ключ будет считаться доверенным для плагинов издателя. Отпечаток: ${fingerprint}`,
    { danger: true, confirmLabel: 'Закрепить ключ' },
  ))) return
  try {
    const result = await bridge.pluginPublisherPin(plugin.publisher_key_id, plugin.publisher_public_key)
    replacePluginList(result.plugins)
    showToast('Ключ издателя закреплён на этом устройстве')
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function unpinPluginPublisher(plugin: PluginManifest) {
  if (!plugin.publisher_key_id) return
  if (!(await confirmDialog(
    `Отозвать доверие ко всем плагинам издателя ${plugin.publisher_key_id}?`,
    { danger: true, confirmLabel: 'Отозвать доверие' },
  ))) return
  try {
    const result = await bridge.pluginPublisherUnpin(plugin.publisher_key_id)
    replacePluginList(result.plugins)
    showToast('Доверие к ключу издателя отозвано')
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function setPluginPublisherRevocationAuthority(payload: { plugin: PluginManifest; enabled: boolean }) {
  const keyId = payload.plugin.publisher_key_id
  if (!keyId) return
  if (payload.enabled && !(await confirmDialog(
    `Разрешить ключу ${keyId} подписывать списки отзыва? Такой список может блокировать указанные в нём плагины и издателей. Принимаются только корректно подписанные документы с новой последовательностью.`,
    { danger: true, confirmLabel: 'Доверять спискам отзыва' },
  ))) return
  try {
    const result = await bridge.pluginPublisherRevocationAuthoritySet(keyId, payload.enabled)
    replacePluginList(result.plugins)
    showToast(payload.enabled ? 'Ключу разрешено подписывать списки отзыва' : 'Полномочие ключа отозвано')
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function importPluginRevocations(documentJson: string) {
  try {
    const result = await bridge.pluginRevocationsImport(documentJson)
    replacePluginList(result.plugins)
    showToast(`Импортирован список отзыва №${result.imported.sequence} от ${result.imported.authority_key_id}`)
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function revokePlugin(plugin: PluginManifest) {
  if (!(await confirmDialog(`Заблокировать плагин «${plugin.name}» на этом устройстве?`, { danger: true, confirmLabel: 'Заблокировать' }))) return
  try {
    await pluginRuntime.stop(plugin.id)
    clearPluginRuntimeState(plugin.id)
    const result = await bridge.pluginLocalRevoke(plugin.id, 'Blocked locally by the user')
    replacePluginList(result.plugins)
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function unrevokePlugin(plugin: PluginManifest) {
  if (!(await confirmDialog(`Снять локальную блокировку плагина «${plugin.name}»?`, { confirmLabel: 'Разблокировать' }))) return
  try {
    const result = await bridge.pluginLocalUnrevoke(plugin.id)
    replacePluginList(result.plugins)
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function installPluginArchive(payload: { name: string; data: string }) {
  openActivity('Установка плагина', payload.name)
  try {
    const result = await bridge.pluginInstall(payload.data, false)
    replacePluginList(result.plugins)
    finishActivity(`Плагин «${result.plugin.name}» установлен и выключен. Проверьте разрешения перед включением.`)
  } catch (error) {
    finishActivity(errorMessage(error))
  }
}

async function installGithubPlugin(payload: { owner: string; repo: string; replace: boolean }) {
  if (githubPluginInstallBusy.value) return
  githubPluginInstallBusy.value = true
  try {
    openActivity('Проверка архива GitHub', `${payload.owner}/${payload.repo}`)
    const result = await bridge.githubPluginInstall(payload.owner, payload.repo, payload.replace)
    replacePluginList(result.plugins)
    finishActivity(`Архив «${result.plugin.name}» проверен и сохранён · SHA-256 ${result.metadata.sha256}. Код не запускается.`)
  } catch (error) {
    finishActivity(errorMessage(error))
  } finally {
    githubPluginInstallBusy.value = false
  }
}

async function uninstallPlugin(plugin: PluginManifest) {
  if (!(await confirmDestructive(`Удалить плагин «${plugin.name}» и его данные?`))) return
  try {
    await pluginRuntime.stop(plugin.id)
    clearPluginRuntimeState(plugin.id)
    const result = await bridge.pluginUninstall(plugin.id)
    replacePluginList(result.plugins)
    showToast(`Плагин «${plugin.name}» удалён`)
  } catch (error) {
    showToast(errorMessage(error))
  }
}

function applyTheme(theme: LauncherThemePreset) {
  activeThemeId.value = theme.id
  appearanceDraft.active_theme_id = theme.id
  appearanceDraft.theme = theme.mode
  appearanceDraft.accent_color = theme.accent
  appearanceDraft.sidebar_mode = theme.sidebar_mode
  appearanceDraft.density = theme.density
  appearanceDraft.color_tokens = {
    ...appearanceDraft.color_tokens,
    accent: theme.accent,
    accent_strong: theme.accent_strong,
    background: theme.background,
    panel: theme.panel,
    panel_soft: theme.panel_soft,
  }
  showToast(`Тема «${theme.name}» применена`)
  if (!busy.value) void saveSettings()
}

async function resetAppearance() {
  if (appearanceDraft.wallpaper_path || appearanceDraft.wallpaper_data || wallpaperPreviewUrl.value) {
    try {
      const result = await bridge.clearWallpaper()
      releaseWallpaperPreview()
      appearanceDraft.wallpaper_path = result.settings.wallpaper_path
      appearanceDraft.wallpaper_data = result.settings.wallpaper_data || ''
      state.settings.wallpaper_path = result.settings.wallpaper_path
      state.settings.wallpaper_data = result.settings.wallpaper_data || ''
    } catch (error) {
      showToast(errorMessage(error))
    }
  }
  activeThemeId.value = 'midnight-signal'
  appearanceDraft.active_theme_id = 'midnight-signal'
  appearanceDraft.theme = 'dark'
  appearanceDraft.accent_color = defaultColorTokens.accent
  appearanceDraft.color_tokens = { ...defaultColorTokens }
  appearanceDraft.radius_tokens = { ...defaultRadiusTokens }
  appearanceDraft.sidebar_position = 'left'
  appearanceDraft.sidebar_mode = 'adaptive'
  appearanceDraft.sidebar_width_mode = 'content'
  appearanceDraft.sidebar_width = 228
  appearanceDraft.sidebar_floating = false
  appearanceDraft.sidebar_visibility = 'always'
  appearanceDraft.animations = 'full'
  appearanceDraft.animation_settings = {
    ...defaultAnimationSettings,
    page_styles: { ...defaultAnimationSettings.page_styles },
    page_durations_ms: { ...defaultAnimationSettings.page_durations_ms },
  } as AnimationSettings
  appearanceDraft.density = 'comfortable'
  appearanceDraft.ui_scale = 100
  appearanceDraft.wallpaper_blur = 12
  appearanceDraft.wallpaper_opacity = 34
  wallpaperRenderBlur.value = 12
  appearanceDraft.page_themes = { instances: 'inherit', 'instance-settings': 'inherit', accounts: 'inherit', mods: 'inherit', modpacks: 'inherit', plugins: 'inherit', themes: 'inherit', settings: 'inherit' }
  appearanceDraft.page_layouts = { instances: 'balanced', 'instance-settings': 'balanced', accounts: 'balanced', mods: 'wide', modpacks: 'wide', plugins: 'wide', themes: 'wide', settings: 'balanced' }
  appearanceDraft.widget_visibility = { 'instances.workspace': true, 'accounts.status': true, 'mods.context': true }
  settingsDraft.font_family = 'system'
  syncActiveTheme()
  showToast('Оформление сброшено к значениям по умолчанию')
  if (!busy.value) void saveSettings()
}

function themeSlug(value: string): string {
  const transliteration: Record<string, string> = {
    а: 'a', б: 'b', в: 'v', г: 'g', д: 'd', е: 'e', ё: 'yo', ж: 'zh', з: 'z', и: 'i', й: 'y', к: 'k', л: 'l', м: 'm', н: 'n', о: 'o', п: 'p', р: 'r', с: 's', т: 't', у: 'u', ф: 'f', х: 'kh', ц: 'ts', ч: 'ch', ш: 'sh', щ: 'shch', ъ: '', ы: 'y', ь: '', э: 'e', ю: 'yu', я: 'ya',
  }
  const slug = [...value.toLowerCase()].map((char) => transliteration[char] ?? char).join('').normalize('NFKD').replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 40)
  return slug || 'local-theme'
}

function uniqueThemeId(name: string): string {
  const existing = new Set(availableThemes.value.map((theme) => theme.id))
  const base = themeSlug(name)
  let id = base
  let suffix = 2
  while (existing.has(id)) {
    id = `${base.slice(0, 40 - String(suffix).length - 1)}-${suffix}`
    suffix += 1
  }
  return id
}

async function refreshLocalThemes() {
  try {
    localThemes.value = (await bridge.themeList()).themes
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function saveCurrentTheme(metadata: ThemeSaveRequest) {
  themeOperationBusy.value = true
  const tokens = appearanceDraft.color_tokens
  const theme = {
    schema_version: 1 as const,
    id: uniqueThemeId(metadata.name),
    name: metadata.name,
    author: metadata.author,
    description: metadata.description,
    mode: appearanceDraft.theme,
    accent: String(tokens.accent || appearanceDraft.accent_color),
    accent_strong: String(tokens.accent_strong || defaultColorTokens.accent_strong),
    background: String(tokens.background || defaultColorTokens.background),
    panel: String(tokens.panel || defaultColorTokens.panel),
    panel_soft: String(tokens.panel_soft || defaultColorTokens.panel_soft),
    density: appearanceDraft.density,
    sidebar_mode: appearanceDraft.sidebar_mode,
  }
  try {
    const result = await bridge.themeSave(theme, metadata)
    localThemes.value = result.themes
    syncActiveTheme()
    showToast(`Тема «${result.theme.name}» сохранена локально`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    themeOperationBusy.value = false
  }
}

async function importThemeArchive(payload: { name: string; data: string; replace: boolean }) {
  themeOperationBusy.value = true
  try {
    const result = await bridge.themeImport(payload.data, payload.replace)
    localThemes.value = result.themes
    showToast(`Тема «${result.theme.name}» импортирована`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    themeOperationBusy.value = false
  }
}

async function deleteLocalTheme(themeId: string) {
  themeOperationBusy.value = true
  try {
    const result = await bridge.themeDelete(themeId)
    localThemes.value = result.themes
    if (activeThemeId.value === themeId) {
      activeThemeId.value = ''
      appearanceDraft.active_theme_id = ''
      syncActiveTheme()
      if (!busy.value) void saveSettings()
    }
    showToast('Локальная тема удалена')
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    themeOperationBusy.value = false
  }
}

// ── Системные метрики в топбаре ─────────────────────────────────────────────
const topbarSystemMetrics = ref<SystemMetricsSnapshot>()
let topbarMetricsTimer: number | undefined

const topbarMetricsVisible = computed(() => settingsDraft.overlay_settings.system_metrics_display === 'always')

const topbarGpus = computed(() => (settingsDraft.overlay_settings.metrics_gpu ? topbarSystemMetrics.value?.gpus || [] : []))

const topbarTemperature = computed(() => {
  if (!settingsDraft.overlay_settings.metrics_temperatures) return undefined
  const temperatures = topbarSystemMetrics.value?.temperatures
  if (!temperatures) return undefined
  if (temperatures.cpu_c !== null && temperatures.cpu_c !== undefined) return { label: 'CPU', value: temperatures.cpu_c }
  if (temperatures.gpu_c !== null && temperatures.gpu_c !== undefined) return { label: 'GPU', value: temperatures.gpu_c }
  return undefined
})

async function pollTopbarMetrics() {
  if (!topbarMetricsVisible.value) return
  try {
    // game_root нужен Rust-движку, чтобы раздел с игрой точно попал в метрики
    // дисков (Python раньше читал его из своей конфигурации сам).
    topbarSystemMetrics.value = await bridge.systemMetrics(false, state.settings.game_dir)
  } catch {
    // метрики не критичны: тихо ждём следующего тика
  }
}

watch(topbarMetricsVisible, (visible) => {
  if (visible) {
    void pollTopbarMetrics()
    if (topbarMetricsTimer === undefined) topbarMetricsTimer = window.setInterval(() => { if (!document.hidden) void pollTopbarMetrics() }, 2000)
  } else if (topbarMetricsTimer !== undefined) {
    window.clearInterval(topbarMetricsTimer)
    topbarMetricsTimer = undefined
    topbarSystemMetrics.value = undefined
  }
}, { immediate: true })

// ── Профили настроек лаунчера ───────────────────────────────────────────────
const settingsProfiles = ref<SettingsProfile[]>([])
const settingsProfilesLoading = ref(false)
const settingsProfileBusy = ref(false)
const settingsProfileName = ref('')
const backupDestinationOptions: SelectOption[] = [
  { value: 'local', label: 'Локально', meta: 'game_dir/backups/<экземпляр>' },
  { value: 'cloud', label: 'WebDAV', meta: 'Тот же endpoint, что у облака' },
]

async function loadSettingsProfiles() {
  settingsProfilesLoading.value = true
  try {
    settingsProfiles.value = (await bridge.settingsProfilesList()).profiles
  } catch {
    settingsProfiles.value = []
  } finally {
    settingsProfilesLoading.value = false
  }
}

function formatDateShort(value: number): string {
  if (!value) return ''
  return new Date(value > 10_000_000_000 ? value : value * 1000).toLocaleString()
}

async function saveSettingsProfileAs() {
  const name = settingsProfileName.value.trim()
  if (!name) return
  settingsProfileBusy.value = true
  try {
    settingsProfiles.value = (await bridge.settingsProfileSave(name)).profiles
    settingsProfileName.value = ''
    showToast(`Профиль «${name}» сохранён`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    settingsProfileBusy.value = false
  }
}

async function overwriteSettingsProfile(name: string) {
  if (!(await confirmDialog(`Перезаписать профиль «${name}» текущими настройками?`, { confirmLabel: 'Перезаписать' }))) return
  settingsProfileBusy.value = true
  try {
    settingsProfiles.value = (await bridge.settingsProfileSave(name)).profiles
    showToast(`Профиль «${name}» обновлён`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    settingsProfileBusy.value = false
  }
}

async function applySettingsProfile(name: string) {
  if (!(await confirmDialog(`Применить профиль «${name}»? Текущие настройки будут заменены.`, { confirmLabel: 'Применить' }))) return
  settingsProfileBusy.value = true
  try {
    const result = await bridge.settingsProfileApply(name)
    applyState({ instances: state.instances, accounts: state.accounts, settings: result.settings, system: state.system })
    showToast(`Профиль «${name}» применён (${result.applied.length} параметров)`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    settingsProfileBusy.value = false
  }
}

async function deleteSettingsProfile(name: string) {
  if (!(await confirmDialog(`Удалить профиль «${name}»?`, { danger: true, confirmLabel: 'Удалить' }))) return
  settingsProfileBusy.value = true
  try {
    settingsProfiles.value = (await bridge.settingsProfileDelete(name)).profiles
    showToast('Профиль удалён')
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    settingsProfileBusy.value = false
  }
}

async function exportSettingsProfile(name: string) {
  settingsProfileBusy.value = true
  try {
    const result = await bridge.settingsProfileExport(name)
    const url = URL.createObjectURL(new Blob([result.json], { type: 'application/json' }))
    const link = document.createElement('a')
    link.href = url
    link.download = result.file_name
    link.click()
    window.setTimeout(() => URL.revokeObjectURL(url), 1000)
    showToast(`Профиль «${result.name}» экспортирован`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    settingsProfileBusy.value = false
  }
}

async function importSettingsProfile() {
  const input = document.createElement('input')
  input.type = 'file'
  input.accept = '.json,application/json'
  input.onchange = async () => {
    const file = input.files?.[0]
    if (!file) return
    settingsProfileBusy.value = true
    try {
      const json = await file.text()
      settingsProfiles.value = (await bridge.settingsProfileImport(json, file.name.replace(/\.json$/i, ''))).profiles
      showToast(`Профиль из «${file.name}» импортирован`)
    } catch (error) {
      showToast(errorMessage(error))
    } finally {
      settingsProfileBusy.value = false
    }
  }
  input.click()
}

async function exportLocalTheme(themeId: string) {
  themeOperationBusy.value = true
  try {
    const result = await bridge.themeExport(themeId)
    const binary = atob(result.data)
    const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0))
    const url = URL.createObjectURL(new Blob([bytes], { type: 'application/zip' }))
    const link = document.createElement('a')
    link.href = url
    link.download = result.file_name
    link.click()
    window.setTimeout(() => URL.revokeObjectURL(url), 1000)
    showToast(`Архив темы «${themeId}» сохранён`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    themeOperationBusy.value = false
  }
}

async function installGithubTheme(payload: { owner: string; repo: string; replace: boolean }) {
  if (githubThemeInstallBusy.value) return
  githubThemeInstallBusy.value = true
  try {
    const result = await bridge.githubThemeInstall(payload.owner, payload.repo, payload.replace)
    localThemes.value = result.themes
    showToast(`Тема «${result.theme.name}» добавлена из GitHub · SHA-256 ${result.metadata.sha256}`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    githubThemeInstallBusy.value = false
  }
}

function themeMatchesSettings(theme: LauncherThemePreset): boolean {
  const tokens = appearanceDraft.color_tokens
  return theme.mode === appearanceDraft.theme
    && theme.accent.toLowerCase() === appearanceDraft.accent_color.toLowerCase()
    && theme.accent.toLowerCase() === String(tokens.accent || '').toLowerCase()
    && theme.accent_strong.toLowerCase() === String(tokens.accent_strong || '').toLowerCase()
    && theme.background.toLowerCase() === String(tokens.background || '').toLowerCase()
    && theme.panel.toLowerCase() === String(tokens.panel || '').toLowerCase()
    && theme.panel_soft.toLowerCase() === String(tokens.panel_soft || '').toLowerCase()
    && theme.density === appearanceDraft.density
    && theme.sidebar_mode === appearanceDraft.sidebar_mode
}

function syncActiveTheme() {
  const persisted = availableThemes.value.find((theme) => theme.id === appearanceDraft.active_theme_id && themeMatchesSettings(theme))
  const matching = persisted || availableThemes.value.find((theme) => themeMatchesSettings(theme))
  activeThemeId.value = matching?.id || ''
  appearanceDraft.active_theme_id = activeThemeId.value
}

function selectSkinModel(value: 'classic' | 'slim') {
  skinModel.value = value
  currentSkinModel.value = value
}

function selectSkin(skin: SkinInfo) {
  selectedSkinId.value = skin.skin_id
  const model = skin.model === 'slim' ? 'slim' : 'classic'
  skinModel.value = model
  currentSkinModel.value = model
}

function selectCurrentSkin() {
  selectedSkinId.value = ''
  const model = activeRemoteSkin.value?.variant?.toUpperCase() === 'SLIM' ? 'slim' : 'classic'
  skinModel.value = model
  currentSkinModel.value = model
}

function updateSidebarWidth(event: PointerEvent) {
  if (!sidebarResizeActive.value) return
  const position = appearanceDraft.sidebar_position
  const next = position === 'right' ? window.innerWidth - event.clientX : event.clientX
  appearanceDraft.sidebar_width = Math.min(360, Math.max(168, Math.round(next)))
}

function stopSidebarResize() {
  if (!sidebarResizeActive.value) return
  sidebarResizeActive.value = false
  document.body.style.cursor = ''
  window.removeEventListener('pointermove', updateSidebarWidth)
  window.removeEventListener('pointerup', stopSidebarResize)
}

function startSidebarResize(event: PointerEvent) {
  if (appearanceDraft.sidebar_width_mode !== 'manual') return
  event.preventDefault()
  sidebarResizeActive.value = true
  document.body.style.cursor = 'col-resize'
  window.addEventListener('pointermove', updateSidebarWidth)
  window.addEventListener('pointerup', stopSidebarResize)
}

function ensureCatalogForView(view: ViewName) {
  if (view !== 'mods' && view !== 'modpacks') return
  const allowed = view === 'mods' && modProjectType.value === 'plugin'
    ? settingsDraft.plugin_catalogs.filter((catalog) => catalog.enabled !== false)
    : settingsDraft.resource_catalogs.filter((catalog) => view === 'modpacks'
      ? ['modrinth', 'ftb', 'technic'].includes(catalog.kind)
      : !['ftb', 'technic'].includes(catalog.kind))
  if (!allowed.length) return
  if (allowed.some((catalog) => catalog.id === activeCatalogId.value)) return
  activeCatalogId.value = allowed[0].id
  if (view === 'mods' && modProjectType.value === 'plugin') settingsDraft.active_plugin_catalog_id = allowed[0].id
  else settingsDraft.active_catalog_id = allowed[0].id
}

function ensureCompatibleInstanceForCatalog() {
  if (currentView.value !== 'mods') return
  if (!selectedInstanceName.value) return
  const requiresServer = modProjectType.value === 'plugin'
  if (selectedInstance.value && (requiresServer ? selectedInstance.value.kind === 'server' : selectedInstance.value.kind !== 'server' && selectedInstance.value.game_edition !== 'bedrock')) return
  selectedInstanceName.value = state.instances.find((instance) => requiresServer ? instance.kind === 'server' : instance.kind !== 'server' && instance.game_edition !== 'bedrock')?.name || ''
}

function setView(view: ViewName, pluginTabId = '') {
  if (!startupViewApplied) userNavigatedDuringBootstrap = true
  const changed = currentView.value !== view || activePluginTabId.value !== pluginTabId
  currentView.value = view
  activePluginTabId.value = pluginTabId
  if (view === 'mods' && modProjectType.value === 'modpack') modProjectType.value = 'mod'
  ensureCatalogForView(view)
  ensureCompatibleInstanceForCatalog()
  if (changed) {
    pageTransitionPulse.value = false
    void nextTick(() => { pageTransitionPulse.value = true })
  }
  mobileMenuOpen.value = false
  if (view !== 'instance-settings' && settingsDraft.remember_last_view && settingsDraft.last_view !== view) {
    const request = ++lastViewSaveRequest
    void bridge.saveSettings({ last_view: view }).then((result) => {
      if (request !== lastViewSaveRequest) return
      settingsDraft.last_view = result.settings.last_view
      state.settings.last_view = result.settings.last_view
      mergeSavedSettingsBaseline({ last_view: result.settings.last_view })
      void broadcastPluginEvent('settings.changed', { keys: ['last_view'] })
    }).catch(() => undefined)
  }
  if (changed && view === 'instances') void refreshBootstrap()
  if (changed && view === 'accounts') void refreshBootstrap()
   if (changed && (view === 'mods' || view === 'modpacks')) {
       ensureCompatibleInstanceForCatalog()
      if (view === 'modpacks' || selectedInstance.value) void searchMods()
   }
  if (changed && view === 'settings') {
    void loadGraphicsDiagnostics()
    void refreshJavaOptions()
  }
}

function selectInstanceGroupFilter(filter: string) {
  instanceGroupFilter.value = filter
  if (currentView.value === 'instances') {
    mobileMenuOpen.value = false
    return
  }
  setView('instances')
}

function selectAccountFromSidebar(accountId: string) {
  selectedAccount.value = accountId
  mobileMenuOpen.value = false
}

function setPluginTab(pluginId: string) {
  if (!enabledPluginTabs.value.some((plugin) => plugin.id === pluginId)) return
  setView('plugins', pluginId)
}

function shortcutMatches(event: KeyboardEvent, binding: string): boolean {
  const parts = binding.split('+').map((part) => part.trim().toLowerCase()).filter(Boolean)
  if (!parts.length) return false
  const key = parts[parts.length - 1]
  const modifiers = new Set(parts.slice(0, -1))
  const normalizedKey = key === 'comma' ? ',' : key === 'space' ? ' ' : key === 'esc' ? 'escape' : key
  if (modifiers.has('ctrl') !== event.ctrlKey) return false
  if (modifiers.has('alt') !== event.altKey) return false
  if (modifiers.has('shift') !== event.shiftKey) return false
  if (modifiers.has('meta') !== event.metaKey) return false
  return event.key.toLowerCase() === normalizedKey || event.code.toLowerCase() === `key${normalizedKey}`
}

function isSafePluginShortcutBinding(binding: string): boolean {
  const parts = binding.split('+').map((part) => part.trim().toLowerCase()).filter(Boolean)
  if (parts.length < 2) return false
  const key = parts[parts.length - 1]
  const modifiers = parts.slice(0, -1)
  const allowedModifiers = new Set(['ctrl', 'alt', 'shift', 'meta'])
  if (modifiers.some((modifier) => !allowedModifiers.has(modifier)) || new Set(modifiers).size !== modifiers.length) return false
  if (!modifiers.some((modifier) => modifier === 'ctrl' || modifier === 'alt' || modifier === 'meta')) return false
  return /^[a-z0-9]$/.test(key)
    || /^f(?:[1-9]|1[0-2])$/.test(key)
    || ['escape', 'esc', 'space', 'comma', 'enter', 'tab', 'backspace', 'delete', 'arrowup', 'arrowdown', 'arrowleft', 'arrowright'].includes(key)
}

function handleGlobalKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape' && accountMenuId.value) {
    accountMenuId.value = ''
    return
  }
  if (event.defaultPrevented || document.querySelector('.dialog-scrim')) return
  const target = event.target as HTMLElement | null
  const editing = target?.matches('input, textarea, select, [contenteditable="true"]')
  // Резерв для оверлея: глобальный перехват клавиш не работает в
  // Wayland-сессиях и занятых комбинациях, но пока окно лаунчера в фокусе,
  // назначенная клавиша обязана открывать оверлей. Это дублирует
  // bindGameHotkeys, а не заменяет его.
  if (!overlayMode && settingsDraft.overlay_settings.enabled) {
    const overlayHotkey = normalizeHotkey(settingsDraft.overlay_settings.hotkey_overlay, 'Shift+Tab')
    const recordingHotkey = normalizeHotkey(settingsDraft.overlay_settings.hotkey_recording, 'F7')
    const hardModifier = event.ctrlKey || event.altKey || event.metaKey
    if (!editing || hardModifier) {
      if (shortcutMatches(event, overlayHotkey)) {
        event.preventDefault()
        void handleOverlayToggleHotkey()
        return
      }
      if (shortcutMatches(event, recordingHotkey)) {
        event.preventDefault()
        void handleRecordingHotkey()
        return
      }
    }
  }
  const shortcuts = settingsDraft.keyboard_shortcuts
  if (shortcutMatches(event, shortcuts.open_settings)) {
    event.preventDefault()
    setView('settings')
  } else if (shortcutMatches(event, shortcuts.refresh)) {
    event.preventDefault()
    void refreshBootstrap()
    void refreshVersions()
  } else if (shortcutMatches(event, shortcuts.new_instance)) {
    event.preventDefault()
    openCreateInstance()
  } else if (shortcutMatches(event, shortcuts.focus_search)) {
    event.preventDefault()
    if (currentView.value === 'instances') {
      void nextTick(() => document.querySelector<HTMLInputElement>('.instance-search input')?.focus())
    } else {
      if (currentView.value !== 'mods') setView('mods')
      void nextTick(() => document.querySelector<HTMLInputElement>('.search-bar input')?.focus())
    }
  } else if (shortcutMatches(event, shortcuts.toggle_activity)) {
    event.preventDefault()
    if (!editing || event.ctrlKey || event.metaKey) activity.open = !activity.open
  } else if (!editing && !event.repeat) {
    const pluginShortcut = pluginShortcuts.value.find((shortcut) => shortcutMatches(event, shortcut.definition.default_key))
    if (pluginShortcut) {
      event.preventDefault()
      void runPluginShortcut(pluginShortcut)
    }
  }
}

function applyState(next: AppState, preserveAppearance = false) {
  const previousSavedSnapshot = preserveAppearance ? savedSettingsSnapshot : undefined
  hydratingSettings.value = true
  const preservedFontFamily = preserveAppearance ? settingsDraft.font_family : ''
  Object.assign(state, next)
  // Сверка меток сборки: если в приложение подменили только Python-бэкенд
  // (или только интерфейс), контракты операций расходятся и «ломаются»
  // случайные экраны. Молчать об этом нельзя — показываем баннер.
  const backendStamp = String(next.build_stamp || '')
  if (backendStamp) {
    if (backendStamp !== FRONTEND_BUILD_STAMP) {
      if (!buildStampMismatch.value) {
        buildStampMismatch.value = { frontend: FRONTEND_BUILD_STAMP, backend: backendStamp }
        showToast(`Части лаунчера собраны из разных версий: интерфейс ${FRONTEND_BUILD_STAMP}, бэкенд ${backendStamp}. Приложение нужно пересобрать целиком.`, 'danger')
      }
    } else {
      buildStampMismatch.value = null
    }
  }
  // Тип сессии решает, как создавать окно оверлея и стоит ли вообще
  // пытаться регистрировать глобальные горячие клавиши (в Wayland это
  // бесполезно). Данные приходят с бэкенда (XDG_SESSION_TYPE).
  // Реальный бэкенд важнее типа сессии: пакетные врапперы форсируют
  // GDK_BACKEND=x11 (XWayland) — в этой связке глобальные клавиши и окно
  // «поверх всех» работают, и отключать их из-за номинального Wayland
  // нельзя (оверлей «не открывался» даже с кастомным хоткеем).
  const sessionWayland = String(state.system.session || '').toLowerCase() === 'wayland'
  const forcedX11 = String(state.system.gdk_backend || '').toLowerCase() === 'x11'
  setOverlayWaylandMode(sessionWayland && !forcedX11)
  // Обратная сторона форсированного X11: на NVIDIA прозрачное окно
  // webkit2gtk под XWayland перерисовывается только под курсором
  // («двинешь мышкой — пропадает»). Прозрачность отключаем превентивно и
  // отдельно от хоткеев — ровно для связки Wayland-сессия + X11 + NVIDIA.
  setOverlayOpaqueWindow(sessionWayland && forcedX11 && state.system.nvidia_smi === true)
  replacePluginList(next.plugins || [])
  Object.assign(settingsDraft, {
    game_dir: next.settings.game_dir,
    java_path: next.settings.java_path,
    memory: next.settings.memory,
    max_downloads: next.settings.max_downloads,
     startup_view: persistentViewOptions.some((option) => option.value === next.settings.startup_view) ? next.settings.startup_view : 'instances',
     last_view: persistentViewOptions.some((option) => option.value === next.settings.last_view) ? next.settings.last_view : 'instances',
    remember_last_view: next.settings.remember_last_view !== false,
    default_version_filter: next.settings.default_version_filter || 'release',
    default_loader: next.settings.default_loader || 'fabric',
    default_account_id: next.settings.default_account_id || 'auto',
    open_last_instance: next.settings.open_last_instance !== false,
    launch_after_install: next.settings.launch_after_install === true,
    auto_refresh_metadata: next.settings.auto_refresh_metadata !== false,
    metadata_refresh_hours: next.settings.metadata_refresh_hours || 6,
    download_timeout_seconds: next.settings.download_timeout_seconds || 30,
    download_retries: next.settings.download_retries || 5,
    confirm_destructive_actions: next.settings.confirm_destructive_actions !== false,
    show_activity_panel: next.settings.show_activity_panel !== false,
    show_toasts: next.settings.show_toasts !== false,
    minimize_on_launch: next.settings.minimize_on_launch === true,
    language: next.settings.language === 'en' ? 'en' : next.settings.language === 'ru' ? 'ru' : null,
    font_family: next.settings.font_family || 'system',
    keep_logs_days: next.settings.keep_logs_days ?? 14,
    telemetry: next.settings.telemetry === true,
     keyboard_shortcuts: (() => {
      const merged = { ...settingsDraft.keyboard_shortcuts, ...(next.settings.keyboard_shortcuts || {}) } as Record<string, string>
      // нормализуем и то, что пришло с бэкенда: настройки могли быть
      // сохранены старой версией со свободным вводом
      for (const row of shortcutRows) merged[row.key] = normalizeHotkey(merged[row.key], row.default)
      return merged
    })(),
     graphics_mode: next.settings.graphics_mode || 'auto',
     overlay_settings: {
      ...settingsDraft.overlay_settings,
      ...(next.settings.overlay_settings || {}),
     } as OverlaySettings,
     backup_schedule: { ...(next.settings.backup_schedule || settingsDraft.backup_schedule) },
     launcher_profiles: (next.settings.launcher_profiles || []).map((profile) => ({ ...profile, jvm_arguments: [...(profile.jvm_arguments || [])], game_arguments: [...(profile.game_arguments || [])], jvm_preset: profile.jvm_preset ?? 'none' })),
     active_launcher_profile_id: next.settings.active_launcher_profile_id || next.settings.launcher_profiles?.[0]?.id || 'default',
      http_proxy: next.settings.http_proxy || '',
     developer_mode: next.settings.developer_mode === true,
    login_source: next.settings.login_source || 'microsoft',
    login_api_url: next.settings.login_api_url || 'https://login.microsoftonline.com/consumers/oauth2/v2.0',
    catalog_source: next.settings.catalog_source || 'modrinth',
    catalog_api_url: next.settings.catalog_api_url || 'https://api.modrinth.com/v2',
    data_source: next.settings.data_source || 'mojang',
    data_api_url: next.settings.data_api_url || 'https://piston-meta.mojang.com/mc/game/version_manifest_v2.json',
     resource_catalogs: (next.settings.resource_catalogs || []).map((catalog) => ({ ...catalog })),
     active_catalog_id: next.settings.active_catalog_id || next.settings.resource_catalogs?.[0]?.id || 'modrinth',
      plugin_catalogs: (next.settings.plugin_catalogs || []).map((catalog) => ({ ...catalog })),
      active_plugin_catalog_id: next.settings.active_plugin_catalog_id || next.settings.plugin_catalogs?.[0]?.id || 'modrinth-plugins',
     cloud_url: next.settings.cloud_url || '',
     cloud_username: next.settings.cloud_username || '',
     cloud_remote_path: next.settings.cloud_remote_path || 'nivora-launcher',
     cloud_include_accounts: next.settings.cloud_include_accounts === true,
     cloud_last_sync: next.settings.cloud_last_sync || 0,
     cloud_configured: next.settings.cloud_configured === true,
   })
   if (preserveAppearance && preservedFontFamily) settingsDraft.font_family = preservedFontFamily
   if (!preserveAppearance) {
     Object.assign(appearanceDraft, {
       theme: next.settings.theme || 'dark',
       active_theme_id: next.settings.active_theme_id || 'midnight-signal',
       wallpaper_path: next.settings.wallpaper_path || '',
       wallpaper_data: next.settings.wallpaper_data || '',
       wallpaper_blur: next.settings.wallpaper_blur,
       wallpaper_opacity: next.settings.wallpaper_opacity,
        accent_color: next.settings.accent_color === legacyDefaultColorTokens.accent
          ? defaultColorTokens.accent
          : next.settings.accent_color || defaultColorTokens.accent,
       sidebar_position: next.settings.sidebar_position || 'left',
       sidebar_mode: next.settings.sidebar_mode || 'adaptive',
       sidebar_width_mode: next.settings.sidebar_width_mode || 'content',
       sidebar_width: next.settings.sidebar_width || 228,
       sidebar_floating: next.settings.sidebar_floating || false,
        sidebar_visibility: next.settings.sidebar_visibility || 'always',
        animations: next.settings.animations || 'full',
        animation_settings: {
          ...defaultAnimationSettings,
          ...(next.settings.animation_settings || {}),
          // Старые настройки хранили явный «fade» для каждой страницы, поэтому
          // глобальный стиль анимаций никогда не применялся. Наследуем: «fade»
          // визуально совпадает с глобальным значением по умолчанию.
          page_styles: (() => {
            const legacyStyles = next.settings.animation_settings?.page_styles || {}
            const merged: Record<string, string> = { ...defaultAnimationSettings.page_styles }
            for (const [view, style] of Object.entries(legacyStyles)) merged[view] = style === 'fade' ? '' : style
            return merged as typeof defaultAnimationSettings.page_styles
          })(),
          page_durations_ms: { ...defaultAnimationSettings.page_durations_ms, ...(next.settings.animation_settings?.page_durations_ms || {}) },
        },
        density: next.settings.density || 'comfortable',
       ui_scale: next.settings.ui_scale || 100,
       radius_tokens: { ...defaultRadiusTokens, ...(next.settings.radius_tokens || {}) },
       page_themes: { ...appearanceDraft.page_themes, ...next.settings.page_themes },
       page_layouts: { ...appearanceDraft.page_layouts, ...next.settings.page_layouts },
       widget_visibility: { ...appearanceDraft.widget_visibility, ...next.settings.widget_visibility },
        color_tokens: migrateDefaultColors(next.settings.color_tokens),
      })
      wallpaperRenderBlur.value = next.settings.wallpaper_blur
      if (next.settings.color_tokens?.accent) {
        appearanceDraft.accent_color = appearanceDraft.color_tokens.accent
      }
     syncActiveTheme()
   }
   activeCatalogId.value = currentView.value === 'mods' && modProjectType.value === 'plugin'
     ? next.settings.active_plugin_catalog_id || next.settings.plugin_catalogs?.[0]?.id || 'modrinth-plugins'
     : next.settings.active_catalog_id || next.settings.resource_catalogs?.[0]?.id || 'modrinth'
   if (currentView.value === 'mods' && modProjectType.value === 'plugin') settingsDraft.active_plugin_catalog_id = activeCatalogId.value
   else settingsDraft.active_catalog_id = activeCatalogId.value
  syncMemoryDraft(next.settings.memory)
  javaSelection.value = next.settings.java_path || 'auto'
  if (next.settings.java_path && !javaOptions.value.some((option) => option.value === next.settings.java_path)) {
    javaOptions.value.push({ value: next.settings.java_path, label: 'Настроенный путь', meta: next.settings.java_path })
  }
    if (!selectedInstanceName.value || !next.instances.some((instance) => instance.name === selectedInstanceName.value)) {
      let rememberedName = ''
      if (settingsDraft.open_last_instance) {
        try { rememberedName = localStorage.getItem(LAST_INSTANCE_STORAGE_KEY) || '' } catch { /* storage may be disabled */ }
      }
      const remembered = next.instances.find((instance) => instance.name === rememberedName)
      const firstClient = next.instances.find((instance) => instance.kind !== 'server')
      const selected = remembered || firstClient || next.instances[0]
      selectedInstanceName.value = selected?.name || ''
       selectedVersion.value = selected?.kind === 'server' ? selected.server_core_version || selected.minecraft_version || selected.version_id || selectedVersion.value : selected?.minecraft_version || selected?.version_id || selectedVersion.value
       selectedLoader.value = selected?.loader || selectedLoader.value
    }
   const nextSavedSnapshot = takeSettingsDraftSnapshot()
   if (previousSavedSnapshot) {
     nextSavedSnapshot.appearance = previousSavedSnapshot.appearance
     nextSavedSnapshot.settings.font_family = previousSavedSnapshot.settings.font_family
   }
   savedSettingsSnapshot = nextSavedSnapshot
   const hasUnsavedSettings = !settingsValuesMatch(currentSettingsDraftValues(), savedSettingsSnapshot)
   settingsSaveState.value = hasUnsavedSettings ? 'dirty' : 'saved'
   settingsSaveMessage.value = hasUnsavedSettings ? 'Есть несохранённые изменения' : 'Изменения сохранены'
   // Язык применяется на каждое обновление состояния: и при старте, и после
   // сохранения настроек. Язык для нового пользователя выбирается в визарде
   // первоначальной настройки (фаза 'language'); здесь — только применение.
   setLocale(settingsDraft.language === 'en' ? 'en' : 'ru')
   void nextTick(() => {
     hydratingSettings.value = false
     refreshSettingsSaveState()
   })
  }

async function refreshBootstrap(preserveAppearance = true) {
  try {
    const next = await bridge.bootstrap()
    const groups = await bridge.instanceGroups().catch(() => ({ groups: next.settings.instance_groups || [] }))
    next.settings.instance_groups = groups.groups || []
    applyState(next, preserveAppearance)
    if (!startupViewApplied) {
      startupViewApplied = true
      const defaultFilter = next.settings.default_version_filter
      if (['all', 'release', 'snapshot', 'old_beta', 'old_alpha'].includes(defaultFilter)) versionFilter.value = defaultFilter as typeof versionFilter.value
      selectedLoader.value = next.settings.default_loader || selectedLoader.value
       selectedAccount.value = next.settings.default_account_id || 'auto'
         if (!userNavigatedDuringBootstrap) {
           const preferredView = next.settings.remember_last_view ? next.settings.last_view : next.settings.startup_view
           currentView.value = persistentViewOptions.some((option) => option.value === preferredView) ? preferredView : 'instances'
         }
        ensureCatalogForView(currentView.value)
         ensureCompatibleInstanceForCatalog()
     }
    // Wayland-пользователь должен один раз узнать, почему глобальная
    // комбинация может не срабатывать, и как открыть оверлей иначе.
    if (!waylandOverlayHintShown && settingsDraft.overlay_settings.enabled && overlayWaylandMode()) {
      waylandOverlayHintShown = true
      showToast('Wayland-сессия: глобальные клавиши не поддерживаются. Комбинация оверлея работает, пока окно лаунчера в фокусе; также оверлей открывается из меню значка в трее.', 'warning')
    }
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    initializing.value = false
  }
}

async function refreshVersions() {
  loadingVersions.value = true
  versionError.value = ''
  try {
    versions.value = sortVersionsByRelease((await bridge.versions(true)).versions)
    if (filteredVersions.value.length && !filteredVersions.value.some((version) => version.id === selectedVersion.value)) {
      selectedVersion.value = filteredVersions.value[0].id
    }
    showToast(`Загружено версий: ${versions.value.length}`)
  } catch (error) {
    versionError.value = errorMessage(error)
    showToast(errorMessage(error))
  } finally {
    loadingVersions.value = false
  }
}

async function refreshJavaOptions() {
  try {
    javaOptions.value = (await bridge.javaOptions()).options
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function validateJavaPath() {
  const path = settingsDraft.java_path.trim()
  if (!path) {
    javaValidation.value = 'Сначала укажите путь к Java executable.'
    return
  }
  try {
    const result = await bridge.javaValidate(path)
    settingsDraft.java_path = result.path
    javaSelection.value = 'manual'
    javaValidation.value = `Java ${result.version} найдена и готова к запуску.`
  } catch (error) {
    javaValidation.value = errorMessage(error)
  }
}

function releaseWallpaperPreview() {
  if (wallpaperPreviewUrl.value.startsWith('blob:')) URL.revokeObjectURL(wallpaperPreviewUrl.value)
  wallpaperPreviewUrl.value = ''
}

function setWallpaperPreview(url: string) {
  releaseWallpaperPreview()
  wallpaperPreviewUrl.value = url
}

function wallpaperMime(file: File): string {
  if (['image/png', 'image/jpeg', 'image/webp'].includes(file.type)) return file.type
  const suffix = file.name.split('.').pop()?.toLowerCase()
  return suffix === 'png' ? 'image/png' : suffix === 'webp' ? 'image/webp' : 'image/jpeg'
}

function wallpaperExtension(mime: string): string {
  return mime === 'image/png' ? 'png' : mime === 'image/webp' ? 'webp' : 'jpg'
}

function encodeBlob(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result).split(',', 2)[1] || '')
    reader.onerror = () => reject(reader.error || new Error('Не удалось прочитать изображение'))
    reader.readAsDataURL(blob)
  })
}

async function prepareWallpaper(file: File): Promise<{ fileName: string; data: string }> {
  let source: CanvasImageSource | undefined
  let width = 0
  let height = 0
  let closeSource: (() => void) | undefined
  const mime = wallpaperMime(file)
  const fileName = `wallpaper.${wallpaperExtension(mime)}`
  try {
    if (typeof createImageBitmap === 'function') {
      const bitmap = await createImageBitmap(file)
      source = bitmap
      width = bitmap.width
      height = bitmap.height
      closeSource = () => bitmap.close()
    } else {
      const objectUrl = URL.createObjectURL(file)
      const image = await new Promise<HTMLImageElement>((resolve, reject) => {
        const value = new Image()
        value.onload = () => resolve(value)
        value.onerror = () => reject(new Error('Не удалось прочитать изображение'))
        value.src = objectUrl
      })
      URL.revokeObjectURL(objectUrl)
      source = image
      width = image.naturalWidth
      height = image.naturalHeight
    }
    const maxDimension = 1440
    if (width <= maxDimension && height <= maxDimension) {
      closeSource?.()
      return { fileName, data: await encodeBlob(file) }
    }
    const scale = Math.min(1, maxDimension / Math.max(width, height))
    const canvas = document.createElement('canvas')
    canvas.width = Math.max(1, Math.round(width * scale))
    canvas.height = Math.max(1, Math.round(height * scale))
    const context = canvas.getContext('2d')
    if (!context || !source || !width || !height) throw new Error('Canvas недоступен')
    context.drawImage(source, 0, 0, canvas.width, canvas.height)
    closeSource?.()
    const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, mime, mime === 'image/jpeg' ? .86 : undefined))
    if (!blob) throw new Error('Не удалось сжать изображение')
    return { fileName, data: await encodeBlob(blob) }
  } catch {
    return { fileName: file.name, data: await encodeBlob(file) }
  }
}

async function importWallpaper(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  try {
    if (file.size > 32 * 1024 * 1024) throw new Error('Файл обоев превышает лимит 32 MiB')
    const prepared = await prepareWallpaper(file)
    setWallpaperPreview(`data:${wallpaperMime(file)};base64,${prepared.data}`)
    const result = await bridge.importWallpaper(prepared.fileName, prepared.data)
    appearanceDraft.wallpaper_path = result.settings.wallpaper_path
    appearanceDraft.wallpaper_data = result.settings.wallpaper_data || ''
    state.settings.wallpaper_path = result.settings.wallpaper_path
    state.settings.wallpaper_data = result.settings.wallpaper_data || ''
    showToast('Обои сохранены')
  } catch (error) {
    releaseWallpaperPreview()
    showToast(errorMessage(error))
  } finally {
    input.value = ''
  }
}

async function clearWallpaper() {
  try {
    const result = await bridge.clearWallpaper()
    releaseWallpaperPreview()
    appearanceDraft.wallpaper_path = result.settings.wallpaper_path
    appearanceDraft.wallpaper_data = result.settings.wallpaper_data || ''
    state.settings.wallpaper_path = result.settings.wallpaper_path
    state.settings.wallpaper_data = result.settings.wallpaper_data || ''
    showToast('Обои убраны')
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function refreshLoaderChoices() {
  const requestId = ++loaderRequestId.value
  if (selectedLoader.value === 'vanilla' || !selectedVersion.value) {
    loaderChoices.value = []
    selectedLoaderVersion.value = ''
    loadingChoices.value = false
    return
  }
  loadingChoices.value = true
  try {
    const result = await bridge.loaderChoices(selectedLoader.value, selectedVersion.value)
    if (requestId !== loaderRequestId.value) return
    loaderChoices.value = result.choices
    selectedLoaderVersion.value = loaderChoices.value[0]?.version || ''
  } catch {
    if (requestId === loaderRequestId.value) {
      loaderChoices.value = []
      selectedLoaderVersion.value = ''
    }
  } finally {
    if (requestId === loaderRequestId.value) loadingChoices.value = false
  }
}

watch([selectedVersion, selectedLoader, currentView], () => {
  if (!createInstanceOpen.value && currentView.value !== 'modpacks') return
  if (loaderChoiceTimer) window.clearTimeout(loaderChoiceTimer)
  loaderChoiceTimer = window.setTimeout(() => {
    loaderChoiceTimer = undefined
    void refreshLoaderChoices()
  }, 120)
})

watch(selectedInstanceName, (value) => {
  if (!value) return
  try { localStorage.setItem(LAST_INSTANCE_STORAGE_KEY, value) } catch { /* storage may be disabled */ }
})

watch(selectedInstanceName, () => {
  void broadcastPluginEvent('instance.selected', { instance: pluginInstanceRef() })
})

watch([wallpaperUrl, wallpaperRenderBlur], () => {
  wallpaperLoadFailed.value = false
  void renderWallpaper()
}, { immediate: true })

watch([modProjectType, selectedInstanceName, currentView, activeCatalogId, selectedVersion], () => {
  if (currentView.value === 'mods') {
    ensureCatalogForView(currentView.value)
    ensureCompatibleInstanceForCatalog()
  }
  modSearchRequestId += 1
  modResults.value = []
  modError.value = ''
  modSearchPerformed.value = false
  if (currentView.value !== 'mods' && currentView.value !== 'modpacks') return
  if (modSearchTimer) window.clearTimeout(modSearchTimer)
  modSearchTimer = window.setTimeout(() => {
    modSearchTimer = undefined
    void searchMods()
  }, 220)
})

watch(memoryUnit, (next, previous) => {
  const kib = Number(memoryValue.value || 1) * memoryMultipliers[previous]
  memoryValue.value = kib / memoryMultipliers[next]
  clampMemoryDraft()
})
watch([memoryValue, () => state.settings.memory_total_kib], () => clampMemoryDraft())
watch([serverMemoryValue, serverMemoryUnit], () => { serverCreation.memory = serverMemorySetting.value })
watch(() => state.settings.memory_total_kib, clampServerMemoryDraft)
watch(() => serverCreation.core, (next, previous) => {
  if (createInstanceOpen.value && creationKind.value === 'server' && serverCreation.install_mode === 'official' && next !== previous) void refreshServerCoreVersions(next)
})

function openActivity(title: string, detail: string, visible = true) {
  if (activityCloseTimer) window.clearTimeout(activityCloseTimer)
  activityCloseTimer = undefined
  activity.open = visible && settingsDraft.show_activity_panel
  activity.title = title
  activity.detail = detail
  activity.progress = 12
  activity.busy = true
  activity.cancelling = false
  activity.logs = []
  activity.verification_code = undefined
  activity.verification_uri = undefined
}

function finishActivity(detail: string, logs?: string[]) {
  activity.detail = detail
  activity.progress = 100
  activity.busy = false
  activity.cancelling = false
  if (logs?.length) {
    if (!activity.logs.length) activity.logs = logs
    else activity.logs.push(...logs.filter((line) => !activity.logs.includes(line)))
  }
  if (activityCloseTimer) window.clearTimeout(activityCloseTimer)
  activityCloseTimer = window.setTimeout(() => {
    if (!activity.busy) activity.open = false
    activityCloseTimer = undefined
  }, 1500)
}

// Прогресс фоновых джоб Bedrock из workspace: тикающие апдейты не должны
// переоткрывать панель (это сбрасывает логи), обновляют только текст.
function onBedrockActivity(payload: { title: string; detail: string; done?: boolean; logs?: string[] }) {
  if (payload.done) {
    finishActivity(payload.detail, payload.logs)
    return
  }
  if (activity.busy && activity.title === payload.title) {
    activity.detail = payload.detail
    return
  }
  openActivity(payload.title, payload.detail)
}

async function pollActivityLog(instanceName: string) {
  if (activityLogPollInFlight) return
  activityLogPollInFlight = true
  try {
    const result = await bridge.instanceLog(instanceName)
    if (result.lines.length) activity.logs = result.lines.slice(-200)
    if (result.running) activity.detail = 'Minecraft работает…'
  } catch {
    // The backend response remains the source of the final operation result.
  } finally {
    activityLogPollInFlight = false
  }
}

function startActivityLogPolling(instanceName: string) {
  if (activityLogTimer) window.clearInterval(activityLogTimer)
  void pollActivityLog(instanceName)
  activityLogTimer = window.setInterval(() => void pollActivityLog(instanceName), 2500)
}

function stopActivityLogPolling() {
  if (activityLogTimer) window.clearInterval(activityLogTimer)
  activityLogTimer = undefined
}

function applyMicrosoftDeviceCode(code: string, verificationUri: string) {
  activity.open = true
  activity.title = 'Вход через Microsoft'
  activity.busy = true
  activity.detail = 'Откройте страницу входа и введите код.'
  activity.verification_code = code
  activity.verification_uri = verificationUri
  const message = `КОД MICROSOFT: ${code}`
  if (!activity.logs.includes(message)) activity.logs.push(message)
}

async function pollMicrosoftLoginStatus() {
  if (microsoftStatusPollInFlight) return
  microsoftStatusPollInFlight = true
  try {
    const result = await bridge.microsoftLoginStatus()
    if (result.status === 'device_code' && result.code) {
      applyMicrosoftDeviceCode(result.code, result.verification_uri || '')
    }
  } catch {
    // The login request remains authoritative for the final result.
  } finally {
    microsoftStatusPollInFlight = false
  }
}

function startMicrosoftStatusPolling() {
  if (microsoftStatusTimer) window.clearInterval(microsoftStatusTimer)
  void pollMicrosoftLoginStatus()
  microsoftStatusTimer = window.setInterval(() => void pollMicrosoftLoginStatus(), 1000)
}

function stopMicrosoftStatusPolling() {
  if (microsoftStatusTimer) window.clearInterval(microsoftStatusTimer)
  microsoftStatusTimer = undefined
}

async function cancelActivity() {
  if (!activity.busy || activity.cancelling) return
  activity.cancelling = true
  activity.detail = 'Останавливаю операцию…'
  try {
    await bridge.cancelActiveOperations()
    // Не всякая операция реагирует на маркер остановки мгновенно: через
    // 5 секунд честно сообщаем состояние вместо вечного «Останавливаю…».
    window.setTimeout(() => {
      if (activity.busy && activity.cancelling) {
        activity.detail = 'Остановка запрошена — операция завершит текущий шаг сама.'
      }
    }, 5000)
  } catch (error) {
    activity.cancelling = false
    showToast(errorMessage(error))
  }
}

async function minimizeForLaunch() {
  if (!settingsDraft.minimize_on_launch || typeof window === 'undefined') return
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window')
    await getCurrentWindow().minimize()
  } catch {
    // Browser preview and older shell builds do not expose a window controller.
  }
}

function isCancelled(error: unknown): boolean {
  return errorMessage(error).toLowerCase().includes('операция остановлена')
}

function openCreateInstance() {
  creationKind.value = 'client'
  bedrockCreationStatus.value = null
  createInstanceOpen.value = true
  creationName.value = ''
  setView('instances')
  void refreshLoaderChoices()
}

function openCreateBedrock() {
  creationKind.value = 'bedrock'
  bedrockCreationStatus.value = null
  createInstanceOpen.value = true
  creationName.value = ''
  setView('instances')
  void bridge.bedrockRuntimeStatus().then((result) => {
    if (creationKind.value === 'bedrock') bedrockCreationStatus.value = result.status
  }).catch((error) => {
    bedrockCreationStatus.value = { available: false, supported: false, version: '', message: errorMessage(error) }
  })
  void loadBedrockCreationVersions()
}

const bedrockCreationEditionOptions: SelectOption[] = [
  { value: 'release', label: 'Обычная версия', meta: 'Стабильный выпуск' },
  { value: 'preview', label: 'Preview', meta: 'Тестовые сборки' },
]
const bedrockCreationVersionOptions = computed<SelectOption[]>(() =>
  bedrockCreationVersions.value
    .filter((item) => item.edition === bedrockCreationEdition.value)
    .map((item) => ({ value: item.version, label: item.version, meta: item.installed ? 'Установлена' : 'Будет загружена' })),
)
watch(bedrockCreationEdition, () => {
  const available = bedrockCreationVersions.value.filter((item) => item.edition === bedrockCreationEdition.value)
  if (!available.some((item) => item.version === bedrockCreationVersion.value)) {
    bedrockCreationVersion.value = available[0]?.version || ''
  }
})

async function loadBedrockCreationVersions(refreshIndex = false) {
  bedrockCreationVersionsBusy.value = true
  bedrockCreationVersionsError.value = ''
  try {
    const result = await bridge.bedrockVersions('', refreshIndex)
    bedrockCreationVersions.value = result.versions.map((item) => ({ edition: item.edition, version: item.version, installed: item.installed }))
    const available = result.versions.filter((item) => item.edition === bedrockCreationEdition.value)
    if (!available.some((item) => item.version === bedrockCreationVersion.value)) {
      bedrockCreationVersion.value = available[0]?.version || ''
    }
  } catch (error) {
    bedrockCreationVersions.value = []
    bedrockCreationVersion.value = ''
    bedrockCreationVersionsError.value = errorMessage(error)
  } finally {
    bedrockCreationVersionsBusy.value = false
  }
}

function openCreateServer() {
  creationKind.value = 'server'
  bedrockCreationStatus.value = null
  createInstanceOpen.value = true
  creationName.value = ''
  serverCreation.display_name = ''
  serverCreation.install_mode = 'official'
  serverCreation.core = serverCoreOptions.value[0]?.value || 'paper'
  serverCreation.version = selectedVersion.value || '1.21.1'
  serverCreation.port = 25565
  serverMemoryValue.value = 4
  serverMemoryUnit.value = 'GiB'
  clampServerMemoryDraft()
  serverCreation.eula_accepted = false
  serverCreation.jar_name = ''
  serverCreation.jar_data = ''
  setView('instances')
  void refreshServerCoreVersions(serverCreation.core)
}

function selectOfficialServerCore() {
  serverCreation.install_mode = 'official'
  if (serverCreation.core === 'custom' || !serverCoreOptions.value.some((option) => option.value === serverCreation.core)) {
    serverCreation.core = serverCoreOptions.value[0]?.value || 'paper'
  }
  void refreshServerCoreVersions(serverCreation.core)
}

function selectCustomServerCore() {
  serverCreation.install_mode = 'custom'
  serverCreation.core = 'custom'
  serverVersionOptions.value = []
}

function closeCreateInstance() {
  createInstanceOpen.value = false
}

async function installSelected() {
  if (creationKind.value === 'bedrock') {
    await createBedrockInstance()
    return
  }
  if (creationKind.value === 'server') {
    await createServer()
    return
  }
  busy.value = true
  openActivity('Установка экземпляра', `${selectedVersion.value} · ${selectedLoaderLabel.value}`)
  const name = creationName.value.trim()
  const replaceExisting = state.instances.some((value) => value.name === name)
  if (!name) {
    busy.value = false
    finishActivity('Укажите имя экземпляра.')
    showToast('Укажите имя экземпляра')
    return
  }
  if (replaceExisting && !(await confirmDestructive(`Экземпляр «${name}» уже существует. Заменить его полностью?`, { confirmLabel: 'Заменить', detail: 'Все файлы текущего экземпляра будут перезаписаны.' }))) {
    busy.value = false
    finishActivity('Создание отменено.')
    return
  }
  try {
    const result = await bridge.install({
      version: selectedVersion.value,
      loader: selectedLoader.value,
      loader_version: selectedLoaderVersion.value || undefined,
      instance_name: name,
      replace_existing: replaceExisting,
      memory: memorySetting.value,
      java_path: effectiveJavaPath(),
    })
    if (result.instances) state.instances = result.instances
    selectedInstanceName.value = name
    createInstanceOpen.value = false
    finishActivity('Экземпляр готов к запуску.', [
      `Minecraft ${result.version_id} установлен.`,
      `Папка: ${result.instance_dir}`,
    ])
    showToast('Экземпляр установлен')
    if (settingsDraft.launch_after_install) {
      const created = state.instances.find((value) => value.name === name)
      if (created) await launchExisting(created)
    }
  } catch (error) {
    activity.logs.push(errorMessage(error))
    finishActivity(isCancelled(error) ? 'Остановлено пользователем.' : 'Установка остановлена.')
    if (!isCancelled(error)) showToast(errorMessage(error))
  } finally {
    busy.value = false
  }
}

async function createBedrockInstance() {
  let bedrockPendingVersionAfterLogin = false
  const name = creationName.value.trim()
  if (!name) {
    showToast('Укажите имя экземпляра')
    return
  }
  if (state.instances.some((instance) => instance.name.toLocaleLowerCase() === name.toLocaleLowerCase())) {
    showToast('Экземпляр с таким именем уже существует')
    return
  }
  busy.value = true
  openActivity('Создание профиля Bedrock', `${name} · отдельный профиль Microsoft Store`)
  try {
    const result = await bridge.createBedrockInstance(name, name)
    state.instances = result.instances
    selectedInstanceName.value = result.instance.name
    instanceWorkspaceFocus.value = 'overview'
    createInstanceOpen.value = false
    let versionNote = 'Версию можно выбрать в центре экземпляра.'
    if (bedrockCreationVersion.value) {
      const editionLabel = bedrockCreationEdition.value === 'preview' ? 'Preview ' : ''
      activity.detail = `Загружаю Minecraft ${editionLabel}${bedrockCreationVersion.value}…`
      activity.progress = 35
      // Установка Bedrock идёт в фоновой нити бэкенда и не блокирует
      // лаунчер; здесь мы лишь опрашиваем её состояние
      await bridge.bedrockSetupVersion(result.instance.name, bedrockCreationEdition.value, bedrockCreationVersion.value)
      let setup: { requires_login?: boolean; installed?: boolean; failed?: boolean; error_hint?: string; logs_tail?: string[]; current_version?: string; exit_code?: number } = {}
      for (;;) {
        await new Promise((resolve) => window.setTimeout(resolve, 1500))
        const status = await bridge.bedrockSetupStatus(result.instance.name)
        if (!status.started && !status.running) break
        const tail = status.logs_tail || []
        if (tail.length) {
          activity.logs = tail.slice()
          activity.detail = `Загружаю Minecraft ${editionLabel}${bedrockCreationVersion.value}… ${tail[tail.length - 1]}`
        }
        if (!status.running && status.finished) {
          if (status.cancelled) {
            versionNote = `Установка Minecraft ${editionLabel}${bedrockCreationVersion.value} остановлена. Версию можно выбрать в центре экземпляра.`
            break
          }
          if (status.error) {
            versionNote = `Не удалось загрузить Minecraft ${editionLabel}${bedrockCreationVersion.value}: ${status.error}`
            break
          }
          setup = status.result || {}
          break
        }
      }
      if (setup.requires_login) {
        versionNote = `Версия ${editionLabel}${bedrockCreationVersion.value} не установлена: нужен вход в Microsoft. В центре Bedrock нажмите «Подключить Microsoft Store» — после входа выбранная версия доустановится сама.`
        bedrockPendingVersionAfterLogin = true
      } else if (setup.failed || !setup.installed) {
        const hint = setup.error_hint || 'проверьте подключение и лицензию Microsoft'
        versionNote = `Не удалось загрузить Minecraft ${editionLabel}${bedrockCreationVersion.value}: ${hint}. Версию можно выбрать в центре экземпляра.`
      } else {
        versionNote = `Minecraft ${setup.current_version || bedrockCreationVersion.value} готов к запуску.`
      }
    }
    setView('instance-settings')
    await nextTick()
    await bedrockWorkspaceRef.value?.ensureLoaded()
    if (bedrockPendingVersionAfterLogin && bedrockCreationVersion.value) {
      bedrockWorkspaceRef.value?.applyPendingVersion(bedrockCreationEdition.value, bedrockCreationVersion.value)
    }
    finishActivity('Профиль Bedrock создан.', [`Экземпляр: ${result.instance.name}`, versionNote])
    showToast(bedrockPendingVersionAfterLogin ? 'Профиль создан — нужен вход в Microsoft для установки версии' : 'Профиль Bedrock создан')
  } catch (error) {
    activity.logs.push(errorMessage(error))
    finishActivity('Не удалось создать профиль Bedrock.')
    showToast(errorMessage(error))
  } finally {
    busy.value = false
  }
}

async function loadServerCoreFile(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  try {
    await selectServerCoreFile(file)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    input.value = ''
  }
}

async function selectServerCoreFile(file: File) {
  if (!file.name.toLowerCase().endsWith('.jar')) throw new Error('Загрузите файл ядра с расширением .jar')
  if (file.size > 512 * 1024 * 1024) throw new Error('Файл ядра превышает лимит 512 MiB')
  selectCustomServerCore()
  serverCreation.jar_name = file.name
  serverCreation.jar_data = await encodeBlob(file)
  showToast(`Выбрано ядро: ${file.name}`)
}

function onServerCoreDrop(event: DragEvent) {
  serverCoreDragActive.value = false
  const file = event.dataTransfer?.files[0]
  if (file) void selectServerCoreFile(file).catch((error) => showToast(errorMessage(error)))
}

async function downloadOfficialServerCore(name: string, core: string, version: string, build = '') {
  try {
    return (await bridge.downloadServerCore(name, core, version, build)).download
  } catch (error) {
    if (!errorMessage(error).includes('[unverified-download]')) throw error
    if (!(await confirmDialog('Источник ядра не дал доверенный хеш. Скачать файл и показать его SHA-256 после загрузки?', { danger: true, confirmLabel: 'Скачать' }))) {
      throw new Error('Загрузка ядра отменена')
    }
    return (await bridge.downloadServerCore(name, core, version, build, true)).download
  }
}

async function createServer() {
  const name = creationName.value.trim()
  if (!name) {
    showToast('Укажите имя сервера')
    return
  }
  if (!serverCreation.eula_accepted) {
    showToast('Примите EULA Minecraft перед созданием сервера')
    return
  }
  if (serverCreation.install_mode === 'official' && !serverCreation.version) {
    showToast('Выберите версию Minecraft для выбранного ядра')
    return
  }
  if (serverCreation.install_mode === 'custom' && !serverCreation.jar_data) {
    showToast('Выберите собственный server.jar или переключитесь на официальное ядро')
    return
  }
  if (state.instances.some((value) => value.name === name)) {
    showToast('Это имя уже занято. Выберите другое: создание сервера не заменяет существующие экземпляры.')
    return
  }
  busy.value = true
  openActivity('Создание сервера', `${serverCreation.core} · ${serverCreation.version}`)
  try {
    const result = await bridge.createServer({
      name,
      display_name: serverCreation.display_name || name,
      core: serverCreation.install_mode === 'official' ? serverCreation.core : 'custom',
      version: serverCreation.version,
      port: Number(serverCreation.port),
      memory: serverMemorySetting.value,
      eula_accepted: serverCreation.eula_accepted,
      custom_jar_name: serverCreation.jar_name,
      custom_jar_data: serverCreation.jar_data || undefined,
    })
    state.instances = result.instances
    selectedInstanceName.value = name
    createInstanceOpen.value = false
    const shouldDownloadCore = serverCreation.install_mode === 'official'
    if (shouldDownloadCore) {
      try {
        const download = await downloadOfficialServerCore(name, serverCreation.core, serverCreation.version)
        finishActivity('Сервер создан, ядро установлено.', [
          `Ядро: ${serverCreation.core} ${serverCreation.version}`,
          `SHA-256: ${download.sha256 || 'неизвестен'}`,
        ])
        showToast(`Сервер создан · ${serverCreation.core} установлен`)
      } catch (error) {
        finishActivity('Сервер создан, но ядро не установлено.', [errorMessage(error), 'Откройте server workspace и повторите загрузку.'])
        showToast(`Сервер создан, но ядро не скачано: ${errorMessage(error)}`)
      }
    } else {
      finishActivity('Сервер создан. Загрузите server.jar, если он ещё не выбран.', [`Ядро: ${serverCreation.core}`, `Порт: ${serverCreation.port}`])
      showToast('Сервер создан')
    }
  } catch (error) {
    finishActivity(errorMessage(error))
    showToast(errorMessage(error))
  } finally {
    busy.value = false
  }
}

async function createOfflineAccount(): Promise<boolean> {
  if (!accountName.value.trim() || busy.value) return false
  busy.value = true
  try {
    const result = await bridge.createOfflineAccount(accountName.value)
    if (!result.account) return false
    state.accounts = [...state.accounts.filter((account) => account.account_id !== result.account.account_id), result.account]
    selectedAccount.value = result.account.account_id
    settingsDraft.default_account_id = result.account.account_id
    await refreshBootstrap()
    selectedAccount.value = result.account.account_id
    settingsDraft.default_account_id = result.account.account_id
    showToast('Offline-аккаунт создан')
    return true
  } catch (error) {
    if (firstRunPhase.value === 'account') firstRunError.value = errorMessage(error)
    showToast(errorMessage(error))
    return false
  } finally {
    busy.value = false
  }
}

async function loginMicrosoft(): Promise<boolean> {
  if (busy.value || !state.settings.has_client_id) return false
  busy.value = true
  openActivity('Вход через Microsoft', 'Откроется device-code авторизация.')
  startMicrosoftStatusPolling()
  try {
    const result = await bridge.loginMicrosoft()
    finishActivity('Microsoft-аккаунт подключён.', result.messages)
    await refreshBootstrap()
    selectedAccount.value = result.account.account_id
    settingsDraft.default_account_id = result.account.account_id
    showToast(`Добро пожаловать, ${result.account.display_name}`)
    return true
  } catch (error) {
    activity.logs.push(errorMessage(error))
    finishActivity(isCancelled(error) ? 'Вход остановлен.' : 'Вход не завершён.')
    if (firstRunPhase.value === 'account') firstRunError.value = isCancelled(error) ? 'Вход остановлен. Повторите попытку или создайте локальный профиль.' : errorMessage(error)
    if (!isCancelled(error)) showToast(errorMessage(error))
    return false
  } finally {
    stopMicrosoftStatusPolling()
    busy.value = false
  }
}

function setFirstRunPhase(phase: FirstRunPhase) {
  firstRunError.value = ''
  firstRunPhase.value = phase
}

// Шаг языка в визарде: применяем сразу, сохраняем в настройки и
// показываем приветствие уже на выбранном языке.
function pickFirstRunLanguage(next: Locale): void {
  settingsDraft.language = next
  setLocale(next)
  try {
    window.localStorage.setItem('nivora-language-picked', next)
  } catch {
    // приватный режим: выбор переживёт сессию через settings.json
  }
  void saveSettings()
  setFirstRunPhase('welcome')
}

function selectFirstRunAccount(accountId: string) {
  selectedAccount.value = accountId
  settingsDraft.default_account_id = accountId
  firstRunError.value = ''
}

async function addFirstRunOfflineAccount(name: string) {
  if (!name.trim() || busy.value) return
  firstRunError.value = ''
  accountName.value = name.trim()
  if (await createOfflineAccount()) firstRunPhase.value = 'basics'
}

async function addFirstRunMicrosoftAccount() {
  if (busy.value) return
  firstRunError.value = ''
  if (await loginMicrosoft()) firstRunPhase.value = 'basics'
}

async function saveFirstRunBasics(values: { gameDirectory: string; memory: string; theme: ThemeMode; graphicsMode: string }) {
  if (busy.value) return
  firstRunError.value = ''
  if (!values.gameDirectory.trim()) {
    firstRunError.value = 'Укажите папку, в которой будут храниться файлы игры.'
    return
  }
  settingsDraft.game_dir = values.gameDirectory.trim()
  settingsDraft.default_account_id = selectedAccount.value
  appearanceDraft.theme = values.theme
  settingsDraft.graphics_mode = (['auto', 'nvidia', 'mesa', 'software'] as const).find((mode) => mode === values.graphicsMode) ?? 'auto'
  syncMemoryDraft(values.memory)
  if (await saveSettings()) firstRunPhase.value = 'finish'
  else firstRunError.value = settingsSaveMessage.value || 'Не удалось сохранить настройки. Проверьте параметры и повторите попытку.'
}

function persistFirstRunState(next: FirstRunExperienceState) {
  firstRunState.value = next
  if (!writeFirstRunExperience(next)) showToast('Настройки первого запуска не удалось сохранить в локальном хранилище.')
}

function finishFirstRunSetup() {
  persistFirstRunState({ setupCompleted: true, tourCompleted: false })
  firstRunError.value = ''
  firstRunPhase.value = 'tour'
}

// «Начать без настройки» — закрываем мастер и экскурсию, всё остаётся
// на значениях по умолчанию; повторить можно из настроек.
function skipFirstRunSetup() {
  persistFirstRunState({ setupCompleted: true, tourCompleted: true })
  firstRunError.value = ''
  firstRunPhase.value = null
}

function finishLauncherTour() {
  persistFirstRunState({ setupCompleted: true, tourCompleted: true })
  firstRunPhase.value = null
}

function replayLauncherTour() {
  firstRunReady.value = true
  firstRunError.value = ''
  firstRunPhase.value = 'tour'
}

function setProfileArguments(profile: LauncherProfile, key: 'jvm_arguments' | 'game_arguments', event: Event) {
  const value = (event.target as HTMLTextAreaElement).value
  profile[key] = value.split('\n').map((item) => item.trim()).filter(Boolean)
}

function addLauncherProfile() {
  let index = settingsDraft.launcher_profiles.length + 1
  const profile: LauncherProfile = {
    ...defaultLauncherProfile,
    id: `profile-${index}`,
    name: `Профиль ${index}`,
    jvm_arguments: [],
    game_arguments: [],
  }
  while (settingsDraft.launcher_profiles.some((item) => item.id === profile.id)) {
    index += 1
    profile.id = `profile-${index}`
    profile.name = `Профиль ${index}`
  }
  settingsDraft.launcher_profiles.push(profile)
  settingsDraft.active_launcher_profile_id = profile.id
}

function removeLauncherProfile(profileId: string) {
  if (settingsDraft.launcher_profiles.length <= 1) {
    showToast('Нельзя удалить последний профиль запуска')
    return
  }
  settingsDraft.launcher_profiles = settingsDraft.launcher_profiles.filter((profile) => profile.id !== profileId)
  if (settingsDraft.active_launcher_profile_id === profileId) settingsDraft.active_launcher_profile_id = settingsDraft.launcher_profiles[0].id
}

function settingsValidationError(): string {
  const integerRules: Array<[string, number, number, number]> = [
    ['Параллельные загрузки', settingsDraft.max_downloads, 1, 32],
    ['Обновление metadata', settingsDraft.metadata_refresh_hours, 1, 168],
    ['Timeout загрузки', settingsDraft.download_timeout_seconds, 5, 120],
    ['Повторные попытки', settingsDraft.download_retries, 1, 6],
    ['Хранение логов', settingsDraft.keep_logs_days, 0, 365],
  ]
  const invalid = integerRules.find(([, value, min, max]) => !Number.isInteger(value) || value < min || value > max)
  if (invalid) return `${invalid[0]}: укажите целое число от ${invalid[2]} до ${invalid[3]}.`
  for (const profile of settingsDraft.launcher_profiles) {
    if (!/^auto$|^\d+(?:\.\d+)?(?:KiB|MiB|GiB)$/i.test(profile.memory.trim())) return `Профиль «${profile.name}»: память должна быть auto, KiB, MiB или GiB.`
    if (!Number.isInteger(profile.window_width) || profile.window_width < 320 || profile.window_width > 8192) return `Профиль «${profile.name}»: ширина окна должна быть от 320 до 8192.`
    if (!Number.isInteger(profile.window_height) || profile.window_height < 240 || profile.window_height > 8192) return `Профиль «${profile.name}»: высота окна должна быть от 240 до 8192.`
  }
  return ''
}

function scrollSettingsSection(id: string) {
  document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
}

async function saveSettings(): Promise<boolean> {
  const validationError = settingsValidationError()
  if (validationError) {
    settingsSaveState.value = 'error'
    settingsSaveMessage.value = validationError
    showToast(validationError)
    return false
  }
  commitWallpaperBlur()
  settingsSaveState.value = 'saving'
  settingsSaveMessage.value = 'Сохраняю изменения…'
  busy.value = true
  try {
    const values: Record<string, unknown> = {
      game_dir: settingsDraft.game_dir,
      java_path: effectiveJavaPath(),
      memory: memorySetting.value,
      max_downloads: settingsDraft.max_downloads,
      startup_view: settingsDraft.startup_view,
      last_view: settingsDraft.last_view,
      remember_last_view: settingsDraft.remember_last_view,
      default_version_filter: settingsDraft.default_version_filter,
      default_loader: settingsDraft.default_loader,
      default_account_id: settingsDraft.default_account_id,
      open_last_instance: settingsDraft.open_last_instance,
      launch_after_install: settingsDraft.launch_after_install,
      auto_refresh_metadata: settingsDraft.auto_refresh_metadata,
      metadata_refresh_hours: settingsDraft.metadata_refresh_hours,
      download_timeout_seconds: settingsDraft.download_timeout_seconds,
      download_retries: settingsDraft.download_retries,
      confirm_destructive_actions: settingsDraft.confirm_destructive_actions,
      show_activity_panel: settingsDraft.show_activity_panel,
      show_toasts: settingsDraft.show_toasts,
       minimize_on_launch: settingsDraft.minimize_on_launch,
        language: settingsDraft.language ?? null,
        font_family: settingsDraft.font_family,
      keep_logs_days: settingsDraft.keep_logs_days,
      telemetry: settingsDraft.telemetry,
      telemetry_endpoint: settingsDraft.telemetry_endpoint.trim(),
       graphics_mode: settingsDraft.graphics_mode,
       keyboard_shortcuts: {
        ...settingsDraft.keyboard_shortcuts,
        // общий черновик тоже мог прийти из старых свободных полей —
        // мусор в значении молча отключал клавишу
        open_settings: normalizeHotkey(settingsDraft.keyboard_shortcuts.open_settings, 'Ctrl+,'),
        refresh: normalizeHotkey(settingsDraft.keyboard_shortcuts.refresh, 'Ctrl+R'),
        new_instance: normalizeHotkey(settingsDraft.keyboard_shortcuts.new_instance, 'Ctrl+N'),
        focus_search: normalizeHotkey(settingsDraft.keyboard_shortcuts.focus_search, 'Ctrl+K'),
        toggle_activity: normalizeHotkey(settingsDraft.keyboard_shortcuts.toggle_activity, 'Ctrl+J'),
      },
       overlay_settings: {
        ...settingsDraft.overlay_settings,
        // черновик мог остаться с мусорной комбинацией со старых версий —
        // нормализуем до сохранения, иначе оверлей молча не откроется
        hotkey_overlay: normalizeHotkey(settingsDraft.overlay_settings.hotkey_overlay, 'Shift+Tab'),
        hotkey_recording: normalizeHotkey(settingsDraft.overlay_settings.hotkey_recording, 'F7'),
      },
       backup_schedule: { ...settingsDraft.backup_schedule },
       launcher_profiles: settingsDraft.launcher_profiles.map((profile) => ({ ...profile, jvm_arguments: [...profile.jvm_arguments], game_arguments: [...profile.game_arguments] })),
       active_launcher_profile_id: settingsDraft.active_launcher_profile_id,
       http_proxy: settingsDraft.http_proxy.trim(),
      developer_mode: settingsDraft.developer_mode,
      ...appearanceDraft,
      page_themes: { ...appearanceDraft.page_themes },
      page_layouts: { ...appearanceDraft.page_layouts },
      widget_visibility: { ...appearanceDraft.widget_visibility },
      color_tokens: { ...appearanceDraft.color_tokens },
      radius_tokens: { ...appearanceDraft.radius_tokens },
      login_source: settingsDraft.login_source,
      login_api_url: settingsDraft.login_api_url.trim(),
      catalog_source: settingsDraft.catalog_source,
      catalog_api_url: settingsDraft.catalog_api_url.trim(),
      data_source: settingsDraft.data_source,
      data_api_url: settingsDraft.data_api_url.trim(),
      resource_catalogs: settingsDraft.resource_catalogs.map((catalog) => ({ ...catalog })),
      active_catalog_id: settingsDraft.active_catalog_id,
      plugin_catalogs: settingsDraft.plugin_catalogs.map((catalog) => ({ ...catalog })),
      active_plugin_catalog_id: settingsDraft.active_plugin_catalog_id,
    }
    const currentSettings = state.settings as unknown as Record<string, unknown>
    const changedKeys = Object.keys(values).filter((key) => JSON.stringify(values[key]) !== JSON.stringify(currentSettings[key]))
    const result = await bridge.saveSettings(values)
    applyState({ instances: state.instances, accounts: state.accounts, settings: result.settings, system: state.system })
    if (changedKeys.length) void broadcastPluginEvent('settings.changed', { keys: changedKeys })
    settingsSaveState.value = 'saved'
    settingsSaveMessage.value = 'Изменения сохранены'
    showToast('Настройки сохранены')
    return true
  } catch (error) {
    settingsSaveState.value = 'error'
    settingsSaveMessage.value = errorMessage(error)
    showToast(errorMessage(error))
    return false
  } finally {
    busy.value = false
  }
}

function addResourceCatalog() {
  let index = settingsDraft.resource_catalogs.length + 1
  const catalog: ResourceCatalog = {
    id: `catalog-${index}`,
    name: `Каталог ${index}`,
    kind: 'custom',
    api_url: 'https://example.invalid/modrinth/v2',
  }
  while (settingsDraft.resource_catalogs.some((item) => item.id === catalog.id)) catalog.id = `catalog-${++index}`
  settingsDraft.resource_catalogs.push(catalog)
  settingsDraft.active_catalog_id = catalog.id
  activeCatalogId.value = catalog.id
}

function removeResourceCatalog(catalogId: string) {
  if (settingsDraft.resource_catalogs.length <= 1) {
    showToast('Нельзя удалить последний каталог')
    return
  }
  settingsDraft.resource_catalogs = settingsDraft.resource_catalogs.filter((catalog) => catalog.id !== catalogId)
  if (settingsDraft.active_catalog_id === catalogId) {
    settingsDraft.active_catalog_id = settingsDraft.resource_catalogs[0].id
    activeCatalogId.value = settingsDraft.active_catalog_id
  }
}

function addPluginCatalog() {
  let index = settingsDraft.plugin_catalogs.length + 1
  const catalog = { id: `plugin-catalog-${index}`, name: `Plugin catalog ${index}`, kind: 'custom', api_url: 'https://example.invalid/plugins', enabled: true }
  while (settingsDraft.plugin_catalogs.some((item) => item.id === catalog.id)) catalog.id = `plugin-catalog-${++index}`
  settingsDraft.plugin_catalogs.push(catalog)
  settingsDraft.active_plugin_catalog_id = catalog.id
}

function removePluginCatalog(catalogId: string) {
  if (settingsDraft.plugin_catalogs.length <= 1) {
    showToast('Нельзя удалить последний каталог плагинов')
    return
  }
  settingsDraft.plugin_catalogs = settingsDraft.plugin_catalogs.filter((catalog) => catalog.id !== catalogId)
  if (settingsDraft.active_plugin_catalog_id === catalogId) settingsDraft.active_plugin_catalog_id = settingsDraft.plugin_catalogs[0].id
}

async function selectActiveCatalog(catalogId: string) {
  const pluginCatalogView = currentView.value === 'mods' && modProjectType.value === 'plugin'
  const allowed = pluginCatalogView
    ? settingsDraft.plugin_catalogs.filter((catalog) => catalog.enabled !== false)
    : currentView.value === 'modpacks'
      ? settingsDraft.resource_catalogs.filter((catalog) => ['modrinth', 'ftb', 'technic'].includes(catalog.kind))
      : currentView.value === 'mods'
        ? settingsDraft.resource_catalogs.filter((catalog) => !['ftb', 'technic'].includes(catalog.kind))
        : settingsDraft.resource_catalogs
  if (!allowed.some((catalog) => catalog.id === catalogId)) {
    ensureCatalogForView(currentView.value)
    showToast('Этот каталог не поддерживает текущий раздел')
    return
  }
  const previous = activeCatalogId.value
  activeCatalogId.value = catalogId
  if (pluginCatalogView) settingsDraft.active_plugin_catalog_id = catalogId
  else settingsDraft.active_catalog_id = catalogId
  try {
    const result = await bridge.saveSettings(pluginCatalogView ? { active_plugin_catalog_id: catalogId } : { active_catalog_id: catalogId })
    state.settings = result.settings
    const savedCatalogId = pluginCatalogView ? result.settings.active_plugin_catalog_id : result.settings.active_catalog_id
    if (pluginCatalogView) settingsDraft.active_plugin_catalog_id = savedCatalogId
    else settingsDraft.active_catalog_id = savedCatalogId
    activeCatalogId.value = savedCatalogId
    mergeSavedSettingsBaseline(pluginCatalogView ? { active_plugin_catalog_id: savedCatalogId } : { active_catalog_id: savedCatalogId })
    void broadcastPluginEvent('settings.changed', { keys: [pluginCatalogView ? 'active_plugin_catalog_id' : 'active_catalog_id'] })
    showToast('Каталог выбран')
  } catch (error) {
    activeCatalogId.value = previous
    if (pluginCatalogView) settingsDraft.active_plugin_catalog_id = previous
    else settingsDraft.active_catalog_id = previous
    refreshSettingsSaveState()
    showToast(errorMessage(error))
  }
}

function selectCatalogInstance(name: string) {
  selectedInstanceName.value = name
  if (selectedInstance.value?.kind === 'server') catalogServerCore.value = selectedInstance.value.server_core || catalogServerCore.value
  modResults.value = []
  modError.value = ''
  if (currentView.value === 'mods') void searchMods()
}

async function openGameFolder() {
  try {
    await bridge.openFolder()
    showToast('Папка игры открыта')
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function loadGraphicsDiagnostics() {
  try {
    const result = await bridge.graphicsDiagnostics()
    const glx = result.glxinfo as { output?: string } | undefined
    const nvidia = result.nvidia_smi as { output?: string } | undefined
    graphicsReport.value = [
      `mode=${String(result.mode || 'auto')} · session=${String(result.session || 'unknown')} · display=${String(result.display || 'none')}`,
      glx?.output ? `glxinfo -B: ${glx.output.split('\n').slice(0, 4).join(' · ')}` : 'glxinfo -B: недоступен',
      nvidia?.output ? `nvidia-smi: ${nvidia.output.split('\n').slice(0, 2).join(' · ')}` : 'nvidia-smi: недоступен',
    ].join('\n')
  } catch (error) {
    graphicsReport.value = errorMessage(error)
  }
}

async function launchBedrockExisting(instance: InstanceInfo) {
  selectedInstanceName.value = instance.name
  instanceWorkspaceFocus.value = 'overview'
  setView('instance-settings')
  await nextTick()
  await bedrockWorkspaceRef.value?.ensureLoaded()
  busy.value = true
  openActivity('Запуск Minecraft Bedrock', `${instance.display_name || instance.name} · лицензия Microsoft Store`)
  startActivityLogPolling(instance.name)
  try {
    await minimizeForLaunch()
    attachOverlayForLaunch(instance.name)
    const result = await bridge.launchInstance(instance.name)
    finishActivity(result.exit_code === 0 ? 'Bedrock завершил работу.' : 'Minecraft Bedrock завершил работу с ошибкой.', result.logs)
    showToast(result.exit_code === 0 ? 'Игра завершила работу' : `Код выхода: ${result.exit_code}`)
  } catch (error) {
    activity.logs.push(errorMessage(error))
    finishActivity(isCancelled(error) ? 'Остановлено пользователем.' : 'Не удалось запустить Bedrock.')
    if (!isCancelled(error)) {
      showToast(errorMessage(error))
      // Если не хватило привязки аккаунта Microsoft — статус в карточке
      // лицензии обязан подсветиться сразу, а не после ручного захода.
      if (errorMessage(error).includes('Подключить аккаунт')) {
        void bedrockWorkspaceRef.value?.refreshAccountStatus?.()
      }
    }
  } finally {
    stopActivityLogPolling()
    busy.value = false
    await refreshBootstrap()
  }
}

async function launchExisting(instance: InstanceInfo) {
  if (instance.running) {
    selectedInstanceName.value = instance.name
    showToast(`«${instance.display_name || instance.name}» уже запущен`)
    return
  }
  selectedInstanceName.value = instance.name
  if (instance.game_edition === 'bedrock') {
    await launchBedrockExisting(instance)
    return
  }
  selectedVersion.value = (instance.kind === 'server' ? instance.server_core_version : instance.minecraft_version || instance.version_id) || selectedVersion.value
  selectedLoader.value = instance.loader
    if (instance.kind === 'server') {
      instanceWorkspaceFocus.value = 'overview'
      setView('instance-settings')
      await nextTick()
      try {
        const status = await bridge.startServer(instance.name)
        showToast(status.running ? `Сервер «${instance.display_name || instance.name}» запускается — вывод на главной странице сервера` : status.start_error || 'Процесс сервера завершился; проверьте консоль и код завершения')
        await refreshBootstrap()
      } catch (error) {
      showToast(errorMessage(error))
    }
    return
  }
  instanceWorkspaceFocus.value = 'console'
  if (currentView.value !== 'instance-settings') {
    setView('instance-settings')
    await nextTick()
  }
  await instanceWorkspaceRef.value?.ensureLoaded?.()
  busy.value = true
  openActivity('Консоль запуска', `${instance.name} · ${instance.minecraft_version || instance.version_id || 'версия не установлена'}`)
  startActivityLogPolling(instance.name)
  const eventInstance = pluginInstanceRef(instance)!
  let launchRequested = false
  try {
    await minimizeForLaunch()
    const profileAccount = activeLauncherProfile.value?.account_id || 'auto'
    await notifyPluginBeforeLaunch(eventInstance)
    launchRequested = true
    attachOverlayForLaunch(instance.name)
    const result = await bridge.launchInstance(instance.name, {
      account: profileAccount !== 'auto' ? profileAccount : selectedAccount.value,
      profile_id: activeLauncherProfile.value?.id,
    })
    await notifyPluginAfterExit(eventInstance, result.exit_code)
    if (result.exit_code !== 0) await notifyPluginCrash(eventInstance, 'Minecraft exited with a non-zero status.', result.exit_code)
    if (result.crash) lastLaunchCrash.value = result.crash
    finishActivity(result.exit_code === 0 ? 'Игра завершила работу.' : 'Игра завершилась с ошибкой.', result.logs)
    if (result.exit_code !== 0) {
      instanceWorkspaceFocus.value = 'overview'
      setView('instance-settings')
    }
    showToast(result.exit_code === 0 ? 'Игра завершила работу' : `Код выхода: ${result.exit_code}`)
  } catch (error) {
    activity.logs.push(errorMessage(error))
    if (launchRequested && !isCancelled(error)) await notifyPluginCrash(eventInstance, errorMessage(error))
    finishActivity(isCancelled(error) ? 'Остановлено пользователем.' : 'Не удалось запустить игру.')
    if (!isCancelled(error)) showToast(errorMessage(error))
  } finally {
    stopActivityLogPolling()
    busy.value = false
    await refreshBootstrap()
  }
}

async function stopExisting(instance: InstanceInfo) {
  if (!instance.running || (busy.value && instance.game_edition !== 'bedrock')) return
  selectedInstanceName.value = instance.name
  busy.value = true
  try {
    if (instance.kind === 'server') {
      if (!(await confirmDestructive(`Остановить сервер «${instance.display_name || instance.name}»?`, { confirmLabel: 'Остановить' }))) return
      await bridge.stopServer(instance.name)
    } else if (instance.game_edition === 'bedrock') {
      await bridge.bedrockStop(instance.name)
    } else {
      await nextTick()
      if (instanceWorkspaceRef.value) await instanceWorkspaceRef.value.stop()
      else await bridge.cancelActiveOperations()
    }
    await refreshBootstrap()
    showToast(`«${instance.display_name || instance.name}» останавливается`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    busy.value = false
  }
}

async function openInstance(instance: InstanceInfo) {
  instanceWorkspaceFocus.value = 'settings'
  selectedInstanceName.value = instance.name
  if (instance.game_edition === 'bedrock') {
    instanceWorkspaceFocus.value = 'overview'
  } else {
    selectedVersion.value = (instance.kind === 'server' ? instance.server_core_version : instance.minecraft_version || instance.version_id) || selectedVersion.value
    selectedLoader.value = instance.loader
    if (instance.kind === 'server') catalogServerCore.value = instance.server_core || catalogServerCore.value
  }
  setView('instance-settings')
}

async function deleteInstance(instance: InstanceInfo) {
  if (!(await confirmDestructive(`Удалить экземпляр «${instance.name}»?`))) return
  try {
    const result = await bridge.deleteInstance(instance.name)
    state.instances = result.instances
    if (selectedInstanceName.value === instance.name) selectedInstanceName.value = ''
    showToast('Экземпляр удалён')
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function createInstanceGroup() {
  mobileMenuOpen.value = false
  const name = (await promptDialog('Название новой группы', { placeholder: 'Например, Выживание', confirmLabel: 'Создать группу' }))?.trim().slice(0, 40)
  if (!name) return
  if (state.settings.instance_groups.includes(name)) {
    showToast(`Группа «${name}» уже существует`)
    return
  }
  if (groupActionBusy.value) return
  groupActionBusy.value = true
  try {
    const result = await bridge.createInstanceGroup(name)
    state.settings.instance_groups = result.groups || [...state.settings.instance_groups, name]
    instanceGroupFilter.value = `group:${name}`
    showToast(`Группа «${name}» создана`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    groupActionBusy.value = false
  }
}

async function renameInstanceGroup(group: string) {
  const name = (await promptDialog(`Новое название группы «${group}»`, { placeholder: group, initialValue: group, confirmLabel: 'Переименовать' }))?.trim().slice(0, 40)
  if (!name || name === group) return
  if (state.settings.instance_groups.includes(name)) {
    showToast(`Группа «${name}» уже существует`)
    return
  }
  if (groupActionBusy.value) return
  groupActionBusy.value = true
  try {
    const result = await bridge.renameInstanceGroup(group, name)
    state.settings.instance_groups = result.groups || state.settings.instance_groups.map((value) => (value === group ? name : value))
    state.instances = state.instances.map((instance) => (instance.group === group ? { ...instance, group: name } : instance))
    state.settings.pinned_instance_groups = state.settings.pinned_instance_groups.map((value) => (value === group ? name : value))
    if (instanceGroupFilter.value === `group:${group}`) instanceGroupFilter.value = `group:${name}`
    showToast(`Группа переименована в «${name}»`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    groupActionBusy.value = false
  }
}

async function deleteInstanceGroup(group: string) {
  if (state.instances.some((instance) => instance.group === group)) {
    showToast('Сначала переместите все экземпляры из этой группы')
    return
  }
  if (!(await confirmDestructive(`Удалить пустую группу «${group}»?`)) || groupActionBusy.value) return
  groupActionBusy.value = true
  try {
    const result = await bridge.deleteInstanceGroup(group)
    state.settings.instance_groups = result.groups || state.settings.instance_groups.filter((name) => name !== group)
    state.settings.pinned_instance_groups = state.settings.pinned_instance_groups.filter((name) => name !== group)
    if (instanceGroupFilter.value === `group:${group}`) instanceGroupFilter.value = 'all'
    showToast(`Группа «${group}» удалена`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    groupActionBusy.value = false
  }
}

async function toggleInstanceGroupPin(group: string) {
  if (groupActionBusy.value) return
  const current = state.settings.pinned_instance_groups || []
  const pinned = current.includes(group)
  if (!pinned && current.length >= 3) {
    showToast('Можно закрепить не более трёх групп')
    return
  }
  const next = pinned ? current.filter((name) => name !== group) : [group, ...current]
  groupActionBusy.value = true
  state.settings.pinned_instance_groups = next
  try {
    const result = await bridge.saveSettings({ pinned_instance_groups: next })
    state.settings.pinned_instance_groups = result.settings.pinned_instance_groups || next
    void broadcastPluginEvent('settings.changed', { keys: ['pinned_instance_groups'] })
    showToast(pinned ? `Группа «${group}» откреплена` : `Группа «${group}» закреплена`)
  } catch (error) {
    state.settings.pinned_instance_groups = current
    showToast(errorMessage(error))
  } finally {
    groupActionBusy.value = false
  }
}

async function moveInstanceToGroup(instance: InstanceInfo, group: string) {
  if ((instance.group || '') === group || groupActionBusy.value) return
  groupActionBusy.value = true
  try {
    const result = await bridge.updateInstanceProfile({
      name: instance.name,
      display_name: instance.display_name || instance.name,
      description: instance.description || '',
      group,
      clear_icon: false,
      clear_banner: false,
    })
    state.instances = state.instances.map((current) => current.name === result.instance.name ? result.instance : current)
    state.settings.instance_groups = result.groups || state.settings.instance_groups
    showToast(group ? `«${instance.display_name || instance.name}» перемещён в «${group}»` : `«${instance.display_name || instance.name}» перемещён в «Без группы»`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    groupActionBusy.value = false
  }
}

async function duplicateInstance(instance: InstanceInfo) {
  const target = await promptDialog('Новое имя экземпляра', { initialValue: `${instance.name}-copy`, confirmLabel: 'Дублировать' })
  if (!target) return
  try {
    await bridge.duplicateInstance(instance.name, target)
    await refreshBootstrap()
    showToast('Экземпляр продублирован')
  } catch (error) {
    showToast(errorMessage(error))
  }
}

function externalImportChoice(candidate: ExternalInstanceCandidate): { value: string; label: string; description: string } {
  const version = candidate.minecraft_version || candidate.version_id || 'версия не определена'
  const loader = candidate.loader && candidate.loader !== 'vanilla' ? candidate.loader : 'vanilla'
  return {
    value: candidate.source_path,
    label: `${candidate.display_name} · ${candidate.launcher}`,
    description: `${version} · ${loader}`,
  }
}

async function importExternalInstances() {
  if (externalImportBusy.value) return
  externalImportBusy.value = true
  try {
    const result = await bridge.externalInstances()
    const candidates = result.candidates || []
    if (!candidates.length) {
      showToast('Поддерживаемые экземпляры Prism/MultiMC/CurseForge/официального лаунчера не найдены')
      return
    }
    const sourcePath = await chooseDialog(
      'Выберите источник импорта',
      candidates.map(externalImportChoice),
      {
        title: 'Импорт из других лаунчеров',
        detail: 'Nivora Launcher найдёт стандартные каталоги Prism Launcher, MultiMC, CurseForge и официального Minecraft Launcher.',
        confirmLabel: 'Далее',
      },
    )
    if (!sourcePath) return
    const candidate = candidates.find((item) => item.source_path === sourcePath)
    const initialName = candidate?.suggested_name || candidate?.display_name || 'imported-instance'
    const targetName = (await promptDialog('Имя нового импортированного экземпляра', {
      initialValue: initialName,
      placeholder: initialName,
      confirmLabel: 'Импортировать',
    }))?.trim()
    if (!targetName) return
    const imported = await bridge.importExternalInstance(sourcePath, targetName)
    state.instances = imported.instances || [...state.instances.filter((item) => item.name !== imported.instance.name), imported.instance]
    selectedInstanceName.value = imported.instance.name
    showToast(`Импортирован «${imported.instance.display_name || imported.instance.name}»`)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    externalImportBusy.value = false
  }
}

async function openCatalogDetails(project: ModResult) {
  const projectId = project.project_id || project.id
  if (!projectId) return
  const projectType: ProjectType = currentView.value === 'modpacks' ? 'modpack' : modProjectType.value
  const requestId = ++catalogDetailsRequestId
  catalogDetailsType.value = projectType
  catalogDetailsProject.value = project
  catalogDetails.value = { ...project }
  catalogDetailsVersions.value = []
  catalogDetailsError.value = ''
  catalogDetailsLoading.value = true
  catalogDetailsOpen.value = true
  try {
    const result = await bridge.modsDetails({
      project_id: projectId,
      instance_name: selectedInstance.value?.name,
      game_version: modInstanceVersion.value || selectedVersion.value,
      loader: selectedInstance.value?.loader || selectedLoader.value,
      server_core: selectedInstance.value?.server_core || catalogServerCore.value,
      project_type: projectType,
      catalog_id: activeCatalogId.value,
    })
    if (requestId !== catalogDetailsRequestId) return
    catalogDetails.value = result.details
    catalogDetailsVersions.value = result.compatible_versions
  } catch (error) {
    if (requestId === catalogDetailsRequestId) catalogDetailsError.value = errorMessage(error)
  } finally {
    if (requestId === catalogDetailsRequestId) catalogDetailsLoading.value = false
  }
}

function closeCatalogDetails() {
  catalogDetailsRequestId += 1
  catalogDetailsOpen.value = false
  catalogDetailsLoading.value = false
}

async function installFromCatalogDetails(versionId: string) {
  const project = catalogDetailsProject.value
  if (!project) return
  if (versionId) {
    project.version_id = versionId
    const version = catalogDetailsVersions.value.find((item) => item.id === versionId)
    if (version) project.version = version.version_number || version.name
  }
  closeCatalogDetails()
  await installMod(project)
}

function catalogTargetInstance() {
  const instance = selectedInstance.value
  return instance && instance.kind === 'client' && instance.game_edition !== 'bedrock' ? instance : null
}

async function searchMods() {
  ensureCatalogForView(currentView.value)
  ensureCompatibleInstanceForCatalog()
  const requestId = ++modSearchRequestId
  const projectType: ProjectType = currentView.value === 'modpacks' ? 'modpack' : modProjectType.value === 'modpack' ? 'mod' : modProjectType.value
  // Bedrock-профиль не принимает Java-контент: в модпаках ищем по версии из
  // селектора, чтобы выбранный Bedrock-экземпляр не ломал каталог.
  const instanceName = currentView.value === 'mods' ? (selectedInstance.value?.name || '') : (catalogTargetInstance()?.name || '')
  const searchInstanceName = () => currentView.value === 'mods' ? (selectedInstance.value?.name || '') : (catalogTargetInstance()?.name || '')
  const targetInstance = catalogTargetInstance()
  const searchView = currentView.value
  const searchProjectType = projectType
  const searchCatalogId = activeCatalogId.value
  const requiresServer = projectType === 'plugin'
  if (currentView.value === 'mods' && selectedInstance.value && (requiresServer ? selectedInstance.value.kind !== 'server' : selectedInstance.value.kind === 'server' || selectedInstance.value.game_edition === 'bedrock')) {
    modResults.value = []
    modError.value = requiresServer ? 'Выберите серверный экземпляр для каталога плагинов.' : 'Выберите Java-экземпляр для каталога ресурсов.'
    modSearchPerformed.value = false
    return
  }
  modBusy.value = true
  modError.value = ''
  const targetLabel = selectedInstance.value?.name || `${selectedProjectLabel.value} · новый экземпляр`
  openActivity('Каталог ресурсов', targetLabel, false)
  try {
    const result = (
      await bridge.modsSearch({
        query: modQuery.value,
        instance_name: instanceName,
        game_version: modInstanceVersion.value || selectedVersion.value,
        loader: targetInstance?.loader || selectedLoader.value,
        server_core: targetInstance?.server_core || catalogServerCore.value,
        project_type: projectType,
        catalog_id: activeCatalogId.value,
        sort_by: modSortMode.value,
      })
    )
    if (
      requestId !== modSearchRequestId
      || searchView !== currentView.value
      || searchProjectType !== (currentView.value === 'modpacks' ? 'modpack' : modProjectType.value)
      || searchCatalogId !== activeCatalogId.value
      || instanceName !== searchInstanceName()
    ) return
    modResults.value = result.results
    modSearchPerformed.value = true
    finishActivity(`Найдено проектов: ${modResults.value.length}`)
  } catch (error) {
    if (requestId !== modSearchRequestId) return
    modError.value = errorMessage(error)
    modSearchPerformed.value = true
    activity.logs.push(errorMessage(error))
    finishActivity(isCancelled(error) ? 'Поиск остановлен.' : 'Каталог недоступен.')
    if (!isCancelled(error)) showToast(errorMessage(error))
  } finally {
    if (requestId === modSearchRequestId) modBusy.value = false
  }
}

watch(modSortMode, () => {
  if (modSearchPerformed.value && (currentView.value === 'mods' || currentView.value === 'modpacks')) void searchMods()
})

async function installMod(mod: ModResult) {
  const projectType: ProjectType = currentView.value === 'modpacks' ? 'modpack' : modProjectType.value
  if (projectType === 'modpack') {
    await createModpackInstance(mod)
    return
  }
  if (!selectedInstance.value) {
    showToast('Сначала выберите экземпляр для установки')
    setView('instances')
    return
  }
  if (projectType === 'datapack') {
    try {
      const details = await bridge.instanceDetails(selectedInstance.value.name)
      if (!details.worlds.length) {
        showToast('Сначала создайте хотя бы один мир в этом экземпляре')
        return
      }
      datapackWorlds.value = details.worlds
      pendingDatapack.value = mod
      datapackWorldId.value = details.worlds[0].id
      datapackDialogOpen.value = true
    } catch (error) {
      showToast(errorMessage(error))
    }
    return
  }
  await installModIntoTarget(mod, undefined, projectType)
}

async function installModIntoTarget(mod: ModResult, worldName?: string, projectType = modProjectType.value) {
  const instance = selectedInstance.value
  if (!instance) return
  const projectId = mod.project_id || mod.id
  if (!projectId) return
  modBusy.value = true
  modInstallId.value = projectId
  modAction.value = 'install'
  modInstallProgress.value = 8
  modInstallDetail.value = 'Подготавливаю загрузку…'
  datapackDialogOpen.value = false
  pendingDatapack.value = undefined
  openActivity('Установка ресурса', `${mod.title || mod.name || projectId} · ${instance.name}`, false)
  try {
    let beforeInstallEmitted = false
    const result = (await withUnverifiedDownloadConfirmation(
      async (confirmUnverified) => {
        if (!beforeInstallEmitted) {
          beforeInstallEmitted = true
          await broadcastPluginEvent('mods.beforeInstall', {
            instance: pluginInstanceRef(instance)!,
            project_id: projectId,
            source: activeCatalogId.value,
            version: mod.version,
          })
        }
        return bridge.modsInstall({
          project_id: projectId,
          instance_name: instance.name,
          project_type: projectType,
          world_name: worldName,
          catalog_id: activeCatalogId.value,
          version: mod.version,
          version_id: mod.version_id,
          display_name: mod.title || mod.name,
          confirm_unverified: confirmUnverified,
        })
      },
      'Источник ресурса не предоставил издательский хеш. Размер не подтверждает подлинность. Скачать файл всё равно?',
      'Установка отменена; файлы экземпляра не изменены.',
    )).value
    mod.installed = true
    mod.file_name = result.file_name || mod.file_name
    finishActivity('Ресурс установлен.', [`Файл: ${result.file_name}`, `Экземпляр: ${instance.name}`])
    showToast('Ресурс установлен в экземпляр')
  } catch (error) {
    activity.logs.push(errorMessage(error))
    finishActivity(isCancelled(error) ? 'Установка остановлена.' : 'Установка ресурса остановлена.')
    if (!isCancelled(error)) showToast(errorMessage(error))
  } finally {
    modBusy.value = false
    modInstallId.value = ''
    modInstallProgress.value = 0
    modInstallDetail.value = ''
  }
}

async function removeMod(mod: ModResult) {
  const instance = selectedInstance.value
  const projectId = mod.project_id || mod.id
  const projectType = modProjectType.value
  if (!instance || !projectId) {
    showToast('Сначала выберите экземпляр для удаления')
    return
  }
  if (instance.running) {
    showToast('Остановите экземпляр или сервер перед удалением ресурса')
    return
  }
  if (projectType === 'plugin' && !mod.file_name) {
    showToast('Не удалось определить установленный файл плагина. Обновите результаты поиска.')
    return
  }
  if (!(await confirmDestructive(`Удалить ресурс «${mod.title || mod.name || projectId}» из «${instance.name}»?`))) return
  modBusy.value = true
  modInstallId.value = projectId
  modAction.value = 'remove'
  modInstallProgress.value = 18
  modInstallDetail.value = 'Удаляю установленные файлы…'
  openActivity('Удаление ресурса', `${mod.title || mod.name || projectId} · ${instance.name}`, false)
  try {
    if (projectType === 'modpack') {
      showToast('Повторный импорт заменит файлы модпака; отдельное удаление не поддерживается')
      return
    }
    let removed: number
    if (projectType === 'plugin') {
      removed = (await bridge.serverPluginRemove(instance.name, mod.file_name || '')).removed ? 1 : 0
    } else {
      removed = (await bridge.modsRemove({
          project_id: projectId,
          instance_name: instance.name,
          project_type: projectType,
          catalog_id: mod.catalog_id || activeCatalogId.value,
      })).removed
    }
    mod.installed = false
    mod.file_name = ''
    const target = projectType === 'plugin' ? 'Сервер' : 'Экземпляр'
    finishActivity('Ресурс удалён.', [`Удалено файлов: ${removed}`, `${target}: ${instance.name}`])
    showToast(projectType === 'plugin' ? 'Плагин удалён из сервера' : 'Ресурс удалён из экземпляра')
  } catch (error) {
    activity.logs.push(errorMessage(error))
    finishActivity(isCancelled(error) ? 'Удаление остановлено.' : 'Удаление ресурса остановлено.')
    if (!isCancelled(error)) showToast(errorMessage(error))
  } finally {
    modBusy.value = false
    modInstallId.value = ''
    modInstallProgress.value = 0
    modInstallDetail.value = ''
  }
}

async function confirmDatapackInstall() {
  if (!pendingDatapack.value || !datapackWorldId.value) return
  await installModIntoTarget(pendingDatapack.value, datapackWorldId.value, 'datapack')
}

async function loadSkins() {
  try {
    skinResults.value = (await bridge.skinsList()).skins
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function loadSkinProfile(accountId = skinAccountId.value) {
  if (!accountId) return
  const requestId = ++skinProfileRequestId
  skinProfileLoading.value = true
  skinProfileError.value = ''
  try {
    const profile = await bridge.skinProfile(accountId)
    if (requestId !== skinProfileRequestId || !skinPanelOpen.value || skinAccountId.value !== accountId) return
    skinProfileData.value = profile
    selectCurrentSkin()
    selectedCapeId.value = activeCape.value?.id || ''
  } catch (error) {
    if (requestId !== skinProfileRequestId || !skinPanelOpen.value || skinAccountId.value !== accountId) return
    skinProfileData.value = undefined
    skinProfileError.value = errorMessage(error)
    showToast(`Не удалось загрузить профиль Minecraft: ${skinProfileError.value}`)
  } finally {
    if (requestId === skinProfileRequestId) skinProfileLoading.value = false
  }
}

async function openSkinSettings(account: AccountInfo) {
  if (account.kind !== 'microsoft') {
    showToast('Настройка скина доступна только для Microsoft-аккаунта')
    return
  }
  accountMenuId.value = ''
  skinAccountId.value = account.account_id
  skinProfileData.value = undefined
  skinProfileError.value = ''
  skinProfileLoading.value = false
  selectedSkinId.value = ''
  selectedCapeId.value = ''
  skinModel.value = 'classic'
  currentSkinModel.value = 'classic'
  skinPanelOpen.value = true
  void loadSkins()
  await loadSkinProfile(account.account_id)
}

function closeSkinSettings() {
  skinProfileRequestId += 1
  skinPanelOpen.value = false
  skinProfileData.value = undefined
  skinProfileLoading.value = false
  skinProfileError.value = ''
  selectedSkinId.value = ''
  selectedCapeId.value = ''
  currentSkinModel.value = 'classic'
}

function closeAccountMenuOnOutside(event: MouseEvent) {
  if ((event.target as Element).closest?.('.account-menu, .account-actions')) return
  accountMenuId.value = ''
}

async function applySelectedSkin(skin: SkinInfo) {
  if (!skinAccountId.value) return
  try {
    await bridge.applySkin(skinAccountId.value, skin.skin_id, skinModel.value)
    showToast(`Скин «${skin.name}» установлен`)
    skinProfileData.value = await bridge.skinProfile(skinAccountId.value)
    selectCurrentSkin()
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function resetSkin() {
  if (!skinAccountId.value) return
  try {
    await bridge.clearSkin(skinAccountId.value)
    showToast('Скин сброшен на стандартный')
    skinProfileData.value = await bridge.skinProfile(skinAccountId.value)
    selectCurrentSkin()
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function selectCape(capeId: string) {
  if (!skinAccountId.value || capeActionBusy.value) return
  if (capeId && !availableCapes.value.some((cape) => cape.id === capeId)) {
    showToast('Можно выбрать только плащ из профиля Minecraft')
    return
  }
  capeActionBusy.value = true
  try {
    if (capeId) await bridge.applyCape(skinAccountId.value, capeId)
    else await bridge.clearCape(skinAccountId.value)
    selectedCapeId.value = capeId
    showToast(capeId ? 'Плащ установлен' : 'Плащ снят')
    skinProfileData.value = await bridge.skinProfile(skinAccountId.value)
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    capeActionBusy.value = false
  }
}

function encodeBytes(bytes: Uint8Array): string {
  let binary = ''
  const chunkSize = 0x8000
  for (let index = 0; index < bytes.length; index += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(index, index + chunkSize))
  }
  return btoa(binary)
}

function chooseSkinFile() {
  skinFileInput.value?.click()
}

async function importSkinFile(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  try {
    const result = await bridge.skinsImport(file.name, encodeBytes(new Uint8Array(await file.arrayBuffer())), skinModel.value)
    skinResults.value.push(result.skin)
    selectSkin(result.skin)
    showToast('Скин добавлен в библиотеку')
  } catch (error) {
    showToast(errorMessage(error))
  } finally {
    input.value = ''
  }
}

async function accountAction(account: AccountInfo, action: 'default' | 'up' | 'down' | 'delete') {
  accountMenuId.value = ''
  try {
    if (action === 'default') {
      const result = await bridge.setDefaultAccount(account.account_id)
      state.accounts = result.accounts
      selectedAccount.value = 'auto'
      showToast(`Профиль «${account.display_name}» выбран по умолчанию`)
    } else if (action === 'up' || action === 'down') {
      state.accounts = (await bridge.moveAccount(account.account_id, action)).accounts
      showToast('Порядок профилей обновлён')
    } else if (await confirmDestructive(`Удалить профиль «${account.display_name}»?`)) {
      await bridge.deleteAccount(account.account_id)
      if (skinAccountId.value === account.account_id) closeSkinSettings()
      await refreshBootstrap()
      showToast('Аккаунт удалён')
    }
  } catch (error) {
    showToast(errorMessage(error))
  }
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  if (typeof error === 'string') return error
  try {
    return JSON.stringify(error)
  } catch {
    return 'Неизвестная ошибка'
  }
}

onMounted(async () => {
  if (overlayMode) {
    // Отдельное окно оверлея: тяжёлый bootstrap лаунчера не нужен,
    // GameOverlay сам опрашивает overlay_state.
    document.title = 'Nivora Overlay'
    document.body.classList.add('overlay-mode')
    // Непрозрачный режим (Wayland): вёрстка оверлея полупрозрачна,
    // без плотного фона она ляжет на белый фон окна
    if (new URLSearchParams(window.location.search).get('opaque') === '1') {
      document.body.classList.add('overlay-mode--opaque')
    }
    return
  }
  if (inDesktopTauri()) {
    startOverlayWatcher()
    // Крестик главного окна прячет лаунчер в трей (решение в Rust-стороне);
    // говорим об этом один раз, иначе «исчезнувший» лаунчер выглядит багом.
    let trayHintShown = false
    try {
      const { listen } = await import('@tauri-apps/api/event')
      await listen('nivora-hidden-to-tray', () => {
        if (trayHintShown) return
        trayHintShown = true
        void bridge.systemNotify('Nivora Launcher', 'Лаунчер свернулся в трей и продолжает работать — глобальные клавиши и оверлей активны. Полный выход: «Выйти» в меню значка.').catch(() => undefined)
      })
    } catch {
      // без подсказки тоже проживём
    }
  }
  void loadSettingsProfiles()
  pluginDevPollTimer = window.setInterval(() => { void pollPluginDevelopment() }, 2000)
  window.addEventListener('keydown', handleGlobalKeydown)
  window.addEventListener('resize', scheduleWallpaperRender)
  window.addEventListener('focus', syncLauncherHidden)
  window.addEventListener('blur', syncLauncherHidden)
  document.addEventListener('visibilitychange', syncLauncherHidden)
  syncLauncherHidden()
  document.addEventListener('click', closeAccountMenuOnOutside)
  systemThemeMedia = window.matchMedia('(prefers-color-scheme: dark)')
  systemDark.value = systemThemeMedia.matches
  systemThemeListener = (event) => { systemDark.value = event.matches }
  systemThemeMedia.addEventListener('change', systemThemeListener)
  try {
    stopBackendEvents = await bridge.subscribeEvents((event) => {
      if (event.kind === 'log' && typeof event.line === 'string') {
        const sourceInstance = event.instance_name ? state.instances.find((instance) => instance.name === event.instance_name) : undefined
        const line = event.line.slice(0, 8192)
        const level: PluginEventPayloadMap['log.line']['level'] = /\b(ERROR|FATAL|EXCEPTION)\b/i.test(line) ? 'error'
          : /\b(WARN|WARNING)\b/i.test(line) ? 'warning'
            : /\bDEBUG\b/i.test(line) ? 'debug' : 'info'
        void broadcastPluginEvent('log.line', {
          ...(sourceInstance ? { instance: pluginInstanceRef(sourceInstance) || undefined } : {}),
          level,
          line,
        })
      }
      const catalogInstallInProgress = modBusy.value && Boolean(modInstallId.value)
      if (event.kind === 'status' && event.event === 'workspace-changed' && event.instance_name === selectedInstanceName.value && currentView.value === 'mods' && !catalogInstallInProgress) {
        void searchMods()
      }
      if (catalogInstallInProgress && event.kind === 'download') {
        modInstallDetail.value = event.destination || 'Загрузка файла…'
        if (event.total && event.total > 0) modInstallProgress.value = Math.min(96, Math.round(((event.current || 0) * 100) / event.total))
        return
      }
      if (catalogInstallInProgress && event.kind === 'status') {
        modInstallDetail.value = event.message || 'Установка ресурса…'
        if (event.event === 'operation-finished') modInstallProgress.value = 100
        return
      }
      if (event.kind === 'status' && event.event === 'server-stopped') return
      if (event.kind === 'status' && ['workspace-changed', 'server-config-changed', 'server-core-uploaded', 'server-core-downloaded'].includes(event.event || '')) return
      if (event.kind === 'status' && event.event === 'operation-finished') {
        if (!activity.busy && !activity.open) return
        finishActivity(event.message || 'Операция завершена.')
        return
      }
      if (!settingsDraft.show_activity_panel || (!activity.open && event.kind !== 'log')) return
      if (!activity.busy) return
      activity.open = true
      if (event.kind === 'log' && event.line) {
        activity.title = 'Консоль запуска'
        activity.detail = 'Minecraft работает…'
        activity.logs.push(event.line)
        if (activity.logs.length > 200) activity.logs.splice(0, activity.logs.length - 200)
        activity.progress = 68
      }
      if (event.kind === 'download' && event.destination) {
        activity.title = 'Загрузка файлов'
        activity.detail = event.destination
        if (event.total && event.total > 0) activity.progress = Math.min(96, Math.round(((event.current || 0) * 100) / event.total))
      }
      if (event.kind === 'status' && event.message) {
        activity.title = event.event === 'microsoft-auth' ? 'Вход через Microsoft' : 'Сеть и файлы'
        activity.detail = event.message
        if (event.event === 'microsoft-auth') {
          if (event.code) applyMicrosoftDeviceCode(event.code, event.verification_uri || activity.verification_uri || '')
          if (event.verification_uri) activity.verification_uri = event.verification_uri
        }
        activity.progress = Math.max(activity.progress, 18)
        if (!activity.logs.includes(event.message)) activity.logs.push(event.message)
        if (activity.logs.length > 80) activity.logs.splice(0, activity.logs.length - 80)
      }
    })
  } catch (error) {
    showToast(errorMessage(error))
  }
  await refreshLocalThemes()
  await refreshBootstrap(false)
  firstRunState.value = readFirstRunExperience()
  firstRunPhase.value = initialFirstRunPhase(firstRunState.value)
  if (settingsDraft.auto_refresh_metadata) void refreshVersions()
  if (loaderChoiceTimer) {
    window.clearTimeout(loaderChoiceTimer)
    loaderChoiceTimer = undefined
  }
  firstRunReady.value = true
})

onBeforeUnmount(() => {
  if (pluginDevPollTimer) window.clearInterval(pluginDevPollTimer)
  if (topbarMetricsTimer !== undefined) window.clearInterval(topbarMetricsTimer)
  stopOverlayWatcher()
  void detachOverlay()
  window.removeEventListener('keydown', handleGlobalKeydown)
  window.removeEventListener('resize', scheduleWallpaperRender)
  window.removeEventListener('focus', syncLauncherHidden)
  window.removeEventListener('blur', syncLauncherHidden)
  document.removeEventListener('visibilitychange', syncLauncherHidden)
  if (wallpaperResizeTimer) window.clearTimeout(wallpaperResizeTimer)
  wallpaperRenderer.release()
  stopBackendEvents?.()
  if (loaderChoiceTimer) window.clearTimeout(loaderChoiceTimer)
  if (modSearchTimer) window.clearTimeout(modSearchTimer)
  if (toastTimer) window.clearTimeout(toastTimer)
  if (activityCloseTimer) window.clearTimeout(activityCloseTimer)
  stopActivityLogPolling()
  stopMicrosoftStatusPolling()
  if (systemThemeMedia && systemThemeListener) systemThemeMedia.removeEventListener('change', systemThemeListener)
  delete document.body.dataset.launcherAnimations
  document.documentElement.style.removeProperty('--ui-scale')
  document.removeEventListener('click', closeAccountMenuOnOutside)
  stopSidebarResize()
  releaseWallpaperPreview()
  void pluginRuntime.dispose()
  pluginViews.value = []
  pluginCommands.value = []
  pluginShortcuts.value = []
  pluginEventSubscriptions.clear()
  pluginLogEventInFlight.clear()
  pluginLogEventLastSentAt.clear()
  pluginNotificationToastIds.clear()
  pluginThemeContributions.clear()
})
</script>

<template>
  <GameOverlay v-if="overlayMode || inAppOverlayOpen" />
  <DialogHost v-else />
  <div v-if="!overlayMode" ref="appShellRef" class="app-shell">
        <div v-if="buildStampMismatch" class="build-mismatch-banner" role="alert">
          <Icon name="info" :size="16" />
          <span>Части лаунчера собраны из разных версий: интерфейс {{ buildStampMismatch.frontend }}, Python-бэкенд — {{ buildStampMismatch.backend }}. Подменён только бэкенд: часть экранов (Bedrock, оверлей и др.) работает непредсказуемо. Пересоберите приложение целиком: <code>cd frontend &amp;&amp; npm install &amp;&amp; npm run tauri build</code></span>
          <button class="icon-button" type="button" aria-label="Скрыть предупреждение" @click="buildStampMismatch = null">
            <Icon name="close" :size="14" />
          </button>
        </div>
     <div v-if="wallpaperUrl && !wallpaperLoadFailed" class="wallpaper-layer" aria-hidden="true">
       <img :src="wallpaperDisplayUrl" alt="" @load="wallpaperLoadFailed = false" @error="wallpaperLoadFailed = true" />
      </div>
     <AppSidebar
         :current="currentView === 'instance-settings' ? 'instances' : currentView"
        :mobile-open="mobileMenuOpen"
         :position="appearanceDraft.sidebar_position"
         :width-mode="appearanceDraft.sidebar_width_mode"
         :cloud-configured="settingsDraft.cloud_configured"
         :plugin-tabs="enabledPluginTabs"
         :active-plugin-tab-id="activePluginTabId"
         :instances="state.instances"
         :groups="state.settings.instance_groups"
         :pinned-groups="state.settings.pinned_instance_groups"
          :group-filter="instanceGroupFilter"
          :group-busy="groupActionBusy"
          :account-id="selectedAccount"
         :account-initial="effectiveAccount?.display_name?.slice(0, 1).toLocaleUpperCase() || 'A'"
         :account-options="accountOptions"
         @navigate="setView"
         @plugin-tab="setPluginTab"
         @group-filter="selectInstanceGroupFilter"
         @create-group="createInstanceGroup"
          @account="selectAccountFromSidebar"
         @close="mobileMenuOpen = false"
         @resize-start="startSidebarResize"
      />

        <main class="main-panel">
        <header class="topbar">
        <button class="mobile-menu-button" type="button" aria-label="Открыть меню" @click="mobileMenuOpen = true">
          <Icon name="instances" :size="20" />
        </button>
        <div class="page-heading">
          <span class="eyebrow">{{ page.kicker }}</span>
          <h1>{{ page.title }}</h1>
        </div>
        <div class="topbar-actions">
          <div v-if="topbarMetricsVisible" class="topbar-metrics" aria-label="Системные метрики">
            <span v-if="settingsDraft.overlay_settings.metrics_cpu && topbarSystemMetrics?.cpu.percent !== null && topbarSystemMetrics?.cpu.percent !== undefined" class="topbar-metric" v-tooltip="'CPU'"><i>CPU</i>{{ topbarSystemMetrics.cpu.percent.toFixed(0) }}%</span>
            <span v-if="settingsDraft.overlay_settings.metrics_ram && topbarSystemMetrics?.memory.percent !== null && topbarSystemMetrics?.memory.percent !== undefined" class="topbar-metric" v-tooltip="'RAM'"><i>RAM</i>{{ topbarSystemMetrics.memory.percent.toFixed(0) }}%</span>
            <span v-for="gpu in topbarGpus" :key="gpu.index" class="topbar-metric" v-tooltip="gpu.name"><i>GPU</i>{{ gpu.utilization_percent !== null && gpu.utilization_percent !== undefined ? `${gpu.utilization_percent.toFixed(0)}%` : '—' }}</span>
            <span v-if="topbarTemperature" class="topbar-metric" v-tooltip="topbarTemperature.label"><i>{{ topbarTemperature.label }}</i>{{ topbarTemperature.value }}°C</span>
          </div>
          <button v-if="inDesktopTauri()" class="activity-trigger" type="button" v-tooltip="`Игровой оверлей (${overlayHotkeyLabel()}) — открывается и без горячей клавиши`" @click="() => { void handleOverlayToggleHotkey() }">
            <Icon name="mods" :size="15" />
            <span>Оверлей</span>
          </button>
          <button class="icon-button" type="button" aria-label="Обновить данные" @click="() => { void refreshBootstrap() }">
            <Icon name="refresh" :size="18" />
          </button>
          <button v-if="settingsDraft.show_activity_panel" class="activity-trigger" type="button" @click="activity.open = !activity.open">
            <span class="status-dot" :class="{ 'status-dot--busy': activity.busy }" />
            <span>{{ activity.busy ? 'В работе' : 'Готово' }}</span>
          </button>
        </div>
      </header>

       <div class="page-scroll">
           <div class="page-transition" :class="{ 'page-transition--animate': pageTransitionPulse }">
               <section data-page-view="instances" class="page-content">
                <div class="section-intro library-intro"><div><span class="eyebrow">Библиотека</span><h2>Экземпляры</h2><p>Профили игры и серверы Minecraft.</p></div><div class="section-intro-actions"><button class="button button--quiet" type="button" :disabled="externalImportBusy" @click="importExternalInstances"><Icon name="upload" :size="16" />{{ externalImportBusy ? 'Ищу источники…' : 'Импорт' }}</button><button class="button button--outline" type="button" @click="openCreateServer"><Icon name="server" :size="16" />Новый сервер</button><button class="button button--accent" type="button" @click="openCreateInstance"><Icon name="plus" :size="16" />Новый экземпляр</button></div></div>
                <div v-if="homeWidgetViews.length" class="plugin-home-widgets">
                  <article v-for="entry in homeWidgetViews" :key="`${entry.pluginId}:${entry.view.id}`" class="plugin-home-widget panel">
                    <div class="panel-heading"><div><span class="eyebrow">Плагин</span><h2>{{ entry.view.title }}</h2></div><span class="tag">{{ entry.pluginName }}</span></div>
                    <PluginViewRenderer
                      :view="entry.view"
                      @action="(payload) => runPluginAction({ pluginId: entry.pluginId, ...payload })"
                      @config="(payload) => void savePluginConfig({ pluginId: entry.pluginId, key: payload.key, value: payload.value })"
                      @open-link="(url) => void openPluginLink({ pluginId: entry.pluginId, url })"
                    />
                  </article>
                </div>
              <section v-if="createInstanceOpen" class="panel launch-panel instance-create-panel">
               <div class="panel-heading">
                  <div><span class="eyebrow">Новый профиль</span><h2>{{ creationKind === 'server' ? 'Создать сервер' : creationKind === 'bedrock' ? 'Создать Bedrock-профиль' : 'Создать экземпляр' }}</h2></div>
                  <button class="icon-button" type="button" aria-label="Закрыть" @click="closeCreateInstance"><Icon name="close" :size="17" /></button>
                </div>
                <div class="creation-mode-switch"><button class="button button--small" :class="creationKind === 'client' ? 'button--accent' : 'button--quiet'" type="button" @click="openCreateInstance">Java</button><button class="button button--small" :class="creationKind === 'bedrock' ? 'button--accent' : 'button--quiet'" type="button" @click="openCreateBedrock">Bedrock</button><button class="button button--small" :class="creationKind === 'server' ? 'button--accent' : 'button--quiet'" type="button" @click="openCreateServer">Server workspace</button></div>
                 <div v-if="creationKind === 'client'" class="field-grid">
                  <label class="field"><span>Имя экземпляра</span><input v-model="creationName" placeholder="Например, Survival" /></label>
                   <div class="field"><span>Версия Minecraft</span><CustomSelect v-model="selectedVersion" :options="versionOptions" /><small v-if="versionError" class="field-error">{{ versionError }}</small></div>
               </div>
               <div v-if="creationKind === 'client'" class="field field--wide">
                <span>Загрузчик</span>
                <div class="loader-picker">
                  <button v-for="loader in loaders" :key="loader.id" class="loader-choice" :class="{ 'loader-choice--active': selectedLoader === loader.id }" type="button" @click="selectedLoader = loader.id">
                    <LoaderBadge :loader="loader.id" size="sm" /><span>{{ loader.label }}</span>
                  </button>
                   </div>
                <div v-if="creationKind === 'client' && selectedLoader !== 'vanilla'" class="choice-row">
                 <span class="choice-label">Версия {{ selectedLoaderLabel }}</span>
                 <CustomSelect v-model="selectedLoaderVersion" :options="loaderVersionOptions" :disabled="loadingChoices" />
                </div>
                 </div>
                <div v-if="creationKind === 'bedrock'" class="bedrock-create-form">
                  <label class="field"><span>Имя экземпляра</span><input v-model="creationName" maxlength="64" placeholder="Например, Bedrock Survival" /></label>
                  <div class="field-grid">
                    <div class="field"><span>Редакция</span><CustomSelect v-model="bedrockCreationEdition" :options="bedrockCreationEditionOptions" :disabled="bedrockCreationVersionsBusy" /></div>
                    <div class="field"><span>Версия Minecraft</span><CustomSelect v-model="bedrockCreationVersion" :options="bedrockCreationVersionOptions" :disabled="bedrockCreationVersionsBusy || !bedrockCreationVersionOptions.length" /><small v-if="bedrockCreationVersionsBusy" class="field-help">Получаю список версий…</small><small v-else-if="bedrockCreationVersionsError" class="field-error">{{ bedrockCreationVersionsError }}</small><small v-else-if="!bedrockCreationVersionOptions.length" class="field-help">Нажмите «Обновить», чтобы получить список версий.</small></div>
                  </div>
                  <div class="bedrock-version-hint">
                    <span>Версия загружается сразу после создания профиля — как в Java-экземплярах. Для загрузки нужен Microsoft-аккаунт с лицензией Minecraft.</span>
                    <button class="button button--small button--quiet" type="button" :disabled="bedrockCreationVersionsBusy" @click="loadBedrockCreationVersions(true)"><Icon name="refresh" :size="14" />{{ bedrockCreationVersionsBusy ? 'Обновляю…' : 'Обновить' }}</button>
                  </div>
                  <div class="bedrock-create-notice" role="status">
                    <strong>{{ bedrockCreationStatus?.supported ? 'Среда запуска готова' : 'Проверяю среду запуска' }}</strong>
                    <span>{{ bedrockCreationStatus?.message || 'Проверяю компонент Nivora…' }}</span>
                    <p v-if="bedrockCreationStatus?.platform === 'windows-official'">Windows использует официальный Minecraft for Windows из Microsoft Store: после создания профиля Nivora откроет страницу установки Store — все зависимости ставятся автоматически.</p>
                    <p v-else-if="bedrockCreationStatus?.platform === 'unsupported'">Официальный Minecraft Bedrock не выпускается для этой ОС: он доступен в Windows (Microsoft Store) и в Linux-сборке Nivora.</p>
                    <p v-else>Для загрузки выбранной версии войдите в Microsoft-аккаунт, которому принадлежит Minecraft.</p>
                    <a v-if="bedrockCreationStatus?.platform !== 'unsupported' && bedrockCreationStatus?.platform !== 'windows-official'" href="https://github.com/Wyze3306/BedrockOnLinux" target="_blank" rel="noreferrer">Системные требования</a>
                  </div>
                </div>
                <div v-if="creationKind === 'server'" class="server-create-form">
                    <div class="field-grid"><label class="field"><span>Имя сервера</span><input v-model="creationName" placeholder="Например, Survival Server" /></label><label class="field"><span>Отображаемое название</span><input v-model="serverCreation.display_name" placeholder="Survival Server" /></label></div>
                    <div class="choice-row server-source-row"><span class="choice-label"><strong>Источник ядра</strong><small>{{ serverCreation.install_mode === 'official' ? 'Официальная сборка загрузится автоматически.' : 'Готовый server.jar будет установлен как есть.' }}</small></span><div class="segmented-control" role="tablist" aria-label="Источник ядра"><button type="button" :class="{ active: serverCreation.install_mode === 'official' }" role="tab" :aria-selected="serverCreation.install_mode === 'official'" @click="selectOfficialServerCore"><Icon name="download" :size="13" />Официальное</button><button type="button" :class="{ active: serverCreation.install_mode === 'custom' }" role="tab" :aria-selected="serverCreation.install_mode === 'custom'" @click="selectCustomServerCore"><Icon name="upload" :size="13" />Собственный JAR</button></div></div>
                     <div v-if="serverCreation.install_mode === 'official'" class="field-grid"><div class="field"><span class="field-heading-with-icon"><span>Ядро</span><CoreBadge :core="serverCreation.core" compact /></span><CustomSelect v-model="serverCreation.core" :options="serverCoreOptions" /></div><div class="field"><span>Версия Minecraft</span><CustomSelect v-model="serverCreation.version" :options="serverVersionOptions" :disabled="serverVersionsBusy" /></div></div>
                      <div class="field-grid"><div class="field"><span>Порт</span><NumericStepper v-model="serverCreation.port" :min="1" :max="65535" aria-label="Порт сервера" /></div><div class="field"><span>Память JVM</span><div class="memory-input-row"><NumericStepper v-model="serverMemoryValue" :min="serverMemoryMinimum" :max="serverMemoryMaxValue" :step="serverMemoryStep" :decimals="serverMemoryDecimals" aria-label="Память JVM сервера" /><CustomSelect :model-value="serverMemoryUnit" :options="memoryUnitOptions" @update:model-value="setServerMemoryUnit" /></div><small class="field-help">Лимит {{ serverMemoryLimitLabel }}; 25% RAM оставляется системе. JVM получит {{ serverMemorySetting }}.</small></div></div>
                     <div v-if="serverCreation.install_mode === 'custom'" class="server-jar-drop" :class="{ 'server-jar-drop--active': serverCoreDragActive }" @dragenter.prevent="serverCoreDragActive = true" @dragover.prevent="serverCoreDragActive = true" @dragleave.prevent="serverCoreDragActive = false" @drop.prevent="onServerCoreDrop"><div><strong>{{ serverCreation.jar_name || 'Перетащите сюда server.jar' }}</strong><small>{{ serverCoreDragActive ? 'Отпустите файл, чтобы выбрать ядро.' : 'Перетащите JAR в эту область или выберите его на устройстве.' }}</small></div><input ref="serverCoreFileInput" class="workspace-file-input" type="file" accept=".jar" @change="loadServerCoreFile" /><button class="button button--small button--outline" type="button" @click="serverCoreFileInput?.click()"><Icon name="upload" :size="14" />Выбрать JAR</button></div>
                    <label class="workspace-check server-eula-check"><input v-model="serverCreation.eula_accepted" type="checkbox" /><span><strong>Я принимаю EULA Minecraft</strong><small>Требуется только для запуска сервера.</small></span></label>
                 </div>
               <div class="launch-actions">
                  <button v-if="creationKind === 'client'" class="button button--quiet" type="button" :disabled="loadingVersions" @click="refreshVersions"><Icon name="refresh" :size="15" />{{ loadingVersions ? 'Загружаю…' : 'Обновить версии' }}</button>
                  <button class="button button--outline" type="button" @click="closeCreateInstance">Отмена</button>
                     <button class="button button--accent" type="button" :disabled="busy || (creationKind === 'bedrock' && (!bedrockCreationStatus || !bedrockCreationStatus.supported))" @click="installSelected"><Icon :name="creationKind === 'server' ? (serverCreation.install_mode === 'official' ? 'download' : 'server') : creationKind === 'bedrock' ? 'archive' : 'download'" :size="16" />{{ creationKind === 'server' ? 'Создать сервер' : creationKind === 'bedrock' ? (bedrockCreationVersion ? 'Создать и установить' : 'Создать профиль') : 'Создать и установить' }}</button>
               </div>
             </section>
               <InstanceGroups
                  :instances="state.instances"
                  :groups="state.settings.instance_groups"
                  :pinned-groups="state.settings.pinned_instance_groups"
                  :active-filter="instanceGroupFilter"
                  :search-shortcut="settingsDraft.keyboard_shortcuts.focus_search"
                 :busy="busy"
                 :group-busy="groupActionBusy"
                 @update:active-filter="instanceGroupFilter = $event"
                 @new-instance="openCreateInstance"
                @create-group="createInstanceGroup"
                @rename-group="renameInstanceGroup"
                 @delete-group="deleteInstanceGroup"
                 @toggle-pin="toggleInstanceGroupPin"
                @move-group="moveInstanceToGroup"
                @play="launchExisting"
                @stop="stopExisting"
                @settings="openInstance"
                @duplicate="duplicateInstance"
                @delete-instance="deleteInstance"
              />
              </section>

                <section data-page-view="instance-settings" class="page-content instance-workspace-page">
                  <div class="section-intro">
                    <div><span class="eyebrow">Профиль игры</span><h2>{{ selectedInstance?.display_name || selectedInstance?.name || 'Рабочая область экземпляра' }}</h2><p>{{ selectedInstance?.game_edition === 'bedrock' ? 'Отдельный профиль Bedrock: версия игры, показатели ресурсов, файлы и контент.' : 'Полноэкранная рабочая область: консоль, ресурсы, файлы и настройки профиля.' }}</p></div>
                    <button class="button button--outline" type="button" @click="setView('instances')"><Icon name="arrow-left" :size="15" />К экземплярам</button>
                  </div>
                   <ServerWorkspace v-if="currentView === 'instance-settings' && selectedInstance?.kind === 'server'" ref="serverWorkspaceRef" :key="`workspace-server-${selectedInstance.name}`" :instance="selectedInstance" :plugin-catalogs="state.settings.plugin_catalogs" :server-cores="state.server_cores" :focus-tab="instanceWorkspaceFocus" :confirm-destructive="settingsDraft.confirm_destructive_actions" :snapshot-created="notifyPluginSnapshotCreated" @toast="showToast" @changed="refreshBootstrap" />
                     <BedrockWorkspace v-else-if="currentView === 'instance-settings' && selectedInstance?.game_edition === 'bedrock'" ref="bedrockWorkspaceRef" :key="`workspace-bedrock-${selectedInstance.name}`" :instance="selectedInstance" :busy="busy" :focus-tab="instanceWorkspaceFocus" :confirm-destructive="settingsDraft.confirm_destructive_actions" @launch="launchExisting(selectedInstance)" @stop="stopExisting(selectedInstance)" @toast="showToast" @changed="refreshBootstrap" @activity="onBedrockActivity" />
                     <InstanceWorkspace v-else-if="currentView === 'instance-settings' && selectedInstance" ref="instanceWorkspaceRef" :key="`workspace-client-${selectedInstance.name}`" :instance="selectedInstance" :account="selectedAccount" :focus-tab="instanceWorkspaceFocus" :confirm-destructive="settingsDraft.confirm_destructive_actions" :before-launch="beforeInstanceLaunch" :after-exit="notifyPluginAfterExit" :crash="notifyPluginCrash" :initial-crash="lastLaunchCrash" :snapshot-created="notifyPluginSnapshotCreated" @toast="showToast" @browse-mods="setView('mods')" @changed="refreshBootstrap" @groups-changed="state.settings.instance_groups = $event" />
                 <div v-else class="empty-card"><span class="empty-orb empty-orb--large"><Icon name="settings" :size="28" /></span><h3>Экземпляр не выбран</h3><p>Вернитесь к списку экземпляров и откройте настройки нужного профиля.</p><button class="button button--accent" type="button" @click="setView('instances')">Открыть экземпляры</button></div>
              </section>

               <section data-page-view="accounts" class="page-content">
              <div class="section-intro"><div><span class="eyebrow">Профили доступа</span><h2>Аккаунты</h2><p>Выбирайте, от чьего имени будет запускаться игра.</p></div><button class="button button--outline" type="button" :disabled="busy || state.settings.login_source !== 'microsoft' || !state.settings.has_client_id" @click="loginMicrosoft"><Icon name="accounts" :size="17" />Войти через Microsoft</button></div>
             <div class="account-layout"><section class="panel account-panel"><div class="panel-heading"><div><span class="eyebrow">Подключённые</span><h2>Профили</h2></div><span class="count-badge">{{ state.accounts.length }}</span></div><p v-if="!state.settings.has_client_id" class="inline-error">Microsoft-вход не настроен в этой сборке. Вводить ID в настройках не нужно; для входа понадобится сборка с настроенным Microsoft OAuth.</p><div v-if="state.accounts.length" class="account-list"><article v-for="account in state.accounts" :key="account.account_id" class="account-row"><span class="avatar avatar--blue">{{ account.display_name.slice(0, 1).toUpperCase() }}</span><div class="account-copy"><strong>{{ account.display_name }} <span v-if="account.is_default" class="default-mark">по умолчанию</span></strong><span>{{ account.kind === 'microsoft' ? 'Microsoft account' : 'Offline debug account' }}</span></div><span class="account-status"><i />{{ account.kind === 'microsoft' ? 'Подключён' : 'Локальный' }}</span><div class="account-actions"><button class="icon-button" type="button" aria-label="Настройки профиля" @click="accountMenuId = accountMenuId === account.account_id ? '' : account.account_id"><Icon name="settings" :size="16" /></button><div v-if="accountMenuId === account.account_id" class="account-menu"><button type="button" :disabled="account.kind !== 'microsoft'" @click="openSkinSettings(account)"><Icon name="skin" :size="14" />Настроить скин</button><button type="button" @click="accountAction(account, 'default')"><Icon name="star" :size="14" />Сделать по умолчанию</button><button type="button" @click="accountAction(account, 'up')"><Icon name="arrow-up" :size="14" />Переместить выше</button><button type="button" @click="accountAction(account, 'down')"><Icon name="arrow-down" :size="14" />Переместить ниже</button><button class="account-menu-danger" type="button" @click="accountAction(account, 'delete')"><Icon name="trash" :size="14" />Удалить аккаунт</button></div></div></article></div><div v-else class="empty-mini empty-mini--roomy"><span class="empty-orb"><Icon name="accounts" :size="19" /></span><strong>Аккаунтов пока нет</strong><span>Создайте локальный аккаунт или войдите через Microsoft.</span></div></section><section class="panel create-account-panel"><span class="eyebrow">Быстрое создание</span><h2>Локальный профиль</h2><p>Подходит для одиночного тестирования и разработки модпаков. Онлайн-серверы недоступны.</p><label class="field"><span>Имя игрока</span><input v-model="accountName" maxlength="16" placeholder="Player" /></label><button class="button button--accent button--large" type="button" :disabled="busy" @click="createOfflineAccount"><Icon name="plus" :size="18" />Создать offline-профиль</button></section></div>
             <section v-if="skinPanelOpen && skinAccountId" class="panel skin-settings-panel">
               <div class="panel-heading">
                 <div><span class="eyebrow">MICROSOFT PROFILE</span><h2>Настройка скина</h2><p>Профиль: {{ state.accounts.find((account) => account.account_id === skinAccountId)?.display_name }}</p></div>
                 <button class="icon-button" type="button" aria-label="Закрыть" @click="closeSkinSettings"><Icon name="close" :size="17" /></button>
                </div>
                <div v-if="skinProfileLoading" class="skin-profile-status" role="status"><span class="skin-profile-spinner" />Загружаю скины и плащи из профиля Minecraft…</div>
                <div v-else-if="skinProfileError" class="skin-profile-status skin-profile-status--error" role="alert"><span>Не удалось получить профиль: {{ skinProfileError }}</span><button class="button button--quiet" type="button" @click="loadSkinProfile()"><Icon name="refresh" :size="14" />Повторить</button></div>
                <div class="skin-settings-layout">
                  <div class="skin-preview-3d"><SkinPreview v-if="currentView === 'accounts'" :image="skinPreviewImage" :model="currentSkinModel" :cape="Boolean(selectedCape?.url)" :cape-image="selectedCape?.url || ''" /><span class="muted">{{ skinProfileLoading ? 'Загружаю профиль Minecraft…' : selectedLocalSkin ? 'Предпросмотр выбранного скина' : activeRemoteSkin ? 'Текущий скин Minecraft' : 'Активный скин не задан' }} · {{ currentSkinModel === 'slim' ? 'стройные руки' : 'широкие руки' }}</span></div>
                  <div class="skin-controls">
                     <div class="field"><span>Модель рук</span><div class="segmented-control"><button type="button" :class="{ active: skinModel === 'classic' }" @click="selectSkinModel('classic')">Широкие</button><button type="button" :class="{ active: skinModel === 'slim' }" @click="selectSkinModel('slim')">Стройные</button></div></div>
                  </div>
                </div>
                <div class="skin-library">
                  <div class="skin-library-heading"><div><h3>Скины</h3><p>Текущий скин аккаунта и локальная библиотека.</p></div></div>
                  <div class="skin-library-grid">
                    <button v-if="activeRemoteSkin" class="skin-library-item skin-library-account-item" :class="{ active: !selectedSkinId }" type="button" @click="selectCurrentSkin">
                      <img v-if="activeRemoteSkin.url" :src="activeRemoteSkin.url" alt="" />
                      <span v-else class="skin-library-avatar-placeholder"><Icon name="skin" :size="18" /></span>
                      <span>{{ skinProfileData?.name || 'Текущий скин' }}</span>
                      <small>Профиль Minecraft · {{ activeRemoteSkin.variant?.toUpperCase() === 'SLIM' ? 'стройные руки' : 'широкие руки' }}</small>
                      <span class="skin-library-current-badge">Экипирован</span>
                    </button>
                    <div v-for="skin in skinResults" :key="skin.skin_id" class="skin-library-item" :class="{ active: selectedSkinId === skin.skin_id }" @click="selectSkin(skin)">
                      <img v-if="skin.image" :src="skin.image" alt="" />
                      <span>{{ skin.name }}</span>
                      <small>{{ skin.model === 'slim' ? 'стройные' : 'широкие' }}</small>
                      <button v-if="selectedSkinId === skin.skin_id" class="button button--small button--accent" type="button" @click.stop="applySelectedSkin(skin)">Установить</button>
                    </div>
                  </div>
                  <div v-if="skinProfileData && !activeRemoteSkin && !skinResults.length" class="empty-mini"><Icon name="skin" :size="18" /><span>В профиле нет активного скина. Библиотека скинов пуста.</span></div>
                  <div v-else-if="skinProfileData && !activeRemoteSkin" class="empty-mini"><Icon name="skin" :size="18" /><span>В профиле Minecraft нет активного скина.</span></div>
                </div>
                <div class="skin-cape-library">
                  <div class="skin-library-heading"><div><h3>Плащи аккаунта</h3><p>Показываются плащи из профиля Minecraft. Если Mojang не вернул идентификатор плаща, его можно только просмотреть.</p></div><span class="skin-cape-count">{{ availableCapes.length }}</span></div>
                  <div v-if="availableCapes.length" class="skin-cape-grid">
                    <button v-for="cape in availableCapes" :key="cape.id || `${cape.alias || cape.name || 'cape'}-${cape.url || cape.state || ''}`" class="skin-cape-card" :class="{ active: (cape.id && selectedCapeId === cape.id) || (!cape.id && selectedCape === cape) }" type="button" :disabled="capeActionBusy || !cape.id" @click="selectCape(cape.id || '')">
                      <span class="skin-cape-art"><img v-if="cape.url" :src="cape.url" alt="" /><Icon v-else name="skin" :size="20" /></span>
                      <span class="skin-cape-name">{{ cape.name || cape.alias || 'Плащ Minecraft' }}</span>
                      <small>{{ !cape.id ? 'Только предпросмотр' : selectedCapeId === cape.id ? 'Выбран' : cape.state?.toUpperCase() === 'ACTIVE' ? 'Надет в Minecraft' : 'Доступен в профиле' }}</small>
                    </button>
                    <button class="skin-cape-card skin-cape-card--none" :class="{ active: !selectedCapeId }" type="button" :disabled="capeActionBusy" @click="selectCape('')">
                      <span class="skin-cape-art"><Icon name="close" :size="20" /></span>
                      <span class="skin-cape-name">Без плаща</span>
                      <small>{{ selectedCapeId ? 'Снять текущий плащ' : 'Сейчас не надет' }}</small>
                    </button>
                  </div>
                  <div v-else-if="skinProfileData" class="empty-mini"><Icon name="skin" :size="18" /><span>В профиле Minecraft пока нет доступных плащей.</span></div>
                </div>
               <input ref="skinFileInput" class="workspace-file-input" type="file" accept="image/png" @change="importSkinFile" />
               <div class="skin-panel-actions"><button class="button button--quiet" type="button" @click="bridge.openSkinsFolder"><Icon name="folder" :size="15" />Открыть папку со скинами</button><button class="button button--outline" type="button" @click="chooseSkinFile"><Icon name="upload" :size="15" />Импортировать файл скина</button><button class="button button--quiet" type="button" @click="resetSkin"><Icon name="refresh" :size="15" />Сбросить скин</button></div>
             </section>
           </section>

               <section data-page-view="mods" class="page-content">
                   <div class="section-intro"><div><span class="eyebrow">Ресурсы экземпляра</span><h2>Каталог</h2><p>{{ modProjectType === 'plugin' ? 'Найдите плагины по версии Minecraft и совместимому server core. Установка доступна только после выбора сервера.' : 'Найдите моды, ресурспаки, шейдеры или датапаки по версии и loader. Установка доступна внутри выбранного профиля.' }}</p></div><div class="catalog-header-controls"><CustomSelect :model-value="activeCatalogId" :options="catalogPickerOptions" @update:model-value="selectActiveCatalog" /><CatalogBadge :kind="activeCatalog?.kind" :name="activeCatalog?.name" /></div></div>
               <section class="panel search-panel">
                 <div class="catalog-instance-picker">
                   <span>Экземпляр для каталога</span>
                    <CustomSelect :model-value="selectedInstanceName" :options="instanceOptions" :disabled="!instanceOptions.length" @update:model-value="selectCatalogInstance" />
                 </div>
                  <div v-if="!selectedInstance" class="catalog-independent-fields">
                    <div class="field"><span>Версия Minecraft</span><CustomSelect v-model="selectedVersion" :options="versionOptions" /></div>
                    <div v-if="modProjectType !== 'plugin'" class="field"><span>Loader</span><CustomSelect v-model="selectedLoader" :options="loaderOptions" /></div>
                    <div v-else class="field"><span>Server core</span><CustomSelect v-model="catalogServerCore" :options="catalogServerCoreOptions" /></div>
                  </div>
                  <div class="search-bar">
                   <Icon name="search" :size="19" />
                   <input v-model="modQuery" placeholder="Например, Sodium или JEI" @keyup.enter="searchMods" />
                      <CustomSelect v-model="modProjectType" :options="modProjectOptions" />
                     <CustomSelect v-model="modSortMode" :options="catalogSortOptions" />
                    <button class="button button--accent" type="button" :disabled="modBusy" @click="searchMods">
                   {{ modBusy ? 'Загружаю…' : (modQuery.trim() ? 'Найти' : 'Показать всё') }}
                 </button>
               </div>
                 <div v-if="selectedInstance && appearanceDraft.widget_visibility['mods.context'] !== false" class="search-context">
                  <span>{{ selectedProjectLabel }} · {{ selectedInstance.name }}</span>
                  <span class="context-divider" />
                   <span>{{ modInstanceVersion }}</span>
                   <span class="context-divider" />
                   <template v-if="selectedInstance.kind === 'server'"><CoreBadge :core="selectedInstance.server_core" compact /><span>{{ selectedInstance.server_core || 'server' }}</span></template>
                   <template v-else><LoaderBadge :loader="selectedInstance.loader" size="sm" /><span>{{ selectedInstance.loader }}</span></template>
                </div>
                 <div v-else class="inline-error">Работа без экземпляра: результаты можно просматривать, но установка отключена до выбора совместимого профиля.</div>
               <p v-if="modError" class="inline-error">{{ modError }}</p>
            </section>
              <div v-if="modResults.length" class="mod-grid">
                <article v-for="mod in modResults" :key="mod.project_id || mod.id" class="mod-card">
                  <div class="mod-icon"><img v-if="mod.icon_url" :src="mod.icon_url" alt="" @error="onModIconError(mod)" /><Icon v-else name="mods" :size="24" /></div>
                 <div class="mod-card-copy">
                   <h3>{{ mod.title || mod.name }}</h3>
                   <p>{{ mod.description || 'Описание не предоставлено.' }}</p>
                    <div class="mod-card-footer">
                       <span class="mod-downloads">{{ mod.downloads ? `${mod.downloads.toLocaleString()} загрузок` : 'Проект' }}</span>
                       <span v-if="modInstallId === (mod.project_id || mod.id)" class="mod-install-status" aria-live="polite"><span class="status-dot status-dot--busy" />{{ modInstallDetail || 'Загрузка…' }} <strong>{{ modInstallProgress }}%</strong></span>
                       <button class="button button--small button--quiet" type="button" :aria-label="`Подробности: ${mod.title || mod.name}`" @click="openCatalogDetails(mod)"><Icon name="info" :size="14" />Информация</button>
                          <button class="button button--small mod-install-button" :class="mod.installed ? 'button--danger' : 'button--accent'" type="button" :disabled="modBusy || !selectedInstance || (modProjectType !== 'modpack' && selectedInstance.game_edition === 'bedrock') || (mod.installed && selectedInstance.running) || (modProjectType === 'plugin' && selectedInstance.running) || (modProjectType === 'modpack' && mod.installed)" :aria-busy="modInstallId === (mod.project_id || mod.id)" @click="modProjectType === 'modpack' ? installMod(mod) : (mod.installed ? removeMod(mod) : installMod(mod))"><Icon :name="modProjectType === 'modpack' && mod.installed ? 'check' : mod.installed ? 'trash' : 'download'" :size="14" />{{ modInstallId === (mod.project_id || mod.id) ? (modAction === 'remove' ? 'Удаляю…' : 'Установка…') : (modProjectType === 'modpack' ? (mod.installed ? 'Установлено' : 'Установить модпак') : (mod.installed ? 'Удалить' : (selectedInstance?.game_edition === 'bedrock' ? 'Выберите Java-экземпляр' : selectedInstance ? 'Установить' : 'Выберите экземпляр'))) }}</button>
                    </div>
                 </div>
               </article>
             </div>
             <div v-else class="empty-card empty-card--short">
               <span class="empty-orb empty-orb--large"><Icon name="search" :size="27" /></span>
                <h3>{{ modError ? 'Каталог недоступен' : modBusy ? 'Ищу проекты…' : !selectedInstance ? 'Выберите экземпляр' : !modSearchPerformed ? 'Начните поиск' : 'Ничего не найдено' }}</h3>
                 <p>{{ modError || (modBusy ? 'Подбираю совместимые проекты для выбранной версии.' : !selectedInstance ? 'Каталог будет искать и устанавливать файлы в контекст выбранного экземпляра.' : !modSearchPerformed ? 'Введите название проекта или нажмите «Показать всё».' : 'Попробуйте изменить запрос или выбрать другой тип ресурса.') }}</p>
              </div>
            </section>

               <section data-page-view="modpacks" class="page-content">
                 <div class="section-intro"><div><span class="eyebrow">Готовые сборки</span><h2>Модпаки</h2><p>Каждая установка создаёт отдельный клиентский экземпляр. FTB, Technic и Modrinth не смешиваются с каталогом обычных ресурсов.</p></div><div class="catalog-header-controls"><CustomSelect :model-value="activeCatalogId" :options="catalogPickerOptions" @update:model-value="selectActiveCatalog" /><CatalogBadge :kind="activeCatalog?.kind" :name="activeCatalog?.name" /></div></div>
                 <section class="panel search-panel">
                   <div class="catalog-filters catalog-filters--modpack"><div class="catalog-filter"><span>Версия нового экземпляра</span><CustomSelect v-model="selectedVersion" :options="versionOptions" /></div><div class="catalog-filter"><span>Загрузчик нового экземпляра</span><CustomSelect v-model="selectedLoader" :options="loaderOptions" /></div></div>
                   <div v-if="selectedLoader !== 'vanilla'" class="catalog-filter"><span>Версия {{ selectedLoaderLabel }} для нового экземпляра</span><CustomSelect v-model="selectedLoaderVersion" :options="loaderVersionOptions" :disabled="loadingChoices" /></div>
                   <p v-else class="field-help">Vanilla не использует отдельную версию загрузчика.</p>
                    <div class="search-bar"><Icon name="search" :size="19" /><input v-model="modQuery" placeholder="Например, Create или Better MC" @keyup.enter="searchMods" /><CustomSelect v-model="modSortMode" :options="catalogSortOptions" /><input ref="modpackFileInput" class="workspace-file-input" type="file" accept=".zip,.mrpack" @change="importModpackFile" /><button class="button button--outline" type="button" :disabled="busy || modBusy" @click="chooseModpackFile"><Icon name="upload" :size="14" />Импортировать архив</button><button class="button button--accent" type="button" :disabled="modBusy" @click="searchMods">{{ modBusy ? 'Загружаю…' : (modQuery.trim() ? 'Найти' : 'Показать всё') }}</button></div>
                   <div class="search-context"><span>Новый экземпляр</span><span class="context-divider" /><span>{{ modContextLabel }}</span><span class="context-divider" /><span>Источник: {{ activeCatalog?.name || 'Modrinth' }}</span></div>
                  <p v-if="modError" class="inline-error">{{ modError }}</p>
                </section>
                  <div v-if="modResults.length" class="mod-grid"><article v-for="mod in modResults" :key="mod.project_id || mod.id" class="mod-card"><div class="mod-icon"><img v-if="mod.icon_url" :src="mod.icon_url" alt="" @error="onModIconError(mod)" /><Icon v-else name="archive" :size="24" /></div><div class="mod-card-copy"><h3>{{ mod.title || mod.name }}</h3><p>{{ mod.description || 'Описание не предоставлено.' }}</p><div class="mod-card-footer"><span class="mod-downloads">{{ mod.downloads ? `${mod.downloads.toLocaleString()} загрузок` : 'Модпак' }}</span><button class="button button--small button--quiet" type="button" :aria-label="`Подробности: ${mod.title || mod.name}`" @click="openCatalogDetails(mod)"><Icon name="info" :size="14" />Информация</button><button class="button button--small button--accent mod-install-button" type="button" :disabled="busy || modBusy || (selectedLoader !== 'vanilla' && loadingChoices)" @click="createModpackInstance(mod)"><Icon name="plus" :size="14" />Создать экземпляр</button></div></div></article></div>
                <div v-else class="empty-card empty-card--short"><span class="empty-orb empty-orb--large"><Icon name="archive" :size="27" /></span><h3>{{ modError ? 'Каталог недоступен' : modBusy ? 'Ищу модпаки…' : !modSearchPerformed ? 'Начните поиск' : 'Ничего не найдено' }}</h3><p>{{ modError || (modBusy ? 'Подбираю сборки для выбранной версии.' : !modSearchPerformed ? 'Введите название сборки или нажмите «Показать всё».' : 'Попробуйте другой запрос или источник.') }}</p></div>
              </section>

              <section data-page-view="plugins" class="page-content">
                <section v-if="activeRuntimeTab" class="panel">
                  <div class="panel-heading">
                    <div><span class="eyebrow">Вкладка плагина</span><h2>{{ activeRuntimeTab.view.title }}</h2><p>{{ activeRuntimeTab.view.description || `Живая страница плагина ${activeRuntimeTab.pluginName}.` }}</p></div>
                    <button class="button button--outline" type="button" @click="setView('plugins')">К плагинам</button>
                  </div>
                  <div class="diagnostic-note"><Icon name="plug" :size="16" /><span>Страница построена декларативными компонентами UI v2 в изолированном QuickJS; HTML плагина не выполняется.</span></div>
                  <div class="path-line"><span>Плагин</span><strong>{{ activeRuntimeTab.pluginName }} · {{ activeRuntimeTab.pluginId }}</strong></div>
                  <PluginViewRenderer
                    :view="activeRuntimeTab.view"
                    @action="(payload) => runPluginAction({ pluginId: activeRuntimeTab!.pluginId, ...payload })"
                    @config="(payload) => void savePluginConfig({ pluginId: activeRuntimeTab!.pluginId, key: payload.key, value: payload.value })"
                    @open-link="(url) => void openPluginLink({ pluginId: activeRuntimeTab!.pluginId, url })"
                  />
                </section>
                <section v-else-if="activePluginTab" class="panel">
                  <div class="panel-heading">
                    <div><span class="eyebrow">Декларативная вкладка</span><h2>{{ activePluginTab.label }}</h2><p>{{ activePluginTab.description || `Вкладка плагина ${activePluginTab.name}.` }}</p></div>
                    <button class="button button--outline" type="button" @click="setView('plugins')">К плагинам</button>
                  </div>
                  <div class="diagnostic-note"><Icon name="lock" :size="16" /><span>Вкладка создана из метаданных manifest. Файл entrypoint плагина не запускается.</span></div>
                  <div class="path-line"><span>Плагин</span><strong>{{ activePluginTab.name }} · {{ activePluginTab.id }}</strong></div>
                </section>
                <PluginCenter
                  v-else
                  :plugins="plugins"
                  :runtime-views="pluginViews"
                  :runtime-commands="pluginCommands"
                  :runtime-shortcuts="pluginShortcuts"
                  :github-install-busy="githubPluginInstallBusy"
                  :active="currentView === 'plugins'"
                  @toggle="togglePlugin"
                  @install-archive="installPluginArchive"
                  @install-github="installGithubPlugin"
                  @uninstall="uninstallPlugin"
                  @update-grants="updatePluginGrants"
                  @allow-local-execution="allowLocalPluginExecution"
                  @pin-publisher="pinPluginPublisher"
                  @unpin-publisher="unpinPluginPublisher"
                  @set-revocation-authority="setPluginPublisherRevocationAuthority"
                  @import-revocations="importPluginRevocations"
                  @revoke="revokePlugin"
                  @unrevoke="unrevokePlugin"
                  @run-view-action="runPluginAction"
                  @run-command="runPluginCommand"
                  @save-view-config="savePluginConfig"
                  @open-view-link="openPluginLink"
                  @toast="showToast"
                />
              </section>

              <section data-page-view="themes" class="page-content">
               <ThemeStudio
                 :appearance="appearanceDraft"
                 :tokens="effectiveColorTokens"
                 :theme="currentTheme"
                 :base-theme="resolvedGlobalTheme"
                 :font-family="settingsDraft.font_family"
                 :pages="customizablePages"
                 :wallpaper-url="wallpaperDisplayUrl"
                 :default-color-tokens="defaultColorTokens"
                 :default-radius-tokens="defaultRadiusTokens"
                 :light-tokens="lightColorTokens"
                 :save-state="settingsSaveState"
                 :save-message="settingsSaveMessage"
                 :busy="busy"
                 @update:font-family="settingsDraft.font_family = $event"
                 @save="void saveSettings()"
                 @reset-appearance="resetAppearance"
                 @open-page="setView"
                 @import-wallpaper="importWallpaper"
                 @clear-wallpaper="clearWallpaper"
                 @commit-wallpaper-blur="commitWallpaperBlur"
               />
               <ThemeGallery
                 :themes="availableThemes"
                 :active-theme-id="activeThemeId"
                 :busy="themeOperationBusy"
                 :github-install-busy="githubThemeInstallBusy"
                 @apply="applyTheme"
                 @save="saveCurrentTheme"
                 @import="importThemeArchive"
                 @export="exportLocalTheme"
                 @remove="deleteLocalTheme"
                 @install-github="installGithubTheme"
                 @toast="showToast"
               />
             </section>

             <section data-page-view="settings" class="page-content settings-page">
               <div class="section-intro">
                 <div>
                   <span class="eyebrow">Параметры лаунчера</span>
                   <h2>Настройки</h2>
                   <p>Пути, память, источники и резервная копия файлов настраиваются здесь.</p>
                 </div>
                 <div class="section-intro-actions">
                   <button class="button button--outline" type="button" @click="replayLauncherTour"><Icon name="refresh" :size="15" />Повторить экскурсию</button>
                   <button class="button button--outline" type="button" @click="setView('themes')"><Icon name="palette" :size="15" />Центр настройки тем</button>
                 </div>
               </div>
               <div class="settings-save-bar" :class="`settings-save-bar--${settingsSaveState}`">
                 <div class="settings-save-copy"><span class="status-dot" :class="{ 'status-dot--green': settingsSaveState === 'saved', 'status-dot--busy': settingsSaveState === 'saving' }" /><div><strong>{{ settingsSaveState === 'saving' ? 'Сохраняю настройки' : settingsSaveState === 'error' ? 'Нужно исправление' : settingsSaveState === 'dirty' ? 'Есть несохранённые изменения' : 'Настройки сохранены' }}</strong><small>{{ settingsSaveMessage }}</small></div></div>
                 <button class="button button--accent" type="button" :disabled="busy" @click="void saveSettings()"><Icon name="check" :size="15" />{{ settingsSaveState === 'saving' ? 'Сохраняю…' : 'Сохранить изменения' }}</button>
               </div>
               <nav class="settings-section-nav" aria-label="Разделы настроек">
                 <button type="button" @click="scrollSettingsSection('settings-system')">Среда запуска</button>
                 <button type="button" @click="scrollSettingsSection('settings-behavior')">Поведение</button>
                 <button type="button" @click="scrollSettingsSection('settings-hotkeys')">Горячие клавиши</button>
                 <button type="button" @click="scrollSettingsSection('settings-overlay')">Оверлей</button>
                 <button type="button" @click="scrollSettingsSection('settings-metrics')">Метрики</button>
                 <button type="button" @click="scrollSettingsSection('settings-sources')">Источники</button>
                 <button type="button" @click="scrollSettingsSection('settings-profiles')">Профили запуска</button>
                 <button type="button" @click="scrollSettingsSection('settings-catalogs')">Каталоги</button>
                 <button type="button" @click="scrollSettingsSection('settings-cloud')">Облако</button>
               </nav>
               <!-- Плитки настроек раскладываются колонками (masonry): короткие
                    плитки встают под длинными, без пустых дыр справа. Каждая
                    плитка — одна тема, а не «всё подряд в одной карточке». -->
               <div class="settings-masonry">
                 <section id="settings-system" class="panel settings-card">
                   <div class="panel-heading"><div><span class="eyebrow">Система</span><h2>Среда запуска</h2></div></div>
                   <div class="settings-form">
                     <div class="field"><span>Язык интерфейса</span><CustomSelect :model-value="settingsDraft.language ?? 'ru'" :options="languageOptions" @update:model-value="setLanguageFromSettings" /></div>
                     <label class="field"><span>Папка игры</span><input v-model="settingsDraft.game_dir" :placeholder="defaultGameDirectory" /></label>
                     <div class="field">
                       <span>Java runtime <button class="text-button" type="button" @click="refreshJavaOptions">Обновить список</button></span>
                       <CustomSelect v-model="javaSelection" :options="javaChoiceOptions" />
                     </div>
                     <div class="field"><span>Параллельные загрузки</span><NumericStepper v-model="settingsDraft.max_downloads" :min="1" :max="32" aria-label="Параллельные загрузки" /></div>
                     <label v-if="javaSelection === 'manual'" class="field field--wide">
                       <span>Путь к Java executable <small>можно указать JDK/JRE или бинарник</small></span>
                       <input v-model="settingsDraft.java_path" placeholder="/usr/lib/jvm/.../bin/java" />
                       <div class="java-path-actions"><button class="button button--small button--quiet" type="button" @click="validateJavaPath">Проверить Java</button><span v-if="javaValidation" class="field-help">{{ javaValidation }}</span></div>
                     </label>
                     <div class="field field--wide memory-control">
                       <span>Память JVM <small>максимум {{ memoryTotalLabel }}</small></span>
                       <div class="memory-input-row">
                         <NumericStepper :model-value="memoryValue" :min="1" :max="memoryMaxValue || 8192" :step="memoryUnit === 'GiB' ? 1 : 128" :decimals="memoryUnit === 'GiB' ? 2 : 0" aria-label="Память JVM" @update:model-value="memoryValue = $event; clampMemoryDraft()" />
                         <CustomSelect v-model="memoryUnit" :options="memoryUnitOptions" />
                       </div>
                       <small class="field-help">До {{ memoryMaxValue || '—' }} {{ memoryUnit }} · рекомендуем {{ formatMemoryLabel(state.settings.memory_recommended_kib) }}</small>
                     </div>
                     <label class="field field--wide"><span>Прокси <small>HTTP/HTTPS или SOCKS5, если сеть блокирует каталоги</small></span><input v-model="settingsDraft.http_proxy" placeholder="http://127.0.0.1:порт или socks5://127.0.0.1:1080" /><small class="field-help">SOCKS5 резолвит имена на стороне прокси; авторизация socks5://user:pass@host:port поддерживается.</small></label>
                     <div class="field field--wide">
                       <span>Graphics / GLX</span>
                       <CustomSelect v-model="settingsDraft.graphics_mode" :options="graphicsOptions" />
                       <small class="field-help">Auto включает NVIDIA PRIME, если nvidia-smi видит видеокарту; иначе используется системный GLX. Экземпляры с Auto наследуют эту настройку.</small>
                     </div>
                     <div class="diagnostic-note">
                       <span class="status-dot" :class="{ 'status-dot--green': state.system.nvidia_smi }" />
                       <span>{{ state.system.session }} · {{ state.system.display }} · {{ state.system.nvidia_smi ? 'nvidia-smi найден' : 'nvidia-smi не найден' }}</span>
                       <button class="text-button" type="button" @click="loadGraphicsDiagnostics">Проверить GLX</button>
                     </div>
                     <pre v-if="graphicsReport" class="diagnostic-output">{{ graphicsReport }}</pre>
                   </div>
                 </section>

                 <section id="settings-behavior" class="panel settings-card">
                   <div class="panel-heading"><div><span class="eyebrow">Поведение</span><h2>Запуск и интерфейс</h2></div></div>
                   <div class="settings-form">
                     <div class="field"><span>Экран при запуске</span><CustomSelect v-model="settingsDraft.startup_view" :options="persistentViewOptions" /></div>
                     <div class="field"><span>Последний экран</span><CustomSelect v-model="settingsDraft.last_view" :options="persistentViewOptions" /></div>
                     <div class="field"><span>Версии по умолчанию</span><CustomSelect v-model="settingsDraft.default_version_filter" :options="versionFilterOptions" /></div>
                     <div class="field"><span>Loader по умолчанию</span><CustomSelect v-model="settingsDraft.default_loader" :options="loaderOptions" /></div>
                     <div class="field field--wide"><span>Аккаунт по умолчанию</span><CustomSelect v-model="settingsDraft.default_account_id" :options="accountOptions" /></div>
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="settingsDraft.remember_last_view" type="checkbox" /><span><strong>Запоминать экран</strong><small>Возвращаться на последний открытый раздел</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.open_last_instance" type="checkbox" /><span><strong>Открывать последний экземпляр</strong><small>Сохранять рабочий контекст выбранного профиля</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.launch_after_install" type="checkbox" /><span><strong>Запускать после установки</strong><small>Автоматически открыть готовый экземпляр</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.minimize_on_launch" type="checkbox" /><span><strong>Сворачивать при запуске</strong><small>Скрыть окно лаунчера после старта игры</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.confirm_destructive_actions" type="checkbox" /><span><strong>Подтверждать опасные действия</strong><small>Удаление экземпляров, ресурсов, плагинов и аккаунтов</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.show_activity_panel" type="checkbox" /><span><strong>Показывать панель активности</strong><small>Логи, прогресс и отмена операций</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.show_toasts" type="checkbox" /><span><strong>Показывать уведомления</strong><small>Короткие сообщения о результате действий</small></span></label>
                     </div>
                   </div>
                 </section>

                 <section id="settings-downloads" class="panel settings-card">
                   <div class="panel-heading"><div><span class="eyebrow">Сеть и журналы</span><h2>Загрузки и metadata</h2></div></div>
                   <div class="settings-form">
                     <div class="field"><span>Обновление metadata, часы</span><NumericStepper v-model="settingsDraft.metadata_refresh_hours" :min="1" :max="168" aria-label="Обновление metadata" /></div>
                     <div class="field"><span>Timeout загрузки, секунд</span><NumericStepper v-model="settingsDraft.download_timeout_seconds" :min="5" :max="120" aria-label="Timeout загрузки" /></div>
                     <div class="field"><span>Повторные попытки загрузки</span><NumericStepper v-model="settingsDraft.download_retries" :min="1" :max="6" aria-label="Повторные попытки загрузки" /></div>
                     <div class="field"><span>Хранить логи, дней <small>0 = без лимита</small></span><NumericStepper v-model="settingsDraft.keep_logs_days" :min="0" :max="365" aria-label="Хранение логов" /></div>
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="settingsDraft.auto_refresh_metadata" type="checkbox" /><span><strong>Обновлять metadata автоматически</strong><small>Проверять версии при открытии лаунчера</small></span></label>
                     </div>
                   </div>
                 </section>

                 <section id="settings-updates" class="panel settings-card">
                   <div class="panel-heading"><div><span class="eyebrow">Обновления</span><h2>Самообновление</h2></div></div>
                   <div class="settings-form">
                     <div class="diagnostic-note diagnostic-note--stacked">
                       <span class="status-dot" :class="{ 'status-dot--busy': updaterBusy, 'status-dot--green': !updaterBusy && updaterVersion }" />
                       <div>
                         <div>{{ updaterStatus }}</div>
                         <small v-if="updaterVersion">К установке: {{ updaterVersion }}</small>
                         <small v-if="updaterProgressTotal">{{ updaterProgressDownloaded }} / {{ updaterProgressTotal }} байт</small>
                       </div>
                     </div>
                     <div class="settings-inline-actions">
                       <button class="button button--small button--quiet" type="button" :disabled="updaterBusy" @click="void checkLauncherUpdates()"><Icon name="refresh" :size="14" />{{ updaterBusy ? 'Проверяю…' : 'Проверить обновления' }}</button>
                       <button class="button button--small button--accent" type="button" :disabled="updaterBusy || !updaterVersion" @click="void installLauncherUpdate()"><Icon name="download" :size="14" />Установить</button>
                     </div>
                     <small v-if="updaterNotes" class="field-help">{{ updaterNotes }}</small>
                   </div>
                 </section>

                 <section id="settings-privacy" class="panel settings-card">
                   <div class="panel-heading"><div><span class="eyebrow">Приватность</span><h2>Телеметрия</h2></div></div>
                   <div class="settings-form">
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="settingsDraft.telemetry" type="checkbox" /><span><strong>Телеметрия</strong><small>Выключено по умолчанию</small></span></label>
                     </div>
                     <label class="field field--wide"><span>Crash-report endpoint <small>Sentry-compatible webhook или self-hosted collector</small></span><input v-model="settingsDraft.telemetry_endpoint" :disabled="!settingsDraft.telemetry" placeholder="https://errors.example.com/api/nivora-launcher/crash" /><small class="field-help">Отправляется только при включённой телеметрии и только для падений/неуспешных завершений Minecraft.</small></label>
                   </div>
                 </section>

                 <section id="settings-hotkeys" class="panel settings-card">
                   <div class="panel-heading"><div><span class="eyebrow">Управление</span><h2>Горячие клавиши</h2></div></div>
                   <div class="settings-form">
                     <!-- Общие клавиши — тем же рекордером, что и клавиши оверлея:
                          свободный текст молча ломал клавиши. -->
                     <div class="hotkey-grid">
                       <label v-for="shortcut in shortcutRows" :key="shortcut.key" class="field"><span>{{ shortcut.label }}</span><HotkeyInput v-model="settingsDraft.keyboard_shortcuts[shortcut.key]" :default-hint="`по умолчанию ${shortcut.default}`" /></label>
                     </div>
                     <small class="field-help">Кликните по полю и нажмите нужную комбинацию — ввести текст руками нельзя.</small>
                   </div>
                 </section>

                 <section id="settings-backups" class="panel settings-card">
                   <div class="panel-heading"><div><span class="eyebrow">Миры</span><h2>Резервные копии по умолчанию</h2></div></div>
                   <div class="settings-form">
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="settingsDraft.backup_schedule.enabled" type="checkbox" /><span><strong>Копировать миры по расписанию</strong><small>Расписание для всех экземпляров; в экземпляре можно переопределить</small></span></label>
                     </div>
                     <div class="field"><span>Интервал, часов</span><NumericStepper v-model="settingsDraft.backup_schedule.interval_hours" :min="1" :max="720" :disabled="!settingsDraft.backup_schedule.enabled" aria-label="Интервал копий" /></div>
                     <div class="field"><span>Хранить копий</span><NumericStepper v-model="settingsDraft.backup_schedule.keep" :min="1" :max="50" :disabled="!settingsDraft.backup_schedule.enabled" aria-label="Количество копий" /></div>
                     <div class="field field--wide"><span>Куда</span><CustomSelect v-model="settingsDraft.backup_schedule.destination" :options="backupDestinationOptions" :disabled="!settingsDraft.backup_schedule.enabled" /></div>
                     <small class="field-help">Копируется только игровое содержимое (saves/ или world*), запущенные экземпляры пропускаются и копируются на следующем цикле. WebDAV использует облачный endpoint из раздела облака.</small>
                   </div>
                 </section>

                 <section id="settings-presets" class="panel settings-card">
                   <div class="panel-heading"><div><span class="eyebrow">Пресеты</span><h2>Профили настроек</h2></div></div>
                   <div class="settings-form">
                     <div class="settings-profiles">
                       <div v-for="profile in settingsProfiles" :key="profile.name" class="settings-profile-row">
                         <span><strong>{{ profile.name }}</strong><small>{{ profile.key_count }} параметров · {{ profile.updated_at ? formatDateShort(profile.updated_at) : '—' }}</small></span>
                         <span>
                           <button class="button button--small button--quiet" type="button" :disabled="settingsProfileBusy" @click="applySettingsProfile(profile.name)">Применить</button>
                           <button class="button button--small button--quiet" type="button" :disabled="settingsProfileBusy" @click="overwriteSettingsProfile(profile.name)">Обновить</button>
                           <button class="button button--small button--quiet" type="button" :disabled="settingsProfileBusy" @click="exportSettingsProfile(profile.name)">Экспорт</button>
                           <button class="icon-button" type="button" aria-label="Удалить профиль" :disabled="settingsProfileBusy" @click="deleteSettingsProfile(profile.name)"><Icon name="trash" :size="14" /></button>
                         </span>
                       </div>
                       <span v-if="!settingsProfiles.length && !settingsProfilesLoading" class="muted">Профилей пока нет — сохраните текущие настройки как пресет.</span>
                       <span v-if="settingsProfilesLoading" class="muted">Загружаю…</span>
                       <div class="settings-profile-actions">
                         <input v-model="settingsProfileName" class="settings-profile-name" placeholder="Имя профиля" maxlength="48" />
                         <button class="button button--small button--accent" type="button" :disabled="settingsProfileBusy || !settingsProfileName.trim()" @click="saveSettingsProfileAs">{{ settingsProfileBusy ? 'Работаю…' : 'Сохранить пресет' }}</button>
                         <button class="button button--small button--outline" type="button" :disabled="settingsProfileBusy" @click="importSettingsProfile">Импорт YML</button>
                       </div>
                       <small class="field-help">Пресет — снимок всех настроек лаунчера, кроме папки игры и client_id: внешность, сеть, профили запуска. Экспорт отдаёт JSON-файл, импорт принимает его же.</small>
                     </div>
                   </div>
                 </section>

                 <section id="settings-overlay" class="panel settings-card">
                   <div class="panel-heading"><div><span class="eyebrow">В игре</span><h2>Игровой оверлей и запись</h2></div></div>
                   <div class="settings-form">
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="settingsDraft.overlay_settings.enabled" type="checkbox" /><span><strong>Оверлей в игре</strong><small>{{ settingsDraft.overlay_settings.hotkey_overlay }} открывает оверлей поверх Minecraft</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.overlay_settings.show_mods" type="checkbox" :disabled="!settingsDraft.overlay_settings.enabled" /><span><strong>Виджет модов</strong><small>Список модов экземпляра в оверлее</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.overlay_settings.show_metrics" type="checkbox" :disabled="!settingsDraft.overlay_settings.enabled" /><span><strong>Виджет метрик</strong><small>CPU, RAM и диск игры</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.overlay_settings.recording_audio" type="checkbox" :disabled="!settingsDraft.overlay_settings.enabled" /><span><strong>Записывать звук</strong><small>Системный звук через PulseAudio</small></span></label>
                     </div>
                     <div class="hotkey-grid">
                       <label class="field"><span>Клавиша оверлея</span><HotkeyInput v-model="settingsDraft.overlay_settings.hotkey_overlay" :disabled="!settingsDraft.overlay_settings.enabled" default-hint="по умолчанию Shift+Tab" /></label>
                       <label class="field"><span>Клавиша записи</span><HotkeyInput v-model="settingsDraft.overlay_settings.hotkey_recording" :disabled="!settingsDraft.overlay_settings.enabled" default-hint="по умолчанию F7" /></label>
                     </div>
                     <div class="field"><span>Кодировщик</span><CustomSelect v-model="settingsDraft.overlay_settings.recording_encoder" :options="recordingEncoderOptions" :disabled="!settingsDraft.overlay_settings.enabled" /></div>
                     <div class="field"><span>Контейнер</span><CustomSelect v-model="settingsDraft.overlay_settings.recording_container" :options="recordingContainerOptions" :disabled="!settingsDraft.overlay_settings.enabled" /></div>
                     <div class="field"><span>FPS записи</span><NumericStepper v-model="settingsDraft.overlay_settings.recording_fps" :min="15" :max="120" :disabled="!settingsDraft.overlay_settings.enabled" aria-label="FPS записи" /></div>
                     <div class="field"><span>Качество <small>CRF/CQ: меньше — лучше</small></span><NumericStepper v-model="settingsDraft.overlay_settings.recording_quality" :min="15" :max="35" :disabled="!settingsDraft.overlay_settings.enabled" aria-label="Качество записи" /></div>
                     <!-- Диагностика оверлея: пошаговый отчёт вместо «не открывается». -->
                     <div class="overlay-selftest">
                       <button class="button button--small button--outline" type="button" :disabled="overlaySelfTestBusy || !settingsDraft.overlay_settings.enabled" @click="() => { void runOverlaySelfTest() }"><Icon name="info" :size="14" />{{ overlaySelfTestBusy ? 'Проверяю…' : 'Диагностика оверлея' }}</button>
                       <pre v-if="overlaySelfTestReport.length" class="selftest-report">{{ overlaySelfTestReport.map((step) => `${step.ok ? '✓' : '✗'} ${step.name} — ${step.detail}`).join('\n') }}</pre>
                     </div>
                     <small class="field-help">Глобальные клавиши не работают в нативных Wayland-сессиях (в пакетной сборке лаунчер работает через XWayland — там клавиши работают) и в эксклюзивном полноэкранном режиме игры: там оверлей открывается кнопкой «Оверлей» в шапке и пунктом «Игровой оверлей» в меню значка Nivora в трее.</small>
                   </div>
                 </section>

                 <section id="settings-metrics" class="panel settings-card">
                   <div class="panel-heading"><div><span class="eyebrow">Оверлей</span><h2>Системные метрики и виджеты</h2></div></div>
                   <div class="settings-form">
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="settingsDraft.overlay_settings.show_system_metrics" type="checkbox" :disabled="!settingsDraft.overlay_settings.enabled" /><span><strong>Системные метрики</strong><small>CPU, GPU, RAM, диски и температуры</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.overlay_settings.metrics_cpu" type="checkbox" :disabled="!settingsDraft.overlay_settings.enabled || !settingsDraft.overlay_settings.show_system_metrics" /><span><strong>CPU</strong><small>Загрузка и по ядрам</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.overlay_settings.metrics_ram" type="checkbox" :disabled="!settingsDraft.overlay_settings.enabled || !settingsDraft.overlay_settings.show_system_metrics" /><span><strong>RAM</strong><small>Память и подкачка</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.overlay_settings.metrics_gpu" type="checkbox" :disabled="!settingsDraft.overlay_settings.enabled || !settingsDraft.overlay_settings.show_system_metrics" /><span><strong>GPU</strong><small>NVIDIA через nvidia-smi</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.overlay_settings.metrics_disk" type="checkbox" :disabled="!settingsDraft.overlay_settings.enabled || !settingsDraft.overlay_settings.show_system_metrics" /><span><strong>Диски</strong><small>Заполнение разделов</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.overlay_settings.metrics_temperatures" type="checkbox" :disabled="!settingsDraft.overlay_settings.enabled || !settingsDraft.overlay_settings.show_system_metrics" /><span><strong>Температуры</strong><small>Linux hwmon, NVIDIA</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.overlay_settings.show_clock" type="checkbox" :disabled="!settingsDraft.overlay_settings.enabled" /><span><strong>Часы</strong><small>Текущее время в оверлее</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.overlay_settings.show_chat" type="checkbox" :disabled="!settingsDraft.overlay_settings.enabled" /><span><strong>Быстрый чат сервера</strong><small>Команды и хвост консоли</small></span></label>
                       <label class="workspace-check"><input v-model="settingsDraft.overlay_settings.compact" type="checkbox" :disabled="!settingsDraft.overlay_settings.enabled" /><span><strong>Компактный режим</strong><small>Меньше отступов</small></span></label>
                     </div>
                     <div class="field"><span>Позиция оверлея</span><CustomSelect v-model="settingsDraft.overlay_settings.position" :options="overlayPositionOptions" :disabled="!settingsDraft.overlay_settings.enabled" /></div>
                     <div class="field"><span>Метрики вне игры</span><CustomSelect v-model="settingsDraft.overlay_settings.system_metrics_display" :options="systemMetricsDisplayOptions" /></div>
                     <div class="field"><span>Масштаб, %</span><NumericStepper v-model="settingsDraft.overlay_settings.scale" :min="80" :max="130" :disabled="!settingsDraft.overlay_settings.enabled" aria-label="Масштаб оверлея" /></div>
                     <div class="field"><span>Непрозрачность, %</span><NumericStepper v-model="settingsDraft.overlay_settings.opacity" :min="60" :max="100" :disabled="!settingsDraft.overlay_settings.enabled" aria-label="Непрозрачность оверлея" /></div>
                     <small class="field-help">Метрики вне игры: «Постоянно» показывает CPU/RAM/GPU в верхней панели лаунчера, «Только оверлей» — лишь по горячей клавише. Датчики и GPU зависят от системы: чего нет — не показываем.</small>
                   </div>
                 </section>

                 <section id="settings-sources" class="panel settings-card">
                   <div class="panel-heading"><div><span class="eyebrow">Интеграции</span><h2>Вход и данные версий</h2></div></div>
                   <div class="settings-form">
                     <div class="automatic-integration"><span class="status-dot" :class="{ 'status-dot--green': settingsDraft.login_source === 'microsoft' && state.settings.has_client_id }" /><div><strong>Microsoft OAuth</strong><span>{{ settingsDraft.login_source !== 'microsoft' ? 'Внешний вход отключён.' : state.settings.has_client_id ? 'Public client ID настроен в сборке; вручную вводить его не нужно.' : 'В этой сборке нет настроенного client ID; он должен поставляться вместе с лаунчером.' }}</span></div></div>
                     <div class="field"><span>Источник входа</span><CustomSelect v-model="settingsDraft.login_source" :options="loginSourceOptions" /></div>
                     <div class="field"><span>Источник данных</span><CustomSelect v-model="settingsDraft.data_source" :options="dataSourceOptions" /></div>
                     <label class="field field--wide"><span>Login API URL <small>база device-code и token endpoint</small></span><input v-model="settingsDraft.login_api_url" placeholder="https://login.microsoftonline.com/consumers/oauth2/v2.0" /></label>
                     <label class="field field--wide"><span>Data API URL <small>{{ settingsDraft.data_source === 'custom' ? 'совместимый version manifest' : 'официальный Mojang manifest по умолчанию' }}</small></span><input v-model="settingsDraft.data_api_url" placeholder="https://piston-meta.mojang.com/mc/game/version_manifest_v2.json" /></label>
                     <div class="settings-toggle-grid">
                       <label class="workspace-check"><input v-model="settingsDraft.developer_mode" type="checkbox" /><span><strong>Режим разработчика</strong><small>Разрешает нестандартные HTTPS endpoint. Не включайте на рабочем аккаунте: токены OAuth могут уйти на указанный адрес.</small></span></label>
                     </div>
                     <div class="security-note"><Icon name="info" :size="18" /><span>Для распространения лаунчер должен содержать собственный Azure public-client ID. Это публичная настройка сборки, а не секрет и не поле для пользователя.</span></div>
                   </div>
                 </section>

                 <section v-if="settingsSectionViews.length" id="settings-plugins" class="panel settings-card">
                   <div class="panel-heading"><div><span class="eyebrow">Плагины</span><h2>Настройки плагинов</h2></div></div>
                   <div class="plugin-settings-sections">
                     <article v-for="entry in settingsSectionViews" :key="`${entry.pluginId}:${entry.view.id}`" class="plugin-settings-section">
                       <div class="workspace-card-heading"><div><h3>{{ entry.view.title }}</h3><span class="muted">{{ entry.pluginName }} · UI v2</span></div></div>
                       <PluginViewRenderer
                         :view="entry.view"
                         @action="(payload) => runPluginAction({ pluginId: entry.pluginId, ...payload })"
                         @config="(payload) => void savePluginConfig({ pluginId: entry.pluginId, key: payload.key, value: payload.value })"
                         @open-link="(url) => void openPluginLink({ pluginId: entry.pluginId, url })"
                       />
                     </article>
                   </div>
                 </section>
               </div>

               <section id="settings-profiles" class="panel settings-card settings-card--wide">
                 <div class="panel-heading">
                   <div><span class="eyebrow">Профили запуска</span><h2>Параметры игры</h2><p>Сохраните отдельные наборы памяти, Java, графики, окна и аргументов. Активный профиль применяется к кнопке «Играть».</p></div>
                   <button class="button button--small button--outline" type="button" @click="addLauncherProfile"><Icon name="plus" :size="14" />Новый профиль</button>
                 </div>
                 <div class="launcher-profile-toolbar">
                   <div class="field"><span>Активный профиль <small>{{ activeLauncherProfile?.name || 'не выбран' }}</small></span><CustomSelect v-model="settingsDraft.active_launcher_profile_id" :options="settingsDraft.launcher_profiles.map((profile) => ({ value: profile.id, label: profile.name, meta: `${profile.memory} · ${profile.graphics_mode}` }))" /></div>
                 </div>
                 <div class="launcher-profile-list">
                   <article v-for="profile in settingsDraft.launcher_profiles" :key="profile.id" class="launcher-profile-card" :class="{ 'launcher-profile-card--active': profile.id === settingsDraft.active_launcher_profile_id }">
                     <div class="launcher-profile-heading"><div><span class="profile-active-dot" :class="{ 'profile-active-dot--on': profile.id === settingsDraft.active_launcher_profile_id }" /><strong>{{ profile.name }}</strong><small>{{ profile.id }}</small></div><div class="launcher-profile-actions"><button class="button button--small button--quiet" type="button" @click="settingsDraft.active_launcher_profile_id = profile.id">Использовать</button><button v-if="settingsDraft.launcher_profiles.length > 1" class="icon-button" type="button" aria-label="Удалить профиль запуска" @click="removeLauncherProfile(profile.id)"><Icon name="trash" :size="15" /></button></div></div>
                     <div class="field-grid">
                       <label class="field"><span>Название</span><input v-model="profile.name" maxlength="80" /></label>
                       <div class="field"><span>Память JVM</span><MemoryField v-model="profile.memory" :max-kib="state.settings.memory_total_kib" /></div>
                       <label class="field"><span>Java executable</span><input v-model="profile.java_path" placeholder="Пусто = системная Java" /></label>
                       <div class="field"><span>Графика</span><CustomSelect v-model="profile.graphics_mode" :options="graphicsOptions" /></div>
                       <div class="field"><span>Аккаунт</span><CustomSelect v-model="profile.account_id" :options="accountOptions" /></div>
                       <div class="field"><span>Ширина окна</span><NumericStepper v-model="profile.window_width" :min="320" :max="8192" aria-label="Ширина окна" /></div>
                       <div class="field"><span>Высота окна</span><NumericStepper v-model="profile.window_height" :min="240" :max="8192" aria-label="Высота окна" /></div>
                       <label class="workspace-check"><input v-model="profile.fullscreen" type="checkbox" /><span><strong>Полноэкранный режим</strong><small>Добавляет --fullscreen к запуску.</small></span></label>
                       <label class="field"><span>Quick Play: мир</span><input v-model="profile.quick_play_world" placeholder="Имя сохранения" /></label>
                       <label class="field"><span>Quick Play: сервер</span><input v-model="profile.quick_play_server" placeholder="host:port" /></label>
                       <div class="field field--wide"><span>JVM-пресет оптимизации</span>
                         <CustomSelect :model-value="profile.jvm_preset ?? 'none'" :options="jvmPresetOptions" @update:model-value="profile.jvm_preset = $event === 'none' ? undefined : $event" />
                       </div>
                       <label class="field field--wide"><span>Дополнительные JVM-аргументы <small>по одному на строку</small></span><textarea :value="profile.jvm_arguments.join('\n')" rows="2" @input="setProfileArguments(profile, 'jvm_arguments', $event)" /></label>
                       <label class="field field--wide"><span>Дополнительные game-аргументы <small>по одному на строку</small></span><textarea :value="profile.game_arguments.join('\n')" rows="2" @input="setProfileArguments(profile, 'game_arguments', $event)" /></label>
                     </div>
                   </article>
                 </div>
               </section>

               <section id="settings-catalogs" class="panel settings-card settings-card--wide">
                 <div class="panel-heading"><div><span class="eyebrow">Интеграции</span><h2>Каталоги</h2></div></div>
                 <div class="settings-catalog-columns">
                   <div class="settings-catalog-block">
                     <div class="source-divider"><span>Каталоги ресурсов</span><i /></div>
                     <div class="resource-catalog-toolbar">
                       <div class="field"><span>Активный каталог <small>используется в поиске и установке</small></span><CustomSelect v-model="settingsDraft.active_catalog_id" :options="resourceCatalogOptions" @update:model-value="selectActiveCatalog" /></div>
                       <button class="button button--small button--outline" type="button" @click="addResourceCatalog"><Icon name="plus" :size="14" />Добавить каталог</button>
                     </div>
                     <div class="resource-catalog-list">
                       <article v-for="catalog in settingsDraft.resource_catalogs" :key="catalog.id" class="resource-catalog-row">
                         <div class="field"><span class="field-heading-with-icon"><span>Название</span><CatalogBadge :kind="catalog.kind" compact /></span><input v-model="catalog.name" maxlength="60" /></div>
                         <div class="field"><span>Тип</span><CustomSelect v-model="catalog.kind" :options="catalogKindOptions" /></div>
                         <div class="field resource-catalog-url"><span>API URL <small>только HTTPS</small></span><input v-model="catalog.api_url" placeholder="https://api.modrinth.com/v2" /></div>
                         <button class="icon-button" type="button" aria-label="Удалить каталог" @click="removeResourceCatalog(catalog.id)"><Icon name="trash" :size="15" /></button>
                       </article>
                     </div>
                   </div>
                   <div class="settings-catalog-block">
                     <div class="source-divider"><span>Каталоги плагинов серверов</span><i /></div>
                     <div class="resource-catalog-toolbar"><div class="field"><span>Активный каталог <small>используется в server workspace</small></span><CustomSelect v-model="settingsDraft.active_plugin_catalog_id" :options="settingsDraft.plugin_catalogs.map((catalog) => ({ value: catalog.id, label: catalog.name, meta: catalog.kind }))" /></div><button class="button button--small button--outline" type="button" @click="addPluginCatalog"><Icon name="plus" :size="14" />Добавить каталог</button></div>
                     <div class="resource-catalog-list">
                       <article v-for="catalog in settingsDraft.plugin_catalogs" :key="catalog.id" class="resource-catalog-row">
                         <div class="field"><span>Название</span><input v-model="catalog.name" maxlength="80" /></div>
                         <div class="field"><span>Тип</span><CustomSelect v-model="catalog.kind" :options="pluginCatalogKindOptions" /></div>
                         <div class="field resource-catalog-url"><span>API URL <small>только HTTPS</small></span><input v-model="catalog.api_url" placeholder="https://api.modrinth.com/v2" /></div>
                         <label class="workspace-check"><input v-model="catalog.enabled" type="checkbox" /><span>Вкл.</span></label>
                         <button class="icon-button" type="button" aria-label="Удалить каталог плагинов" @click="removePluginCatalog(catalog.id)"><Icon name="trash" :size="15" /></button>
                       </article>
                     </div>
                   </div>
                 </div>
               </section>

               <div id="settings-cloud" class="settings-cloud-anchor">
                 <CloudStoragePanel
                   :url="settingsDraft.cloud_url"
                   :username="settingsDraft.cloud_username"
                   :remote-path="settingsDraft.cloud_remote_path"
                   :include-accounts="settingsDraft.cloud_include_accounts"
                   @saved="Object.assign(settingsDraft, $event)"
                   @toast="showToast"
                   @changed="refreshBootstrap"
                 />
               </div>
               <section class="panel paths-panel">
                 <div class="path-line"><span>Платформа</span><strong>{{ state.system.platform }} · {{ state.system.machine }}</strong></div>
                 <div class="path-line"><span>Python backend</span><strong>{{ state.system.python || 'подключение…' }}</strong></div>
                 <button class="button button--quiet" type="button" @click="openGameFolder"><Icon name="folder" :size="16" />Открыть папку игры</button>
               </section>
             </section>
          </div>
        </div>
       </main>

       <!-- Первый запуск: выбор языка. Кандидаты — языки, для которых есть
            словарь (см. src/i18n.ts). -->

              <div v-if="datapackDialogOpen" class="modal-scrim" role="presentation" @click.self="closeDatapackDialog"><section ref="datapackDialogElement" class="modal-card" role="dialog" tabindex="-1" aria-modal="true" aria-labelledby="datapack-dialog-title" @keydown="handleDatapackDialogKeydown"><div class="panel-heading"><div><span class="eyebrow">DATAPACK INSTALL</span><h2 id="datapack-dialog-title">Выберите мир</h2><p>{{ pendingDatapack?.title || pendingDatapack?.name }}</p></div><button class="icon-button" type="button" aria-label="Закрыть" @click="closeDatapackDialog"><Icon name="close" :size="17" /></button></div><div class="datapack-world-list"><label v-for="world in datapackWorlds" :key="world.id" class="datapack-world-option" :class="{ 'datapack-world-option--active': datapackWorldId === world.id }"><input v-model="datapackWorldId" type="radio" :value="world.id" /><span class="world-icon world-icon--empty"><img v-if="world.icon" :src="world.icon" alt="" /><Icon v-else name="instances" :size="16" /></span><span><strong>{{ world.name }}</strong><small>{{ world.id }}</small></span></label></div><div class="modal-actions"><button class="button button--quiet" type="button" @click="closeDatapackDialog">Отмена</button><button class="button button--accent" type="button" :disabled="modBusy || !datapackWorldId" @click="confirmDatapackInstall"><Icon name="download" :size="15" />Установить в мир</button></div></section></div>
              <div v-if="modpackPreviewOpen" class="modal-scrim" role="presentation" @click.self="closeModpackPreview(false)"><section class="modal-card modpack-preview-card" role="dialog" aria-modal="true" aria-labelledby="modpack-preview-title">
                <div class="panel-heading">
                  <div>
                    <span class="eyebrow">MODPACK PREVIEW</span>
                    <h2 id="modpack-preview-title">Моды в модпаке</h2>
                    <p>{{ modpackPreviewTitle }} · {{ modpackPreviewTotal }} мод(ов) будет установлено</p>
                  </div>
                  <button class="icon-button" type="button" aria-label="Закрыть" @click="closeModpackPreview(false)"><Icon name="close" :size="17" /></button>
                </div>
                <div v-if="modpackPreviewItems.length > 12" class="search-bar modpack-preview-search"><Icon name="search" :size="16" /><input v-model="modpackPreviewQuery" placeholder="Поиск по названию мода" /></div>
                <ul class="modpack-preview-list">
                  <li v-for="item in modpackPreviewFiltered" :key="item.name" class="modpack-preview-item">
                    <span class="world-icon world-icon--empty"><Icon name="archive" :size="14" /></span>
                    <span class="modpack-preview-name">{{ item.name }}</span>
                    <span class="modpack-preview-source">{{ modpackPreviewSourceLabel(item.source) }}</span>
                    <span v-if="item.optional" class="modpack-preview-optional">необязательный</span>
                  </li>
                </ul>
                <p v-if="modpackPreviewTruncated" class="modpack-preview-more">Показаны первые {{ modpackPreviewItems.length }} из {{ modpackPreviewTotal }} — остальное увидите в каталоге модов после импорта.</p>
                <div class="modal-actions">
                  <button class="button button--quiet" type="button" @click="closeModpackPreview(false)">Отмена</button>
                  <button class="button button--accent" type="button" @click="closeModpackPreview(true)"><Icon name="download" :size="15" />Импортировать</button>
                </div>
              </section></div>
      <Transition name="catalog-details">
        <CatalogDetailsDialog v-if="catalogDetailsOpen" :details="catalogDetails" :versions="catalogDetailsVersions" :project-type="catalogDetailsType" :catalog-name="activeCatalog?.name || 'Каталог'" :loading="catalogDetailsLoading" :error="catalogDetailsError" :can-install="catalogDetailsType === 'modpack' || Boolean(selectedInstance)" :installed="Boolean(catalogDetailsProject?.installed)" :busy="modBusy" @close="closeCatalogDetails" @install="installFromCatalogDetails" />
      </Transition>
     <ActivityPanel :activity="activity" @close="activity.open = false" @cancel="cancelActivity" />
      <Teleport to="body">
         <Transition name="toast"><div v-if="toast" :key="toastId" class="toast-message" :style="toastThemeStyle" role="status" aria-live="polite"><div class="toast-copy"><span class="status-dot" :class="toastToneClass" />{{ toast }}</div><div class="toast-progress" aria-hidden="true"><span /></div></div></Transition>
      </Teleport>
      <div v-if="initializing" class="boot-overlay"><BrandLogo class="boot-logo" label="Nivora Launcher" /><span>Подключаю launcher…</span></div>
      <FirstRunExperience
        v-if="firstRunReady"
        :phase="firstRunPhase"
        :accounts="state.accounts"
        :selected-account-id="selectedAccount"
        :busy="busy"
        :microsoft-login-available="state.settings.has_client_id"
        :error="firstRunError"
        :microsoft-code="activity.verification_code || ''"
        :microsoft-verification-uri="activity.verification_uri || ''"
        :game-directory="settingsDraft.game_dir || defaultGameDirectory"
        :memory-gib="firstRunMemoryGiB"
        :recommended-memory-gib="firstRunRecommendedMemoryGiB"
        :maximum-memory-gib="firstRunMaximumMemoryGiB"
        :theme="appearanceDraft.theme"
        :graphics-mode="settingsDraft.graphics_mode"
        @phase-change="setFirstRunPhase"
        @pick-language="pickFirstRunLanguage"
        @select-account="selectFirstRunAccount"
        @add-microsoft="addFirstRunMicrosoftAccount"
        @add-offline="addFirstRunOfflineAccount"
        @save-basics="saveFirstRunBasics"
        @finish-setup="finishFirstRunSetup"
        @skip-setup="skipFirstRunSetup"
        @finish-tour="finishLauncherTour"
        @skip-tour="finishLauncherTour"
      />
  </div>
</template>
