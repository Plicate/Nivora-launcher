import assert from 'node:assert/strict'
import test from 'node:test'
import fs from 'node:fs'
import vm from 'node:vm'
import { stripTypeScriptTypes } from 'node:module'
import { PluginRuntimeService } from '../src/services/pluginRuntime.ts'
import { buildPluginThemeContext, dispatchPluginThemeHostCall } from '../src/services/pluginThemeContext.ts'

function facade(themeSnapshot = () => null) {
  const source = fs.readFileSync(new URL('../src/plugins/quickjs.worker.ts', import.meta.url), 'utf8')
  const begin = source.indexOf('function facadeSource(')
  const end = source.indexOf('\nfunction invokeInternal(', begin)
  const builder = stripTypeScriptTypes(source.slice(begin, end))
  const build = vm.runInNewContext(builder + '\nfacadeSource', { quoteJs: JSON.stringify, MAX_RPC_BYTES: 4 * 1024 * 1024, MAX_VIEW_BYTES: 256 * 1024 })
  const calls = [], sandbox = {
    __hostCall: async (operation, args) => {
      const parsed = JSON.parse(args)
      calls.push({ operation, args: parsed })
      return JSON.stringify({ ok: true, hasValue: true, value: operation === 'theme.get' ? themeSnapshot() : null })
    },
  }
  sandbox.entry = (api) => { sandbox.api = api }
  vm.runInNewContext(build(JSON.stringify({ id: 'test', api_version: '3', i18n: {}, events: ['settings.changed'] }), { start: 'start', dispatch: 'dispatch', dispose: 'dispose', defaultExport: 'entry' }), sandbox)
  return { sandbox, calls }
}
const settle = () => new Promise((resolve) => setImmediate(resolve))

test('theme snapshot is frozen, filtered and exposes read-only helpers', async () => {
  const sample = {
    mode: 'dark', accent: '#336699', locale: 'ru',
    colors: { accent: '#336699', background: '#ffffff' },
    tokens: { accent: '#336699', radius: 8 },
    buttonStyle: { width: 152, outline: { enabled: false } },
    buttonLabels: { instances: 'Играть' },
    fonts: { ui: 'system', button: 'inter', available: ['system', 'inter'] },
  }
  const { sandbox, calls } = facade(() => sample)
  await sandbox.start()
  const theme = sandbox.api.theme
  const snapshot = await theme.get()
  assert.equal(snapshot.mode, 'dark')
  assert.equal(await theme.getColor('accent'), '#336699')
  assert.equal(await theme.getColor('unknown'), undefined)
  assert.equal(await theme.getToken('radius'), 8)
  assert.equal(await theme.getLocale(), 'ru')
  assert.equal((await theme.getButtonStyle()).width, 152)
  assert.equal((await theme.getButtonLabels()).instances, 'Играть')
  assert.deepEqual(Array.from((await theme.getFonts()).available), ['system', 'inter'])
  assert.equal(Object.isFrozen(snapshot), true)
  assert.equal(Object.isFrozen(snapshot.tokens), true)
  assert.equal(Object.getPrototypeOf(snapshot)?.constructor?.name, 'Object')
  assert.equal(Object.hasOwn(snapshot, '__proto__'), false)
  assert.throws(() => theme.getToken('__proto__'), /safe name/)
  assert.throws(() => theme.getToken('a'.repeat(257)), /safe name/)
  assert.equal(theme.normalizeColor(' #AbC '), '#aabbcc')
  assert.equal(theme.normalizeColor('url(javascript:alert(1))', '#ABC'), '#aabbcc')
  assert.equal(theme.contrastRatio('#000', '#fff'), 21)
  assert.throws(() => theme.contrastRatio('url(javascript:alert(1))', '#fff'), /hex values/)
  assert.equal(['apply', 'set', 'replace', 'injectCss'].some((name) => name in theme), false)
  assert.equal(calls.filter((call) => call.operation === 'theme.get').length, 8)
  await sandbox.dispose()
})

test('plugin theme snapshot strips unsafe keys, colors and asset data', () => {
  const snapshot = buildPluginThemeContext({
    theme: 'dark', accent_color: 'url(javascript:alert(1))',
    color_tokens: { accent: '#ABCDEF', background: '#ffffff', unsafe: 'url(javascript:alert(1))', '__proto__': '#ffffff' },
    button_customization: {
      width: 900, height: -1, font_family: 'external-font', font_size: 99,
      crop: { x: 90, y: 99, width: 50, height: 25 },
      texture_data: 'data:image/png;base64,AA==', icon_data: 'secret',
      labels: { instances: 'Launch', injected: 'ignored' },
    },
  }, 'glass', 'en', '__proto__')
  assert.equal(snapshot.mode, 'glass')
  assert.equal(snapshot.locale, 'en')
  assert.equal(snapshot.accent, '#abcdef')
  assert.deepEqual({ ...snapshot.colors }, { accent: '#abcdef', background: '#ffffff' })
  assert.equal(snapshot.buttonStyle.width, 280)
  assert.equal(snapshot.buttonStyle.height, 28)
  assert.deepEqual({ ...snapshot.buttonStyle.crop }, { x: 50, y: 75, width: 50, height: 25 })
  assert.equal(snapshot.buttonStyle.font_family, 'system')
  assert.deepEqual({ ...snapshot.buttonLabels }, { instances: 'Launch', accounts: '', mods: '', modpacks: '', themes: '', settings: '' })
  assert.equal(snapshot.fonts.ui, 'system')
  assert.equal(Object.hasOwn(snapshot.buttonStyle, 'texture_data'), false)
  assert.equal(Object.hasOwn(snapshot.tokens, 'unsafe'), false)
  assert.equal(Object.hasOwn(snapshot.tokens, '__proto__'), false)
  assert.equal(Object.isFrozen(snapshot), true)
  assert.equal(Object.isFrozen(snapshot.buttonStyle.outline), true)
})

test('theme contributions require consent and reject CSS color payloads', async () => {
  let authorized = false
  const registrations = []
  const theme = { mode: 'dark', accent: '#000000', tokens: {}, colors: {}, locale: 'en', buttonStyle: {}, buttonLabels: {}, fonts: {} }
  const context = {
    theme: () => theme,
    authorize: async (permission) => {
      if (!authorized || permission !== 'ui:compose') throw new Error('permission denied')
    },
    registerTheme: (...args) => registrations.push(args),
    unregisterTheme() {},
  }
  await assert.rejects(dispatchPluginThemeHostCall('theme.register', { registration_id: 'test', definition: { colors: { primary: '#123456' } } }, context), /permission denied/)
  assert.equal(registrations.length, 0)
  authorized = true
  await assert.rejects(dispatchPluginThemeHostCall('theme.register', { registration_id: 'test', definition: { colors: { primary: 'url(javascript:alert(1))' } } }, context), /Invalid theme color/)
  await assert.rejects(dispatchPluginThemeHostCall('theme.register', { registration_id: 'x'.repeat(97), definition: {} }, context), /at most 96 characters/)
  await assert.rejects(dispatchPluginThemeHostCall('theme.register', { registration_id: 'test', definition: { colors: Object.fromEntries(Array.from({ length: 65 }, (_, index) => [`color${index}`, '#123456'])) } }, context), /at most 64 entries/)
  await assert.rejects(dispatchPluginThemeHostCall('theme.register', { registration_id: 'test', definition: { tokens: { long: 'x'.repeat(257) } } }, context), /Invalid theme token/)
  assert.equal(registrations.length, 0)
  await dispatchPluginThemeHostCall('theme.register', { registration_id: 'test', definition: { colors: { primary: '#ABCDEF' } } }, context)
  assert.deepEqual(registrations, [['test', { colors: { primary: '#abcdef' } }]])
  const oldApiContext = await dispatchPluginThemeHostCall('theme.get', {}, context)
  assert.equal(oldApiContext.mode, 'dark')
})

test('real guest facade expands fragments, refreshes and rejects cycles', async () => {
  const { sandbox, calls } = facade(); await sandbox.start()
  sandbox.api.ui.registerFragment('leaf', { type: 'text', id: 'value', text: 'old' })
  sandbox.api.ui.registerView({ id: 'view', slot: 'home_widget', title: 'View', root: { type: 'include', id: 'include', fragment: 'leaf' } })
  await settle()
  assert.equal(calls.find((call) => call.operation === 'ui.registerView').args.view.root.text, 'old')
  await sandbox.api.ui.updateFragment('leaf', { type: 'text', id: 'value', text: 'new' })
  assert.equal(calls.findLast((call) => call.operation === 'ui.updateView').args.view.root.text, 'new')
  await assert.rejects(sandbox.api.ui.updateFragment('leaf', { type: 'include', id: 'cycle', fragment: 'leaf' }), /cyclic/)
  await sandbox.dispose()
})

test('static and event loaders stay in guest and bindings are cleaned up', async () => {
  const { sandbox, calls } = facade(); await sandbox.start()
  sandbox.api.ui.registerFragment('leaf', { type: 'list', id: 'rows', items: [] })
  sandbox.api.ui.registerView({ id: 'view', slot: 'home_widget', title: 'View', root: { type: 'include', id: 'include', fragment: 'leaf' } })
  let value = 'one'
  const remove = sandbox.api.ui.bindData('view', 'rows', { type: 'event', event: 'settings.changed', loader: async () => [value] })
  await settle()
  assert.deepEqual(calls.findLast((call) => call.operation === 'ui.updateView').args.view.root.items, ['one'])
  assert.equal(sandbox.api.events.listListeners()[0].event, 'settings.changed')
  value = 'two'; await sandbox.dispatch('event', 'settings.changed', '{}')
  assert.deepEqual(calls.findLast((call) => call.operation === 'ui.updateView').args.view.root.items, ['two'])
  await sandbox.api.ui.updateFragment('leaf', { type: 'list', id: 'rows', items: [] })
  assert.deepEqual(calls.findLast((call) => call.operation === 'ui.updateView').args.view.root.items, ['two'])
  await remove(); assert.equal(sandbox.api.events.listListeners().length, 0)
  await sandbox.dispose()
  assert.ok(calls.every((call) => !JSON.stringify(call.args).includes('loader')))
})

test('removing a view unsubscribes its poll and suppresses stale loader result', async () => {
  const { sandbox, calls } = facade(); await sandbox.start()
  sandbox.api.ui.registerView({ id: 'view', slot: 'home_widget', title: 'View', root: { type: 'text', id: 'data', text: 'old' } })
  let finish
  sandbox.api.ui.bindData('view', 'data', { type: 'poll', intervalMs: 2000, loader: () => new Promise((resolve) => { finish = resolve }) })
  await settle(); await sandbox.api.ui.removeView('view')
  finish('stale'); await settle()
  assert.ok(calls.some((call) => call.operation === 'ui.unbindPoll'))
  assert.ok(!calls.some((call) => call.operation === 'ui.updateView'))
  await sandbox.dispose()
})

function installWorker(t) {
  const originalWorker = globalThis.Worker, originalUrl = globalThis.URL
  class FakeWorker {
    static latest
    constructor() { FakeWorker.latest = this; this.messages = [] }
    postMessage(message) {
      this.messages.push(message)
      if (['start', 'stop', 'action'].includes(message.type)) queueMicrotask(() => this.emit({ type: 'result', requestId: message.requestId, ok: true, resultJson: 'null', status: message.type === 'start' ? 'started' : message.type === 'stop' ? 'stopped' : undefined }))
    }
    emit(data) { this.onmessage?.({ data }) }
    terminate() { this.terminated = true }
  }
  globalThis.Worker = FakeWorker
  globalThis.URL = class { constructor(path) { this.href = path } }
  t.after(() => { globalThis.Worker = originalWorker; globalThis.URL = originalUrl })
  return FakeWorker
}

test('trusted runtime enforces call budget, validates extended UI and clears polling', async (t) => {
  const Worker = installWorker(t), service = new PluginRuntimeService(), views = [], calls = []
  await service.start('test', { id: 'test', name: 'Test', version: '1', api_version: '3', entrypoint: 'main.js', permissions: [] }, '', { hostCall: (operation) => { calls.push(operation); if (operation === 'runtime.getQuotaUsage') return { used_bytes: 0 } }, onView: (view) => views.push(view), onError() {} })
  const worker = Worker.latest
  const send = async (id, operation, args = {}) => { worker.emit({ type: 'host-call', requestId: '', id, operation, argsJson: JSON.stringify(args) }); await settle(); return JSON.parse(worker.messages.findLast((item) => item.type === 'host-call-result' && item.id === id).replyJson) }
  const view = { id: 'view', title: 'View', slot: 'home_widget', root: { type: 'table', id: 'rows', columns: [{ key: 'x', label: 'X' }], rows: [{ x: 1 }] } }
  assert.equal((await send('ui', 'ui.registerView', { view })).ok, true)
  assert.equal(views.length, 1)
  let root = { type: 'text', text: 'Leaf' }; for (let i = 0; i < 11; i++) root = { type: 'group', children: [root] }
  assert.equal((await send('depth', 'ui.updateView', { view: { ...view, root } })).ok, false)
  assert.equal((await send('poll', 'ui.bindPoll', { id: 'data-1', intervalMs: 1 })).ok, false)
  assert.equal((await send('poll2', 'ui.bindPoll', { id: 'data-1', intervalMs: 2000 })).ok, true)
  const quota = await send('quota', 'runtime.getQuotaUsage')
  assert.equal(quota.value.hostCalls.enforced, true)
  assert.equal(quota.value.hostCalls.callLimit, 180)
  for (let i = 5; i < 180; i++) assert.equal((await send('request-' + i, 'runtime.getInfo')).ok, true)
  assert.equal((await send('overflow', 'runtime.getInfo')).ok, false)
  await service.stop('test')
  assert.equal(worker.terminated, true)
  assert.equal(service.runtimeIds().length, 0)
})
