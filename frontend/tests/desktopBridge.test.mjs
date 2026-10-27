import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { bridgeAllowList } from './helpers/registry.mjs'

function rustAllowList() {
  return bridgeAllowList()
}

function frontendOperations() {
  const bridge = readFileSync(new URL('../src/services/bridge.ts', import.meta.url), 'utf8')
  const operations = new Set()
  // вложенные типы требуют разбирать баланс угловых скобок
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

function nativeOperations() {
  const rust = readFileSync(new URL('../src-tauri/src/rust_engine/mod.rs', import.meta.url), 'utf8')
  const start = rust.indexOf('pub const NATIVE_OPS')
  assert.ok(start >= 0, 'rust_engine/mod.rs must declare NATIVE_OPS')
  const end = rust.indexOf('];', start)
  return new Set([...rust.slice(start, end).matchAll(/"([a-z0-9_]+)"/g)].map((match) => match[1]))
}

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
    'instance_autotune',
    'auto_repair_diagnose', 'auto_repair_mute',
    'instance_snapshot_create', 'instance_snapshot_delete', 'instance_snapshot_list',
    'instance_snapshot_restore', 'instance_compare', 'instance_port_changes',
    'instance_pins_get', 'instance_pin_toggle', 'open_external', 'open_folder',
    'instance_log', 'system_notify',
    'mod_audit_prepare', 'mod_audit_accept', 'mod_audit_cancel', 'mod_audit_status',
    'server_file_list', 'server_file_read', 'server_file_write', 'server_file_mkdir',
    'server_file_upload', 'server_file_delete', 'server_details', 'server_save_config',
    'server_snapshot_list', 'server_snapshot_create', 'server_snapshot_delete',
    'server_snapshot_restore',
    'world_import_begin', 'world_import_chunk', 'world_import_cancel', 'world_import_finish',
    'wallpaper_import', 'wallpaper_clear', 'instance_icon_blocks',
    'graphics_diagnostics', 'update_instance_profile',
    'external_instance_import',
    'instance_versions', 'instance_version_override_save', 'instance_version_import',
    'mods_remove',
    'instance_repair_version', 'auto_repair_apply',
    'plugin_dev_init', 'plugin_dev_lint', 'plugin_dev_pack', 'plugin_dev_verify', 'plugin_dev_keypair',
  ]) {
    assert.ok(allowed.has(operation), operation)
  }
})
