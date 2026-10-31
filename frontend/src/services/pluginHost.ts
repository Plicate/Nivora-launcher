import type {
  PluginEventName,
  PluginInstanceRef,
  PluginCommandDefinition,
  PluginNetworkRequest,
  PluginServerInput,
  PluginShortcutDefinition,
  PluginThemeDefinition,
  PluginThemeContext,
  PluginUiTone,
} from '../../../plugins/api'
import type { HttpRequest, ProxyRoutePolicy } from '../../../plugins/api-v3'
import type { PluginPermission } from '../types'
import type { RuntimePluginManifest } from '../plugins/runtimeManifest'
import type { PluginRuntimeOptions } from './pluginRuntime'
import { alertDialog, chooseDialog, confirmDialog, promptDialog } from './dialogs'
import { bridge } from './bridge'

const tones = new Set<PluginUiTone>(['default', 'muted', 'success', 'warning', 'danger'])

interface PluginHostContext {
  pluginId: string
  manifest: RuntimePluginManifest
  selectedInstance(): PluginInstanceRef | null
  theme(): PluginThemeContext
  removeView(id: string): boolean
  subscribe(event: PluginEventName, listenerId: string): void
  unsubscribe(listenerId: string): void
  registerCommand(definition: PluginCommandDefinition): void
  unregisterCommand(id: string): void
  registerShortcut(definition: PluginShortcutDefinition): void
  unregisterShortcut(id: string): void
  registerTheme(id: string, definition: PluginThemeDefinition): void
  unregisterTheme(id: string): void
  notify(title: string, message: string, tone: PluginUiTone, id?: string): string
  dismissNotification(id: string): boolean
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function stringArg(value: Record<string, unknown>, key: string, max = 8192): string {
  const field = value[key]
  if (typeof field !== 'string' || field.length > max) throw new TypeError(`${key} must be a string of at most ${max} characters`)
  return field
}

function optionalStringArg(value: Record<string, unknown>, key: string, fallback = '', max = 8192): string {
  return value[key] === undefined ? fallback : stringArg(value, key, max)
}

function numberArg(value: Record<string, unknown>, key: string, fallback: number): number {
  const field = value[key]
  if (field === undefined) return fallback
  if (typeof field !== 'number' || !Number.isFinite(field)) throw new TypeError(`${key} must be a finite number`)
  return field
}

function optionsArg(value: Record<string, unknown>): Record<string, unknown> {
  if (!isRecord(value.options)) throw new TypeError('options must be an object')
  return value.options
}

function serverInputArg(value: unknown): PluginServerInput {
  if (!isRecord(value)) throw new TypeError('server must be an object')
  return value as unknown as PluginServerInput
}

function serverInputsArg(value: unknown): readonly PluginServerInput[] {
  if (!Array.isArray(value) || value.length > 200 || value.some((item) => !isRecord(item))) {
    throw new TypeError('servers must be an array of at most 200 objects')
  }
  return value as PluginServerInput[]
}

function declaredDefinition(
  manifest: RuntimePluginManifest,
  value: unknown,
  field: 'actions' | 'commands' | 'shortcuts',
): { id: string } {
  if (!isRecord(value)) throw new TypeError(`${field} registration must be an object`)
  const id = stringArg(value, 'id', 96)
  const declarations: readonly { id: string }[] = manifest[field] || []
  const declaration = declarations.find((entry) => entry.id === id)
  if (!declaration) throw new Error(`${field} id is not declared in plugin.json: ${id}`)
  return declaration
}

function themeDefinition(value: unknown): PluginThemeDefinition {
  if (!isRecord(value) || Object.keys(value).some((key) => !['name', 'colors', 'tokens'].includes(key))) {
    throw new TypeError('theme definition must contain only name, colors, and tokens')
  }
  const name = optionalStringArg(value, 'name', '', 80)
  const colors: Record<string, string> = {}
  const tokens: Record<string, string | number> = {}
  if (value.colors !== undefined) {
    if (!isRecord(value.colors) || Object.keys(value.colors).length > 64) throw new TypeError('theme colors must be an object of at most 64 entries')
    for (const [key, color] of Object.entries(value.colors)) {
      if (!/^[a-z][a-z0-9_.-]{0,63}$/i.test(key) || typeof color !== 'string' || !/^#[0-9a-f]{6}$/i.test(color)) {
        throw new TypeError(`Invalid theme color: ${key}`)
      }
      colors[key] = color.toLowerCase()
    }
  }
  if (value.tokens !== undefined) {
    if (!isRecord(value.tokens) || Object.keys(value.tokens).length > 64) throw new TypeError('theme tokens must be an object of at most 64 entries')
    for (const [key, token] of Object.entries(value.tokens)) {
      if (!/^[a-z][a-z0-9_.-]{0,63}$/i.test(key)) throw new TypeError(`Invalid theme token: ${key}`)
      if (typeof token === 'string' && token.length <= 256) {
        tokens[key] = token
      } else if (typeof token === 'number' && Number.isFinite(token)) {
        tokens[key] = token
      } else {
        throw new TypeError(`Invalid theme token: ${key}`)
      }
    }
  }
  return {
    ...(name ? { name } : {}),
    ...(value.colors !== undefined ? { colors: Object.freeze(colors) } : {}),
    ...(value.tokens !== undefined ? { tokens: Object.freeze(tokens) } : {}),
  }
}

function eventName(value: string, manifest: RuntimePluginManifest): PluginEventName {
  if (!manifest.events?.includes(value as PluginEventName)) throw new Error(`Event is not declared in plugin.json: ${value}`)
  return value as PluginEventName
}

function pluginFileScope(value: Record<string, unknown>): 'plugin' | 'instance' | 'launcher-root-readonly' {
  const scope = value.scope
  if (scope !== 'plugin' && scope !== 'instance' && scope !== 'launcher-root-readonly') {
    throw new TypeError('scope must be "plugin", "instance", or "launcher-root-readonly"')
  }
  return scope
}

function selectedInstanceName(context: PluginHostContext): string {
  const selected = context.selectedInstance()
  if (!selected) throw new Error('Select an instance before using files:instance')
  return selected.id
}

async function authorize(context: PluginHostContext, permission: PluginPermission, signal?: AbortSignal): Promise<void> {
  await bridge.pluginApiAuthorize(context.pluginId, permission, signal)
  if (signal?.aborted) throw new Error('Plugin host call was cancelled')
}

async function authorizeAny(context: PluginHostContext, permissions: readonly PluginPermission[], signal?: AbortSignal): Promise<void> {
  let lastError: unknown = null
  for (const permission of permissions) {
    try {
      await authorize(context, permission, signal)
      return
    } catch (error) {
      lastError = error
    }
  }
  throw (lastError instanceof Error ? lastError : new Error(`Missing required permission: ${permissions.join(' or ')}`))
}

export function createPluginHostCall(context: PluginHostContext): PluginRuntimeOptions['hostCall'] {
  const dispatch: PluginRuntimeOptions['hostCall'] = async (operation, args, signal) => {
    if (!isRecord(args)) throw new TypeError('Plugin host-call arguments must be an object')

    switch (operation) {
      case 'runtime.getInfo':
        return bridge.pluginApiRuntimeInfo(context.pluginId, signal)
      case 'runtime.getCapabilities':
        return (await bridge.pluginApiCapabilities(context.pluginId, signal)).capabilities
      case 'runtime.getQuotaUsage':
        return bridge.pluginApiQuotaUsage(context.pluginId, signal)
      case 'runtime.assertCapability': {
        const capability = stringArg(args, 'name', 256)
        const info = await bridge.pluginApiCapabilities(context.pluginId, signal)
        if (!info.capabilities.includes(capability)) throw new Error(`Runtime capability is unavailable: ${capability}`)
        return undefined
      }
      case 'permissions.get':
        return bridge.pluginApiPermissionsGet(context.pluginId, signal)
      case 'permissions.set': {
        const permissions = args.permissions
        if (!Array.isArray(permissions) || permissions.some((item) => typeof item !== 'string')) {
          throw new TypeError('permissions must be an array of strings')
        }
        return (await bridge.pluginApiPermissionsSet(context.pluginId, permissions, signal)).permissions
      }
      case 'ui.fragment':
      case 'ui.bindPoll':
      case 'ui.unbindPoll':
        await authorize(context, 'ui:compose', signal)
        return undefined
      case 'ui.dataError':
        await authorize(context, 'ui:compose', signal)
        return context.notify('Данные плагина', optionalStringArg(args, 'message', 'UI data binding failed', 1024), 'warning', `plugin-data-error-${Date.now()}`)
      case 'ui.registerView':
      case 'ui.updateView':
        await authorize(context, 'ui:compose', signal)
        return undefined
      case 'ui.removeView': {
        await authorize(context, 'ui:compose', signal)
        return context.removeView(stringArg(args, 'id', 128))
      }
      case 'actions.register':
        await authorize(context, 'ui:compose', signal)
        declaredDefinition(context.manifest, args.definition, 'actions')
        return undefined
      case 'actions.unregister':
        return undefined
      case 'commands.register':
        await authorizeAny(context, ['commands:register', 'ui:compose'], signal)
        context.registerCommand(declaredDefinition(context.manifest, args.definition, 'commands') as PluginCommandDefinition)
        return undefined
      case 'commands.unregister':
        context.unregisterCommand(stringArg(args, 'id', 96))
        return undefined
      case 'shortcuts.register':
        await authorizeAny(context, ['shortcuts:register', 'ui:compose'], signal)
        context.registerShortcut(declaredDefinition(context.manifest, args.definition, 'shortcuts') as PluginShortcutDefinition)
        return undefined
      case 'shortcuts.unregister':
        context.unregisterShortcut(stringArg(args, 'id', 96))
        return undefined
      case 'events.on':
      case 'launcher.on': {
        await authorizeAny(context, ['events:launcher', 'lifecycle:observe'], signal)
        const event = eventName(stringArg(args, 'event', 96), context.manifest)
        context.subscribe(event, stringArg(args, 'listener_id', 96))
        return undefined
      }
      case 'events.off':
      case 'launcher.off':
        context.unsubscribe(stringArg(args, 'listener_id', 96))
        return undefined
      case 'theme.get':
        return context.theme()
      case 'theme.register':
        await authorize(context, 'ui:compose', signal)
        context.registerTheme(stringArg(args, 'registration_id', 96), themeDefinition(args.definition))
        return undefined
      case 'theme.unregister':
        context.unregisterTheme(stringArg(args, 'registration_id', 96))
        return undefined
      case 'storage.get':
        return (await bridge.pluginStorageGet(context.pluginId, stringArg(args, 'key'), signal)).value
      case 'storage.set':
        await bridge.pluginStorageSet(context.pluginId, stringArg(args, 'key'), args.value, signal)
        return undefined
      case 'storage.remove':
        return (await bridge.pluginStorageRemove(context.pluginId, stringArg(args, 'key'), signal)).removed
      case 'config.get':
        return (await bridge.pluginConfigGet(context.pluginId, stringArg(args, 'key'), signal)).value
      case 'config.set':
        await bridge.pluginConfigSet(context.pluginId, stringArg(args, 'key'), args.value, signal)
        return undefined
      case 'config.remove':
        return (await bridge.pluginConfigRemove(context.pluginId, stringArg(args, 'key'), signal)).removed
      case 'config.readProjected':
        return bridge.pluginApiConfigProjectedRead(context.pluginId, stringArg(args, 'schemaId', 120), signal)
      case 'files.list': {
        const scope = pluginFileScope(args)
        const instanceName = scope === 'instance' ? selectedInstanceName(context) : ''
        return (await bridge.pluginApiFilesList(
          context.pluginId,
          scope,
          optionalStringArg(args, 'directory'),
          instanceName,
          signal,
        )).files
      }
      case 'files.readText': {
        const scope = pluginFileScope(args)
        const instanceName = scope === 'instance' ? selectedInstanceName(context) : ''
        return (await bridge.pluginApiFilesRead(
          context.pluginId,
          scope,
          stringArg(args, 'path'),
          instanceName,
          signal,
        )).content
      }
      case 'files.writeText': {
        const scope = pluginFileScope(args)
        if (scope === 'launcher-root-readonly') {
          throw new TypeError('launcher-root-readonly is read-only; file writes are not permitted')
        }
        const instanceName = scope === 'instance' ? selectedInstanceName(context) : ''
        await bridge.pluginApiFilesWrite(
          context.pluginId,
          scope,
          stringArg(args, 'path'),
          stringArg(args, 'content', 4 * 1024 * 1024),
          instanceName,
          signal,
        )
        return undefined
      }
      case 'files.remove': {
        const scope = pluginFileScope(args)
        if (scope === 'launcher-root-readonly') {
          throw new TypeError('launcher-root-readonly is read-only; file removal is not permitted')
        }
        const instanceName = scope === 'instance' ? selectedInstanceName(context) : ''
        return (await bridge.pluginApiFilesRemove(
          context.pluginId,
          scope,
          stringArg(args, 'path'),
          instanceName,
          signal,
        )).removed
      }
      case 'database.query': {
        const parameters = args.parameters === undefined ? [] : args.parameters
        if (!Array.isArray(parameters)) throw new TypeError('parameters must be an array')
        return (await bridge.pluginDatabaseQuery(context.pluginId, stringArg(args, 'sql', 64 * 1024), parameters, signal)).rows
      }
      case 'database.execute': {
        const parameters = args.parameters === undefined ? [] : args.parameters
        if (!Array.isArray(parameters)) throw new TypeError('parameters must be an array')
        return bridge.pluginDatabaseExecute(context.pluginId, stringArg(args, 'sql', 64 * 1024), parameters, signal)
      }
      case 'http.request': {
        const request = args.request
        if (!isRecord(request)) throw new TypeError('request must be an object')
        return bridge.pluginHttpRequest(context.pluginId, request as unknown as HttpRequest, signal)
      }
      case 'network.fetchJson': {
        const request = args.request === undefined ? undefined : args.request
        if (request !== undefined && !isRecord(request)) throw new TypeError('request must be an object')
        return (await bridge.pluginNetworkFetch(
          context.pluginId,
          stringArg(args, 'url', 4096),
          request as PluginNetworkRequest | undefined,
          signal,
        )).value
      }
      case 'network.resolve':
        return bridge.pluginNetworkDns(
          context.pluginId,
          stringArg(args, 'host', 253),
          args.port === undefined ? undefined : numberArg(args, 'port', 0),
          signal,
        )
      case 'network.ping':
        return bridge.pluginNetworkPing(
          context.pluginId,
          stringArg(args, 'host', 253),
          numberArg(args, 'port', 25565),
          numberArg(args, 'timeout_ms', 3000),
          signal,
        )
      case 'network.checkPort':
        return bridge.pluginNetworkPortCheck(
          context.pluginId,
          stringArg(args, 'host', 253),
          numberArg(args, 'port', 25565),
          numberArg(args, 'timeout_ms', 3000),
          signal,
        )
      case 'instances.list':
        return (await bridge.pluginApiInstancesList(context.pluginId, signal)).instances
      case 'instances.getProcessMetrics':
        return bridge.pluginApiProcessMetrics(context.pluginId, stringArg(args, 'instanceName', 120), signal)
      case 'launcher.servers.list':
        await authorize(context, 'launcher:servers', signal)
        return (await bridge.pluginServerListGet(context.pluginId, signal)).servers
      case 'launcher.servers.save':
        await authorize(context, 'launcher:servers', signal)
        return (await bridge.pluginServerListSave(context.pluginId, serverInputsArg(args.servers), signal)).servers
      case 'launcher.servers.add':
        await authorize(context, 'launcher:servers', signal)
        return bridge.pluginServerListAdd(context.pluginId, serverInputArg(args.server), signal)
      case 'launcher.servers.remove':
        await authorize(context, 'launcher:servers', signal)
        return bridge.pluginServerListRemove(context.pluginId, stringArg(args, 'id', 80), signal)
      case 'launcher.dialogs.alert': {
        await authorize(context, 'launcher:dialogs', signal)
        const options = optionsArg(args)
        await alertDialog(stringArg(options, 'message', 2000), { title: optionalStringArg(options, 'title', 'Плагин', 120) }, signal)
        return undefined
      }
      case 'launcher.dialogs.confirm': {
        await authorize(context, 'launcher:dialogs', signal)
        const options = optionsArg(args)
        return confirmDialog(stringArg(options, 'message', 2000), {
          title: optionalStringArg(options, 'title', 'Плагин', 120),
          confirmLabel: optionalStringArg(options, 'confirm_label', 'Подтвердить', 80),
        }, signal)
      }
      case 'launcher.dialogs.prompt': {
        await authorize(context, 'launcher:dialogs', signal)
        const options = optionsArg(args)
        return (await promptDialog(stringArg(options, 'message', 2000), {
          title: optionalStringArg(options, 'title', 'Плагин', 120),
          initialValue: optionalStringArg(options, 'initial_value', '', 500),
        }, signal)) ?? null
      }
      case 'launcher.dialogs.choose': {
        await authorize(context, 'launcher:dialogs', signal)
        const options = optionsArg(args)
        if (!Array.isArray(options.choices) || options.choices.length > 100) throw new TypeError('choices must be an array of at most 100 entries')
        const choices = options.choices.map((choice) => {
          if (!isRecord(choice)) throw new TypeError('Each choice must be an object')
          return {
            value: stringArg(choice, 'value', 256),
            label: stringArg(choice, 'label', 256),
            description: optionalStringArg(choice, 'description', '', 500),
          }
        })
        const title = stringArg(options, 'title', 120)
        return (await chooseDialog(title, choices, { title: 'Выбор плагина' }, signal)) ?? null
      }
      case 'launcher.openExternal':
        await bridge.pluginApiOpenExternal(context.pluginId, stringArg(args, 'url', 4096), signal)
        return undefined
      case 'os.openUrl':
        return bridge.pluginApiOpenExternal(context.pluginId, stringArg(args, 'url', 4096), signal)
      case 'launcher.selectedInstance':
        await authorizeAny(context, ['events:launcher', 'lifecycle:observe'], signal)
        return context.selectedInstance()
      case 'proxy.listRoutes':
        return (await bridge.pluginApiProxyRoutesGet(context.pluginId, signal)).routes
      case 'proxy.saveRoutes': {
        const routes = args.routes
        if (!Array.isArray(routes)) throw new TypeError('routes must be an array')
        return (await bridge.pluginApiProxyRoutesSave(context.pluginId, routes as ProxyRoutePolicy[], signal)).routes
      }
      case 'proxy.addRoute':
        return bridge.pluginApiProxyRoutesAdd(context.pluginId, (args.route ?? {}) as Omit<ProxyRoutePolicy, 'id'> & { id?: string }, signal)
      case 'proxy.removeRoute':
        return bridge.pluginApiProxyRoutesRemove(context.pluginId, stringArg(args, 'routeId', 80), signal)
      case 'notifications.show': {
        await authorize(context, 'launcher:dialogs', signal)
        const options = optionsArg(args)
        const rawTone = optionalStringArg(options, 'tone', 'default', 16) as PluginUiTone
        if (!tones.has(rawTone)) throw new TypeError('Unsupported notification tone')
        const id = optionalStringArg(options, 'id', `plugin-${Date.now()}`, 96)
        return context.notify(
          optionalStringArg(options, 'title', 'Плагин', 120),
          stringArg(options, 'message', 1000),
          rawTone,
          id,
        )
      }
      case 'notifications.dismiss':
        await authorize(context, 'launcher:dialogs', signal)
        return context.dismissNotification(stringArg(args, 'id', 96))
      default:
        throw new Error(`Unsupported plugin host operation: ${operation}`)
    }
  }
  return async (operation: string, args: Record<string, unknown>, signal?: AbortSignal): Promise<unknown> => {
    const started = performance.now()
    await bridge.pluginOperationTrace(context.pluginId, operation, args, signal, 'begin', 0)
    let outcome: 'ok' | 'denied' = 'denied'
    try { const result = await dispatch(operation, args, signal); outcome = 'ok'; return result }
    finally { await bridge.pluginOperationTrace(context.pluginId, operation, args, undefined, outcome, performance.now() - started) }
  }
}
