import assert from 'node:assert/strict'
import { registerHooks } from 'node:module'
import test from 'node:test'

// Load the real host dispatcher while replacing only desktop RPC and dialogs.
// This keeps these tests runnable without a Tauri window or Vue application.
const hostUrl = new URL('../src/services/pluginHost.ts', import.meta.url).href
const bridgeUrl = `data:text/javascript,${encodeURIComponent(`
  export const calls = [];
  function record(operation, args, result) {
    calls.push({ operation, args });
    return Promise.resolve(result);
  }
  export const bridge = {
    pluginOperationTrace: () => Promise.resolve({ recorded: true }),
    pluginApiFilesList: (...args) => record('list', args, { files: ['sample.txt'] }),
    pluginApiFilesRead: (...args) => record('read', args, { content: 'sample' }),
    pluginApiFilesWrite: (...args) => record('write', args, { saved: true }),
    pluginApiFilesRemove: (...args) => record('remove', args, { removed: true }),
  };
`)}`
const dialogsUrl = `data:text/javascript,${encodeURIComponent(`
  const unexpected = () => { throw new Error('File operations must not open dialogs'); };
  export const alertDialog = unexpected;
  export const chooseDialog = unexpected;
  export const confirmDialog = unexpected;
  export const promptDialog = unexpected;
`)}`
const hooks = registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL === hostUrl && specifier === './bridge') {
      return { url: bridgeUrl, shortCircuit: true }
    }
    if (context.parentURL === hostUrl && specifier === './dialogs') {
      return { url: dialogsUrl, shortCircuit: true }
    }
    return nextResolve(specifier, context)
  },
})
let createPluginHostCall
let calls
try {
  ;({ createPluginHostCall } = await import(hostUrl))
  ;({ calls } = await import(bridgeUrl))
} finally {
  hooks.deregister()
}

function host(selected = { id: 'selected-instance', name: 'Display name' }) {
  return createPluginHostCall({
    pluginId: 'file-scope-test',
    selectedInstance: () => selected,
  })
}

for (const scope of ['plugin', 'instance', 'launcher-root-readonly']) {
  test(`file listing and reading accept ${scope}`, async () => {
    calls.length = 0
    const signal = new AbortController().signal
    const invoke = host()
    assert.deepEqual(await invoke('files.list', { scope, directory: 'docs' }, signal), ['sample.txt'])
    assert.equal(await invoke('files.readText', { scope, path: 'docs/sample.txt' }, signal), 'sample')
    const instance = scope === 'instance' ? 'selected-instance' : ''
    assert.deepEqual(calls, [
      { operation: 'list', args: ['file-scope-test', scope, 'docs', instance, signal] },
      { operation: 'read', args: ['file-scope-test', scope, 'docs/sample.txt', instance, signal] },
    ])
  })
}

for (const scope of ['plugin', 'instance']) {
  test(`file writing and removal accept ${scope}`, async () => {
    calls.length = 0
    const signal = new AbortController().signal
    const invoke = host()
    assert.equal(await invoke('files.writeText', { scope, path: 'sample.txt', content: 'new text' }, signal), undefined)
    assert.equal(await invoke('files.remove', { scope, path: 'sample.txt' }, signal), true)
    const instance = scope === 'instance' ? 'selected-instance' : ''
    assert.deepEqual(calls, [
      { operation: 'write', args: ['file-scope-test', scope, 'sample.txt', 'new text', instance, signal] },
      { operation: 'remove', args: ['file-scope-test', scope, 'sample.txt', instance, signal] },
    ])
  })
}

for (const operation of ['files.writeText', 'files.remove']) {
  test(`${operation} rejects launcher-root-readonly before desktop RPC`, async () => {
    calls.length = 0
    await assert.rejects(
      host(null)(operation, { scope: 'launcher-root-readonly', path: 'config.yml', content: 'changed' }),
      { name: 'TypeError', message: /launcher-root-readonly is read-only/ },
    )
    assert.deepEqual(calls, [])
  })
}

test('file operations reject invalid scopes before desktop RPC', async () => {
  calls.length = 0
  for (const operation of ['files.list', 'files.readText', 'files.writeText', 'files.remove']) {
    for (const scope of [undefined, null, '', 'launcher-root', 'filesystem:unrestricted', 1, {}]) {
      await assert.rejects(
        host()(operation, { scope, path: 'sample.txt', content: 'sample' }),
        { name: 'TypeError', message: /scope must be/ },
      )
    }
  }
  assert.deepEqual(calls, [])
})

test('instance file operations require an explicitly selected instance', async () => {
  calls.length = 0
  for (const operation of ['files.list', 'files.readText', 'files.writeText', 'files.remove']) {
    await assert.rejects(
      host(null)(operation, { scope: 'instance', path: 'sample.txt', content: 'sample' }),
      /Select an instance/,
    )
  }
  assert.deepEqual(calls, [])
})
