import assert from 'node:assert/strict'
import test from 'node:test'
import fs from 'node:fs'
import vm from 'node:vm'
import { stripTypeScriptTypes } from 'node:module'
import { PluginRuntimeService } from '../src/services/pluginRuntime.ts'

function facade() {
  const source = fs.readFileSync(new URL('../src/plugins/quickjs.worker.ts', import.meta.url), 'utf8')
  const begin = source.indexOf('function facadeSource(')
  const end = source.indexOf('\nfunction invokeInternal(', begin)
  const builder = stripTypeScriptTypes(source.slice(begin, end))
  const build = vm.runInNewContext(builder + '\nfacadeSource', { quoteJs: JSON.stringify, MAX_RPC_BYTES: 4 * 1024 * 1024, MAX_VIEW_BYTES: 256 * 1024 })
  const calls = [], sandbox = {
    __hostCall: async (operation, args) => {
      calls.push({ operation, args: JSON.parse(args) })
      return JSON.stringify({ ok: true, hasValue: true, value: null })
    },
  }
  sandbox.entry = (api) => { sandbox.api = api }
  vm.runInNewContext(build(JSON.stringify({ id: 'test', api_version: '3', i18n: {}, events: ['settings.changed'] }), { start: 'start', dispatch: 'dispatch', dispose: 'dispose', defaultExport: 'entry' }), sandbox)
  return { sandbox, calls }
}
const settle = () => new Promise((resolve) => setImmediate(resolve))

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
