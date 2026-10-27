import assert from 'node:assert/strict'
import test from 'node:test'
import { normalizeHotkey, overlayWindowRequested, recordingFileName } from '../src/services/overlayHelpers.ts'

test('normalizeHotkey keeps printable combinations and rejects anything suspicious', () => {
  assert.equal(normalizeHotkey('Shift+Tab', 'Shift+Tab'), 'Shift+Tab')
  assert.equal(normalizeHotkey('F7', 'F7'), 'F7')
  assert.equal(normalizeHotkey('Ctrl+Shift+R', 'Ctrl+R'), 'Ctrl+Shift+R')
  assert.equal(normalizeHotkey('', 'F7'), 'F7')
  assert.equal(normalizeHotkey(undefined, 'F7'), 'F7')
  assert.equal(normalizeHotkey('Ctrl+X; rm -rf', 'Shift+Tab'), 'Shift+Tab')
  assert.equal(normalizeHotkey('Shift Tab', 'Shift+Tab'), 'Shift+Tab')
})

test('normalizeHotkey canonicalizes synonyms and rejects bare modifiers', () => {
  assert.equal(normalizeHotkey('Control+Alt+T', 'F7'), 'Ctrl+Alt+T')
  assert.equal(normalizeHotkey('ctrl+alt+t', 'F7'), 'Ctrl+Alt+T')
  assert.equal(normalizeHotkey('CommandOrControl+K', 'F7'), 'Meta+K')
  assert.equal(normalizeHotkey('option+L', 'F7'), 'Alt+L')
  assert.equal(normalizeHotkey('Ctrl+Shift', 'F7'), 'F7')
  assert.equal(normalizeHotkey('мама+мыла', 'F7'), 'F7')
  assert.equal(normalizeHotkey('Ctrl+F13', 'F7'), 'Ctrl+F13')
  assert.equal(normalizeHotkey('Ctrl+F25', 'F7'), 'F7')
  // "Ctrl+," должно оставаться допустимым сочетанием
  assert.equal(normalizeHotkey('Ctrl+,', 'F7'), 'Ctrl+,')
  assert.equal(normalizeHotkey('ctrl+comma', 'F7'), 'Ctrl+,')
  assert.equal(normalizeHotkey('Ctrl+.', 'F7'), 'Ctrl+.')
  assert.equal(normalizeHotkey('Shift+F24', 'F7'), 'Shift+F24')
  assert.equal(normalizeHotkey('ArrowUp', 'F7'), 'Up')
})

test('hotkeyFromEvent assembles combinations from live key presses', async () => {
  const { hotkeyFromEvent, hotkeyModifiersFromEvent } = await import('../src/services/overlayHelpers.ts')
  assert.equal(hotkeyFromEvent({ ctrlKey: true, shiftKey: false, altKey: false, metaKey: false, key: 'Tab' }), 'Ctrl+Tab')
  assert.equal(hotkeyFromEvent({ ctrlKey: false, shiftKey: true, altKey: true, metaKey: false, key: 'r' }), 'Alt+Shift+R')
  assert.equal(hotkeyFromEvent({ key: 'F9' }), 'F9')
  assert.equal(hotkeyFromEvent({ key: ' ' }), 'Space')
  assert.equal(hotkeyFromEvent({ key: 'ArrowLeft' }), 'Left')
  assert.equal(hotkeyFromEvent({ ctrlKey: true, key: 'Shift' }), '')
  assert.deepEqual(hotkeyModifiersFromEvent({ ctrlKey: true, shiftKey: true, key: 'Shift' }), ['Ctrl', 'Shift'])
  assert.equal(hotkeyFromEvent({ ctrlKey: true, key: 'ф' }), '')
  assert.equal(hotkeyFromEvent({ key: 'Delete' }), 'Delete')
  assert.equal(hotkeyFromEvent({ ctrlKey: true, key: ',' }), 'Ctrl+,')
  assert.equal(hotkeyFromEvent({ key: 'F7' }), 'F7')
})

test('recordingFileName reports the last path segment on every platform style', () => {
  assert.equal(recordingFileName('/home/user/game/recordings/2026-10-11 demo.mp4'), '2026-10-11 demo.mp4')
  assert.equal(recordingFileName('C:\\Games\\recordings\\clip.mkv'), 'clip.mkv')
  assert.equal(recordingFileName('clip.mp4'), 'clip.mp4')
  assert.equal(recordingFileName(''), '')
})

test('hidden overlay requests do not create a second window', async () => {
  const { overlayWindowAction } = await import('../src/services/overlayHelpers.ts')
  assert.equal(overlayWindowAction(false, false), 'noop')
  assert.equal(overlayWindowAction(false, true), 'hide')
  assert.equal(overlayWindowAction(true, false), 'create')
  assert.equal(overlayWindowAction(true, true), 'show')
})

test('overlayWindowRequested detects the overlay query parameter', () => {
  globalThis.window = { location: { search: '?window=overlay' } }
  assert.equal(overlayWindowRequested(), true)
  globalThis.window = { location: { search: '' } }
  assert.equal(overlayWindowRequested(), false)
  globalThis.window = { location: { search: '?window=main' } }
  assert.equal(overlayWindowRequested(), false)
  delete globalThis.window
})

test('overlayWindowRequested falls back to the Tauri window label', () => {
  // метка Tauri остаётся доступна, если параметр URL потерялся при редиректе
  globalThis.window = {
    location: { search: '' },
    __TAURI_INTERNALS__: { metadata: { currentWindow: { label: 'nivora-overlay' } } },
  }
  assert.equal(overlayWindowRequested(), true)
  globalThis.window = {
    location: { search: '' },
    __TAURI_INTERNALS__: { metadata: { currentWindow: { label: 'main' } } },
  }
  assert.equal(overlayWindowRequested(), false)
  globalThis.window = { location: { search: '?window=overlay' }, __TAURI_INTERNALS__: { metadata: {} } }
  assert.equal(overlayWindowRequested(), true)
  delete globalThis.window
})

test('overlayWindowPlan: прозрачный полноэкранный план для X11/XWayland', async () => {
  const { overlayWindowPlan } = await import('../src/services/overlayHelpers.ts')
  const plan = overlayWindowPlan('x11', { width: 1920, height: 1080, x: 0, y: 0 })
  assert.equal(plan.transparent, true)
  assert.equal(plan.decorations, false)
  assert.equal(plan.alwaysOnTop, true)
  assert.equal(plan.skipTaskbar, true)
  assert.equal(plan.center, false)
  assert.equal(plan.width, 1920)
  assert.equal(plan.height, 1080)
  assert.equal(plan.x, 0)
  assert.equal(plan.url, 'index.html?window=overlay')
})

test('overlayWindowPlan: совместимый план для Wayland и аварийного режима', async () => {
  const { overlayWindowPlan } = await import('../src/services/overlayHelpers.ts')
  for (const mode of ['wayland', 'fallback']) {
    const plan = overlayWindowPlan(mode, { width: 1920, height: 1080, x: 10, y: 10 })
    assert.equal(plan.transparent, false, mode)
    assert.equal(plan.decorations, true, mode)
    assert.equal(plan.alwaysOnTop, false, mode)
    assert.equal(plan.center, true, mode)
    assert.equal(plan.url, 'index.html?window=overlay&opaque=1', mode)
    assert.equal(plan.x, undefined, mode)
    assert.equal(plan.width, 980, mode)
    assert.equal(plan.height, 640, mode)
  }
  const tiny = overlayWindowPlan('wayland', { width: 100, height: 100, x: 0, y: 0 })
  assert.equal(tiny.width, 640)
  assert.equal(tiny.height, 480)
  const tinyX11 = overlayWindowPlan('x11', { width: 100, height: 100, x: 0, y: 0 })
  assert.equal(tinyX11.width, 640)
  assert.equal(tinyX11.height, 480)
})
