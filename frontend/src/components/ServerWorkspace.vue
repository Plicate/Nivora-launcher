<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { pickFile } from '../services/fileBrowser'
import Icon from './Icon.vue'
import Collapsible from './Collapsible.vue'
import CustomSelect from './CustomSelect.vue'
import CatalogDetailsDialog from './CatalogDetailsDialog.vue'
import ServerNetworkPanel from './ServerNetworkPanel.vue'
import ServerAccessPanel from './ServerAccessPanel.vue'
import ServerConsole from './ServerConsole.vue'
import CoreBadge from './CoreBadge.vue'
import ModAuditDialog from './ModAuditDialog.vue'
import NumericStepper from './NumericStepper.vue'
import { bridge } from '../services/bridge'
import { confirmDialog } from '../services/dialogs'
import type { CatalogSort, InstanceInfo, MemoryUnit, PluginCatalog, ServerConfig, ServerCore, ServerDetails, ServerFile, ServerPluginResult, ServerPluginUpdate, ServerSnapshot, ServerStatus, SelectOption, ServerMetrics } from '../types'
import type { PluginInstanceRef } from '../../../plugins/api'

type ServerTab = 'overview' | 'console' | 'files' | 'editor' | 'players' | 'plugins' | 'snapshots' | 'network' | 'settings'
type ServerGroup = 'overview' | 'runtime' | 'management' | 'settings'

const props = withDefaults(defineProps<{
  instance?: InstanceInfo
  pluginCatalogs?: PluginCatalog[]
  serverCores?: ServerCore[]
  confirmDestructive?: boolean
  focusTab?: ServerTab
  settingsOnly?: boolean
  showSettingsTab?: boolean
  snapshotCreated?: (instance: PluginInstanceRef, snapshot: ServerSnapshot) => Promise<void>
}>(), { settingsOnly: false, showSettingsTab: true })
const emit = defineEmits<{ (event: 'toast', message: string): void; (event: 'changed'): void }>()

const tabs: Array<{ id: ServerTab; label: string }> = [
  { id: 'overview', label: 'Dashboard' },
  { id: 'console', label: 'Консоль' },
  { id: 'files', label: 'Файлы' },
  { id: 'editor', label: 'Редактор' },
  { id: 'players', label: 'Доступ и свойства' },
  { id: 'plugins', label: 'Плагины' },
  { id: 'snapshots', label: 'Снапшоты' },
  { id: 'network', label: 'Сеть и белый IP' },
  { id: 'settings', label: 'Настройки' },
]
const tabGroups: Array<{ id: ServerGroup; label: string }> = [
  { id: 'overview', label: 'Dashboard' },
  { id: 'runtime', label: 'Runtime' },
  { id: 'management', label: 'Управление' },
  { id: 'settings', label: 'Настройки' },
]
const groupTabs: Record<ServerGroup, ServerTab[]> = {
  overview: ['overview'],
  runtime: ['console', 'files', 'editor'],
  management: ['players', 'plugins', 'snapshots', 'network'],
  settings: ['settings'],
}

const tab = ref<ServerTab>(props.focusTab || (props.settingsOnly ? 'settings' : 'overview'))
const activeGroup = ref<ServerGroup>('overview')
const details = ref<ServerDetails>()
const loading = ref(false)
const detailsError = ref('')
const command = ref('')
const logs = ref<string[]>([])
const logCursor = ref(0)
const metricHistory = ref<ServerMetrics[]>([])
const currentDirectory = ref('')
const selectedFile = ref<ServerFile>()
const editorContent = ref('')
const editorDirty = ref(false)
const coreDropActive = ref(false)
let coreDragDepth = 0
const profileIconFile = ref<{ fileName: string; data: string; size: number }>()
const profileBannerFile = ref<{ fileName: string; data: string; size: number }>()
const profileIconPreview = ref('')
const profileBannerPreview = ref('')
const clearProfileIcon = ref(false)
const clearProfileBanner = ref(false)
const profileSaving = ref(false)
const profileGroups = ref<string[]>([])
const profileDraft = reactive({ display_name: '', description: '', group: '' })
const pluginQuery = ref('')
const pluginCatalogId = ref('')
const pluginSortMode = ref<CatalogSort>('relevance')
const pluginResults = ref<ServerPluginResult[]>([])
const pluginBusy = ref(false)
const pluginUpdates = ref<ServerPluginUpdate[]>([])
const pluginUpdatesBusy = ref(false)
const pluginDetailsOpen = ref(false)
const pluginDetailsLoading = ref(false)
const pluginDetailsError = ref('')
const pluginDetails = ref<Record<string, unknown>>()
const selectedPluginDetails = ref<ServerPluginResult>()
let pluginDetailsRequestId = 0
const pluginSortOptions: SelectOption[] = [
  { value: 'relevance', label: 'Релевантность' },
  { value: 'downloads', label: 'Больше загрузок' },
  { value: 'newest', label: 'Сначала новые' },
  { value: 'updated', label: 'Недавно обновлялись' },
]
const coreVersionBusy = ref(false)
const settingsSaveState = ref<'saved' | 'dirty' | 'saving' | 'error'>('saved')
const settingsSaveMessage = ref('Изменения сохранены')
const syncingSettings = ref(false)
const coreVersionOptions = ref<SelectOption[]>([])
let coreVersionRequestId = 0
const memoryValue = ref(4)
const memoryUnit = ref<MemoryUnit>('GiB')
const snapshotCategory = ref<'mods' | 'plugins'>('plugins')
const snapshotBusy = ref(false)
const coreDownloadBusy = ref(false)
const statusBusy = ref(false)
const logBusy = ref(false)
const actionBusy = ref(false)
const activeAction = ref<'start' | 'stop' | 'restart' | ''>('')
const launchError = ref('')
const securityProfileOptions = [
  { value: 'strict', label: 'Strict', meta: 'bubblewrap' },
  { value: 'manual', label: 'Manual', meta: 'без полной изоляции' },
  { value: 'off', label: 'Off', meta: 'только для доверенного ядра' },
]
const draft = reactive<ServerConfig>({
  core: 'custom', version: '', build: '', port: 25565, max_players: 20, motd: '', memory: '4GiB', java_path: '',
  security_profile: 'strict', security_permissions: ['network:game', 'files:instance'],
  eula_accepted: false, auto_restart: false,
  restart_schedule: { enabled: false, time: '04:00', warn_minutes: 1, last_run: '' },
  snapshot_retention_hours: 5, snapshot_max_count: 1,
})
let refreshTimer: number | undefined
let logTimer: number | undefined
let loadRequestId = 0

const instance = computed(() => props.instance)
const files = computed(() => details.value?.files || [])
const snapshots = computed<ServerSnapshot[]>(() => details.value?.snapshots?.[snapshotCategory.value] || [])
const catalogOptions = computed(() => (props.pluginCatalogs || []).filter((item) => item.enabled !== false).map((item) => ({ value: item.id, label: item.name, meta: item.kind })))
const coreOptions = computed<SelectOption[]>(() => (props.serverCores || []).map((item) => ({ value: item.id, label: item.name, meta: `${item.family} · ${item.description}` })))
const memoryUnitOptions: SelectOption[] = [
  { value: 'MiB', label: 'MiB', meta: '1024 KiB' },
  { value: 'GiB', label: 'GiB', meta: '1024 MiB' },
]
const memoryUnitKiB = computed(() => memoryUnit.value === 'GiB' ? 1024 * 1024 : 1024)
const memoryMaximum = computed(() => {
  const limit = Number(details.value?.memory_limit_kib || 0)
  const precision = memoryUnit.value === 'GiB' ? 100 : 1
  return Math.max(0.01, Math.floor(limit / memoryUnitKiB.value * precision) / precision)
})
const memoryMinimum = computed(() => Math.min(memoryUnit.value === 'GiB' ? 0.25 : 128, memoryMaximum.value))
const memoryStep = computed(() => Math.min(memoryMinimum.value, memoryMaximum.value))
const memoryDecimals = computed(() => memoryUnit.value === 'GiB' ? 2 : 0)
const memoryLimitLabel = computed(() => formatBytes(Number(details.value?.memory_limit_kib || 0) * 1024))
const systemMemoryLabel = computed(() => formatBytes(Number(details.value?.memory_total_kib || 0) * 1024))
const memorySetting = computed(() => {
  const minimum = Math.min(memoryMinimum.value, memoryMaximum.value)
  const value = Math.min(memoryMaximum.value, Math.max(minimum, Number(memoryValue.value) || minimum))
  return `${Number(value.toFixed(memoryDecimals.value))}${memoryUnit.value}`
})
const running = computed(() => Boolean(details.value?.status.running))
const coreLabel = computed(() => details.value?.config.core || instance.value?.server_core || 'custom')
const visibleTabs = computed(() => tabs.filter((item) =>
  groupTabs[activeGroup.value].includes(item.id)
  && (props.showSettingsTab || item.id !== 'settings')
,
))

function selectGroup(group: ServerGroup) {
  activeGroup.value = group
  const nextTab = groupTabs[group][0]
  if (nextTab) tab.value = nextTab
}

function selectTab(value: ServerTab) {
  tab.value = value
  const group = (Object.keys(groupTabs) as ServerGroup[]).find((key) => groupTabs[key].includes(value))
  if (group) activeGroup.value = group
}

function recordMetric(value?: ServerMetrics) {
  if (!value) return
  const previous = metricHistory.value[metricHistory.value.length - 1]
  if (value.pid > 0 && previous?.pid !== value.pid) metricHistory.value = []
  metricHistory.value = [...metricHistory.value, value].slice(-60)
}

function metricPoints(key: 'cpu_percent' | 'ram_bytes' | 'tps' | 'network_rx_bps' | 'network_tx_bps'): string {
  const values = metricHistory.value.map((value) => Number(value[key] || 0))
  if (!values.length) return '0,40 100,40'
  const min = Math.min(...values)
  const max = Math.max(...values)
  const span = Math.max(1, max - min)
  return values.map((value, index) => `${(index / Math.max(1, values.length - 1)) * 100},${38 - ((value - min) / span) * 32}`).join(' ')
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return typeof error === 'string' ? error : 'Неизвестная ошибка'
}

function formatBytes(value: number): string {
  if (value >= 1024 ** 3) return `${(value / 1024 ** 3).toFixed(1)} GiB`
  if (value >= 1024 ** 2) return `${(value / 1024 ** 2).toFixed(1)} MiB`
  if (value >= 1024) return `${(value / 1024).toFixed(1)} KiB`
  return `${Math.max(0, Math.round(value || 0))} B`
}

function formatUptime(value: number): string {
  const seconds = Math.max(0, Math.round(value || 0))
  const hours = Math.floor(seconds / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  const remainder = seconds % 60
  return `${String(hours).padStart(2, '0')}:${String(minutes).padStart(2, '0')}:${String(remainder).padStart(2, '0')}`
}

function formatDate(value: number): string {
  return value ? new Date(value * 1000).toLocaleString() : 'только что'
}

function syncDetails(value: ServerDetails) {
  details.value = value
  syncProfileDraft(value.server)
  syncingSettings.value = true
  Object.assign(draft, value.config)
  syncMemoryDraft(value.config.memory)
  settingsSaveState.value = 'saved'
  settingsSaveMessage.value = 'Изменения сохранены'
  void nextTick(() => { syncingSettings.value = false })
  void refreshCoreVersions(value.config.core, false)
  logs.value = value.logs.lines || []
  logCursor.value = value.logs.cursor || 0
  metricHistory.value = value.status.metrics ? [value.status.metrics] : []
  if (!pluginCatalogId.value) pluginCatalogId.value = catalogOptions.value[0]?.value || ''
}

function syncProfileDraft(value: InstanceInfo) {
  profileDraft.display_name = value.display_name || value.name
  profileDraft.description = value.description || ''
  profileDraft.group = value.group || ''
  if (profileIconPreview.value.startsWith('blob:')) URL.revokeObjectURL(profileIconPreview.value)
  if (profileBannerPreview.value.startsWith('blob:')) URL.revokeObjectURL(profileBannerPreview.value)
  profileIconPreview.value = value.icon || ''
  profileBannerPreview.value = value.banner || ''
  profileIconFile.value = undefined
  profileBannerFile.value = undefined
  clearProfileIcon.value = false
  clearProfileBanner.value = false
}

const profileGroupOptions = computed<SelectOption[]>(() => [
  { value: '', label: 'Без группы', meta: 'Не добавлять в подборку' },
  ...profileGroups.value.map((group) => ({ value: group, label: group })),
])

function chooseProfileAsset(kind: 'icon' | 'banner') {
  void pickFile({
    accept: 'image/png,image/jpeg,image/webp',
    title: kind === 'icon' ? 'Иконка сервера' : 'Баннер сервера',
  }).then((file) => {
    if (file) loadProfileAsset(file, kind)
  })
}

function loadProfileAsset(file: { fileName: string; data: string; size: number }, kind: 'icon' | 'banner') {
  const limit = kind === 'icon' ? 2 * 1024 * 1024 : 8 * 1024 * 1024
  if (file.size > limit) {
    emit('toast', 'Файл профиля слишком большой')
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

function profileAssetMime(name: string): string {
  const lower = name.toLowerCase()
  if (lower.endsWith('.png')) return 'image/png'
  if (lower.endsWith('.webp')) return 'image/webp'
  return 'image/jpeg'
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
  if (!instance.value) return
  profileSaving.value = true
  try {
    const payload: Parameters<typeof bridge.updateInstanceProfile>[0] = {
      name: instance.value.name,
      display_name: profileDraft.display_name,
      description: profileDraft.description,
      group: profileDraft.group,
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
    if (details.value) details.value.server = result.instance
    syncProfileDraft(result.instance)
    emit('changed')
    emit('toast', 'Профиль сервера сохранён')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    profileSaving.value = false
  }
}

function syncMemoryDraft(value: string) {
  const match = String(value || '').trim().match(/^(\d+(?:\.\d+)?)\s*(MiB|GiB)$/i)
  if (!match) return
  memoryUnit.value = match[2].toLowerCase() === 'gib' ? 'GiB' : 'MiB'
  const minimum = Math.min(memoryMinimum.value, memoryMaximum.value)
  memoryValue.value = Math.min(memoryMaximum.value, Math.max(minimum, Number(match[1])))
}

async function refreshCoreVersions(core: string, resetCurrent = true) {
  if (!core) return
  const requestId = ++coreVersionRequestId
  coreVersionBusy.value = true
  try {
    const result = await bridge.serverCoreVersions(core)
    if (requestId !== coreVersionRequestId) return
    const versions = (result.versions || []).filter(Boolean)
    coreVersionOptions.value = versions.map((version) => ({ value: version, label: version, meta: 'Официальный каталог ядра' }))
    if (!coreVersionOptions.value.length && draft.version) {
      coreVersionOptions.value = [{ value: draft.version, label: draft.version, meta: 'Текущая версия' }]
    } else if (!coreVersionOptions.value.some((option) => option.value === draft.version)) {
      // сменили ядро, а прежней версии у нового нет — берём свежую;
      // при обычной перезагрузке списка сохранённую версию не теряем.
      if (resetCurrent) draft.version = coreVersionOptions.value[0].value
      else if (draft.version) coreVersionOptions.value.unshift({ value: draft.version, label: draft.version, meta: 'Текущая версия' })
    }
  } catch {
    if (requestId === coreVersionRequestId) coreVersionOptions.value = draft.version ? [{ value: draft.version, label: draft.version, meta: 'Текущая версия' }] : []
  } finally {
    if (requestId === coreVersionRequestId) coreVersionBusy.value = false
  }
}

async function load() {
  const instanceName = instance.value?.name
  if (!instanceName) {
    loadRequestId += 1
    details.value = undefined
    detailsError.value = ''
    loading.value = false
    return
  }
  const directory = currentDirectory.value
  const requestId = ++loadRequestId
  loading.value = true
  detailsError.value = ''
  try {
    const result = await bridge.serverDetails(instanceName, directory)
    if (requestId !== loadRequestId || instance.value?.name !== instanceName || currentDirectory.value !== directory) return
    syncDetails(result)
  } catch (error) {
    if (requestId !== loadRequestId || instance.value?.name !== instanceName || currentDirectory.value !== directory) return
    detailsError.value = errorMessage(error)
    if (details.value) emit('toast', detailsError.value)
  } finally {
    if (requestId === loadRequestId) loading.value = false
  }
}

async function refreshStatus() {
  if (!instance.value || statusBusy.value) return
  statusBusy.value = true
  try {
    const status = await bridge.serverStatus(instance.value.name)
    if (details.value) details.value.status = status
    recordMetric(status.metrics)
  } catch {
    // the dashboard is best effort while the backend is restarting.
  } finally {
    statusBusy.value = false
  }
}

async function refreshLogs() {
  if (!instance.value || logBusy.value) return
  logBusy.value = true
  try {
    const value = await bridge.serverLog(instance.value.name, logCursor.value)
    logs.value = value.reset || !logCursor.value ? (value.lines || []).slice(-1000) : [...logs.value, ...(value.lines || [])].slice(-1000)
    logCursor.value = value.cursor ?? logCursor.value
    if (details.value) details.value.status.running = value.running
  } catch {
    // the status poll remains the fallback.
  } finally {
    logBusy.value = false
  }
}

async function start() {
  if (!instance.value || actionBusy.value || running.value) return
  actionBusy.value = true
  activeAction.value = 'start'
  launchError.value = ''
  try {
    if (!details.value?.config.eula_accepted) throw new Error('Сначала примите EULA в настройках сервера')
    const status = await bridge.startServer(instance.value.name)
    if (details.value) details.value.status = status
    recordMetric(status.metrics)
    launchError.value = status.running ? '' : status.start_error || 'Процесс уже завершился. Причина и код завершения доступны в консоли.'
    await refreshLogs()
    emit('toast', status.running ? 'Сервер запускается — следите за консолью' : launchError.value)
    emit('changed')
  } catch (error) {
    launchError.value = errorMessage(error)
    await refreshLogs()
    emit('toast', launchError.value)
  } finally {
    actionBusy.value = false
    activeAction.value = ''
  }
}

async function stop() {
  if (!instance.value || actionBusy.value || !running.value) return
  actionBusy.value = true
  activeAction.value = 'stop'
  try {
    if (props.confirmDestructive !== false && !(await confirmDialog('Остановить сервер?', { danger: true, confirmLabel: 'Остановить' }))) return
    if (details.value) {
      details.value.status = await bridge.stopServer(instance.value.name)
      recordMetric(details.value.status.metrics)
    }
    emit('changed')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    actionBusy.value = false
    activeAction.value = ''
  }
}

async function restart() {
  if (!instance.value || actionBusy.value || !running.value) return
  actionBusy.value = true
  activeAction.value = 'restart'
  launchError.value = ''
  try {
    if (props.confirmDestructive !== false && !(await confirmDialog('Перезапустить сервер? Игроки будут отключены; лаунчер сначала отправит stop и дождётся завершения. Созданный через лаунчер UPnP-проброс будет закрыт — откройте его снова после запуска.', { danger: true, confirmLabel: 'Перезапустить' }))) return
    const status = await bridge.restartServer(instance.value.name)
    if (details.value) details.value.status = status
    metricHistory.value = []
    recordMetric(status.metrics)
    await refreshLogs()
    emit('toast', status.running ? 'Сервер перезапускается — следите за консолью' : 'Процесс завершился; проверьте консоль')
    emit('changed')
  } catch (error) {
    launchError.value = errorMessage(error)
    await refreshLogs()
    emit('toast', launchError.value)
  } finally {
    actionBusy.value = false
    activeAction.value = ''
  }
}

async function sendCommand() {
  if (!instance.value || !command.value.trim()) return
  try {
    if (details.value) details.value.status = await bridge.serverCommand(instance.value.name, command.value)
    command.value = ''
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

async function sendQuickCommand(value: string) {
  if (!instance.value || !running.value) return
  try {
    if (details.value) details.value.status = await bridge.serverCommand(instance.value.name, value)
    emit('toast', `Отправлено: ${value}`)
    await refreshLogs()
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

async function openFile(file: ServerFile) {
  if (file.kind !== 'file' || !file.editable || !instance.value) return
  try {
    selectedFile.value = file
    editorContent.value = (await bridge.serverFileRead(instance.value.name, file.path)).content
    editorDirty.value = false
    selectTab('editor')
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

async function saveFile() {
  if (!instance.value || !selectedFile.value || running.value) return
  try {
    await bridge.serverFileWrite(instance.value.name, selectedFile.value.path, editorContent.value)
    editorDirty.value = false
    emit('toast', 'Файл сохранён')
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

async function chooseServerFile(core = false) {
  if (!instance.value || running.value) return
  const file = await pickFile(core ? { accept: '.jar', title: 'Ядро сервера (server.jar)' } : { title: 'Загрузка файла на сервер' })
  if (!file) return
  try {
    if (core) {
      await uploadCoreFile(file)
    } else {
      await bridge.serverFileUpload(instance.value.name, `${currentDirectory.value ? `${currentDirectory.value}/` : ''}${file.fileName}`, file.data)
      await load()
      emit('toast', 'Файл загружен')
    }
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

async function uploadCoreFile(file: { fileName: string; data: string; size: number }) {
  if (!instance.value || running.value) return
  if (!file.fileName.toLowerCase().endsWith('.jar')) throw new Error('Загрузите файл ядра с расширением .jar')
  if (file.size > 512 * 1024 * 1024) throw new Error('Файл ядра превышает лимит 512 MiB')
  await bridge.uploadServerCore(instance.value.name, file.fileName, file.data)
  await load()
  emit('toast', 'Ядро загружено')
}

function onCoreDragEnter(event: DragEvent) {
  if (!Array.from(event.dataTransfer?.types || []).includes('Files')) return
  coreDragDepth += 1
  coreDropActive.value = true
}

function onCoreDragLeave() {
  coreDragDepth = Math.max(0, coreDragDepth - 1)
  if (!coreDragDepth) coreDropActive.value = false
}

function onCoreDrop(event: DragEvent) {
  coreDragDepth = 0
  coreDropActive.value = false
  const file = event.dataTransfer?.files[0]
  if (!file) return
  // drag-and-drop остаётся быстрым путём: File читается прямо здесь.
  void file.arrayBuffer().then((buffer) => {
    let binary = ''
    const bytes = new Uint8Array(buffer)
    for (let offset = 0; offset < bytes.length; offset += 0x8000) {
      binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000))
    }
    return { fileName: file.name, data: btoa(binary), size: file.size }
  }).then((picked) => uploadCoreFile(picked)).catch((error) => emit('toast', errorMessage(error)))
}

async function downloadCore() {
  if (!instance.value || running.value || coreDownloadBusy.value) return
  coreDownloadBusy.value = true
  try {
    let download: { sha256?: string; trusted_hash?: boolean } | undefined
    try {
      download = (await bridge.downloadServerCore(instance.value.name, details.value?.config.core, details.value?.config.version, details.value?.config.build)).download
    } catch (error) {
      if (!errorMessage(error).includes('[unverified-download]') || !(await confirmDialog('Источник ядра не дал доверенный хеш. Скачать файл и показать его SHA-256 после загрузки?', { danger: true, confirmLabel: 'Скачать' }))) throw error
      download = (await bridge.downloadServerCore(instance.value.name, details.value?.config.core, details.value?.config.version, details.value?.config.build, true)).download
    }
    await load()
    emit('toast', `Ядро загружено · SHA-256 ${download?.sha256 || 'неизвестен'}`)
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    coreDownloadBusy.value = false
  }
}

function enterDirectory(file: ServerFile) {
  if (file.kind !== 'directory') return
  currentDirectory.value = file.path
  selectedFile.value = undefined
  editorContent.value = ''
  void load()
}

function parentDirectory() {
  const parts = currentDirectory.value.split('/').filter(Boolean)
  parts.pop()
  currentDirectory.value = parts.join('/')
  selectedFile.value = undefined
  void load()
}

async function searchPlugins() {
  if (!instance.value) return
  pluginBusy.value = true
  try {
    pluginResults.value = (await bridge.serverPluginSearch(instance.value.name, pluginQuery.value, pluginCatalogId.value || undefined, pluginSortMode.value)).results
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    pluginBusy.value = false
  }
}

async function openPluginDetails(plugin: ServerPluginResult) {
  if (!instance.value) return
  const requestId = ++pluginDetailsRequestId
  selectedPluginDetails.value = plugin
  pluginDetails.value = { ...plugin }
  pluginDetailsError.value = ''
  pluginDetailsLoading.value = true
  pluginDetailsOpen.value = true
  try {
    const result = await bridge.serverPluginDetails(instance.value.name, plugin.project_id, pluginCatalogId.value || undefined)
    if (requestId === pluginDetailsRequestId) pluginDetails.value = result.details
  } catch (error) {
    if (requestId === pluginDetailsRequestId) pluginDetailsError.value = errorMessage(error)
  } finally {
    if (requestId === pluginDetailsRequestId) pluginDetailsLoading.value = false
  }
}

function closePluginDetails() {
  pluginDetailsRequestId += 1
  pluginDetailsOpen.value = false
  pluginDetailsLoading.value = false
}

function installPluginFromDetails() {
  const plugin = selectedPluginDetails.value
  closePluginDetails()
  if (plugin) void installPlugin(plugin)
}

async function checkPluginUpdates() {
  if (!instance.value || pluginUpdatesBusy.value) return
  pluginUpdatesBusy.value = true
  try {
    const result = await bridge.serverPluginUpdates(instance.value.name)
    pluginUpdates.value = result.updates
    emit('toast', result.updates.length ? `Доступны обновления плагинов: ${result.updates.length}` : 'Все плагины уже обновлены')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    pluginUpdatesBusy.value = false
  }
}

async function updateServerPlugin(update: ServerPluginUpdate) {
  if (!instance.value || running.value || pluginUpdatesBusy.value) return
  pluginUpdatesBusy.value = true
  try {
    let result: Awaited<ReturnType<typeof bridge.serverPluginUpdate>>
    try {
      result = await bridge.serverPluginUpdate(instance.value.name, update.file_name, update.catalog_id)
    } catch (error) {
      if (!errorMessage(error).includes('[unverified-download]') || !(await confirmDialog('Каталог обновления не дал доверенный хеш. Скачать файл и показать его SHA-256 после загрузки?', { danger: true, confirmLabel: 'Обновить' }))) throw error
      result = await bridge.serverPluginUpdate(instance.value.name, update.file_name, update.catalog_id, true)
    }
    pluginUpdates.value = pluginUpdates.value.filter((item) => item.file_name !== update.file_name)
    await load()
    emit('toast', `Плагин обновлён · SHA-256 ${result.plugin.sha256 || 'неизвестен'}`)
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    pluginUpdatesBusy.value = false
  }
}

async function installPlugin(plugin: ServerPluginResult) {
  if (!instance.value || running.value || pluginBusy.value) return
  pluginBusy.value = true
  try {
    let installed: { sha256?: string; file_name?: string } | undefined
    try {
      installed = (await bridge.serverPluginInstall(instance.value.name, plugin.project_id, pluginCatalogId.value || undefined)).plugin
    } catch (error) {
      if (!errorMessage(error).includes('[unverified-download]') || !(await confirmDialog(`Каталог не дал доверенный хеш для «${plugin.title}». Скачать файл и показать его SHA-256 после загрузки?`, { danger: true, confirmLabel: 'Скачать' }))) throw error
      installed = (await bridge.serverPluginInstall(instance.value.name, plugin.project_id, pluginCatalogId.value || undefined, true)).plugin
    }
    pluginResults.value = pluginResults.value.map((item) => item.project_id === plugin.project_id
      ? { ...item, installed: true, file_name: installed?.file_name || item.file_name }
      : item)
    await load()
    emit('toast', `Плагин «${plugin.title}» установлен · SHA-256 ${installed?.sha256 || 'неизвестен'}`)
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    pluginBusy.value = false
  }
}

async function removePlugin(file: Pick<ServerFile, 'name'>) {
  if (!instance.value || running.value || pluginBusy.value || props.confirmDestructive !== false && !(await confirmDialog(`Удалить «${file.name}»?`, { danger: true, confirmLabel: 'Удалить' }))) return
  pluginBusy.value = true
  try {
    await bridge.serverPluginRemove(instance.value.name, file.name)
    pluginResults.value = pluginResults.value.map((plugin) => plugin.file_name === file.name ? { ...plugin, installed: false } : plugin)
    await load()
    emit('toast', 'Плагин удалён')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    pluginBusy.value = false
  }
}

const modAuditDialog = ref<InstanceType<typeof ModAuditDialog> | null>(null)

function auditPlugin(file: Pick<ServerFile, 'name' | 'path'>) {
  if (!instance.value) return
  modAuditDialog.value?.start({ instanceName: instance.value.name, path: file.path, fileName: file.name })
}

function removeCatalogPlugin(plugin: ServerPluginResult) {
  if (plugin.file_name) void removePlugin({ name: plugin.file_name })
}

async function createSnapshot() {
  const target = instance.value
  if (!target) return
  snapshotBusy.value = true
  try {
    const result = await bridge.serverSnapshotCreate(target.name, snapshotCategory.value)
    if (details.value) details.value.snapshots[snapshotCategory.value] = result.snapshots
    await props.snapshotCreated?.({
      id: target.name,
      name: target.display_name || target.name,
      version: target.minecraft_version || target.version_id || undefined,
      loader: target.loader,
    }, result.snapshot)
    emit('toast', 'Снапшот создан')
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    snapshotBusy.value = false
  }
}

async function restoreSnapshot(snapshot: ServerSnapshot) {
  if (!instance.value || running.value) return
  try {
    const result = await bridge.serverSnapshotRestore(instance.value.name, snapshotCategory.value, snapshot.id)
    if (details.value) {
      details.value.snapshots[snapshotCategory.value] = result.snapshots
      details.value.files = result.files
    }
    emit('toast', 'Снапшот восстановлен')
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

async function deleteSnapshot(snapshot: ServerSnapshot) {
  if (!instance.value || props.confirmDestructive !== false && !(await confirmDialog('Удалить снапшот?', { danger: true, confirmLabel: 'Удалить' }))) return
  try {
    const result = await bridge.serverSnapshotDelete(instance.value.name, snapshotCategory.value, snapshot.id)
    if (details.value) details.value.snapshots[snapshotCategory.value] = result.snapshots
  } catch (error) {
    emit('toast', errorMessage(error))
  }
}

async function saveSettings() {
  if (!instance.value) return
  if (!Number.isInteger(draft.port) || draft.port < 1 || draft.port > 65535) {
    settingsSaveState.value = 'error'
    settingsSaveMessage.value = 'Порт должен быть целым числом от 1 до 65535.'
    emit('toast', settingsSaveMessage.value)
    return
  }
  if (!Number.isInteger(draft.max_players) || draft.max_players < 1 || draft.max_players > 100000) {
    settingsSaveState.value = 'error'
    settingsSaveMessage.value = 'Максимум игроков должен быть целым числом от 1 до 100000.'
    emit('toast', settingsSaveMessage.value)
    return
  }
  settingsSaveState.value = 'saving'
  settingsSaveMessage.value = 'Сохраняю изменения…'
  try {
    const result = await bridge.saveServerConfig(instance.value.name, { ...draft })
    if (details.value) {
      details.value.config = result.config
      details.value.server = result.server
    }
    settingsSaveState.value = 'saved'
    settingsSaveMessage.value = 'Изменения сохранены'
    emit('changed')
    emit('toast', 'Настройки сервера сохранены')
  } catch (error) {
    settingsSaveState.value = 'error'
    settingsSaveMessage.value = errorMessage(error)
    emit('toast', errorMessage(error))
  }
}

function openSettings() {
  selectTab('settings')
}

watch(() => instance.value?.name, () => {
  loadRequestId += 1
  details.value = undefined
  detailsError.value = ''
  selectTab(props.settingsOnly ? 'settings' : 'overview')
  currentDirectory.value = ''
  selectedFile.value = undefined
  pluginResults.value = []
  pluginUpdates.value = []
  closePluginDetails()
  logCursor.value = 0
  metricHistory.value = []
  launchError.value = ''
  void load()
  void loadProfileGroups()
}, { immediate: true })

watch(() => props.focusTab, (value) => {
  if (value) selectTab(value)
})

watch(() => props.settingsOnly, (value) => {
  if (value) selectTab('settings')
})

watch(tab, (value) => {
  if (value !== 'overview' && value !== 'console') return
  void refreshStatus()
  void refreshLogs()
})

async function loadProfileGroups() {
  try {
    profileGroups.value = (await bridge.instanceGroups()).groups || []
  } catch {
    // groups are optional profile metadata.
  }
}

watch(memoryUnit, (next, previous) => {
  if (syncingSettings.value) return
  const oldUnitKiB = previous === 'GiB' ? 1024 * 1024 : 1024
  const nextUnitKiB = next === 'GiB' ? 1024 * 1024 : 1024
  const minimum = Math.min(memoryMinimum.value, memoryMaximum.value)
  memoryValue.value = Math.min(memoryMaximum.value, Math.max(minimum, memoryValue.value * oldUnitKiB / nextUnitKiB))
})

watch([memoryValue, memoryUnit], () => {
  draft.memory = memorySetting.value
})

watch(draft, () => {
  if (!syncingSettings.value && settingsSaveState.value !== 'saving') {
    settingsSaveState.value = 'dirty'
    settingsSaveMessage.value = 'Есть несохранённые изменения'
  }
}, { deep: true })

watch(() => draft.core, (value, previous) => {
  if (value && previous && value !== previous) void refreshCoreVersions(value)
})

onMounted(() => {
  void refreshStatus()
  void refreshLogs()
  refreshTimer = window.setInterval(() => {
    if (!document.hidden && (tab.value === 'overview' || tab.value === 'console')) void refreshStatus()
  }, 5000)
  logTimer = window.setInterval(() => {
    if (!document.hidden && (tab.value === 'overview' || tab.value === 'console') && (details.value?.status.running || actionBusy.value)) void refreshLogs()
  }, 1000)
})

onBeforeUnmount(() => {
  if (refreshTimer) window.clearInterval(refreshTimer)
  if (logTimer) window.clearInterval(logTimer)
  if (profileIconPreview.value.startsWith('blob:')) URL.revokeObjectURL(profileIconPreview.value)
  if (profileBannerPreview.value.startsWith('blob:')) URL.revokeObjectURL(profileBannerPreview.value)
})

defineExpose({ openSettings, stop })
</script>

<template>
   <section v-if="instance" class="server-workspace">
    <template v-if="!settingsOnly">
     <header class="server-header">
      <div>
        <span class="eyebrow">SERVER WORKSPACE</span>
        <h2>{{ instance.display_name || instance.name }}</h2>
         <p><span class="server-status-dot" :class="{ 'server-status-dot--online': running }" /><CoreBadge :core="coreLabel" compact />{{ running ? 'Процесс работает' : 'Остановлен' }}<span v-if="running" class="server-uptime-label"> · В работе {{ formatUptime(details?.status.uptime_seconds || 0) }}</span> · {{ coreLabel }} · {{ details?.config.version || instance.server_core_version || 'версия не указана' }} · {{ instance.name }}</p>
     </div>
      <div class="server-header-actions">
         <button v-if="running" class="button button--outline" type="button" :disabled="actionBusy" @click="stop"><Icon name="stop" :size="14" />{{ activeAction === 'stop' ? 'Останавливаю…' : 'Остановить' }}</button>
         <button v-else class="button button--accent" type="button" :disabled="actionBusy" @click="start"><Icon name="play" :size="14" />{{ activeAction === 'start' ? 'Запускаю…' : 'Запустить' }}</button>
          <button v-if="running" class="button button--accent" type="button" :disabled="actionBusy" @click="restart"><Icon name="refresh" :size="14" />{{ activeAction === 'restart' ? 'Перезапускаю…' : 'Рестарт' }}</button>
          <button class="button button--small button--quiet" type="button" :disabled="running || coreDownloadBusy || actionBusy" @click="downloadCore"><Icon name="download" :size="14" />{{ coreDownloadBusy ? 'Загрузка…' : 'Скачать core' }}</button>
       </div>
    </header>
    <nav class="workspace-tabs workspace-tabs--primary" role="tablist" aria-label="Разделы сервера">
      <button v-for="item in tabGroups" :key="item.id" class="workspace-tab" :class="{ 'workspace-tab--active': activeGroup === item.id }" type="button" role="tab" :aria-selected="activeGroup === item.id" @click="selectGroup(item.id)">{{ item.label }}</button>
    </nav>
     <nav class="workspace-tabs workspace-tabs--secondary" role="tablist" aria-label="Подразделы сервера">
       <button v-for="item in visibleTabs" :key="item.id" class="workspace-tab" :class="{ 'workspace-tab--active': tab === item.id }" type="button" role="tab" :aria-selected="tab === item.id" @click="selectTab(item.id)">{{ item.label }}<small v-if="item.id === 'plugins'">{{ details?.files.filter((file) => file.path.startsWith('plugins/')).length || 0 }}</small></button>
     </nav>
    </template>
      <div v-if="loading && !details" class="workspace-loading" role="status">Читаю состояние сервера…</div>
      <div v-else-if="!details && detailsError" class="workspace-load-error" role="alert"><div><strong>Не удалось загрузить сервер</strong><span>{{ detailsError }}</span></div><button class="button button--small button--outline" type="button" :disabled="loading" @click="load">{{ loading ? 'Повторяю…' : 'Повторить' }}</button></div>
      <div v-else-if="details" class="server-content">
       <div v-if="detailsError" class="workspace-load-error workspace-load-error--inline" role="alert"><div><strong>Не удалось обновить данные</strong><span>{{ detailsError }}</span></div><button class="button button--small button--outline" type="button" :disabled="loading" @click="load">{{ loading ? 'Повторяю…' : 'Повторить' }}</button></div>
      <div v-if="launchError || details.status.start_error" class="server-launch-error" role="alert"><strong>Не удалось запустить сервер</strong><span>{{ launchError || details.status.start_error }}</span><small>Проверьте Java, EULA, наличие ядра и выбранный профиль изоляции. Подробности — в консоли; профиль безопасности автоматически не ослабляется.</small></div>
      <div v-if="details.status.crash" class="crash-banner" role="alert">
        <div class="crash-banner-head">
          <span class="crash-banner-icon"><Icon name="warning" :size="18" /></span>
          <div>
            <strong>{{ details.status.crash.title }}</strong>
            <small v-if="details.status.exit_code">Код выхода: {{ details.status.exit_code }}</small>
          </div>
        </div>
        <ul class="crash-banner-hints">
          <li v-for="(hint, index) in details.status.crash.hints" :key="index"><strong>{{ hint.title }}</strong><small>{{ hint.detail }}</small></li>
        </ul>
        <Collapsible class="crash-banner-log" title="Выдержка лога">
          <pre>{{ details.status.crash.excerpt.join('\n') }}</pre>
        </Collapsible>
      </div>
      <ServerAccessPanel v-if="tab === 'players'" :key="instance.name" :name="instance.name" :running="running" @toast="emit('toast', $event)" />
      <ServerNetworkPanel v-if="tab === 'network'" :key="instance.name" :name="instance.name" :port="details.config.port" :running="running" :core="coreLabel" @toast="emit('toast', $event)" />
      <div v-else-if="tab === 'overview'" class="server-overview">
          :name="instance.name"
          :display-name="instance.display_name || instance.name"
          :running="running"
          :busy="actionBusy"
          :active-action="activeAction"
          :port="details.config.port"
          :players="details.status.players"
          :max-players="details.status.max_players"
          :plugin-files="details.plugin_files || []"
          @start="start"
          @stop="stop"
          @restart="restart"
          @command="sendQuickCommand"
          @toast="emit('toast', $event)"
        />
        <div class="server-metric-grid">
           <div class="server-metric"><span>CPU</span><strong>{{ details.status.metrics.cpu_percent.toFixed(1) }}%</strong><svg viewBox="0 0 100 42" preserveAspectRatio="none" aria-label="График CPU"><polyline :points="metricPoints('cpu_percent')" /></svg><small>пиковое {{ (details.status.metrics.peak_cpu_percent || 0).toFixed(1) }}%</small></div>
           <div class="server-metric"><span>RAM</span><strong>{{ formatBytes(details.status.metrics.ram_bytes) }}</strong><svg viewBox="0 0 100 42" preserveAspectRatio="none" aria-label="График RAM"><polyline :points="metricPoints('ram_bytes')" /></svg><small>пик {{ formatBytes(details.status.metrics.peak_ram_bytes || 0) }}</small></div>
           <div class="server-metric"><span>Игроки</span><strong>{{ details.status.players }} / {{ details.status.max_players }}</strong><svg viewBox="0 0 100 42" preserveAspectRatio="none" aria-label="График TPS"><polyline :points="metricPoints('tps')" /></svg><small>TPS {{ details.status.tps || '—' }}</small></div>
          <div class="server-metric"><span>Время работы</span><strong>{{ formatUptime(details.status.uptime_seconds) }}</strong><small>storage {{ formatBytes(details.status.metrics.storage_bytes) }}</small></div>
        </div>
        <div class="server-traffic-grid">
          <div class="server-metric"><span>Входящий трафик · RX</span><strong>{{ details.status.metrics.network_available && details.status.metrics.network_rx_bps != null ? formatBytes(details.status.metrics.network_rx_bps) + '/с' : '—' }}</strong><svg v-if="details.status.metrics.network_available" viewBox="0 0 100 42" preserveAspectRatio="none" aria-label="График входящего трафика сервера"><polyline :points="metricPoints('network_rx_bps')" /></svg><small>Наблюдалось за запуск: {{ formatBytes(details.status.metrics.network_rx_bytes || 0) }}</small></div>
          <div class="server-metric server-metric--tx"><span>Исходящий трафик · TX</span><strong>{{ details.status.metrics.network_available && details.status.metrics.network_tx_bps != null ? formatBytes(details.status.metrics.network_tx_bps) + '/с' : '—' }}</strong><svg v-if="details.status.metrics.network_available" viewBox="0 0 100 42" preserveAspectRatio="none" aria-label="График исходящего трафика сервера"><polyline :points="metricPoints('network_tx_bps')" /></svg><small>Наблюдалось за запуск: {{ formatBytes(details.status.metrics.network_tx_bytes || 0) }}</small></div>
        </div>
        <p class="server-traffic-note">{{ details.status.metrics.network_available ? `Только трафик этого сервера: TCP payload игрового порта и сокетов процесса (${details.status.metrics.network_source === 'windows-tcp-estats' ? 'Windows: TCP ESTATS' : 'Linux: inet-diag'}). Короткие соединения между выборками могут быть пропущены; это наблюдаемые данные, не полный сетевой биллинг.` : details.status.metrics.network_error || (running ? 'Ожидание измерений трафика…' : 'Измерение начнётся после запуска сервера.') }}</p>
        <ServerConsole v-model:command="command" :logs="logs" :running="running" :busy="actionBusy" @send="sendCommand" />
        <section class="workspace-card"><div class="workspace-card-heading"><div><h3>Сервер для друзей</h3><span class="muted">Откройте игровой TCP-порт через белый IPv4 и UPnP или настройте ручной проброс.</span></div><button class="button button--outline" type="button" @click="selectTab('network')"><Icon name="globe" :size="15" />Сеть и белый IP</button></div></section>
        <div class="server-columns">
           <section class="workspace-card">
             <div class="workspace-card-heading"><div><h3>Ядро и запуск</h3><span class="muted">server.jar в корне экземпляра</span></div><span class="tag" :class="{ 'tag--success': details.server.server_eula }">{{ details.server.server_eula ? 'EULA принята' : 'EULA нужна' }}</span></div>
             <p class="server-copy">{{ details.config.motd || 'Без MOTD' }}</p>
             <div class="server-meta-list"><span><b>Порт</b>{{ details.config.port }}</span><span><b>Память</b>{{ details.config.memory }}</span><span><b>Java</b>{{ details.config.java_path || 'глобальная' }}</span></div>
             <div class="server-core-drop" :class="{ 'server-core-drop--active': coreDropActive, 'server-core-drop--disabled': running }" :aria-disabled="running" @dragenter.prevent="onCoreDragEnter" @dragover.prevent="coreDropActive = true" @dragleave.prevent="onCoreDragLeave" @drop.prevent="onCoreDrop">
               <Icon name="upload" :size="17" />
               <span><strong>{{ coreDropActive ? 'Отпустите JAR, чтобы заменить ядро' : 'Заменить ядро' }}</strong><small>{{ running ? 'Сначала остановите сервер.' : 'Перетащите server.jar сюда или выберите файл.' }}</small></span>
               <button class="button button--small button--outline" type="button" :disabled="running" @click="chooseServerFile(true)">Выбрать JAR</button>
               
             </div>
             <button class="button button--small button--quiet" type="button" @click="selectTab('settings')">Открыть конфигурацию</button>
           </section>
          <section class="workspace-card"><div class="workspace-card-heading"><div><h3>Последний запуск</h3><span class="muted">Сохраняется локально</span></div></div><div v-if="details.status.last_metrics && Object.keys(details.status.last_metrics).length" class="server-last-run"><strong>{{ formatUptime(Number(details.status.last_metrics.uptime_seconds || 0)) }}</strong><span>CPU peak {{ Number(details.status.last_metrics.peak_cpu_percent || 0).toFixed(1) }}% · RAM peak {{ formatBytes(Number(details.status.last_metrics.peak_ram_bytes || 0)) }}</span><small>{{ formatDate(Number(details.status.last_metrics.ended_at || details.status.last_metrics.timestamp || 0)) }}</small></div><span v-else class="muted workspace-empty">Сервер ещё не запускался.</span></section>
        </div>
      </div>
      <ServerConsole v-else-if="tab === 'console'" v-model:command="command" :logs="logs" :running="running" :busy="actionBusy" expanded @send="sendCommand" />
      <section v-else-if="tab === 'files'" class="workspace-card server-files"><div class="workspace-card-heading"><div><h3>Файлы сервера</h3><span class="muted">/{{ currentDirectory || 'root' }}</span></div><div class="workspace-card-actions"><button v-if="currentDirectory" class="button button--small button--quiet" type="button" @click="parentDirectory">Вверх</button><button class="button button--small button--outline" type="button" @click="chooseServerFile(false)"><Icon name="upload" :size="14" />Загрузить</button></div></div><div class="server-file-list"><button v-for="file in files" :key="file.path" class="server-file-row" type="button" @click="file.kind === 'directory' ? enterDirectory(file) : openFile(file)"><Icon :name="file.kind === 'directory' ? 'folder' : 'file'" :size="15" /><span><strong>{{ file.name }}</strong><small>{{ file.kind === 'directory' ? 'Папка' : formatBytes(file.size) }}</small></span><Icon v-if="file.kind === 'file' && file.editable" name="edit" :size="14" /></button><span v-if="!files.length" class="muted workspace-empty">В этой папке нет файлов.</span></div></section>
      <section v-else-if="tab === 'plugins'" class="workspace-card">
        <div class="workspace-card-heading">
          <div><h3>Плагины сервера</h3><span class="muted">Установка из Modrinth, Spigot, Hangar или custom API</span></div>
          <div class="workspace-card-actions"><button class="button button--small button--quiet" type="button" :disabled="pluginUpdatesBusy || running" @click="checkPluginUpdates"><Icon name="refresh" :size="14" />{{ pluginUpdatesBusy ? 'Проверяю…' : 'Проверить обновления' }}</button><button class="button button--small button--quiet" type="button" @click="tab = 'snapshots'">Сделать снапшот</button></div>
        </div>
        <div class="server-plugin-search"><input v-model="pluginQuery" placeholder="Поиск плагина" @keyup.enter="searchPlugins" /><CustomSelect v-model="pluginCatalogId" :options="catalogOptions" /><CustomSelect v-model="pluginSortMode" :options="pluginSortOptions" /><button class="button button--small button--accent" type="button" :disabled="pluginBusy" @click="searchPlugins">{{ pluginBusy ? 'Ищу…' : 'Найти' }}</button></div>
        <div v-if="pluginUpdates.length" class="server-plugin-results server-plugin-updates"><article v-for="update in pluginUpdates" :key="`${update.source}-${update.file_name}`" class="server-plugin-result"><div><strong>{{ update.file_name }}</strong><small>{{ update.catalog_name || update.source }} · {{ update.current_version }} → {{ update.version }}</small></div><button class="button button--small button--accent" type="button" :disabled="pluginUpdatesBusy || running" @click="updateServerPlugin(update)"><Icon name="refresh" :size="13" />{{ pluginUpdatesBusy ? 'Обновляю…' : 'Обновить' }}</button></article></div>
        <div class="server-installed-plugins"><div v-for="file in details.plugin_files || []" :key="file.path" class="server-plugin-row"><span><Icon name="plug" :size="15" /><strong>{{ file.name }}</strong><small>{{ formatBytes(file.size) }}</small></span><span class="server-plugin-row-actions"><button class="icon-button" type="button" aria-label="Проверить безопасность плагина" v-tooltip="'Проверить безопасность'" :disabled="running" @click="auditPlugin(file)"><Icon name="shield" :size="15" /></button><button class="icon-button" type="button" aria-label="Удалить плагин" :disabled="running" @click="removePlugin(file)"><Icon name="trash" :size="15" /></button></span></div></div>
        <div v-if="pluginResults.length" class="server-plugin-results"><article v-for="plugin in pluginResults" :key="`${plugin.source}-${plugin.project_id}`" class="server-plugin-result"><div><strong>{{ plugin.title }}</strong><small>{{ plugin.author || plugin.source }} · {{ plugin.downloads.toLocaleString() }} загрузок</small><p>{{ plugin.description || 'Описание не предоставлено.' }}</p></div><span class="server-plugin-result-actions"><button class="button button--small button--quiet" type="button" :disabled="pluginBusy" @click="openPluginDetails(plugin)"><Icon name="info" :size="14" />Информация</button><button class="button button--small" :class="plugin.installed ? 'button--danger' : 'button--accent'" type="button" :disabled="pluginBusy || running" @click="plugin.installed ? removeCatalogPlugin(plugin) : installPlugin(plugin)"><Icon :name="plugin.installed ? 'trash' : 'download'" :size="14" />{{ plugin.installed ? 'Удалить' : 'Установить' }}</button></span></article></div><span v-else class="muted workspace-empty">Введите запрос, чтобы найти плагины.</span>
        <CatalogDetailsDialog v-if="pluginDetailsOpen" :details="pluginDetails" :versions="[]" project-type="plugin" :catalog-name="catalogOptions.find((item) => item.value === pluginCatalogId)?.label || 'Каталог плагинов'" :loading="pluginDetailsLoading" :error="pluginDetailsError" :can-install="Boolean(instance) && !running" :busy="pluginBusy || pluginUpdatesBusy" @close="closePluginDetails" @install="installPluginFromDetails" />
      </section>
       <section v-else-if="tab === 'snapshots'" class="workspace-card"><div class="workspace-card-heading"><div><h3>Снапшоты ресурсов</h3><span class="muted">Авто-retention {{ details.config.snapshot_retention_hours }} ч · максимум {{ details.config.snapshot_max_count }}</span></div><div class="workspace-card-actions"><CustomSelect v-model="snapshotCategory" :options="[{ value: 'plugins', label: 'Плагины' }, { value: 'mods', label: 'Моды' }]" /><button class="button button--small button--accent" type="button" :disabled="snapshotBusy" @click="createSnapshot">Создать</button></div></div><div class="server-snapshot-list"><article v-for="snapshot in snapshots" :key="snapshot.id" class="server-snapshot-row"><span><strong>{{ snapshot.id }}</strong><small>{{ snapshot.file_count }} файлов · {{ formatBytes(snapshot.source_bytes) }} · {{ formatDate(snapshot.created_at) }}</small></span><span><button class="button button--small button--quiet" type="button" :disabled="running || snapshotBusy" @click="restoreSnapshot(snapshot)">Восстановить</button><button class="icon-button" type="button" aria-label="Удалить снапшот" @click="deleteSnapshot(snapshot)"><Icon name="trash" :size="14" /></button></span></article><span v-if="!snapshots.length" class="muted workspace-empty">Снапшотов пока нет.</span></div></section>
       <section v-else-if="tab === 'settings'" class="workspace-card server-config-card"><div class="workspace-card-heading"><div><h3>Настройки сервера</h3><span class="muted">Изменения сохраняются в .nivora-launcher-server.json</span></div><button class="button button--small button--accent" type="button" :disabled="settingsSaveState === 'saving'" @click="saveSettings">{{ settingsSaveState === 'saving' ? 'Сохраняю…' : 'Сохранить' }}</button></div><div class="workspace-form-grid"><label class="field"><span>Ядро</span><CustomSelect v-model="draft.core" :options="coreOptions" /></label><label class="field"><span>Версия Minecraft</span><CustomSelect v-model="draft.version" :options="coreVersionOptions" :disabled="coreVersionBusy" /></label><div class="field"><span>Порт</span><NumericStepper v-model="draft.port" :min="1" :max="65535" aria-label="Порт сервера" /></div><div class="field"><span>Максимум игроков</span><NumericStepper v-model="draft.max_players" :min="1" :max="100000" aria-label="Максимум игроков" /></div><div class="field"><span>Память JVM</span><div class="memory-input-row"><NumericStepper v-model="memoryValue" :min="memoryMinimum" :max="memoryMaximum" :step="memoryStep" :decimals="memoryDecimals" aria-label="Память сервера" /><CustomSelect v-model="memoryUnit" :options="memoryUnitOptions" /></div><small class="field-help">Лимит {{ memoryLimitLabel }} из {{ systemMemoryLabel }}; 25% RAM оставляется системе. JVM получит {{ memorySetting }}.</small></div><label class="field"><span>Java executable</span><input v-model="draft.java_path" placeholder="глобальная Java" /></label><label class="field"><span>Профиль изоляции</span><CustomSelect v-model="draft.security_profile" :options="securityProfileOptions" /></label><label class="field field--wide"><span>MOTD</span><input v-model="draft.motd" /></label><div class="field"><span>Retention, часы</span><NumericStepper v-model="draft.snapshot_retention_hours" :min="1" :max="720" aria-label="Retention снапшотов" /></div><div class="field"><span>Максимум снапшотов</span><NumericStepper v-model="draft.snapshot_max_count" :min="1" :max="50" aria-label="Максимум снапшотов" /></div><label class="workspace-check"><input v-model="draft.eula_accepted" type="checkbox" /><span><strong>Я принимаю EULA Minecraft</strong><small>Без этого запуск сервера блокируется.</small></span></label><label class="workspace-check"><input v-model="draft.auto_restart" type="checkbox" /><span><strong>Автоперезапуск</strong><small>Подготовлено для supervisor policy.</small></span></label>
<div class="field"><span>Плановый рестарт в <small>локальное время сервера</small></span><input v-model="draft.restart_schedule.time" :disabled="!draft.restart_schedule.enabled" placeholder="04:00" maxlength="5" /></div>
<div class="field"><span>Предупреждать, минут</span><NumericStepper v-model="draft.restart_schedule.warn_minutes" :min="0" :max="30" :disabled="!draft.restart_schedule.enabled" aria-label="Предупреждение о рестарте" /><small class="field-help">say в консоли перед рестартом.</small></div>
<label class="workspace-check"><input v-model="draft.restart_schedule.enabled" type="checkbox" /><span><strong>Расписание перезапуска</strong><small>Рестарт каждый день в указанное время{{ draft.restart_schedule.last_run ? `; последний ${draft.restart_schedule.last_run}` : '' }}.</small></span></label>
</div></section>
        <section v-else-if="tab === 'editor'" class="workspace-card server-editor-page">
          <div class="workspace-card-heading"><div><h3>{{ selectedFile?.path || 'Редактор файла' }}</h3><span class="muted">Редактирование файлов вынесено в отдельную вкладку.</span></div><button class="button button--small button--accent" type="button" :disabled="!selectedFile || !editorDirty || running" @click="saveFile">{{ running ? 'Недоступно во время запуска' : 'Сохранить файл' }}</button></div>
          <textarea v-if="selectedFile" v-model="editorContent" spellcheck="false" @input="editorDirty = true" />
          <div v-else class="empty-card empty-card--short"><span class="empty-orb"><Icon name="edit" :size="20" /></span><h3>Файл не выбран</h3><p>Откройте редактируемый файл во вкладке «Файлы».</p><button class="button button--outline" type="button" @click="selectTab('files')">Открыть файлы</button></div>
        </section>
        <section v-if="tab === 'settings'" class="workspace-card server-profile-card">
          <div class="settings-save-bar" :class="`settings-save-bar--${settingsSaveState}`">
            <div class="settings-save-copy"><span class="status-dot" :class="{ 'status-dot--green': settingsSaveState === 'saved', 'status-dot--busy': settingsSaveState === 'saving' }" /><div><strong>{{ settingsSaveState === 'saving' ? 'Сохраняю настройки' : settingsSaveState === 'error' ? 'Нужно исправление' : settingsSaveState === 'dirty' ? 'Есть несохранённые изменения' : 'Настройки сервера сохранены' }}</strong><small>{{ settingsSaveMessage }}</small></div></div>
            <button class="button button--small button--accent" type="button" :disabled="settingsSaveState === 'saving'" @click="saveSettings">{{ settingsSaveState === 'saving' ? 'Сохраняю…' : 'Сохранить изменения' }}</button>
          </div>
          <div class="workspace-card-heading">
           <div><h3>Профиль сервера</h3><span class="muted">Название, описание, иконка и баннер карточки.</span></div>
           <button class="button button--small button--outline" type="button" :disabled="profileSaving" @click="saveProfile">{{ profileSaving ? 'Сохраняю…' : 'Сохранить профиль' }}</button>
         </div>
         <div class="instance-profile-media">
           <div class="profile-upload profile-upload--icon">
             <img v-if="profileIconPreview" :src="profileIconPreview" alt="" />
             <span v-else class="profile-upload-placeholder"><Icon name="server" :size="22" /></span>
             <button class="button button--small button--quiet" type="button" @click="chooseProfileAsset('icon')">Иконка</button>
             <button v-if="profileIconPreview" class="text-button" type="button" @click="clearProfileAsset('icon')">Убрать</button>
           </div>
           <div class="profile-upload profile-upload--banner">
             <img v-if="profileBannerPreview" :src="profileBannerPreview" alt="" />
             <span v-else class="profile-upload-placeholder"><Icon name="image" :size="22" /></span>
             <span class="profile-upload-caption">Баннер · виден на карточке сервера</span>
             <button class="button button--small button--quiet" type="button" @click="chooseProfileAsset('banner')">Загрузить баннер</button>
             <button v-if="profileBannerPreview" class="text-button" type="button" @click="clearProfileAsset('banner')">Убрать</button>
           </div>
         </div>
         
         <div class="workspace-form-grid">
           <label class="field"><span>Отображаемое название</span><input v-model="profileDraft.display_name" maxlength="80" /></label>
           <div class="field"><span>Группа</span><CustomSelect v-model="profileDraft.group" :options="profileGroupOptions" /></div>
           <label class="field field--wide"><span>Описание</span><textarea v-model="profileDraft.description" maxlength="500" rows="3" /></label>
         </div>
       </section>
     </div>
   </section>
   <ModAuditDialog ref="modAuditDialog" @toast="(message: string) => emit('toast', message)" />
 </template>
