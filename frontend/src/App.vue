<script setup lang="ts">
import { computed, defineAsyncComponent, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch, watchEffect } from 'vue'
import ActivityPanel from './components/ActivityPanel.vue'
import AppSidebar from './components/AppSidebar.vue'
import AccountsPage from './components/AccountsPage.vue'
import SettingsPage from './components/SettingsPage.vue'
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
import ModCardMeta from './components/ModCardMeta.vue'
import BlockIconPicker from './components/BlockIconPicker.vue'
import PluginCenter from './components/PluginCenter.vue'
import PluginViewRenderer from './components/PluginViewRenderer.vue'
import ThemeGallery from './components/ThemeGallery.vue'
import ThemeStudio from './components/ThemeStudio.vue'
import { FRONTEND_BUILD_STAMP, bridge } from './services/bridge'
import { createCatalogWorkflow } from './services/catalogWorkflow'
import { chooseAllowedCatalog } from './services/catalogSelection'
import { shouldOpenActivityAfterSessionStart } from './services/activityVisibility'
import { LAUNCHER_VERSION } from './version'
import { composedBlockIcon } from './services/blockIcon'
import { setLocale, type Locale } from './i18n'
import { DEFAULT_OVERLAY_SETTINGS, bindGameHotkeys, initOverlayEventBridge, normalizeHotkey, normalizeOverlayHotkey, overlayLastError, overlayWaylandMode, overlayWindowRequested, setOverlayOpaqueWindow, setOverlayVisible, setOverlayWaylandMode, shouldDetachOverlayAfterMisses, toggleOverlay, toggleRecording, unbindGameHotkeys } from './services/overlay'
import { chooseDialog, confirmDialog, promptDialog } from './services/dialogs'
import { fileBrowserRequest, openInLauncherExplorer, pickFile, pickFiles, resolveFileBrowser, resolveFileBrowserDirectory, resolveFileBrowserMany, type PickedFile } from './services/fileBrowser'
import ContextMenuHost from './components/ContextMenuHost.vue'
import FileBrowserDialog from './components/FileBrowserDialog.vue'
import GlobalSearchDialog from './components/GlobalSearchDialog.vue'
import InstanceCompareDialog from './components/InstanceCompareDialog.vue'
import AutotuneDialog from './components/AutotuneDialog.vue'
import { createPluginController } from './services/pluginController'
import { pluginRuntime } from './services/pluginRuntime'
import { useModalFocus } from './services/modalFocus'
import { initialFirstRunPhase, readFirstRunExperience, writeFirstRunExperience, type FirstRunExperienceState, type FirstRunPhase } from './services/firstRunExperience'
import { createWallpaperRenderer, prepareWallpaperForStorage } from './services/wallpaper'
import DialogHost from './components/DialogHost.vue'
import AutoRepairDialog from './components/AutoRepairDialog.vue'
import { diagnoseLaunchFailure, offerAutoRepair } from './services/autoRepair'
import { nicknameError } from './services/nickname'
import { createModpackWorkflow } from './services/modpackWorkflow'
import { createThemeManager } from './services/themeManager'
import { DEFAULT_BUTTON_CUSTOMIZATION, buttonCustomizationRole, normalizeButtonCustomization, resolveButtonCustomization } from './services/buttonCustomization'
import type { ButtonCustomizationTarget } from './services/buttonCustomization'
import { createSettingsWorkflow } from './services/settingsWorkflow'
const GameOverlay = defineAsyncComponent(() => import('./components/GameOverlay.vue'))
import type {
  PluginEventPayloadMap,
  PluginInstanceRef,
  PluginUiTone,
  PluginUiView,
} from '../../plugins/api'
import type {
  AccountInfo,
  SettingsInfo,
  ActivityState,
  AnimationSettings,
  AppearanceSettings,
  AppState,
  BedrockRuntimeStatus,
  CatalogSort,
  ExternalInstanceCandidate,
  InstanceIconBlock,
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
  SystemMetricsSnapshot,
  BackupSchedule,
  CrashReport,
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
  accent: '#3d8bfd',
  accent_strong: '#6ea8fe',
  background: '#0f1115',
  panel: '#15181d',
  panel_raised: '#1a1e24',
  panel_soft: '#20242c',
  text: '#eceff4',
  muted: '#7d8591',
  quiet: '#b7beca',
  border: '#98a2b3',
  danger: '#e5484d',
  success: '#46a758',
  warning: '#e0a63c',
  banner_shade: '#0f1115',
}

const defaultRadiusTokens: Record<string, number> = {
  card: 12,
  button: 8,
  control: 8,
  sidebar: 0,
}

// старые палитры распознаются при миграции настроек
const legacyDefaultColorTokenValues: Record<string, string[]> = {
  accent: ['#8b653f', '#7aa2ff', '#5b8dd9', '#2f7ff6'],
  accent_strong: ['#c5a274', '#b7c9ff', '#8fb3ea', '#6aa5ff'],
  background: ['#191a17', '#080d16', '#0b1220', '#0f1218', '#121417'],
  panel: ['#20221e', '#101a29', '#111a2c', '#151a22', '#1a1d22'],
  panel_raised: ['#282a25', '#142236', '#16223a', '#1a202a', '#21252c'],
  panel_soft: ['#30322c', '#182941', '#1a2740', '#1f2632', '#262b33'],
  text: ['#e9e8de', '#f1f5ff', '#e8edf7', '#e6eaf2', '#f0f3f7'],
  muted: ['#a2a395', '#8595b0', '#8fa0be', '#8b95a7', '#8a919e'],
  quiet: ['#c7c7bb', '#aebbd1', '#b9c6de', '#b4bdd0', '#b9c0cd'],
  border: ['#77796b', '#92ace2', '#94a8cc', '#8b98b0', '#5f6b7d'],
  danger: ['#d58a7d', '#f47f9a'],
  success: ['#9bac78', '#62d49a'],
  warning: ['#d2ad72', '#f3c76f'],
  banner_shade: ['#191a17', '#080d16', '#0b1220', '#0f1218', '#121417'],
}
function migrateDefaultColors(value?: Record<string, string>): Record<string, string> {
  const colors = { ...defaultColorTokens, ...(value || {}) }
  for (const [key, previousValues] of Object.entries(legacyDefaultColorTokenValues)) {
    const current = colors[key]?.toLowerCase()
    if (current && previousValues.includes(current)) colors[key] = defaultColorTokens[key]
  }
  return colors
}
const lightColorTokens: Record<string, string> = {
  ...defaultColorTokens,
  accent: '#2f6fdd',
  accent_strong: '#1f56bd',
  background: '#f4f6f9',
  panel: '#ffffff',
  panel_raised: '#ffffff',
  panel_soft: '#e9edf3',
  text: '#1a2230',
  muted: '#66707f',
  quiet: '#3d4756',
  border: '#8b96a8',
  danger: '#b04a41',
  success: '#3f7d4e',
  warning: '#8a6420',
  banner_shade: '#f4f6f9',
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
  page_style: 'slide',
  page_duration_ms: 280,
  surface_duration_ms: 240,
  surface_stagger_ms: 30,
  page_styles: { instances: '', 'instance-settings': '', accounts: '', mods: '', modpacks: '', plugins: '', themes: '', settings: '' },
  page_durations_ms: { instances: 280, 'instance-settings': 280, accounts: 280, mods: 280, modpacks: 280, plugins: 280, themes: 280, settings: 280 },
}

const initialState: AppState = {
  instances: [],
  accounts: [],
  settings: {
    game_dir: '',
    java_path: '',
    security_profile: 'strict',
    file_explorer: 'builtin',
    file_editor: 'builtin',
    security_permissions: ['network:game', 'files:instance', 'desktop:display', 'desktop:audio'],
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
    download_source: 'auto',
    confirm_destructive_actions: true,
    auto_repair_enabled: true,
    auto_repair_muted: [],
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
     active_theme_id: 'nivora-dark',
    wallpaper_path: '',
    wallpaper_data: '',
    wallpaper_blur: 12,
    wallpaper_opacity: 34,
    sidebar_texture_data: '',
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
      instance_layout: 'default',
     ui_scale: 100,
     radius_tokens: { card: 7, button: 8, control: 10, sidebar: 0 },
      page_themes: { instances: 'inherit', 'instance-settings': 'inherit', accounts: 'inherit', mods: 'inherit', modpacks: 'inherit', plugins: 'inherit', themes: 'inherit', settings: 'inherit' },
      page_layouts: { instances: 'balanced', 'instance-settings': 'balanced', accounts: 'balanced', mods: 'wide', modpacks: 'wide', plugins: 'wide', themes: 'wide', settings: 'balanced' },
    widget_visibility: { 'instances.workspace': true, 'accounts.status': true, 'mods.context': true },
     color_tokens: { ...defaultColorTokens },
     button_customization: normalizeButtonCustomization(DEFAULT_BUTTON_CUSTOMIZATION),
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
const pluginController = createPluginController({
  plugins,
  settingsDraft: () => settingsDraft,
  appearanceDraft: () => appearanceDraft,
  systemDark: () => systemDark.value,
  selectedInstance: () => selectedInstance.value,
  showToast,
  errorMessage,
  isSafePluginShortcutBinding,
  confirmDestructive,
  openActivity,
  finishActivity,
  currentToastId: () => toastId.value,
  dismissCurrentToast: (id) => {
    if (toastId.value !== id) return
    if (toastTimer) window.clearTimeout(toastTimer)
    toastTimer = undefined
    toastId.value += 1
    toast.value = ''
    toastAction.value = null
  },
})
const {
  pluginViews,
  githubPluginInstallBusy,
  pluginCommands,
  pluginShortcuts,
  replacePluginList,
  clearPluginRuntimeState,
  pluginInstanceRef,
  notifyPluginBeforeLaunch,
  notifyPluginAfterExit,
  notifyPluginCrash,
  notifyPluginSnapshotCreated,
  emitPluginEvent,
  broadcastPluginEvent,
  schedulePluginRuntimeSync,
  runPluginAction,
  runPluginCommand,
  runPluginShortcut,
  savePluginConfig,
  openPluginLink,
  togglePlugin,
  updatePluginGrants,
  allowLocalPluginExecution,
  pinPluginPublisher,
  unpinPluginPublisher,
  setPluginPublisherRevocationAuthority,
  importPluginRevocations,
  revokePlugin,
  unrevokePlugin,
  installPluginArchive,
  installGithubPlugin,
  uninstallPlugin,
} = pluginController

async function beforeInstanceLaunch(instance: PluginInstanceRef): Promise<void> {
  attachOverlayForLaunch(String(instance.name || ''))
  await notifyPluginBeforeLaunch(instance)
}

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
const localThemes = ref<LauncherThemePreset[]>([])
const themeOperationBusy = ref(false)
const githubThemeInstallBusy = ref(false)
const activeThemeId = ref('nivora-dark')
const createInstanceOpen = ref(false)
const creationKind = ref<'client' | 'bedrock' | 'server'>('client')
const bedrockCreationStatus = ref<BedrockRuntimeStatus | null>(null)
const bedrockCreationEdition = ref<'release' | 'preview'>('release')
const bedrockCreationVersion = ref('')
const bedrockCreationVersions = ref<Array<{ edition: string; version: string; installed: boolean }>>([])
const bedrockCreationVersionsBusy = ref(false)
const bedrockCreationVersionsError = ref('')
const serverCoreDragActive = ref(false)
// на узком окне сайдбар открывается в выдвижном режиме
const mobileMenuOpen = ref(false)
const instanceGroupFilter = ref('all')
const initializing = ref(true)
const firstRunReady = ref(false)
const firstRunPhase = ref<FirstRunPhase | null>(null)
const firstRunError = ref('')
const firstRunState = ref<FirstRunExperienceState>({ setupCompleted: false, tourCompleted: false })
const busy = ref(false)
// во время игры интерфейс остаётся доступным
const gameSessionActive = ref(false)
watch(firstRunPhase, (phase) => {
  mobileMenuOpen.value = phase === 'tour' && window.innerWidth <= 760
})
const groupActionBusy = ref(false)
const externalImportBusy = ref(false)
const externalImportScanning = ref(false)
const loadingVersions = ref(false)
const versionError = ref('')
const loadingChoices = ref(false)
const toast = ref('')
// фронтенд и нативное ядро должны быть из одной сборки
const buildStampMismatch = ref<{ frontend: string; backend: string } | null>(null)
const toastId = ref(0)
const toastToneClass = ref('status-dot--green')
// тост с действием (например, "Открыть директорию" у свежесозданного файла)
const toastAction = ref<{ label: string; run: () => void } | null>(null)
const toastDuration = ref(2800)
const pageTransitionPulse = ref(true)
const pageScrollMemory = new Map<string, number>()
const versions = ref<VersionInfo[]>([])
// порядок манифеста не гарантируется (зеркала отдают по-разному): сортируем
// сами от новых к старым, чтобы "первая версия" всегда была свежей
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
const creationIcon = ref('')
const iconBlocks = ref<InstanceIconBlock[]>([])
const iconBlocksLoading = ref(false)

async function ensureIconBlocks() {
  if (iconBlocks.value.length || iconBlocksLoading.value) return
  iconBlocksLoading.value = true
  try {
    const result = await bridge.instanceIconBlocks()
    iconBlocks.value = result.blocks
  } catch {
    iconBlocks.value = []
  } finally {
    iconBlocksLoading.value = false
  }
}
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
const instanceWorkspaceRef = ref<{ openConsoleIfOverview: () => boolean; openSettings: () => void; stop: () => Promise<void>; ensureLoaded?: () => Promise<void>; launch?: () => Promise<void> } | null>(null)
const bedrockWorkspaceRef = ref<{ ensureLoaded: () => Promise<void>; applyPendingVersion: (edition: 'release' | 'preview', version: string) => void; refreshAccountStatus?: () => Promise<void> } | null>(null)
const serverWorkspaceRef = ref<{ openSettings: () => void; stop: () => Promise<void> } | null>(null)
const instanceWorkspaceFocus = ref<'overview' | 'settings' | 'console'>('overview')
const catalogServerCore = ref('paper')
const javaSelection = ref('auto')
const memoryValue = ref(8)
const memoryUnit = ref<MemoryUnit>('GiB')
const accountName = ref('Player')
const accountNameError = computed(() => nicknameError(accountName.value.trim()))
const modQuery = ref('')
const modProjectType = ref<ProjectType>('mod')
const selectedModCategory = ref('')
const modSortMode = ref<CatalogSort>('relevance')
const modResults = ref<ModResult[]>([])
const modSearchPerformed = ref(false)
const modBusy = ref(false)
const modError = ref('')
const modInstallId = ref('')
const modAction = ref<'install' | 'remove'>('install')
const modInstallProgress = ref(0)
const modInstallDetail = ref('')
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
const graphicsReport = ref('')
const systemDark = ref(false)
const appShellRef = ref<HTMLElement | null>(null)
const wallpaperPreviewUrl = ref('')
const wallpaperProcessedUrl = ref('')
const wallpaperLoadFailed = ref(false)
const wallpaperRenderBlur = ref(12)
let wallpaperResizeTimer: number | undefined
// путь для автозаполнения до загрузки настроек
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
  download_source: 'auto',
  confirm_destructive_actions: true,
  auto_repair_enabled: true,
  auto_repair_muted: [] as string[],
  show_activity_panel: true,
  show_toasts: true,
  minimize_on_launch: false,
  language: null as string | null,
    font_family: 'system',
  file_explorer: 'builtin' as 'builtin' | 'system',
  file_editor: 'builtin' as 'builtin' | 'system',
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
const lastLaunchCrash = ref<CrashReport>()
const appearanceDraft = reactive<AppearanceSettings>({
   theme: 'dark',
    active_theme_id: 'nivora-dark',
  wallpaper_path: '',
  wallpaper_data: '',
  wallpaper_blur: 12,
  wallpaper_opacity: 34,
  sidebar_texture_data: '',
    accent_color: defaultColorTokens.accent,
  sidebar_position: 'left',
  sidebar_mode: 'adaptive',
  sidebar_width_mode: 'content',
  sidebar_width: 228,
  sidebar_floating: false,
   sidebar_visibility: 'always',
   animations: 'full',
   density: 'comfortable',
   instance_layout: 'default',
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
  button_customization: normalizeButtonCustomization(DEFAULT_BUTTON_CUSTOMIZATION),
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

const {
  availableThemes,
  languageOptions,
  setLanguageFromSettings,
  fontFamilyCss,
  settingsSaveState,
  settingsSaveMessage,
  beginSettingsHydration,
  finishSettingsHydration,
  resolvedGlobalTheme,
  currentTheme,
  currentLayout,
  effectiveColorTokens,
  effectiveAccent,
  wallpaperUrl,
  wallpaperDisplayUrl,
  accentForeground,
  sidebarWidth,
  sidebarHeight,
  activeLauncherProfile,
  effectiveAnimationStyle,
  effectiveAnimationDuration,
  appStyle,
  toastThemeStyle,
  setInstanceLayout,
  mergeSavedSettingsBaseline,
  updateSavedSettingsBaseline,
  settingsProfiles,
  settingsProfilesLoading,
  settingsProfileBusy,
  settingsProfileName,
  backupDestinationOptions,
  loadSettingsProfiles,
  formatDateShort,
  saveSettingsProfileAs,
  overwriteSettingsProfile,
  applySettingsProfile,
  deleteSettingsProfile,
  exportSettingsProfile,
  importSettingsProfile,
  updateSidebarWidth,
  stopSidebarResize,
  startSidebarResize,
  addLauncherProfile,
  removeLauncherProfile,
  saveSettings,
  addResourceCatalog,
  removeResourceCatalog,
  addPluginCatalog,
  removePluginCatalog,
  selectActiveCatalog,
  setSimpleAppearance,
  setSimpleAccent,
} = createSettingsWorkflow({
  settingsDraft,
  appearanceDraft,
  state,
  currentView,
  systemDark,
  localThemes,
  wallpaperPreviewUrl,
  wallpaperProcessedUrl,
  memoryValue,
  memoryUnit,
  javaSelection,
  activeCatalogId,
  modProjectType,
  busy,
  persistentViewOptions,
  shortcutRows,
  defaultColorTokens,
  lightColorTokens,
  defaultLauncherProfile,
  showToast,
  errorMessage,
  memorySetting: () => memorySetting.value,
  effectiveJavaPath,
  commitWallpaperBlur,
  applyState,
  ensureCatalogForView,
  broadcastSettingsChanged: (keys) => { void broadcastPluginEvent('settings.changed', { keys }) },
  syncActiveTheme: () => syncActiveTheme(),
})



const page = computed(() => activePluginTab.value
  ? { title: activePluginTab.value.label, kicker: 'Вкладка плагина' }
  : views[currentView.value])
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
  { value: 'custom', label: 'Совместимый API', meta: 'Свой HTTPS-адрес каталога' },
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
  if (currentView.value === 'modpacks') return `${selectedVersion.value} · ${selectedLoaderLabel.value}`
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
  { value: 'nvidia', label: 'NVIDIA (GLX)', meta: 'Проприетарный драйвер' },
  { value: 'mesa', label: 'Mesa (AMD/Intel)', meta: 'Открытый драйвер GLX' },
  { value: 'software', label: 'Программный рендерер', meta: 'Запасной вариант для несовместимого GLX' },
]
const catalogSourceOptions: SelectOption[] = [
  { value: 'modrinth', label: 'Modrinth API', meta: 'Официальный endpoint по умолчанию' },
  { value: 'custom', label: 'Свой каталог', meta: 'Совместимый API endpoint' },
]
const modProjectOptions: SelectOption[] = [
  { value: 'mod', label: 'Моды', meta: 'Изменения и дополнения игры' },
  { value: 'resourcepack', label: 'Ресурспаки', meta: 'Текстуры и звуки' },
  { value: 'shader', label: 'Шейдеры', meta: 'Графические эффекты' },
  { value: 'datapack', label: 'Датапаки', meta: 'Дополнения к мирам' },
  { value: 'plugin', label: 'Плагины сервера', meta: 'Для серверного экземпляра' },
]
const modCategoryOptions: SelectOption[] = [
  { value: '', label: 'Все категории' },
  { value: 'adventure', label: 'Приключения' },
  { value: 'cursed', label: 'Нестандартные' },
  { value: 'decoration', label: 'Декор' },
  { value: 'economy', label: 'Экономика' },
  { value: 'equipment', label: 'Снаряжение' },
  { value: 'food', label: 'Еда' },
  { value: 'game-mechanics', label: 'Игровые механики' },
  { value: 'library', label: 'Библиотеки' },
  { value: 'magic', label: 'Магия' },
  { value: 'management', label: 'Управление' },
  { value: 'minigame', label: 'Мини-игры' },
  { value: 'mobs', label: 'Мобы' },
  { value: 'optimization', label: 'Оптимизация' },
  { value: 'social', label: 'Социальные' },
  { value: 'storage', label: 'Хранилища' },
  { value: 'technology', label: 'Технологии' },
  { value: 'transportation', label: 'Транспорт' },
  { value: 'utility', label: 'Инструменты' },
  { value: 'worldgen', label: 'Карты и генерация мира' },
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
const {
  modPage,
  modTotal,
  modHasMore,
  modPageCount,
  modPagerPages,
  catalogSelection,
  catalogBulkProgress,
  catalogSelectable,
  catalogKey,
  toggleCatalogSelection,
  selectCatalogPage,
  clearCatalogSelection,
  installSelectedMods,
  goToCatalogPage,
  searchMods,
  installMod,
  installModIntoTarget,
  removeMod,
  confirmDatapackInstall,
} = createCatalogWorkflow({
  currentView,
  selectedInstance,
  selectedInstanceName,
  modQuery,
  modProjectType,
  selectedModCategory,
  activeCatalogId,
  activeCatalog,
  catalogServerCore,
  selectedVersion,
  selectedLoader,
  selectedLoaderLabel,
  selectedProjectLabel,
  modInstanceVersion,
  modSortMode,
  modResults,
  modSearchPerformed,
  modBusy,
  modError,
  modInstallId,
  modAction,
  modInstallProgress,
  modInstallDetail,
  pendingDatapack,
  datapackWorlds,
  datapackDialogOpen,
  datapackWorldId,
  activity,
  showToast,
  openActivity,
  finishActivity,
  errorMessage,
  isCancelled,
  confirmDestructive,
  ensureCatalogForView,
  ensureCompatibleInstanceForCatalog,
  setView,
  createModpackInstance: (mod) => createModpackInstance(mod),
  broadcastPluginEvent,
  pluginInstanceRef,
  withUnverifiedDownloadConfirmation: <T>(attempt: (confirmUnverified: boolean) => Promise<T>, prompt: string, cancelMessage: string) =>
    withUnverifiedDownloadConfirmation(attempt, prompt, cancelMessage),
})

const toastDurationMs = 2800
// тост с действием показываем дольше, чтобы пользователь успел его нажать
const toastActionDurationMs = 7000
let toastTimer: number | undefined
let toastTimerStartedAt = 0
let toastTimerRemaining = 0
let activityCloseTimer: number | undefined
let loaderChoiceTimer: number | undefined
let activityLogTimer: number | undefined
let activityDismissedForRun = false
let activityLogPollInFlight = false
let activityLogRevision = 0
let activityLogInstanceName = ''
let microsoftStatusTimer: number | undefined
let microsoftStatusPollInFlight = false
let stopBackendEvents: (() => void) | undefined
let systemThemeMedia: MediaQueryList | undefined
let systemThemeListener: ((event: MediaQueryListEvent) => void) | undefined
const loaderRequestId = ref(0)
let startupViewApplied = false
let userNavigatedDuringBootstrap = false

function clearPluginNotificationToast(toastIdToClear: number) {
  pluginController.clearNotificationsForToast(toastIdToClear)
}

const launcherHidden = ref(false)

function syncLauncherHidden() {
  launcherHidden.value = document.hidden || !document.hasFocus()
}

function notifySystemIfHidden(message: string) {
  if (!launcherHidden.value) return
  void bridge.systemNotify('Nivora Launcher', message).catch(() => undefined)
}

function inDesktopTauri(): boolean {
  return typeof window !== 'undefined' && Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
}

const overlayMode = overlayWindowRequested()
let overlayWatchTimer: number | undefined
let overlayAttachedInstance = ''
let overlayRecordingBusy = false
let overlayMissedPolls = 0

function overlaySettingsDraft(): OverlaySettings {
  return settingsDraft.overlay_settings
}

function overlayHotkeyLabel(): string {
  return normalizeOverlayHotkey(settingsDraft.overlay_settings.hotkey_overlay)
}

const inAppOverlayOpen = ref(false)

async function handleOverlayToggleHotkey() {
  if (overlayWaylandMode()) {
    inAppOverlayOpen.value = !inAppOverlayOpen.value
    return
  }
  const visible = await toggleOverlay()
  if (!visible) {
    syncLauncherHidden()
    inAppOverlayOpen.value = true
    if (overlayLastError()) {
      const message = `Отдельное окно оверлея не открылось (${overlayLastError()}) — оверлей показан поверх лаунчера. Подробный разбор — кнопка «Диагностика оверлея» в Настройках.`
      showToast(message, 'danger')
      notifySystemIfHidden(message)
    }
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
    const wayland = overlayWaylandMode()
    showToast(wayland
      ? `Глобальные клавиши (${failed.join(', ')}) не работают в нативной Wayland-сессии. Пока окно лаунчера в фокусе, комбинация работает как обычно; иначе открывайте оверлей кнопкой «Оверлей» в шапке или пунктом в меню значка в трее.`
      : `Комбинация не зарегистрировалась (занята другой программой?): ${failed.join(', ')}. Оверлей также открывается кнопкой «Оверлей» в шапке и из меню значка в трее.`, 'danger')
  }
}

let overlayHintShown = false
let waylandOverlayHintShown = false

function attachOverlayForLaunch(instanceName: string) {
  if (overlayMode || !inDesktopTauri()) return
  if (!settingsDraft.overlay_settings.enabled) return
  void attachOverlay(instanceName)
  if (!overlayHintShown) {
    overlayHintShown = true
    showToast(`Оверлей готов: ${normalizeOverlayHotkey(settingsDraft.overlay_settings.hotkey_overlay)}. Если игра в полноэкранном режиме — переключите её в оконный (F11 в Minecraft)`)
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
  }
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
      if (gameSessionActive.value) overlayMissedPolls = 0
      else overlayMissedPolls += 1
      if (shouldDetachOverlayAfterMisses(overlayMissedPolls, gameSessionActive.value)) await detachOverlay()
    }
  } catch {
  }
}

function startOverlayWatcher() {
  if (overlayWatchTimer !== undefined) return
  void initOverlayEventBridge(() => { void handleOverlayToggleHotkey() }, () => { inAppOverlayOpen.value = false })
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
  if (settingsDraft.overlay_settings.enabled) {
    void attachOverlay(overlayAttachedInstance, { silent: true })
  } else {
    void detachOverlay()
    void unbindGameHotkeys()
  }
}, { deep: true })

function showToast(message: string, tone: PluginUiTone = 'success', action?: { label: string; run: () => void } | null) {
  if (tone === 'danger') notifySystemIfHidden(message)
  if (!settingsDraft.show_toasts) return
  const previousToastId = toastId.value
  toastId.value += 1
  clearPluginNotificationToast(previousToastId)
  toast.value = message
  toastAction.value = action || null
  toastDuration.value = action ? toastActionDurationMs : toastDurationMs
  toastToneClass.value = {
    default: '',
    muted: 'status-dot--muted',
    success: 'status-dot--green',
    warning: 'status-dot--warning',
    danger: 'status-dot--danger',
  }[tone]
  const currentToastId = toastId.value
  if (toastTimer) window.clearTimeout(toastTimer)
  toastTimerStartedAt = Date.now()
  toastTimerRemaining = toastDuration.value
  toastTimer = window.setTimeout(() => {
    if (toastId.value !== currentToastId) return
    toast.value = ''
    toastAction.value = null
    clearPluginNotificationToast(currentToastId)
    toastTimer = undefined
  }, toastTimerRemaining)
}

// пауза тоста действует, пока курсор находится над ним
function pauseToastTimer() {
  if (!toastTimer) return
  window.clearTimeout(toastTimer)
  toastTimer = undefined
  toastTimerRemaining = Math.max(400, toastTimerRemaining - (Date.now() - toastTimerStartedAt))
}

function resumeToastTimer() {
  if (toastTimer || !toast.value) return
  const currentToastId = toastId.value
  toastTimerStartedAt = Date.now()
  toastTimer = window.setTimeout(() => {
    if (toastId.value !== currentToastId) return
    toast.value = ''
    toastAction.value = null
    clearPluginNotificationToast(currentToastId)
    toastTimer = undefined
  }, toastTimerRemaining)
}

function runToastAction() {
  const action = toastAction.value
  toastAction.value = null
  if (!action) return
  // после действия закрываем тост
  toastTimerRemaining = 0
  if (toastTimer) {
    window.clearTimeout(toastTimer)
    toastTimer = undefined
  }
  const currentToastId = toastId.value
  toastId.value += 1
  toast.value = ''
  clearPluginNotificationToast(currentToastId)
  action.run()
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

const wallpaperRenderer = createWallpaperRenderer()

async function renderWallpaper() {
  const next = await wallpaperRenderer.render(wallpaperUrl.value, wallpaperRenderBlur.value)
  // более старый запрос не заменяет результат нового
  if (next !== null) wallpaperProcessedUrl.value = next
}

function scheduleWallpaperRender() {
  if (wallpaperResizeTimer) window.clearTimeout(wallpaperResizeTimer)
  wallpaperResizeTimer = window.setTimeout(() => {
    wallpaperResizeTimer = undefined
    void renderWallpaper()
  }, 120)
}

// сокращает счётчик загрузок для карточки каталога
function formatDownloadsCompact(value: unknown): string {
  const total = Number(value)
  if (!Number.isFinite(total) || total <= 0) return ''
  if (total >= 1000000) {
    const millions = total / 1000000
    const text = (millions >= 100 ? millions.toFixed(1) : millions.toFixed(2)).replace('.', ',').replace(/,?0+$/, '')
    return `${text} млн`
  }
  return total.toLocaleString('ru-RU')
}

// число подписок Modrinth показываем как избранное проекта
function formatLikes(value: unknown): string {
  const total = Number(value)
  return Number.isFinite(total) && total > 0 ? total.toLocaleString('ru-RU') : ''
}

function onModIconError(mod: ModResult) {
  mod.icon_url = ''
}

const {
  modpackMenu,
  chooseModpackFile,
  importModpackArchive,
  modpackPreviewOpen,
  modpackPreviewTitle,
  modpackPreviewItems,
  modpackPreviewTotal,
  modpackPreviewTruncated,
  modpackPreviewQuery,
  modpackPreviewFiltered,
  modpackPreviewSourceLabel,
  closeModpackPreview,
  createModpackInstance,
  withUnverifiedDownloadConfirmation,
} = createModpackWorkflow({
  state,
  currentView,
  selectedInstance,
  modBusy,
  modInstallDetail,
  modInstanceVersion,
  selectedVersion,
  selectedLoader,
  selectedLoaderVersion,
  loadingChoices,
  selectedInstanceName,
  activeCatalogId,
  memorySetting,
  settingsDraft,
  busy,
  activity,
  showToast,
  openActivity,
  finishActivity,
  searchMods,
  errorMessage,
  isCancelled,
  setView,
  launchExisting,
  openCatalogDetails,
  effectiveJavaPath,
})

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


const {
  applyTheme,
  resetAppearance,
  refreshLocalThemes,
  saveCurrentTheme,
  importThemeArchive,
  deleteLocalTheme,
  exportLocalTheme,
  installGithubTheme,
  themeMatchesSettings,
  syncActiveTheme,
} = createThemeManager({
  state,
  appearanceDraft,
  settingsDraft,
  localThemes,
  themeOperationBusy,
  githubThemeInstallBusy,
  activeThemeId,
  availableThemes,
  defaultColorTokens,
  lightColorTokens,
  defaultRadiusTokens,
  defaultAnimationSettings,
  busy,
  wallpaperPreviewUrl,
  wallpaperRenderBlur,
  showToast,
  errorMessage,
  saveSettings,
  releaseWallpaperPreview,
  setLanguageFromSettings,
})

let activeButtonSound: HTMLAudioElement | null = null
function playButtonSound(event: MouseEvent) {
  if (!(event.target instanceof Element)) return
  const button = event.target.closest<HTMLButtonElement>('button:not(:disabled)')
  if (!button) return
  const target = button.dataset.buttonTarget
  const role = target ? buttonCustomizationRole(target) : undefined
  const source = role && target
    ? resolveButtonCustomization(appearanceDraft.button_customization, target as ButtonCustomizationTarget).sound_data
    : appearanceDraft.button_customization.sound_data
  if (!source) return
  if (activeButtonSound) {
    activeButtonSound.pause()
    activeButtonSound.currentTime = 0
  }
  activeButtonSound = new Audio(source)
  void activeButtonSound.play().catch(() => {})
}

const topbarSystemMetrics = ref<SystemMetricsSnapshot>()
const globalSearchOpen = ref(false)

function openSearchInstance(name: string) {
  const instance = state.instances.find((value) => value.name === name)
  if (instance) void openInstance(instance)
}

function openSearchFolder(path: string) {
  openInLauncherExplorer(path)
}
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
    // каталог игры нужен для метрик её диска
    topbarSystemMetrics.value = await bridge.systemMetrics(false, state.settings.game_dir)
  } catch {
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

function ensureCatalogForView(view: ViewName) {
  if (view !== 'mods' && view !== 'modpacks') return
  const pluginView = view === 'mods' && modProjectType.value === 'plugin'
  const allowed = pluginView
    ? settingsDraft.plugin_catalogs.filter((catalog) => catalog.enabled !== false)
    : settingsDraft.resource_catalogs.filter((catalog) => view === 'modpacks'
      ? ['modrinth', 'ftb', 'technic'].includes(catalog.kind)
      : !['ftb', 'technic'].includes(catalog.kind))
  if (!allowed.length) return
  const preferredId = pluginView ? settingsDraft.active_plugin_catalog_id : settingsDraft.active_catalog_id
  const selectedId = chooseAllowedCatalog(allowed.map((catalog) => catalog.id), preferredId, activeCatalogId.value)
  if (!selectedId) return
  activeCatalogId.value = selectedId
  if (pluginView) settingsDraft.active_plugin_catalog_id = selectedId
  else if (!settingsDraft.resource_catalogs.some((catalog) => catalog.id === settingsDraft.active_catalog_id)) settingsDraft.active_catalog_id = selectedId
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
  if (changed) {
    const leaving = appShellRef.value?.querySelector<HTMLElement>('.page-scroll')
    if (leaving) pageScrollMemory.set(currentView.value, leaving.scrollTop)
  }
  currentView.value = view
  activePluginTabId.value = pluginTabId
  if (view === 'mods' && modProjectType.value === 'modpack') modProjectType.value = 'mod'
  ensureCatalogForView(view)
  ensureCompatibleInstanceForCatalog()
  if (changed) {
    pageTransitionPulse.value = false
    void nextTick(() => {
      pageTransitionPulse.value = true
      const entering = appShellRef.value?.querySelector<HTMLElement>('.page-scroll')
      entering?.scrollTo({ top: pageScrollMemory.get(view) ?? 0, behavior: 'instant' as ScrollBehavior })
    })
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
  if (event.defaultPrevented || document.querySelector('.dialog-scrim')) return
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k') {
    event.preventDefault()
    globalSearchOpen.value = true
    return
  }
  const target = event.target as HTMLElement | null
  const editing = target?.matches('input, textarea, select, [contenteditable="true"]')
  if (!overlayMode && settingsDraft.overlay_settings.enabled) {
    const overlayHotkey = normalizeOverlayHotkey(settingsDraft.overlay_settings.hotkey_overlay)
    const recordingHotkey = normalizeHotkey(settingsDraft.overlay_settings.hotkey_recording, 'F7')
    const hardModifier = event.ctrlKey || event.altKey || event.metaKey
    if (!editing || hardModifier) {
      if (shortcutMatches(event, overlayHotkey)) {
        event.preventDefault()
        event.stopImmediatePropagation()
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
    if (!editing || event.ctrlKey || event.metaKey) setActivityOpen(!activity.open, true)
  } else if (!editing && !event.repeat) {
    const pluginShortcut = pluginShortcuts.value.find((shortcut) => shortcutMatches(event, shortcut.definition.default_key))
    if (pluginShortcut) {
      event.preventDefault()
      void runPluginShortcut(pluginShortcut)
    }
  }
}

function applyState(next: AppState, preserveAppearance = false) {
  const settingsHydration = beginSettingsHydration(preserveAppearance)
  Object.assign(state, next)
  const backendStamp = String(next.build_stamp || '')
  if (backendStamp) {
    if (backendStamp !== FRONTEND_BUILD_STAMP) {
      if (!buildStampMismatch.value) {
        buildStampMismatch.value = { frontend: FRONTEND_BUILD_STAMP, backend: backendStamp }
        showToast(`Части лаунчера собраны из разных версий: интерфейс ${FRONTEND_BUILD_STAMP}, нативное ядро ${backendStamp}. Приложение нужно пересобрать целиком.`, 'danger')
      }
    } else {
      buildStampMismatch.value = null
    }
  }
  const sessionWayland = String(state.system.session || '').toLowerCase() === 'wayland'
  const forcedX11 = String(state.system.gdk_backend || '').toLowerCase() === 'x11'
  setOverlayWaylandMode(sessionWayland && !forcedX11)
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
    download_source: next.settings.download_source || 'auto',
    confirm_destructive_actions: next.settings.confirm_destructive_actions !== false,
    auto_repair_enabled: next.settings.auto_repair_enabled !== false,
    auto_repair_muted: Array.isArray(next.settings.auto_repair_muted) ? [...next.settings.auto_repair_muted] : [],
    show_activity_panel: next.settings.show_activity_panel !== false,
    show_toasts: next.settings.show_toasts !== false,
    minimize_on_launch: next.settings.minimize_on_launch === true,
    language: next.settings.language === 'en' ? 'en' : next.settings.language === 'ru' ? 'ru' : null,
    font_family: next.settings.font_family || 'system',
    file_explorer: next.settings.file_explorer === 'system' ? 'system' : 'builtin',
    file_editor: next.settings.file_editor === 'system' ? 'system' : 'builtin',
    keep_logs_days: next.settings.keep_logs_days ?? 14,
    telemetry: next.settings.telemetry === true,
     keyboard_shortcuts: (() => {
      const merged = { ...settingsDraft.keyboard_shortcuts, ...(next.settings.keyboard_shortcuts || {}) } as Record<string, string>
      // старые версии разрешали сохранять сочетания в произвольном формате
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
   if (preserveAppearance && settingsHydration.preservedFontFamily) settingsDraft.font_family = settingsHydration.preservedFontFamily
   if (!preserveAppearance) {
     Object.assign(appearanceDraft, {
       theme: next.settings.theme || 'dark',
       active_theme_id: next.settings.active_theme_id || 'nivora-dark',
       wallpaper_path: next.settings.wallpaper_path || '',
       wallpaper_data: next.settings.wallpaper_data || '',
       wallpaper_blur: next.settings.wallpaper_blur,
       wallpaper_opacity: next.settings.wallpaper_opacity,
       sidebar_texture_data: next.settings.sidebar_texture_data || '',
        accent_color: legacyDefaultColorTokenValues.accent.includes((next.settings.accent_color || '').toLowerCase())
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
          // старые настройки могли хранить отдельное значение fade для каждой страницы
          page_styles: (() => {
            const legacyStyles = next.settings.animation_settings?.page_styles || {}
            const merged: Record<string, string> = { ...defaultAnimationSettings.page_styles }
            for (const [view, style] of Object.entries(legacyStyles)) merged[view] = style === 'fade' ? '' : style
            return merged as typeof defaultAnimationSettings.page_styles
          })(),
          page_durations_ms: { ...defaultAnimationSettings.page_durations_ms, ...(next.settings.animation_settings?.page_durations_ms || {}) },
        },
        density: next.settings.density || 'comfortable',
       instance_layout: next.settings.instance_layout === 'compact' ? 'compact' : 'default',
       ui_scale: next.settings.ui_scale || 100,
       radius_tokens: { ...defaultRadiusTokens, sidebar: 0, ...(next.settings.radius_tokens || {}) },
       page_themes: { ...appearanceDraft.page_themes, ...next.settings.page_themes },
       page_layouts: { ...appearanceDraft.page_layouts, ...next.settings.page_layouts },
       widget_visibility: { ...appearanceDraft.widget_visibility, ...next.settings.widget_visibility },
        color_tokens: migrateDefaultColors(next.settings.color_tokens),
        button_customization: normalizeButtonCustomization(next.settings.button_customization),
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
   ensureCatalogForView(currentView.value)
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
   finishSettingsHydration(settingsHydration)
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
    // уведомляем, что глобальные клавиши недоступны в Wayland
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

function releaseWallpaperPreview() {
  if (wallpaperPreviewUrl.value.startsWith('blob:')) URL.revokeObjectURL(wallpaperPreviewUrl.value)
  wallpaperPreviewUrl.value = ''
}

function setWallpaperPreview(url: string) {
  releaseWallpaperPreview()
  wallpaperPreviewUrl.value = url
}

function encodeBlob(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result || '').replace(/^data:[^;]*;base64,/, ''))
    reader.onerror = () => reject(new Error('Не удалось прочитать файл'))
    reader.readAsDataURL(blob)
  })
}

async function importWallpaperFile() {
  const file = await pickFile({ accept: 'image/png,image/jpeg,image/webp', title: 'Изображение для обоев' })
  if (!file) return
  try {
    // сохраняем уменьшенную копию в WebP, чтобы не хранить исходный файл
    const mime = file.fileName.toLowerCase().endsWith('.png') ? 'image/png' : file.fileName.toLowerCase().endsWith('.webp') ? 'image/webp' : 'image/jpeg'
    const rawUrl = `data:${mime};base64,${file.data}`
    const prepared = await prepareWallpaperForStorage(rawUrl)
    if (!prepared && file.size > 32 * 1024 * 1024) throw new Error('Файл обоев превышает лимит 32 MiB')
    const fileName = prepared ? prepared.fileName : file.fileName
    const data = prepared ? prepared.data : file.data
    const previewMime = prepared ? 'image/webp' : mime
    const previewData = prepared ? prepared.data : file.data
    setWallpaperPreview(`data:${previewMime};base64,${previewData}`)
    const result = await bridge.importWallpaper(fileName, data)
    if (prepared) URL.revokeObjectURL(prepared.previewUrl)
    appearanceDraft.wallpaper_path = result.settings.wallpaper_path
    appearanceDraft.wallpaper_data = result.settings.wallpaper_data || ''
    state.settings.wallpaper_path = result.settings.wallpaper_path
    state.settings.wallpaper_data = result.settings.wallpaper_data || ''
    showToast('Обои сохранены')
  } catch (error) {
    releaseWallpaperPreview()
    showToast(errorMessage(error))
  }
}

function onFileBrowserClosed() {
  resolveFileBrowser(null)
  resolveFileBrowserMany(null)
}

function openSkinsFolderBrowser() {
  void bridge.skinsFolder().then((result) => openInLauncherExplorer(result.path)).catch(() => openInLauncherExplorer())
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

function setActivityOpen(open: boolean, manual = false) {
  if (open) activityDismissedForRun = false
  else if (manual && gameSessionActive.value) activityDismissedForRun = true
  activity.open = open
}

function closeActivityManually() {
  setActivityOpen(false, true)
}

function openActivity(title: string, detail: string, visible = true) {
  if (!gameSessionActive.value) activityDismissedForRun = false
  stopActivityLogPolling()
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
  if (logs?.length && !activity.logs.length) activity.logs = logs.slice(-200)
  if (activityCloseTimer) window.clearTimeout(activityCloseTimer)
  activityCloseTimer = window.setTimeout(() => {
    if (!activity.busy) activity.open = false
    activityCloseTimer = undefined
  }, 1500)
}

function onBedrockActivity(payload: { title: string; detail: string; done?: boolean; logs?: string[] }) {
  if (payload.done) {
    finishActivity(payload.detail, payload.logs)
    return
  }
  if (activity.busy && activity.title === payload.title) {
    activity.detail = payload.detail
    return
  }
  if (gameSessionActive.value && activityDismissedForRun) {
    activity.title = payload.title
    activity.detail = payload.detail
    return
  }
  openActivity(payload.title, payload.detail)
}

async function pollActivityLog(instanceName: string) {
  if (activityLogPollInFlight) return
  const revision = activityLogRevision
  activityLogPollInFlight = true
  try {
    const result = await bridge.instanceLog(instanceName)
    if (revision !== activityLogRevision || activityLogInstanceName !== instanceName) return
    activity.logs = result.lines.slice(-200)
    if (result.running && activity.busy) activity.detail = 'Minecraft работает…'
  } catch {
  } finally {
    activityLogPollInFlight = false
  }
}

function startActivityLogPolling(instanceName: string) {
  if (activityLogTimer) window.clearInterval(activityLogTimer)
  activityLogRevision += 1
  activityLogInstanceName = instanceName
  void pollActivityLog(instanceName)
  activityLogTimer = window.setInterval(() => void pollActivityLog(instanceName), 2500)
}

function stopActivityLogPolling() {
  activityLogRevision += 1
  activityLogInstanceName = ''
  if (activityLogTimer) window.clearInterval(activityLogTimer)
  activityLogTimer = undefined
}

function applyMicrosoftDeviceCode(code: string, verificationUri: string) {
  // во время первого запуска код входа показывается в карточке визарда
  if (firstRunPhase.value && firstRunPhase.value !== 'tour') {
    activity.verification_code = code
    activity.verification_uri = verificationUri
    const entry = `КОД MICROSOFT: ${code}`
    if (!activity.logs.includes(entry)) activity.logs.push(entry)
    return
  }
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
    // если остановка затянулась, через 5 секунд показываем текущее состояние
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
    // предпросмотр и старые сборки не предоставляют контроллер окна
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
  creationIcon.value = ''
  setView('instances')
  void refreshLoaderChoices()
  void ensureIconBlocks()
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

const downloadSourceOptions: SelectOption[] = [
  { value: 'auto', label: 'Авто: официальные серверы, при сбое — зеркало' },
  { value: 'mirror', label: 'Сначала зеркало BMCLAPI' },
  { value: 'official', label: 'Только официальные серверы' },
]
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
    // сохраняем выбранную иконку в профиль сразу после установки
    const chosenIcon = iconBlocks.value.find((block) => block.id === creationIcon.value)
    if (chosenIcon) {
      const created = state.instances.find((value) => value.name === name)
      try {
        const blockUrl = await composedBlockIcon(chosenIcon)
        const profile = await bridge.updateInstanceProfile({
          name,
          display_name: created?.display_name || '',
          description: created?.description || '',
          group: created?.group || '',
          icon_file_name: `${chosenIcon.id}.png`,
          icon_data: blockUrl.split(',')[1] || '',
        })
        if (profile.instance) {
          const index = state.instances.findIndex((value) => value.name === name)
          if (index >= 0) state.instances.splice(index, 1, profile.instance)
        }
      } catch {
        // экземпляр уже создан, поэтому сбой иконки не отменяет операцию
      }
    }
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
      // установка идёт в фоне, здесь обновляется только её статус
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

async function chooseServerCoreFile() {
  const file = await pickFile({ accept: '.jar', title: 'Ядро сервера (server.jar)' })
  if (!file) return
  try {
    await selectServerCoreData(file)
  } catch (error) {
    showToast(errorMessage(error))
  }
}

async function selectServerCoreData(file: { fileName: string; data: string; size: number }) {
  if (!file.fileName.toLowerCase().endsWith('.jar')) throw new Error('Загрузите файл ядра с расширением .jar')
  if (file.size > 512 * 1024 * 1024) throw new Error('Файл ядра превышает лимит 512 MiB')
  selectCustomServerCore()
  serverCreation.jar_name = file.fileName
  serverCreation.jar_data = file.data
  showToast(`Выбрано ядро: ${file.fileName}`)
}

function onServerCoreDrop(event: DragEvent) {
  serverCoreDragActive.value = false
  const file = event.dataTransfer?.files[0]
  if (file) void encodeBlob(file)
    .then((data) => selectServerCoreData({ fileName: file.name, data, size: file.size }))
    .catch((error) => showToast(errorMessage(error)))
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
  if (busy.value) return false
  const username = accountName.value.trim()
  const invalid = nicknameError(username)
  if (invalid) {
    if (firstRunPhase.value === 'account') firstRunError.value = invalid
    showToast(invalid)
    return false
  }
  busy.value = true
  try {
    const result = await bridge.createOfflineAccount(username)
    const account = result.account
    if (!account) return false
    state.accounts = [...state.accounts.filter((existing) => existing.account_id !== account.account_id), account]
    const defaultResult = await bridge.setDefaultAccount(account.account_id)
    state.accounts = defaultResult.accounts
    await refreshBootstrap()
    selectedAccount.value = account.account_id
    settingsDraft.default_account_id = account.account_id
    accountName.value = ''
    showToast('Offline-аккаунт добавлен')
    return true
  } catch (error) {
    if (firstRunPhase.value === 'account') firstRunError.value = errorMessage(error)
    showToast(errorMessage(error))
    return false
  } finally {
    busy.value = false
  }
}

const loginBusy = ref(false)
async function loginMicrosoft(): Promise<boolean> {
  if (loginBusy.value || !state.settings.has_client_id) return false
  loginBusy.value = true
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
    loginBusy.value = false
  }
}

const elyLoginOpen = ref(false)
const elyUsername = ref('')
const elyPassword = ref('')
const elyLoginBusy = ref(false)
const elyLoginError = ref('')

function openElyLogin() {
  elyLoginOpen.value = true
  elyLoginError.value = ''
  elyPassword.value = ''
}

async function loginEly(): Promise<boolean> {
  if (elyLoginBusy.value) return false
  if (!elyUsername.value.trim() || !elyPassword.value) {
    elyLoginError.value = 'Введите логин и пароль ely.by'
    return false
  }
  elyLoginBusy.value = true
  elyLoginError.value = ''
  openActivity('Вход через ely.by', 'Проверяю логин и пароль.')
  try {
    const result = await bridge.loginEly(elyUsername.value.trim(), elyPassword.value)
    finishActivity('Аккаунт ely.by подключён.', result.messages)
    await refreshBootstrap()
    selectedAccount.value = result.account.account_id
    settingsDraft.default_account_id = result.account.account_id
    showToast(`Добро пожаловать, ${result.account.display_name}`)
    elyLoginOpen.value = false
    elyPassword.value = ''
    return true
  } catch (error) {
    activity.logs.push(errorMessage(error))
    finishActivity('Вход не завершён.')
    elyLoginError.value = errorMessage(error)
    showToast(errorMessage(error))
    return false
  } finally {
    elyLoginBusy.value = false
  }
}

function setFirstRunPhase(phase: FirstRunPhase) {
  firstRunError.value = ''
  firstRunPhase.value = phase
}

function pickFirstRunLanguage(next: Locale): void {
  settingsDraft.language = next
  setLocale(next)
  try {
    window.localStorage.setItem('nivora-language-picked', next)
  } catch {
    // сохраняем выбор приватного режима в настройках
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
  if (busy.value) return
  firstRunError.value = ''
  accountName.value = name
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

function selectCatalogInstance(name: string) {
  selectedInstanceName.value = name
  if (selectedInstance.value?.kind === 'server') catalogServerCore.value = selectedInstance.value.server_core || catalogServerCore.value
  modResults.value = []
  modError.value = ''
  if (currentView.value === 'mods') void searchMods()
}

function openGameFolder() {
  openInLauncherExplorer()
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
  if (effectiveAccount.value?.kind === 'offline') {
    showToast('Bedrock требует аккаунт Microsoft: одиночная игра без входа недоступна, в отличие от Java')
    return
  }
  activityDismissedForRun = false
  gameSessionActive.value = true
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
      if (errorMessage(error).includes('Подключить аккаунт')) {
        void bedrockWorkspaceRef.value?.refreshAccountStatus?.()
      }
    }
  } finally {
    stopActivityLogPolling()
    gameSessionActive.value = false
    await refreshBootstrap()
  }
}

function applyAutoRepairSettings(next: SettingsInfo) {
  const muted = Array.isArray(next.auto_repair_muted) ? [...next.auto_repair_muted] : []
  state.settings.auto_repair_muted = muted
  settingsDraft.auto_repair_muted = [...muted]
  updateSavedSettingsBaseline({ auto_repair_muted: [...muted] })
}

async function relaunchAfterRepair(instanceName: string) {
  await refreshBootstrap()
  const instance = state.instances.find((item) => item.name === instanceName)
  if (!instance) return
  if (instanceWorkspaceRef.value && selectedInstance.value?.name === instanceName && currentView.value === 'instance-settings') {
    await instanceWorkspaceRef.value.launch?.()
    return
  }
  await launchExisting(instance)
}

// обрабатываем запуск экземпляра из ярлыка
async function takeShortcutLaunchRequest() {
  try {
    const { name } = await bridge.shortcutTakeLaunchRequest()
    if (name) await launchFromShortcut(name)
  } catch {
    // ярлык могли удалить, пока выполнялся запрос
  }
}

async function launchFromShortcut(name: string) {
  for (let attempt = 0; attempt < 40 && state.instances.length === 0; attempt += 1) {
    await new Promise((resolve) => window.setTimeout(resolve, 250))
  }
  const instance = state.instances.find((item) => item.name === name)
  if (!instance) {
    showToast(`Экземпляр «${name}» из ярлыка не найден`)
    return
  }
  await launchExisting(instance)
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
  activityDismissedForRun = false
  gameSessionActive.value = true
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
    if (result.exit_code !== 0) offerAutoRepair(result.repair)
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
    if (!isCancelled(error)) {
      showToast(errorMessage(error))
      if (launchRequested) void diagnoseLaunchFailure(instance.name, errorMessage(error))
    }
  } finally {
    stopActivityLogPolling()
    gameSessionActive.value = false
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

const compareDialogSource = ref('')
const compareDialogOpen = ref(false)
const autotuneInstance = ref<InstanceInfo | null>(null)

function openInstanceCompare(instance: InstanceInfo) {
  compareDialogSource.value = instance.name
  compareDialogOpen.value = true
}

function openInstanceAutotune(instance: InstanceInfo) {
  autotuneInstance.value = instance
}

async function onAutotuneApplied() {
  await refreshBootstrap()
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
  externalImportScanning.value = true
  try {
    let result: { candidates: ExternalInstanceCandidate[] }
    try {
      result = await bridge.externalInstances()
    } finally {
      externalImportScanning.value = false
    }
    const candidates = result.candidates || []
    if (!candidates.length) {
      showToast('Поддерживаемые экземпляры Prism/MultiMC/CurseForge/официального лаунчера не найдены')
      return
    }
    const sourcePath = await chooseDialog(
      'Выберите источник импорта',
      [
        { value: '__mrpack__', label: 'Файл .mrpack (Modrinth)', description: 'Модпак Modrinth из файла — лаунчер создаст под него новый экземпляр' },
        ...candidates.map(externalImportChoice),
      ],
      {
        title: 'Импорт экземпляров',
        detail: 'Модпак .mrpack или стандартные каталоги Prism Launcher, MultiMC, CurseForge и официального Minecraft Launcher.',
        confirmLabel: 'Далее',
      },
    )
    if (!sourcePath) return
    if (sourcePath === '__mrpack__') {
      const file = await pickFile({ accept: '.mrpack', title: 'Модпак Modrinth (.mrpack)' })
      if (file) void createModpackInstance(undefined, { fileName: file.fileName, data: file.data })
      return
    }
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
    externalImportScanning.value = false
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
    document.title = 'Nivora Overlay'
    document.body.classList.add('overlay-mode')
    if (new URLSearchParams(window.location.search).get('opaque') === '1') {
      document.body.classList.add('overlay-mode--opaque')
    }
    return
  }
  if (inDesktopTauri()) {
    startOverlayWatcher()
    // закрытие прячет лаунчер в трей, поэтому объясняем это один раз
    let trayHintShown = false
    try {
      const { listen } = await import('@tauri-apps/api/event')
      await listen('nivora-hidden-to-tray', () => {
        if (trayHintShown) return
        trayHintShown = true
        void bridge.systemNotify('Nivora Launcher', 'Лаунчер свернулся в трей и продолжает работать — глобальные клавиши и оверлей активны. Полный выход: «Выйти» в меню значка.').catch(() => undefined)
      })
      // при уже запущенном лаунчере цель приходит отдельным событием
      await listen<{ name: string | null }>('nivora-launch-request', (event) => {
        const name = event.payload?.name
        if (name) void launchFromShortcut(name)
      })
    } catch {
    }
    // при холодном старте цель ярлыка доступна после инициализации интерфейса
    void takeShortcutLaunchRequest()
  }
  void loadSettingsProfiles()
  pluginController.startDevelopmentPolling()
  window.addEventListener('keydown', handleGlobalKeydown)
  window.addEventListener('resize', scheduleWallpaperRender)
  window.addEventListener('focus', syncLauncherHidden)
  window.addEventListener('blur', syncLauncherHidden)
  document.addEventListener('visibilitychange', () => {
    syncLauncherHidden()
  })
  syncLauncherHidden()
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
      if (event.kind === 'status' && event.event === 'session-started') {
        if (event.instance_name === selectedInstanceName.value) {
          activityLogRevision += 1
          activity.open = shouldOpenActivityAfterSessionStart(settingsDraft.show_activity_panel, activityDismissedForRun)
          activity.title = 'Консоль запуска'
          activity.detail = 'Minecraft работает…'
          activity.progress = 68
          activity.busy = true
          activity.cancelling = false
          activity.logs = []
          activity.verification_code = undefined
          activity.verification_uri = undefined
        }
        return
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
      if (event.kind === 'log' || !settingsDraft.show_activity_panel || !activity.busy) return
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
  if (toastTimer) window.clearTimeout(toastTimer)
  if (activityCloseTimer) window.clearTimeout(activityCloseTimer)
  stopActivityLogPolling()
  stopMicrosoftStatusPolling()
  if (systemThemeMedia && systemThemeListener) systemThemeMedia.removeEventListener('change', systemThemeListener)
  delete document.body.dataset.launcherAnimations
  document.documentElement.style.removeProperty('--ui-scale')
  stopSidebarResize()
  releaseWallpaperPreview()
  void pluginRuntime.dispose()
  pluginController.dispose()
})
const settingsPageModel = {
  accountOptions, activeLauncherProfile, addLauncherProfile, addPluginCatalog, addResourceCatalog, applySettingsProfile, backupDestinationOptions,
  busy, catalogKindOptions, clampMemoryDraft, defaultGameDirectory, deleteSettingsProfile, downloadSourceOptions, errorMessage,
  exportSettingsProfile, formatDateShort, formatMemoryLabel, graphicsOptions, graphicsReport, importSettingsProfile, javaChoiceOptions,
  javaSelection, languageOptions, loadGraphicsDiagnostics, loaderOptions, memoryMaxValue, memoryTotalLabel, memoryUnit,
  memoryUnitOptions, memoryValue, openGameFolder, openPluginLink, overwriteSettingsProfile, persistentViewOptions, pluginCatalogKindOptions,
  refreshBootstrap, refreshJavaOptions, removeLauncherProfile, removePluginCatalog, removeResourceCatalog, replayLauncherTour, resourceCatalogOptions,
  runPluginAction, savePluginConfig, saveSettings, saveSettingsProfileAs, selectActiveCatalog, setLanguageFromSettings, setView,
  settingsDraft, settingsProfileBusy, settingsProfileName, settingsProfiles, settingsProfilesLoading, settingsSaveMessage, settingsSaveState,
  settingsSectionViews, shortcutRows, showToast, state,
}
</script>

<template>
  <GameOverlay v-if="overlayMode || inAppOverlayOpen" :in-app="!overlayMode" />
  <DialogHost v-else />
  <ContextMenuHost />
  <Transition name="modal">
  <FileBrowserDialog
    v-if="fileBrowserRequest"
    :request="fileBrowserRequest"
    :editor-mode="settingsDraft.file_editor"
    @close="onFileBrowserClosed"
    @chosen="resolveFileBrowser"
    @chosen-many="resolveFileBrowserMany"
    @chosen-directory="resolveFileBrowserDirectory"
    @toast="showToast"
  />
  </Transition>
  <Transition name="dialog">
  <GlobalSearchDialog
    v-if="globalSearchOpen"
    @close="globalSearchOpen = false"
    @open-instance="openSearchInstance"
    @open-path="openSearchFolder"
  />
  </Transition>
  <AutoRepairDialog v-if="!overlayMode" @toast="showToast" @settings="applyAutoRepairSettings" @repaired="refreshBootstrap()" @relaunch="relaunchAfterRepair" @login="loginMicrosoft()" />
  <div v-if="!overlayMode" ref="appShellRef" class="app-shell" :data-button-icon="String(Boolean(appearanceDraft.button_customization.icon_data))" :data-button-fill="String(appearanceDraft.button_customization.fill)" :data-button-border="String(appearanceDraft.button_customization.border)" @click.capture="playButtonSound">
        <div v-if="buildStampMismatch" class="build-mismatch-banner" role="alert">
          <Icon name="info" :size="16" />
          <span>Части лаунчера собраны из разных версий: интерфейс {{ buildStampMismatch.frontend }}, нативное ядро {{ buildStampMismatch.backend }}. При несовпадении версий некоторые экраны (Bedrock, оверлей и другие) могут работать неправильно. Пересоберите приложение целиком: <code>cd frontend &amp;&amp; npm install &amp;&amp; npm run tauri build</code></span>
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
         :button-labels="appearanceDraft.button_customization.labels"
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

          <h1>{{ page.title }}</h1>
        </div>
        <div class="topbar-actions">
          <button class="icon-button" type="button" aria-label="Глобальный поиск (Ctrl+K)" v-tooltip="'Где установлен мод, мир или ресурспак — по всем экземплярам'" @click="globalSearchOpen = true">
            <Icon name="search" :size="17" />
          </button>
          <div v-if="topbarMetricsVisible" class="topbar-metrics" aria-label="Системные метрики">
            <span v-if="settingsDraft.overlay_settings.metrics_cpu && topbarSystemMetrics?.cpu.percent !== null && topbarSystemMetrics?.cpu.percent !== undefined" class="topbar-metric" v-tooltip="'Загрузка процессора'"><i>CPU</i>{{ topbarSystemMetrics.cpu.percent.toFixed(0) }}%</span>
            <span v-if="settingsDraft.overlay_settings.metrics_ram && topbarSystemMetrics?.memory.percent !== null && topbarSystemMetrics?.memory.percent !== undefined" class="topbar-metric" v-tooltip="'Занятая память'"><i>RAM</i>{{ topbarSystemMetrics.memory.percent.toFixed(0) }}%</span>
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
          <button v-if="settingsDraft.show_activity_panel" class="activity-trigger" type="button" @click="setActivityOpen(!activity.open, true)">
            <span class="status-dot" :class="{ 'status-dot--busy': activity.busy }" />
            <span>{{ activity.busy ? 'В работе' : 'Готово' }}</span>
          </button>
        </div>
      </header>

       <div class="page-scroll">
           <div class="page-transition" :class="{ 'page-transition--animate': pageTransitionPulse }">
               <section data-page-view="instances" class="page-content">
                <div class="section-intro library-intro"><div><h2>Экземпляры</h2><p>Профили игры и серверы Minecraft.</p></div><div class="section-intro-actions"><button class="button button--quiet" type="button" :disabled="externalImportBusy" @click="importExternalInstances"><Icon name="upload" :size="16" />{{ externalImportScanning ? 'Ищу источники…' : 'Импорт' }}</button><button class="button button--outline" type="button" @click="openCreateServer"><Icon name="server" :size="16" />Новый сервер</button><button class="button button--accent" type="button" @click="openCreateInstance"><Icon name="plus" :size="16" />Новый экземпляр</button></div></div>
                <div v-if="homeWidgetViews.length" class="plugin-home-widgets">
                  <article v-for="entry in homeWidgetViews" :key="`${entry.pluginId}:${entry.view.id}`" class="plugin-home-widget panel">
                    <div class="panel-heading"><div><h2>{{ entry.view.title }}</h2></div><span class="tag">{{ entry.pluginName }}</span></div>
                    <PluginViewRenderer
                      :view="entry.view"
                      @action="(payload) => runPluginAction({ pluginId: entry.pluginId, ...payload })"
                      @config="(payload) => void savePluginConfig({ pluginId: entry.pluginId, key: payload.key, value: payload.value })"
                      @open-link="(url) => void openPluginLink({ pluginId: entry.pluginId, url })"
                    />
                  </article>
                </div>
              <Transition name="instance-create">
              <section v-if="createInstanceOpen" class="panel launch-panel instance-create-panel">
               <div class="panel-heading">
                  <div><h2>{{ creationKind === 'server' ? 'Создать сервер' : creationKind === 'bedrock' ? 'Создать Bedrock-профиль' : 'Создать экземпляр' }}</h2></div>
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
                <div v-if="creationKind === 'client'" class="field field--wide">
                  <span>Иконка экземпляра</span>
                  <BlockIconPicker v-model="creationIcon" :blocks="iconBlocks" :loading="iconBlocksLoading" />
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
                     <div v-if="serverCreation.install_mode === 'custom'" class="server-jar-drop" :class="{ 'server-jar-drop--active': serverCoreDragActive }" @dragenter.prevent="serverCoreDragActive = true" @dragover.prevent="serverCoreDragActive = true" @dragleave.prevent="serverCoreDragActive = false" @drop.prevent="onServerCoreDrop"><div><strong>{{ serverCreation.jar_name || 'Перетащите сюда server.jar' }}</strong><small>{{ serverCoreDragActive ? 'Отпустите файл, чтобы выбрать ядро.' : 'Перетащите JAR в эту область или выберите его на устройстве.' }}</small></div><button class="button button--small button--outline" type="button" @click="chooseServerCoreFile"><Icon name="upload" :size="14" />Выбрать JAR</button></div>
                    <label class="workspace-check server-eula-check"><input v-model="serverCreation.eula_accepted" type="checkbox" /><span><strong>Я принимаю EULA Minecraft</strong><small>Требуется только для запуска сервера.</small></span></label>
                 </div>
               <div class="launch-actions">
                  <button v-if="creationKind === 'client'" class="button button--quiet" type="button" :disabled="loadingVersions" @click="refreshVersions"><Icon name="refresh" :size="15" />{{ loadingVersions ? 'Загружаю…' : 'Обновить версии' }}</button>
                  <button class="button button--outline" type="button" @click="closeCreateInstance">Отмена</button>
                     <button class="button button--accent" type="button" :disabled="busy || (creationKind === 'bedrock' && (!bedrockCreationStatus || !bedrockCreationStatus.supported))" @click="installSelected"><Icon :name="creationKind === 'server' ? (serverCreation.install_mode === 'official' ? 'download' : 'server') : creationKind === 'bedrock' ? 'archive' : 'download'" :size="16" />{{ creationKind === 'server' ? 'Создать сервер' : creationKind === 'bedrock' ? (bedrockCreationVersion ? 'Создать и установить' : 'Создать профиль') : 'Создать и установить' }}</button>
               </div>
             </section>
              </Transition>
               <InstanceGroups
                  :instances="state.instances"
                  :groups="state.settings.instance_groups"
                  :pinned-groups="state.settings.pinned_instance_groups"
                  :active-filter="instanceGroupFilter"
                  :search-shortcut="settingsDraft.keyboard_shortcuts.focus_search"
                 :busy="busy"
                 :group-busy="groupActionBusy"
                 :layout="appearanceDraft.instance_layout"
                 :button-customization="appearanceDraft.button_customization"
                 @update:layout="setInstanceLayout"
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
                @compare="openInstanceCompare"
                @delete-instance="deleteInstance"
              />
              </section>

                <section data-page-view="instance-settings" class="page-content instance-workspace-page">
                  <div class="section-intro">
                    <div><h2>{{ selectedInstance?.display_name || selectedInstance?.name || 'Рабочая область экземпляра' }}</h2><p>{{ selectedInstance ? [selectedInstance.kind === 'server' ? (selectedInstance.server_core || 'Сервер') : selectedInstance.game_edition === 'bedrock' ? 'Bedrock Edition' : (selectedInstance.loader || 'Vanilla'), selectedInstance.minecraft_version || selectedInstance.version_id || ''].filter(Boolean).join(' · ') : '' }}</p></div>
                    <button class="button button--outline" type="button" @click="setView('instances')"><Icon name="arrow-left" :size="15" />К экземплярам</button>
                  </div>
                   <ServerWorkspace v-if="currentView === 'instance-settings' && selectedInstance?.kind === 'server'" ref="serverWorkspaceRef" :key="`workspace-server-${selectedInstance.name}`" :instance="selectedInstance" :plugin-catalogs="state.settings.plugin_catalogs" :server-cores="state.server_cores" :focus-tab="instanceWorkspaceFocus" :confirm-destructive="settingsDraft.confirm_destructive_actions" :file-editor-mode="settingsDraft.file_editor" :snapshot-created="notifyPluginSnapshotCreated" @toast="showToast" @changed="refreshBootstrap" />
                     <BedrockWorkspace v-else-if="currentView === 'instance-settings' && selectedInstance?.game_edition === 'bedrock'" ref="bedrockWorkspaceRef" :key="`workspace-bedrock-${selectedInstance.name}`" :instance="selectedInstance" :busy="busy" :focus-tab="instanceWorkspaceFocus" :confirm-destructive="settingsDraft.confirm_destructive_actions" :file-editor-mode="settingsDraft.file_editor" @launch="launchExisting(selectedInstance)" @stop="stopExisting(selectedInstance)" @toast="showToast" @changed="refreshBootstrap" @activity="onBedrockActivity" />
                     <InstanceWorkspace v-else-if="currentView === 'instance-settings' && selectedInstance" ref="instanceWorkspaceRef" :key="`workspace-client-${selectedInstance.name}`" :instance="selectedInstance" :account="selectedAccount" :account-info="effectiveAccount" :focus-tab="instanceWorkspaceFocus" :confirm-destructive="settingsDraft.confirm_destructive_actions" :file-editor-mode="settingsDraft.file_editor" :before-launch="beforeInstanceLaunch" :after-exit="notifyPluginAfterExit" :crash="notifyPluginCrash" :initial-crash="lastLaunchCrash" :snapshot-created="notifyPluginSnapshotCreated" @toast="showToast" @browse-mods="setView('mods')" @changed="refreshBootstrap" @groups-changed="state.settings.instance_groups = $event" @autotune="openInstanceAutotune(selectedInstance!)" />
                 <div v-else class="empty-card"><span class="empty-orb empty-orb--large"><Icon name="settings" :size="28" /></span><h3>Экземпляр не выбран</h3><p>Вернитесь к списку экземпляров и откройте настройки нужного профиля.</p><button class="button button--accent" type="button" @click="setView('instances')">Открыть экземпляры</button></div>
              </section>

               <AccountsPage
                 :accounts="state.accounts"
                 :has-client-id="state.settings.has_client_id"
                 :current-view="currentView"
                 :busy="busy"
                 :login-busy="loginBusy"
                 :login-enabled="state.settings.login_source === 'microsoft' && state.settings.has_client_id"
                 :ely-login-busy="elyLoginBusy"
                 v-model:account-name="accountName"
                 :account-name-error="accountNameError"
                 :confirm-destructive="confirmDestructive"
                 :refresh-bootstrap="refreshBootstrap"
                 :error-message="errorMessage"
                 :show-toast="showToast"
                 :open-skins-folder-browser="openSkinsFolderBrowser"
                 @update:accounts="state.accounts = $event"
                 @select-account="selectedAccount = $event"
                 @create-offline-account="createOfflineAccount"
                 @login-microsoft="loginMicrosoft"
                 @login-ely="openElyLogin"
               />

               <section data-page-view="mods" class="page-content">
                   <div class="section-intro"><div><h2>Каталог</h2><p>{{ modProjectType === 'plugin' ? 'Найдите плагины по версии Minecraft и совместимому server core. Установка доступна только после выбора сервера.' : 'Найдите моды, ресурспаки, шейдеры или датапаки по версии и loader. Установка доступна внутри выбранного профиля.' }}</p></div><div class="catalog-header-controls"><CustomSelect :model-value="activeCatalogId" :options="catalogPickerOptions" @update:model-value="selectActiveCatalog" /></div></div>
               <section class="panel search-panel">
                 <div class="catalog-instance-picker">
                   <span>Экземпляр для каталога</span>
                    <CustomSelect :model-value="selectedInstanceName" :options="instanceOptions" :disabled="!instanceOptions.length" @update:model-value="selectCatalogInstance" />
                 </div>
                  <div v-if="!selectedInstance" class="catalog-independent-fields">
                    <div class="field"><span>Версия Minecraft</span><CustomSelect v-model="selectedVersion" :options="versionOptions" /></div>
                    <div v-if="modProjectType !== 'plugin'" class="field"><span>Загрузчик</span><CustomSelect v-model="selectedLoader" :options="loaderOptions" /></div>
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
                 <div v-if="activeCatalog?.kind === 'modrinth' && modProjectType === 'mod'" class="catalog-category-filter catalog-filter"><span>Категория Modrinth</span><CustomSelect v-model="selectedModCategory" :options="modCategoryOptions" /></div>
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
              <div v-if="catalogSelectable && modResults.length" class="catalog-bulkbar">
                <button class="button button--small button--quiet" type="button" :disabled="modBusy" @click="selectCatalogPage"><Icon name="check" :size="13" />Выбрать все на странице</button>
                <span class="muted">{{ catalogBulkProgress || (catalogSelection.size ? `Выбрано: ${catalogSelection.size} (выбор сохраняется между страницами)` : 'Отметьте проекты, чтобы установить их разом') }}</span>
                <span class="catalog-bulkbar-actions">
                  <button v-if="catalogSelection.size" class="button button--small button--quiet" type="button" :disabled="modBusy" @click="clearCatalogSelection">Снять выделение</button>
                  <button class="button button--small button--accent" type="button" :disabled="modBusy || !catalogSelection.size" @click="installSelectedMods"><Icon name="download" :size="13" />{{ catalogBulkProgress ? 'Устанавливаю…' : `Установить выбранные${catalogSelection.size ? ` (${catalogSelection.size})` : ''}` }}</button>
                </span>
              </div>
              <div v-if="modResults.length" class="mod-grid">
                <article v-for="mod in modResults" :key="mod.project_id || mod.id" class="mod-card" :class="{ 'mod-card--selected': catalogSelection.has(catalogKey(mod)) }">
                  <label v-if="catalogSelectable && !mod.installed" class="mod-card-select" :title="catalogSelection.has(catalogKey(mod)) ? 'Убрать из выбранных' : 'Выбрать для установки'"><input type="checkbox" :checked="catalogSelection.has(catalogKey(mod))" :disabled="modBusy" :aria-label="`Выбрать ${mod.title || mod.name}`" @change="toggleCatalogSelection(mod)" /></label>
                  <div class="mod-icon"><img v-if="mod.icon_url" :src="mod.icon_url" alt="" @error="onModIconError(mod)" /><Icon v-else name="mods" :size="24" /></div>
                 <div class="mod-card-copy">
                   <h3>{{ mod.title || mod.name }}<span v-if="mod.author" class="mod-card-author"> by {{ mod.author }}</span></h3>
                   <p>{{ mod.description || 'Описание не предоставлено.' }}</p>
                   <ModCardMeta :mod="mod" />
                    <div class="mod-card-footer">
                       <span class="mod-card-stats"><span class="mod-stat" :title="mod.downloads ? `${mod.downloads.toLocaleString('ru-RU')} загрузок` : undefined"><Icon name="arrow-down-to-line" :size="12" />{{ formatDownloadsCompact(mod.downloads) || '0' }}</span><span v-if="formatLikes(mod.follows)" class="mod-stat mod-stat--likes" :title="`${formatLikes(mod.follows)} в избранном`"><Icon name="thumbs-up" :size="12" />{{ formatLikes(mod.follows) }}</span></span>
                       <span v-if="modInstallId === (mod.project_id || mod.id)" class="mod-install-status" aria-live="polite"><span class="status-dot status-dot--busy" />{{ modInstallDetail || 'Загрузка…' }} <strong>{{ modInstallProgress }}%</strong></span>
                       <button class="button button--small button--quiet" type="button" :aria-label="`Подробности: ${mod.title || mod.name}`" @click="openCatalogDetails(mod)"><Icon name="info" :size="14" />Информация</button>
                          <button class="button button--small mod-install-button" :class="mod.installed ? 'button--danger' : 'button--accent'" type="button" :disabled="modBusy || !selectedInstance || (modProjectType !== 'modpack' && selectedInstance.game_edition === 'bedrock') || (mod.installed && selectedInstance.running) || (modProjectType === 'plugin' && selectedInstance.running) || (modProjectType === 'modpack' && mod.installed)" :aria-busy="modInstallId === (mod.project_id || mod.id)" @click="modProjectType === 'modpack' ? installMod(mod) : (mod.installed ? removeMod(mod) : installMod(mod))"><Icon :name="modProjectType === 'modpack' && mod.installed ? 'check' : mod.installed ? 'trash' : 'download'" :size="14" />{{ modInstallId === (mod.project_id || mod.id) ? (modAction === 'remove' ? 'Удаляю…' : 'Установка…') : (modProjectType === 'modpack' ? (mod.installed ? 'Установлено' : 'Установить модпак') : (mod.installed ? 'Удалить' : (selectedInstance?.game_edition === 'bedrock' ? 'Выберите Java-экземпляр' : selectedInstance ? 'Установить' : 'Выберите экземпляр'))) }}</button>
                    </div>
                 </div>
               </article>
             </div>
             <nav v-if="modResults.length && (modPage > 0 || modHasMore)" class="catalog-pager" aria-label="Страницы каталога"><button class="button button--small button--quiet" type="button" :disabled="modBusy || modPage === 0" @click="goToCatalogPage(modPage - 1)">‹ Назад</button><template v-for="(item, index) in modPagerPages" :key="`page-${index}-${item}`"><span v-if="item < 0" class="catalog-pager-info">…</span><button v-else class="button button--small catalog-pager-page" :class="item === modPage ? 'button--accent' : 'button--quiet'" type="button" :disabled="modBusy" :aria-current="item === modPage ? 'page' : undefined" @click="goToCatalogPage(item)">{{ item + 1 }}</button></template><button class="button button--small button--quiet" type="button" :disabled="modBusy || !modHasMore" @click="goToCatalogPage(modPage + 1)">Вперёд ›</button><span class="catalog-pager-info">{{ modTotal !== null ? `Найдено: ${modTotal.toLocaleString()}` : `Страница ${modPage + 1}` }}</span></nav>
             <div v-if="!modResults.length" class="empty-card empty-card--short">
               <span class="empty-orb empty-orb--large"><Icon name="search" :size="27" /></span>
                <h3>{{ modError ? 'Каталог недоступен' : modBusy ? 'Ищу проекты…' : currentView === 'mods' && !selectedInstance && !modSearchPerformed ? 'Выберите экземпляр' : !modSearchPerformed ? 'Начните поиск' : 'Ничего не найдено' }}</h3>
                 <p>{{ modError || (modBusy ? 'Подбираю совместимые проекты для выбранной версии.' : currentView === 'mods' && !selectedInstance && !modSearchPerformed ? 'Можно просматривать каталог без экземпляра, но установить проект получится только в выбранный профиль.' : !modSearchPerformed ? 'Введите название проекта или нажмите «Показать всё».' : 'Попробуйте изменить запрос или выбрать другой тип ресурса.') }}</p>
              </div>
            </section>

               <section data-page-view="modpacks" class="page-content">
                 <div class="section-intro"><div><h2>Модпаки</h2><p>Каждая установка создаёт отдельный клиентский экземпляр. FTB, Technic и Modrinth не смешиваются с каталогом обычных ресурсов.</p></div><div class="catalog-header-controls"><CustomSelect :model-value="activeCatalogId" :options="catalogPickerOptions" @update:model-value="selectActiveCatalog" /></div></div>
                 <section class="panel search-panel">
                   <div class="catalog-filters catalog-filters--modpack"><div class="catalog-filter"><span>Версия нового экземпляра</span><CustomSelect v-model="selectedVersion" :options="versionOptions" /></div><div class="catalog-filter"><span>Загрузчик нового экземпляра</span><CustomSelect v-model="selectedLoader" :options="loaderOptions" /></div></div>
                   <div v-if="selectedLoader !== 'vanilla'" class="catalog-filter"><span>Версия {{ selectedLoaderLabel }} для нового экземпляра</span><CustomSelect v-model="selectedLoaderVersion" :options="loaderVersionOptions" :disabled="loadingChoices" /></div>
                   <p v-else class="field-help">Vanilla не использует отдельную версию загрузчика.</p>
                    <div class="search-bar"><Icon name="search" :size="19" /><input v-model="modQuery" placeholder="Например, Create или Better MC" @keyup.enter="searchMods" /><CustomSelect v-if="activeCatalog?.kind === 'modrinth'" v-model="modSortMode" :options="catalogSortOptions" /><button class="button button--outline" type="button" :disabled="busy || modBusy" @click="chooseModpackFile"><Icon name="upload" :size="14" />Импортировать архив</button><button class="button button--accent" type="button" :disabled="modBusy" @click="searchMods">{{ modBusy ? 'Загружаю…' : (modQuery.trim() ? 'Найти' : 'Показать всё') }}</button></div>
                   <div class="search-context"><span>Новый экземпляр</span><span class="context-divider" /><span>{{ modContextLabel }}</span><span class="context-divider" /><span>Источник: {{ activeCatalog?.name || 'Modrinth' }}</span></div>
                  <p v-if="modError" class="inline-error">{{ modError }}</p>
                </section>
                  <div v-if="modResults.length" class="mod-grid"><article v-for="mod in modResults" :key="mod.project_id || mod.id" class="mod-card" @contextmenu="modpackMenu($event, mod)"><div class="mod-icon"><img v-if="mod.icon_url" :src="mod.icon_url" alt="" @error="onModIconError(mod)" /><Icon v-else name="archive" :size="24" /></div><div class="mod-card-copy"><h3>{{ mod.title || mod.name }}<span v-if="mod.author" class="mod-card-author"> by {{ mod.author }}</span></h3><p>{{ mod.description || 'Описание не предоставлено.' }}</p><ModCardMeta :mod="mod" /><div class="mod-card-footer"><span class="mod-card-stats"><span class="mod-stat" :title="mod.downloads ? `${mod.downloads.toLocaleString('ru-RU')} загрузок` : undefined"><Icon name="arrow-down-to-line" :size="12" />{{ formatDownloadsCompact(mod.downloads) || '0' }}</span><span v-if="formatLikes(mod.follows)" class="mod-stat mod-stat--likes" :title="`${formatLikes(mod.follows)} в избранном`"><Icon name="thumbs-up" :size="12" />{{ formatLikes(mod.follows) }}</span></span><button class="button button--small button--quiet" type="button" :aria-label="`Подробности: ${mod.title || mod.name}`" @click="openCatalogDetails(mod)"><Icon name="info" :size="14" />Информация</button><button class="button button--small button--accent mod-install-button" type="button" :disabled="busy || modBusy || (selectedLoader !== 'vanilla' && loadingChoices)" @click="createModpackInstance(mod)"><Icon name="plus" :size="14" />Создать экземпляр</button></div></div></article></div>
                <nav v-if="modResults.length && (modPage > 0 || modHasMore)" class="catalog-pager" aria-label="Страницы каталога"><button class="button button--small button--quiet" type="button" :disabled="modBusy || modPage === 0" @click="goToCatalogPage(modPage - 1)">‹ Назад</button><template v-for="(item, index) in modPagerPages" :key="`page-${index}-${item}`"><span v-if="item < 0" class="catalog-pager-info">…</span><button v-else class="button button--small catalog-pager-page" :class="item === modPage ? 'button--accent' : 'button--quiet'" type="button" :disabled="modBusy" :aria-current="item === modPage ? 'page' : undefined" @click="goToCatalogPage(item)">{{ item + 1 }}</button></template><button class="button button--small button--quiet" type="button" :disabled="modBusy || !modHasMore" @click="goToCatalogPage(modPage + 1)">Вперёд ›</button><span class="catalog-pager-info">{{ modTotal !== null ? `Найдено: ${modTotal.toLocaleString()}` : `Страница ${modPage + 1}` }}</span></nav>
                <div v-if="!modResults.length" class="empty-card empty-card--short"><span class="empty-orb empty-orb--large"><Icon name="archive" :size="27" /></span><h3>{{ modError ? 'Каталог недоступен' : modBusy ? 'Ищу модпаки…' : !modSearchPerformed ? 'Начните поиск' : 'Ничего не найдено' }}</h3><p>{{ modError || (modBusy ? 'Подбираю сборки для выбранной версии.' : !modSearchPerformed ? 'Введите название сборки или нажмите «Показать всё».' : 'Попробуйте другой запрос или источник.') }}</p></div>
              </section>

              <section data-page-view="plugins" class="page-content">
                <section v-if="activeRuntimeTab" class="panel">
                  <div class="panel-heading">
                    <div><h2>{{ activeRuntimeTab.view.title }}</h2><p>{{ activeRuntimeTab.view.description || `Живая страница плагина ${activeRuntimeTab.pluginName}.` }}</p></div>
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
                    <div><h2>{{ activePluginTab.label }}</h2><p>{{ activePluginTab.description || `Вкладка плагина ${activePluginTab.name}.` }}</p></div>
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
                 :instances="state.instances"
                 :tokens="effectiveColorTokens"
                 :theme="currentTheme"
                 :base-theme="resolvedGlobalTheme"
                 :font-family="settingsDraft.font_family"
                 :language="settingsDraft.language || 'ru'"
                 :language-options="languageOptions"
                 :pages="customizablePages"
                 :wallpaper-url="wallpaperDisplayUrl"
                 :default-color-tokens="defaultColorTokens"
                 :default-radius-tokens="defaultRadiusTokens"
                 :light-tokens="lightColorTokens"
                 :save-state="settingsSaveState"
                 :save-message="settingsSaveMessage"
                 :busy="busy"
                 @update:font-family="settingsDraft.font_family = $event"
                 @update:language="setLanguageFromSettings"
                 @save="void saveSettings()"
                 @reset-appearance="resetAppearance"
                 @open-page="setView"
                 @import-wallpaper-file="importWallpaperFile"
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

             <SettingsPage :model="settingsPageModel" />
          </div>
        </div>
       </main>


              <Transition name="modal">
              <div v-if="elyLoginOpen" class="modal-scrim" role="presentation" @click.self="elyLoginOpen = false"><section class="modal-card ely-login-card" role="dialog" aria-modal="true" aria-labelledby="ely-login-title">
        <div class="panel-heading">
          <div>

            <h2 id="ely-login-title">Вход через ely.by</h2>
            <p>Аккаунт альтернативного сервиса: одиночная игра и серверы с авторизацией ely.by.</p>
          </div>
          <button class="icon-button" type="button" aria-label="Закрыть" @click="elyLoginOpen = false"><Icon name="close" :size="17" /></button>
        </div>
        <label class="field"><span>Логин или e-mail</span><input v-model="elyUsername" autocomplete="username" spellcheck="false" placeholder="example@mail.com" @keydown.enter.prevent="loginEly" /></label>
        <label class="field"><span>Пароль</span><input v-model="elyPassword" type="password" autocomplete="current-password" placeholder="Пароль ely.by" @keydown.enter.prevent="loginEly" /></label>
        <p v-if="elyLoginError" class="inline-error" role="alert">{{ elyLoginError }}</p>
        <p class="field-help">Пароль используется один раз для получения токена и не сохраняется: лаунчер держит только accessToken в системном хранилище секретов.</p>
        <div class="modal-actions">
          <button class="button button--quiet" type="button" :disabled="elyLoginBusy" @click="elyLoginOpen = false">Отмена</button>
          <button class="button button--accent" type="button" :disabled="elyLoginBusy || !elyUsername.trim() || !elyPassword" @click="loginEly"><Icon name="check" :size="15" />{{ elyLoginBusy ? 'Вхожу…' : 'Войти' }}</button>
        </div>
      </section></div>
      </Transition>
      <Transition name="modal">
      <div v-if="datapackDialogOpen" class="modal-scrim" role="presentation" @click.self="closeDatapackDialog"><section ref="datapackDialogElement" class="modal-card" role="dialog" tabindex="-1" aria-modal="true" aria-labelledby="datapack-dialog-title" @keydown="handleDatapackDialogKeydown"><div class="panel-heading"><div><h2 id="datapack-dialog-title">Выберите мир</h2><p>{{ pendingDatapack?.title || pendingDatapack?.name }}</p></div><button class="icon-button" type="button" aria-label="Закрыть" @click="closeDatapackDialog"><Icon name="close" :size="17" /></button></div><div class="datapack-world-list"><label v-for="world in datapackWorlds" :key="world.id" class="datapack-world-option" :class="{ 'datapack-world-option--active': datapackWorldId === world.id }"><input v-model="datapackWorldId" type="radio" :value="world.id" /><span class="world-icon world-icon--empty"><img v-if="world.icon" :src="world.icon" alt="" /><Icon v-else name="instances" :size="16" /></span><span><strong>{{ world.name }}</strong><small>{{ world.id }}</small></span></label></div><div class="modal-actions"><button class="button button--quiet" type="button" @click="closeDatapackDialog">Отмена</button><button class="button button--accent" type="button" :disabled="modBusy || !datapackWorldId" @click="confirmDatapackInstall"><Icon name="download" :size="15" />Установить в мир</button></div></section></div>
      </Transition>
      <Transition name="modal">
              <div v-if="modpackPreviewOpen" class="modal-scrim" role="presentation" @click.self="closeModpackPreview(false)"><section class="modal-card modpack-preview-card" role="dialog" aria-modal="true" aria-labelledby="modpack-preview-title">
                <div class="panel-heading">
                  <div>

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
      </Transition>
      <Transition name="catalog-details">
        <CatalogDetailsDialog v-if="catalogDetailsOpen" :details="catalogDetails" :versions="catalogDetailsVersions" :project-type="catalogDetailsType" :catalog-name="activeCatalog?.name || 'Каталог'" :loading="catalogDetailsLoading" :error="catalogDetailsError" :can-install="catalogDetailsType === 'modpack' || Boolean(selectedInstance)" :installed="Boolean(catalogDetailsProject?.installed)" :busy="modBusy" @close="closeCatalogDetails" @install="installFromCatalogDetails" />
      </Transition>
     <ActivityPanel :activity="activity" @close="closeActivityManually" @cancel="cancelActivity" />
      <Teleport to="body">
         <Transition name="toast"><div v-if="toast" :key="toastId" class="toast-message" :class="{ 'toast-message--action': toastAction }" :style="toastThemeStyle" role="status" aria-live="polite" @mouseenter="pauseToastTimer" @mouseleave="resumeToastTimer"><div class="toast-copy"><span class="status-dot" :class="toastToneClass" /><span class="toast-text">{{ toast }}</span><button v-if="toastAction" class="toast-action" type="button" @click="runToastAction"><Icon name="arrow-right" :size="12" />{{ toastAction.label }}</button></div><div class="toast-progress" aria-hidden="true"><span :style="{ animationDuration: `${toastDuration}ms` }" /></div></div></Transition>
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
      <Transition name="dialog">
      <InstanceCompareDialog
        v-if="compareDialogOpen"
        :instances="state.instances"
        :initial-source="compareDialogSource"
        @close="compareDialogOpen = false"
        @toast="showToast"
      />
      </Transition>
      <Transition name="dialog">
      <AutotuneDialog
        v-if="autotuneInstance"
        :instance="autotuneInstance"
        @close="autotuneInstance = null"
        @applied="onAutotuneApplied"
        @toast="showToast"
      />
      </Transition>
  </div>
</template>
