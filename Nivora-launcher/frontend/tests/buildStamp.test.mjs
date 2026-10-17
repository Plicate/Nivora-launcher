import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'

// Метки сборки интерфейса и Python-бэкенда обязаны совпадать: рассинхрон
// включает в приложении баннер «части лаунчера собраны из разных версий».
// Этот тест ловит забытую правку одной из сторон при подготовке релиза.

function frontendStamp() {
  const source = readFileSync(new URL('../src/services/bridge.ts', import.meta.url), 'utf8')
  const match = source.match(/FRONTEND_BUILD_STAMP = '([^']+)'/)
  assert.ok(match, 'bridge.ts должен объявлять FRONTEND_BUILD_STAMP')
  return match[1]
}

function backendStamp() {
  const source = readFileSync(new URL('../../nivora_launcher/constants.py', import.meta.url), 'utf8')
  const match = source.match(/BUILD_STAMP = "([^"]+)"/)
  assert.ok(match, 'constants.py должен объявлять BUILD_STAMP')
  return match[1]
}

test('метки сборки фронтенда и бэкенда совпадают', () => {
  assert.ok(frontendStamp(), 'метка фронтенда не пустая')
  assert.ok(backendStamp(), 'метка бэкенда не пустая')
  assert.equal(frontendStamp(), backendStamp())
})
