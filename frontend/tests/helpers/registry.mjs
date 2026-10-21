import { readFileSync } from 'node:fs'

// Белый список desktop-моста = NATIVE_OPS (rust_engine/mod.rs) ∪ WORKER_OPS
// (rust_engine/registry.rs). Отдельного массива в lib.rs больше нет.
function listBody(file, marker) {
  const source = readFileSync(new URL(`../../src-tauri/src/rust_engine/${file}`, import.meta.url), 'utf8')
  const start = source.indexOf(marker)
  if (start < 0) throw new Error(`${file}: не найден ${marker}`)
  return source.slice(start, source.indexOf('];', start)).replace(/\/\/.*$/gm, '')
}

export function bridgeAllowList() {
  const operations = new Set()
  for (const [file, marker] of [['mod.rs', 'pub const NATIVE_OPS'], ['registry.rs', 'pub const WORKER_OPS']]) {
    for (const match of listBody(file, marker).matchAll(/"([a-z0-9_]+)"/g)) operations.add(match[1])
  }
  return operations
}
