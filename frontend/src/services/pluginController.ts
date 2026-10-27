import { ref, type Ref } from 'vue'
import { bridge } from './bridge'
import { createPluginHostCall } from './pluginHost'
import { pluginRuntime } from './pluginRuntime'
import { chooseDialog, confirmDialog, promptDialog } from './dialogs'
import { LAUNCHER_VERSION } from '../version'
import type { RuntimePluginManifest } from '../plugins/runtimeManifest'
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
} from '../../../plugins/api'
import type {
  AppearanceSettings,
  InstanceInfo,
  PluginManifest,
  ServerSnapshot,
  SettingsInfo,
} from '../types'

type ActivePluginView = { pluginId: string; pluginName: string; view: PluginUiView }
type ActivePluginCommand = { pluginId: string; pluginName: string; definition: PluginCommandDefinition }
export type ActivePluginShortcut = { pluginId: string; pluginName: string; definition: PluginShortcutDefinition }

type PluginControllerDeps = {
  plugins: Ref<PluginManifest[]>
  settingsDraft: () => Pick<SettingsInfo, 'developer_mode' | 'show_toasts'>
  appearanceDraft: () => AppearanceSettings
  systemDark: () => boolean
  selectedInstance: () => InstanceInfo | undefined
  showToast: (message: string, tone?: PluginUiTone) => void
  errorMessage: (error: unknown) => string
  isSafePluginShortcutBinding: (binding: string) => boolean
  confirmDestructive: (message: string) => Promise<boolean>
  openActivity: (title: string, detail: string) => void
  finishActivity: (detail: string) => void
  currentToastId: () => number
  dismissCurrentToast: (id: number) => void
}

export function createPluginController(deps: PluginControllerDeps) {
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
  const githubPluginInstallBusy = ref(false)
  let pluginDevPollTimer: number | undefined
  let pluginDevGeneration = ''
  let pluginDevRequests: Record<string, number> = {}
  let pluginDevPollBusy = false
  const pluginDevStartedAt = Date.now() * 1000000

  function replacePluginList(next: PluginManifest[]) {
    deps.plugins.value = next.map((plugin) => ({
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
    if (!deps.isSafePluginShortcutBinding(definition.default_key)) {
      deps.showToast(`${pluginName}: используйте Ctrl, Alt или Meta в сочетании клавиш`, 'warning')
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

  function pluginInstanceRef(instance = deps.selectedInstance()): PluginInstanceRef | null {
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
      // разрешение могли отозвать до доставки события
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
    const eligible = deps.plugins.value.filter((plugin) => ['2', '3'].includes(String(plugin.api_version || '')) && plugin.installed && plugin.execution_allowed)
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
        deps.showToast(`${plugin.name}: ${deps.errorMessage(error)}`)
        continue
      }

      const current = deps.plugins.value.find((entry) => entry.id === plugin.id)
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
              const currentMode = deps.appearanceDraft().theme === 'system'
                ? (deps.systemDark() ? 'dark' : 'light')
                : deps.appearanceDraft().theme
              const mode = currentMode === 'glass' ? 'glass' : currentMode === 'light' ? 'light' : 'dark'
              const tokens: Record<string, string | number> = { ...deps.appearanceDraft().color_tokens }
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
              return { mode, accent: deps.appearanceDraft().accent_color, tokens }
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
              deps.showToast(`${title}: ${message}`, tone)
              if (deps.settingsDraft().show_toasts) {
                const notifications = pluginNotificationToastIds.get(plugin.id) || new Map<string, number>()
                notifications.set(notificationId, deps.currentToastId())
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
                    if (notificationToastId === deps.currentToastId()) deps.dismissCurrentToast(notificationToastId)
              return true
            },
          }),
          onView: (view) => publishPluginView(plugin.id, plugin.name, view),
          onError: (error) => {
            deps.showToast(`${plugin.name}: ${error.message}`)
            queueMicrotask(() => {
              if (!pluginRuntime.runtimeIds().includes(plugin.id)) clearPluginRuntimeState(plugin.id)
            })
          },
        })
        runningPluginDigests.set(plugin.id, source.package_sha256)
        await emitPluginEvent(plugin.id, 'launcher.ready', { version: LAUNCHER_VERSION })
        await emitPluginEvent(plugin.id, 'instance.selected', { instance: pluginInstanceRef() })
      } catch (error) {
        clearPluginRuntimeState(plugin.id)
        deps.showToast(`${plugin.name}: ${deps.errorMessage(error)}`)
      }
    }
  }

  async function pollPluginDevelopment() {
    if (!deps.settingsDraft().developer_mode || pluginDevPollBusy) return
    pluginDevPollBusy = true
    try {
      const next = await bridge.pluginDevChanges()
      if (!next.enabled || next.generation === pluginDevGeneration) return
      const initial = !pluginDevGeneration
      pluginDevGeneration = next.generation
      const changed = Object.keys(next.requests).filter((id) => next.requests[id] !== pluginDevRequests[id])
      pluginDevRequests = next.requests
      if (initial && !changed.some((id) => next.requests[id] >= pluginDevStartedAt)) return
      // сериализация исключает гонки с установкой и сменой разрешений
      pluginRuntimeSyncQueue = pluginRuntimeSyncQueue.then(async () => {
        for (const id of changed) { await pluginRuntime.stop(id); clearPluginRuntimeState(id) }
        await reconcilePluginRuntimes()
      }).catch((error: unknown) => deps.showToast(deps.errorMessage(error)))
    } catch (error) { deps.showToast(deps.errorMessage(error)) }
    finally { pluginDevPollBusy = false }
  }

  function schedulePluginRuntimeSync() {
    pluginRuntimeSyncQueue = pluginRuntimeSyncQueue
      .then(reconcilePluginRuntimes)
      .catch((error: unknown) => deps.showToast(deps.errorMessage(error)))
  }

  function runPluginAction(payload: { pluginId: string; action: string; input: PluginJsonValue }) {
    void (async () => {
      try {
        await bridge.pluginApiAuthorize(payload.pluginId, 'ui:compose')
        await pluginRuntime.runAction(payload.pluginId, payload.action, payload.input)
      } catch (error) {
        deps.showToast(deps.errorMessage(error))
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
          deps.showToast(`Заполните аргумент «${argument.name}» команды «${command.definition.label}»`)
          return
        }
        if (argument.choices?.length && !argument.choices.includes(value)) {
          deps.showToast(`Недопустимое значение аргумента «${argument.name}»`)
          return
        }
        arguments_[argument.name] = value
      }
      await pluginRuntime.runCommand(payload.pluginId, payload.commandId, arguments_)
    } catch (error) {
      deps.showToast(`${command.pluginName}: ${deps.errorMessage(error)}`)
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
      deps.showToast(`${shortcut.pluginName}: ${deps.errorMessage(error)}`)
    }
  }

  async function savePluginConfig(payload: { pluginId: string; key: string; value: PluginJsonValue }) {
    try {
      await bridge.pluginConfigSet(payload.pluginId, payload.key, payload.value)
    } catch (error) {
      deps.showToast(deps.errorMessage(error))
    }
  }

  async function openPluginLink(payload: { pluginId: string; url: string }) {
    try {
      await bridge.pluginApiOpenExternal(payload.pluginId, payload.url)
    } catch (error) {
      deps.showToast(deps.errorMessage(error))
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
      deps.showToast(`${plugin.name}: ${result.plugin.enabled ? 'включён' : 'выключен'}`)
    } catch (error) {
      deps.showToast(deps.errorMessage(error))
      schedulePluginRuntimeSync()
    }
  }

  async function updatePluginGrants(payload: { pluginId: string; permissions: PluginManifest['permissions'] }) {
    try {
      const result = await bridge.pluginPermissionGrantsSet(payload.pluginId, payload.permissions)
      replacePluginList(result.plugins)
      deps.showToast('Разрешения плагина обновлены')
    } catch (error) {
      deps.showToast(deps.errorMessage(error))
    }
  }

  async function allowLocalPluginExecution(payload: { pluginId: string; allowed: boolean }) {
    if (payload.allowed) {
      const plugin = deps.plugins.value.find((entry) => entry.id === payload.pluginId)
      if (!plugin || !(await confirmDialog(
        `Разрешить выполнение неподписанного кода плагина «${plugin.name}» на этом устройстве?`,
        { danger: true, confirmLabel: 'Разрешить выполнение' },
      ))) return
    }
    try {
      const result = await bridge.pluginLocalUntrustedExecution(payload.pluginId, payload.allowed)
      replacePluginList(result.plugins)
    } catch (error) {
      deps.showToast(deps.errorMessage(error))
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
      deps.showToast('Ключ издателя закреплён на этом устройстве')
    } catch (error) {
      deps.showToast(deps.errorMessage(error))
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
      deps.showToast('Доверие к ключу издателя отозвано')
    } catch (error) {
      deps.showToast(deps.errorMessage(error))
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
      deps.showToast(payload.enabled ? 'Ключу разрешено подписывать списки отзыва' : 'Полномочие ключа отозвано')
    } catch (error) {
      deps.showToast(deps.errorMessage(error))
    }
  }

  async function importPluginRevocations(documentJson: string) {
    try {
      const result = await bridge.pluginRevocationsImport(documentJson)
      replacePluginList(result.plugins)
      deps.showToast(`Импортирован список отзыва №${result.imported.sequence} от ${result.imported.authority_key_id}`)
    } catch (error) {
      deps.showToast(deps.errorMessage(error))
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
      deps.showToast(deps.errorMessage(error))
    }
  }

  async function unrevokePlugin(plugin: PluginManifest) {
    if (!(await confirmDialog(`Снять локальную блокировку плагина «${plugin.name}»?`, { confirmLabel: 'Разблокировать' }))) return
    try {
      const result = await bridge.pluginLocalUnrevoke(plugin.id)
      replacePluginList(result.plugins)
    } catch (error) {
      deps.showToast(deps.errorMessage(error))
    }
  }

  async function installPluginArchive(payload: { name: string; data: string }) {
    deps.openActivity('Установка плагина', payload.name)
    try {
      const result = await bridge.pluginInstall(payload.data, false)
      replacePluginList(result.plugins)
      deps.finishActivity(`Плагин «${result.plugin.name}» установлен и выключен. Проверьте разрешения перед включением.`)
    } catch (error) {
      deps.finishActivity(deps.errorMessage(error))
    }
  }

  async function installGithubPlugin(payload: { owner: string; repo: string; replace: boolean }) {
    if (githubPluginInstallBusy.value) return
    githubPluginInstallBusy.value = true
    try {
      deps.openActivity('Проверка архива GitHub', `${payload.owner}/${payload.repo}`)
      const result = await bridge.githubPluginInstall(payload.owner, payload.repo, payload.replace)
      replacePluginList(result.plugins)
      deps.finishActivity(`Архив «${result.plugin.name}» проверен и сохранён · SHA-256 ${result.metadata.sha256}. Код не запускается.`)
    } catch (error) {
      deps.finishActivity(deps.errorMessage(error))
    } finally {
      githubPluginInstallBusy.value = false
    }
  }

  async function uninstallPlugin(plugin: PluginManifest) {
    if (!(await deps.confirmDestructive(`Удалить плагин «${plugin.name}» и его данные?`))) return
    try {
      await pluginRuntime.stop(plugin.id)
      clearPluginRuntimeState(plugin.id)
      const result = await bridge.pluginUninstall(plugin.id)
      replacePluginList(result.plugins)
      deps.showToast(`Плагин «${plugin.name}» удалён`)
    } catch (error) {
      deps.showToast(deps.errorMessage(error))
    }
  }

  function clearNotificationsForToast(toastIdToClear: number) {
    for (const [pluginId, notifications] of pluginNotificationToastIds) {
      for (const [notificationId, notificationToastId] of notifications) {
        if (notificationToastId === toastIdToClear) notifications.delete(notificationId)
      }
      if (notifications.size === 0) pluginNotificationToastIds.delete(pluginId)
    }
  }

  function startDevelopmentPolling() {
    if (pluginDevPollTimer !== undefined) return
    pluginDevPollTimer = window.setInterval(() => { void pollPluginDevelopment() }, 2000)
  }

  function dispose() {
    if (pluginDevPollTimer !== undefined) window.clearInterval(pluginDevPollTimer)
    pluginDevPollTimer = undefined
    pluginViews.value = []
    pluginCommands.value = []
    pluginShortcuts.value = []
    pluginEventSubscriptions.clear()
    pluginLogEventInFlight.clear()
    pluginLogEventLastSentAt.clear()
    pluginNotificationToastIds.clear()
    pluginThemeContributions.clear()
    runningPluginDigests.clear()
  }

  return {
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
    reconcilePluginRuntimes,
    pollPluginDevelopment,
    schedulePluginRuntimeSync,
    uninstallPlugin,
    installGithubPlugin,
    installPluginArchive,
    unrevokePlugin,
    revokePlugin,
    importPluginRevocations,
    setPluginPublisherRevocationAuthority,
    unpinPluginPublisher,
    pinPluginPublisher,
    allowLocalPluginExecution,
    updatePluginGrants,
    togglePlugin,
    openPluginLink,
    savePluginConfig,
    runPluginShortcut,
    runPluginCommand,
    runPluginAction,
    clearNotificationsForToast,
    startDevelopmentPolling,
    dispose,
  }
}
