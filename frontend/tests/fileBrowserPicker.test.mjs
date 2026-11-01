import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'

const source = readFileSync(new URL('../src/services/fileBrowser.ts', import.meta.url), 'utf8')

function extractFunction(startMarker, endMarker, from, to) {
  const start = source.indexOf(startMarker)
  assert.ok(start >= 0, `${startMarker} must exist in fileBrowser.ts`)
  const end = source.indexOf(endMarker, start)
  assert.ok(end > start, `${endMarker} must delimit ${startMarker}`)
  return source.slice(start, end + 2).replace(from, to).replace('(value: PickedFile | null)', '(value)')
}

const fallback = extractFunction(
  'function browserFallbackPick(',
  '\n}\n\n/** открывает picker по сохранённому режиму проводника',
  'function browserFallbackPick(accept: string, maxSize?: number): Promise<PickedFile | null>',
  'function browserFallbackPick(accept, maxSize)',
)
const readBase64 = extractFunction(
  'export async function readFileBase64(',
  '\n}\n\nfunction browserFallbackPick',
  'export async function readFileBase64(path: string, onProgress?: (loaded: number, total: number) => void): Promise<{ data: string; size: number }>',
  'async function readFileBase64(path, onProgress)',
)
const picker = extractFunction(
  'export function pickFile(',
  '\n}\n\n/** открывает каталог',
  'export function pickFile(options: { accept?: string; title?: string; startPath?: string; maxSize?: number } = {}): Promise<PickedFile | null>',
  'function pickFile(options = {})',
)

function makePicker(mode, desktop = true) {
  const fileBrowserRequest = { value: null }
  const harness = new Function('isDesktopShell', 'explorerMode', 'fileBrowserRequest', 'bridge', `
    let fileBrowserResolve = null
    ${readBase64}
    ${fallback}
    ${picker}
    function resolveFileBrowser(file) {
      fileBrowserRequest.value = null
      fileBrowserResolve?.(file)
      fileBrowserResolve = null
    }
    return { pickFile, readFileBase64, resolveFileBrowser, fileBrowserRequest }
  `)
  const bridge = { fsReadFile: async () => { throw new Error('unexpected file read') } }
  return { ...harness(() => desktop, { value: mode }, fileBrowserRequest, bridge), bridge }
}

function installNativePicker(files) {
  let input
  globalThis.document = {
    body: { appendChild() {} },
    createElement() {
      const listeners = new Map()
      input = {
        files,
        style: {},
        addEventListener(name, listener) { listeners.set(name, listener) },
        remove() {},
        click() { listeners.get('change')?.() },
      }
      return input
    },
  }
  return () => input
}

class MockFileReader {
  result = ''
  static calls = 0
  readAsDataURL() {
    MockFileReader.calls += 1
    this.result = 'data:image/png;base64,QUJD'
    this.onload?.()
  }
}

globalThis.FileReader = MockFileReader

test('builtin mode opens the existing file browser and preserves its picked payload', async () => {
  const picker = makePicker('builtin')
  const result = picker.pickFile({ accept: '.png,.jpg', title: 'Choose image', maxSize: 15 * 1024 * 1024 })
  assert.deepEqual(picker.fileBrowserRequest.value, {
    mode: 'open',
    accept: '.png,.jpg',
    title: 'Choose image',
    maxSize: 15 * 1024 * 1024,
  })

  const picked = { fileName: 'original image.png', data: 'QUJD', size: 3 }
  picker.resolveFileBrowser(picked)
  assert.strictEqual(await result, picked)
})

test('builtin picker stops reading once the selected file exceeds its cap', async () => {
  const picker = makePicker('builtin')
  const maxSize = 15 * 1024 * 1024
  picker.fileBrowserRequest.value = { mode: 'open', maxSize }
  let reads = 0
  picker.bridge.fsReadFile = async () => {
    reads += 1
    return { size: maxSize + 1, data: 'QUJD', length: 3, eof: false }
  }
  await assert.rejects(picker.readFileBase64('/large'), /15 MiB/)
  assert.equal(reads, 1)
})

test('system mode uses a native input and preserves its original filename and base64', async () => {
  MockFileReader.calls = 0
  installNativePicker([{ name: 'original image.png', size: 3 }])
  const picked = await makePicker('system').pickFile({ accept: '.png', maxSize: 15 })
  assert.deepEqual(picked, { fileName: 'original image.png', data: 'QUJD', size: 3 })
  assert.equal(MockFileReader.calls, 1)
})

test('browser runtime stays on the native input and oversized files are not read', async () => {
  MockFileReader.calls = 0
  installNativePicker([{ name: 'large.png', size: 16 }])
  const picked = await makePicker('builtin', false).pickFile({ accept: '.png', maxSize: 15 })
  assert.deepEqual(picked, { fileName: 'large.png', data: '', size: 16 })
  assert.equal(MockFileReader.calls, 0)
})
