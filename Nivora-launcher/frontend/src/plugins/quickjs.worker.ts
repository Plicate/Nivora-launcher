import { getQuickJS } from 'quickjs-emscripten'
import type { QuickJSContext, QuickJSDeferredPromise, QuickJSHandle, QuickJSRuntime } from 'quickjs-emscripten'
import type { RuntimePluginManifest } from './runtimeManifest'

const MEMORY_LIMIT_BYTES = 16 * 1024 * 1024
const STACK_LIMIT_BYTES = 512 * 1024
const MAX_SOURCE_BYTES = 4 * 1024 * 1024
const MAX_RPC_BYTES = 4 * 1024 * 1024
const MAX_VIEW_BYTES = 256 * 1024
const MAX_ERROR_BYTES = 8 * 1024
const INTERRUPT_BUDGET_MS = 1000
const INTERRUPT_CYCLES = 4096
const MAX_JOBS_PER_TURN = 64
const MAX_PENDING_HOST_CALLS = 32

type ParentMessage =
  | { type: 'start'; requestId: string; manifestJson: string; source: string }
  | { type: 'event'; requestId: string; event: string; payloadJson: string }
  | { type: 'action'; requestId: string; actionId: string; inputJson: string }
  | { type: 'command'; requestId: string; commandId: string; argumentsJson: string }
  | { type: 'shortcut'; requestId: string; shortcutId: string }
  | { type: 'stop'; requestId: string }
  | { type: 'host-call-result'; id: string; replyJson: string }

type WorkerMessage =
  | { type: 'host-call'; id: string; requestId: string; operation: string; argsJson: string }
  | { type: 'result'; requestId: string; ok: true; resultJson: string; status?: 'started' | 'stopped' }
  | { type: 'result'; requestId: string; ok: false; errorJson: string; fatal?: boolean; status?: 'stopped' }
  | { type: 'fatal'; errorJson: string }
  | { type: 'error'; errorJson: string }

type WorkerScope = {
  onmessage: ((event: MessageEvent<ParentMessage>) => void) | null
  postMessage(message: WorkerMessage): void
}

const workerScope = self as unknown as WorkerScope
let runtime: QuickJSRuntime | undefined
let context: QuickJSContext | undefined
let started = false
let disposed = false
let fatalSent = false
let nextHostCallId = 0
let activeControlRequestId = ''
let pendingHostCalls = new Map<string, QuickJSDeferredPromise>()
let internalFunctions = new Map<string, QuickJSHandle>()
let controlQueue: Promise<void> = Promise.resolve()
let interruptDeadline = 0
let interruptCycles = 0
let interruptExceeded = false
let pumping = false
let pumpTimer: number | undefined
let internalDispatchName = ''
let internalDisposeName = ''

const byteLength = (value: string) => new TextEncoder().encode(value).byteLength
const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value)

function errorFields(error: unknown) {
  const candidate = error as { name?: unknown; message?: unknown } | null
  return {
    name: typeof candidate?.name === 'string' ? candidate.name.slice(0, 128) : 'Error',
    message: typeof candidate?.message === 'string'
      ? candidate.message
      : typeof error === 'string' ? error : 'Plugin runtime failed',
  }
}

function errorJson(error: unknown): string {
  const fields = errorFields(error)
  let message = fields.message
  let encoded = JSON.stringify({ name: fields.name, message }) || '{"name":"Error","message":"Plugin runtime failed"}'
  while (byteLength(encoded) > MAX_ERROR_BYTES && message.length > 0) {
    message = message.slice(0, Math.floor(message.length / 2))
    encoded = JSON.stringify({ name: fields.name, message }) || '{"name":"Error","message":"Plugin runtime failed"}'
  }
  return encoded
}

function post(message: WorkerMessage) {
  try {
    workerScope.postMessage(message)
  } catch {
    // A terminated parent cannot receive any further runtime messages.
  }
}

function guestError(ctx: QuickJSContext, handle: QuickJSHandle): Error {
  let dumped: unknown
  try {
    dumped = ctx.dump(handle)
  } catch {
    dumped = undefined
  } finally {
    handle.dispose()
  }
  const fields = isRecord(dumped)
    ? {
        name: typeof dumped.name === 'string' ? dumped.name : 'Error',
        message: typeof dumped.message === 'string' ? dumped.message : 'Plugin code threw an error',
      }
    : { name: 'Error', message: typeof dumped === 'string' ? dumped : 'Plugin code threw an error' }
  const error = new Error(fields.message)
  error.name = fields.name
  return error
}

function isInterrupt(error: unknown): boolean {
  const fields = errorFields(error)
  return fields.name === 'InternalError' && fields.message === 'interrupted'
}

function failRuntime(error: unknown) {
  if (fatalSent) return
  fatalSent = true
  post({ type: 'fatal', errorJson: errorJson(error) })
  disposeRuntime()
}

function disposeRuntime() {
  if (disposed) return
  disposed = true
  started = false
  if (pumpTimer !== undefined) clearTimeout(pumpTimer)
  pumpTimer = undefined
  for (const deferred of pendingHostCalls.values()) {
    try {
      deferred.dispose()
    } catch {
      // The context may already have been interrupted or disposed.
    }
  }
  pendingHostCalls.clear()
  for (const handle of internalFunctions.values()) {
    try {
      handle.dispose()
    } catch {
      // The context may already have been interrupted or disposed.
    }
  }
  internalFunctions.clear()
  try {
    context?.dispose()
  } catch {
    // Continue disposing the runtime even if QuickJS reports a leaked handle.
  }
  context = undefined
  try {
    runtime?.dispose()
  } catch {
    // The worker will be terminated by the parent after a fatal error.
  }
  runtime = undefined
}

function resetInterruptBudget(budgetMs = INTERRUPT_BUDGET_MS) {
  interruptDeadline = Date.now() + budgetMs
  interruptCycles = 0
  interruptExceeded = false
}

function resetInterruptBudgetIfIdle() {
  if (!runtime || !runtime.hasPendingJob()) resetInterruptBudget()
}

function reportJobError(error: Error) {
  if (isInterrupt(error) || interruptExceeded) {
    failRuntime(error)
    return
  }
  post({ type: 'error', errorJson: errorJson(error) })
}

function pumpPendingJobs() {
  const activeRuntime = runtime
  if (!activeRuntime || !context || disposed || pumping) return
  if (!activeRuntime.hasPendingJob()) {
    resetInterruptBudget()
    return
  }
  pumping = true
  try {
    const result = activeRuntime.executePendingJobs(MAX_JOBS_PER_TURN)
    if (result.error) {
      const error = guestError(result.error.context, result.error)
      reportJobError(error)
    }
  } catch (error) {
    reportJobError(error instanceof Error ? error : new Error(String(error)))
  } finally {
    pumping = false
  }
  if (disposed || runtime !== activeRuntime) return
  if (!activeRuntime.hasPendingJob()) {
    resetInterruptBudget()
    return
  }
  if (pumpTimer === undefined) {
    pumpTimer = setTimeout(() => {
      pumpTimer = undefined
      pumpPendingJobs()
    }, 0)
  }
}

function resolveHostCallError(ctx: QuickJSContext, deferred: QuickJSDeferredPromise, error: unknown) {
  const replyJson = JSON.stringify({ ok: false, error: JSON.parse(errorJson(error)) }) || '{"ok":false}'
  const reply = ctx.newString(replyJson)
  try {
    deferred.resolve(reply)
  } finally {
    reply.dispose()
  }
}

function installHostBridge(ctx: QuickJSContext) {
  const bridge = ctx.newFunction('__hostCall', (operationHandle, argsHandle) => {
    const id = `host-${++nextHostCallId}`
    const deferred = ctx.newPromise()
    try {
      const operation = ctx.getString(operationHandle)
      const argsJson = ctx.getString(argsHandle)
      if (operation.length > 128 || !/^[a-z][a-z0-9]*(?:\.[a-z][a-z0-9]*)+$/i.test(operation)) {
        throw new Error('Host-call operation must be namespaced')
      }
      if (byteLength(argsJson) > MAX_RPC_BYTES) throw new Error('Host-call arguments exceed the size limit')
      const args = JSON.parse(argsJson) as unknown
      if (!isRecord(args)) throw new Error('Host-call arguments must be a JSON object')
      if (pendingHostCalls.size >= MAX_PENDING_HOST_CALLS) throw new Error('Too many pending plugin host calls')
      pendingHostCalls.set(id, deferred)
      post({ type: 'host-call', id, requestId: activeControlRequestId, operation, argsJson })
    } catch (error) {
      resolveHostCallError(ctx, deferred, error)
    }
    return deferred.handle
  })
  try {
    ctx.defineProp(ctx.global, '__hostCall', {
      value: bridge,
      configurable: false,
      enumerable: false,
    })
  } finally {
    bridge.dispose()
  }
}

function lockHostBridge(ctx: QuickJSContext) {
  resetInterruptBudget()
  const result = ctx.evalCode("Object.defineProperty(globalThis, '__hostCall', { writable: false })", '<plugin-bridge>')
  if (result.error) throw guestError(ctx, result.error)
  result.value.dispose()
}

function uniqueName(prefix: string): string {
  const random = typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID().replace(/-/g, '')
    : `${Date.now().toString(36)}${Math.random().toString(36).slice(2)}`
  return `__${prefix}_${random}`
}

function quoteJs(value: string): string {
  const encoded = JSON.stringify(value)
  if (typeof encoded !== 'string') throw new Error('Could not encode plugin runtime data')
  return encoded.replace(/\u2028/g, '\\u2028').replace(/\u2029/g, '\\u2029')
}

function skipQuoted(source: string, start: number, quote: string): number {
  for (let index = start + 1; index < source.length; index += 1) {
    if (source[index] === '\\') index += 1
    else if (source[index] === quote) return index + 1
  }
  return source.length
}

function skipComment(source: string, start: number): number {
  if (source.startsWith('//', start)) {
    const newline = source.indexOf('\n', start + 2)
    return newline < 0 ? source.length : newline + 1
  }
  const end = source.indexOf('*/', start + 2)
  return end < 0 ? source.length : end + 2
}

function canStartRegex(previous: string): boolean {
  return !previous || ['(', '[', '{', '=', ',', ':', ';', '!', '?', '&', '|', 'return', 'throw', 'case', '=>'].includes(previous)
}

function skipRegex(source: string, start: number): number {
  let inClass = false
  for (let index = start + 1; index < source.length; index += 1) {
    const char = source[index]
    if (char === '\\') index += 1
    else if (char === '[') inClass = true
    else if (char === ']') inClass = false
    else if (char === '/' && !inClass) {
      index += 1
      while (/[a-z]/i.test(source[index] || '')) index += 1
      return index
    } else if (char === '\n') return index
  }
  return source.length
}

function skipTemplateExpression(source: string, start: number): number {
  let depth = 1
  let previous = ''
  for (let index = start; index < source.length;) {
    const char = source[index]
    if (/\s/.test(char)) {
      index += 1
      continue
    }
    if (source.startsWith('//', index) || source.startsWith('/*', index)) {
      index = skipComment(source, index)
      continue
    }
    if (char === '"' || char === "'") {
      index = skipQuoted(source, index, char)
      previous = 'literal'
      continue
    }
    if (char === '`') {
      index = skipTemplateLiteral(source, index)
      previous = 'literal'
      continue
    }
    if (char === '/' && canStartRegex(previous)) {
      index = skipRegex(source, index)
      previous = 'literal'
      continue
    }
    if (/[A-Za-z_$]/.test(char)) {
      const startIndex = index++
      while (/[\w$]/.test(source[index] || '')) index += 1
      previous = source.slice(startIndex, index)
      continue
    }
    if (char === '{') depth += 1
    if (char === '}' && --depth === 0) return index + 1
    if (source.startsWith('=>', index)) {
      previous = '=>'
      index += 2
    } else {
      previous = char
      index += 1
    }
  }
  return source.length
}

function skipTemplateLiteral(source: string, start: number): number {
  for (let index = start + 1; index < source.length;) {
    if (source[index] === '\\') index += 2
    else if (source[index] === '`') return index + 1
    else if (source.startsWith('${', index)) index = skipTemplateExpression(source, index + 2)
    else index += 1
  }
  return source.length
}

function nextIdentifier(source: string, start: number): { value: string; end: number } | undefined {
  let index = start
  while (index < source.length) {
    if (/\s/.test(source[index])) index += 1
    else if (source.startsWith('//', index) || source.startsWith('/*', index)) index = skipComment(source, index)
    else break
  }
  if (!/[A-Za-z_$]/.test(source[index] || '')) return undefined
  const tokenStart = index++
  while (/[\w$]/.test(source[index] || '')) index += 1
  return { value: source.slice(tokenStart, index), end: index }
}

function captureDefaultExport(source: string, globalName: string): string {
  let previous = ''
  for (let index = 0; index < source.length;) {
    const char = source[index]
    if (/\s/.test(char)) {
      index += 1
      continue
    }
    if (source.startsWith('//', index) || source.startsWith('/*', index)) {
      index = skipComment(source, index)
      continue
    }
    if (char === '"' || char === "'") {
      index = skipQuoted(source, index, char)
      previous = 'literal'
      continue
    }
    if (char === '`') {
      index = skipTemplateLiteral(source, index)
      previous = 'literal'
      continue
    }
    if (char === '/' && canStartRegex(previous)) {
      index = skipRegex(source, index)
      previous = 'literal'
      continue
    }
    if (/[A-Za-z_$]/.test(char)) {
      const tokenStart = index++
      while (/[\w$]/.test(source[index] || '')) index += 1
      const token = source.slice(tokenStart, index)
      if (token === 'export') {
        const next = nextIdentifier(source, index)
        if (next?.value === 'default') {
          return `${source.slice(0, tokenStart)}globalThis[${quoteJs(globalName)}] =${source.slice(next.end)}`
        }
      }
      previous = token
      continue
    }
    if (source.startsWith('=>', index)) {
      previous = '=>'
      index += 2
    } else {
      previous = char
      index += 1
    }
  }
  throw new Error('Plugin entrypoint must have a default export')
}

function facadeSource(manifestJson: string, names: { start: string; dispatch: string; dispose: string; defaultExport: string }): string {
  const manifestLiteral = quoteJs(manifestJson)
  return `(() => {
    const MAX_RPC_CHARS = ${MAX_RPC_BYTES};
    const MAX_VIEW_CHARS = ${MAX_VIEW_BYTES};
    const manifest = JSON.parse(${manifestLiteral});
    const freezeDeep = (value) => {
      if (value && typeof value === 'object' && !Object.isFrozen(value)) {
        Object.keys(value).forEach((key) => freezeDeep(value[key]));
        Object.freeze(value);
      }
      return value;
    };
    const cloneJson = (value, maxChars = MAX_RPC_CHARS) => {
      const text = JSON.stringify(value);
      if (typeof text !== 'string' || text.length > maxChars) throw new RangeError('Plugin payload exceeds the size limit');
      return JSON.parse(text);
    };
    const request = (operation, args = {}) => {
      const text = JSON.stringify(args);
      if (typeof text !== 'string' || text.length > MAX_RPC_CHARS) return Promise.reject(new RangeError('Host-call payload exceeds the size limit'));
      return __hostCall(operation, text).then((replyText) => {
        const reply = JSON.parse(replyText);
        if (!reply || reply.ok !== true) {
          const message = reply && reply.error && typeof reply.error.message === 'string' ? reply.error.message : 'Host request failed';
          throw new Error(message);
        }
        return reply.hasValue === false ? undefined : reply.value;
      });
    };
    const subscriptions = new Set();
    const eventHandlers = new Map();
    const actionHandlers = new Map();
    const commandHandlers = new Map();
    const shortcutHandlers = new Map();
    const views = new Map();
    let nextRegistrationId = 0;
    let disposer;
    const subscribe = (registerOperation, unregisterOperation, registerArgs, unregisterArgs, cleanup) => {
      let active = true;
      const registered = request(registerOperation, registerArgs);
      registered.catch(() => undefined);
      const unsubscribe = () => {
        if (!active) return Promise.resolve();
        active = false;
        subscriptions.delete(unsubscribe);
        if (cleanup) cleanup();
        return registered.catch(() => undefined)
          .then(() => request(unregisterOperation, unregisterArgs))
          .then(() => undefined)
          .catch(() => undefined);
      };
      subscriptions.add(unsubscribe);
      return unsubscribe;
    };
    const requireHandler = (definition, handler) => {
      if (!definition || typeof definition.id !== 'string' || !definition.id || typeof handler !== 'function') throw new TypeError('A registration needs an id and a handler');
    };
    const registerHandler = (map, registerOperation, unregisterOperation, definition, handler) => {
      requireHandler(definition, handler);
      if (map.has(definition.id)) throw new Error('A handler is already registered for ' + definition.id);
      map.set(definition.id, handler);
      return subscribe(registerOperation, unregisterOperation, { definition }, { id: definition.id }, () => {
        if (map.get(definition.id) === handler) map.delete(definition.id);
      });
    };
    const registerEvent = (operation, event, listener) => {
      if (typeof event !== 'string' || typeof listener !== 'function') throw new TypeError('An event name and listener are required');
      const listenerId = 'listener-' + (++nextRegistrationId);
      let listeners = eventHandlers.get(event);
      if (!listeners) eventHandlers.set(event, (listeners = new Map()));
      listeners.set(listenerId, listener);
      return subscribe(operation, operation === 'events.on' ? 'events.off' : 'launcher.off', { event, listener_id: listenerId }, { event, listener_id: listenerId }, () => {
        listeners.delete(listenerId);
        if (!listeners.size) eventHandlers.delete(event);
      });
    };
    const fragments = new Map();
    const rawViews = new Map();
    const boundData = new Map();
    const dataBindings = new Map();
    const cancelViewBindings = (viewId) => { for (const binding of Array.from(dataBindings.values())) if (binding.viewId === viewId) void binding.remove(); };
    const expand = (node, depth = 0, budget = { nodes: 0 }, trail = []) => {
      if (!node || typeof node !== 'object' || depth > 10 || ++budget.nodes > 512) throw new Error('UI complexity limit exceeded');
      if (node.type === 'include') {
        if (!fragments.has(node.fragment) || trail.includes(node.fragment)) throw new Error('Missing or cyclic UI fragment');
        return expand(fragments.get(node.fragment), depth + 1, budget, trail.concat(node.fragment));
      }
      const copy = cloneJson(node, MAX_VIEW_CHARS);
      for (const key of ['children', 'fields']) if (Array.isArray(copy[key])) copy[key] = copy[key].map((item) => expand(item, depth + 1, budget, trail));
      for (const key of ['tabs', 'sections']) if (Array.isArray(copy[key])) copy[key] = copy[key].map((item) => ({ ...item, content: expand(item.content, depth + 1, budget, trail) }));
      return copy;
    };
    const materialize = (view) => {
      const copy = { ...cloneJson(view, MAX_VIEW_CHARS), root: expand(view.root) };
      const visit = (node) => {
        const data = boundData.get(JSON.stringify([view.id, node.id]));
        if (data) node[data.field] = cloneJson(data.value, MAX_VIEW_CHARS);
        for (const key of ['children','fields']) for (const child of node[key] || []) visit(child);
        for (const key of ['tabs','sections']) for (const entry of node[key] || []) visit(entry.content);
      };
      visit(copy.root); return copy;
    };
    const refreshFragments = async () => {
      for (const [id, view] of rawViews) { const copy = materialize(view); await request('ui.updateView', { view: copy }); views.set(id, copy); }
    };
    const localize = manifest.i18n || {};
    const locale = typeof localize.default_locale === 'string' ? localize.default_locale : 'en';
    const messages = localize.messages || {};
    const metadata = freezeDeep({
      id: manifest.id,
      name: manifest.name,
      version: manifest.version,
      author: manifest.author || '',
      description: manifest.description || '',
      api_version: manifest.api_version,
      apiVersion: manifest.api_version,
    });
    freezeDeep(manifest);
    const api = Object.freeze({
      metadata,
      manifest,
      runtime: Object.freeze({
        getInfo() { return request('runtime.getInfo'); },
        getCapabilities() { return request('runtime.getCapabilities'); },
        getQuotaUsage() { return request('runtime.getQuotaUsage'); },
        assertCapability(name) { return request('runtime.assertCapability', { name }); },
      }),
      permissions: Object.freeze({
        get() { return request('permissions.get'); },
        set(permissions) { return request('permissions.set', { permissions }); },
      }),
      ui: Object.freeze({
        registerFragment(id, node) {
          if (typeof id !== 'string' || !/^[a-zA-Z0-9._-]{1,96}$/.test(id) || fragments.size >= 128 || fragments.has(id)) throw new Error('Invalid, duplicate or excessive fragment');
          fragments.set(id, cloneJson(node, MAX_VIEW_CHARS));
          return subscribe('ui.fragment', 'ui.fragment', { id }, { id }, () => fragments.delete(id));
        },
        async updateFragment(id, node) {
          if (!fragments.has(id)) throw new Error('Unknown fragment');
          await request('ui.fragment', { id });
          const before = fragments.get(id);
          fragments.set(id, cloneJson(node, MAX_VIEW_CHARS));
          try { for (const view of rawViews.values()) materialize(view); await refreshFragments(); }
          catch (error) { fragments.set(id, before); throw error; }
        },
        bindData(viewId, nodeId, source) {
          if (!source || !['static','event','poll'].includes(source.type) || typeof source.loader !== 'function') throw new TypeError('Invalid UI data source');
          if (!rawViews.has(viewId)) throw new Error('Unknown view');
          let alive = true, active = false;
          if (dataBindings.size >= 16) throw new Error('Too many UI bindings');
          let id = 'data-' + (++nextRegistrationId);
          while (actionHandlers.has(id)) id = 'data-' + (++nextRegistrationId);
          const reload = async () => {
            if (!alive || active) return;
            active = true;
            try {
              const data = cloneJson(await source.loader(), MAX_VIEW_CHARS);
              if (!alive || !rawViews.has(viewId)) return;
              const view = materialize(rawViews.get(viewId));
              let found = 0;
              const patch = (node, depth = 0) => {
                if (depth > 10) throw new Error('UI data depth exceeded');
                if (node.id === nodeId) {
                  found++;
                  const field = { list: 'items', table: 'rows', chart: 'values', text: 'text', checkbox: 'checked', input: 'value', textarea: 'value', number: 'value', slider: 'value', color: 'value', datetime: 'value', select: 'value', progress: 'value', switch: 'checked', metric: 'value', alert: 'text', code: 'text' }[node.type];
                  if (!field) throw new Error('Unsupported dynamic UI node');
                  if (node.type === 'list') node.items = data;
                  else if (node.type === 'table') node.rows = data;
                  else if (node.type === 'chart') node.values = data;
                  else if (node.type === 'text' || node.type === 'alert' || node.type === 'code') node.text = data;
                  else if (node.type === 'checkbox' || node.type === 'switch') node.checked = data;
                  else if (['input','select','progress','textarea','number','slider','color','datetime','metric'].includes(node.type)) node.value = data;
                  else throw new Error('Unsupported dynamic UI node');
                }
                for (const key of ['children','fields']) for (const child of node[key] || []) patch(child, depth + 1);
                for (const key of ['tabs','sections']) for (const entry of node[key] || []) patch(entry.content, depth + 1);
              };
              patch(view.root);
              if (found !== 1) throw new Error('Dynamic node id must be unique');
              await request('ui.updateView', { view });
              views.set(viewId, view);
              const find = (node) => { if (node.id === nodeId) return node; for (const key of ['children','fields']) for (const child of node[key] || []) { const match = find(child); if (match) return match; } for (const key of ['tabs','sections']) for (const entry of node[key] || []) { const match = find(entry.content); if (match) return match; } };
              const target = find(view.root);
              const field = { list: 'items', table: 'rows', chart: 'values', text: 'text', checkbox: 'checked', input: 'value', select: 'value', progress: 'value' }[target.type];
              boundData.set(JSON.stringify([viewId, nodeId]), { field, value: data });
            } finally { active = false; }
          };
          let remove;
          if (source.type === 'event') remove = registerEvent('events.on', source.event, reload);
          else if (source.type === 'poll') {
            actionHandlers.set(id, reload);
            remove = subscribe('ui.bindPoll','ui.unbindPoll',{ id, intervalMs: source.intervalMs || 5000 },{ id }, () => actionHandlers.delete(id));
          } else remove = subscribe('ui.fragment','ui.fragment',{ id },{ id }, () => {});
          void reload().catch((error) => request('ui.dataError', { message: String(error.message || error).slice(0, 1024) }));
          const cleanup = () => { alive = false; dataBindings.delete(id); boundData.delete(JSON.stringify([viewId, nodeId])); return remove(); };
          dataBindings.set(id, { viewId, remove: cleanup });
          return cleanup;
        },
        registerView(view) {
          const copy = materialize(view);
          if (rawViews.size >= 32 && !rawViews.has(copy.id)) throw new Error('Too many plugin views');
          rawViews.set(copy.id, cloneJson(view, MAX_VIEW_CHARS));
          if (!copy || typeof copy.id !== 'string' || !copy.id) throw new TypeError('A view needs an id');
          views.set(copy.id, copy);
          return subscribe('ui.registerView', 'ui.removeView', { view: copy }, { id: copy.id }, () => { cancelViewBindings(copy.id); views.delete(copy.id); rawViews.delete(copy.id); });
        },
        async updateView(view) {
          const copy = materialize(view);
          if (rawViews.size >= 32 && !rawViews.has(copy.id)) throw new Error('Too many plugin views');
          rawViews.set(copy.id, cloneJson(view, MAX_VIEW_CHARS));
          if (!copy || typeof copy.id !== 'string' || !copy.id) throw new TypeError('A view needs an id');
          await request('ui.updateView', { view: copy });
          views.set(copy.id, copy);
        },
        async removeView(id) {
          const removed = await request('ui.removeView', { id });
          cancelViewBindings(id); views.delete(id); rawViews.delete(id);
          return removed;
        },
        compose(node) {
          const copy = cloneJson(node, MAX_VIEW_CHARS);
          const supported = ['text', 'badge', 'button', 'input', 'textarea', 'number', 'slider', 'color', 'datetime', 'select', 'checkbox', 'switch', 'progress', 'metric', 'alert', 'code', 'image', 'list', 'link', 'divider', 'group', 'card', 'grid', 'form', 'tabs', 'accordion', 'table', 'chart', 'include'];
          if (!copy || !supported.includes(copy.type)) throw new TypeError('Unsupported plugin UI node');
          return freezeDeep(copy);
        },
      }),
      i18n: Object.freeze({
        locale,
        availableLocales() { return Object.freeze(Object.keys(messages)); },
        t(key, values = {}) {
          const table = messages[locale] || {};
          const fallback = messages[localize.default_locale] || {};
          const template = typeof table[key] === 'string' ? table[key] : typeof fallback[key] === 'string' ? fallback[key] : key;
          return template.replace(/\\{([A-Za-z0-9_.-]+)\\}/g, (_match, name) => Object.prototype.hasOwnProperty.call(values, name) ? String(values[name]) : '{' + name + '}');
        },
      }),
      theme: Object.freeze({
        get() { return request('theme.get'); },
        register(definition) {
          const registrationId = 'theme-' + (++nextRegistrationId);
          return subscribe('theme.register', 'theme.unregister', { registration_id: registrationId, definition }, { registration_id: registrationId });
        },
      }),
      actions: Object.freeze({
        register(definition, handler) { return registerHandler(actionHandlers, 'actions.register', 'actions.unregister', definition, handler); },
        async run(id, input) {
          const handler = actionHandlers.get(id);
          if (!handler) throw new Error('No plugin action is registered for ' + id);
          return await handler(input === undefined ? null : input);
        },
      }),
      events: Object.freeze({ on(event, listener) { return registerEvent('events.on', event, listener); }, listListeners() { return Object.freeze(Array.from(eventHandlers, ([event, listeners]) => Object.freeze({ event, count: listeners.size, listenerIds: Object.freeze(Array.from(listeners.keys())) }))); } }),
      storage: Object.freeze({
        get(key, fallback) { return request('storage.get', { key }).then((value) => value === undefined ? fallback : value); },
        set(key, value) { return request('storage.set', { key, value }); },
        remove(key) { return request('storage.remove', { key }); },
      }),
      config: Object.freeze({
        get(key, fallback) { return request('config.get', { key }).then((value) => value === undefined ? fallback : value); },
        set(key, value) { return request('config.set', { key, value }); },
        remove(key) { return request('config.remove', { key }); },
        readProjected(schemaId) { return request('config.readProjected', { schemaId }); },
      }),
      files: Object.freeze({
        list(scope, directory) { return request('files.list', { scope, directory: directory || '' }); },
        readText(scope, path) { return request('files.readText', { scope, path }); },
        writeText(scope, path, content) { return request('files.writeText', { scope, path, content }); },
        remove(scope, path) { return request('files.remove', { scope, path }); },
      }),
      http: Object.freeze({
        request(requestOptions) { return request('http.request', { request: requestOptions }); },
      }),
      database: Object.freeze({
        query(sql, parameters = []) { return request('database.query', { sql, parameters }); },
        execute(sql, parameters = []) { return request('database.execute', { sql, parameters }); },
      }),
      network: Object.freeze({
        fetchJson(url, requestOptions = {}) { return request('network.fetchJson', { url, request: requestOptions }); },
        resolve(host, port) { return request('network.resolve', port === undefined ? { host } : { host, port }); },
        ping(host, port, timeout_ms) { return request('network.ping', { host, port: port === undefined ? 25565 : port, timeout_ms: timeout_ms === undefined ? 3000 : timeout_ms }); },
        checkPort(host, port, timeout_ms) { return request('network.checkPort', { host, port: port === undefined ? 25565 : port, timeout_ms: timeout_ms === undefined ? 3000 : timeout_ms }); },
      }),
      instances: Object.freeze({
        list() { return request('instances.list'); },
        getProcessMetrics(instanceName) { return request('instances.getProcessMetrics', { instanceName }); },
      }),
      os: Object.freeze({
        openUrl(url) { return request('os.openUrl', { url }); },
      }),
      proxy: Object.freeze({
        listRoutes() { return request('proxy.listRoutes'); },
        saveRoutes(routes) { return request('proxy.saveRoutes', { routes }); },
        addRoute(route) { return request('proxy.addRoute', { route }); },
        removeRoute(routeId) { return request('proxy.removeRoute', { routeId }); },
      }),
      launcher: Object.freeze({
        dialogs: Object.freeze({
          alert(options) { return request('launcher.dialogs.alert', { options }); },
          confirm(options) { return request('launcher.dialogs.confirm', { options }); },
          prompt(options) { return request('launcher.dialogs.prompt', { options }); },
          choose(options) { return request('launcher.dialogs.choose', { options }); },
        }),
        servers: Object.freeze({
          list() { return request('launcher.servers.list'); },
          save(servers) { return request('launcher.servers.save', { servers }); },
          add(server) { return request('launcher.servers.add', { server }); },
          remove(id) { return request('launcher.servers.remove', { id }); },
        }),
        openExternal(url) { return request('launcher.openExternal', { url }); },
        selectedInstance() { return request('launcher.selectedInstance'); },
        on(event, listener) { return registerEvent('launcher.on', event, listener); },
      }),
      notifications: Object.freeze({
        show(options) { return request('notifications.show', { options }); },
        dismiss(id) { return request('notifications.dismiss', { id }); },
      }),
      commands: Object.freeze({
        register(definition, handler) { return registerHandler(commandHandlers, 'commands.register', 'commands.unregister', definition, handler); },
      }),
      shortcuts: Object.freeze({
        register(definition, handler) { return registerHandler(shortcutHandlers, 'shortcuts.register', 'shortcuts.unregister', definition, handler); },
      }),
    });
    globalThis[${quoteJs(names.start)}] = async () => {
      const entry = globalThis[${quoteJs(names.defaultExport)}];
      try { delete globalThis[${quoteJs(names.defaultExport)}]; } catch (_error) { globalThis[${quoteJs(names.defaultExport)}] = undefined; }
      const receiver = typeof entry === 'function' ? undefined : entry;
      const activate = typeof entry === 'function' ? entry : entry && entry.activate;
      if (typeof activate !== 'function') throw new TypeError('Plugin default export must be a function or an object with activate(api)');
      const result = await activate.call(receiver, api);
      if (typeof result === 'function') disposer = result;
      return 'null';
    };
    globalThis[${quoteJs(names.dispatch)}] = async (kind, id, payloadJson) => {
      const payload = JSON.parse(payloadJson);
      if (kind === 'event') {
        const listeners = eventHandlers.get(id);
        if (listeners) for (const listener of listeners.values()) await listener(payload);
        return 'null';
      }
      const handler = kind === 'action'
        ? actionHandlers.get(id)
        : kind === 'command'
          ? commandHandlers.get(id)
          : kind === 'shortcut'
            ? shortcutHandlers.get(id)
            : undefined;
      if (!handler) throw new Error('No plugin handler is registered for ' + id);
      const result = await handler(payload);
      const text = JSON.stringify(result === undefined ? null : result);
      if (typeof text !== 'string' || text.length > MAX_RPC_CHARS) throw new RangeError('Plugin result exceeds the size limit');
      return text;
    };
    globalThis[${quoteJs(names.dispose)}] = async () => {
      let failure;
      for (const binding of Array.from(dataBindings.values())) { try { await binding.remove(); } catch (error) { failure = error; } }
      if (disposer) {
        try { await disposer(); } catch (error) { failure = error; }
        disposer = undefined;
      }
      for (const unsubscribe of Array.from(subscriptions)) {
        try { await unsubscribe(); } catch (error) { if (!failure) failure = error; }
      }
      if (failure) throw failure;
      return 'null';
    };
    return 'ready';
  })()`
}

function invokeInternal(name: string, args: string[]): QuickJSHandle {
  if (!context) throw new Error('QuickJS context is unavailable')
  const ctx = context
  const fn = internalFunctions.get(name)
  if (!fn) throw new Error('QuickJS internal function is unavailable')
  const handles = args.map((value) => ctx.newString(value))
  try {
    const result = ctx.callFunction(fn, ctx.undefined, ...handles)
    if (result.error) throw guestError(ctx, result.error)
    return result.value
  } finally {
    handles.forEach((handle) => handle.dispose())
  }
}

async function settleGuestHandle(handle: QuickJSHandle): Promise<QuickJSHandle> {
  if (!context || !runtime) {
    handle.dispose()
    throw new Error('QuickJS runtime is unavailable')
  }
  const ctx = context
  let pending: ReturnType<QuickJSContext['resolvePromise']>
  try {
    pending = ctx.resolvePromise(handle)
  } finally {
    handle.dispose()
  }
  pumpPendingJobs()
  const result = await pending
  if (result.error) throw guestError(ctx, result.error)
  return result.value
}

async function callInternal(name: string, args: string[]): Promise<string> {
  const ctx = context
  if (!ctx) throw new Error('QuickJS context is unavailable')
  resetInterruptBudgetIfIdle()
  const handle = invokeInternal(name, args)
  const result = await settleGuestHandle(handle)
  try {
    if (ctx.typeof(result) !== 'string') throw new Error('Plugin runtime returned a non-string result')
    return ctx.getString(result)
  } finally {
    result.dispose()
  }
}

async function startPlugin(message: Extract<ParentMessage, { type: 'start' }>) {
  if (runtime || context || disposed) throw new Error('Plugin runtime can only be started once')
  if (byteLength(message.source) > MAX_SOURCE_BYTES) throw new Error('Plugin entrypoint exceeds the size limit')
  if (byteLength(message.manifestJson) > MAX_RPC_BYTES) throw new Error('Plugin manifest exceeds the size limit')
  const manifest = JSON.parse(message.manifestJson) as RuntimePluginManifest
  if (!isRecord(manifest)) throw new Error('Plugin manifest must be a JSON object')

  const quickJS = await getQuickJS()
  runtime = quickJS.newRuntime()
  runtime.setMemoryLimit(MEMORY_LIMIT_BYTES)
  runtime.setMaxStackSize(STACK_LIMIT_BYTES)
  runtime.setInterruptHandler(() => {
    interruptCycles += 1
    interruptExceeded = interruptExceeded || interruptCycles > INTERRUPT_CYCLES || Date.now() > interruptDeadline
    return interruptExceeded
  })
  runtime.setModuleLoader((moduleName) => {
    throw new Error(`Plugin module imports are disabled: ${moduleName.slice(0, 128)}`)
  })
  context = runtime.newContext()
  installHostBridge(context)
  lockHostBridge(context)

  const names = {
    defaultExport: uniqueName('plugin_default'),
    start: uniqueName('plugin_start'),
    dispatch: uniqueName('plugin_dispatch'),
    dispose: uniqueName('plugin_dispose'),
  }
  internalDispatchName = names.dispatch
  internalDisposeName = names.dispose
  const moduleSource = captureDefaultExport(message.source, names.defaultExport)
  resetInterruptBudgetIfIdle()
  const moduleResult = context.evalCode(moduleSource, manifest.entrypoint || `plugin-${manifest.id}.js`, { type: 'module' })
  if (moduleResult.error) throw guestError(context, moduleResult.error)
  moduleResult.value.dispose()

  resetInterruptBudgetIfIdle()
  const facadeResult = context.evalCode(facadeSource(message.manifestJson, names), '<plugin-api>', { type: 'global' })
  if (facadeResult.error) throw guestError(context, facadeResult.error)
  facadeResult.value.dispose()

  for (const name of [names.start, names.dispatch, names.dispose]) {
    internalFunctions.set(name, context.getProp(context.global, name))
  }
  await callInternal(names.start, [])
  started = true
}

function replySuccess(requestId: string, value: unknown, status?: 'started' | 'stopped') {
  const resultJson = JSON.stringify(value === undefined ? null : value)
  if (typeof resultJson !== 'string') throw new Error('Plugin result is not JSON serializable')
  if (byteLength(resultJson) > MAX_RPC_BYTES) throw new Error('Plugin result exceeds the size limit')
  post({ type: 'result', requestId, ok: true, resultJson, status })
}

function replyFailure(requestId: string, error: unknown, fatal = false, status?: 'stopped') {
  post({ type: 'result', requestId, ok: false, errorJson: errorJson(error), fatal, status })
}

async function processControl(message: Exclude<ParentMessage, { type: 'host-call-result' }>) {
  const previousRequestId = activeControlRequestId
  activeControlRequestId = message.requestId
  try {
    await processControlRequest(message)
  } finally {
    activeControlRequestId = previousRequestId
  }
}

async function processControlRequest(message: Exclude<ParentMessage, { type: 'host-call-result' }>) {
  if (message.type === 'start') {
    try {
      await startPlugin(message)
      replySuccess(message.requestId, null, 'started')
    } catch (error) {
      const failure = error instanceof Error ? error : new Error(String(error))
      if (isInterrupt(failure) || interruptExceeded) {
        failRuntime(failure)
      } else {
        replyFailure(message.requestId, failure, true)
        disposeRuntime()
      }
    }
    return
  }

  if (message.type === 'event' || message.type === 'action' || message.type === 'command' || message.type === 'shortcut') {
    try {
      if (!started || !context) throw new Error('Plugin runtime is not active')
      const text = message.type === 'event'
        ? message.payloadJson
        : message.type === 'action'
          ? message.inputJson
          : message.type === 'command'
            ? message.argumentsJson
            : 'null'
      if (byteLength(text) > MAX_RPC_BYTES) throw new Error('Plugin dispatch payload exceeds the size limit')
      JSON.parse(text)
      const kind = message.type
      const name = message.type === 'event'
        ? message.event
        : message.type === 'action'
          ? message.actionId
          : message.type === 'command'
            ? message.commandId
            : message.shortcutId
      const result = await callInternal(internalDispatchName, [kind, name, text])
      replySuccess(message.requestId, JSON.parse(result))
    } catch (error) {
      const failure = error instanceof Error ? error : new Error(String(error))
      if (isInterrupt(failure) || interruptExceeded) failRuntime(failure)
      else replyFailure(message.requestId, failure)
    }
    return
  }

  if (message.type === 'stop') {
    let failure: Error | undefined
    try {
      if (started && context) await callInternal(internalDisposeName, [])
    } catch (error) {
      failure = error instanceof Error ? error : new Error(String(error))
    } finally {
      disposeRuntime()
    }
    if (failure) replyFailure(message.requestId, failure, false, 'stopped')
    else replySuccess(message.requestId, null, 'stopped')
  }
}

function processHostCallResult(message: Extract<ParentMessage, { type: 'host-call-result' }>) {
  const deferred = pendingHostCalls.get(message.id)
  if (!deferred) return
  pendingHostCalls.delete(message.id)
  if (!context || disposed) {
    try { deferred.dispose() } catch { /* disposed with the runtime */ }
    return
  }
  if (byteLength(message.replyJson) > MAX_RPC_BYTES) {
    failRuntime(new Error('Host-call reply exceeds the size limit'))
    return
  }
  try {
    JSON.parse(message.replyJson)
    resetInterruptBudget()
    const value = context.newString(message.replyJson)
    try {
      deferred.resolve(value)
    } finally {
      value.dispose()
    }
    pumpPendingJobs()
  } catch (error) {
    try { deferred.dispose() } catch { /* disposed with the runtime */ }
    failRuntime(error)
  }
}

function receive(message: ParentMessage) {
  if (!message || typeof message !== 'object' || typeof message.type !== 'string') {
    failRuntime(new Error('Invalid plugin worker message'))
    return
  }
  if (message.type === 'host-call-result') {
    processHostCallResult(message)
    return
  }
  controlQueue = controlQueue.then(() => processControl(message)).catch((error: unknown) => {
    failRuntime(error)
  })
}

workerScope.onmessage = (event) => receive(event.data)
