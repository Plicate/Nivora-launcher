import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'

// Раньше почти треть операций бэкенда отбивалась на desktop-мосте ошибкой
// «Операция GUI не разрешена desktop bridge»: Python-сторона операцию знала,
// а белый список в lib.rs — нет. Этот тест сверяет оба списка напрямую,
// чтобы новые операции больше не терялись между слоями.

function rustAllowList() {
  const rust = readFileSync(new URL('../src-tauri/src/lib.rs', import.meta.url), 'utf8')
  const start = rust.indexOf('const ALLOWED')
  assert.ok(start >= 0, 'lib.rs must declare the ALLOWED list')
  const body = rust.slice(start, rust.indexOf('if !ALLOWED.contains'))
  return new Set([...body.matchAll(/"([a-z0-9_]+)"/g)].map((match) => match[1]))
}

function pythonOperations() {
  const source = readFileSync(new URL('../../nivora_launcher/gui_service.py', import.meta.url), 'utf8')
  const operations = new Set()
  for (const match of source.matchAll(/operation == "([a-z0-9_]+)"/g)) operations.add(match[1])
  for (const block of source.matchAll(/operation in \{([^}]+)\}/g)) {
    for (const match of block[1].matchAll(/"([a-z0-9_]+)"/g)) operations.add(match[1])
  }
  assert.ok(operations.size > 100, 'gui_service.py dispatch looks unexpected')
  return operations
}

function frontendOperations() {
  const bridge = readFileSync(new URL('../src/services/bridge.ts', import.meta.url), 'utf8')
  const operations = new Set()
  for (const match of bridge.matchAll(/request(<[^>]*>)?\(\s*'([a-z0-9_]+)'/g)) operations.add(match[2])
  for (const match of bridge.matchAll(/request(<[^>]*>)?\(\s*`([a-z0-9_]+)`/g)) operations.add(match[2])
  assert.ok(operations.size > 100, 'bridge.ts request surface looks unexpected')
  return operations
}

test('every backend operation passes the desktop bridge allow list', () => {
  const allowed = rustAllowList()
  const missing = [...pythonOperations()].filter((operation) => !allowed.has(operation)).sort()
  assert.deepEqual(missing, [], 'operations implemented in Python but rejected by the desktop bridge')
})

test('every bridge.ts request passes the desktop bridge allow list', () => {
  const allowed = rustAllowList()
  const missing = [...frontendOperations()].filter((operation) => !allowed.has(operation)).sort()
  assert.deepEqual(missing, [], 'operations called by the UI but rejected by the desktop bridge')
})

test('known regressions stay admitted', () => {
  const allowed = rustAllowList()
  for (const operation of [
    'settings_profiles_list', 'settings_profile_save', 'settings_profile_apply',
    'theme_import', 'theme_export', 'github_theme_search', 'github_theme_install',
    'world_backup_list', 'world_backup_create', 'world_backup_restore',
    'cache_stats', 'cache_prune', 'launch_history', 'system_metrics',
    'mod_dependency_graph', 'mod_removal_impact', 'export_modpack',
    'optimization_analyze', 'optimization_apply', 'jvm_presets_list',
    'modpack_import', 'overlay_state', 'bedrock_setup_version',
  ]) {
    assert.ok(allowed.has(operation), operation)
  }
})
