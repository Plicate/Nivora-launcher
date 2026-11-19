import assert from 'node:assert/strict'
import test from 'node:test'
import { PluginRuntimeService } from '../src/services/pluginRuntime.ts'

test('dialogs get a five-minute cap without extending ordinary requests', async (t) => {
  const originalWorker = globalThis.Worker
  const originalUrl = globalThis.URL
  const calls = []

  class FakeWorker {
    static instances = []

    constructor() {
      this.messages = []
      this.onmessage = null
      this.onerror = null
      this.onmessageerror = null
      this.terminated = false
      FakeWorker.instances.push(this)
    }

    postMessage(message) {
      this.messages.push(message)
      if (message.type === 'start' || message.type === 'stop') {
        queueMicrotask(() => this.emit({
          type: 'result',
          requestId: message.requestId,
          ok: true,
          resultJson: 'null',
          status: message.type === 'start' ? 'started' : 'stopped',
        }))
      }
    }

    emit(data) {
      this.onmessage?.({ data })
    }

    terminate() {
      this.terminated = true
    }
  }

  let resolveDialog
  const service = new PluginRuntimeService()
  globalThis.Worker = FakeWorker
  globalThis.URL = class { constructor(path) { this.href = path } }
  t.mock.timers.enable({ apis: ['Date', 'setTimeout'] })

  try {
    await service.start('dialog-test', {
      id: 'dialog-test',
      name: 'Dialog Test',
      version: '1.0.0',
      api_version: '2',
      entrypoint: 'main.js',
      permissions: [],
    }, '', {
      hostCall(operation) {
        calls.push(operation)
        return new Promise((resolve) => { resolveDialog = resolve })
      },
      onView() {},
      onError(error) { throw error },
    })

    const worker = FakeWorker.instances[0]
    const commandResult = service.runCommand('dialog-test', 'confirm', {}).then(
      () => 'resolved',
      (error) => `rejected: ${error.message}`,
    )
    const command = worker.messages.findLast((message) => message.type === 'command')
    worker.emit({
      type: 'host-call',
      id: 'host-dialog-1',
      requestId: command.requestId,
      operation: 'launcher.dialogs.confirm',
      argsJson: '{}',
    })
    await new Promise((resolve) => setImmediate(resolve))

    assert.deepEqual(calls, ['launcher.dialogs.confirm'])
    assert.equal(typeof resolveDialog, 'function')
    t.mock.timers.tick(5001)
    assert.equal(worker.terminated, false)

    resolveDialog(true)
    await new Promise((resolve) => setImmediate(resolve))
    assert.ok(worker.messages.some((message) => message.type === 'host-call-result'))
    worker.emit({ type: 'result', requestId: command.requestId, ok: true, resultJson: 'null' })
    assert.equal(await commandResult, 'resolved')

    const ordinaryResult = service.emit('dialog-test', 'launcher.ready', { version: '1.0.0' }).then(
      () => 'resolved',
      (error) => `rejected: ${error.message}`,
    )
    t.mock.timers.tick(5001)
    assert.equal(await ordinaryResult, 'rejected: Plugin runtime timed out during event')
    assert.equal(worker.terminated, true)

    await service.start('dialog-limit-test', {
      id: 'dialog-limit-test',
      name: 'Dialog Limit Test',
      version: '1.0.0',
      api_version: '2',
      entrypoint: 'main.js',
      permissions: [],
    }, '', {
      hostCall(operation) {
        calls.push(operation)
        return new Promise(() => {})
      },
      onView() {},
      onError(error) { throw error },
    })
    const limitWorker = FakeWorker.instances.at(-1)
    const timedOutCommand = service.runCommand('dialog-limit-test', 'confirm', {}).then(
      () => 'resolved',
      (error) => `rejected: ${error.message}`,
    )
    const limitCommand = limitWorker.messages.findLast((message) => message.type === 'command')
    limitWorker.emit({
      type: 'host-call',
      id: 'host-dialog-limit',
      requestId: limitCommand.requestId,
      operation: 'launcher.dialogs.confirm',
      argsJson: '{}',
    })
    await new Promise((resolve) => setImmediate(resolve))
    t.mock.timers.tick(5 * 60 * 1000 + 1)
    assert.equal(await timedOutCommand, 'rejected: Plugin runtime timed out during command')
    assert.equal(limitWorker.terminated, true)
  } finally {
    await service.dispose()
    t.mock.timers.reset()
    globalThis.Worker = originalWorker
    globalThis.URL = originalUrl
  }
})

test('runtime start supports api v3 manifests and forwards them to the worker', async (t) => {
  const originalWorker = globalThis.Worker
  const originalUrl = globalThis.URL

  class FakeWorker {
    static instances = []

    constructor() {
      this.messages = []
      this.onmessage = null
      this.onerror = null
      this.onmessageerror = null
      this.terminated = false
      FakeWorker.instances.push(this)
    }

    postMessage(message) {
      this.messages.push(message)
      if (message.type === 'start' || message.type === 'stop') {
        queueMicrotask(() => this.emit({
          type: 'result',
          requestId: message.requestId,
          ok: true,
          resultJson: 'null',
          status: message.type === 'start' ? 'started' : 'stopped',
        }))
      }
    }

    emit(data) {
      this.onmessage?.({ data })
    }

    terminate() {
      this.terminated = true
    }
  }

  const service = new PluginRuntimeService()
  globalThis.Worker = FakeWorker
  globalThis.URL = class { constructor(path) { this.href = path } }

  try {
    const manifest = {
      id: 'runtime-v3-test',
      name: 'Runtime V3 Test',
      version: '1.0.0',
      api_version: '3',
      entrypoint: 'main.js',
      permissions: ['runtime:observe', 'network:proxy-control'],
      sandbox_profile: 'strict',
      capabilities: { network: { proxy: true } },
      compatibility_matrix: { min_launcher_version: '0.1.0' },
    }
    await service.start('runtime-v3-test', manifest, 'export default { activate() {} }', {
      hostCall() {
        return Promise.resolve(undefined)
      },
      onView() {},
      onError(error) { throw error },
    })

    const worker = FakeWorker.instances[0]
    const startMessage = worker.messages.find((message) => message.type === 'start')
    assert.ok(startMessage)
    assert.equal(startMessage.manifestJson.includes('"api_version":"3"'), true)
    assert.equal(startMessage.manifestJson.includes('"sandbox_profile":"strict"'), true)
    assert.equal(startMessage.manifestJson.includes('"network:proxy-control"'), true)
    assert.deepEqual(service.runtimeIds(), ['runtime-v3-test'])
  } finally {
    await service.dispose()
    globalThis.Worker = originalWorker
    globalThis.URL = originalUrl
  }
})

test('UI v2 принимает новые узлы и режет мусор', async () => {
  const { isUiViewForTests } = await import('../src/services/pluginRuntime.ts')
  const view = (root) => ({ id: 'v', slot: 'sidebar_tab', title: 'V', root })
  const accepted = [
    { type: 'textarea', id: 'notes', label: 'Notes', value: 'x', rows: 6, onChange: 'changed' },
    { type: 'slider', id: 'fps', label: 'FPS', min: 30, max: 144, step: 6, value: 60, unit: 'fps' },
    { type: 'number', id: 'count', label: 'Count', value: 5, min: 0, max: 10, unit: 'шт' },
    { type: 'color', id: 'accent', label: 'Accent', value: '#8b653f' },
    { type: 'datetime', id: 'when', label: 'Когда', value: '2026-01-01T10:00' },
    { type: 'switch', id: 'auto', label: 'Авто', checked: true, hint: 'подсказка' },
    { type: 'metric', id: 'players', label: 'Игроки', value: '12', unit: 'онлайн', tone: 'success' },
    { type: 'alert', id: 'warn', title: 'Внимание', text: 'Проверьте настройки', tone: 'warning' },
    { type: 'code', id: 'snippet', text: 'say hello' },
    { type: 'image', id: 'logo', url: 'https://example.com/l.png', alt: 'logo', height: 120 },
    { type: 'card', id: 'box', label: 'Блок', tone: 'muted', children: [{ type: 'text', text: 'внутри' }] },
    { type: 'grid', id: 'g', columns: 3, children: [{ type: 'badge', label: '1' }] },
    { type: 'form', id: 'f', submitAction: 'go', fields: [{ type: 'slider', id: 'fps', label: 'FPS', min: 0, max: 100 }] },
  ]
  for (const root of accepted) assert.equal(isUiViewForTests(view(root)), true, JSON.stringify(root))
  const rejected = [
    { type: 'slider', id: 'fps', label: 'FPS', min: 100, max: 10 },
    { type: 'slider', id: 'fps', label: 'FPS' },
    { type: 'color', id: 'c', label: 'C', value: '12zz34' },
    { type: 'grid', id: 'g', columns: 9, children: [] },
    { type: 'grid', id: 'g', columns: 2.5, children: [] },
    { type: 'switch', id: 's', label: 'S', checked: 'yes' },
    { type: 'metric', id: 'm', label: 'M' },
    { type: 'card', id: 'c', children: 'nope' },
    { type: 'textarea', id: 't', label: 'T', rows: '6' },
    { type: 'datetime', id: 'd', label: 'D', config_key: 'x', extra: 1 },
    { type: 'unknown-new', id: 'x' },
  ]
  for (const root of rejected) assert.equal(isUiViewForTests(view(root)), false, JSON.stringify(root))
})
