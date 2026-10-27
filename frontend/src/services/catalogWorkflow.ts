import { computed, onScopeDispose, ref, watch, type ComputedRef, type Ref } from 'vue'
import type { PluginEventPayloadMap, PluginInstanceRef, PluginUiTone } from '../../../plugins/api'
import { modCatalogSearchContext } from './modCatalogSearch'
import { bridge } from './bridge'
import type {
  ActivityState,
  CatalogSort,
  InstanceInfo,
  InstanceWorld,
  ModResult,
  ProjectType,
  ViewName,
  LoaderId,
} from '../types'

type CatalogWorkflowDependencies = {
  currentView: Ref<ViewName>
  selectedInstance: ComputedRef<InstanceInfo | undefined>
  selectedInstanceName: Ref<string>
  modQuery: Ref<string>
  modProjectType: Ref<ProjectType>
  selectedModCategory: Ref<string>
  activeCatalogId: Ref<string>
  activeCatalog: ComputedRef<{ kind?: string } | undefined>
  catalogServerCore: Ref<string>
  selectedVersion: Ref<string>
  selectedLoader: Ref<LoaderId>
  selectedLoaderLabel: ComputedRef<string>
  selectedProjectLabel: ComputedRef<string>
  modInstanceVersion: ComputedRef<string>
  modSortMode: Ref<CatalogSort>
  modResults: Ref<ModResult[]>
  modSearchPerformed: Ref<boolean>
  modBusy: Ref<boolean>
  modError: Ref<string>
  modInstallId: Ref<string>
  modAction: Ref<'install' | 'remove'>
  modInstallProgress: Ref<number>
  modInstallDetail: Ref<string>
  pendingDatapack: Ref<ModResult | undefined>
  datapackWorlds: Ref<InstanceWorld[]>
  datapackDialogOpen: Ref<boolean>
  datapackWorldId: Ref<string>
  activity: ActivityState
  showToast: (message: string, tone?: PluginUiTone, action?: { label: string; run: () => void } | null) => void
  openActivity: (title: string, detail: string, visible?: boolean) => void
  finishActivity: (detail: string, logs?: string[]) => void
  errorMessage: (error: unknown) => string
  isCancelled: (error: unknown) => boolean
  confirmDestructive: (message: string) => Promise<boolean>
  ensureCatalogForView: (view: ViewName) => void
  ensureCompatibleInstanceForCatalog: () => void
  setView: (view: ViewName) => void
  createModpackInstance: (mod?: ModResult) => Promise<void>
  broadcastPluginEvent: (event: 'mods.beforeInstall', payload: PluginEventPayloadMap['mods.beforeInstall']) => Promise<unknown>
  pluginInstanceRef: (instance: InstanceInfo) => PluginInstanceRef | null
  withUnverifiedDownloadConfirmation: <T>(
    attempt: (confirmUnverified: boolean) => Promise<T>,
    prompt: string,
    cancelMessage: string,
  ) => Promise<{ value: T; confirmed: boolean }>
}

const CATALOG_PAGE_SIZE = 20
const CATALOG_MAX_PAGES = 500

export function createCatalogWorkflow(deps: CatalogWorkflowDependencies) {
  const {
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
    createModpackInstance,
    broadcastPluginEvent,
    pluginInstanceRef,
    withUnverifiedDownloadConfirmation,
  } = deps

  const modPage = ref(0)
  const modTotal = ref<number | null>(null)
  const modHasMore = ref(false)
  const modPageCount = computed(() => {
    const known = modTotal.value !== null ? Math.ceil(modTotal.value / CATALOG_PAGE_SIZE) : modPage.value + (modHasMore.value ? 2 : 1)
    return Math.max(1, Math.min(CATALOG_MAX_PAGES, known))
  })
  /** индекс страницы; -1 обозначает многоточие */
  const modPagerPages = computed(() => {
    const last = modPageCount.value - 1
    const current = modPage.value
    const pages = new Set<number>([0, last])
    for (let page = current - 2; page <= current + 2; page += 1) if (page >= 0 && page <= last) pages.add(page)
    const sorted = [...pages].sort((left, right) => left - right)
    const result: number[] = []
    for (const page of sorted) {
      if (result.length && page - result[result.length - 1] > 1) result.push(-1)
      result.push(page)
    }
    return result
  })
  const catalogSelection = ref(new Map<string, ModResult>())
  const catalogBulkProgress = ref('')
  const catalogSelectable = computed(() => {
    const instance = selectedInstance.value
    if (currentView.value !== 'mods' || !instance) return false
    if (modProjectType.value === 'modpack' || modProjectType.value === 'datapack') return false
    if (modProjectType.value === 'plugin') return instance.kind === 'server' && !instance.running
    return instance.kind === 'client' && instance.game_edition !== 'bedrock'
  })
  let modSearchTimer: number | undefined
  let modSearchRequestId = 0

  function catalogKey(mod: ModResult): string {
    return mod.project_id || mod.id || ''
  }

  function toggleCatalogSelection(mod: ModResult) {
    const key = catalogKey(mod)
    if (!key) return
    const next = new Map(catalogSelection.value)
    if (next.has(key)) next.delete(key)
    else next.set(key, mod)
    catalogSelection.value = next
  }

  function selectCatalogPage() {
    const next = new Map(catalogSelection.value)
    for (const mod of modResults.value) {
      const key = catalogKey(mod)
      if (key && !mod.installed) next.set(key, mod)
    }
    catalogSelection.value = next
  }

  function clearCatalogSelection() {
    catalogSelection.value = new Map()
  }

  async function installSelectedMods() {
    const targets = [...catalogSelection.value.values()].filter((mod) => !mod.installed)
    if (!targets.length || modBusy.value || !catalogSelectable.value) return
    const projectType = modProjectType.value
    let installed = 0
    const failed: string[] = []
    for (const [index, mod] of targets.entries()) {
      catalogBulkProgress.value = `Устанавливаю ${index + 1} из ${targets.length}: ${mod.title || mod.name || catalogKey(mod)}`
      const ok = await installModIntoTarget(mod, undefined, projectType, { quiet: true })
      const key = catalogKey(mod)
      if (ok) {
        installed += 1
        // сохраняем отметку об установке и в видимых результатах
        const visible = modResults.value.find((item) => catalogKey(item) === key)
        if (visible && visible !== mod) visible.installed = true
        const next = new Map(catalogSelection.value)
        next.delete(key)
        catalogSelection.value = next
      } else {
        failed.push(mod.title || mod.name || key)
      }
    }
    catalogBulkProgress.value = ''
    showToast(failed.length ? `Установлено: ${installed} из ${targets.length}. Не удалось: ${failed.slice(0, 3).join(', ')}${failed.length > 3 ? '…' : ''}` : `Установлено проектов: ${installed}`)
  }

  function catalogTargetInstance() {
    const instance = selectedInstance.value
    return instance && instance.kind === 'client' && instance.game_edition !== 'bedrock' ? instance : null
  }

  function goToCatalogPage(page: number) {
    if (page < 0 || page === modPage.value || modBusy.value) return
    void searchMods(page).then(() => {
      document.querySelector(`[data-page-view="${currentView.value}"]`)?.scrollIntoView({ block: 'start', behavior: 'smooth' })
    })
  }

  async function searchMods(pageArgument: unknown = 0) {
    const page = typeof pageArgument === 'number' && Number.isFinite(pageArgument) ? Math.max(0, Math.min(CATALOG_MAX_PAGES - 1, Math.floor(pageArgument))) : 0
    ensureCatalogForView(currentView.value)
    ensureCompatibleInstanceForCatalog()
    const requestId = ++modSearchRequestId
    const projectType: ProjectType = currentView.value === 'modpacks' ? 'modpack' : modProjectType.value === 'modpack' ? 'mod' : modProjectType.value
    // для модпака используем версию и загрузчик нового экземпляра
    const searchView = currentView.value
    const isModpackSearch = searchView === 'modpacks'
    const targetInstance = isModpackSearch ? null : catalogTargetInstance()
    const searchContext = modCatalogSearchContext({
      view: isModpackSearch ? 'modpacks' : 'mods',
      selectedInstanceName: selectedInstance.value?.name,
      selectedInstanceVersion: modInstanceVersion.value,
      selectedInstanceLoader: targetInstance?.loader,
      selectedVersion: selectedVersion.value,
      selectedLoader: selectedLoader.value,
    })
    const instanceName = searchContext.instance_name
    const searchInstanceName = () => isModpackSearch ? '' : (selectedInstance.value?.name || '')
    const searchProjectType = projectType
    const searchCatalogId = activeCatalogId.value
    const searchCategory = projectType === 'mod' && activeCatalog.value?.kind === 'modrinth' ? selectedModCategory.value : ''
    const requiresServer = projectType === 'plugin'
    if (currentView.value === 'mods' && selectedInstance.value && (requiresServer ? selectedInstance.value.kind !== 'server' : selectedInstance.value.kind === 'server' || selectedInstance.value.game_edition === 'bedrock')) {
      modResults.value = []
      modError.value = requiresServer ? 'Выберите серверный экземпляр для каталога плагинов.' : 'Выберите Java-экземпляр для каталога ресурсов.'
      modSearchPerformed.value = false
      return
    }
    modBusy.value = true
    modError.value = ''
    const targetLabel = isModpackSearch
      ? `Новый экземпляр · ${selectedVersion.value} · ${selectedLoaderLabel.value}`
      : selectedInstance.value?.name || `${selectedProjectLabel.value} · новый экземпляр`
    openActivity('Каталог ресурсов', targetLabel, false)
    try {
      const result = await bridge.modsSearch({
        query: modQuery.value,
        instance_name: searchContext.instance_name,
        game_version: searchContext.game_version,
        loader: searchContext.loader,
        server_core: targetInstance?.server_core || catalogServerCore.value,
        project_type: projectType,
        category: searchCategory || undefined,
        catalog_id: activeCatalogId.value,
        sort_by: isModpackSearch && activeCatalog.value?.kind !== 'modrinth' ? 'relevance' : modSortMode.value,
        page,
        limit: CATALOG_PAGE_SIZE,
      })
      if (
        requestId !== modSearchRequestId
        || searchView !== currentView.value
        || searchProjectType !== (currentView.value === 'modpacks' ? 'modpack' : modProjectType.value)
        || searchCatalogId !== activeCatalogId.value
        || searchCategory !== (searchProjectType === 'mod' && activeCatalog.value?.kind === 'modrinth' ? selectedModCategory.value : '')
        || instanceName !== searchInstanceName()
      ) return
      modResults.value = result.results
      modPage.value = result.page ?? page
      modTotal.value = typeof result.total === 'number' ? result.total : null
      modHasMore.value = Boolean(result.has_more)
      modSearchPerformed.value = true
      finishActivity(modTotal.value !== null ? `Найдено проектов: ${modTotal.value} · страница ${modPage.value + 1} из ${modPageCount.value}` : `Страница ${modPage.value + 1}: проектов ${modResults.value.length}`)
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

  async function installModIntoTarget(mod: ModResult, worldName?: string, projectType = modProjectType.value, options: { quiet?: boolean } = {}): Promise<boolean> {
    const instance = selectedInstance.value
    if (!instance) return false
    const projectId = mod.project_id || mod.id
    if (!projectId) return false
    let succeeded = false
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
      if (!options.quiet) showToast('Ресурс установлен в экземпляр')
      succeeded = true
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
    return succeeded
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

  watch([() => selectedInstance.value?.name, modProjectType, activeCatalogId, currentView], () => clearCatalogSelection())
  watch(modSortMode, () => {
    if (modSearchPerformed.value && (currentView.value === 'mods' || currentView.value === 'modpacks')) void searchMods()
  })
  onScopeDispose(() => {
    if (modSearchTimer) window.clearTimeout(modSearchTimer)
  })
  watch([modProjectType, selectedInstanceName, currentView, activeCatalogId, selectedVersion, selectedModCategory], () => {
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

  return {
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
  }
}
