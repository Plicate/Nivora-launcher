import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'


function frontendStamp() {
  const source = readFileSync(new URL('../src/services/bridge.ts', import.meta.url), 'utf8')
  const match = source.match(/FRONTEND_BUILD_STAMP = '([^']+)'/)
  assert.ok(match, 'bridge.ts должен объявлять FRONTEND_BUILD_STAMP')
  return match[1]
}

function rustEngineStamp() {
  const source = readFileSync(new URL('../src-tauri/src/rust_engine/constants.rs', import.meta.url), 'utf8')
  const match = source.match(/BUILD_STAMP: &str = "([^"]+)"/)
  assert.ok(match, 'rust_engine/constants.rs должен объявлять BUILD_STAMP')
  return match[1]
}

test('метки сборки фронтенда и нативного движка совпадают', () => {
  assert.ok(frontendStamp(), 'метка фронтенда не пустая')
  assert.ok(rustEngineStamp(), 'метка движка не пустая')
  assert.equal(frontendStamp(), rustEngineStamp(), 'Rust-движок отчитывается своей меткой в bootstrap')
})
