import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'

const launcher = readFileSync(new URL('../src-tauri/src/rust_engine/launcher.rs', import.meta.url), 'utf8')
const workspace = readFileSync(new URL('../src/components/InstanceWorkspace.vue', import.meta.url), 'utf8')

test('Java session lifecycle events reset stale logs and stop the matching instance', () => {
  assert.match(launcher, /emit_instance_status_event\("session-started",\s*"",\s*&session_name\)/)
  assert.equal((launcher.match(/emit_instance_status_event\("operation-finished"/g) || []).length, 2)
  assert.match(workspace, /if \(event\.instance_name !== props\.instance\?\.name\) return/)
  assert.match(workspace, /event\.event === 'session-started'[\s\S]*?liveLogRevision \+= 1[\s\S]*?logs\.value = \[\]/)
  assert.match(workspace, /event\.event === 'operation-finished'[\s\S]*?running\.value = false[\s\S]*?loading\.value = false/)
})

test('profile edits use the shared settings save action', () => {
  assert.doesNotMatch(workspace, /Сохранить профиль/)
  assert.match(workspace, /async function save\(\)[\s\S]*?persistProfileDraft\(\)/)
  assert.match(workspace, /settings-save-button[\s\S]*?@click="save"/)
})
