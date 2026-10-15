import type { PluginManifest, PluginPermission, PluginUiFragment } from '../types'

export const NIVORA_PLUGIN_API_VERSION = '1'

export interface PluginYml {
  id: string
  name: string
  version: string
  api_version: string
  entrypoint: string
  permissions: PluginPermission[]
  sidebar_tab?: { label: string }
  network_hosts?: string[]
}

export interface PluginConfig {
  get<T>(key: string, fallback?: T): T | undefined | Promise<T | undefined>
  set<T>(key: string, value: T): void | Promise<void>
  remove(key: string): void | Promise<void>
}

export interface PluginStorage extends PluginConfig {
  namespace: string
}

export interface PluginFiles {
  list(directory?: string): Promise<string[]>
  readText(path: string): Promise<string>
  writeText(path: string, content: string): Promise<void>
  remove(path: string): Promise<void>
}

export interface PluginNetwork {
  fetchJson<T = unknown>(url: string): Promise<T>
  resolve(host: string): Promise<{ host: string; port?: number | null; addresses: Array<{ family: string; address: string }> }>
  ping(host: string, port?: number, timeoutMs?: number): Promise<{ host: string; port: number; reachable: boolean; latency_ms: number; error: string }>
  checkPort(host: string, port?: number, timeoutMs?: number): Promise<{ host: string; port: number; reachable: boolean; latency_ms: number; error: string }>
}

export interface PluginServer {
  id: string
  name: string
  address: string
  port: number
  description: string
  tags: readonly string[]
}

export interface PluginServerInput {
  id?: string
  name: string
  address: string
  port?: number
  description?: string
  tags?: readonly string[]
}

export interface PluginServerList {
  list(): Promise<PluginServer[]>
  save(servers: readonly PluginServerInput[]): Promise<PluginServer[]>
  add(server: PluginServerInput): Promise<{ server: PluginServer; servers: PluginServer[] }>
  remove(id: string): Promise<{ removed: boolean; servers: PluginServer[] }>
}

export interface PluginDatabase {
  query<T = Record<string, unknown>>(sql: string, parameters?: readonly unknown[]): Promise<T[]>
  execute(sql: string, parameters?: readonly unknown[]): Promise<{ changes: number }>
}

export interface PluginLauncherEvents {
  subscribe(event: 'launcher:ready' | 'instance:selected' | 'instance:launching' | 'instance:stopped', listener: (payload: unknown) => void): () => void
  servers: PluginServerList
}

export interface PluginUiApi {
  compose(fragment: PluginUiFragment): Readonly<PluginUiFragment>
}

export interface TrialPluginContext {
  manifest: PluginManifest
  config: PluginConfig
  storage: PluginStorage
  files: PluginFiles
  network: PluginNetwork
  database: PluginDatabase
  launcher: PluginLauncherEvents
  ui: PluginUiApi
}

export interface TrialPlugin {
  manifest: PluginYml
  activate(context: TrialPluginContext): void | (() => void) | Promise<void | (() => void)>
}

export type PluginFactory = (context: TrialPluginContext) => TrialPlugin | Promise<TrialPlugin>

export interface PluginRpc {
  configGet(pluginId: string, key: string): Promise<{ value?: unknown }>
  configSet(pluginId: string, key: string, value: unknown): Promise<{ saved: boolean }>
  configRemove(pluginId: string, key: string): Promise<{ removed: boolean }>
  storageGet(pluginId: string, key: string): Promise<{ value?: unknown }>
  storageSet(pluginId: string, key: string, value: unknown): Promise<{ saved: boolean }>
  storageRemove(pluginId: string, key: string): Promise<{ removed: boolean }>
  fileList(pluginId: string, directory?: string): Promise<{ files: string[] }>
  fileRead(pluginId: string, path: string): Promise<{ content: string }>
  fileWrite(pluginId: string, path: string, content: string): Promise<{ saved: boolean }>
  fileRemove(pluginId: string, path: string): Promise<{ removed: boolean }>
  databaseQuery(pluginId: string, sql: string, parameters?: readonly unknown[]): Promise<{ rows: Record<string, unknown>[] }>
  databaseExecute(pluginId: string, sql: string, parameters?: readonly unknown[]): Promise<{ changes: number }>
  networkFetch(pluginId: string, url: string): Promise<{ value: unknown }>
  networkDns(pluginId: string, host: string, port?: number): Promise<{ host: string; port?: number | null; addresses: Array<{ family: string; address: string }> }>
  networkPing(pluginId: string, host: string, port?: number, timeoutMs?: number): Promise<{ host: string; port: number; reachable: boolean; latency_ms: number; error: string }>
  networkPortCheck(pluginId: string, host: string, port?: number, timeoutMs?: number): Promise<{ host: string; port: number; reachable: boolean; latency_ms: number; error: string }>
  serverListGet(pluginId: string): Promise<{ servers: PluginServer[] }>
  serverListSave(pluginId: string, servers: readonly PluginServerInput[]): Promise<{ servers: PluginServer[] }>
  serverListAdd(pluginId: string, server: PluginServerInput): Promise<{ server: PluginServer; servers: PluginServer[] }>
  serverListRemove(pluginId: string, serverId: string): Promise<{ removed: boolean; servers: PluginServer[] }>
  subscribe?(event: string, listener: (payload: unknown) => void): () => void
}

export function createPluginContext(manifest: PluginManifest, rpc: PluginRpc): TrialPluginContext {
  const config: PluginConfig = {
    get: async <T>(key: string, fallback?: T) => {
      requirePermission(manifest, 'storage:workspace')
      const value = await rpc.configGet(manifest.id, key)
      return (value.value as T | undefined) ?? fallback
    },
    set: async <T>(key: string, value: T) => {
      requirePermission(manifest, 'storage:workspace')
      await rpc.configSet(manifest.id, key, value)
    },
    remove: async (key: string) => {
      requirePermission(manifest, 'storage:workspace')
      await rpc.configRemove(manifest.id, key)
    },
  }
  const storage: PluginStorage = {
    namespace: manifest.id,
    get: async <T>(key: string, fallback?: T) => {
      requirePermission(manifest, 'storage:workspace')
      const value = await rpc.storageGet(manifest.id, key)
      return (value.value as T | undefined) ?? fallback
    },
    set: async <T>(key: string, value: T) => {
      requirePermission(manifest, 'storage:workspace')
      await rpc.storageSet(manifest.id, key, value)
    },
    remove: async (key: string) => {
      requirePermission(manifest, 'storage:workspace')
      await rpc.storageRemove(manifest.id, key)
    },
  }
  return {
    manifest,
    config,
    storage,
    files: {
      list: async (directory = '') => {
        requirePermission(manifest, 'files:plugin')
        return (await rpc.fileList(manifest.id, directory)).files
      },
      readText: async (path: string) => {
        requirePermission(manifest, 'files:plugin')
        return (await rpc.fileRead(manifest.id, path)).content
      },
      writeText: async (path: string, content: string) => {
        requirePermission(manifest, 'files:plugin')
        await rpc.fileWrite(manifest.id, path, content)
      },
      remove: async (path: string) => {
        requirePermission(manifest, 'files:plugin')
        await rpc.fileRemove(manifest.id, path)
      },
    },
    network: {
      fetchJson: async <T>(url: string) => {
        requirePermission(manifest, 'network:fetch')
        return (await rpc.networkFetch(manifest.id, url)).value as T
      },
      resolve: async (host: string) => {
        requirePermission(manifest, 'network:dns')
        return rpc.networkDns(manifest.id, host)
      },
      ping: async (host: string, port = 25565, timeoutMs = 3000) => {
        requirePermission(manifest, 'network:ping')
        return rpc.networkPing(manifest.id, host, port, timeoutMs)
      },
      checkPort: async (host: string, port = 25565, timeoutMs = 3000) => {
        requirePermission(manifest, 'network:ping')
        return rpc.networkPortCheck(manifest.id, host, port, timeoutMs)
      },
    },
    database: {
      query: async <T>(sql: string, parameters: readonly unknown[] = []) => {
        requirePermission(manifest, 'storage:database')
        return (await rpc.databaseQuery(manifest.id, sql, parameters)).rows as T[]
      },
      execute: async (sql: string, parameters: readonly unknown[] = []) => {
        requirePermission(manifest, 'storage:database')
        return rpc.databaseExecute(manifest.id, sql, parameters)
      },
    },
    launcher: {
      subscribe: (event, listener) => {
        requirePermission(manifest, 'events:launcher')
        if (!rpc.subscribe) throw new Error('События лаунчера недоступны в текущем host')
        return rpc.subscribe(event, listener)
      },
      servers: {
        list: async () => {
          requirePermission(manifest, 'launcher:servers')
          return (await rpc.serverListGet(manifest.id)).servers
        },
        save: async (servers) => {
          requirePermission(manifest, 'launcher:servers')
          return (await rpc.serverListSave(manifest.id, servers)).servers
        },
        add: async (server) => {
          requirePermission(manifest, 'launcher:servers')
          return rpc.serverListAdd(manifest.id, server)
        },
        remove: async (id) => {
          requirePermission(manifest, 'launcher:servers')
          return rpc.serverListRemove(manifest.id, id)
        },
      },
    },
    ui: {
      compose: (fragment) => {
        requirePermission(manifest, 'ui:compose')
        return sanitizeFragment(fragment)
      },
    },
  }
}

export function definePlugin(factory: PluginFactory): PluginFactory {
  return factory
}

function requirePermission(manifest: PluginManifest, permission: PluginPermission) {
  if (!manifest.permissions.includes(permission)) throw new Error(`Плагину ${manifest.name} не выдано разрешение ${permission}`)
}

function sanitizeFragment(fragment: PluginUiFragment): Readonly<PluginUiFragment> {
  if (!fragment || typeof fragment !== 'object' || !['text', 'input', 'button', 'form', 'list'].includes(fragment.kind)) {
    throw new Error('Неподдерживаемый UI-фрагмент плагина')
  }
  if (fragment.kind === 'form') {
    return Object.freeze({ ...fragment, children: fragment.children.map((child) => sanitizeFragment(child)) })
  }
  return Object.freeze({ ...fragment })
}
