import test from 'node:test'
import assert from 'node:assert/strict'
import {
  applyPluginThemeOperation,
  pluginThemeEffects,
  pluginThemeEffectsSnapshot,
} from '../src/services/pluginThemeEffects.ts'

const PLUGIN = 'test-plugin'

function clear() {
  pluginThemeEffects.clearPlugin(PLUGIN)
}

test('particle presets spawn with defaults and stop operations work', () => {
  clear()
  const snow = applyPluginThemeOperation('theme.particles.spawn', PLUGIN, { spec: { preset: 'snow' } })
  assert.ok(snow.id.startsWith('emitter-'))
  const custom = applyPluginThemeOperation('theme.particles.spawn', PLUGIN, {
    spec: { count: 900, size: 500, speed: -50, color: '#10b981', shape: 'star', zone: 'top', layer: 'above' },
  })
  assert.throws(() => applyPluginThemeOperation('theme.particles.spawn', PLUGIN, { spec: { color: 'not-a-color' } }), TypeError)
  const snapshot = pluginThemeEffectsSnapshot()
  const customEmitter = snapshot.emitters.find((emitter) => emitter.id === custom.id)
  assert.equal(customEmitter.count, 400)
  assert.equal(customEmitter.size, 64)
  assert.equal(customEmitter.shape, 'star')
  assert.equal(customEmitter.zone, 'top')
  assert.equal(customEmitter.layer, 'above')
  assert.throws(() => applyPluginThemeOperation('theme.particles.spawn', PLUGIN, { spec: { preset: 'blizzard' } }), TypeError)
  assert.throws(() => applyPluginThemeOperation('theme.particles.spawn', PLUGIN, { spec: { image: 'https://example.com/x.png' } }), TypeError)
  assert.equal(applyPluginThemeOperation('theme.particles.stop', PLUGIN, { id: custom.id }).stopped, true)
  assert.equal(applyPluginThemeOperation('theme.particles.stop', PLUGIN, { id: custom.id }).stopped, false)
  const active = applyPluginThemeOperation('theme.particles.active', PLUGIN, {})
  assert.deepEqual(active.ids, [snow.id])
  const burst = applyPluginThemeOperation('theme.particles.burst', PLUGIN, { spec: { x: 0.5, y: 0.5 } })
  assert.ok(burst.id.startsWith('burst-'))
  assert.equal(pluginThemeEffectsSnapshot().bursts.length, 1)
  assert.throws(() => applyPluginThemeOperation('theme.particles.burst', PLUGIN, { spec: { x: 5, y: 0.5 } }), TypeError)
  pluginThemeEffects.particlesRemoveBurst(burst.id)
  assert.equal(applyPluginThemeOperation('theme.particles.stopAll', PLUGIN, {}).removed, 1)
  clear()
})

test('style overrides validate properties and reset by scope', () => {
  clear()
  applyPluginThemeOperation('theme.style.set', PLUGIN, { scope: 'sidebar', patch: { 'background-color': '#101828', 'border-radius': '12px' } })
  let snapshot = pluginThemeEffectsSnapshot()
  assert.equal(snapshot.styles.length, 1)
  assert.deepEqual(snapshot.styles[0].css, { 'background-color': '#101828', 'border-radius': '12px' })
  applyPluginThemeOperation('theme.style.set', PLUGIN, { scope: 'sidebar', patch: { 'background-color': '#202838' } })
  snapshot = pluginThemeEffectsSnapshot()
  assert.equal(snapshot.styles.length, 1)
  assert.equal(snapshot.styles[0].css['background-color'], '#202838')
  assert.throws(() => applyPluginThemeOperation('theme.style.set', PLUGIN, { scope: 'sidebar', patch: { position: 'absolute' } }), TypeError)
  assert.throws(() => applyPluginThemeOperation('theme.style.set', PLUGIN, { scope: 'sidebar', patch: { color: 'url(https://x)' } }), TypeError)
  assert.throws(() => applyPluginThemeOperation('theme.style.set', PLUGIN, { scope: 'unknown-scope', patch: { color: '#101828' } }), TypeError)
  applyPluginThemeOperation('theme.style.reset', PLUGIN, { scope: 'sidebar' })
  assert.equal(pluginThemeEffectsSnapshot().styles.length, 0)
  clear()
})

test('slot replacement accepts whitelisted slots and view trees only', () => {
  clear()
  applyPluginThemeOperation('theme.slots.set', PLUGIN, { slot: 'sidebar.footer', view: { type: 'text', id: 'a', text: 'Привет' } })
  assert.equal(pluginThemeEffectsSnapshot().slots.length, 1)
  assert.throws(() => applyPluginThemeOperation('theme.slots.set', PLUGIN, { slot: 'topbar.brand', view: { type: 'text', text: 'x' } }), TypeError)
  assert.throws(() => applyPluginThemeOperation('theme.slots.set', PLUGIN, { slot: 'sidebar.footer', view: { type: 'unknown-node!' } }), TypeError)
  assert.throws(() => applyPluginThemeOperation('theme.slots.set', PLUGIN, { slot: 'sidebar.footer', view: 'text' }), TypeError)
  applyPluginThemeOperation('theme.slots.set', PLUGIN, { slot: 'instances.top', view: { type: 'text', text: 'Верх' } })
  assert.equal(pluginThemeEffectsSnapshot().slots.length, 2)
  applyPluginThemeOperation('theme.slots.reset', PLUGIN, { slot: 'sidebar.footer' })
  assert.equal(pluginThemeEffectsSnapshot().slots.length, 1)
  clear()
})

test('sounds register, list, and unregister with data URL validation', () => {
  clear()
  const audio = 'data:audio/mpeg;base64,AAAA'
  applyPluginThemeOperation('theme.sounds.register', PLUGIN, { name: 'click', audio })
  assert.deepEqual(applyPluginThemeOperation('theme.sounds.list', PLUGIN, {}).names, ['click'])
  assert.throws(() => applyPluginThemeOperation('theme.sounds.register', PLUGIN, { name: 'bad name', audio }), TypeError)
  assert.throws(() => applyPluginThemeOperation('theme.sounds.register', PLUGIN, { name: 'ok', audio: 'data:text/html;base64,AAAA' }), TypeError)
  applyPluginThemeOperation('theme.sounds.play', PLUGIN, { name: 'click', options: { volume: 2, rate: 9 } })
  assert.equal(applyPluginThemeOperation('theme.sounds.unregister', PLUGIN, { name: 'click' }).removed, true)
  assert.deepEqual(applyPluginThemeOperation('theme.sounds.list', PLUGIN, {}).names, [])
  clear()
})

test('cursor, wallpaper, and effect specs are validated and cleared per plugin', () => {
  clear()
  const png = 'data:image/png;base64,AAAA'
  applyPluginThemeOperation('theme.cursor.set', PLUGIN, { spec: { image: png, size: 64 } })
  assert.equal(pluginThemeEffectsSnapshot().cursor.size, 64)
  assert.throws(() => applyPluginThemeOperation('theme.cursor.set', PLUGIN, { spec: { image: 'data:application/octet-stream;base64,AAAA' } }), TypeError)
  applyPluginThemeOperation('theme.wallpaper.set', PLUGIN, { spec: { image: png, blur: 90, dim: 5 } })
  const wallpaper = pluginThemeEffectsSnapshot().wallpapers[0]
  assert.equal(wallpaper.blur, 40)
  assert.equal(wallpaper.dim, 0.9)
  applyPluginThemeOperation('theme.effects.set', PLUGIN, { spec: { panelGlow: '#3d8bfd', vignette: 4, scanlines: true } })
  const effects = pluginThemeEffectsSnapshot().effects[0].spec
  assert.equal(effects.vignette, 1)
  assert.equal(effects.scanlines, true)
  assert.throws(() => applyPluginThemeOperation('theme.effects.set', PLUGIN, { spec: {} }), TypeError)
  clear()
  const cleared = pluginThemeEffectsSnapshot()
  assert.equal(cleared.cursor, null)
  assert.equal(cleared.wallpapers.length, 0)
  assert.equal(cleared.effects.length, 0)
})

test('hud widgets, transitions, fonts, and tokens are scoped per plugin', () => {
  clear()
  const widget = applyPluginThemeOperation('theme.hud.add', PLUGIN, { view: { type: 'text', text: 'Пинг' }, options: { position: 'top-center', width: 9999 } })
  assert.ok(widget.id.startsWith('hud-'))
  assert.equal(pluginThemeEffectsSnapshot().hud[0].width, 480)
  assert.equal(applyPluginThemeOperation('theme.hud.remove', PLUGIN, { id: widget.id }).removed, true)
  applyPluginThemeOperation('theme.transitions.set', PLUGIN, { preset: 'cinematic' })
  assert.equal(pluginThemeEffectsSnapshot().transitions.preset, 'cinematic')
  assert.throws(() => applyPluginThemeOperation('theme.transitions.set', PLUGIN, { preset: 'warp' }), TypeError)
  const font = applyPluginThemeOperation('theme.fonts.register', PLUGIN, { name: 'Display', font: 'data:font/woff2;base64,AAAA' })
  assert.match(font.family, /^plugin-test-plugin-display$/)
  assert.deepEqual(applyPluginThemeOperation('theme.fonts.list', PLUGIN, {}).fonts, [{ name: 'Display', family: font.family }])
  applyPluginThemeOperation('theme.tokens.set', PLUGIN, { overrides: { 'panel.radius': 14, accent: '#ff8800' } })
  const tokens = pluginThemeEffectsSnapshot().tokens[0].vars
  assert.equal(tokens['panel.radius'], 14)
  assert.equal(tokens.accent, '#ff8800')
  assert.throws(() => applyPluginThemeOperation('theme.tokens.set', PLUGIN, { overrides: { __proto__: 1 } }), TypeError)
  clear()
  const snapshot = pluginThemeEffectsSnapshot()
  assert.equal(snapshot.hud.length, 0)
  assert.equal(snapshot.transitions, null)
  assert.equal(snapshot.fonts.length, 0)
  assert.equal(snapshot.tokens.length, 0)
})

test('per-plugin caps are enforced', () => {
  clear()
  for (let index = 0; index < 8; index += 1) {
    applyPluginThemeOperation('theme.particles.spawn', PLUGIN, { spec: {} })
  }
  assert.throws(() => applyPluginThemeOperation('theme.particles.spawn', PLUGIN, { spec: {} }), /8 active emitters/)
  clear()
  for (let index = 0; index < 8; index += 1) {
    applyPluginThemeOperation('theme.hud.add', PLUGIN, { view: { type: 'text', text: 'w' } })
  }
  assert.throws(() => applyPluginThemeOperation('theme.hud.add', PLUGIN, { view: { type: 'text', text: 'w' } }), /8 HUD widgets/)
  clear()
})

test('unknown theme operations are rejected', () => {
  assert.throws(() => applyPluginThemeOperation('theme.warp.drive', PLUGIN, {}), /Unsupported theme operation/)
})
