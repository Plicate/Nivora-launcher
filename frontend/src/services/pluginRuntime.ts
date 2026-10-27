import type {
  PluginEventName,
  PluginEventPayloadMap,
  PluginJsonValue,
  PluginUiNode,
  PluginUiView,
} from '../../../plugins/api'
import type { RuntimePluginManifest } from '../plugins/runtimeManifest'

const MAX_TIMEOUT_MS = 5000
const MAX_DIALOG_TIMEOUT_MS = 5 * 60 * 1000
const MAX_SOURCE_BYTES = 4 * 1024 * 1024
const MAX_RPC_BYTES = 4 * 1024 * 1024
const MAX_VIEW_BYTES = 256 * 1024
const MAX_ERROR_BYTES = 8 * 1024
const MAX_PENDING_HOST_CALLS = 32
const MAX_UI_NODES = 512
const MAX_UI_COLLECTION_ITEMS = 512
const MAX_UI_TREE_DEPTH = 10
const MAX_UI_STRING_CHARS = 8192
const UI_TONES = new Set(['default', 'muted', 'success', 'warning', 'danger'])
const UI_SLOTS = new Set(['sidebar_tab', 'instance_tab', 'settings_section', 'home_widget'])

export interface PluginRuntimeOptions {
  hostCall(operation: string, args: Readonly<Record<string, unknown>>, signal?: AbortSignal): Promise<unknown> | unknown
  onView(view: PluginUiView): void
  onError(error: Error): void
}

type WorkerRequest =
  | { type: 'start'; manifestJson: string; source: string }
  | { type: 'event'; event: string; payloadJson: string }
  | { type: 'action'; actionId: string; inputJson: string }
  | { type: 'command'; commandId: string; argumentsJson: string }
  | { type: 'shortcut'; shortcutId: string }
  | { type: 'stop' }

type WorkerResponse =
  | { type: 'host-call'; id: string; requestId: string; operation: string; argsJson: string }
  | { type: 'host-call-result'; id: string; replyJson: string }
  | { type: 'result'; requestId: string; ok: true; resultJson: string; status?: 'started' | 'stopped' }
  | { type: 'result'; requestId: string; ok: false; errorJson: string; fatal?: boolean; status?: 'stopped' }
  | { type: 'fatal'; errorJson: string }
  | { type: 'error'; errorJson: string }

type PendingRequest = {
  resolve(value: unknown): void
  reject(error: Error): void
  timer: number
  type: WorkerRequest['type']
  timeoutMs: number
  dialogDeadlines: Map<string, number>
}

type PendingHostCall = {
  timer: number
  requestId: string
  dialogDeadline?: number
  cancel(error: Error): void
  controller: AbortController
}

type RuntimeRecord = {
  pluginId: string
  runtimeId: string
  worker: Worker
  options: PluginRuntimeOptions
  state: 'starting' | 'running' | 'stopping'
  terminated: boolean
  polls: Map<string, ReturnType<typeof setInterval>>
  meter: Array<{ at: number; bytes: number }>
  nextRequestId: number
  pendingRequests: Map<string, PendingRequest>
  pendingHostCalls: Map<string, PendingHostCall>
}

const byteLength = (value: string) => new TextEncoder().encode(value).byteLength
const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

type UiValidationBudget = { nodes: number; collectionItems: number }

function hasOnlyKeys(value: Record<string, unknown>, allowed: readonly string[]): boolean {
  return Object.keys(value).every((key) => allowed.includes(key))
}

function isStringField(value: Record<string, unknown>, key: string, required = false): boolean {
  const field = value[key]
  return field === undefined
    ? !required
    : typeof field === 'string' && field.length <= MAX_UI_STRING_CHARS
}

function isBooleanField(value: Record<string, unknown>, key: string): boolean {
  const field = value[key]
  return field === undefined || typeof field === 'boolean'
}

function isNumberField(value: Record<string, unknown>, key: string, required = false): boolean {
  const field = value[key]
  return field === undefined
    ? !required
    : typeof field === 'number' && Number.isFinite(field)
}

function isToneField(value: Record<string, unknown>): boolean {
  const tone = value.tone
  return tone === undefined || (typeof tone === 'string' && UI_TONES.has(tone))
}

function consumeUiArray(value: unknown, budget: UiValidationBudget): value is unknown[] {
  if (!Array.isArray(value) || value.length > MAX_UI_COLLECTION_ITEMS) return false
  if (budget.collectionItems + value.length > MAX_UI_COLLECTION_ITEMS) return false
  budget.collectionItems += value.length
  return true
}

function isUiOption(value: unknown): boolean {
  return isRecord(value)
    && hasOnlyKeys(value, ['value', 'label', 'description', 'disabled'])
    && isStringField(value, 'value', true)
    && isStringField(value, 'label', true)
    && isStringField(value, 'description')
    && isBooleanField(value, 'disabled')
}

function isUiListItem(value: unknown): boolean {
  if (typeof value === 'string') return value.length <= MAX_UI_STRING_CHARS
  return isRecord(value)
    && hasOnlyKeys(value, ['id', 'label', 'detail', 'badge', 'action'])
    && isStringField(value, 'id', true)
    && isStringField(value, 'label', true)
    && isStringField(value, 'detail')
    && isStringField(value, 'badge')
    && isStringField(value, 'action')
        && isStringField(value, 'onChange')
}

function isUiNode(value: unknown, budget: UiValidationBudget, depth = 0): value is PluginUiNode {
  if (!isRecord(value) || depth > MAX_UI_TREE_DEPTH || budget.nodes >= MAX_UI_NODES) return false
  budget.nodes += 1

  switch (value.type) {
    case 'tabs':
    case 'accordion': {
      const key = value.type === 'tabs' ? 'tabs' : 'sections'
      const entries = value[key]
      return hasOnlyKeys(value, ['type', 'id', key]) && isStringField(value, 'id', true)
        && consumeUiArray(entries, budget) && entries.every((item) => isRecord(item)
          && hasOnlyKeys(item, ['id', 'label', 'content']) && isStringField(item, 'id', true)
          && isStringField(item, 'label', true) && isUiNode(item.content, budget, depth + 1))
    }
    case 'form': {
      const fields = value.fields
      return hasOnlyKeys(value, ['type', 'id', 'fields', 'submitAction']) && isStringField(value, 'id', true)
        && isStringField(value, 'submitAction', true) && consumeUiArray(fields, budget)
        && fields.every((item) => isUiNode(item, budget, depth + 1))
    }
    case 'table': {
      const columns = value.columns, rows = value.rows
      return hasOnlyKeys(value, ['type', 'id', 'columns', 'rows']) && isStringField(value, 'id', true)
        && consumeUiArray(columns, budget) && columns.length <= 32
        && columns.every((column) => isRecord(column) && hasOnlyKeys(column, ['key', 'label'])
          && isStringField(column, 'key', true) && isStringField(column, 'label', true))
        && consumeUiArray(rows, budget) && rows.every((row) => isRecord(row) && Object.keys(row).length <= 32
          && Object.values(row).every((cell) => typeof cell === 'string' && cell.length <= MAX_UI_STRING_CHARS || typeof cell === 'number' && Number.isFinite(cell)))
    }
    case 'chart': {
      const values = value.values
      return hasOnlyKeys(value, ['type', 'id', 'label', 'values']) && isStringField(value, 'id', true)
        && isStringField(value, 'label') && consumeUiArray(values, budget)
        && values.every((item) => typeof item === 'number' && Number.isFinite(item))
    }
    case 'text':
      return hasOnlyKeys(value, ['type', 'id', 'text', 'tone'])
        && isStringField(value, 'id')
        && isStringField(value, 'text', true)
        && isToneField(value)
    case 'badge':
      return hasOnlyKeys(value, ['type', 'id', 'label', 'tone'])
        && isStringField(value, 'id')
        && isStringField(value, 'label', true)
        && isToneField(value)
    case 'button':
      return hasOnlyKeys(value, ['type', 'id', 'label', 'action', 'disabled', 'tone'])
        && isStringField(value, 'id', true)
        && isStringField(value, 'label', true)
        && isStringField(value, 'action', true)
        && isBooleanField(value, 'disabled')
        && isToneField(value)
    case 'input':
      return hasOnlyKeys(value, ['type', 'id', 'label', 'value', 'placeholder', 'config_key', 'action', 'onChange'])
        && isStringField(value, 'id', true)
        && isStringField(value, 'label', true)
        && isStringField(value, 'value')
        && isStringField(value, 'placeholder')
        && isStringField(value, 'config_key')
        && isStringField(value, 'action')
        && isStringField(value, 'onChange')
    case 'textarea':
      return hasOnlyKeys(value, ['type', 'id', 'label', 'value', 'placeholder', 'rows', 'config_key', 'action', 'onChange'])
        && isStringField(value, 'id', true)
        && isStringField(value, 'label', true)
        && isStringField(value, 'value')
        && isStringField(value, 'placeholder')
        && isNumberField(value, 'rows')
        && isStringField(value, 'config_key')
        && isStringField(value, 'action')
        && isStringField(value, 'onChange')
    case 'datetime':
      return hasOnlyKeys(value, ['type', 'id', 'label', 'value', 'config_key', 'action', 'onChange'])
        && isStringField(value, 'id', true)
        && isStringField(value, 'label', true)
        && isStringField(value, 'value')
        && isStringField(value, 'config_key')
        && isStringField(value, 'action')
        && isStringField(value, 'onChange')
    case 'color':
      return hasOnlyKeys(value, ['type', 'id', 'label', 'value', 'config_key', 'onChange'])
        && isStringField(value, 'id', true)
        && isStringField(value, 'label', true)
        && (value.value === undefined || (typeof value.value === 'string' && /^#[0-9AaBbCcDdEeFf]{6}$/.test(value.value)))
        && isStringField(value, 'config_key')
        && isStringField(value, 'onChange')
    case 'number':
    case 'slider': {
      const isRange = value.type === 'slider'
      return hasOnlyKeys(value, ['type', 'id', 'label', 'value', 'min', 'max', 'step', 'unit', 'config_key', 'action', 'onChange'])
        && isStringField(value, 'id', true)
        && isStringField(value, 'label', true)
        && isNumberField(value, 'value')
        && isNumberField(value, 'min', isRange)
        && isNumberField(value, 'max', isRange)
        && isNumberField(value, 'step')
        && isStringField(value, 'unit')
        && isStringField(value, 'config_key')
        && isStringField(value, 'action')
        && isStringField(value, 'onChange')
        && (value.min === undefined || value.max === undefined || Number(value.min) <= Number(value.max))
    }
    case 'select': {
      const options = value.options
      return hasOnlyKeys(value, ['type', 'id', 'label', 'options', 'value', 'config_key', 'action', 'onChange'])
        && isStringField(value, 'id', true)
        && isStringField(value, 'label', true)
        && isStringField(value, 'value')
        && isStringField(value, 'config_key')
        && isStringField(value, 'action')
        && isStringField(value, 'onChange')
        && consumeUiArray(options, budget)
        && options.every(isUiOption)
    }
    case 'checkbox':
      return hasOnlyKeys(value, ['type', 'id', 'label', 'checked', 'config_key', 'action', 'onChange'])
        && isStringField(value, 'id', true)
        && isStringField(value, 'label', true)
        && isBooleanField(value, 'checked')
        && isStringField(value, 'config_key')
        && isStringField(value, 'action')
        && isStringField(value, 'onChange')
    case 'switch':
      return hasOnlyKeys(value, ['type', 'id', 'label', 'checked', 'hint', 'config_key', 'action', 'onChange'])
        && isStringField(value, 'id', true)
        && isStringField(value, 'label', true)
        && isBooleanField(value, 'checked')
        && isStringField(value, 'hint')
        && isStringField(value, 'config_key')
        && isStringField(value, 'action')
        && isStringField(value, 'onChange')
    case 'metric':
      return hasOnlyKeys(value, ['type', 'id', 'label', 'value', 'unit', 'tone'])
        && isStringField(value, 'id')
        && isStringField(value, 'label', true)
        && isStringField(value, 'value', true)
        && isStringField(value, 'unit')
        && isToneField(value)
    case 'alert':
      return hasOnlyKeys(value, ['type', 'id', 'title', 'text', 'tone'])
        && isStringField(value, 'id')
        && isStringField(value, 'title')
        && isStringField(value, 'text', true)
        && isToneField(value)
    case 'code':
      return hasOnlyKeys(value, ['type', 'id', 'text'])
        && isStringField(value, 'id')
        && isStringField(value, 'text', true)
    case 'image':
      return hasOnlyKeys(value, ['type', 'id', 'url', 'alt', 'height'])
        && isStringField(value, 'id')
        && isStringField(value, 'url', true)
        && isStringField(value, 'alt')
        && isNumberField(value, 'height')
    case 'progress':
      return hasOnlyKeys(value, ['type', 'id', 'value', 'max', 'label'])
        && isStringField(value, 'id')
        && isNumberField(value, 'value', true)
        && isNumberField(value, 'max')
        && isStringField(value, 'label')
    case 'list': {
      const items = value.items
      return hasOnlyKeys(value, ['type', 'id', 'items'])
        && isStringField(value, 'id')
        && consumeUiArray(items, budget)
        && items.every(isUiListItem)
    }
    case 'link':
      return hasOnlyKeys(value, ['type', 'id', 'label', 'url', 'action'])
        && isStringField(value, 'id')
        && isStringField(value, 'label', true)
        && isStringField(value, 'url', true)
        && isStringField(value, 'action')
        && isStringField(value, 'onChange')
    case 'divider':
      return hasOnlyKeys(value, ['type', 'id']) && isStringField(value, 'id')
    case 'group': {
      const children = value.children
      const direction = value.direction
      return hasOnlyKeys(value, ['type', 'id', 'label', 'direction', 'children'])
        && isStringField(value, 'id')
        && isStringField(value, 'label')
        && (direction === undefined || direction === 'vertical' || direction === 'horizontal')
        && consumeUiArray(children, budget)
        && children.every((child) => isUiNode(child, budget, depth + 1))
    }
    case 'card': {
      const children = value.children
      return hasOnlyKeys(value, ['type', 'id', 'label', 'tone', 'children'])
        && isStringField(value, 'id')
        && isStringField(value, 'label')
        && isToneField(value)
        && consumeUiArray(children, budget)
        && children.every((child) => isUiNode(child, budget, depth + 1))
    }
    case 'grid': {
      const children = value.children
      const columns = value.columns
      return hasOnlyKeys(value, ['type', 'id', 'columns', 'children'])
        && isStringField(value, 'id')
        && (columns === undefined || (typeof columns === 'number' && Number.isInteger(columns) && columns >= 1 && columns <= 4))
        && consumeUiArray(children, budget)
        && children.every((child) => isUiNode(child, budget, depth + 1))
    }
    default:
      return false
  }
}

function jsonString(value: unknown, maxBytes: number, label: string): string {
  let result: string | undefined
  try {
    result = JSON.stringify(value)
  } catch {
    throw new Error(`${label} is not JSON serializable`)
  }
  if (typeof result !== 'string') throw new Error(`${label} is not JSON serializable`)
  if (byteLength(result) > maxBytes) throw new Error(`${label} exceeds the size limit`)
  return result
}

function toError(value: unknown): Error {
  if (value instanceof Error) return value
  if (typeof value === 'string') return new Error(value)
  if (isRecord(value)) {
    const error = new Error(typeof value.message === 'string' ? value.message : 'Plugin runtime failed')
    if (typeof value.name === 'string') error.name = value.name
    return error
  }
  return new Error('Plugin runtime failed')
}

function errorJson(value: unknown): string {
  const error = toError(value)
  let message = error.message
  let result = JSON.stringify({ name: error.name.slice(0, 128), message }) || '{"name":"Error","message":"Plugin runtime failed"}'
  while (byteLength(result) > MAX_ERROR_BYTES && message.length > 0) {
    message = message.slice(0, Math.floor(message.length / 2))
    result = JSON.stringify({ name: error.name.slice(0, 128), message }) || '{"name":"Error","message":"Plugin runtime failed"}'
  }
  return result
}

function errorFromJson(value: unknown): Error {
  if (typeof value !== 'string' || byteLength(value) > MAX_ERROR_BYTES) return new Error('Plugin runtime failed')
  try {
    return toError(JSON.parse(value))
  } catch {
    return new Error('Plugin runtime failed')
  }
}

function hostReply(ok: boolean, value: unknown): string {
  if (!ok) return JSON.stringify({ ok: false, error: JSON.parse(errorJson(value)) }) || '{"ok":false}'
  try {
    const result = JSON.stringify(value === undefined
      ? { ok: true, hasValue: false }
      : { ok: true, hasValue: true, value })
    if (typeof result !== 'string' || byteLength(result) > MAX_RPC_BYTES) {
      throw new Error('Host reply exceeds the size limit')
    }
    return result
  } catch (error) {
    return JSON.stringify({ ok: false, error: JSON.parse(errorJson(error)) }) || '{"ok":false}'
  }
}

function isUiView(value: unknown): value is PluginUiView {
  if (!isRecord(value)
    || !hasOnlyKeys(value, ['id', 'slot', 'title', 'description', 'icon', 'order', 'root'])
    || !isStringField(value, 'id', true)
    || value.id === ''
    || typeof value.slot !== 'string'
    || !UI_SLOTS.has(value.slot)
    || !isStringField(value, 'title', true)
    || !isStringField(value, 'description')
    || !isStringField(value, 'icon')
    || !isNumberField(value, 'order')) return false
  return isUiNode(value.root, { nodes: 0, collectionItems: 0 })
}

// экспортировано для тестов контракта интерфейса версии 2
export function isUiViewForTests(value: unknown): boolean {
  return isUiView(value)
}

function timeoutValue(timeoutMs: number | undefined): number {
  if (timeoutMs === undefined || !Number.isFinite(timeoutMs)) return MAX_TIMEOUT_MS
  return Math.max(1, Math.min(MAX_TIMEOUT_MS, Math.floor(timeoutMs)))
}

function makeId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') return crypto.randomUUID()
  return `plugin-runtime-${Date.now()}-${Math.random().toString(36).slice(2)}`
}

export class PluginRuntimeService {
  private readonly runtimes = new Map<string, RuntimeRecord>()
  private disposed = false

  async start(
    pluginId: string,
    manifest: RuntimePluginManifest,
    source: string,
    options: PluginRuntimeOptions,
  ): Promise<void> {
    if (this.disposed) throw new Error('Plugin runtime service has been disposed')
    if (!pluginId || pluginId !== manifest.id) throw new Error('Plugin runtime id must match the manifest id')
    if (typeof source !== 'string' || byteLength(source) > MAX_SOURCE_BYTES) throw new Error('Plugin entrypoint exceeds the size limit')
    if (!options || typeof options.hostCall !== 'function' || typeof options.onView !== 'function' || typeof options.onError !== 'function') {
      throw new Error('Plugin runtime callbacks are required')
    }
    const manifestJson = jsonString(manifest, MAX_RPC_BYTES, 'Plugin manifest')

    if (this.runtimes.has(pluginId)) await this.stop(pluginId)
    if (this.disposed) throw new Error('Plugin runtime service has been disposed')

    const worker = new Worker(new URL('../plugins/quickjs.worker.ts', import.meta.url), {
      type: 'module',
      name: `plugin-${pluginId}`,
    })
    const record: RuntimeRecord = {
      pluginId,
      runtimeId: makeId(),
      worker,
      options,
      state: 'starting',
      terminated: false,
      polls: new Map(),
      meter: [],
      nextRequestId: 0,
      pendingRequests: new Map(),
      pendingHostCalls: new Map(),
    }
    this.runtimes.set(pluginId, record)
    worker.onmessage = (event: MessageEvent<WorkerResponse>) => this.handleMessage(record, event.data)
    worker.onerror = (event: ErrorEvent) => {
      event.preventDefault()
      this.failRuntime(record, new Error(event.message || 'Plugin worker failed'), true)
    }
    worker.onmessageerror = () => this.failRuntime(record, new Error('Plugin worker sent an invalid message'), true)

    try {
      await this.request(record, { type: 'start', manifestJson, source }, MAX_TIMEOUT_MS)
      if (record.terminated) throw new Error(`Plugin runtime stopped during startup: ${pluginId}`)
      record.state = 'running'
    } catch (error) {
      this.failRuntime(record, toError(error), false)
      throw toError(error)
    }
  }

  async emit<K extends PluginEventName>(
    pluginId: string,
    event: K,
    payload: PluginEventPayloadMap[K],
    timeoutMs = MAX_TIMEOUT_MS,
  ): Promise<void> {
    const record = this.requireRuntime(pluginId)
    const payloadJson = jsonString(payload, MAX_RPC_BYTES, 'Plugin event payload')
    await this.request(record, { type: 'event', event, payloadJson }, timeoutValue(timeoutMs))
  }

  async runAction(
    pluginId: string,
    actionId: string,
    input?: PluginJsonValue,
    timeoutMs = MAX_TIMEOUT_MS,
  ): Promise<PluginJsonValue | void> {
    const record = this.requireRuntime(pluginId)
    if (!actionId) throw new Error('Action id is required')
    const inputJson = jsonString(input === undefined ? null : input, MAX_RPC_BYTES, 'Plugin action input')
    return await this.request(record, { type: 'action', actionId, inputJson }, timeoutValue(timeoutMs)) as PluginJsonValue | void
  }

  async runCommand(pluginId: string, commandId: string, arguments_: Readonly<Record<string, string>>): Promise<void> {
    const record = this.requireRuntime(pluginId)
    if (!commandId) throw new Error('Command id is required')
    const argumentsJson = jsonString({ ...arguments_ }, MAX_RPC_BYTES, 'Plugin command arguments')
    await this.request(record, { type: 'command', commandId, argumentsJson }, MAX_TIMEOUT_MS)
  }

  async runShortcut(pluginId: string, shortcutId: string): Promise<void> {
    const record = this.requireRuntime(pluginId)
    if (!shortcutId) throw new Error('Shortcut id is required')
    await this.request(record, { type: 'shortcut', shortcutId }, MAX_TIMEOUT_MS)
  }

  async stop(pluginId: string): Promise<void> {
    const record = this.runtimes.get(pluginId)
    if (!record || record.terminated) return
    record.state = 'stopping'
    try {
      await this.request(record, { type: 'stop' }, MAX_TIMEOUT_MS)
    } catch (error) {
      if (!record.terminated) this.notifyError(record, toError(error))
    } finally {
      this.terminateRuntime(record, new Error(`Plugin runtime stopped: ${pluginId}`), false)
    }
  }

  async dispose(): Promise<void> {
    this.disposed = true
    await Promise.allSettled([...this.runtimes.keys()].map((pluginId) => this.stop(pluginId)))
    for (const record of this.runtimes.values()) {
      this.terminateRuntime(record, new Error('Plugin runtime service disposed'), false)
    }
    this.runtimes.clear()
  }

  runtimeIds(): string[] {
    return [...this.runtimes.values()].filter((runtime) => !runtime.terminated).map((runtime) => runtime.pluginId)
  }

  private requireRuntime(pluginId: string): RuntimeRecord {
    const record = this.runtimes.get(pluginId)
    if (!record || record.terminated || record.state !== 'running') throw new Error(`Plugin runtime is not active: ${pluginId}`)
    return record
  }

  private request(record: RuntimeRecord, message: WorkerRequest, timeoutMs: number): Promise<unknown> {
    if (record.terminated) return Promise.reject(new Error(`Plugin runtime is unavailable: ${record.pluginId}`))
    const requestId = `${record.runtimeId}:${++record.nextRequestId}`
    return new Promise((resolve, reject) => {
      record.pendingRequests.set(requestId, {
        resolve,
        reject,
        timer: 0,
        type: message.type,
        timeoutMs: timeoutValue(timeoutMs),
        dialogDeadlines: new Map(),
      })
      this.armRequestTimeout(record, requestId, timeoutValue(timeoutMs))
      try {
        record.worker.postMessage({ ...message, requestId })
      } catch (error) {
        this.failRuntime(record, toError(error), true)
      }
    })
  }

  private handleMessage(record: RuntimeRecord, message: WorkerResponse) {
    if (record.terminated || !message || typeof message !== 'object' || typeof message.type !== 'string') return
    if (message.type === 'host-call') {
      void this.handleHostCall(record, message)
      return
    }
    if (message.type === 'fatal') {
      this.failRuntime(record, errorFromJson(message.errorJson), true)
      return
    }
    if (message.type === 'error') {
      this.notifyError(record, errorFromJson(message.errorJson))
      return
    }
    if (message.type === 'host-call-result') return
    if (message.type !== 'result') {
      this.failRuntime(record, new Error('Plugin worker sent an unknown message'), true)
      return
    }

    const pending = record.pendingRequests.get(message.requestId)
    if (!pending) return
    if (!message.ok && message.fatal) {
      this.failRuntime(record, errorFromJson(message.errorJson), true)
      return
    }
    record.pendingRequests.delete(message.requestId)
    clearTimeout(pending.timer)
    if (!message.ok) {
      const error = errorFromJson(message.errorJson)
      if (pending.type !== 'stop') this.notifyError(record, error)
      pending.reject(error)
      return
    }
    if (message.status === 'started') record.state = 'running'
    if (message.status === 'stopped') record.state = 'stopping'
    if (typeof message.resultJson !== 'string' || byteLength(message.resultJson) > MAX_RPC_BYTES) {
      const error = new Error('Plugin worker result exceeds the size limit')
      this.failRuntime(record, error, true)
      return
    }
    try {
      pending.resolve(JSON.parse(message.resultJson))
    } catch {
      this.failRuntime(record, new Error('Plugin worker returned invalid JSON'), true)
    }
  }

  private async handleHostCall(
    record: RuntimeRecord,
    message: Extract<WorkerResponse, { type: 'host-call' }>,
  ) {
    if (!message.id || typeof message.requestId !== 'string'
      || typeof message.operation !== 'string' || typeof message.argsJson !== 'string') {
      this.failRuntime(record, new Error('Plugin worker sent an invalid host call'), true)
      return
    }
    if (record.pendingHostCalls.has(message.id)) {
      this.failRuntime(record, new Error('Plugin worker reused a host-call id'), true)
      return
    }
    if (record.pendingHostCalls.size >= MAX_PENDING_HOST_CALLS) {
      this.sendHostCallReply(record, message.id, hostReply(false, new Error('Too many pending plugin host calls')))
      return
    }
    if (byteLength(message.argsJson) > MAX_RPC_BYTES || !/^[a-z][a-z0-9]*(?:\.[a-z][a-z0-9]*)+$/i.test(message.operation)) {
      this.failRuntime(record, new Error('Plugin worker sent an invalid host-call payload'), true)
      return
    }
    const now = performance.now()
    record.meter = record.meter.filter((item) => now - item.at < 60000)
    const bytes = byteLength(message.argsJson)
    if (record.meter.length >= 180 || record.meter.reduce((sum, item) => sum + item.bytes, 0) + bytes > 8 * 1024 * 1024) {
      this.sendHostCallReply(record, message.id, hostReply(false, new Error('Plugin host-call rate/data limit exceeded')))
      return
    }
    const meterEntry = { at: now, bytes }
    record.meter.push(meterEntry)
    let args: unknown
    try {
      args = JSON.parse(message.argsJson)
    } catch {
      this.failRuntime(record, new Error('Plugin worker sent invalid host-call JSON'), true)
      return
    }
    if (!isRecord(args)) {
      this.failRuntime(record, new Error('Plugin worker host-call arguments must be an object'), true)
      return
    }
    let viewToPublish: PluginUiView | undefined
    if (message.operation === 'ui.registerView' || message.operation === 'ui.updateView') {
      let viewJson: string | undefined
      try {
        if (isUiView(args.view)) {
          viewJson = JSON.stringify(args.view)
          if (typeof viewJson === 'string' && byteLength(viewJson) <= MAX_VIEW_BYTES) {
            viewToPublish = JSON.parse(viewJson) as PluginUiView
          }
        }
      } catch {
        viewToPublish = undefined
      }
      if (!viewToPublish) {
        this.sendHostCallReply(record, message.id, hostReply(false, new Error('Plugin view is invalid, too complex, or exceeds the size limit')))
        return
      }
    }

    let cancel!: (error: Error) => void
    const cancelled = new Promise<never>((_resolve, reject) => { cancel = reject })
    const controller = new AbortController()
    const dialogDeadline = message.operation.startsWith('launcher.dialogs.')
      ? Date.now() + MAX_DIALOG_TIMEOUT_MS
      : undefined
    if (dialogDeadline !== undefined) {
      const request = record.pendingRequests.get(message.requestId)
      request?.dialogDeadlines.set(message.id, dialogDeadline)
      this.armRequestTimeout(record, message.requestId, dialogDeadline - Date.now())
    }
    const hostCallTimeout = dialogDeadline === undefined ? MAX_TIMEOUT_MS : MAX_DIALOG_TIMEOUT_MS
    const timer = setTimeout(() => {
      this.failRuntime(record, new Error(`Plugin host call timed out: ${message.operation}`), true)
    }, hostCallTimeout)
    record.pendingHostCalls.set(message.id, { timer, requestId: message.requestId, dialogDeadline, cancel, controller })
    try {
      let value = await Promise.race([
        Promise.resolve().then(() => record.options.hostCall(message.operation, args, controller.signal)),
        cancelled,
      ])
      if (record.terminated) return
      if (message.operation === 'ui.bindPoll') {
        const id = String(args.id || ''), interval = Number(args.intervalMs || 5000)
        if (!/^[a-zA-Z0-9._-]{1,96}$/.test(id) || !Number.isInteger(interval) || interval < 2000 || interval > 60000 || record.polls.size >= 16) throw new Error('Invalid or excessive UI poll binding')
        if (record.polls.has(id)) clearInterval(record.polls.get(id))
        let active = false
        record.polls.set(id, setInterval(() => {
          if (active || record.state !== 'running' || record.terminated) return
          active = true
          void this.runAction(record.pluginId, id).catch((error) => this.notifyError(record, toError(error))).finally(() => { active = false })
        }, interval))
      }
      if (message.operation === 'ui.unbindPoll') {
        const id = String(args.id || '')
        clearInterval(record.polls.get(id)); record.polls.delete(id)
      }
      if (viewToPublish) record.options.onView(viewToPublish)
      if (message.operation === 'runtime.getQuotaUsage' && isRecord(value)) value = { ...value, hostCalls: { calls: record.meter.length, bytes: record.meter.reduce((sum, item) => sum + item.bytes, 0), callLimit: 180, byteLimit: 8 * 1024 * 1024, windowSeconds: 60, enforced: true } }
      const reply = hostReply(true, value)
      const responseBytes = byteLength(reply)
      const current = performance.now()
      record.meter = record.meter.filter((item) => current - item.at < 60000)
      if (record.meter.reduce((sum, item) => sum + item.bytes, 0) + responseBytes > 8 * 1024 * 1024) throw new Error('Plugin response byte budget exceeded')
      if (!record.meter.includes(meterEntry)) { meterEntry.at = current; meterEntry.bytes = 0; record.meter.push(meterEntry) }
      meterEntry.bytes += responseBytes
      this.sendHostCallReply(record, message.id, reply)
    } catch (error) {
      if (!record.terminated) this.sendHostCallReply(record, message.id, hostReply(false, error))
    } finally {
      const pending = record.pendingHostCalls.get(message.id)
      if (pending) {
        clearTimeout(pending.timer)
        record.pendingHostCalls.delete(message.id)
        if (pending.dialogDeadline !== undefined) this.finishDialogWait(record, message.id, pending)
      }
    }
  }

  private armRequestTimeout(record: RuntimeRecord, requestId: string, timeoutMs: number) {
    const pending = record.pendingRequests.get(requestId)
    if (!pending || record.terminated) return
    clearTimeout(pending.timer)
    pending.timer = setTimeout(() => {
      this.failRuntime(record, new Error(`Plugin runtime timed out during ${pending.type}`), true)
    }, Math.max(1, Math.ceil(timeoutMs)))
  }

  private finishDialogWait(record: RuntimeRecord, dialogId: string, hostCall: PendingHostCall) {
    const request = record.pendingRequests.get(hostCall.requestId)
    if (!request || record.terminated) return
    request.dialogDeadlines.delete(dialogId)
    const latestDeadline = Math.max(0, ...request.dialogDeadlines.values())
    this.armRequestTimeout(
      record,
      hostCall.requestId,
      latestDeadline > Date.now() ? latestDeadline - Date.now() : request.timeoutMs,
    )
  }

  private sendHostCallReply(record: RuntimeRecord, id: string, replyJson: string) {
    if (record.terminated) return
    if (byteLength(replyJson) > MAX_RPC_BYTES) replyJson = hostReply(false, new Error('Host reply exceeds the size limit'))
    try {
      record.worker.postMessage({ type: 'host-call-result', id, replyJson })
    } catch (error) {
      this.failRuntime(record, toError(error), true)
    }
  }

  private notifyError(record: RuntimeRecord, error: Error) {
    try {
      record.options.onError(error)
    } catch {
      // сбой обработчика приложения не должен останавливать другие плагины
    }
  }

  private failRuntime(record: RuntimeRecord, error: Error, report: boolean) {
    if (record.terminated) return
    if (report) this.notifyError(record, error)
    this.terminateRuntime(record, error, true)
  }

  private terminateRuntime(record: RuntimeRecord, error: Error, rejectWithError: boolean) {
    if (record.terminated) return
    record.terminated = true
    for (const timer of record.polls.values()) clearInterval(timer)
    record.polls.clear()
    if (this.runtimes.get(record.pluginId) === record) this.runtimes.delete(record.pluginId)
    record.worker.onmessage = null
    record.worker.onerror = null
    record.worker.onmessageerror = null
    record.worker.terminate()
    const failure = rejectWithError ? error : new Error(`Plugin runtime stopped: ${record.pluginId}`)
    for (const pending of record.pendingRequests.values()) {
      clearTimeout(pending.timer)
      pending.reject(failure)
    }
    record.pendingRequests.clear()
    for (const pending of record.pendingHostCalls.values()) {
      clearTimeout(pending.timer)
      try {
        pending.controller.abort()
      } catch {
        // сбой обработчика отмены не должен прерывать очистку
      }
      pending.cancel(failure)
    }
    record.pendingHostCalls.clear()
  }
}

export const pluginRuntime = new PluginRuntimeService()
