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

export type PluginParticlePreset =
  | 'snow'
  | 'rain'
  | 'embers'
  | 'bubbles'
  | 'stars'
  | 'fireflies'
  | 'confetti'
  | 'sparks'
  | 'dust'
  | 'hearts'

export type PluginParticleShape = 'circle' | 'square' | 'star' | 'heart' | 'diamond'

export type PluginParticleZone = 'full' | 'top' | 'bottom' | 'left' | 'right' | 'center'

export type PluginParticleLayer = 'behind' | 'above'

export type PluginParticleBlend = 'normal' | 'lighter'

export interface PluginParticleSpec {
  preset?: PluginParticlePreset
  shape?: PluginParticleShape
  image?: string
  color?: string
  count?: number
  size?: number
  sizeVariance?: number
  speed?: number
  speedVariance?: number
  direction?: number
  spread?: number
  gravity?: number
  wind?: number
  sway?: number
  opacity?: number
  lifetime?: number
  blend?: PluginParticleBlend
  zone?: PluginParticleZone
  layer?: PluginParticleLayer
  respawn?: boolean
  label?: string
}

export interface PluginParticleBurstSpec {
  x: number
  y: number
  shape?: PluginParticleShape
  image?: string
  color?: string
  colors?: readonly string[]
  count?: number
  speed?: number
  size?: number
  sizeVariance?: number
  spread?: number
  gravity?: number
  lifetime?: number
  blend?: PluginParticleBlend
  layer?: PluginParticleLayer
  label?: string
}

export type PluginStyleScope = 'window' | 'sidebar' | 'cards' | 'buttons' | 'inputs' | 'header' | 'scrollbars'

export type PluginStylePatch = Readonly<Record<string, string | number>>

export type PluginSlotName = 'sidebar.footer' | 'instances.top' | 'settings.footer'

export interface PluginSoundOptions {
  volume?: number
  loop?: boolean
  rate?: number
}

export interface PluginCursorSpec {
  image: string
  hotspotX?: number
  hotspotY?: number
  size?: number
}

export interface PluginWallpaperSpec {
  image: string
  blur?: number
  dim?: number
  fit?: 'cover' | 'contain' | 'tile'
}

export interface PluginGradientSpec {
  from: string
  to: string
  angle?: number
}

export interface PluginEffectSpec {
  panelGlow?: string | null
  panelBlur?: number | null
  gradient?: PluginGradientSpec | null
  vignette?: number | null
  noise?: number | null
  scanlines?: boolean | null
  shimmer?: boolean | null
}

export type PluginHudPosition = 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right' | 'top-center' | 'bottom-center'

export interface PluginHudOptions {
  position?: PluginHudPosition
  width?: number
  margin?: number
}

export type PluginTransitionPreset = 'instant' | 'swift' | 'smooth' | 'cinematic'

export type PluginTokenOverrides = Readonly<Record<string, string | number>>

export interface PluginThemeContext {
  mode: string
  name?: string
  accent: string
  tokens: Readonly<Record<string, string | number>>
  colors: Readonly<Record<string, string>>
  locale: string
  buttonStyle: Readonly<Record<string, unknown>>
  buttonLabels: Readonly<Record<string, string>>
  fonts: Readonly<{ ui: string; button: string; available: readonly string[] }>
}

export interface PluginThemeApi {
  get(): Promise<PluginThemeContext>
  style: {
    set(scope: PluginStyleScope, patch: PluginStylePatch): Promise<void>
    reset(scope?: PluginStyleScope): Promise<void>
  }
  slots: {
    set(slot: PluginSlotName, node: unknown): Promise<void>
    reset(slot?: PluginSlotName): Promise<void>
  }
  particles: {
    spawn(spec: PluginParticleSpec): Promise<{ id: string }>
    burst(spec: PluginParticleBurstSpec): Promise<{ id: string }>
    stop(id: string): Promise<void>
    stopAll(): Promise<void>
    active(): Promise<string[]>
  }
  sounds: {
    register(name: string, audio: string): Promise<void>
    play(name: string, options?: PluginSoundOptions): Promise<void>
    list(): Promise<string[]>
    unregister(name: string): Promise<void>
  }
  cursor: {
    set(spec: PluginCursorSpec): Promise<void>
    reset(): Promise<void>
  }
  wallpaper: {
    set(spec: PluginWallpaperSpec): Promise<void>
    reset(): Promise<void>
  }
  effects: {
    set(spec: PluginEffectSpec): Promise<void>
    reset(): Promise<void>
  }
  hud: {
    add(node: unknown, options?: PluginHudOptions): Promise<{ id: string }>
    remove(id: string): Promise<void>
    list(): Promise<Array<{ id: string; position: PluginHudPosition }>>
  }
  transitions: {
    set(preset: PluginTransitionPreset): Promise<void>
    reset(): Promise<void>
  }
  fonts: {
    register(name: string, font: string): Promise<{ family: string }>
    unregister(name: string): Promise<void>
    list(): Promise<Array<{ name: string; family: string }>>
  }
  tokens: {
    set(overrides: PluginTokenOverrides): Promise<void>
    reset(): Promise<void>
  }
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
  theme: PluginThemeApi
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
  themeCall?(pluginId: string, operation: string, args?: Readonly<Record<string, unknown>>): Promise<unknown>
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
    theme: createPluginThemeApi(manifest, rpc),
  }
}

function createPluginThemeApi(manifest: PluginManifest, rpc: PluginRpc): PluginThemeApi {
  const call = (operation: string, args: Record<string, unknown> = {}) => {
    if (!rpc.themeCall) throw new Error('Тема-эффекты недоступны в текущем host')
    return rpc.themeCall(manifest.id, operation, args)
  }
  const requireTheme = () => requirePermission(manifest, 'theme:effects')
  const requireCompose = () => requirePermission(manifest, 'ui:compose')
  return {
    get: async () => await call('theme.get') as PluginThemeContext,
    style: {
      set: async (scope, patch) => { requireTheme(); await call('theme.style.set', { scope, patch }) },
      reset: async (scope) => { requireTheme(); await call('theme.style.reset', { scope }) },
    },
    slots: {
      set: async (slot: PluginSlotName, node: unknown) => { requireCompose(); await call('theme.slots.set', { slot, view: node }) },
      reset: async (slot?: PluginSlotName) => { requireCompose(); await call('theme.slots.reset', { slot }) },
    },
    particles: {
      spawn: async (spec) => { requireTheme(); return await call('theme.particles.spawn', { spec }) as { id: string } },
      burst: async (spec) => { requireTheme(); return await call('theme.particles.burst', { spec }) as { id: string } },
      stop: async (id) => { requireTheme(); await call('theme.particles.stop', { id }) },
      stopAll: async () => { requireTheme(); await call('theme.particles.stopAll', {}) },
      active: async () => { requireTheme(); return await call('theme.particles.active', {}) as string[] },
    },
    sounds: {
      register: async (name, audio) => { requireTheme(); await call('theme.sounds.register', { name, audio }) },
      play: async (name, options) => { requireTheme(); await call('theme.sounds.play', { name, options: options || {} }) },
      list: async () => { requireTheme(); return await call('theme.sounds.list', {}) as string[] },
      unregister: async (name) => { requireTheme(); await call('theme.sounds.unregister', { name }) },
    },
    cursor: {
      set: async (spec) => { requireTheme(); await call('theme.cursor.set', { spec }) },
      reset: async () => { requireTheme(); await call('theme.cursor.reset', {}) },
    },
    wallpaper: {
      set: async (spec) => { requireTheme(); await call('theme.wallpaper.set', { spec }) },
      reset: async () => { requireTheme(); await call('theme.wallpaper.reset', {}) },
    },
    effects: {
      set: async (spec) => { requireTheme(); await call('theme.effects.set', { spec }) },
      reset: async () => { requireTheme(); await call('theme.effects.reset', {}) },
    },
    hud: {
      add: async (node, options) => { requireCompose(); return await call('theme.hud.add', { view: node, options: options || {} }) as { id: string } },
      remove: async (id) => { requireCompose(); await call('theme.hud.remove', { id }) },
      list: async () => { requireCompose(); return await call('theme.hud.list', {}) as Array<{ id: string; position: PluginHudPosition }> },
    },
    transitions: {
      set: async (preset) => { requireTheme(); await call('theme.transitions.set', { preset }) },
      reset: async () => { requireTheme(); await call('theme.transitions.reset', {}) },
    },
    fonts: {
      register: async (name, font) => { requireTheme(); return await call('theme.fonts.register', { name, font }) as { family: string } },
      unregister: async (name) => { requireTheme(); await call('theme.fonts.unregister', { name }) },
      list: async () => { requireTheme(); return await call('theme.fonts.list', {}) as Array<{ name: string; family: string }> },
    },
    tokens: {
      set: async (overrides) => { requireTheme(); await call('theme.tokens.set', { overrides }) },
      reset: async () => { requireTheme(); await call('theme.tokens.reset', {}) },
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
