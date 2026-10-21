<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { openInLauncherExplorer, pickDirectory, pickFile, pickFiles } from '../services/fileBrowser'
import { openContextMenu, type ContextMenuItem } from '../services/contextMenu'
import CustomSelect from './CustomSelect.vue'
import Icon from './Icon.vue'
import Collapsible from './Collapsible.vue'
import LoaderBadge from './LoaderBadge.vue'
import MemoryField from './MemoryField.vue'
import ModAuditDialog from './ModAuditDialog.vue'
import NumericStepper from './NumericStepper.vue'
import ShaderAnalysisDialog from './ShaderAnalysisDialog.vue'
import UsageStatisticsDialog from './UsageStatisticsDialog.vue'
import { bridge } from '../services/bridge'
import { chooseDialog, confirmDialog, promptDialog } from '../services/dialogs'
import { diagnoseLaunchFailure, diagnoseManually, offerAutoRepair } from '../services/autoRepair'
import type { AccountInfo, BackupSchedule, CrashReport, InstanceAsset, InstanceConsoleColors, InstanceDetails, InstanceFile, InstanceInfo, InstanceLogArchive, InstanceSettings, InstanceVersionsPayload, InstanceVersionComponent, InstanceWorld, ModDependencyGraph, ProjectType, ResourceUpdate, SecurityPermission, SecurityProfile, ServerMetrics, ServerSnapshot, WorldBackup } from '../types'
import type { PluginInstanceRef } from '../../../plugins/api'

type WorkspaceTab = 'overview' | 'console' | 'mods' | 'resourcepacks' | 'shaders' | 'datapacks' | 'worlds' | 'servers' | 'versions' | 'snapshots' | 'backups' | 'screenshots' | 'files' | 'settings'

const props = withDefaults(defineProps<{
  instance?: InstanceInfo
  account?: string
  accountInfo?: AccountInfo
  confirmDestructive?: boolean
  focusTab?: WorkspaceTab
  settingsOnly?: boolean
  showSettingsTab?: boolean
  beforeLaunch?: (instance: PluginInstanceRef) => Promise<void>
  afterExit?: (instance: PluginInstanceRef, exitCode: number | null) => Promise<void>
  crash?: (instance: PluginInstanceRef, message?: string, exitCode?: number | null) => Promise<void>
  initialCrash?: CrashReport
  snapshotCreated?: (instance: PluginInstanceRef, snapshot: ServerSnapshot) => Promise<void>
}>(), { settingsOnly: false, showSettingsTab: true })
const emit = defineEmits<{
  (event: 'toast', message: string): void
  (event: 'browse-mods'): void
  (event: 'changed'): void
  (event: 'groups-changed', groups: string[]): void
}>()

const tabs: Array<{ id: WorkspaceTab; label: string; icon: string }> = [
  { id: 'overview', label: 'Главная', icon: 'home' },
  { id: 'console', label: 'Консоль', icon: 'activity' },
  { id: 'mods', label: 'Моды', icon: 'mods' },
  { id: 'resourcepacks', label: 'Ресурспаки', icon: 'archive' },
  { id: 'shaders', label: 'Шейдеры', icon: 'image' },
  { id: 'datapacks', label: 'Датапаки', icon: 'file' },
  { id: 'worlds', label: 'Миры', icon: 'instances' },
  { id: 'servers', label: 'Серверы', icon: 'globe' },
  { id: 'versions', label: 'Версии', icon: 'list' },
  { id: 'snapshots', label: 'Снапшоты', icon: 'archive' },
  { id: 'backups', label: 'Бэкапы', icon: 'shield' },
  { id: 'screenshots', label: 'Скриншоты', icon: 'image' },
  { id: 'files', label: 'Файлы', icon: 'folder' },
  { id: 'settings', label: 'Настройки', icon: 'settings' },
]
const defaultConsoleColors: InstanceConsoleColors = { error: '#ff657a', info: '#e6edf7', warning: '#f5c451' }
const consoleColorRows: Array<{ key: keyof InstanceConsoleColors; label: string; sample: string }> = [
  { key: 'error', label: 'Ошибки', sample: '[ERROR] Не удалось загрузить ресурс' },
  { key: 'info', label: 'Информация', sample: '[INFO] Мир успешно сохранён' },
  { key: 'warning', label: 'Предупреждения', sample: '[WARN] Устаревший параметр конфигурации' },
]

const graphicsOptions = [
  { value: 'auto', label: 'Автоматически', meta: 'Наследовать глобальный режим' },
  { value: 'nvidia', label: 'NVIDIA GLX', meta: 'Proprietary driver' },
  { value: 'mesa', label: 'Mesa GLX', meta: 'Mesa / compatibility' },
  { value: 'software', label: 'Software llvmpipe', meta: 'Медленный fallback' },
]

const securityProfileOptions: Array<{ value: SecurityProfile; label: string; meta: string }> = [
  { value: 'compat', label: 'Compatibility', meta: 'Старые моды: исходные HOME/XDG и без sandbox' },
  { value: 'manual', label: 'Manual', meta: 'Разрешения экземпляра и мягкая cell-изоляция' },
  { value: 'strict', label: 'Strict', meta: 'bubblewrap (Linux) / AppContainer (Windows), закрытая домашняя папка и deny-by-default' },
  { value: 'off', label: 'Off', meta: 'Без ограничений лаунчера, только для доверенных сборок' },
]

const securityPermissionOptions: Array<{ value: SecurityPermission; label: string; description: string; highRisk?: boolean }> = [
  { value: 'network:game', label: 'Сеть игры', description: 'Доступ к серверам: интернет и локальная сеть. Без него игра не находит ни один сервер', highRisk: true },
  { value: 'files:instance', label: 'Файлы экземпляра', description: 'Запись в папку текущего экземпляра' },
  { value: 'desktop:display', label: 'Экран игры', description: 'Окно игры и GPU; X11/XWayland может раскрывать экран и ввод других окон. На Windows без него закрыты буфер обмена и чужие окна', highRisk: true },
  { value: 'desktop:audio', label: 'Звук игры', description: 'Подключение к PipeWire/PulseAudio и доступ к звуковым устройствам; на Windows звук отдельно не запрещается', highRisk: true },
  { value: 'files:host-read', label: 'Чтение файлов компьютера', description: 'Доступ за пределами папки экземпляра; в Strict на Windows не поддерживается', highRisk: true },
  { value: 'files:host-write', label: 'Запись файлов компьютера', description: 'Изменение данных за пределами папки экземпляра', highRisk: true },
  { value: 'process:spawn', label: 'Запуск процессов', description: 'Не ограничивается внутри одной JVM; доступно только в Off', highRisk: true },
  { value: 'chat:read', label: 'Чтение чата и команд', description: 'Требует Java instrumentation; доступно только в Off', highRisk: true },
  { value: 'input:capture', label: 'Перехват ввода', description: 'Требует отдельного instrumentation слоя; доступно только в Off', highRisk: true },
  { value: 'native:load', label: 'Нативный код', description: 'Требует отдельного instrumentation слоя; доступно только в Off', highRisk: true },
]

const tab = ref<WorkspaceTab>(props.focusTab || (props.settingsOnly ? 'settings' : 'overview'))
const instance = computed(() => props.instance)
const workspaceRoot = ref<HTMLElement | null>(null)
const details = ref<InstanceDetails>()
const loading = ref(false)
const detailsError = ref('')
const saving = ref(false)
const settingsSaveState = ref<'saved' | 'dirty' | 'saving' | 'error'>('saved')
const settingsSaveMessage = ref('Изменения сохранены')
const syncingSettings = ref(false)
const importing = ref(false)
const profileIconInput = ref<HTMLInputElement | null>(null)
const profileBannerInput = ref<HTMLInputElement | null>(null)
const logs = ref<string[]>([])
const logArchives = ref<InstanceLogArchive[]>([])
const nextArchiveOffset = ref<number | null>(null)
const archiveListLoading = ref(false)
const archiveListOpen = ref(false)
const activeArchiveId = ref('')
const archiveText = ref('')
const archiveNextOffset = ref(0)
const archiveEof = ref(true)
const archiveLoading = ref(false)
const running = ref(false)
const metrics = ref<ServerMetrics>()
const metricHistory = ref<ServerMetrics[]>([])
const lastMetrics = ref<ServerMetrics | Record<string, unknown>>()
const usageDialogOpen = ref(false)
const versionsPayload = ref<InstanceVersionsPayload>()
const versionsLoading = ref(false)
const versionsBusy = ref(false)
const versionJsonInput = ref<HTMLInputElement | null>(null)
const shaderAnalysisTarget = ref<{ name: string; size?: number } | null>(null)
const consoleRef = ref<HTMLElement | null>(null)
const consoleStickToBottom = ref(true)
const instanceFiles = ref<InstanceFile[]>([])
const fileDirectory = ref('')
const fileLoading = ref(false)
const fileMutationBusy = ref(false)
const fileDropActive = ref(false)
const fileDragDepth = ref(0)
const editorLoading = ref(false)
const editorPath = ref('')
const editorContent = ref('')
const editorOriginalContent = ref('')
const editorSaving = ref(false)
const filePreview = ref<{ path: string; source: string }>()
const screenshotFiles = ref<InstanceFile[]>([])
const screenshotPreviews = ref<Array<{ file: InstanceFile; source: string }>>([])
const screenshotPage = ref(0)
const screenshotLoading = ref(false)
const SCREENSHOT_PAGE_SIZE = 6
const snapshotCategory = ref('mods')
const snapshotBusy = ref(false)
const profileGroups = ref<string[]>([])
const updateResults = ref<ResourceUpdate[]>([])
const checkingUpdates = ref(false)
const updatingResourceId = ref('')
const profileSaving = ref(false)
const profileIconFile = ref<{ fileName: string; data: string; size: number }>()
const profileBannerFile = ref<{ fileName: string; data: string; size: number }>()
const profileIconPreview = ref('')
const profileBannerPreview = ref('')
const clearProfileIcon = ref(false)
const clearProfileBanner = ref(false)
const failedAssetIcons = ref(new Set<string>())
const profileDraft = reactive({ display_name: '', description: '', group: '', notes: '', tags: '' })
const MAX_SESSION_LOG_LINES = 500
let liveLogCount = 0
let liveLogRevision = 0
let stopBackendEvents: (() => void) | undefined
let liveLogTimer: number | undefined
let liveLogPollInFlight = false
let workspaceRefreshTimer: number | undefined
let workspaceRefreshInFlight = false
let fileListRequestId = 0
let screenshotRequestId = 0
let loadPromise: Promise<void> | null = null
let loadPromiseName = ''
let loadRequestId = 0
const draft = reactive<InstanceSettings>({
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
  console_colors: { ...defaultConsoleColors },
  security_profile: 'strict',
  security_permissions: ['network:game', 'files:instance', 'desktop:display', 'desktop:audio'],
})

const activeAssets = computed<InstanceAsset[]>(() => {
  if (tab.value === 'datapacks') {
    return (details.value?.worlds || []).flatMap((world) =>
      (world.datapacks || []).map((asset) => ({
        ...asset,
        world_id: world.id,
        path: `${world.id}/${asset.path}`,
        display_name: `${world.name} / ${asset.display_name || asset.name}`,
        name: `${world.name} / ${asset.name}`,
        kind: 'datapacks',
      })),
    )
  }
  return details.value?.assets[tab.value] || []
})
const assetLabels: Record<string, string> = { mods: 'модов', resourcepacks: 'ресурспаков', shaders: 'шейдеров', datapacks: 'датапаков' }
const activeAssetLabel = computed(() => assetLabels[tab.value] || 'файлов')
const activeResourceProjectType = computed<Exclude<ProjectType, 'plugin' | 'modpack'> | undefined>(() => {
  if (tab.value === 'mods' || tab.value === 'resourcepacks' || tab.value === 'shaders' || tab.value === 'datapacks') return tab.value === 'mods' ? 'mod' : tab.value === 'resourcepacks' ? 'resourcepack' : tab.value === 'shaders' ? 'shader' : 'datapack'
  return undefined
})
const canCheckResourceUpdates = computed(() => Boolean(activeResourceProjectType.value))
const visibleTabs = computed(() => tabs.filter((item) =>
  props.showSettingsTab || item.id !== 'settings',
))
const snapshots = computed<ServerSnapshot[]>(() => details.value?.snapshots?.[snapshotCategory.value] || [])
const snapshotCount = computed(() => Object.values(details.value?.snapshots || {}).reduce((total, values) => total + values.length, 0))
const consoleDisplayLines = computed(() => {
  const source = activeArchiveId.value ? archiveText.value : logs.value.join('\n')
  return source ? source.replace(/\r/g, '').split('\n') : []
})
const selectedArchive = computed(() => logArchives.value.find((log) => log.id === activeArchiveId.value))
const fileBreadcrumbs = computed(() => fileDirectory.value.split('/').filter(Boolean).map((name, index, parts) => ({
  name,
  path: parts.slice(0, index + 1).join('/'),
})))
const screenshotPageCount = computed(() => Math.ceil(screenshotFiles.value.length / SCREENSHOT_PAGE_SIZE))
const assetAcceptValues: Partial<Record<WorkspaceTab, string>> = {
  mods: '.jar,.zip',
  resourcepacks: '.zip,.mcpack',
  shaders: '.zip',
}
const assetAccept = computed(() => assetAcceptValues[tab.value] || '.jar,.zip')

function assetIconSource(asset: InstanceAsset): string {
  return asset.icon_data || asset.icon_url || ''
}

function selectTab(value: WorkspaceTab) {
  tab.value = value
  archiveListOpen.value = false
  if (value === 'console') activeArchiveId.value = ''
  if (value === 'files') void loadWorkspaceFiles()
  if (value === 'screenshots') void loadScreenshots()
  if (value === 'versions') void loadVersions()
}

async function loadVersions() {
  if (!props.instance || versionsLoading.value) return
  versionsLoading.value = true
  try {
    versionsPayload.value = await bridge.instanceVersions(props.instance.name)
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    versionsLoading.value = false
  }
}

async function changeMinecraftVersion(component: InstanceVersionComponent) {
  if (!props.instance || versionsBusy.value) return
  try {
    const versions = await bridge.versions(true)
    const releases = versions.versions.filter((value) => value.type === 'release').slice(0, 60)
    const choice = await chooseDialog(
      `Текущая версия: ${component.version}`,
      releases.map((value) => ({ value: value.id, label: value.id, description: formatDate(Math.round(new Date(value.release_time).getTime() / 1000)) })),
      { title: 'Смена версии Minecraft', detail: 'Моды, миры и настройки сохранятся; файлы версии будут скачаны заново. Моды, собранные под старую версию, могут не запуститься.', confirmLabel: 'Сменить' },
    )
    if (!choice || choice === component.version) return
    if (!(await confirmDialog(`Заменить Minecraft ${component.version} на ${choice} в этом экземпляре?`, { confirmLabel: 'Заменить' }))) return
    versionsBusy.value = true
    await bridge.repairInstanceVersion(props.instance.name, { version: choice })
    emit('toast', `Версия Minecraft изменена на ${choice}`)
    emit('changed')
    await loadVersions()
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    versionsBusy.value = false
  }
}

async function changeLoader(component: InstanceVersionComponent) {
  if (!props.instance || versionsBusy.value) return
  try {
    const loaderKinds: Array<{ value: string; label: string; description: string }> = [
      { value: 'vanilla', label: 'Vanilla', description: 'Без модлоадера' },
      { value: 'fabric', label: 'Fabric', description: 'Легкий модлоадер, свежие версии игры' },
      { value: 'quilt', label: 'Quilt', description: 'Форк Fabric' },
      { value: 'forge', label: 'Forge', description: 'Классика модов (установка требует Java)' },
      { value: 'neoforge', label: 'NeoForge', description: 'Современный форк Forge' },
    ]
    const choice = await chooseDialog(
      `Текущий модлоадер: ${component.title}${component.version !== '—' ? ` ${component.version}` : ''}`,
      loaderKinds,
      { title: 'Смена модлоадера', detail: 'Моды, миры и настройки сохранятся. Моды другого модлоадера работать не будут.', confirmLabel: 'Далее' },
    )
    if (!choice) return
    let loaderVersion = ''
    if (choice !== 'vanilla') {
      const result = await bridge.loaderChoices(choice as 'fabric', props.instance.minecraft_version || component.version)
      const available = result.choices.slice(0, 30)
      if (!available.length) {
        emit('toast', `Для ${props.instance.minecraft_version || component.version} нет сборок ${choice}`)
        return
      }
      loaderVersion = await chooseDialog(
        'Версия модлоадера',
        available.map((value) => ({ value: value.version, label: value.version, description: '' })),
        { title: choice, detail: 'Обычно подходит свежая версия.', confirmLabel: 'Применить' },
      ) || available[0].version
    } else if (component.removable) {
      if (!(await confirmDialog('Убрать модлоадер и вернуть чистую версию Minecraft?', { confirmLabel: 'Убрать' }))) return
    }
    versionsBusy.value = true
    await bridge.repairInstanceVersion(props.instance.name, { loader: choice as 'vanilla', loader_version: loaderVersion })
    emit('toast', choice === 'vanilla' ? 'Экземпляр переведён на чистую версию' : `Модлоадер изменён на ${choice}`)
    emit('changed')
    await loadVersions()
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    versionsBusy.value = false
  }
}

async function changeLwjgl(component: InstanceVersionComponent) {
  if (!props.instance || versionsBusy.value || !versionsPayload.value) return
  try {
    const options = versionsPayload.value.lwjgl_versions.map((value) => ({
      value,
      label: `LWJGL ${value}`,
      description: value === component.version ? 'текущая версия из version JSON' : '',
    }))
    if (component.overridden) options.unshift({ value: '__reset__', label: 'Вернуть версию из version JSON', description: `сейчас переопределено на ${component.overridden}` })
    const choice = await chooseDialog(
      `Текущая версия: ${component.version}${component.overridden ? ` (переопределена на ${component.overridden})` : ''}`,
      options,
      { title: 'Версия LWJGL 3', detail: 'Библиотека окна, ввода и OpenGL. Новые сборки качаются с Maven Central и помогают с багами драйверов на новых версиях.', confirmLabel: 'Применить' },
    )
    if (!choice) return
    const overrides = { ...(versionsPayload.value.library_overrides || {}) }
    if (choice === '__reset__') {
      delete overrides['org.lwjgl:lwjgl']
      delete overrides['org.lwjgl:lwjgl-glfw']
      delete overrides['org.lwjgl:lwjgl-freetype']
      delete overrides['org.lwjgl:lwjgl-jemalloc']
      delete overrides['org.lwjgl:lwjgl-openal']
      delete overrides['org.lwjgl:lwjgl-opengl']
      delete overrides['org.lwjgl:lwjgl-stb']
      delete overrides['org.lwjgl:lwjgl-tinyfd']
    } else {
      for (const name of ['org.lwjgl:lwjgl', 'org.lwjgl:lwjgl-glfw', 'org.lwjgl:lwjgl-freetype', 'org.lwjgl:lwjgl-jemalloc', 'org.lwjgl:lwjgl-openal', 'org.lwjgl:lwjgl-opengl', 'org.lwjgl:lwjgl-stb', 'org.lwjgl:lwjgl-tinyfd']) {
        overrides[name] = choice
      }
    }
    versionsBusy.value = true
    await bridge.instanceVersionOverrideSave(props.instance.name, overrides)
    emit('toast', choice === '__reset__' ? 'Версия LWJGL возвращена к версии Minecraft' : `LWJGL переопределён на ${choice}`)
    await loadVersions()
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    versionsBusy.value = false
  }
}

async function chooseVersionJson() {
  const file = await pickFile({ accept: '.json,application/json', title: 'JSON-манифест версии' })
  if (!file) return
  await importVersionJson(file)
}

async function importVersionJson(file: { fileName: string; data: string; size: number }) {
  if (!props.instance) return
  if (file.size > 4 * 1024 * 1024) {
    emit('toast', 'Файл версии превышает лимит 4 МиБ')
    return
  }
  if (!(await confirmDialog(`Установить свою версию «${file.fileName}» в этот экземпляр? Файлы версии и библиотеки будут скачаны.`, { confirmLabel: 'Установить' }))) return
  versionsBusy.value = true
  try {
    await bridge.instanceVersionImport(props.instance.name, file.fileName, file.data)
    emit('toast', `Своя версия «${file.fileName.replace(/\.json$/i, '')}» установлена`)
    emit('changed')
    await loadVersions()
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    versionsBusy.value = false
  }
}

function consoleLineColor(line: string): string {
  const colors = draft.console_colors || defaultConsoleColors
  if (/\b(ERROR|FATAL)\b|\bException\b|\bCaused by:/i.test(line)) return colors.error
  if (/\b(WARN|WARNING)\b|\bdeprecated\b/i.test(line)) return colors.warning
  return colors.info
}

function recordMetrics(value?: ServerMetrics) {
  if (!value) return
  metrics.value = value
  metricHistory.value = [...metricHistory.value, value].slice(-60)
}

function metricPoints(key: 'cpu_percent' | 'ram_bytes' | 'storage_bytes'): string {
  const values = metricHistory.value.map((value) => Number(value[key] || 0))
  if (!values.length) return '0,40 100,40'
  const min = Math.min(...values)
  const max = Math.max(...values)
  const span = Math.max(1, max - min)
  return values.map((value, index) => `${(index / Math.max(1, values.length - 1)) * 100},${38 - ((value - min) / span) * 32}`).join(' ')
}

function formatMetricBytes(value: number): string {
  if (value >= 1024 ** 3) return `${(value / 1024 ** 3).toFixed(1)} GiB`
  if (value >= 1024 ** 2) return `${(value / 1024 ** 2).toFixed(0)} MiB`
  if (value >= 1024) return `${(value / 1024).toFixed(0)} KiB`
  return `${Math.max(0, Math.round(value || 0))} B`
}

async function scrollConsoleIfNeeded() {
  await nextTick()
  if (consoleStickToBottom.value && consoleRef.value) consoleRef.value.scrollTop = consoleRef.value.scrollHeight
}

function onConsoleScroll() {
  if (!consoleRef.value) return
  consoleStickToBottom.value = consoleRef.value.scrollHeight - consoleRef.value.scrollTop - consoleRef.value.clientHeight < 24
}

async function toggleLogHistory() {
  archiveListOpen.value = !archiveListOpen.value
  if (archiveListOpen.value) await loadLogArchives(false)
}

async function loadLogArchives(append: boolean) {
  if (!props.instance || archiveListLoading.value) return
  archiveListLoading.value = true
  try {
    const offset = append ? nextArchiveOffset.value : 0
    if (append && offset === null) return
    const result = await bridge.instanceLogArchives(props.instance.name, offset || 0, 50)
    logArchives.value = append ? [...logArchives.value, ...result.logs] : result.logs
    nextArchiveOffset.value = result.next_offset
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    archiveListLoading.value = false
  }
}

async function openLogArchive(log: InstanceLogArchive) {
  selectTab('console')
  activeArchiveId.value = log.id
  archiveText.value = ''
  archiveNextOffset.value = 0
  archiveEof.value = false
  await loadMoreArchive()
}

async function loadMoreArchive() {
  if (!props.instance || !activeArchiveId.value || archiveLoading.value || archiveEof.value) return
  archiveLoading.value = true
  try {
    const result = await bridge.instanceLogArchiveRead(
      props.instance.name,
      activeArchiveId.value,
      archiveNextOffset.value,
    )
    archiveText.value += result.content
    archiveNextOffset.value = result.next_offset
    archiveEof.value = result.eof
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    archiveLoading.value = false
  }
}

function markAssetIconFailed(path: string) {
  failedAssetIcons.value = new Set(failedAssetIcons.value).add(path)
}

function assetFallbackIcon(asset: InstanceAsset): 'file' | 'folder' | 'image' {
  if (asset.kind === 'mods') return 'file'
  if (asset.kind === 'datapacks') return 'folder'
  return 'image'
}

function assetProvider(asset: InstanceAsset): string {
  if (asset.source?.toLowerCase() === 'modrinth') return 'Modrinth'
  return asset.source || 'Локальный файл'
}

function formatSize(value: number): string {
  if (value >= 1024 ** 3) return `${(value / 1024 ** 3).toFixed(1)} GiB`
  if (value >= 1024 ** 2) return `${(value / 1024 ** 2).toFixed(1)} MiB`
  if (value >= 1024) return `${(value / 1024).toFixed(1)} KiB`
  return `${value} B`
}

function formatDate(value: number | null): string {
  if (!value) return 'дата не записана'
  return new Date(value > 10_000_000_000 ? value : value * 1000).toLocaleString()
}

function formatPlayedTime(value: number): string {
  const seconds = Math.max(0, Math.round(value || 0))
  const hours = Math.floor(seconds / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  if (hours) return `${hours} ч ${minutes} мин`
  return `${minutes} мин`
}

async function load(instanceName: string, requestId: number) {
  if (!props.instance || props.instance.name !== instanceName) {
    details.value = undefined
    return
  }
  loading.value = true
  detailsError.value = ''
  try {
    const next = await bridge.instanceDetails(instanceName)
    if (requestId !== loadRequestId || props.instance?.name !== instanceName) return
    details.value = next
    syncProfileDraft(details.value.instance)
    syncingSettings.value = true
    Object.assign(draft, details.value.settings)
    void nextTick(() => { syncingSettings.value = false })
    running.value = Boolean(details.value.instance.running)
    metricHistory.value = []
  } catch (error) {
    if (requestId !== loadRequestId || props.instance?.name !== instanceName) return
    detailsError.value = errorMessage(error)
    if (details.value) emit('toast', detailsError.value)
  } finally {
    if (requestId === loadRequestId) loading.value = false
  }
}

function queueLoad(force = false): Promise<void> {
  const instanceName = props.instance?.name
  if (!instanceName) {
    loadRequestId += 1
    loadPromise = null
    loadPromiseName = ''
    loading.value = false
    details.value = undefined
    return Promise.resolve()
  }
  if (loadPromise && loadPromiseName === instanceName) return loadPromise
  if (!force && details.value) return Promise.resolve()
  const requestId = ++loadRequestId
  loadPromiseName = instanceName
  loadPromise = load(instanceName, requestId).finally(() => {
    if (requestId === loadRequestId) {
      loadPromise = null
      loadPromiseName = ''
    }
  })
  return loadPromise
}

async function ensureLoaded() {
  await queueLoad()
}

function syncProfileDraft(value: InstanceInfo) {
  profileDraft.display_name = value.display_name || value.name
  profileDraft.description = value.description || ''
  profileDraft.group = value.group || ''
  profileDraft.notes = value.notes || ''
  profileDraft.tags = (value.tags || []).join(', ')
  profileIconPreview.value = value.icon || ''
  profileBannerPreview.value = value.banner || ''
  profileIconFile.value = undefined
  profileBannerFile.value = undefined
  clearProfileIcon.value = false
  clearProfileBanner.value = false
}

async function loadProfileGroups() {
  try {
    profileGroups.value = (await bridge.instanceGroups()).groups || []
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

const profileGroupOptions = computed(() => [
  { value: '', label: 'Без группы', meta: 'Не добавлять в подборку' },
  ...profileGroups.value.map((group) => ({ value: group, label: group })),
])

function chooseProfileAsset(kind: 'icon' | 'banner') {
  void pickFile({
    accept: 'image/png,image/jpeg,image/webp',
    title: kind === 'icon' ? 'Иконка экземпляра' : 'Баннер экземпляра',
  }).then((file) => {
    if (file) loadProfileAsset(file, kind)
  })
}

// ── Ярлыки экземпляра (рабочий стол / меню приложений) ──────────────
const shortcutBusy = ref(false)
const shortcutStatus = ref<{ supported: boolean; platform: string; desktop: { exists: boolean; path: string }; menu: { exists: boolean; path: string } } | null>(null)

async function refreshShortcutStatus() {
  if (!props.instance) return
  try {
    shortcutStatus.value = await bridge.instanceShortcutStatus(props.instance.name)
  } catch {
    // недоступно (например, браузерный предпросмотр) — просто не показываем
    shortcutStatus.value = null
  }
}

async function toggleShortcut(location: 'desktop' | 'menu') {
  if (!props.instance || shortcutBusy.value) return
  shortcutBusy.value = true
  try {
    if (shortcutStatus.value?.[location]?.exists) {
      await bridge.instanceShortcutRemove(props.instance.name, location)
      emit('toast', location === 'desktop' ? 'Ярлык убран с рабочего стола' : 'Ярлык убран из меню приложений')
    } else {
      const created = await bridge.instanceShortcutCreate(props.instance.name, location)
      emit('toast', location === 'desktop' ? `Ярлык создан: ${created.path}` : `Ярлык создан: ${created.path}`)
    }
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    shortcutBusy.value = false
    void refreshShortcutStatus()
  }
}

// ── Экспорт списка модов (Markdown / CSV / текст) ───────────────────
const modListBusy = ref(false)

async function exportModList(format: 'markdown' | 'csv' | 'text') {
  if (!props.instance || modListBusy.value) return
  modListBusy.value = true
  try {
    const result = await bridge.instanceModListExport(props.instance.name, format)
    const url = URL.createObjectURL(new Blob([result.content], { type: format === 'csv' ? 'text/csv;charset=utf-8' : 'text/plain;charset=utf-8' }))
    const link = document.createElement('a')
    link.href = url
    link.download = result.file_name
    link.click()
    window.setTimeout(() => URL.revokeObjectURL(url), 1000)
    emit('toast', result.count > 0 ? `Список модов сохранён: ${result.file_name} (${result.count})` : 'В экземпляре нет модов — файл пустой')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    modListBusy.value = false
  }
}

function profileAssetMime(name: string): string {  const lower = name.toLowerCase()
  if (lower.endsWith('.png')) return 'image/png'
  if (lower.endsWith('.webp')) return 'image/webp'
  return 'image/jpeg'
}

function loadProfileAsset(file: { fileName: string; data: string; size: number }, kind: 'icon' | 'banner') {
  const limit = kind === 'icon' ? 2 * 1024 * 1024 : 8 * 1024 * 1024
  if (file.size > limit) {
    emit('toast', `Файл ${kind === 'icon' ? 'иконки' : 'баннера'} слишком большой`)
    return
  }
  const preview = `data:${profileAssetMime(file.fileName)};base64,${file.data}`
  if (kind === 'icon') {
    if (profileIconPreview.value.startsWith('blob:')) URL.revokeObjectURL(profileIconPreview.value)
    profileIconFile.value = file
    profileIconPreview.value = preview
    clearProfileIcon.value = false
  } else {
    if (profileBannerPreview.value.startsWith('blob:')) URL.revokeObjectURL(profileBannerPreview.value)
    profileBannerFile.value = file
    profileBannerPreview.value = preview
    clearProfileBanner.value = false
  }
}

function clearProfileAsset(kind: 'icon' | 'banner') {
  if (kind === 'icon') {
    if (profileIconPreview.value.startsWith('blob:')) URL.revokeObjectURL(profileIconPreview.value)
    profileIconFile.value = undefined
    profileIconPreview.value = ''
    clearProfileIcon.value = true
  } else {
    if (profileBannerPreview.value.startsWith('blob:')) URL.revokeObjectURL(profileBannerPreview.value)
    profileBannerFile.value = undefined
    profileBannerPreview.value = ''
    clearProfileBanner.value = true
  }
}

async function saveProfile() {
  if (!props.instance) return
  profileSaving.value = true
  try {
    const payload: Parameters<typeof bridge.updateInstanceProfile>[0] = {
      name: props.instance.name,
      display_name: profileDraft.display_name,
      description: profileDraft.description,
      group: profileDraft.group,
      notes: profileDraft.notes,
      tags: profileDraft.tags.split(',').map((tag) => tag.trim()).filter(Boolean).slice(0, 12),
      clear_icon: clearProfileIcon.value,
      clear_banner: clearProfileBanner.value,
    }
    if (profileIconFile.value) {
      payload.icon_file_name = profileIconFile.value.fileName
      payload.icon_data = profileIconFile.value.data
    }
    if (profileBannerFile.value) {
      payload.banner_file_name = profileBannerFile.value.fileName
      payload.banner_data = profileBannerFile.value.data
    }
    const result = await bridge.updateInstanceProfile(payload)
    profileGroups.value = result.groups || profileGroups.value
    if (details.value) details.value.instance = result.instance
    syncProfileDraft(result.instance)
    emit('changed')
    emit('toast', 'Профиль экземпляра сохранён')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    profileSaving.value = false
  }
}

async function createProfileGroup() {
  const name = (await promptDialog('Название новой группы', { placeholder: 'Например, Тестовая сборка', confirmLabel: 'Создать' }))?.trim().slice(0, 40)
  if (!name) return
  try {
    profileGroups.value = (await bridge.createInstanceGroup(name)).groups
    profileDraft.group = name
    emit('groups-changed', profileGroups.value)
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

async function refreshWorkspaceSnapshot() {
  if (!props.instance || loading.value || saving.value || importing.value || workspaceRefreshInFlight) return
  workspaceRefreshInFlight = true
  try {
    const next = await bridge.instanceDetails(props.instance.name)
    details.value = next
    syncProfileDraft(next.instance)
    running.value = Boolean(next.instance.running)
  } catch {
    // polling is best effort; the next event or interval will retry.
  } finally {
    workspaceRefreshInFlight = false
  }
}

function restartWorkspacePolling() {
  if (workspaceRefreshTimer) window.clearInterval(workspaceRefreshTimer)
  workspaceRefreshTimer = undefined
  if (!props.instance) return
  workspaceRefreshTimer = window.setInterval(() => {
    if (!document.hidden) void refreshWorkspaceSnapshot()
  }, 15000)
}

async function pollLiveLog() {
  if (!props.instance || liveLogPollInFlight) return
  liveLogPollInFlight = true
  const revision = liveLogRevision
  try {
    const result = await bridge.instanceLog(props.instance.name)
    if (revision !== liveLogRevision) return
    const nextLines = result.lines || []
    const previousLast = logs.value[logs.value.length - 1]
    const nextLast = nextLines[nextLines.length - 1]
    if (nextLines.length !== logs.value.length || nextLast !== previousLast) {
      logs.value = nextLines.slice(-MAX_SESSION_LOG_LINES)
    }
    running.value = result.running
    recordMetrics(result.metrics)
    if (result.last_metrics) lastMetrics.value = result.last_metrics
    void scrollConsoleIfNeeded()
  } catch {
    // the event stream remains the fast path while the backend is restarting.
  } finally {
    liveLogPollInFlight = false
  }
}

function restartLiveLogPolling() {
  if (liveLogTimer) window.clearInterval(liveLogTimer)
  liveLogTimer = undefined
  if (!props.instance) return
  void pollLiveLog()
  liveLogTimer = window.setInterval(() => {
    if (!document.hidden) void pollLiveLog()
  }, 1000)
}

async function save() {
  if (!props.instance || saving.value || settingsSaveState.value === 'saved') return
  settingsSaveState.value = 'saving'
  settingsSaveMessage.value = 'Сохраняю изменения…'
  saving.value = true
  try {
    const result = await bridge.saveInstanceSettings(props.instance.name, { ...draft })
    Object.assign(draft, result.settings)
    if (details.value) details.value.settings = result.settings
    settingsSaveState.value = 'saved'
    settingsSaveMessage.value = 'Изменения сохранены'
    emit('toast', 'Настройки экземпляра сохранены')
  } catch (error) {
    settingsSaveState.value = 'error'
    settingsSaveMessage.value = errorMessage(error)
    emit('toast', errorMessage(error))
  } finally {
    saving.value = false
  }
}

// ── Краш-баннер ─────────────────────────────────────────────────────────────
const lastCrash = ref<CrashReport>()
watch(() => props.initialCrash, (value) => {
  if (value) lastCrash.value = value
}, { immediate: true })

async function removeSuspectedMod(file: string) {
  if (!props.instance || !file) return
  if (props.confirmDestructive !== false && !(await confirmDialog(`Удалить мод ${file}?`, { danger: true, confirmLabel: 'Удалить' }))) return
  try {
    details.value = await bridge.removeInstanceAsset(props.instance.name, 'mods', file)
    if (lastCrash.value) lastCrash.value.suspected_mods = lastCrash.value.suspected_mods.filter((mod) => mod.file !== file)
    emit('toast', `Мод ${file} удалён`)
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

// ── Резервные копии миров ───────────────────────────────────────────────────
const backups = ref<WorldBackup[]>([])
const backupsLoading = ref(false)
const backupBusy = ref(false)
const backupSchedule = reactive<BackupSchedule>({ enabled: false, interval_hours: 24, keep: 5, destination: 'local', override: false })
const backupDestinationOptions = [
  { value: 'local', label: 'Локально', meta: 'game_dir/backups' },
  { value: 'cloud', label: 'WebDAV', meta: 'Тот же endpoint, что у облачной синхронизации' },
]

async function loadBackups() {
  if (!props.instance) return
  backupsLoading.value = true
  try {
    const result = await bridge.worldBackupList(props.instance.name)
    backups.value = result.backups
    Object.assign(backupSchedule, result.schedule)
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    backupsLoading.value = false
  }
}

async function saveBackupSchedule() {
  if (!props.instance) return
  backupBusy.value = true
  try {
    const result = await bridge.worldBackupScheduleSet(props.instance.name, {
      enabled: backupSchedule.enabled,
      interval_hours: backupSchedule.interval_hours,
      keep: backupSchedule.keep,
      destination: backupSchedule.destination,
    }, backupSchedule.override)
    Object.assign(backupSchedule, result.schedule)
    emit('toast', 'Расписание резервных копий сохранено')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    backupBusy.value = false
  }
}

async function createBackupNow() {
  if (!props.instance) return
  backupBusy.value = true
  try {
    const result = await bridge.worldBackupCreate(props.instance.name, backupSchedule.destination)
    backups.value = (await bridge.worldBackupList(props.instance.name)).backups
    emit('toast', `Резервная копия создана: ${result.backup.backup_id} (${result.backup.files} файлов)`)
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    backupBusy.value = false
  }
}

async function restoreBackup(backup: WorldBackup) {
  if (!props.instance) return
  if (!(await confirmDialog(`Восстановить миры из копии ${formatDate(Math.round(backup.created_at))}? Текущие миры будут сохранены рядом.`, { confirmLabel: 'Восстановить' }))) return
  backupBusy.value = true
  try {
    const result = await bridge.worldBackupRestore(props.instance.name, backup.id)
    emit('toast', result.restored.length ? `Восстановлено миров: ${result.restored.length}` : 'Копия восстановлена')
    await loadBackups()
    await refreshWorkspaceSnapshot()
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    backupBusy.value = false
  }
}

async function deleteBackup(backup: WorldBackup) {
  if (!props.instance) return
  if (!(await confirmDialog(`Удалить копию ${formatDate(Math.round(backup.created_at))}?`, { danger: true, confirmLabel: 'Удалить' }))) return
  try {
    backups.value = (await bridge.worldBackupDelete(props.instance.name, backup.id)).backups
    emit('toast', 'Копия удалена')
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

// ── Дерево зависимостей модов ───────────────────────────────────────────────
const depGraph = ref<ModDependencyGraph>()
const depGraphOpen = ref(false)
const depGraphLoading = ref(false)
const depImpact = ref<Record<string, string[]>>({})

async function openDependencyGraph() {
  if (!props.instance) return
  depGraphOpen.value = true
  depGraphLoading.value = true
  try {
    depGraph.value = await bridge.modDependencyGraph(props.instance.name)
  } catch (error) {
    emit('toast', errorMessage(error))
    depGraphOpen.value = false
  } finally {
    depGraphLoading.value = false
  }
}

async function checkRemovalImpact(nodeId: string) {
  if (!props.instance || depImpact.value[nodeId]) return
  try {
    const result = await bridge.modRemovalImpact(props.instance.name, nodeId)
    depImpact.value = { ...depImpact.value, [nodeId]: result.breaks }
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

// ── Экспорт модпака ─────────────────────────────────────────────────────────
const exportBusy = ref(false)
const duplicateBusy = ref(false)

const canExportModpack = computed(() => {
  const instance = props.instance
  return Boolean(instance && (instance.game_edition || 'java') === 'java' && instance.loader !== 'vanilla')
})

const repairBusy = ref(false)
const diagnoseBusy = ref(false)
async function diagnoseInstance(exitCode?: number | null) {
  if (!props.instance || diagnoseBusy.value) return
  diagnoseBusy.value = true
  try {
    await diagnoseManually(props.instance.name, exitCode)
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    diagnoseBusy.value = false
  }
}

async function repairVersion() {
  if (!props.instance) return
  if (!(await confirmDialog('Пересоздать версию и модлоадер этого экземпляра? Моды, конфиги, миры и настройки не затрагиваются; файлы версии будут скачаны заново.', { confirmLabel: 'Восстановить' }))) return
  repairBusy.value = true
  try {
    const result = await bridge.repairInstanceVersion(props.instance.name)
    emit('toast', `Версия восстановлена: ${result.version_id}`)
    emit('changed')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    repairBusy.value = false
  }
}

async function duplicateCurrent() {
  if (!props.instance) return
  const target = await promptDialog(`Имя копии экземпляра «${props.instance.display_name || props.instance.name}»`, { placeholder: 'latin-имя-копии', initialValue: `${props.instance.name}-copy` })
  if (!target?.trim()) return
  duplicateBusy.value = true
  try {
    await bridge.duplicateInstance(props.instance.name, target.trim())
    emit('toast', `Экземпляр скопирован: ${target.trim()}`)
    emit('changed')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    duplicateBusy.value = false
  }
}

async function exportModpack() {
  if (!props.instance) return
  const label = await promptDialog('Метка версии экспорта', { placeholder: 'например 1.0 или дата' })
  if (label === undefined) return
  exportBusy.value = true
  try {
    const result = await bridge.exportModpack(props.instance.name, props.instance.display_name, label.trim())
    emit('toast', `Модпак собран: ${result.file_name} (${result.indexed_files} по индексу, ${result.override_files} в overrides)`)
    openInLauncherExplorer(result.path.replace(/[\\/][^\\/]+$/, ''))
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    exportBusy.value = false
  }
}

// JVM-аргументы экземпляра: по одному на строку, как в профилях запуска
function setDraftArguments(key: 'jvm_arguments' | 'game_arguments', event: Event) {
  const value = (event.target as HTMLTextAreaElement).value
  draft[key] = value.split('\n').map((line) => line.trim()).filter(Boolean).slice(0, 32)
}

const jvmPresetOptions = [
  { value: 'none', label: 'Без пресета', meta: 'Только аргументы экземпляра' },
  { value: 'fps', label: 'FPS', meta: 'Минимальные паузы GC' },
  { value: 'balanced', label: 'Balanced', meta: 'Универсальный баланс' },
  { value: 'heavy_modpack', label: 'Heavy Modpack', meta: 'Флаги Aikar для больших сборок' },
  { value: 'compatibility', label: 'Compatibility', meta: 'SerialGC, без экспериментов' },
  { value: 'streaming', label: 'Streaming', meta: 'Стабильные кадры при стриме' },
]

async function removeAsset(asset: InstanceAsset) {
  if (!props.instance || (props.confirmDestructive !== false && !(await confirmDialog(`Удалить «${asset.name}»?`, { danger: true, confirmLabel: 'Удалить' })))) return
  try {
    details.value = await bridge.removeInstanceAsset(props.instance.name, asset.kind, asset.path)
    emit('toast', 'Файл удалён')
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

async function toggleMod(asset: InstanceAsset) {
  if (!props.instance || asset.kind !== 'mods') return
  try {
    details.value = await bridge.toggleMod(props.instance.name, asset.path, asset.enabled === false)
    emit('toast', asset.enabled === false ? 'Мод включён' : 'Мод отключён')
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

// правый клик по ресурсу: проводник, путь, вкл/выкл, проверка, удаление
function assetMenu(asset: InstanceAsset, event: MouseEvent) {
  const parent = asset.path.replace(/[\\/][^\\/]+$/, '')
  const items: ContextMenuItem[] = [
    {
      id: 'reveal',
      label: 'Открыть во встроенном проводнике',
      icon: 'folder',
      action: () => openInLauncherExplorer(parent || asset.path),
    },
    {
      id: 'copy-path',
      label: 'Копировать путь',
      icon: 'copy',
      action: () => void navigator.clipboard?.writeText(asset.path),
    },
  ]
  if (asset.kind === 'mods') {
    items.push({
      id: 'toggle',
      label: asset.enabled === false ? 'Включить мод' : 'Отключить мод',
      icon: 'check',
      action: () => void toggleMod(asset),
    })
    items.push({ id: 'audit', label: 'Проверить безопасность', icon: 'shield', action: () => auditMod(asset) })
  }
  if (asset.kind === 'shaders') {
    items.push({ id: 'analyze', label: 'Анализ шейдера', icon: 'activity', action: () => analyzeShader(asset) })
  }
  items.push({ id: 'delete', label: 'Удалить', icon: 'trash', danger: true, action: () => void removeAsset(asset) })
  openContextMenu(event, items, asset.display_name || asset.name)
}

// правый клик по миру: папка мира и запуск
function worldMenu(world: InstanceWorld, event: MouseEvent) {
  openContextMenu(event, [
    { id: 'reveal', label: 'Открыть папку мира', icon: 'folder', action: () => openInLauncherExplorer(world.path) },
    { id: 'copy-path', label: 'Копировать путь', icon: 'copy', action: () => void navigator.clipboard?.writeText(world.path) },
    { id: 'play', label: 'Играть в этот мир', icon: 'play', action: () => launchWorld(world.id) },
  ], world.name)
}

// ── Выделение модов и массовые действия ─────────────────────────────────
const selectedModPaths = ref(new Set<string>())
const modBulkBusy = ref(false)
const installedModPaths = computed(() => (tab.value === 'mods' ? activeAssets.value.map((asset) => asset.path) : []))
const selectedModCount = computed(() => installedModPaths.value.filter((path) => selectedModPaths.value.has(path)).length)
const allModsSelected = computed(() => installedModPaths.value.length > 0 && selectedModCount.value === installedModPaths.value.length)

function toggleModSelection(path: string) {
  const next = new Set(selectedModPaths.value)
  if (next.has(path)) next.delete(path)
  else next.add(path)
  selectedModPaths.value = next
}

function toggleSelectAllMods() {
  selectedModPaths.value = allModsSelected.value ? new Set() : new Set(installedModPaths.value)
}

function selectedMods(): InstanceAsset[] {
  return activeAssets.value.filter((asset) => asset.kind === 'mods' && selectedModPaths.value.has(asset.path))
}

async function removeSelectedMods() {
  const targets = selectedMods()
  if (!props.instance || !targets.length || modBulkBusy.value) return
  if (running.value) {
    emit('toast', 'Остановите игру перед удалением модов')
    return
  }
  const preview = targets.slice(0, 5).map((asset) => `«${asset.display_name || asset.name}»`).join(', ') + (targets.length > 5 ? ` и ещё ${targets.length - 5}` : '')
  if (props.confirmDestructive !== false && !(await confirmDialog(`Удалить выбранные моды (${targets.length}): ${preview}?`, { danger: true, confirmLabel: 'Удалить' }))) return
  modBulkBusy.value = true
  try {
    const result = await bridge.modsRemoveFiles(props.instance.name, targets.map((asset) => asset.path))
    details.value = result.details
    selectedModPaths.value = new Set()
    emit('changed')
    emit('toast', result.failed.length ? `Удалено модов: ${result.removed.length}, не удалось: ${result.failed.length} (${result.failed[0].error})` : `Удалено модов: ${result.removed.length}`)
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    modBulkBusy.value = false
  }
}

async function setSelectedModsEnabled(enabled: boolean) {
  const targets = selectedMods().filter((asset) => (asset.enabled !== false) !== enabled)
  if (!props.instance || !targets.length || modBulkBusy.value) return
  modBulkBusy.value = true
  let done = 0
  const renamed = new Set<string>()
  try {
    for (const asset of targets) {
      details.value = await bridge.toggleMod(props.instance.name, asset.path, enabled)
      // файл переименовывается (*.disabled) — выделение переносим на новое имя.
      renamed.add(enabled ? asset.path.replace(/\.disabled$/, '') : `${asset.path}.disabled`)
      done += 1
    }
    emit('toast', enabled ? `Включено модов: ${done}` : `Отключено модов: ${done}`)
  } catch (error) {
    emit('toast', `${errorMessage(error)} (обработано: ${done})`)
  } finally {
    selectedModPaths.value = new Set([...selectedModPaths.value].filter((path) => !targets.some((asset) => asset.path === path)).concat([...renamed]))
    modBulkBusy.value = false
  }
}

watch([tab, () => props.instance?.name], () => { selectedModPaths.value = new Set() })
watch(details, () => {
  // удалённые/переименованные извне файлы не должны оставаться «выбранными».
  const existing = new Set((details.value?.assets?.mods || []).map((asset) => asset.path))
  if ([...selectedModPaths.value].some((path) => !existing.has(path))) {
    selectedModPaths.value = new Set([...selectedModPaths.value].filter((path) => existing.has(path)))
  }
})

// ── Импорт миров (архив или папка) ───────────────────────────────────────
const worldImportBusy = ref(false)
const worldImportProgress = ref(0)
const worldImportLabel = ref('')
const WORLD_ARCHIVE_ACCEPT = '.zip,.tar,.gz,.tgz,.bz2,.tbz2,.xz,.txz'
const WORLD_CHUNK_BYTES = 4 * 1024 * 1024

function selectWorldArchive() {
  if (!worldImportBusy.value) void chooseWorldArchive()
}

async function selectWorldFolder() {
  if (worldImportBusy.value || !props.instance) return
  if (running.value) {
    emit('toast', 'Остановите игру перед импортом мира')
    return
  }
  const picked = await pickDirectory({ title: 'Папка мира (или saves/ с мирами)' })
  if (!picked) return
  worldImportBusy.value = true
  worldImportProgress.value = 0
  worldImportLabel.value = `Переношу папку «${picked.name}»…`
  try {
    const result = await bridge.worldImportPath(props.instance.name, picked.path)
    details.value = await bridge.instanceDetails(props.instance.name)
    emit('changed')
    emit('toast', result.imported.length === 1 ? `Мир «${result.imported[0]}» добавлен` : `Добавлено миров: ${result.imported.length} (${result.imported.join(', ')})`)
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    worldImportBusy.value = false
    worldImportProgress.value = 0
    worldImportLabel.value = ''
  }
}

async function uploadWorld(kind: 'archive' | 'folder', label: string, entries: Array<{ path: string; file: Blob & { size: number } }>) {
  if (!props.instance || !entries.length) return
  if (running.value) {
    emit('toast', 'Остановите игру перед импортом мира')
    return
  }
  const instanceName = props.instance.name
  const total = entries.reduce((sum, entry) => sum + entry.file.size, 0) || 1
  let sent = 0
  let uploadId = ''
  worldImportBusy.value = true
  worldImportProgress.value = 0
  worldImportLabel.value = kind === 'archive' ? `Загружаю архив «${label}»…` : `Загружаю папку «${label}»…`
  try {
    uploadId = (await bridge.worldImportBegin(instanceName, kind, label)).upload_id
    for (const entry of entries) {
      let offset = 0
      do {
        const piece = entry.file.slice(offset, offset + WORLD_CHUNK_BYTES)
        const bytes = new Uint8Array(await piece.arrayBuffer())
        await bridge.worldImportChunk(instanceName, uploadId, entry.path, offset, toBase64(bytes))
        offset += bytes.length
        sent += bytes.length
        worldImportProgress.value = Math.min(99, Math.round((sent / total) * 100))
      } while (offset < entry.file.size)
    }
    worldImportLabel.value = kind === 'archive' ? 'Распаковываю и ищу миры…' : 'Переношу мир в saves/…'
    const result = await bridge.worldImportFinish(instanceName, uploadId)
    uploadId = ''
    if (props.instance?.name === instanceName) details.value = result.details
    emit('changed')
    emit('toast', result.imported.length === 1 ? `Мир «${result.imported[0]}» добавлен` : `Добавлено миров: ${result.imported.length} (${result.imported.join(', ')})`)
  } catch (error) {
    emit('toast', errorMessage(error))
    if (uploadId) void bridge.worldImportCancel(instanceName, uploadId).catch(() => undefined)
  } finally {
    worldImportBusy.value = false
    worldImportProgress.value = 0
    worldImportLabel.value = ''
  }
}

async function chooseWorldArchive() {
  if (worldImportBusy.value) return
  const picked = await pickFile({ accept: '.zip,application/zip', title: 'Архив мира (.zip)' })
  if (!picked) return
  await uploadWorld('archive', picked.fileName, [{ path: picked.fileName, file: pickedFileBlob(picked) }])
}

/** picked-файл → Blob для пути загрузки мира кусками. */
function pickedFileBlob(file: { fileName: string; data: string; size: number }): Blob {
  const bytes = Uint8Array.from(atob(file.data), (character) => character.charCodeAt(0))
  return new Blob([bytes], { type: 'application/octet-stream' })
}

const modAuditDialog = ref<InstanceType<typeof ModAuditDialog> | null>(null)

function analyzeShader(asset: InstanceAsset) {
  shaderAnalysisTarget.value = { name: asset.name, size: asset.size }
}

function auditMod(asset: InstanceAsset) {
  if (!props.instance || asset.kind !== 'mods') return
  modAuditDialog.value?.start({ instanceName: props.instance.name, path: asset.path, fileName: asset.display_name || asset.name })
}

async function checkResourceUpdates() {
  if (!props.instance || !activeResourceProjectType.value || checkingUpdates.value) return
  checkingUpdates.value = true
  updateResults.value = []
  try {
    const result = await bridge.modsCheckUpdates(props.instance.name, activeResourceProjectType.value)
    updateResults.value = result.updates
    if (result.errors?.length) {
      const firstError = result.errors[0].length > 180 ? `${result.errors[0].slice(0, 177)}…` : result.errors[0]
      const updates = result.updates.length ? `Доступны обновления: ${result.updates.length}. ` : ''
      const remainingErrors = result.errors.length > 1 ? ` (+${result.errors.length - 1})` : ''
      emit('toast', `${updates}Не удалось проверить каталог: ${firstError}${remainingErrors}`)
    } else {
      emit('toast', result.updates.length ? `Доступны обновления: ${result.updates.length}` : 'Обновлений у ресурсов с метаданными каталога нет')
    }
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    checkingUpdates.value = false
  }
}

async function updateResource(update: ResourceUpdate) {
  if (!props.instance || running.value || updatingResourceId.value) return
  const key = update.path || update.project_id
  updatingResourceId.value = key
  try {
    const install = (confirmUnverified: boolean) => bridge.modsUpdate({
      project_id: update.project_id,
      version_id: update.version_id,
      instance_name: props.instance!.name,
      project_type: update.project_type,
      world_name: update.world_name,
      catalog_id: update.catalog_id,
      confirm_unverified: confirmUnverified,
    })
    let result: Awaited<ReturnType<typeof install>>
    try {
      result = await install(false)
    } catch (error) {
      if (!errorMessage(error).includes('[unverified-download]') || !(await confirmDialog('Источник обновления не предоставил доверенный хеш. Скачать файл и показать его SHA-256 после загрузки?', { danger: true, confirmLabel: 'Обновить' }))) throw error
      result = await install(true)
    }
    details.value = result.details
    updateResults.value = updateResults.value.filter((item) => item.path !== update.path)
    emit('changed')
    emit('toast', `Обновлено: ${result.name}`)
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    updatingResourceId.value = ''
  }
}

function selectFiles() {
  void chooseAssetFiles()
}

function toBase64(bytes: Uint8Array): string {
  let binary = ''
  const chunkSize = 0x8000
  for (let index = 0; index < bytes.length; index += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(index, index + chunkSize))
  }
  return btoa(binary)
}

async function chooseAssetFiles() {
  if (!props.instance) return
  const picked = await pickFiles({ title: 'Файлы для импорта' })
  if (!picked?.length || !['mods', 'resourcepacks', 'shaders'].includes(tab.value)) return
  importing.value = true
  try {
    for (const file of picked) {
      if (file.size > 256 * 1024 * 1024) {
        throw new Error(`Файл «${file.fileName}» превышает лимит 256 MiB`)
      }
      details.value = await bridge.importAsset(props.instance.name, tab.value, file.fileName, file.data)
    }
    emit('toast', `Импортировано файлов: ${picked.length}`)
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    importing.value = false
  }
}

async function loadWorkspaceFiles() {
  if (!props.instance) return
  const requestId = ++fileListRequestId
  const instanceName = props.instance.name
  const directory = fileDirectory.value
  fileLoading.value = true
  try {
    const result = await bridge.instanceFileList(instanceName, directory)
    if (requestId !== fileListRequestId || props.instance?.name !== instanceName || fileDirectory.value !== directory) return
    instanceFiles.value = result.files
  } catch (error) {
    if (requestId !== fileListRequestId || props.instance?.name !== instanceName || fileDirectory.value !== directory) return
    instanceFiles.value = []
    emit('toast', errorMessage(error))
  } finally {
    if (requestId === fileListRequestId) fileLoading.value = false
  }
}

function enterWorkspaceDirectory(file: InstanceFile) {
  if (file.kind !== 'directory') return
  fileDirectory.value = file.path
  void loadWorkspaceFiles()
}

function goToWorkspaceDirectory(directory: string) {
  fileDirectory.value = directory
  void loadWorkspaceFiles()
}

function goToParentDirectory() {
  const parts = fileDirectory.value.split('/').filter(Boolean)
  parts.pop()
  goToWorkspaceDirectory(parts.join('/'))
}

function isImageFile(file: InstanceFile): boolean {
  return /\.(png|jpe?g|webp|gif)$/i.test(file.name)
}

async function previewWorkspaceImage(file: InstanceFile) {
  if (!props.instance || !isImageFile(file)) throw new Error('Предпросмотр доступен только для изображений')
  const result = await bridge.instanceFileRead(props.instance.name, file.path, 'base64')
  if (!result.mime_type) throw new Error('Формат изображения не поддерживается')
  filePreview.value = { path: file.path, source: `data:${result.mime_type};base64,${result.content}` }
}

async function openWorkspaceFile(file: InstanceFile) {
  if (!props.instance || editorLoading.value || editorSaving.value) return
  if (file.kind === 'directory') {
    enterWorkspaceDirectory(file)
    return
  }
  if (!file.editable && !isImageFile(file)) {
    emit('toast', 'Редактор поддерживает текстовые файлы до 2 MiB. Изображения можно открыть в предпросмотре.')
    return
  }
  if (editorPath.value === file.path) return
  if (editorPath.value && !(await confirmDiscardWorkspaceFile())) return
  clearWorkspaceEditor()
  filePreview.value = undefined
  editorLoading.value = true
  try {
    if (!file.editable) {
      await previewWorkspaceImage(file)
      return
    }
    try {
      const result = await bridge.instanceFileRead(props.instance.name, file.path, 'text')
      editorPath.value = file.path
      editorContent.value = result.content
      editorOriginalContent.value = result.content
      filePreview.value = undefined
    } catch (error) {
      if (!isImageFile(file)) throw error
      await previewWorkspaceImage(file)
    }
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    editorLoading.value = false
  }
}

async function saveWorkspaceFile() {
  if (!props.instance || !editorPath.value || running.value || editorSaving.value) return
  editorSaving.value = true
  try {
    await bridge.instanceFileWrite(props.instance.name, editorPath.value, editorContent.value)
    editorOriginalContent.value = editorContent.value
    emit('toast', 'Файл сохранён')
    await loadWorkspaceFiles()
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    editorSaving.value = false
  }
}

async function confirmDiscardWorkspaceFile(): Promise<boolean> {
  if (editorContent.value === editorOriginalContent.value) return true
  return confirmDialog('Закрыть файл и потерять несохранённые изменения?', { danger: true, confirmLabel: 'Закрыть без сохранения' })
}

function clearWorkspaceEditor() {
  editorPath.value = ''
  editorContent.value = ''
  editorOriginalContent.value = ''
}

async function closeWorkspaceFile() {
  if (editorSaving.value || !(await confirmDiscardWorkspaceFile())) return
  clearWorkspaceEditor()
}

async function selectWorkspaceUpload() {
  if (running.value || fileMutationBusy.value) {
    if (running.value) emit('toast', 'Остановите игру перед изменением файлов экземпляра')
    return
  }
  const picked = await pickFiles({ title: 'Загрузка файлов в экземпляр' })
  if (!picked?.length) return
  await uploadWorkspaceFiles(picked)
}

async function uploadWorkspaceFiles(files: Array<{ fileName: string; data: string; size: number }>) {
  if (!props.instance || running.value || fileMutationBusy.value) {
    if (running.value) emit('toast', 'Остановите игру перед изменением файлов экземпляра')
    return
  }
  if (!files.length) return
  fileMutationBusy.value = true
  try {
    for (const file of files) {
      if (!file.fileName || file.fileName === '.' || file.fileName === '..' || file.fileName.includes('/') || file.fileName.includes('\\')) {
        throw new Error(`Недопустимое имя файла: ${file.fileName || 'без имени'}`)
      }
      if (file.size > 32 * 1024 * 1024) throw new Error(`Файл «${file.fileName}» превышает лимит 32 MiB`)
      const target = [fileDirectory.value, file.fileName].filter(Boolean).join('/')
      await bridge.instanceFileUpload(props.instance.name, target, file.data)
    }
    emit('toast', `Загружено файлов: ${files.length}`)
    await loadWorkspaceFiles()
    if (fileDirectory.value === 'screenshots') await loadScreenshots()
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    fileMutationBusy.value = false
  }
}

function onWorkspaceDragEnter(event: DragEvent) {
  if (!Array.from(event.dataTransfer?.types || []).includes('Files')) return
  fileDragDepth.value += 1
  fileDropActive.value = true
}

function onWorkspaceDragLeave(event: DragEvent) {
  if (!Array.from(event.dataTransfer?.types || []).includes('Files')) return
  fileDragDepth.value = Math.max(0, fileDragDepth.value - 1)
  fileDropActive.value = fileDragDepth.value > 0
}

function onWorkspaceDrop(event: DragEvent) {
  fileDragDepth.value = 0
  fileDropActive.value = false
  const files = Array.from(event.dataTransfer?.files || [])
  if (!files.length) return
  void (async () => {
    const picked = await Promise.all(files.map(async (file) => ({
      fileName: file.name,
      data: toBase64(new Uint8Array(await file.arrayBuffer())),
      size: file.size,
    })))
    await uploadWorkspaceFiles(picked)
  })()
}

async function createWorkspaceDirectory() {
  if (!props.instance || running.value || fileMutationBusy.value) return
  const name = (await promptDialog('Название новой папки', { placeholder: 'Например, config', confirmLabel: 'Создать' }))?.trim()
  if (!name) return
  const path = [fileDirectory.value, name].filter(Boolean).join('/')
  fileMutationBusy.value = true
  try {
    await bridge.instanceFileMkdir(props.instance.name, path)
    await loadWorkspaceFiles()
    emit('toast', 'Папка создана')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    fileMutationBusy.value = false
  }
}

async function deleteWorkspaceEntry(file: InstanceFile) {
  if (!props.instance || running.value || fileMutationBusy.value || editorSaving.value) return
  const containsPath = (path: string) => path === file.path || (file.kind === 'directory' && path.startsWith(`${file.path}/`))
  const removesEditor = Boolean(editorPath.value && containsPath(editorPath.value))
  const removesPreview = Boolean(filePreview.value && containsPath(filePreview.value.path))
  const losesUnsavedChanges = removesEditor && editorContent.value !== editorOriginalContent.value
  const needsConfirmation = props.confirmDestructive !== false || losesUnsavedChanges
  const warning = losesUnsavedChanges ? ' Открытый файл содержит несохранённые изменения, они будут потеряны.' : ''
  if (needsConfirmation && !(await confirmDialog(`Удалить «${file.name}»${file.kind === 'directory' ? ' со всем содержимым' : ''}?${warning}`, { danger: true, confirmLabel: 'Удалить' }))) return
  fileMutationBusy.value = true
  try {
    await bridge.instanceFileDelete(props.instance.name, file.path)
    if (removesEditor) clearWorkspaceEditor()
    if (removesPreview) filePreview.value = undefined
    await loadWorkspaceFiles()
    emit('toast', 'Элемент удалён')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    fileMutationBusy.value = false
  }
}

async function loadScreenshotPage(requestId: number, instanceName: string) {
  const start = screenshotPage.value * SCREENSHOT_PAGE_SIZE
  const pageFiles = screenshotFiles.value.slice(start, start + SCREENSHOT_PAGE_SIZE)
  const previews = await Promise.all(pageFiles.map(async (file) => {
    try {
      const result = await bridge.instanceFileRead(instanceName, file.path, 'base64')
      return result.mime_type ? { file, source: `data:${result.mime_type};base64,${result.content}` } : { file, source: '' }
    } catch {
      return { file, source: '' }
    }
  }))
  if (requestId !== screenshotRequestId || props.instance?.name !== instanceName) return
  screenshotPreviews.value = previews
}

async function loadScreenshots() {
  if (!props.instance) return
  const requestId = ++screenshotRequestId
  const instanceName = props.instance.name
  screenshotLoading.value = true
  try {
    const result = await bridge.instanceFileList(instanceName, 'screenshots')
    if (requestId !== screenshotRequestId || props.instance?.name !== instanceName) return
    screenshotFiles.value = result.files
      .filter((file) => file.kind === 'file' && isImageFile(file) && file.size <= 8 * 1024 * 1024)
      .sort((left, right) => right.modified_at - left.modified_at)
    screenshotPage.value = Math.min(screenshotPage.value, Math.max(0, screenshotPageCount.value - 1))
    await loadScreenshotPage(requestId, instanceName)
  } catch (error) {
    if (requestId !== screenshotRequestId || props.instance?.name !== instanceName) return
    screenshotFiles.value = []
    screenshotPreviews.value = []
    if (!errorMessage(error).includes('папку экземпляра')) emit('toast', errorMessage(error))
  } finally {
    if (requestId === screenshotRequestId) screenshotLoading.value = false
  }
}

async function changeScreenshotPage(direction: -1 | 1) {
  if (screenshotLoading.value || !props.instance) return
  const nextPage = screenshotPage.value + direction
  if (nextPage < 0 || nextPage >= screenshotPageCount.value) return
  const requestId = ++screenshotRequestId
  const instanceName = props.instance.name
  screenshotPage.value = nextPage
  screenshotLoading.value = true
  try {
    await loadScreenshotPage(requestId, instanceName)
  } finally {
    if (requestId === screenshotRequestId) screenshotLoading.value = false
  }
}

async function launchWorld(world: string) {
  if (!props.instance) return
  await launch({ world })
}

function pluginInstanceRef(source: InstanceInfo): PluginInstanceRef {
  return {
    id: source.name,
    name: source.display_name || source.name,
    version: source.minecraft_version || source.version_id || undefined,
    loader: source.loader,
  }
}

async function launchServer(server: string) {
  if (!props.instance) return
  await launch({ server })
}

/** Оффлайн-аккаунт: мир — можно, серверы — предупреждаем; Bedrock — только Microsoft. */
async function checkAccountForLaunch(options: { world?: string; server?: string }): Promise<boolean> {
  const target = props.instance
  if (!target) return true
  const offline = props.accountInfo?.kind === 'offline'
  if (target.game_edition === 'bedrock' && offline) {
    await chooseDialog(
      'Bedrock не запустится с оффлайн-аккаунтом',
      [{ value: 'accounts', label: 'Открыть аккаунты', description: 'Подключить Microsoft-аккаунт во вкладке «Аккаунты»' }],
      {
        title: 'Нужен аккаунт Microsoft',
        detail: 'Лицензия Bedrock привязана к аккаунту Microsoft: одиночные миры без входа недоступны, в отличие от Java Edition. Подключите аккаунт, которому принадлежит игра, и запустите снова.',
        confirmLabel: 'Понятно',
      },
    )
    emit('toast', 'Bedrock требует аккаунт Microsoft: одиночная игра без входа недоступна')
    return false
  }
  if (offline && options.server && target.game_edition !== 'bedrock') {
    return confirmDialog(
      'Серверы с онлайн-проверкой сессий не пустят оффлайн-аккаунт. Войти на сервер всё равно?',
      { confirmLabel: 'Подключиться' },
    )
  }
  return true
}

async function launch(options: { world?: string; server?: string } = {}) {
  const launchTarget = props.instance
  if (!launchTarget || loading.value || running.value) return
  if (!(await checkAccountForLaunch(options))) return
  const eventInstance = pluginInstanceRef(launchTarget)
  loading.value = true
  running.value = true
  liveLogRevision += 1
  logs.value = []
  liveLogCount = 0
  let launchRequested = false
  try {
    await props.beforeLaunch?.(eventInstance)
    launchRequested = true
    const result = await bridge.launchInstance(launchTarget.name, { account: props.account || 'auto', ...options })
    await props.afterExit?.(eventInstance, result.exit_code)
    if (result.exit_code !== 0) {
      await props.crash?.(eventInstance, 'Minecraft exited with a non-zero status.', result.exit_code)
      if (result.crash) lastCrash.value = result.crash
      offerAutoRepair(result.repair)
    }
    if (!liveLogCount) {
      liveLogRevision += 1
      logs.value = (result.logs || []).slice(-MAX_SESSION_LOG_LINES)
    }
    emit('toast', result.exit_code === 0 ? 'Игра завершила работу' : `Игра завершилась с кодом ${result.exit_code}`)
  } catch (error) {
    liveLogRevision += 1
    logs.value.push(errorMessage(error))
    if (logs.value.length > MAX_SESSION_LOG_LINES) logs.value.splice(0, logs.value.length - MAX_SESSION_LOG_LINES)
    if (launchRequested && !isCancelled(error)) {
      await props.crash?.(eventInstance, errorMessage(error))
      void diagnoseLaunchFailure(launchTarget.name, errorMessage(error))
    }
    emit('toast', errorMessage(error))
  } finally {
    loading.value = false
    running.value = false
    await refreshWorkspaceSnapshot()
    emit('changed')
  }
}

async function createSnapshot() {
  const snapshotTarget = props.instance
  if (!snapshotTarget || snapshotBusy.value || running.value) return
  const eventInstance = pluginInstanceRef(snapshotTarget)
  snapshotBusy.value = true
  try {
    const result = await bridge.instanceSnapshotCreate(snapshotTarget.name, snapshotCategory.value)
    details.value = result.details
    await props.snapshotCreated?.(eventInstance, result.snapshot)
    emit('toast', 'Снапшот создан')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    snapshotBusy.value = false
  }
}

async function restoreSnapshot(snapshot: ServerSnapshot) {
  if (!props.instance || running.value || snapshotBusy.value) return
  if (props.confirmDestructive !== false && !(await confirmDialog('Заменить текущие файлы этим снапшотом?', { danger: true, confirmLabel: 'Восстановить' }))) return
  snapshotBusy.value = true
  try {
    const result = await bridge.instanceSnapshotRestore(props.instance.name, snapshotCategory.value, snapshot.id)
    details.value = result.details
    emit('toast', 'Снапшот восстановлен')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    snapshotBusy.value = false
  }
}

async function deleteSnapshot(snapshot: ServerSnapshot) {
  if (!props.instance || snapshotBusy.value) return
  if (props.confirmDestructive !== false && !(await confirmDialog('Удалить снапшот?', { danger: true, confirmLabel: 'Удалить' }))) return
  snapshotBusy.value = true
  try {
    const result = await bridge.instanceSnapshotDelete(props.instance.name, snapshotCategory.value, snapshot.id)
    details.value = result.details
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    snapshotBusy.value = false
  }
}

async function stop() {
  if (!props.instance || (!running.value && !props.instance.running)) return
  try {
    await bridge.cancelActiveOperations()
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

function openFolder() {
  if (!details.value) return
  openInLauncherExplorer(details.value.instance.path)
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  if (typeof error === 'string') return error
  return 'Неизвестная ошибка'
}

function isCancelled(error: unknown): boolean {
  return errorMessage(error).toLowerCase().includes('операция остановлена')
}

watch(() => props.instance?.name, () => {
  details.value = undefined
  detailsError.value = ''
  shortcutStatus.value = null
  void refreshShortcutStatus()
  loadRequestId += 1
  loadPromise = null
  loadPromiseName = ''
  fileListRequestId += 1
  screenshotRequestId += 1
  liveLogRevision += 1
  liveLogCount = 0
  selectTab(props.settingsOnly ? 'settings' : 'overview')
  updateResults.value = []
  logs.value = []
  logArchives.value = []
  nextArchiveOffset.value = null
  archiveListOpen.value = false
  activeArchiveId.value = ''
  archiveText.value = ''
  archiveNextOffset.value = 0
  archiveEof.value = true
  running.value = Boolean(props.instance?.running)
  metrics.value = undefined
  metricHistory.value = []
  consoleStickToBottom.value = true
  fileDirectory.value = ''
  fileLoading.value = false
  instanceFiles.value = []
  clearWorkspaceEditor()
  filePreview.value = undefined
  screenshotFiles.value = []
  screenshotPreviews.value = []
  screenshotPage.value = 0
  screenshotLoading.value = false
  void queueLoad(true)
  void loadProfileGroups()
  restartLiveLogPolling()
  restartWorkspacePolling()
}, { immediate: true })

watch(tab, (value) => {
  updateResults.value = []
  if (value === 'backups') void loadBackups()
})

watch(() => props.instance, (value, previous) => {
  if (!value) {
    details.value = undefined
    return
  }
  if (!previous || value.name !== previous.name) return
  if (details.value) details.value.instance = { ...details.value.instance, ...value }
  syncProfileDraft(value)
  if (value.running !== undefined && !loading.value) running.value = Boolean(value.running)
}, { deep: true })

watch(draft, () => {
  if (!syncingSettings.value && settingsSaveState.value !== 'saving') {
    settingsSaveState.value = 'dirty'
    settingsSaveMessage.value = 'Есть несохранённые изменения'
  }
}, { deep: true })

watch(() => props.focusTab, (value) => {
  if (!value) return
  selectTab(value)
  if (value === 'settings') void nextTick(() => workspaceRoot.value?.scrollIntoView({ behavior: 'smooth', block: 'start' }))
})

watch(() => props.settingsOnly, (value) => {
  if (value) selectTab('settings')
})

function handleWorkspaceVisibility() {
  if (document.hidden) return
  void refreshWorkspaceSnapshot()
  void pollLiveLog()
}

onMounted(async () => {
  document.addEventListener('visibilitychange', handleWorkspaceVisibility)
  void refreshShortcutStatus()
  stopBackendEvents = await bridge.subscribeEvents((event) => {
    if (event.instance_name !== props.instance?.name) return
    if (event.kind === 'status' && (event.event === 'workspace-changed' || event.event === 'operation-finished')) {
      void refreshWorkspaceSnapshot()
    }
    if (event.kind === 'status' && event.event === 'operation-finished') {
      running.value = false
      loading.value = false
      void refreshWorkspaceSnapshot()
      emit('changed')
      return
    }
    if (event.kind !== 'log' || !event.line) return
    liveLogRevision += 1
    logs.value.push(event.line)
    liveLogCount += 1
    if (logs.value.length > MAX_SESSION_LOG_LINES) logs.value.splice(0, logs.value.length - MAX_SESSION_LOG_LINES)
    running.value = true
    void scrollConsoleIfNeeded()
  })
})

watch(logs, () => { void scrollConsoleIfNeeded() }, { deep: true })

onBeforeUnmount(() => {
  fileListRequestId += 1
  screenshotRequestId += 1
  stopBackendEvents?.()
  document.removeEventListener('visibilitychange', handleWorkspaceVisibility)
  if (liveLogTimer) window.clearInterval(liveLogTimer)
  if (workspaceRefreshTimer) window.clearInterval(workspaceRefreshTimer)
  if (profileIconPreview.value.startsWith('blob:')) URL.revokeObjectURL(profileIconPreview.value)
  if (profileBannerPreview.value.startsWith('blob:')) URL.revokeObjectURL(profileBannerPreview.value)
})

function openConsoleIfOverview(): boolean {
  if (tab.value !== 'overview') return false
  selectTab('console')
  return true
}

function openSettings() {
  selectTab('settings')
  void nextTick(() => workspaceRoot.value?.scrollIntoView({ behavior: 'smooth', block: 'start' }))
}

defineExpose({ openConsoleIfOverview, openSettings, stop, ensureLoaded, launch })
</script>

<template>
  <section v-if="instance" ref="workspaceRoot" class="workspace-shell">
    <template v-if="!settingsOnly">
    <div class="workspace-header">
      <div>
        <span class="eyebrow">INSTANCE WORKSPACE</span>
         <h2>{{ instance.display_name || instance.name }}</h2>
         <p><LoaderBadge :loader="instance.loader" size="sm" /> {{ instance.minecraft_version || instance.version_id || 'Версия не установлена' }} · {{ instance.name }} · {{ details?.instance.path }}</p>
      </div>
      <div class="workspace-header-actions">
        <div class="workspace-log-history">
          <button class="button button--quiet" type="button" :aria-expanded="archiveListOpen" @click="toggleLogHistory"><Icon name="archive" :size="15" />История логов<small v-if="logArchives.length">{{ logArchives.length }}</small></button>
          <div v-if="archiveListOpen" class="workspace-log-popover">
            <div class="workspace-log-popover-heading"><strong>Журналы запуска</strong><span>{{ logArchives.length }}</span></div>
            <button class="workspace-log-row workspace-log-row--live" type="button" @click="selectTab('console')"><Icon name="activity" :size="15" /><span><strong>Консоль в реальном времени</strong><small>Текущий поток вывода</small></span></button>
            <button v-for="log in logArchives" :key="log.id" class="workspace-log-row" type="button" @click="openLogArchive(log)"><Icon name="file" :size="15" /><span><strong>{{ formatDate(Math.round(log.modified_at)) }}</strong><small>{{ formatSize(log.size) }}</small></span></button>
            <button v-if="archiveListLoading" class="workspace-log-more" type="button" disabled>Загружаю журналы…</button>
            <button v-else-if="nextArchiveOffset !== null" class="workspace-log-more" type="button" @click="loadLogArchives(true)">Показать ещё</button>
            <span v-else-if="!logArchives.length" class="workspace-log-empty">Предыдущих запусков пока нет.</span>
          </div>
        </div>
        <button class="button button--quiet" type="button" @click="openFolder"><Icon name="folder" :size="15" />Папка</button>
        <button v-if="canExportModpack" class="button button--quiet" type="button" :disabled="exportBusy" @click="exportModpack"><Icon name="archive" :size="15" />{{ exportBusy ? 'Экспорт…' : 'Экспорт .mrpack' }}</button>
        <button class="button button--quiet" type="button" :disabled="duplicateBusy" @click="duplicateCurrent"><Icon name="instances" :size="15" />{{ duplicateBusy ? 'Копирую…' : 'Дублировать' }}</button>
      </div>
    </div>

    <nav class="workspace-tabs workspace-tabs--flat" role="tablist" aria-label="Разделы экземпляра">
      <button v-for="item in visibleTabs" :key="item.id" class="workspace-tab" :class="{ 'workspace-tab--active': tab === item.id }" type="button" role="tab" :aria-selected="tab === item.id" @click="selectTab(item.id)">
        <Icon :name="item.icon" :size="14" />{{ item.label }}
        <small v-if="item.id === 'mods' && details">{{ details.assets.mods?.length || 0 }}</small>
        <small v-if="item.id === 'datapacks' && details">{{ details.worlds.reduce((total, world) => total + (world.datapacks?.length || 0), 0) }}</small>
        <small v-if="item.id === 'worlds' && details">{{ details.worlds.length }}</small>
        <small v-if="item.id === 'servers' && details">{{ details.servers.length }}</small>
        <small v-if="item.id === 'snapshots' && details">{{ snapshotCount }}</small>
      </button>
    </nav>
    </template>

    <div v-if="loading && !details" class="workspace-loading" role="status">Читаю содержимое экземпляра…</div>
    <div v-else-if="!details && detailsError" class="workspace-load-error" role="alert"><div><strong>Не удалось загрузить экземпляр</strong><span>{{ detailsError }}</span></div><button class="button button--small button--outline" type="button" :disabled="loading" @click="ensureLoaded">{{ loading ? 'Повторяю…' : 'Повторить' }}</button></div>

    <div v-else-if="details" class="workspace-content">
      <div v-if="detailsError" class="workspace-load-error workspace-load-error--inline" role="alert"><div><strong>Не удалось обновить данные</strong><span>{{ detailsError }}</span></div><button class="button button--small button--outline" type="button" :disabled="loading" @click="queueLoad(true)">{{ loading ? 'Повторяю…' : 'Повторить' }}</button></div>
      <Transition name="workspace-tab" mode="out-in">
      <div v-if="tab === 'overview'" key="overview" class="workspace-overview">
        <section class="workspace-launch-banner">
          <div class="workspace-launch-copy">
            <span class="workspace-instance-status" :class="{ 'workspace-instance-status--running': running }"><i />{{ running ? 'Игра запущена' : 'Готов к запуску' }}</span>
            <h3>{{ instance.display_name || instance.name }}</h3>
            <p>{{ instance.minecraft_version || instance.version_id || 'Версия не установлена' }} · {{ instance.loader }} · {{ formatPlayedTime(details.instance.played_seconds) }} в игре</p>
          </div>
          <div class="workspace-launch-actions">
            <button v-if="running" class="button button--outline" type="button" @click="stop"><Icon name="stop" :size="14" />Остановить</button>
            <button v-else class="button button--accent" type="button" :disabled="loading" @click="launch()"><Icon name="play" :size="15" />{{ loading ? 'Запускаю…' : 'Запустить игру' }}</button>
            <button class="button button--quiet" type="button" :disabled="repairBusy || running" @click="repairVersion"><Icon name="refresh" :size="14" />{{ repairBusy ? 'Восстанавливаю…' : 'Восстановить версию' }}</button>
            <button class="button button--quiet" type="button" :disabled="diagnoseBusy || running" @click="diagnoseInstance()"><Icon name="shield" :size="14" />{{ diagnoseBusy ? 'Проверяю…' : 'Проверить и починить' }}</button>
            <button class="button button--quiet" type="button" @click="selectTab('snapshots')"><Icon name="archive" :size="14" />Снапшоты</button>
          </div>
        </section>
        <section class="workspace-card workspace-console workspace-overview-console">
          <div class="workspace-card-heading">
            <div><h3>Консоль в реальном времени</h3><span class="muted">{{ running ? 'Игра запущена · вывод обновляется автоматически' : 'Последний вывод запуска' }}</span></div>
            <button class="button button--small button--quiet" type="button" @click="selectTab('console')"><Icon name="activity" :size="14" />Все логи</button>
          </div>
          <div class="console-viewport">
            <pre v-if="logs.length" ref="consoleRef" @scroll="onConsoleScroll"><span v-for="(line, index) in logs" :key="`${index}-${line}`" :style="{ color: consoleLineColor(line) }">{{ line }}</span></pre>
            <span v-else class="muted workspace-empty">Логи появятся здесь после запуска игры.</span>
            <button v-if="logs.length && !consoleStickToBottom" class="console-jump-button" type="button" @click="consoleStickToBottom = true; void scrollConsoleIfNeeded()"><Icon name="arrow-down" :size="14" />К последней строке</button>
          </div>
        </section>
        <div class="workspace-stat-grid">
          <div class="workspace-stat"><strong>{{ details.assets.mods?.length || 0 }}</strong><span>модов</span></div>
          <div class="workspace-stat"><strong>{{ details.worlds.length }}</strong><span>миров</span></div>
          <div class="workspace-stat"><strong>{{ details.servers.length }}</strong><span>серверов</span></div>
          <div class="workspace-stat"><strong>{{ snapshotCount }}</strong><span>снапшотов</span></div>
        </div>
        <div class="workspace-metric-grid">
          <article class="workspace-metric workspace-metric--cpu"><div><span><Icon name="activity" :size="14" />CPU</span><strong>{{ (metrics?.cpu_percent || 0).toFixed(1) }}%</strong></div><svg viewBox="0 0 100 42" preserveAspectRatio="none" aria-label="График CPU"><polyline :points="metricPoints('cpu_percent')" /></svg><small>{{ running ? 'Текущая загрузка процесса' : 'Последний доступный замер' }}</small></article>
          <article class="workspace-metric workspace-metric--ram"><div><span><Icon name="memory" :size="14" />RAM</span><strong>{{ formatMetricBytes(metrics?.ram_bytes || 0) }}</strong></div><svg viewBox="0 0 100 42" preserveAspectRatio="none" aria-label="График RAM"><polyline :points="metricPoints('ram_bytes')" /></svg><small>Память, занятая игрой</small></article>
          <article class="workspace-metric workspace-metric--storage"><div><span><Icon name="hard-drive" :size="14" />Диск</span><strong>{{ formatMetricBytes(metrics?.storage_bytes || 0) }}</strong></div><svg viewBox="0 0 100 42" preserveAspectRatio="none" aria-label="График размера файлов"><polyline :points="metricPoints('storage_bytes')" /></svg><small>Размер файлов экземпляра</small></article>
          <button class="workspace-metric-toggle" type="button" aria-label="Полная статистика использования" title="Полная статистика: GPU, моды, прошлые запуски" @click="usageDialogOpen = true"><Icon name="mods" :size="17" /></button>
        </div>
        <div class="workspace-columns">
          <section class="workspace-card">
            <div class="workspace-card-heading"><div><h3>Быстро подключиться</h3><span class="muted">Недавние миры</span></div><button class="text-button" type="button" @click="selectTab('worlds')">Все миры</button></div>
            <button v-for="world in details.worlds.slice(0, 4)" :key="world.id" class="workspace-list-row" type="button" :disabled="loading || running" @click="launchWorld(world.id)">
              <img v-if="world.icon" :src="world.icon" alt="" class="world-icon" />
              <span v-else class="world-icon world-icon--empty"><Icon name="instances" :size="16" /></span>
              <span><strong>{{ world.name }}</strong><small>{{ formatSize(world.size) }} · {{ formatDate(world.last_played) }}</small></span>
              <Icon name="play" :size="15" />
            </button>
            <div v-if="!details.worlds.length" class="workspace-connect-empty"><span class="world-icon world-icon--empty"><Icon name="instances" :size="16" /></span><span><strong>Миров пока нет</strong><small>Ваши сохранения появятся здесь.</small></span><button class="text-button" type="button" @click="selectTab('worlds')">Открыть</button></div>
          </section>
          <section class="workspace-card">
            <div class="workspace-card-heading"><div><h3>Быстро подключиться</h3><span class="muted">Недавние серверы</span></div><button class="text-button" type="button" @click="selectTab('servers')">Все серверы</button></div>
            <button v-for="server in details.servers.slice(0, 4)" :key="server.id" class="workspace-list-row" type="button" :disabled="loading || running" @click="launchServer(server.address)">
              <span class="world-icon world-icon--server"><Icon name="globe" :size="16" /></span>
              <span><strong>{{ server.name }}</strong><small>{{ server.address }}</small></span>
              <Icon name="play" :size="15" />
            </button>
            <div v-if="!details.servers.length" class="workspace-connect-empty"><span class="world-icon world-icon--server"><Icon name="globe" :size="16" /></span><span><strong>Серверов пока нет</strong><small>Сохранённые адреса появятся здесь.</small></span><button class="text-button" type="button" @click="selectTab('servers')">Открыть</button></div>
          </section>
        </div>
        <section class="workspace-quick-access" aria-label="Инструменты экземпляра">
          <span>Инструменты</span>
          <button type="button" @click="selectTab('mods')"><Icon name="mods" :size="14" />Моды</button>
          <button type="button" @click="selectTab('resourcepacks')"><Icon name="archive" :size="14" />Ресурспаки</button>
          <button type="button" @click="selectTab('shaders')"><Icon name="image" :size="14" />Шейдеры</button>
          <button type="button" @click="selectTab('files')"><Icon name="folder" :size="14" />Файлы</button>
          <button type="button" @click="selectTab('screenshots')"><Icon name="image" :size="14" />Скриншоты</button>
        </section>
      </div>

      <div v-else-if="tab === 'console'" key="console" class="workspace-card workspace-console">
        <div class="workspace-card-heading">
          <div><h3>{{ selectedArchive ? 'Архив запуска' : 'Консоль в реальном времени' }}</h3><span class="muted">{{ selectedArchive ? `${formatDate(Math.round(selectedArchive.modified_at))} · ${formatSize(selectedArchive.size)}` : running ? 'Игра запущена · вывод обновляется автоматически' : 'Вывод последнего запуска' }}</span></div>
          <div class="workspace-card-actions">
            <button v-if="selectedArchive" class="button button--small button--quiet" type="button" @click="selectTab('console')"><Icon name="activity" :size="14" />К live-консоли</button>
            <button v-if="running && !selectedArchive" class="button button--small button--outline" type="button" @click="stop"><Icon name="stop" :size="14" />Остановить</button>
            <button v-else-if="!selectedArchive" class="button button--small button--accent" type="button" :disabled="loading" @click="launch()"><Icon name="play" :size="14" />Запустить</button>
          </div>
        </div>
        <div class="console-viewport">
          <pre v-if="consoleDisplayLines.length" ref="consoleRef" @scroll="onConsoleScroll"><span v-for="(line, index) in consoleDisplayLines" :key="`${index}-${line}`" :style="{ color: consoleLineColor(line) }">{{ line }}</span></pre>
          <span v-else class="muted workspace-empty">{{ selectedArchive ? 'Этот журнал пуст.' : 'Логи появятся после запуска.' }}</span>
          <button v-if="consoleDisplayLines.length && !activeArchiveId && !consoleStickToBottom" class="console-jump-button" type="button" @click="consoleStickToBottom = true; void scrollConsoleIfNeeded()"><Icon name="arrow-down" :size="14" />К последней строке</button>
        </div>
        <div v-if="selectedArchive && !archiveEof" class="workspace-archive-footer"><span>Загружено {{ formatSize(archiveNextOffset) }} из {{ formatSize(selectedArchive.size) }}</span><button class="button button--small button--quiet" type="button" :disabled="archiveLoading" @click="loadMoreArchive">{{ archiveLoading ? 'Загружаю…' : 'Загрузить ещё' }}</button></div>
        <div v-else-if="selectedArchive" class="workspace-archive-footer"><span>Журнал загружен полностью</span><span>{{ formatSize(selectedArchive.size) }}</span></div>
      </div>

      <div v-else-if="tab === 'screenshots'" key="screenshots" class="workspace-card">
        <div class="workspace-card-heading"><div><h3>Скриншоты</h3><span class="muted">Из папки screenshots/ · изображения до 8 MiB</span></div><div class="workspace-card-actions"><button class="button button--small button--quiet" type="button" :disabled="screenshotLoading" @click="loadScreenshots"><Icon name="refresh" :size="14" />Обновить</button><button class="button button--small button--outline" type="button" @click="fileDirectory = 'screenshots'; selectTab('files')"><Icon name="folder" :size="14" />Папка</button></div></div>
        <div v-if="screenshotLoading" class="workspace-loading">Загружаю скриншоты…</div>
        <div v-else-if="screenshotPreviews.length" class="instance-screenshot-grid">
          <figure v-for="item in screenshotPreviews" :key="item.file.path" class="instance-screenshot">
            <img v-if="item.source" :src="item.source" :alt="item.file.name" loading="lazy" />
            <span v-else class="instance-screenshot-fallback"><Icon name="image" :size="24" />Предпросмотр недоступен</span>
            <figcaption><strong>{{ item.file.name }}</strong><small>{{ formatDate(Math.round(item.file.modified_at)) }} · {{ formatSize(item.file.size) }}</small></figcaption>
          </figure>
        </div>
        <div v-else class="workspace-empty-panel"><span class="world-icon world-icon--empty"><Icon name="image" :size="18" /></span><strong>Скриншотов пока нет</strong><span>Снимки игры будут отображаться здесь.</span><button class="button button--small button--quiet" type="button" @click="fileDirectory = 'screenshots'; selectTab('files')">Открыть папку screenshots/</button></div>
        <div v-if="screenshotPageCount > 1" class="instance-screenshot-pagination"><span>{{ screenshotPage + 1 }} / {{ screenshotPageCount }} · {{ screenshotFiles.length }} файлов</span><div><button class="button button--small button--quiet" type="button" :disabled="screenshotLoading || screenshotPage === 0" @click="changeScreenshotPage(-1)">Назад</button><button class="button button--small button--quiet" type="button" :disabled="screenshotLoading || screenshotPage >= screenshotPageCount - 1" @click="changeScreenshotPage(1)">Дальше</button></div></div>
      </div>

      <section v-else-if="tab === 'files'" key="files" class="workspace-card instance-file-explorer" :class="{ 'instance-file-explorer--dragging': fileDropActive }" @dragenter.prevent="onWorkspaceDragEnter" @dragover.prevent @dragleave.prevent="onWorkspaceDragLeave" @drop.prevent="onWorkspaceDrop">
        <div class="workspace-card-heading"><div><h3>Файлы экземпляра</h3><span class="muted">{{ fileDirectory ? `/${fileDirectory}` : 'Корневая папка' }} · изменения доступны после остановки игры</span></div><div class="workspace-card-actions"><button class="button button--small button--quiet" type="button" :disabled="fileLoading || fileMutationBusy" @click="loadWorkspaceFiles"><Icon name="refresh" :size="14" />Обновить</button><button class="button button--small button--quiet" type="button" :disabled="running || fileMutationBusy" @click="createWorkspaceDirectory"><Icon name="plus" :size="14" />Новая папка</button><button class="button button--small button--outline" type="button" :disabled="running || fileMutationBusy" @click="selectWorkspaceUpload"><Icon name="upload" :size="14" />Загрузить</button></div></div>
        
        <nav class="instance-file-breadcrumbs" aria-label="Путь к папке">
          <button type="button" :class="{ active: !fileDirectory }" @click="goToWorkspaceDirectory('')">Экземпляр</button>
          <template v-for="crumb in fileBreadcrumbs" :key="crumb.path"><Icon name="chevron" :size="12" /><button type="button" :class="{ active: fileDirectory === crumb.path }" @click="goToWorkspaceDirectory(crumb.path)">{{ crumb.name }}</button></template>
          <button v-if="fileDirectory" class="instance-file-up" type="button" @click="goToParentDirectory"><Icon name="arrow-up" :size="13" />Вверх</button>
        </nav>
        <div v-if="fileLoading" class="workspace-loading">Читаю папку…</div>
        <div v-else class="instance-file-list">
          <div v-for="file in instanceFiles" :key="file.path" class="instance-file-row">
              <button class="instance-file-open" type="button" :disabled="editorLoading || editorSaving" @click="openWorkspaceFile(file)">
              <Icon :name="file.kind === 'directory' ? 'folder' : file.editable ? 'file' : isImageFile(file) ? 'image' : 'archive'" :size="17" />
              <span><strong>{{ file.name }}</strong><small>{{ file.kind === 'directory' ? 'Папка' : formatSize(file.size) }}<template v-if="file.kind === 'file' && file.editable"> · текстовый файл</template></small></span>
              <Icon v-if="file.kind === 'file' && file.editable" name="edit" :size="14" />
            </button>
            <button class="icon-button" type="button" :disabled="running || fileMutationBusy || editorSaving" :aria-label="`Удалить ${file.name}`" @click="deleteWorkspaceEntry(file)"><Icon name="trash" :size="14" /></button>
          </div>
          <div v-if="!instanceFiles.length" class="workspace-empty-panel"><span class="world-icon world-icon--empty"><Icon name="folder" :size="18" /></span><strong>{{ fileDirectory ? 'Папка пуста' : 'В папке нет файлов' }}</strong><span>Перетащите файлы сюда или выберите их на устройстве.</span></div>
        </div>
        <div v-if="fileMutationBusy" class="instance-file-busy">Выполняю операцию с файлами…</div>
        <div v-if="fileDropActive" class="instance-file-drop-overlay"><Icon name="upload" :size="25" /><strong>Отпустите файлы для загрузки</strong><span>Файлы будут добавлены в {{ fileDirectory ? `/${fileDirectory}` : 'корень экземпляра' }}.</span></div>
        <section v-if="editorPath" class="instance-file-editor">
          <div class="instance-file-editor-heading"><span><Icon name="edit" :size="15" /><strong>{{ editorPath }}</strong></span><button class="icon-button" type="button" aria-label="Закрыть редактор" :disabled="editorSaving" @click="closeWorkspaceFile"><Icon name="close" :size="15" /></button></div>
          <textarea v-model="editorContent" spellcheck="false" aria-label="Содержимое файла" />
          <div class="instance-file-editor-footer"><span>{{ editorContent.length.toLocaleString() }} символов<template v-if="editorContent !== editorOriginalContent"> · есть несохранённые изменения</template></span><button class="button button--small button--accent" type="button" :disabled="running || editorSaving || editorContent === editorOriginalContent" @click="saveWorkspaceFile"><Icon name="save" :size="14" />{{ editorSaving ? 'Сохраняю…' : 'Сохранить' }}</button></div>
        </section>
        <div v-if="filePreview" class="instance-file-preview"><div><strong>{{ filePreview.path }}</strong><button class="icon-button" type="button" aria-label="Закрыть предпросмотр" @click="filePreview = undefined"><Icon name="close" :size="15" /></button></div><img :src="filePreview.source" :alt="filePreview.path" /></div>
      </section>

       <div v-else-if="['mods', 'resourcepacks', 'shaders', 'datapacks'].includes(tab)" key="assets" class="workspace-card">
          <div class="workspace-card-heading"><div><h3>Установленные {{ activeAssetLabel }}</h3><span class="muted">{{ tab === 'datapacks' ? 'По мирам экземпляра' : 'Файлы этого экземпляра' }}</span></div><div class="workspace-card-actions"><button v-if="canCheckResourceUpdates" class="button button--small button--quiet" type="button" :disabled="checkingUpdates || running" @click="checkResourceUpdates"><Icon name="refresh" :size="14" />{{ checkingUpdates ? 'Проверяю…' : 'Проверить обновления' }}</button><button v-if="tab !== 'datapacks'" class="button button--small button--outline" type="button" :disabled="importing" @click="selectFiles"><Icon name="upload" :size="14" />{{ importing ? 'Импорт…' : 'Импортировать' }}</button><button v-if="tab === 'mods' || tab === 'datapacks'" class="button button--small button--accent" type="button" @click="emit('browse-mods')"><Icon name="search" :size="14" />Каталог</button><button v-if="tab === 'mods'" class="button button--small button--quiet" type="button" @click="openDependencyGraph"><Icon name="globe" :size="14" />Зависимости</button><button v-if="tab === 'mods'" class="button button--small button--quiet" type="button" :disabled="modListBusy" @click="exportModList('markdown')">Список .md</button></div></div>
          <div v-if="updateResults.length" class="instance-resource-updates"><article v-for="update in updateResults" :key="`${update.world_name || ''}:${update.path}`" class="instance-resource-update"><span><strong>{{ update.name }}</strong><small>{{ update.current_version || 'Установленная версия' }} → {{ update.version }}<template v-if="update.world_title"> · {{ update.world_title }}</template></small></span><button class="button button--small button--accent" type="button" :disabled="running || Boolean(updatingResourceId)" @click="updateResource(update)"><Icon name="refresh" :size="13" />{{ updatingResourceId === (update.path || update.project_id) ? 'Обновляю…' : 'Обновить' }}</button></article></div>
        
          <div v-if="tab === 'mods' && activeAssets.length" class="workspace-mod-bulkbar">
            <button class="button button--small button--quiet" type="button" :disabled="modBulkBusy" @click="toggleSelectAllMods"><Icon :name="allModsSelected ? 'close' : 'check'" :size="13" />{{ allModsSelected ? 'Снять выделение' : 'Выделить все' }}</button>
            <span class="muted">Выбрано: {{ selectedModCount }} из {{ installedModPaths.length }}</span>
            <span class="workspace-mod-bulkbar-actions">
              <button class="button button--small button--quiet" type="button" :disabled="!selectedModCount || modBulkBusy || running" @click="setSelectedModsEnabled(true)">Включить</button>
              <button class="button button--small button--quiet" type="button" :disabled="!selectedModCount || modBulkBusy || running" @click="setSelectedModsEnabled(false)">Отключить</button>
              <button class="button button--small button--danger" type="button" :disabled="!selectedModCount || modBulkBusy || running" @click="removeSelectedMods"><Icon name="trash" :size="13" />{{ modBulkBusy ? 'Работаю…' : `Удалить выбранные${selectedModCount ? ` (${selectedModCount})` : ''}` }}</button>
            </span>
          </div>
          <div v-if="activeAssets.length" class="workspace-assets">
            <div v-for="asset in activeAssets" :key="asset.path" class="workspace-asset" :class="{ 'workspace-asset--clickable': true, 'workspace-asset--disabled': asset.kind === 'mods' && asset.enabled === false, 'workspace-asset--selected': asset.kind === 'mods' && selectedModPaths.has(asset.path) }" @contextmenu.prevent="assetMenu(asset, $event)"><input v-if="asset.kind === 'mods'" class="workspace-asset-check" type="checkbox" :checked="selectedModPaths.has(asset.path)" :disabled="modBulkBusy" :aria-label="`Выбрать ${asset.display_name || asset.name}`" @change="toggleModSelection(asset.path)" /><span class="workspace-asset-copy"><span class="asset-icon"><img v-if="assetIconSource(asset) && !failedAssetIcons.has(asset.path)" :src="assetIconSource(asset)" alt="" loading="lazy" @error="markAssetIconFailed(asset.path)" /><Icon v-else :name="assetFallbackIcon(asset)" :size="16" /></span><span><strong>{{ asset.display_name || asset.name }}</strong><small><template v-if="asset.version">{{ asset.version }} · </template>{{ asset.author || assetProvider(asset) }}<template v-if="asset.author"> · {{ assetProvider(asset) }}</template> · {{ formatSize(asset.size) }}<template v-if="asset.kind === 'mods'"> · {{ asset.enabled === false ? 'отключён' : 'включён' }}</template></small></span></span><span class="workspace-asset-actions"><button v-if="asset.kind === 'mods'" class="button button--small button--quiet" type="button" @click="toggleMod(asset)">{{ asset.enabled === false ? 'Включить' : 'Отключить' }}</button><button v-if="asset.kind === 'mods'" class="icon-button" type="button" aria-label="Проверить безопасность мода" v-tooltip="'Проверить безопасность'" @click="auditMod(asset)"><Icon name="shield" :size="15" /></button><button v-if="asset.kind === 'shaders'" class="icon-button" type="button" aria-label="Проанализировать шейдер" v-tooltip="'Анализ шейдера и тест на железе'" @click="analyzeShader(asset)"><Icon name="activity" :size="15" /></button><button class="icon-button" type="button" aria-label="Удалить" @click="removeAsset(asset)"><Icon name="trash" :size="15" /></button></span></div>
        </div>
        <span v-else class="muted workspace-empty">Пока ничего не установлено.</span>
      </div>

      <div v-else-if="tab === 'worlds'" key="worlds" class="workspace-card">
        <div class="workspace-card-heading"><div><h3>Миры</h3><span class="muted">saves/ · архив распакуется автоматически, найденные миры попадут в saves/</span></div><div class="workspace-card-actions"><button class="button button--small button--outline" type="button" :disabled="worldImportBusy || running" @click="selectWorldArchive"><Icon name="archive" :size="14" />Импорт архива</button><button class="button button--small button--outline" type="button" :disabled="worldImportBusy || running" @click="selectWorldFolder"><Icon name="folder" :size="14" />Импорт папки</button></div></div>
        
        <div v-if="worldImportBusy" class="workspace-world-import" aria-live="polite"><span><span class="status-dot status-dot--busy" />{{ worldImportLabel }}</span><strong>{{ worldImportProgress }}%</strong><span class="workspace-world-import-bar"><span :style="{ width: `${worldImportProgress}%` }" /></span></div>
        <div v-if="details.worlds.length" class="workspace-assets">
          <div v-for="world in details.worlds" :key="world.id" class="workspace-asset workspace-asset--world" @contextmenu.prevent="worldMenu(world, $event)"><span class="workspace-world-copy"><img v-if="world.icon" :src="world.icon" alt="" class="world-icon" /><span><strong>{{ world.name }}</strong><small>{{ formatSize(world.size) }} · {{ formatDate(world.last_played) }}</small></span></span><button class="button button--small button--accent" type="button" :disabled="loading" @click="launchWorld(world.id)"><Icon name="play" :size="13" />Играть</button></div>
        </div>
        <span v-else class="muted workspace-empty">Миров пока нет. Импортируйте архив (.zip, .tar.gz…) или папку сохранения.</span>
      </div>

      <div v-else-if="tab === 'servers'" key="servers" class="workspace-card">
        <div class="workspace-card-heading"><h3>Серверы</h3><span class="muted">servers.dat</span></div>
        <div v-if="details.servers.length" class="workspace-assets"><div v-for="server in details.servers" :key="server.id" class="workspace-asset"><span><strong>{{ server.name }}</strong><small>{{ server.address }}</small></span><button class="button button--small button--accent" type="button" :disabled="loading" @click="launchServer(server.address)"><Icon name="play" :size="13" />Играть</button></div></div>
        <span v-else class="muted workspace-empty">Посещённых серверов пока нет.</span>
      </div>

      <div v-else-if="tab === 'versions'" key="versions" class="workspace-card">
        <div class="workspace-card-heading">
          <div><h3>Версии</h3><span class="muted">Состав версии экземпляра · смена, удаление и свои файлы</span></div>
          <div class="workspace-card-actions">
            
            <button class="button button--small button--quiet" type="button" :disabled="versionsLoading || versionsBusy" @click="loadVersions"><Icon name="refresh" :size="14" />Обновить</button>
            <button class="button button--small button--outline" type="button" :disabled="versionsBusy || running" @click="chooseVersionJson"><Icon name="upload" :size="14" />Своя версия</button>
          </div>
        </div>
        <div v-if="versionsLoading" class="workspace-loading">Читаю состав версии…</div>
        <div v-else-if="versionsPayload" class="workspace-version-list">
          <div v-for="component in versionsPayload.components" :key="component.id" class="workspace-version-row">
            <span class="workspace-version-icon"><Icon :name="component.id === 'minecraft' ? 'instances' : component.id === 'loader' ? 'mods' : component.id === 'lwjgl' ? 'monitor' : component.id === 'java' ? 'settings' : 'archive'" :size="16" /></span>
            <span class="workspace-version-copy">
              <strong>{{ component.title }}</strong>
              <small>{{ component.version }} · {{ component.meta }}</small>
            </span>
            <span class="workspace-version-actions">
              <button v-if="component.changeable" class="button button--small button--outline" type="button" :disabled="versionsBusy || running" @click="component.id === 'minecraft' ? changeMinecraftVersion(component) : component.id === 'loader' ? changeLoader(component) : changeLwjgl(component)">
                <Icon name="refresh" :size="13" />{{ versionsBusy ? 'Меняю…' : 'Поменять версию' }}
              </button>
              <button v-if="component.removable && component.id === 'loader'" class="button button--small button--quiet" type="button" :disabled="versionsBusy || running" @click="changeLoader(component)"><Icon name="trash" :size="13" />Убрать</button>
              <button v-if="component.removable && component.id === 'lwjgl'" class="button button--small button--quiet" type="button" :disabled="versionsBusy || running" @click="changeLwjgl(component)"><Icon name="undo" :size="13" />Вернуть</button>
            </span>
          </div>
          <p class="field-help">Смена версии Minecraft или модлоадера не трогает моды, миры и настройки — только файлы самой версии. Своя версия — это JSON-файл версии (например, кастомный профиль или фиксы); библиотеки скачаются автоматически.</p>
        </div>
        <span v-else class="muted workspace-empty">Откройте вкладку ещё раз — не удалось прочитать состав версии.</span>
      </div>

      <div v-else-if="tab === 'snapshots'" key="snapshots" class="workspace-card">
        <div class="workspace-card-heading"><div><h3>Снапшоты ресурсов</h3><span class="muted">Резервные копии файлов экземпляра без остановки во время чтения.</span></div><div class="workspace-card-actions"><CustomSelect v-model="snapshotCategory" :options="[{ value: 'mods', label: 'Моды' }, { value: 'resourcepacks', label: 'Ресурспаки' }, { value: 'shaders', label: 'Шейдеры' }, { value: 'saves', label: 'Миры' }]" /><button class="button button--small button--accent" type="button" :disabled="snapshotBusy || running" @click="createSnapshot">{{ snapshotBusy ? 'Работаю…' : 'Создать' }}</button></div></div>
        <div class="server-snapshot-list"><article v-for="snapshot in snapshots" :key="snapshot.id" class="server-snapshot-row"><span><strong>{{ snapshot.id }}</strong><small>{{ snapshot.file_count }} файлов · {{ formatSize(snapshot.source_bytes) }} · {{ formatDate(snapshot.created_at) }}</small></span><span><button class="button button--small button--quiet" type="button" :disabled="snapshotBusy || running" @click="restoreSnapshot(snapshot)">Восстановить</button><button class="icon-button" type="button" aria-label="Удалить снапшот" :disabled="snapshotBusy" @click="deleteSnapshot(snapshot)"><Icon name="trash" :size="14" /></button></span></article><span v-if="!snapshots.length" class="muted workspace-empty">Снапшотов этой категории пока нет.</span></div>
      </div>

      <div v-else-if="tab === 'backups'" key="backups" class="workspace-card">
        <div class="workspace-card-heading"><div><h3>Резервные копии миров</h3><span class="muted">Архивы saves/ (или world* у сервера) по расписанию и вручную. {{ backupSchedule.last_run ? `Последний запуск: ${formatDate(backupSchedule.last_run)}` : 'Расписание ещё не срабатывало.' }}{{ backupSchedule.last_error ? ` Ошибка: ${backupSchedule.last_error}` : '' }}</span></div><div class="workspace-card-actions"><button class="button button--small button--accent" type="button" :disabled="backupBusy || running || loading" @click="createBackupNow">{{ backupBusy ? 'Работаю…' : 'Создать копию' }}</button></div></div>
        <div class="workspace-form-grid">
          <label class="workspace-check workspace-check--wide"><input v-model="backupSchedule.enabled" type="checkbox" /><span><strong>Резервное копирование по расписанию</strong><small>Глобальные настройки задают значения по умолчанию; здесь можно переопределить.</small></span></label>
          <label class="workspace-check workspace-check--wide"><input v-model="backupSchedule.override" type="checkbox" /><span><strong>Переопределить глобальное расписание</strong><small>Без галочки параметры берутся из общих настроек лаунчера.</small></span></label>
          <div class="field"><span>Интервал, часов</span><NumericStepper v-model="backupSchedule.interval_hours" :min="1" :max="720" aria-label="Интервал в часах" /></div>
          <div class="field"><span>Хранить копий</span><NumericStepper v-model="backupSchedule.keep" :min="1" :max="50" aria-label="Количество копий" /></div>
          <div class="field"><span>Куда сохранять</span><CustomSelect v-model="backupSchedule.destination" :options="backupDestinationOptions" /><small class="field-help">WebDAV использует облачный endpoint из общих настроек.</small></div>
          <div class="workspace-card-actions"><button class="button button--small button--outline" type="button" :disabled="backupBusy" @click="saveBackupSchedule">Сохранить расписание</button></div>
        </div>
        <div class="server-snapshot-list">
          <article v-for="backup in backups" :key="backup.id" class="server-snapshot-row">
            <span><strong>{{ backup.id }}</strong><small>{{ formatSize(backup.size_bytes) }} · {{ formatDate(Math.round(backup.created_at)) }} · {{ backup.destination === 'cloud' ? 'WebDAV' : 'локально' }}</small></span>
            <span><button class="button button--small button--quiet" type="button" :disabled="backupBusy || running" @click="restoreBackup(backup)">Восстановить</button><button class="icon-button" type="button" aria-label="Удалить копию" :disabled="backupBusy" @click="deleteBackup(backup)"><Icon name="trash" :size="14" /></button></span>
          </article>
          <span v-if="backupsLoading" class="muted workspace-empty">Загружаю копии…</span>
          <span v-else-if="!backups.length" class="muted workspace-empty">Копий пока нет. Создайте первую вручную или включите расписание.</span>
        </div>
      </div>

       <div v-else key="settings" class="workspace-card workspace-settings-card">
          <div class="settings-save-bar" :class="`settings-save-bar--${settingsSaveState}`">
            <div class="settings-save-copy"><span class="status-dot" :class="{ 'status-dot--green': settingsSaveState === 'saved', 'status-dot--busy': settingsSaveState === 'saving' }" /><div><strong>{{ settingsSaveState === 'saving' ? 'Сохраняю настройки' : settingsSaveState === 'error' ? 'Нужно исправление' : settingsSaveState === 'dirty' ? 'Есть несохранённые изменения' : 'Настройки экземпляра сохранены' }}</strong><small>{{ settingsSaveMessage }}</small></div></div>
             <button class="button button--small button--accent settings-save-button" :class="{ 'settings-save-button--saved': settingsSaveState === 'saved' }" type="button" :disabled="saving || settingsSaveState === 'saved'" @click="save"><Icon v-if="settingsSaveState === 'saved'" name="check" :size="13" />{{ settingsSaveState === 'saving' ? 'Сохраняю…' : settingsSaveState === 'error' ? 'Повторить' : settingsSaveState === 'saved' ? 'Сохранено' : 'Сохранить изменения' }}</button>
          </div>
           <div class="workspace-card-heading"><div><h3>Настройки экземпляра</h3><span class="muted">Переопределяют глобальные параметры</span></div></div>
          <section class="instance-console-settings">
            <div class="workspace-card-heading"><div><h3>Цвета консоли</h3><span class="muted">Изменения применяются к live-логам и архивам.</span></div></div>
            <div class="instance-console-color-grid">
              <label v-for="color in consoleColorRows" :key="color.key" class="instance-console-color-row">
                <span><strong>{{ color.label }}</strong><code>{{ draft.console_colors[color.key] }}</code></span>
                <input v-model="draft.console_colors[color.key]" type="color" :aria-label="`Цвет: ${color.label}`" />
              </label>
            </div>
            <div class="instance-console-preview"><span>Предпросмотр</span><pre><span v-for="color in consoleColorRows" :key="color.key" :style="{ color: draft.console_colors[color.key] }">{{ color.sample }}</span></pre></div>
          </section>
          <section class="instance-profile-editor">
           <div class="workspace-card-heading"><div><h3>Профиль экземпляра</h3><span class="muted">Время игры: {{ formatPlayedTime(details.instance.played_seconds) }}</span></div><button class="button button--small button--outline" type="button" :disabled="profileSaving" @click="saveProfile">{{ profileSaving ? 'Сохраняю…' : 'Сохранить профиль' }}</button></div>
           <div class="instance-profile-media">
             <div class="profile-upload profile-upload--icon">
               <img v-if="profileIconPreview" :src="profileIconPreview" alt="" />
               <span v-else class="profile-upload-placeholder"><Icon name="instances" :size="22" /></span>
               <button class="button button--small button--quiet" type="button" @click="chooseProfileAsset('icon')">Иконка</button>
               <button v-if="profileIconPreview" class="text-button" type="button" @click="clearProfileAsset('icon')">Убрать</button>
             </div>
             <div class="profile-upload profile-upload--banner">
               <img v-if="profileBannerPreview" :src="profileBannerPreview" alt="" />
               <span v-else class="profile-upload-placeholder"><Icon name="image" :size="22" /></span>
               <span class="profile-upload-caption">Баннер · затемнение снизу зависит от темы</span>
               <button class="button button--small button--quiet" type="button" @click="chooseProfileAsset('banner')">Загрузить баннер</button>
               <button v-if="profileBannerPreview" class="text-button" type="button" @click="clearProfileAsset('banner')">Убрать</button>
             </div>
           </div>
           
           <div class="workspace-form-grid">
             <label class="field"><span>Отображаемое название</span><input v-model="profileDraft.display_name" maxlength="80" placeholder="Например, Survival" /></label>
             <div class="field"><span>Группа <button class="text-button" type="button" @click="createProfileGroup">Создать группу</button></span><CustomSelect v-model="profileDraft.group" :options="profileGroupOptions" /></div>
             <label class="field field--wide"><span>Описание</span><textarea v-model="profileDraft.description" maxlength="500" rows="3" placeholder="Короткое описание профиля и его задачи" /></label>
              <label class="field field--wide"><span>Заметки <small>видны только вам: порядок модов, что доделать</small></span><textarea v-model="profileDraft.notes" maxlength="4000" rows="4" placeholder="Заметки об экземпляре" /></label>
              <label class="field field--wide"><span>Теги <small>через запятую, до 12; фильтруют список экземпляров</small></span><input v-model="profileDraft.tags" placeholder="выживание, моды, тесты" /></label>
           </div>
         </section>
         <section class="instance-integration-panel">
           <div class="workspace-card-heading"><div><h3>Ярлыки и списки</h3><span class="muted">Запуск в один клик из системы и экспорт состава сборки</span></div></div>
           <div class="workspace-form-grid">
             <div class="field"><span>Ярлык на рабочем столе</span>
               <div class="integration-toggle-row">
                 <button class="button button--small button--outline" type="button" :disabled="shortcutBusy || !shortcutStatus" @click="toggleShortcut('desktop')">
                   <Icon name="instances" :size="13" />{{ shortcutBusy ? '…' : shortcutStatus?.desktop?.exists ? 'Убрать ярлык' : 'Создать ярлык' }}
                 </button>
                 <small class="field-help">{{ shortcutStatus?.desktop?.exists ? shortcutStatus.desktop.path : 'Двойной клик запускает лаунчер и сразу этот экземпляр' }}</small>
               </div>
             </div>
             <div class="field"><span>Ярлык в меню приложений</span>
               <div class="integration-toggle-row">
                 <button class="button button--small button--outline" type="button" :disabled="shortcutBusy || !shortcutStatus" @click="toggleShortcut('menu')">
                   <Icon name="list" :size="13" />{{ shortcutBusy ? '…' : shortcutStatus?.menu?.exists ? 'Убрать ярлык' : 'Создать ярлык' }}
                 </button>
                 <small class="field-help">{{ shortcutStatus?.menu?.exists ? shortcutStatus.menu.path : 'Появляется в меню/пуске среди приложений' }}</small>
               </div>
             </div>
             <div class="field field--wide"><span>Экспорт списка модов</span>
               <div class="integration-toggle-row">
                 <button class="button button--small button--quiet" type="button" :disabled="modListBusy" @click="exportModList('markdown')">Markdown</button>
                 <button class="button button--small button--quiet" type="button" :disabled="modListBusy" @click="exportModList('csv')">CSV</button>
                 <button class="button button--small button--quiet" type="button" :disabled="modListBusy" @click="exportModList('text')">Текст</button>
                 <small class="field-help">Имена, версии и состояние включённости из манифестов jar</small>
               </div>
             </div>
           </div>
         </section>
         <div class="workspace-form-grid">
           <div class="field"><span>Память JVM</span><MemoryField v-model="draft.memory" /></div>
          <label class="field"><span>Java executable</span><input v-model="draft.java_path" placeholder="auto или /usr/lib/jvm/.../bin/java" /></label>
           <div class="field"><span>Graphics / GLX</span><CustomSelect v-model="draft.graphics_mode" :options="graphicsOptions" /><small class="field-help">Auto наследует глобальный режим; если глобально тоже Auto, NVIDIA выбирается при обнаружении через nvidia-smi.</small></div>
            <div class="field"><span>Пресет GC экземпляра</span><CustomSelect v-model="draft.jvm_preset" :options="jvmPresetOptions" /><small class="field-help">Память, Java, пресет и аргументы ниже важнее профиля запуска.</small></div>
           <div class="field"><span>Ширина окна</span><NumericStepper v-model="draft.window_width" :min="320" :max="7680" aria-label="Ширина окна" /></div>
           <div class="field"><span>Высота окна</span><NumericStepper v-model="draft.window_height" :min="240" :max="7680" aria-label="Высота окна" /></div>
          <label class="workspace-check"><input v-model="draft.fullscreen" type="checkbox" /><span>Полноэкранный режим</span></label>
           <label class="field"><span>Мир для Quick Play</span><input v-model="draft.quick_play_world" placeholder="Пусто = обычный запуск" /></label>
            <label class="field"><span>Сервер для Quick Play</span><input v-model="draft.quick_play_server" placeholder="example.org:25565" /></label>
             <label class="field field--wide"><span>Дополнительные JVM-аргументы <small>по одному на строку</small></span><textarea :value="draft.jvm_arguments.join('\n')" rows="3" @change="setDraftArguments('jvm_arguments', $event)" placeholder="-Dfml.readTimeout=30" /></label>
             <label class="field field--wide"><span>Аргументы игры <small>по одному на строку</small></span><textarea :value="draft.game_arguments.join('\n')" rows="2" @change="setDraftArguments('game_arguments', $event)" placeholder="--width 1280" /></label>
          </div>
          <section class="instance-security-panel">
             <div class="workspace-card-heading"><div><h3>Безопасность экземпляра</h3><span class="muted">Разрешения действуют для всей JVM. Сеть игры включена по умолчанию — без неё не работают серверы; выключите её только для недоверенных сборок.</span></div><span class="security-profile-badge" :class="`security-profile-badge--${draft.security_profile}`">{{ draft.security_profile }}</span></div>
            <div class="workspace-form-grid">
              <div class="field"><span>Security profile</span><CustomSelect v-model="draft.security_profile" :options="securityProfileOptions" /></div>
            </div>
             <p class="field-help">Strict использует OS-level sandbox: bubblewrap на Linux, AppContainer и Job Object на Windows. На Windows нельзя отдельно запретить звук и скрыть окно игры, а подключение к серверу на этом же компьютере (localhost) блокируется системой. Manual ограничивает окружение, но не заменяет OS-level sandbox. Разрешения JVM/mod, которые нельзя надёжно перехватить внутри одной JVM, отклоняются до запуска и доступны только в Off.</p>
            <div class="security-permission-grid">
              <label v-for="permission in securityPermissionOptions" :key="permission.value" class="workspace-check security-permission" :class="{ 'security-permission--risk': permission.highRisk }">
                <input v-model="draft.security_permissions" type="checkbox" :value="permission.value" />
                <span><strong>{{ permission.label }}</strong><small>{{ permission.description }}</small></span>
              </label>
            </div>
             <p class="field-help">Права отдельных Java-модов не показываются: все моды работают внутри одного JVM и лаунчер не может безопасно применить такие разрешения. Изоляция действует на весь экземпляр.</p>
          </section>
         </div>
      </Transition>
    </div>
  </section>
  <section v-else class="workspace-shell workspace-empty-state"><span class="empty-orb empty-orb--large"><Icon name="instances" :size="28" /></span><h3>Выберите экземпляр</h3><p>Здесь появятся его ресурсы, миры, серверы и отдельные настройки.</p></section>
  <ModAuditDialog ref="modAuditDialog" @toast="(message: string) => emit('toast', message)" />
  <ShaderAnalysisDialog v-if="shaderAnalysisTarget && instance" :instance="instance" :shader="shaderAnalysisTarget" @close="shaderAnalysisTarget = null" @toast="(message: string) => emit('toast', message)" />
  <UsageStatisticsDialog v-if="usageDialogOpen && instance" :instance="instance" :metrics="metrics" :metric-history="metricHistory" :last-metrics="lastMetrics" :assets="details?.assets?.mods || []" :running="running" @close="usageDialogOpen = false" />

  <div v-if="lastCrash" class="crash-banner" role="alert">
    <div class="crash-banner-head">
      <span class="crash-banner-icon"><Icon name="warning" :size="18" /></span>
      <div>
        <strong>{{ lastCrash.title }}</strong>
        <small v-if="lastCrash.exit_code !== null && lastCrash.exit_code !== 0">Код выхода: {{ lastCrash.exit_code }}</small>
      </div>
      <button class="button button--small button--outline" type="button" :disabled="diagnoseBusy || running" @click="diagnoseInstance(lastCrash?.exit_code ?? 1)"><Icon name="shield" :size="13" />Починить…</button>
      <button class="icon-button" type="button" aria-label="Закрыть" @click="lastCrash = undefined"><Icon name="close" :size="14" /></button>
    </div>
    <ul class="crash-banner-hints">
      <li v-for="(hint, index) in lastCrash.hints" :key="index"><strong>{{ hint.title }}</strong><small>{{ hint.detail }}</small></li>
    </ul>
    <div v-if="lastCrash.suspected_mods.length" class="crash-banner-mods">
      <span>Подозреваемые моды:</span>
      <span v-for="mod in lastCrash.suspected_mods" :key="mod.name" class="crash-mod-chip">
        {{ mod.file || mod.name }}
        <button v-if="mod.file" class="text-button" type="button" @click="removeSuspectedMod(mod.file)">удалить</button>
      </span>
    </div>
    <Collapsible class="crash-banner-log" title="Выдержка лога">
      <pre>{{ lastCrash.excerpt.join('\n') }}</pre>
    </Collapsible>
  </div>

  <div v-if="depGraphOpen" class="dep-graph-dialog" role="dialog" aria-label="Дерево зависимостей модов" @click.self="depGraphOpen = false">
    <div class="dep-graph-card">
      <div class="workspace-card-heading">
        <div><h3>Дерево зависимостей модов</h3><span class="muted">{{ depGraph?.nodes.length || 0 }} модов · {{ depGraph?.edges.length || 0 }} связей · читается из манифестов jar без сети</span></div>
        <button class="icon-button" type="button" aria-label="Закрыть" @click="depGraphOpen = false"><Icon name="close" :size="14" /></button>
      </div>
      <p v-if="depGraphLoading" class="muted">Читаю манифесты…</p>
      <template v-else-if="depGraph">
        <div v-if="depGraph.missing.length" class="dep-graph-missing">
          <strong>Отсутствуют:</strong>
          <span v-for="missing in depGraph.missing" :key="missing.id" class="dep-chip" :class="{ 'dep-chip--optional': missing.optional }">
            {{ missing.id }} <small>← {{ missing.required_by }}{{ missing.optional ? ' (необязательный)' : '' }}</small>
          </span>
        </div>
        <div v-else class="muted">Отсутствующих зависимостей не найдено.</div>
        <ul class="dep-graph-list">
          <li v-for="node in depGraph.nodes" :key="node.id" class="dep-graph-node" :class="{ 'dep-graph-node--disabled': !node.enabled }">
            <div class="dep-graph-node-head">
              <strong>{{ node.display_name }}</strong>
              <small>{{ node.file }}<template v-if="node.version"> · {{ node.version }}</template><template v-if="!node.enabled"> · отключён</template></small>
              <button class="button button--small button--quiet" type="button" @click="checkRemovalImpact(node.id)">Что сломается?</button>
            </div>
            <p v-if="depImpact[node.id]" class="dep-impact">{{ depImpact[node.id].length ? `При удалении сломаются: ${depImpact[node.id].join(', ')}` : 'При удалении ничего не сломается' }}</p>
            <div v-if="node.dependencies.length" class="dep-graph-deps">
              <span v-for="dependency in node.dependencies" :key="dependency.id" class="dep-chip" :class="{ 'dep-chip--optional': dependency.optional, 'dep-chip--missing': !depGraph.nodes.some((candidate) => candidate.id === dependency.id) }">
                {{ dependency.id }}<small v-if="dependency.constraint && dependency.constraint !== 'любая'"> {{ dependency.constraint }}</small>
              </span>
            </div>
          </li>
        </ul>
      </template>
    </div>
  </div>
</template>
