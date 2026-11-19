import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'

const app = readFileSync(new URL('../src/App.vue', import.meta.url), 'utf8')
const mountedStart = app.indexOf('onMounted(async () => {')
const unmountedStart = app.indexOf('\nonBeforeUnmount(() => {', mountedStart)
const mounted = app.slice(mountedStart, unmountedStart)
const unmounted = app.slice(unmountedStart)

test('root lifecycle removes Tauri event listeners and the visibility handler', () => {
  assert.ok(mountedStart >= 0 && unmountedStart > mountedStart, 'root lifecycle boundaries are present')
  assert.match(app, /function handleVisibilityChange\(\)\s*\{\s*syncLauncherHidden\(\)\s*\}/)
  assert.match(mounted, /document\.addEventListener\('visibilitychange', handleVisibilityChange\)/)
  assert.doesNotMatch(mounted, /document\.addEventListener\('visibilitychange',\s*\(\)/)
  assert.match(mounted, /else unlistenHiddenToTray = unlisten/)
  assert.match(mounted, /else unlistenLaunchRequest = unlisten/)
  assert.match(unmounted, /document\.removeEventListener\('visibilitychange', handleVisibilityChange\)/)
  assert.match(unmounted, /appUnmounted = true[\s\S]*?unlistenHiddenToTray\?\.\(\)[\s\S]*?unlistenLaunchRequest\?\.\(\)/)
})

test('late Tauri listener registrations are immediately released after unmount', () => {
  assert.match(mounted, /if \(appUnmounted\) return\s+void loadSettingsProfiles\(\)/)
  assert.match(mounted, /if \(!appUnmounted\)\s*\{\s*const unlisten = await listen\('nivora-hidden-to-tray'[\s\S]*?if \(appUnmounted\) unlisten\(\)/)
  assert.match(mounted, /if \(!appUnmounted\)\s*\{\s*const unlisten = await listen<\{ name: string \| null \}>\('nivora-launch-request'[\s\S]*?if \(appUnmounted\) unlisten\(\)/)
})
