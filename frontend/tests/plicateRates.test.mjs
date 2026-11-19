import assert from 'node:assert/strict'
import test from 'node:test'
import { createPlicateRatesApi, DEFAULT_PLICATE_API_BASE_URL, externalJoinServer, isRatesJoinableInstance, serverAddress } from '../src/services/plicateRates.ts'

function response(data, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: { get: (name) => name.toLowerCase() === 'content-type' ? 'application/json' : null },
    json: async () => data,
    text: async () => JSON.stringify(data),
  }
}

test('Plicate Rates defaults to the same-domain /api base path', () => {
  assert.equal(DEFAULT_PLICATE_API_BASE_URL, 'https://plicate.art/api')
})

test('Plicate Rates catalog uses the existing public servers API and preserves server data', async () => {
  const calls = []
  const api = createPlicateRatesApi('https://rates.example.test/', async (url, init) => {
    calls.push({ url, init })
    return response({ servers: [{ slug: 'green-world', name: 'Green World', host: 'play.example.test', port: 25565, edition: 'Java', online: 17, maximum: 100 }], categories: ['Выживание'] })
  })

  const catalog = await api.listServers()

  assert.deepEqual(catalog, {
    servers: [{ slug: 'green-world', name: 'Green World', host: 'play.example.test', port: 25565, edition: 'Java', online: 17, maximum: 100 }],
    categories: ['Выживание'],
  })
  assert.equal(calls[0].url, 'https://rates.example.test/api/servers')
  assert.equal(calls[0].init.method, undefined)
  assert.equal(calls[0].init.credentials, 'omit')
})

test('Plicate Rates accepts a same-domain /api base and rejects unrelated paths', async () => {
  const calls = []
  const api = createPlicateRatesApi('https://plicate.art/api/', async (url) => {
    calls.push(url)
    return response({ servers: [], categories: [] })
  })
  await api.listServers()
  assert.equal(calls[0], 'https://plicate.art/api/servers')
  assert.throws(() => createPlicateRatesApi('https://plicate.art/private'), /путём \/api/)
})

test('server status includes the normalized MOTD and favicon payload for its card', async () => {
  const server = { slug: 'green-world', name: 'Green World', host: 'play.example.test', port: 25565, edition: 'Java', online: 17, maximum: 100, motd: 'Welcome to Green World', icon: 'data:image/png;base64,iVBORw0KGgo=' }
  const api = createPlicateRatesApi('https://rates.example.test', async () => response({ servers: [server], categories: [] }))
  const catalog = await api.listServers()
  assert.equal(catalog.servers[0].motd, 'Welcome to Green World')
  assert.equal(catalog.servers[0].icon, server.icon)
})

test('catalog search is URL encoded through the same API query contract', async () => {
  const calls = []
  const api = createPlicateRatesApi('https://rates.example.test', async (url, init) => {
    calls.push({ url, init })
    return response({ servers: [], categories: [] })
  })

  await api.listServers('зелёный мир')

  assert.equal(calls[0].url, 'https://rates.example.test/api/servers?q=%D0%B7%D0%B5%D0%BB%D1%91%D0%BD%D1%8B%D0%B9+%D0%BC%D0%B8%D1%80')
})

test('copy IP records through the existing Rates copy endpoint after the UI copy succeeds', async () => {
  const calls = []
  const api = createPlicateRatesApi('https://rates.example.test', async (url, init) => {
    calls.push({ url, init })
    return response({ copied: true })
  })

  await api.recordCopy('green-world')

  assert.equal(calls[0].url, 'https://rates.example.test/api/servers/green-world/copy')
  assert.equal(calls[0].init.method, 'POST')
  assert.equal(calls[0].init.body, '{}')
  assert.equal(calls[0].init.credentials, 'omit')
})

test('quick join chooser only offers Java client instances', () => {
  assert.equal(isRatesJoinableInstance({ name: 'java', kind: 'client', game_edition: 'java' }), true)
  assert.equal(isRatesJoinableInstance({ name: 'legacy', kind: undefined, game_edition: undefined }), true)
  assert.equal(isRatesJoinableInstance({ name: 'bedrock', kind: 'client', game_edition: 'bedrock' }), false)
  assert.equal(isRatesJoinableInstance({ name: 'server', kind: 'server', game_edition: 'java' }), false)
})

test('вшитого каталога серверов нет: ошибки API доходят до интерфейса', async () => {
  const html404 = createPlicateRatesApi('https://rates.example.test', async () => ({
    ok: false,
    status: 404,
    headers: { get: () => 'text/html' },
    text: async () => '<!doctype html><title>404</title>',
  }))
  await assert.rejects(html404.listServers(), /вернул HTTP 404 вместо JSON API/)

  const unreachable = createPlicateRatesApi('https://rates.example.test', async () => {
    throw new TypeError('network is unreachable')
  })
  await assert.rejects(unreachable.listServers(), /network is unreachable/)
})

test('IP formatting only omits the edition default port', () => {
  assert.equal(serverAddress({ host: 'play.example.test', port: 25565, edition: 'Java' }), 'play.example.test')
  assert.equal(serverAddress({ host: 'play.example.test', port: 19132, edition: 'Bedrock' }), 'play.example.test')
  assert.equal(serverAddress({ host: 'play.example.test', port: 25566, edition: 'Java' }), 'play.example.test:25566')
  assert.equal(serverAddress({ host: 'play.example.test', port: 19133, edition: 'Bedrock' }), 'play.example.test:19133')
})

test('ошибки каталога и действий одинаково доходят до вызывающего кода', async () => {
  const api = createPlicateRatesApi('https://rates.example.test', async () => response({ error: 'Каталог временно недоступен.' }, 503))
  await assert.rejects(api.listServers(), /Каталог временно недоступен/)
  await assert.rejects(api.recordCopy('green-world'), /Каталог временно недоступен/)
})

test('цель nivora://join превращается в сервер для диалога выбора экземпляра', () => {
  // форма ответа shortcut_take_join_request из desktop-части лаунчера
  const java = externalJoinServer({ host: 'play.example.com', port: 25566, edition: 'Java', name: 'Зелёный мир', slug: 'green-world' })
  assert.deepEqual(java, { slug: 'quick-join', name: 'Зелёный мир', host: 'play.example.com', port: 25566, edition: 'Java', online: null, maximum: null })

  // значения по умолчанию как в rust-парсере ссылки
  const defaults = externalJoinServer({ host: 'rates.example.net', port: 25565, edition: 'Java', name: 'rates.example.net', slug: 'quick-join' })
  assert.equal(defaults.port, 25565)
  assert.equal(defaults.name, 'rates.example.net')

  const bedrock = externalJoinServer({ host: 'bedrock.example.net', port: 19132, edition: 'Bedrock', name: 'Bedrock World', slug: 'x' })
  assert.equal(bedrock.edition, 'Bedrock')
  assert.equal(bedrock.port, 19132)

  // пустой адрес не открывает диалог
  assert.equal(externalJoinServer({ host: '   ', port: 25565, edition: 'Java', name: 'x', slug: 'x' }), null)
  assert.equal(externalJoinServer({ host: '', edition: 'Java', name: '', slug: '' }), null)
  // имя подставляется из адреса, порт — по изданию
  assert.deepEqual(
    externalJoinServer({ host: 'play.example.com', edition: 'Java', name: '', slug: '' }),
    { slug: 'quick-join', name: 'play.example.com', host: 'play.example.com', port: 25565, edition: 'Java', online: null, maximum: null },
  )
})
