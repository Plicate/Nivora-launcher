import { computed, ref, type ComputedRef, type Ref } from 'vue'
import { chooseDialog, confirmDialog, promptDialog } from './dialogs'
import { openContextMenu } from './contextMenu'
import { pickFile } from './fileBrowser'
import { bridge } from './bridge'
import type { PluginUiTone } from '../../../plugins/api'
import type {
  ActivityState,
  AppState,
  InstanceInfo,
  LoaderId,
  ModpackDependencyRequirements,
  ModpackModPreview,
  ModpackModPreviewItem,
  ModResult,
  SettingsInfo,
  ViewName,
} from '../types'

type ModpackWorkflowDependencies = {
  state: AppState
  currentView: Ref<ViewName>
  selectedInstance: ComputedRef<InstanceInfo | undefined>
  modBusy: Ref<boolean>
  modInstallDetail: Ref<string>
  modInstanceVersion: ComputedRef<string>
  selectedVersion: Ref<string>
  selectedLoader: Ref<LoaderId>
  selectedLoaderVersion: Ref<string>
  loadingChoices: Ref<boolean>
  selectedInstanceName: Ref<string>
  activeCatalogId: Ref<string>
  memorySetting: ComputedRef<string>
  settingsDraft: Pick<SettingsInfo, 'launch_after_install'>
  busy: Ref<boolean>
  activity: ActivityState
  showToast: (message: string, tone?: PluginUiTone) => void
  openActivity: (title: string, detail: string, visible?: boolean) => void
  finishActivity: (detail: string, logs?: string[]) => void
  searchMods: (pageArgument?: unknown) => Promise<void>
  errorMessage: (error: unknown) => string
  isCancelled: (error: unknown) => boolean
  setView: (view: ViewName, pluginTabId?: string) => void
  launchExisting: (instance: InstanceInfo) => Promise<void>
  openCatalogDetails: (mod: ModResult) => Promise<void>
  effectiveJavaPath: () => string
}

export function createModpackWorkflow(deps: ModpackWorkflowDependencies) {
  const {
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
  } = deps

async function modpackMenu(event: MouseEvent, mod: ModResult) {
  openContextMenu(event, [
    { id: 'details', label: 'Подробности', icon: 'info', action: () => openCatalogDetails(mod) },
    { id: 'create', label: 'Создать экземпляр', icon: 'plus', action: () => void createModpackInstance(mod) },
  ], mod.title || mod.name || 'Модпак')
}

async function chooseModpackFile() {
  const file = await pickFile({ accept: '.zip,.mrpack', title: 'Архив модпака' })
  if (file) void importModpackArchive(file)
}

async function importModpackArchive(file: { fileName: string; data: string; size: number }) {
  if (file.size > 512 * 1024 * 1024) {
    showToast('Архив модпака превышает лимит 512 MiB')
    return
  }
  if (currentView.value === 'modpacks') {
    await createModpackInstance(undefined, { fileName: file.fileName, data: file.data })
    return
  }
  const instance = selectedInstance.value
  if (!instance) return
  modBusy.value = true
  modInstallDetail.value = 'Читаю архив и проверяю файлы…'
  openActivity('Импорт модпака', `${file.fileName} · ${instance.name}`, false)
  try {
    const preflight = await preflightArchiveModpack({
      instance_name: instance.name,
      file_name: file.fileName,
      data: file.data,
      game_version: modInstanceVersion.value,
      loader: instance.loader,
      loader_version: instance.loader_version,
    })
    if (!(await showModpackPreviewDialog(file.fileName, modpackPreviewData(preflight.value.dependencies)))) {
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
        await createModpackInstance(undefined, { fileName: file.fileName, data: file.data })
        finishActivity('Импорт перенаправлен в новый экземпляр.')
        return
      }
      throw new Error('Импорт отменён: требования модпака не совпадают с текущим экземпляром.')
    }
    const result = await bridge.importModpack({
      instance_name: instance.name,
      file_name: file.fileName,
      data: file.data,
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
  }
}

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


  return {
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
  }
}
