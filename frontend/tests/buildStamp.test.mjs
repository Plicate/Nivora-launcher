import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'

// Метки сборки интерфейса и бэкендов обязаны совпадать: рассинхрон включает
// в приложении баннер «части лаунчера собраны из разных версий». Раньше
// сверялись только фронтенд и Python — Rust-движок разъехался на семь
// релизов незамеченным. Этот тест ловит забытую правку любой из сторон.

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

function rustEngineStamp() {
  const source = readFileSync(new URL('../src-tauri/src/rust_engine/constants.rs', import.meta.url), 'utf8')
  const match = source.match(/BUILD_STAMP: &str = "([^"]+)"/)
  assert.ok(match, 'rust_engine/constants.rs должен объявлять BUILD_STAMP')
  return match[1]
}

test('метки сборки фронтенда и бэкендов совпадают', () => {
  assert.ok(frontendStamp(), 'метка фронтенда не пустая')
  assert.ok(backendStamp(), 'метка бэкенда не пустая')
  assert.equal(frontendStamp(), backendStamp())
  assert.equal(frontendStamp(), rustEngineStamp(), 'Rust-движок отчитывается своей меткой в bootstrap')
})
