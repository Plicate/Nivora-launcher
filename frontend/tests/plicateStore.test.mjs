import assert from 'node:assert/strict'
import test from 'node:test'
import { compareStoreVersions, createPlicateStoreApi, isPresetStoreItem, presetStoreItems } from '../src/services/plicateStore.ts'

function jsonResponse(data, status = 200) {
  return new Response(JSON.stringify(data), { status, headers: { 'content-type': 'application/json' } })
}

const theme = { id: 'soft-night', name: 'Soft Night', author: 'Plicate', description: 'A calm palette', version: '1.2.0', kind: 'theme', download_url: '/api/store/items/soft-night.zip', screenshots: [{ url: '/api/store/media/0123456789abcdef0123456789abcdef.png' }] }

test('Plicate Store uses public catalog and omits publisher cookies', async () => {
  const calls = []
  const api = createPlicateStoreApi('https://store.example.test/', async (url, init) => {
    calls.push({ url, init })
    return jsonResponse({ items: [theme, { ...theme, id: '../bad' }] })
  })
  const items = await api.listCatalog()
  assert.deepEqual(items, [theme])
  assert.equal(calls[0].url, 'https://store.example.test/api/store/catalog')
  assert.equal(calls[0].init.credentials, 'omit')
  assert.equal(api.assetUrl(theme.screenshots[0].url), 'https://store.example.test/api/store/media/0123456789abcdef0123456789abcdef.png')
})

test('Plicate Store accepts the same-domain /api base path', async () => {
  const api = createPlicateStoreApi('https://plicate.art/api', async (url) => {
    assert.equal(url, 'https://plicate.art/api/store/catalog')
    return jsonResponse({ items: [] })
  })
  assert.deepEqual(await api.listCatalog(), [])
  assert.throws(() => createPlicateStoreApi('https://plicate.art/not-api'), /путём \/api/)
})

test('Plicate Store downloads a bounded ZIP archive and encodes it for the launcher importer', async () => {
  const api = createPlicateStoreApi('https://store.example.test', async (url, init) => {
    assert.equal(url, 'https://store.example.test/api/store/items/soft-night.zip')
    assert.equal(init.credentials, 'omit')
    return new Response(Uint8Array.from([0x50, 0x4b, 0x03, 0x04]), { headers: { 'content-type': 'application/zip' } })
  })
  assert.deepEqual(await api.download(theme), { name: 'soft-night.zip', data: 'UEsDBA==' })
})

test('Plicate Store rejects untrusted origins, asset paths and non-ZIP responses', async () => {
  assert.throws(() => createPlicateStoreApi('http://store.example.test'), /HTTPS/)
  let calls = 0
  const api = createPlicateStoreApi('https://store.example.test', async () => { calls += 1; return new Response('<html>404</html>', { headers: { 'content-type': 'text/html' } }) })
  assert.throws(() => api.assetUrl('https://attacker.invalid/track'), /небезопасную ссылку/)
  await assert.rejects(api.download({ ...theme, download_url: 'https://attacker.invalid/soft-night.zip' }), /небезопасную ссылку/)
  await assert.rejects(api.download(theme), /Вместо ZIP-архива/)
  await assert.rejects(api.listCatalog(), /HTML или другой формат/)
  assert.equal(calls, 2)
})

test('Store versions compare numeric release components', () => {
  assert.equal(compareStoreVersions('1.10.0', '1.9.9'), 1)
  assert.equal(compareStoreVersions('v2.0.0', '2.0.0'), 0)
  assert.equal(compareStoreVersions('1.0.0', '1.0.1'), -1)
})

const builtinTheme = { id: 'nivora-dark', schema_version: 1, name: 'Nivora Dark', author: 'Nivora Launcher', source: 'builtin', description: 'Основная тема лаунчера', sidebar_mode: 'adaptive' }
const localTheme = { ...builtinTheme, id: 'my-theme', source: 'local' }

test('Маркет строит офлайн-каталог из встроенных тем лаунчера', () => {
  const items = presetStoreItems([builtinTheme, localTheme])
  assert.equal(items.length, 1)
  assert.deepEqual(items[0], {
    id: 'nivora-dark',
    name: 'Nivora Dark',
    author: 'Nivora Launcher',
    description: 'Основная тема лаунчера',
    version: '1.0.0',
    kind: 'theme',
    download_url: '',
    preset: true,
  })
  assert.equal(isPresetStoreItem(items[0]), true)
})

test('Обычные элементы каталога не считаются встроенными темами', () => {
  assert.equal(isPresetStoreItem(theme), false)
  assert.equal(isPresetStoreItem({ ...theme, preset: true }), false)
  assert.equal(isPresetStoreItem({ ...builtinTheme, version: '1.0.0', kind: 'theme', download_url: '/api/store/items/nivora-dark.zip', preset: true }), false)
  assert.deepEqual(presetStoreItems([]), [])
})
