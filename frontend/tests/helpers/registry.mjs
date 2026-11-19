import { readFileSync } from 'node:fs'

function listBody(file, marker) {
  const source = readFileSync(new URL(`../../src-tauri/src/rust_engine/${file}`, import.meta.url), 'utf8')
  const start = source.indexOf(marker)
  if (start < 0) throw new Error(`${file}: не найден ${marker}`)
  return source.slice(start, source.indexOf('];', start)).replace(/\/\/.*$/gm, '')
}

export function bridgeAllowList() {
  const operations = new Set()
  for (const match of listBody('mod.rs', 'pub const NATIVE_OPS').matchAll(/"([a-z0-9_]+)"/g)) {
    operations.add(match[1])
  }
  return operations
}
