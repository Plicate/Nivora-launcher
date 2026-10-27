import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import ts from 'typescript'

const component = readFileSync(new URL('../src/components/ServerAccessPanel.vue', import.meta.url), 'utf8')
const script = component.match(/<script setup lang="ts">([\s\S]*?)<\/script>/)?.[1]
assert.ok(script, 'ServerAccessPanel has a script setup block')
const helperSource = script.slice(script.indexOf('function playerName'), script.indexOf('function parseProperties'))
const helpers = new Function(`${ts.transpile(helperSource)}; return { playerName, parseJsonArray, parseWhitelist, parseOps }`)()

test('RCON player names accept only valid Minecraft usernames', () => {
  assert.equal(helpers.playerName('Steve_1'), 'Steve_1')
  for (const name of ['ab', 'a'.repeat(17), 'two words', 'Steve\nwhitelist on', 'Alex;op Steve', 'Игрок']) {
    assert.equal(helpers.playerName(name), null, name)
  }
})

test('malformed whitelist and operator JSON is rejected instead of becoming an empty list', () => {
  assert.throws(() => helpers.parseJsonArray('{', 'whitelist.json'), /некорректный JSON/)
  assert.throws(() => helpers.parseJsonArray('{}', 'ops.json'), /ожидался JSON-массив/)
  assert.throws(() => helpers.parseWhitelist('[{"name":"Steve\\nwhitelist on"}]'), /некорректное имя/)
  assert.throws(() => helpers.parseOps('[{"name":"Alex","level":9}]'), /некорректный уровень/)
  assert.deepEqual(helpers.parseWhitelist('[{"name":"Steve"}]'), [{ name: 'Steve' }])
  assert.deepEqual(helpers.parseOps('[{"name":"Alex","level":3}]'), [{ name: 'Alex', level: 3 }])
})

test('loads reject stale server results and only treat confirmed absent files as optional', () => {
  assert.match(script, /let loadGeneration = 0/)
  assert.match(script, /const generation = \+\+loadGeneration/)
  assert.match(script, /generation !== loadGeneration \|\| serverName !== props\.name/)
  assert.match(script, /if \(message !== 'Файл недоступен' && message !== 'Текстовый файл не найден или недоступен'\) throw error/)
  assert.match(script, /const \{ files \} = await bridge\.serverFileList\(serverName\)/)
  assert.match(script, /if \(!files\.some\(\(file\) => file\.kind === 'file' && file\.path === path\)\) return null\s+throw error/)
})

test('mutations use their captured server name for bridge writes and RCON commands', () => {
  assert.match(script, /async function sendCommand\(serverName: string, command: string\)/)
  assert.match(script, /bridge\.serverCommand\(serverName, command\)/)
  assert.match(script, /bridge\.serverFileWrite\(serverName, 'whitelist\.json'/)
  assert.match(script, /bridge\.serverFileWrite\(serverName, 'ops\.json'/)
  assert.match(script, /bridge\.serverFileWrite\(serverName, 'server\.properties'/)
  assert.doesNotMatch(script, /bridge\.serverCommand\(props\.name/)
})
