import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { bridgeAllowList } from './helpers/registry.mjs'

// Раньше почти треть операций бэкенда отбивалась на desktop-мосте ошибкой
// «Операция GUI не разрешена desktop bridge»: Python-сторона операцию знала,
// а белый список в lib.rs — нет. Этот тест сверяет оба списка напрямую,
// чтобы новые операции больше не терялись между слоями.

function rustAllowList() {
  return bridgeAllowList()
}

function pythonOperations() {
  const operations = new Set()
  for (const name of ['gui_service.py', 'gui_cloud_ops.py']) {
    const source = readFileSync(new URL(`../../nivora_launcher/${name}`, import.meta.url), 'utf8')
    for (const match of source.matchAll(/operation == "([a-z0-9_]+)"/g)) operations.add(match[1])
    for (const block of source.matchAll(/operation in \{([^}]+)\}/g)) {
      for (const match of block[1].matchAll(/"([a-z0-9_]+)"/g)) operations.add(match[1])
    }
  }
  assert.ok(operations.size > 100, 'gui_service.py dispatch looks unexpected')
  return operations
}

function frontendOperations() {
  const bridge = readFileSync(new URL('../src/services/bridge.ts', import.meta.url), 'utf8')
  const operations = new Set()
  // Дженерики бывают вложенными (Array<{...}>, import('../types')): простой
  // [^>]* терял такие вызовы, и новые операции проходили мимо проверки.
  for (const match of bridge.matchAll(/\brequest\b/g)) {
    let cursor = match.index + 'request'.length
    if (bridge[cursor] === '<') {
      let depth = 0
      for (; cursor < bridge.length; cursor += 1) {
        const char = bridge[cursor]
        if (char === '<') depth += 1
        else if (char === '>' && bridge[cursor - 1] !== '=') {
          depth -= 1
          if (depth === 0) { cursor += 1; break }
        }
      }
    }
    const call = /^\(\s*['`]([a-z0-9_]+)['`]/.exec(bridge.slice(cursor, cursor + 200))
    if (call) operations.add(call[1])
  }
  assert.ok(operations.size > 200, 'bridge.ts request surface looks unexpected')
  return operations
}

// Нативные операции движка обслуживаются без Python-воркера — но allowlist
// проходят точно так же. Список ведётся руками, поэтому сверяем его с mod.rs.
function nativeOperations() {
  const rust = readFileSync(new URL('../src-tauri/src/rust_engine/mod.rs', import.meta.url), 'utf8')
  const start = rust.indexOf('pub const NATIVE_OPS')
  assert.ok(start >= 0, 'rust_engine/mod.rs must declare NATIVE_OPS')
  const end = rust.indexOf('];', start)
  return new Set([...rust.slice(start, end).matchAll(/"([a-z0-9_]+)"/g)].map((match) => match[1]))
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

test('every rust_engine native operation passes the desktop bridge allow list', () => {
  const allowed = rustAllowList()
  const missing = [...nativeOperations()].filter((operation) => !allowed.has(operation)).sort()
  assert.deepEqual(missing, [], 'native engine operations rejected by the desktop bridge')
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
    'shader_analyze', 'shader_benchmark_start', 'shader_benchmark_status',
    'instance_versions', 'instance_version_import', 'instance_version_override_save',
    'login_ely',
  ]) {
    assert.ok(allowed.has(operation), operation)
  }
})
