<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { pickFile, pickFiles } from '../services/fileBrowser'
import CustomSelect from './CustomSelect.vue'
import Icon from './Icon.vue'
import { bridge } from '../services/bridge'
import { confirmDialog, promptDialog } from '../services/dialogs'
import type { BedrockCategory, BedrockContentItem, BedrockRuntimeStatus, BedrockScreenshotItem, InstanceFile, InstanceInfo, InstanceLogArchive, InstanceLogArchiveReadResult, InstanceLog, SelectOption, ServerMetrics, ServerSnapshot } from '../types'

type WorkspaceTab = 'overview' | 'version' | 'content' | 'files' | 'console' | 'snapshots' | 'screenshots'

const props = withDefaults(defineProps<{
  instance: InstanceInfo
  busy?: boolean
  confirmDestructive?: boolean
  focusTab?: string
  fileEditorMode?: 'builtin' | 'system' | string
}>(), { busy: false, confirmDestructive: true, focusTab: '', fileEditorMode: 'builtin' })

const emit = defineEmits<{
  launch: []
  stop: []
  toast: [message: string]
  changed: []
  activity: [payload: { title: string; detail: string; done?: boolean; logs?: string[] }]
}>()

const workspaceTabs: Array<{ id: WorkspaceTab; label: string; icon: string }> = [
  { id: 'overview', label: 'Обзор', icon: 'instances' },
  { id: 'version', label: 'Версия', icon: 'download' },
  { id: 'content', label: 'Контент', icon: 'archive' },
  { id: 'console', label: 'Консоль', icon: 'activity' },
  { id: 'screenshots', label: 'Скриншоты', icon: 'image' },
  { id: 'snapshots', label: 'Снапшоты', icon: 'archive' },
  { id: 'files', label: 'Файлы', icon: 'folder' },
]
const visibleWorkspaceTabs = computed(() => workspaceTabs)
const tab = ref<WorkspaceTab>('overview')

function selectTab(value: WorkspaceTab) {
  tab.value = value
  if (value === 'console') {
    startConsolePolling()
    void loadLogArchives()
  } else {
    stopConsolePolling()
  }
  if (value === 'screenshots') void loadScreenshots()
  if (value === 'snapshots') void loadSnapshots()
}

const categories: Array<{ id: BedrockCategory; label: string }> = [
  { id: 'addons', label: 'Аддоны' },
  { id: 'resourcepacks', label: 'Ресурспаки' },
  { id: 'worlds', label: 'Миры' },
  { id: 'skins', label: 'Скины' },
]
const activeCategory = ref<BedrockCategory>('addons')
const status = ref<BedrockRuntimeStatus | null>(null)
const items = ref<BedrockContentItem[]>([])
const versions = ref<Array<{ edition: 'release' | 'preview'; version: string; installed: boolean }>>([])
const versionEdition = ref<'release' | 'preview'>('release')
const pendingInstallAfterLogin = ref(false)
const versionSelection = ref('')
const currentEdition = ref('')
const currentVersion = ref('')
const versionsLoading = ref(false)
const versionInstallBusy = ref(false)
const installLog = ref('')
const metrics = ref<ServerMetrics>()
const metricHistory = ref<ServerMetrics[]>([])
const workspaceFiles = ref<InstanceFile[]>([])
const fileDirectory = ref('')
const fileLoading = ref(false)
const fileMutationBusy = ref(false)
const editorPath = ref('')
const editorContent = ref('')
const editorOriginalContent = ref('')
const editorSaving = ref(false)
const filePreview = ref<{ path: string; source: string }>()
const loading = ref(false)
const actionBusy = ref(false)
const loginPending = ref(false)
const loginCancelBusy = ref(false)
const loginCancelRequested = ref(false)
const notice = ref('')
const visibleItems = computed(() => items.value.filter((item) => item.category === activeCategory.value))
const categoryCount = (category: BedrockCategory) => items.value.filter((item) => item.category === category).length
const editionOptions: SelectOption[] = [
  { value: 'release', label: 'Обычная версия', meta: 'Стабильный выпуск' },
  { value: 'preview', label: 'Preview', meta: 'Тестовые сборки' },
]
const visibleVersions = computed(() => versions.value.filter((item) => item.edition === versionEdition.value))
const versionOptions = computed<SelectOption[]>(() => visibleVersions.value.map((item) => ({
  value: item.version,
  label: item.version,
  meta: item.installed ? 'Установлена' : 'Будет загружена',
})))
const fileBreadcrumbs = computed(() => fileDirectory.value.split('/').filter(Boolean).map((name, index, parts) => ({
  name,
  path: parts.slice(0, index + 1).join('/'),
})))
const currentVersionLabel = computed(() => currentVersion.value
  ? `${currentEdition.value === 'preview' ? 'Preview' : 'Minecraft'} ${currentVersion.value}`
  : 'Версия ещё не выбрана')
const gameRunning = computed(() => props.instance.running || Boolean(metrics.value?.running))
/** официальная версия Bedrock для Windows из Microsoft Store */
const officialWindows = computed(() => status.value?.platform === 'windows-official' || status.value?.official === true)
/** на macOS официальный Bedrock недоступен, поэтому показываем причину */
const unsupportedPlatform = computed(() => status.value?.platform === 'unsupported')

watch([versionEdition, versions], () => {
  if (visibleVersions.value.some((item) => item.version === versionSelection.value)) return
  const current = visibleVersions.value.find((item) => item.version === currentVersion.value)
  versionSelection.value = current?.version || visibleVersions.value[0]?.version || ''
})

watch(() => props.focusTab, (value) => {
  if (value === 'overview') tab.value = 'overview'
})

async function ensureLoaded() {
  await refresh()
}

const accountStatus = ref<{ signed_in: boolean; platform: string } | null>(null)

async function refreshAccountStatus() {
  try {
    accountStatus.value = await bridge.bedrockAccountStatus(props.instance.name)
  } catch {
    accountStatus.value = null
  }
}

async function refresh() {
  loading.value = true
  try {
    const [runtime, content, account] = await Promise.all([
      bridge.bedrockRuntimeStatus(props.instance.name),
      bridge.bedrockContentList(props.instance.name),
      bridge.bedrockAccountStatus(props.instance.name).catch(() => null),
    ])
    status.value = runtime.status
    items.value = content.items
    accountStatus.value = account
    await pollMetrics()
  } catch (error) {
    notice.value = error instanceof Error ? error.message : String(error)
  } finally {
    loading.value = false
  }
}

async function pollMetrics() {
  try {
    const result = await bridge.instanceLog(props.instance.name)
    metrics.value = result.metrics
    metricHistory.value = [...metricHistory.value, result.metrics].slice(-60)
  } catch (error) {
    if (!metrics.value) notice.value = error instanceof Error ? error.message : String(error)
  }
}

async function loadVersions(refreshIndex = false) {
  versionsLoading.value = true
  try {
    const result = await bridge.bedrockVersions(props.instance.name, refreshIndex)
    versions.value = result.versions
    currentEdition.value = result.current_edition
    currentVersion.value = result.current_version
    if (result.current_edition) versionEdition.value = result.current_edition
    if (refreshIndex) notice.value = 'Список версий обновлён.'
  } catch (error) {
    notice.value = error instanceof Error ? error.message : String(error)
  } finally {
    versionsLoading.value = false
  }
}

let setupWatcherDisposed = false

async function watchSetupJob(edition: string, version: string, resumed = false) {
  let lastLogLine = ''
  let pollFailures = 0
  for (;;) {
    await new Promise((resolve) => window.setTimeout(resolve, 1500))
    if (setupWatcherDisposed) return
    let status: Awaited<ReturnType<typeof bridge.bedrockSetupStatus>>
    try {
      status = await bridge.bedrockSetupStatus(props.instance.name)
      pollFailures = 0
    } catch (error) {
      // один сбой опроса не прерывает фоновую установку
      pollFailures += 1
      if (pollFailures < 5) continue
      notice.value = `Не удалось получить статус установки: ${error instanceof Error ? error.message : String(error)}`
      emit('activity', { title: 'Установка Bedrock', detail: notice.value, done: true })
      return
    }
    if (!status.started && !status.running) {
      if (!resumed) notice.value = 'Установка не запущена.'
      return
    }
    const tail = status.logs_tail || []
    if (tail.length) {
      installLog.value = tail.join('\n')
      lastLogLine = tail[tail.length - 1]
    }
    if (!status.running && status.finished) {
      if (status.cancelled) {
        notice.value = 'Установка остановлена.'
        emit('activity', { title: 'Установка Bedrock', detail: 'Установка остановлена.', done: true, logs: tail })
        return
      }
      if (status.error) {
        notice.value = `Версия не установлена: ${status.error}`
        emit('activity', { title: 'Установка Bedrock', detail: `Версия не установлена: ${status.error}`, done: true, logs: tail })
        return
      }
      finishSetupResult(status.result || {}, edition, version, lastLogLine)
      emit('activity', { title: 'Установка Bedrock', detail: notice.value, done: true, logs: tail })
      return
    }
    const detail = `Устанавливаю Minecraft ${edition === 'preview' ? 'Preview ' : ''}${version}…${lastLogLine ? ` ${lastLogLine}` : ''}`
    if (!resumed) notice.value = detail
    emit('activity', { title: 'Установка Bedrock', detail })
  }
}

async function resumeSetupJob() {
  try {
    const status = await bridge.bedrockSetupStatus(props.instance.name)
    if (status.running) {
      versionInstallBusy.value = true
      notice.value = 'Установка продолжается — статус ниже.'
      installLog.value = (status.logs_tail || []).join('\n')
      emit('activity', { title: 'Установка Bedrock', detail: 'Устанавливаю Minecraft Bedrock…' })
      const edition = versionEdition.value || 'release'
      const version = currentVersion.value || versionSelection.value || ''
      await watchSetupJob(edition, version, true)
    } else if (status.finished && status.error) {
      notice.value = `Прошлая установка не завершилась: ${status.error}`
    }
  } catch {
  }
}

async function installVersion() {
  if (!versionSelection.value) return
  const edition = versionEdition.value
  const version = versionSelection.value
  versionInstallBusy.value = true
  notice.value = `Подготавливаю Minecraft ${edition === 'preview' ? 'Preview ' : ''}${version}…`
  emit('activity', { title: 'Установка Bedrock', detail: notice.value })
  try {
    await bridge.bedrockSetupVersion(props.instance.name, edition, version)
    await watchSetupJob(edition, version)
  } catch (error) {
    notice.value = error instanceof Error ? error.message : String(error)
  } finally {
    versionInstallBusy.value = false
  }
}

function finishSetupResult(
  result: { exit_code?: number; error_hint?: string; requires_login?: boolean; installed?: boolean; failed?: boolean; logs_tail?: string[]; current_edition?: string; current_version?: string },
  edition: string,
  version: string,
  lastLogLine: string,
) {
  installLog.value = (result.logs_tail || []).join('\n') || installLog.value
  if (result.requires_login) {
    pendingInstallAfterLogin.value = true
    notice.value = 'Версия не установлена: нужен вход в Microsoft. Нажмите «Подключить Microsoft Store» — после входа версия доустановится сама.'
    return
  }
  if (result.failed || !result.installed) {
    // bedrockOnLinux может выйти с кодом 0 после собственной "error":
    // показываем настоящую причину, а не молчаливое "ничего не скачалось"
    const hint = result.error_hint || lastLogLine || ''
    notice.value = `Версия не установлена.${hint ? ` Причина: ${hint}` : ''} Полный ход установки — ниже.`
    return
  }
  currentEdition.value = result.current_edition || edition
  currentVersion.value = result.current_version || version
  notice.value = `Minecraft ${currentVersion.value} установлен.`
  emit('changed')
  void pollMetrics()
}

async function cancelVersionInstall() {
  try {
    const result = await bridge.bedrockSetupCancel(props.instance.name)
    if (!result.stopped) notice.value = 'Установка уже завершена.'
  } catch (error) {
    notice.value = error instanceof Error ? error.message : String(error)
  }
}


function formatBytes(value: number) {
  if (value >= 1024 ** 3) return `${(value / 1024 ** 3).toFixed(1)} GiB`
  if (value >= 1024 ** 2) return `${(value / 1024 ** 2).toFixed(0)} MiB`
  if (value >= 1024) return `${(value / 1024).toFixed(0)} KiB`
  return `${Math.max(0, Math.round(value || 0))} B`
}

function fileSize(file: InstanceFile) {
  if (!file.size) return 'Папка'
  return formatBytes(file.size)
}

function joinFilePath(name: string) {
  return [fileDirectory.value, name].filter(Boolean).join('/')
}

async function loadWorkspaceFiles() {
  fileLoading.value = true
  try {
    const result = await bridge.instanceFileList(props.instance.name, fileDirectory.value)
    workspaceFiles.value = result.files
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : String(error))
  } finally {
    fileLoading.value = false
  }
}

function goToWorkspaceDirectory(path: string) {
  fileDirectory.value = path
  void loadWorkspaceFiles()
}

function goToParentDirectory() {
  goToWorkspaceDirectory(fileDirectory.value.split('/').slice(0, -1).join('/'))
}

async function openWorkspaceFile(file: InstanceFile) {
  if (file.kind === 'directory') {
    goToWorkspaceDirectory(joinFilePath(file.name))
    return
  }
  if (editorPath.value && editorContent.value !== editorOriginalContent.value && !(await confirmDialog(
    'Открыть другой файл и потерять несохранённые изменения?',
    { title: 'Есть несохранённые изменения', confirmLabel: 'Открыть файл', danger: true },
  ))) return
  const path = joinFilePath(file.name)
  try {
    if (file.editable && props.fileEditorMode === 'system') {
      const root = props.instance.path
      const separator = root.includes('\\') ? '\\' : '/'
      const fullPath = `${root.replace(/[\\/]+$/, '')}${separator}${path.replace(/^[\\/]+/, '').replace(/[\\/]/g, separator)}`
      await bridge.fsOpenFile(fullPath)
      return
    }
    if (file.editable) {
      const result = await bridge.instanceFileRead(props.instance.name, path)
      editorPath.value = path
      editorContent.value = result.content
      editorOriginalContent.value = result.content
      filePreview.value = undefined
      return
    }
    const result = await bridge.instanceFileRead(props.instance.name, path, 'base64')
    filePreview.value = { path, source: `data:${result.mime_type || 'image/png'};base64,${result.content}` }
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : String(error))
  }
}

async function closeEditor() {
  if (editorContent.value !== editorOriginalContent.value && !(await confirmDialog(
    'Закрыть редактор и потерять несохранённые изменения?',
    { title: 'Есть несохранённые изменения', confirmLabel: 'Закрыть', danger: true },
  ))) return
  editorPath.value = ''
  editorContent.value = ''
  editorOriginalContent.value = ''
}

async function saveWorkspaceFile() {
  editorSaving.value = true
  try {
    await bridge.instanceFileWrite(props.instance.name, editorPath.value, editorContent.value)
    editorOriginalContent.value = editorContent.value
    emit('toast', 'Файл сохранён')
    await loadWorkspaceFiles()
    await pollMetrics()
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : String(error))
  } finally {
    editorSaving.value = false
  }
}

async function createWorkspaceDirectory() {
  const name = (await promptDialog('Название новой папки', { placeholder: 'Например, config', confirmLabel: 'Создать' }))?.trim()
  if (!name) return
  fileMutationBusy.value = true
  try {
    await bridge.instanceFileMkdir(props.instance.name, joinFilePath(name))
    await loadWorkspaceFiles()
    await pollMetrics()
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : String(error))
  } finally {
    fileMutationBusy.value = false
  }
}

async function deleteWorkspaceFile(file: InstanceFile) {
  if (props.confirmDestructive && !(await confirmDialog(`Удалить «${file.name}»?`, {
    title: 'Удалить файл или папку',
    confirmLabel: 'Удалить',
    danger: true,
    detail: joinFilePath(file.name),
  }))) return
  fileMutationBusy.value = true
  try {
    const path = joinFilePath(file.name)
    await bridge.instanceFileDelete(props.instance.name, path)
    if (editorPath.value === path) {
      editorPath.value = ''
      editorContent.value = ''
      editorOriginalContent.value = ''
    }
    if (filePreview.value?.path === path) filePreview.value = undefined
    await loadWorkspaceFiles()
    await pollMetrics()
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : String(error))
  } finally {
    fileMutationBusy.value = false
  }
}

async function selectWorkspaceUpload() {
  const picked = await pickFiles({ title: 'Загрузка файлов' })
  if (!picked?.length) return
  fileMutationBusy.value = true
  try {
    for (const file of picked) {
      if (file.size > 32 * 1024 * 1024) {
        emit('toast', `${file.fileName}: превышен лимит 32 MiB`)
        continue
      }
      await bridge.instanceFileUpload(props.instance.name, joinFilePath(file.fileName), file.data)
    }
    await loadWorkspaceFiles()
    await pollMetrics()
    if (picked.some((file) => file.size <= 32 * 1024 * 1024)) emit('toast', 'Файлы загружены')
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : String(error))
  } finally {
    fileMutationBusy.value = false
  }
}

function chooseWorkspaceFiles() {
  void selectWorkspaceUpload()
}

async function connectMicrosoftStore() {
  actionBusy.value = true
  loginPending.value = true
  loginCancelRequested.value = false
  notice.value = 'Откроется отдельное окно Microsoft. Войдите в аккаунт, которому принадлежит Minecraft.'
  try {
    // вход идёт в фоновой нити: браузерная авторизация занимает минуты,
    // и раньше это время намертво блокировало весь лаунчер
    await bridge.bedrockStoreLogin(props.instance.name)
    for (;;) {
      await new Promise((resolve) => window.setTimeout(resolve, 1500))
      const status = await bridge.bedrockStoreLoginStatus(props.instance.name)
      if (!status.started && !status.running) {
        notice.value = 'Вход не запущен.'
        return
      }
      const tail = status.logs_tail || []
      if (tail.length) {
        installLog.value = tail.join('\n')
        const hint = tail[tail.length - 1]
        if (status.running) notice.value = `Ожидаю вход в Microsoft… ${hint}`
      }
      if (!status.running && status.finished) {
        if (status.cancelled) {
          notice.value = 'Вход отменён.'
          return
        }
        if (status.error) {
          notice.value = `Вход не завершён: ${status.error}`
          return
        }
        const result: Partial<NonNullable<import('../types').BedrockSetupStatus['result']>> = status.result || {}
        if (result.failed) {
          notice.value = `Вход не завершён.${result.error_hint ? ` Причина: ${result.error_hint}` : ''}`
          return
        }
        notice.value = 'Аккаунт Microsoft подключён. Лицензия будет проверена при запуске игры.'
        void refreshAccountStatus()
        // после входа продолжаем ожидающую установку версии
        if (pendingInstallAfterLogin.value && versionSelection.value && versionSelection.value !== currentVersion.value) {
          pendingInstallAfterLogin.value = false
          void installVersion()
        }
        return
      }
    }
  } catch (error) {
    notice.value = loginCancelRequested.value
      ? 'Вход отменён.'
      : error instanceof Error ? error.message : String(error)
  } finally {
    loginPending.value = false
    actionBusy.value = false
  }
}

async function cancelMicrosoftStoreLogin() {
  loginCancelBusy.value = true
  loginCancelRequested.value = true
  try {
    const result = await bridge.bedrockCancelStoreLogin(props.instance.name)
    if (!result.stopped && loginPending.value) {
      loginCancelRequested.value = false
      notice.value = 'Окно входа уже завершилось или не найдено.'
    } else if (loginPending.value) {
      notice.value = 'Закрываю окно входа Microsoft…'
    }
  } catch (error) {
    loginCancelRequested.value = false
    notice.value = error instanceof Error ? error.message : String(error)
  } finally {
    loginCancelBusy.value = false
  }
}

function chooseContentFile() {
  void pickFile({
    accept: '.mcpack,.mcaddon,.mcworld,.mctemplate,.mcskin',
    title: 'Bedrock-контент',
  }).then((file) => {
    if (file) void importContent(file)
  })
}

async function importContent(file: { fileName: string; data: string; size: number }) {
  if (file.size > 128 * 1024 * 1024) {
    emit('toast', 'Файл больше 128 MiB')
    return
  }
  actionBusy.value = true
  notice.value = `Импортирую ${file.fileName}…`
  try {
    const result = await bridge.bedrockContentImport(props.instance.name, file.fileName, file.data)
    items.value = result.items
    await pollMetrics()
    notice.value = `Импорт завершён: ${file.fileName}`
    emit('toast', 'Bedrock-контент импортирован')
    emit('changed')
  } catch (error) {
    notice.value = error instanceof Error ? error.message : String(error)
    emit('toast', notice.value)
  } finally {
    actionBusy.value = false
  }
}

async function removeContent(item: BedrockContentItem) {
  if (props.confirmDestructive && !(await confirmDialog(
    `Удалить «${item.name}»?`,
    { title: 'Удалить Bedrock-контент', confirmLabel: 'Удалить', danger: true, detail: `${item.scope_label} · ${item.directory}` },
  ))) return
  actionBusy.value = true
  try {
    const result = await bridge.bedrockContentRemove(props.instance.name, item.scope, item.directory, item.name)
    items.value = result.items
    await pollMetrics()
    emit('toast', 'Bedrock-контент удалён')
    emit('changed')
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : String(error))
  } finally {
    actionBusy.value = false
  }
}

function formatSize(bytes: number) {
  if (!bytes) return 'Папка'
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KiB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`
}

watch(() => props.instance.name, () => void ensureLoaded())
let metricsTimer: number | undefined
const consoleLines = ref<string[]>([])
const consoleRunning = ref(false)
const consoleBusy = ref(false)
const logArchives = ref<InstanceLogArchive[]>([])
const archiveViewer = ref<{ id: string; content: string } | null>(null)
const archiveBusy = ref(false)
let consoleTimer: number | undefined

async function pollConsole() {
  try {
    const result = await bridge.instanceLog(props.instance.name)
    consoleLines.value = result.lines.slice(-300)
    consoleRunning.value = result.running
  } catch {
    // журнал появится после создания сессии
  }
}

function startConsolePolling() {
  if (consoleTimer !== undefined) return
  void pollConsole()
  consoleTimer = window.setInterval(() => void pollConsole(), 2500)
}

function stopConsolePolling() {
  if (consoleTimer !== undefined) window.clearInterval(consoleTimer)
  consoleTimer = undefined
}

async function loadLogArchives() {
  try {
    const result = await bridge.instanceLogArchives(props.instance.name, 0, 50)
    logArchives.value = result.logs
  } catch {
    logArchives.value = []
  }
}

async function openLogArchive(id: string) {
  archiveBusy.value = true
  try {
    const result = await bridge.instanceLogArchiveRead(props.instance.name, id, 0, 400)
    archiveViewer.value = { id, content: result.content }
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : String(error))
  } finally {
    archiveBusy.value = false
  }
}

// скриншоты Bedrock
const screenshots = ref<BedrockScreenshotItem[]>([])
const screenshotsLoading = ref(false)
const screenshotPreview = ref<{ name: string; source: string } | null>(null)
const screenshotBusy = ref(false)

async function loadScreenshots() {
  screenshotsLoading.value = true
  try {
    const result = await bridge.bedrockScreenshotsList(props.instance.name)
    screenshots.value = result.items
  } catch (error) {
    screenshots.value = []
    if (!String(error instanceof Error ? error.message : error).includes('недоступен')) {
      emit('toast', 'Скриншоты недоступны: запустите игру хотя бы один раз.')
    }
  } finally {
    screenshotsLoading.value = false
  }
}

async function openScreenshot(item: BedrockScreenshotItem) {
  if (item.size > 32 * 1024 * 1024) {
    emit('toast', 'Скриншот слишком большой для предпросмотра.')
    return
  }
  screenshotBusy.value = true
  try {
    const result = await bridge.instanceFileRead(props.instance.name, item.read_path, 'base64')
    screenshotPreview.value = { name: item.name, source: `data:image/png;base64,${result.content}` }
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : String(error))
  } finally {
    screenshotBusy.value = false
  }
}

// резервные копии контента Bedrock
const snapshotCategories: Array<{ id: string; label: string }> = [
  { id: 'resourcepacks', label: 'Паки ресурсов' },
  { id: 'addons', label: 'Аддоны' },
  { id: 'worlds', label: 'Миры' },
]
const snapshotCategory = ref('resourcepacks')
const snapshots = ref<ServerSnapshot[]>([])
const snapshotsLoading = ref(false)
const snapshotBusy = ref(false)

async function loadSnapshots() {
  snapshotsLoading.value = true
  try {
    const result = await bridge.instanceSnapshotList(props.instance.name, snapshotCategory.value)
    snapshots.value = result.snapshots
  } catch (error) {
    snapshots.value = []
    const message = error instanceof Error ? error.message : String(error)
    if (!message.includes('недоступен')) emit('toast', message)
  } finally {
    snapshotsLoading.value = false
  }
}

async function createSnapshot() {
  if (snapshotBusy.value) return
  snapshotBusy.value = true
  try {
    const result = await bridge.instanceSnapshotCreate(props.instance.name, snapshotCategory.value)
    snapshots.value = result.snapshots
    emit('toast', 'Снапшот Bedrock создан.')
    emit('changed')
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : String(error))
  } finally {
    snapshotBusy.value = false
  }
}

async function restoreSnapshot(snapshot: ServerSnapshot) {
  if (snapshotBusy.value) return
  if (props.instance.running) {
    emit('toast', 'Остановите игру перед восстановлением снапшота.')
    return
  }
  snapshotBusy.value = true
  try {
    const result = await bridge.instanceSnapshotRestore(props.instance.name, snapshotCategory.value, snapshot.id)
    snapshots.value = result.snapshots
    emit('toast', 'Снапшот восстановлен.')
    emit('changed')
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : String(error))
  } finally {
    snapshotBusy.value = false
  }
}

async function deleteSnapshot(snapshot: ServerSnapshot) {
  if (snapshotBusy.value) return
  snapshotBusy.value = true
  try {
    const result = await bridge.instanceSnapshotDelete(props.instance.name, snapshotCategory.value, snapshot.id)
    snapshots.value = result.snapshots
    emit('changed')
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : String(error))
  } finally {
    snapshotBusy.value = false
  }
}

function formatSnapshotTime(seconds: number): string {
  if (!seconds) return ''
  return new Date(seconds * 1000).toLocaleString()
}

onMounted(() => {
  void ensureLoaded()
  void loadVersions()
  void loadWorkspaceFiles()
  void resumeSetupJob()
  metricsTimer = window.setInterval(() => {
    if (props.instance.running || metrics.value?.running) void pollMetrics()
  }, 2500)
})
onBeforeUnmount(() => {
  setupWatcherDisposed = true
  if (metricsTimer !== undefined) window.clearInterval(metricsTimer)
  stopConsolePolling()
})
// app вызывает после неудачной установки из диалога создания:
// запоминаем выбранную версию и доустанавливаем её после входа
function applyPendingVersion(edition: 'release' | 'preview', version: string) {
  if (!version) return
  versionEdition.value = edition
  versionSelection.value = version
  pendingInstallAfterLogin.value = true
  notice.value = `Версия ${edition === 'preview' ? 'Preview ' : ''}${version} ждёт входа в Microsoft — нажмите «Подключить Microsoft Store».`
}

defineExpose({ ensureLoaded, applyPendingVersion, refreshAccountStatus })
</script>

<template>
  <div class="bedrock-workspace">
    <div class="workspace-header">
      <div>
        <!-- имя профиля уже показано в заголовке страницы -->
        <p><span class="tag">{{ currentVersionLabel }}</span> {{ status?.message || 'Проверяю готовность игры…' }}</p>
      </div>
      <div class="workspace-header-actions">
        <button class="button button--quiet" type="button" :disabled="loading || actionBusy" @click="refresh">
          <Icon name="refresh" :size="15" />Проверить
        </button>
        <button v-if="gameRunning" class="button button--outline" type="button" @click="emit('stop')">
          <Icon name="stop" :size="15" />Остановить
        </button>
        <button v-else class="button button--accent" type="button" :disabled="busy || actionBusy || versionInstallBusy || !status?.supported" @click="emit('launch')">
          <Icon name="play" :size="15" />Играть
        </button>
      </div>
    </div>

    <nav class="workspace-tabs workspace-tabs--flat" role="tablist" aria-label="Разделы экземпляра Bedrock">
      <button v-for="item in visibleWorkspaceTabs" :key="item.id" class="workspace-tab" :class="{ 'workspace-tab--active': tab === item.id }" type="button" role="tab" :aria-selected="tab === item.id" @click="selectTab(item.id)">
        <Icon :name="item.icon" :size="14" />{{ item.label }}
        <small v-if="item.id === 'content'">{{ items.length }}</small>
      </button>
    </nav>

    <div v-if="loading && !status" class="workspace-loading" role="status">Читаю содержимое профиля Bedrock…</div>

    <div v-else class="workspace-content">
      <Transition name="workspace-tab" mode="out-in">
        <div v-if="tab === 'overview'" key="overview" class="workspace-overview">
          <section class="workspace-launch-banner">
            <div class="workspace-launch-copy">
              <span class="workspace-instance-status" :class="{ 'workspace-instance-status--running': gameRunning }"><i />{{ gameRunning ? 'Игра запущена' : status?.supported ? 'Готов к запуску' : 'Среда запуска проверяется' }}</span>
              <p>{{ currentVersionLabel }} · Bedrock Edition · {{ officialWindows ? 'официальная версия из Microsoft Store' : `профиль ${instance.bedrock_profile || instance.name}` }}</p>
              <p class="bedrock-online-status">
                <span :class="{ 'bedrock-online-status--on': status?.store_signed_in }"><i />{{ status?.store_signed_in ? (officialWindows ? 'Онлайн-режим: вход в аккаунт Microsoft/Xbox выполняется в игре' : `Онлайн-режим: ${status.gamertag || 'аккаунт Store подключён'}`) : officialWindows ? 'Онлайн-режим: войдите в аккаунт Microsoft/Xbox при первом запуске игры' : 'Онлайн-режим выключен: нужен отдельный вход в Microsoft Store' }}</span>
                <span v-if="status?.supported && status.engine_ready === false"><i />{{ officialWindows ? 'Игра ещё не установлена — откройте вкладку «Версия» для установки из Microsoft Store' : 'Окружение GDK-Proton будет доустановлено при запуске' }}</span>
              </p>
            </div>
            <div class="workspace-launch-actions">
              <button v-if="gameRunning" class="button button--outline" type="button" @click="emit('stop')"><Icon name="stop" :size="14" />Остановить</button>
              <button v-else class="button button--accent" type="button" :disabled="busy || actionBusy || !status?.supported" @click="emit('launch')"><Icon name="play" :size="15" />Запустить игру</button>
              <button class="button button--quiet" type="button" @click="selectTab('version')"><Icon name="download" :size="14" />Версия</button>
            </div>
          </section>

          <p v-if="notice" class="bedrock-notice" role="status">{{ notice }}</p>
          <div v-if="status && !status.supported" class="bedrock-install-hint">
            <span v-if="unsupportedPlatform">Официальный Minecraft Bedrock не выпускается для этой ОС: он доступен в Windows (Microsoft Store) и в Linux-сборке Nivora.</span>
            <span v-else>Среда запуска недоступна. Проверьте системные требования и зависимости приложения; игровые файлы загружаются из Microsoft Store под вашей лицензией.</span>
            <a v-if="!unsupportedPlatform" href="https://github.com/Wyze3306/BedrockOnLinux" target="_blank" rel="noreferrer">Проверить системные требования</a>
          </div>

          <div class="workspace-columns">
            <section class="workspace-card bedrock-license-card">
              <div class="workspace-card-heading">
                <div><h3>Лицензия Microsoft Store</h3><span class="muted">{{ officialWindows ? 'Игра лицензируется вашим аккаунтом Microsoft.' : 'Nivora не получает пароль; право на игру проверяется при запуске.' }}</span></div>
              </div>
              <p class="bedrock-license-copy">{{ officialWindows ? 'Официальный Minecraft for Windows устанавливается и обновляется через Microsoft Store. Вход в аккаунт Microsoft/Xbox выполняется в самой игре — Nivora не хранит учётные данные.' : 'Используйте Microsoft-аккаунт, которому принадлежит Minecraft для Windows.' }}</p>
              <p v-if="!officialWindows && accountStatus && !accountStatus.signed_in" class="bedrock-license-state bedrock-license-state--off" role="alert">
                <Icon name="info" :size="15" />
                Аккаунт Microsoft не подключён — без него игра не запустится (её файлы зашифрованы лицензией аккаунта-владельца). Нажмите «Подключить аккаунт»: откроется окно входа Microsoft, пароль вводится на их странице и не сохраняется в Nivora. Вход один и общий для всех экземпляров Bedrock — повторных регистраций устройства он не создаёт.
              </p>
              <p v-else-if="!officialWindows && accountStatus?.signed_in" class="bedrock-license-state bedrock-license-state--on">
                <Icon name="check" :size="15" />
                Аккаунт Microsoft подключён — лицензия проверяется при каждом запуске.
              </p>
              <div v-if="!officialWindows" class="bedrock-license-actions">
                <button v-if="loginPending" class="button button--outline" type="button" :disabled="loginCancelBusy" @click="cancelMicrosoftStoreLogin">
                  <Icon name="close" :size="15" />{{ loginCancelBusy ? 'Закрываю…' : 'Отменить вход' }}
                </button>
                <button v-else class="button" :class="accountStatus && !accountStatus.signed_in ? 'button--accent' : 'button--outline'" type="button" :disabled="actionBusy || gameRunning || !status?.supported" @click="connectMicrosoftStore">
                  <Icon name="accounts" :size="15" />Подключить аккаунт
                </button>
              </div>
            </section>
            <section class="workspace-card">
              <div class="workspace-card-heading"><div><h3>Версия игры</h3><span class="muted">Активная версия профиля</span></div><button class="text-button" type="button" @click="selectTab('version')">Изменить</button></div>
              <div class="bedrock-version-summary">
                <span class="world-icon world-icon--empty"><Icon name="download" :size="16" /></span>
                <span><strong>{{ currentVersionLabel }}</strong><small>{{ officialWindows ? 'Установка и обновления выполняются через Microsoft Store — вкладка «Версия» откроет официальную страницу.' : 'Редакцию и версию можно поменять на вкладке «Версия» — список подтягивается из Microsoft Store.' }}</small></span>
              </div>
            </section>
          </div>

          <section class="workspace-quick-access" aria-label="Инструменты профиля Bedrock">
            <span>Инструменты</span>
            <button type="button" @click="selectTab('version')"><Icon name="download" :size="14" />Версия</button>
            <button type="button" @click="selectTab('content')"><Icon name="archive" :size="14" />Контент</button>
            <button type="button" @click="selectTab('files')"><Icon name="folder" :size="14" />Файлы</button>
          </section>
        </div>

        <div v-else-if="tab === 'version'" key="version" class="bedrock-version-card workspace-card">
          <header class="workspace-card-heading">
            <div>
              <h3>Настройка версии Bedrock</h3>
              <span class="muted">Активная версия: <strong>{{ currentVersionLabel }}</strong></span>
            </div>
            <button class="button button--small button--quiet" type="button" :disabled="versionsLoading || versionInstallBusy || gameRunning" @click="loadVersions(true)">
              <Icon name="refresh" :size="14" />{{ versionsLoading ? 'Обновляю список…' : 'Обновить список' }}
            </button>
          </header>
          <div class="bedrock-version-controls">
            <label class="field"><span>Редакция</span><CustomSelect v-model="versionEdition" :options="editionOptions" :disabled="versionsLoading || versionInstallBusy || gameRunning" /></label>
            <label class="field"><span>Версия Minecraft</span><CustomSelect v-model="versionSelection" :options="versionOptions" :disabled="versionsLoading || versionInstallBusy || !versionOptions.length || gameRunning" /></label>
            <button class="button button--accent bedrock-version-apply" type="button" :disabled="!versionSelection || versionsLoading || versionInstallBusy || gameRunning || !status?.supported || (versionSelection === currentVersion && versionEdition === currentEdition)" @click="installVersion">
              <Icon :name="versionSelection && visibleVersions.find((item) => item.version === versionSelection)?.installed ? 'check' : 'download'" :size="14" />
              {{ versionInstallBusy ? 'Устанавливаю…' : visibleVersions.find((item) => item.version === versionSelection)?.installed ? 'Выбрать версию' : 'Установить и выбрать' }}
            </button>
            <button v-if="versionInstallBusy" class="button button--quiet" type="button" @click="cancelVersionInstall">Отменить установку</button>
          </div>
          <div v-if="versionsLoading && !versions.length" class="bedrock-content-empty">Получаю список стабильных и тестовых выпусков…</div>
          <div v-else-if="!versionOptions.length" class="bedrock-content-empty">Для этой редакции пока нет доступных версий. Нажмите «Обновить список» или проверьте подключение к интернету.</div>
          <p class="bedrock-version-note">Выбор сохраняется отдельно для этого экземпляра. Для загрузки новых сборок нужна лицензия Microsoft Store и подключение к интернету.</p>
          <p v-if="status && !status.supported" class="bedrock-notice bedrock-notice--warn" role="status">Установка версий недоступна в этой сборке: {{ status.message }}</p>
          <p v-if="notice" class="bedrock-notice" role="status">{{ notice }}</p>
          <details v-if="installLog" class="bedrock-install-log">
            <summary>Хвост лога установки ({{ installLog.split('\n').length }} строк)</summary>
            <pre>{{ installLog }}</pre>
          </details>
        </div>

        <div v-else-if="tab === 'content'" key="content" class="bedrock-content-card workspace-card">
          <header class="workspace-card-heading">
            <div>
              <h3>Аддоны, ресурспаки и миры</h3>
              <span class="muted">Контент хранится отдельно от Java-модов и ресурсов других экземпляров.</span>
            </div>
            <div class="bedrock-import-actions">
              
              <button class="button button--accent" type="button" :disabled="actionBusy || gameRunning || !status?.supported" @click="chooseContentFile">
                <Icon name="upload" :size="15" />Импортировать
              </button>
            </div>
          </header>

          <nav class="bedrock-category-tabs" aria-label="Категории Bedrock-контента">
            <button v-for="category in categories" :key="category.id" type="button" :class="{ active: activeCategory === category.id }" @click="activeCategory = category.id">
              {{ category.label }} <span>{{ categoryCount(category.id) }}</span>
            </button>
          </nav>

          <div v-if="loading" class="bedrock-content-empty">Загружаю содержимое профиля…</div>
          <div v-else-if="!visibleItems.length" class="bedrock-content-empty">
            {{ activeCategory === 'addons' ? 'Аддонов пока нет. Импортируйте .mcpack или .mcaddon.' : activeCategory === 'resourcepacks' ? 'Ресурспаков пока нет.' : activeCategory === 'worlds' ? 'Миров пока нет. Импортируйте .mcworld.' : 'Пользовательских скинов пока нет.' }}
          </div>
          <div v-else class="bedrock-content-list">
            <article v-for="item in visibleItems" :key="`${item.scope}:${item.directory}:${item.name}`" class="bedrock-content-row">
              <div>
                <strong>{{ item.name }}</strong>
                <span>{{ item.scope_label }} · {{ item.directory }} · {{ formatSize(item.size) }}</span>
              </div>
              <button class="icon-button icon-button--danger" type="button" :disabled="actionBusy || gameRunning" :aria-label="`Удалить ${item.name}`" @click="removeContent(item)">
                <Icon name="trash" :size="15" />
              </button>
            </article>
          </div>
        </div>

        <div v-else-if="tab === 'console'" key="console" class="workspace-card bedrock-console-card">
          <header class="workspace-card-heading">
            <div>
              <h3>Консоль сессии</h3>
              <span class="muted">Живой поток вывода игры Bedrock и архив прошлых сессий.</span>
            </div>
            <span class="count-badge">{{ consoleRunning ? 'Работает' : 'Остановлена' }}</span>
          </header>
          <div class="console-output" aria-live="polite">
            <p v-if="!consoleLines.length" class="muted">Лог появится после запуска игры.</p>
            <p v-for="(line, index) in consoleLines" :key="`${index}-${line}`">{{ line }}</p>
          </div>
          <section class="bedrock-archives">
            <h4>Архив логов</h4>
            <p v-if="!logArchives.length" class="muted">Прошлых сессий пока нет.</p>
            <div v-else class="bedrock-archive-list">
              <button v-for="archive in logArchives" :key="archive.id" type="button" :disabled="archiveBusy" @click="openLogArchive(archive.id)">
                <Icon name="file" :size="14" />
                <span>{{ archive.id }}</span>
                <small>{{ formatSize(archive.size) }}</small>
              </button>
            </div>
            <div v-if="archiveViewer" class="instance-file-preview bedrock-archive-viewer">
              <div><strong>{{ archiveViewer.id }}</strong><button class="icon-button" type="button" aria-label="Закрыть архив" @click="archiveViewer = null"><Icon name="close" :size="15" /></button></div>
              <pre class="console-output console-output--archive">{{ archiveViewer.content }}</pre>
            </div>
          </section>
        </div>

        <div v-else-if="tab === 'screenshots'" key="screenshots" class="workspace-card bedrock-screenshots-card">
          <header class="workspace-card-heading">
            <div>
              <h3>Скриншоты Bedrock</h3>
              <span class="muted">Снимки внутриигровой камеры игры (com.mojang/minecraftScreenshots).</span>
            </div>
            <button class="button button--outline" type="button" :disabled="screenshotsLoading" @click="loadScreenshots">
              <Icon name="refresh" :size="15" />{{ screenshotsLoading ? 'Загружаю…' : 'Обновить' }}
            </button>
          </header>
          <div v-if="screenshotsLoading" class="bedrock-content-empty">Загружаю скриншоты…</div>
          <div v-else-if="!screenshots.length" class="bedrock-content-empty">
            Скриншотов пока нет: сделайте снимок камерой в игре (или Win+G) и обновите список.
          </div>
          <div v-else class="bedrock-screenshot-grid">
            <button v-for="item in screenshots" :key="item.read_path" type="button" class="bedrock-screenshot-tile" :disabled="screenshotBusy" @click="openScreenshot(item)">
              <span class="bedrock-screenshot-name">{{ item.name }}</span>
              <small>{{ item.scope_label }} · {{ formatSize(item.size) }}</small>
            </button>
          </div>
          <div v-if="screenshotPreview" class="instance-file-preview bedrock-screenshot-viewer">
            <div><strong>{{ screenshotPreview.name }}</strong><button class="icon-button" type="button" aria-label="Закрыть скриншот" @click="screenshotPreview = null"><Icon name="close" :size="15" /></button></div>
            <img :src="screenshotPreview.source" :alt="screenshotPreview.name" />
          </div>
        </div>

        <div v-else-if="tab === 'snapshots'" key="snapshots" class="workspace-card bedrock-snapshots-card">
          <header class="workspace-card-heading">
            <div>
              <h3>Снапшоты контента</h3>
              <span class="muted">Резервные копии паков, аддонов и миров перед обновлениями контента.</span>
            </div>
            <button class="button button--accent" type="button" :disabled="snapshotBusy || snapshotsLoading" @click="createSnapshot">
              <Icon name="plus" :size="15" />Создать снапшот
            </button>
          </header>
          <nav class="bedrock-category-tabs" aria-label="Категории снапшотов Bedrock">
            <button v-for="category in snapshotCategories" :key="category.id" type="button" :class="{ active: snapshotCategory === category.id }" @click="snapshotCategory = category.id; loadSnapshots()">
              {{ category.label }}
            </button>
          </nav>
          <div v-if="snapshotsLoading" class="bedrock-content-empty">Загружаю снапшоты…</div>
          <div v-else-if="!snapshots.length" class="bedrock-content-empty">Снапшотов этой категории пока нет.</div>
          <div v-else class="bedrock-content-list">
            <article v-for="snapshot in snapshots" :key="snapshot.id" class="bedrock-content-row">
              <div>
                <strong>{{ formatSnapshotTime(snapshot.created_at) || snapshot.id }}</strong>
                <span>{{ snapshot.file_count }} файлов · {{ formatSize(snapshot.source_bytes) }}</span>
              </div>
              <div class="bedrock-row-actions">
                <button class="button button--small button--outline" type="button" :disabled="snapshotBusy || gameRunning" @click="restoreSnapshot(snapshot)">Восстановить</button>
                <button class="icon-button icon-button--danger" type="button" :disabled="snapshotBusy" :aria-label="`Удалить ${snapshot.id}`" @click="deleteSnapshot(snapshot)">
                  <Icon name="trash" :size="15" />
                </button>
              </div>
            </article>
          </div>
        </div>

        <div v-else-if="tab === 'files'" key="files" class="bedrock-files-card workspace-card instance-file-explorer">
          <div class="workspace-card-heading">
            <div><h3>Файлы экземпляра</h3><span class="muted">{{ fileDirectory ? `/${fileDirectory}` : 'Папка профиля' }} · редактирование после остановки игры</span></div>
            <div class="workspace-card-actions">
              <button class="button button--small button--quiet" type="button" :disabled="fileLoading || fileMutationBusy" @click="loadWorkspaceFiles"><Icon name="refresh" :size="14" />Обновить</button>
              <button class="button button--small button--quiet" type="button" :disabled="gameRunning || fileMutationBusy" @click="createWorkspaceDirectory"><Icon name="plus" :size="14" />Новая папка</button>
              <button class="button button--small button--outline" type="button" :disabled="gameRunning || fileMutationBusy" @click="chooseWorkspaceFiles"><Icon name="upload" :size="14" />Загрузить</button>
            </div>
          </div>
          
          <nav class="instance-file-breadcrumbs" aria-label="Путь к папке">
            <button type="button" :class="{ active: !fileDirectory }" @click="goToWorkspaceDirectory('')">Профиль</button>
            <template v-for="crumb in fileBreadcrumbs" :key="crumb.path"><Icon name="chevron" :size="12" /><button type="button" :class="{ active: fileDirectory === crumb.path }" @click="goToWorkspaceDirectory(crumb.path)">{{ crumb.name }}</button></template>
            <button v-if="fileDirectory" class="instance-file-up" type="button" @click="goToParentDirectory"><Icon name="arrow-up" :size="13" />Вверх</button>
          </nav>
          <div v-if="fileLoading" class="workspace-loading">Читаю папку…</div>
          <div v-else class="instance-file-list">
            <div v-for="file in workspaceFiles" :key="file.path" class="instance-file-row">
              <button class="instance-file-open" type="button" :disabled="editorSaving" @click="openWorkspaceFile(file)">
                <Icon :name="file.kind === 'directory' ? 'folder' : file.editable ? 'file' : 'image'" :size="17" />
                <span><strong>{{ file.name }}</strong><small>{{ fileSize(file) }}<template v-if="file.kind === 'file' && file.editable"> · текстовый файл</template></small></span>
                <Icon v-if="file.kind === 'file' && file.editable" name="edit" :size="14" />
              </button>
              <button class="icon-button" type="button" :disabled="gameRunning || fileMutationBusy || editorSaving" :aria-label="`Удалить ${file.name}`" @click="deleteWorkspaceFile(file)"><Icon name="trash" :size="14" /></button>
            </div>
            <div v-if="!workspaceFiles.length" class="workspace-empty-panel"><span class="world-icon world-icon--empty"><Icon name="folder" :size="18" /></span><strong>{{ fileDirectory ? 'Папка пуста' : 'Профиль пуст' }}</strong><span>Создайте папку или загрузите файлы в профиль Bedrock.</span></div>
          </div>
          <section v-if="editorPath" class="instance-file-editor">
            <div class="instance-file-editor-heading"><span><Icon name="edit" :size="15" /><strong>{{ editorPath }}</strong></span><button class="icon-button" type="button" aria-label="Закрыть редактор" :disabled="editorSaving" @click="closeEditor"><Icon name="close" :size="15" /></button></div>
            <textarea v-model="editorContent" spellcheck="false" aria-label="Содержимое файла" />
            <div class="instance-file-editor-footer"><span>{{ editorContent.length.toLocaleString() }} символов<template v-if="editorContent !== editorOriginalContent"> · есть несохранённые изменения</template></span><button class="button button--small button--accent" type="button" :disabled="gameRunning || editorSaving || editorContent === editorOriginalContent" @click="saveWorkspaceFile"><Icon name="save" :size="14" />{{ editorSaving ? 'Сохраняю…' : 'Сохранить' }}</button></div>
          </section>
          <div v-if="filePreview" class="instance-file-preview"><div><strong>{{ filePreview.path }}</strong><button class="icon-button" type="button" aria-label="Закрыть предпросмотр" @click="filePreview = undefined"><Icon name="close" :size="15" /></button></div><img :src="filePreview.source" :alt="filePreview.path" /></div>
        </div>
      </Transition>
    </div>
  </div>
</template>

<style scoped>
.bedrock-workspace { display: grid; min-width: 0; gap: 12px; }
.bedrock-workspace .workspace-header-actions { display: flex; flex: 0 0 auto; align-items: center; gap: 7px; }
.bedrock-workspace .workspace-overview { display: grid; min-width: 0; gap: 12px; }
.bedrock-workspace .workspace-metric-grid { margin-bottom: 0; }
.bedrock-version-card, .bedrock-content-card, .bedrock-files-card { overflow: hidden; }
.bedrock-version-controls { display: grid; grid-template-columns: minmax(160px, .8fr) minmax(180px, 1fr) auto; align-items: end; gap: 12px; }
.bedrock-version-apply { min-height: 38px; }
.bedrock-version-note { margin: 12px 0 0; color: var(--muted); font-size: 10px; line-height: 1.5; }
.bedrock-notice, .bedrock-install-hint { margin: 0; padding: 11px 14px; border: 1px solid var(--line); border-radius: var(--radius-control, 10px); color: var(--muted); background: var(--panel-soft); font-size: 11px; line-height: 1.5; }
.bedrock-notice--warn { border-color: color-mix(in srgb, var(--warning, #d2ad72) 45%, transparent); color: var(--text); }
.bedrock-install-log { border: 1px solid var(--line); border-radius: var(--radius-control, 10px); background: var(--panel-soft); font-size: 11px; }
.bedrock-install-log summary { padding: 9px 14px; color: var(--muted); cursor: pointer; }
.bedrock-install-log pre { max-height: 220px; margin: 0; padding: 0 14px 12px; overflow: auto; color: var(--quiet); font-size: 10.5px; line-height: 1.55; white-space: pre-wrap; word-break: break-word; }
.bedrock-install-hint { display: flex; justify-content: space-between; gap: 12px; }
.bedrock-install-hint a { flex: 0 0 auto; color: var(--accent-strong); }
.bedrock-license-card { display: grid; align-content: start; gap: 10px; }
.bedrock-license-card p { margin: 0; color: var(--muted); font-size: 11px; line-height: 1.55; }
.bedrock-license-actions { display: flex; gap: 8px; }
.bedrock-license-state { display: flex; align-items: flex-start; gap: 8px; margin: 0 0 10px; padding: 10px 12px; border-radius: 10px; font-size: 13px; line-height: 1.45; }
.bedrock-license-state--off { border: 1px solid rgba(231, 111, 81, 0.45); background: rgba(231, 111, 81, 0.12); }
.bedrock-license-state--on { border: 1px solid rgba(108, 190, 116, 0.4); background: rgba(108, 190, 116, 0.1); }
.bedrock-version-summary { display: flex; align-items: center; gap: 12px; }
.bedrock-version-summary > span:last-child { display: grid; gap: 4px; }
.bedrock-version-summary strong { color: var(--text); font-size: 12px; }
.bedrock-version-summary small { color: var(--muted); font-size: 10px; line-height: 1.5; }
.bedrock-import-actions { display: flex; flex: 0 0 auto; align-items: center; gap: 7px; }
.bedrock-category-tabs { display: flex; gap: 4px; overflow-x: auto; margin-bottom: 10px; padding-bottom: 10px; border-bottom: 1px solid var(--line); }
.bedrock-category-tabs button { display: inline-flex; align-items: center; gap: 7px; padding: 7px 10px; border: 1px solid transparent; border-radius: var(--radius-button, 8px); color: var(--muted); background: transparent; font: inherit; font-size: 11px; white-space: nowrap; cursor: pointer; }
.bedrock-category-tabs button.active { border-color: var(--line); color: var(--text); background: var(--panel-soft); }
.bedrock-category-tabs span { color: var(--quiet); font-variant-numeric: tabular-nums; }
.bedrock-content-empty { padding: 22px 4px 10px; color: var(--muted); font-size: 11px; }
.bedrock-content-row { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 11px 4px; border-bottom: 1px solid var(--line); }
.bedrock-row-actions { display: flex; align-items: center; gap: 7px; flex: 0 0 auto; }
.console-output { display: grid; gap: 2px; max-height: 260px; overflow: auto; margin: 0; padding: 12px 14px; border: 1px solid var(--line); border-radius: var(--radius-control, 10px); background: color-mix(in srgb, var(--bg) 82%, black); color: var(--text); font-family: var(--font-mono); font-size: 11px; line-height: 1.55; white-space: pre-wrap; word-break: break-word; }
.console-output--archive { max-height: 320px; }
.bedrock-archives { display: grid; gap: 8px; margin-top: 12px; }
.bedrock-archives h4 { margin: 0; font-size: 11px; color: var(--muted); }
.bedrock-archive-list { display: grid; gap: 4px; }
.bedrock-archive-list button { display: flex; align-items: center; gap: 8px; padding: 7px 10px; border: 1px solid var(--line); border-radius: var(--radius-control, 10px); background: transparent; color: var(--text); font-size: 11px; cursor: pointer; text-align: left; }
.bedrock-archive-list button:hover { background: var(--panel-soft); }
.bedrock-archive-list span { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); }
.bedrock-archive-list small { color: var(--muted); }
.bedrock-archive-viewer { margin-top: 8px; }
.bedrock-screenshot-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 10px; }
.bedrock-screenshot-tile { display: grid; gap: 4px; padding: 12px 10px; border: 1px solid var(--line); border-radius: var(--radius-card, 7px); background: var(--panel-soft); color: var(--text); cursor: pointer; text-align: left; }
.bedrock-screenshot-tile:hover { border-color: var(--accent); }
.bedrock-screenshot-name { font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.bedrock-screenshot-tile small { color: var(--muted); font-size: 10px; }
.bedrock-screenshot-viewer img { width: 100%; height: auto; border-radius: var(--radius-control, 10px); }
.bedrock-content-row:last-child { border-bottom: 0; }
.bedrock-content-row > div { display: grid; min-width: 0; gap: 4px; }
.bedrock-content-row strong { overflow: hidden; color: var(--text); font-size: 12px; text-overflow: ellipsis; white-space: nowrap; }
.bedrock-content-row span { overflow: hidden; color: var(--muted); font-size: 10px; text-overflow: ellipsis; white-space: nowrap; }
@media (max-width: 640px) {
  .bedrock-version-controls { grid-template-columns: 1fr; }
  .bedrock-version-apply { width: 100%; justify-content: center; }
  .bedrock-install-hint { flex-direction: column; }
}
</style>
