import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'

const root = new URL('../src/', import.meta.url)
const [settings, pluginCenter, pluginCatalog, themeGallery, bridge, config] = await Promise.all([
  readFile(fileURLToPath(new URL('components/SettingsPage.vue', root)), 'utf8'),
  readFile(fileURLToPath(new URL('components/PluginCenter.vue', root)), 'utf8'),
  readFile(fileURLToPath(new URL('components/GitHubPluginCatalog.vue', root)), 'utf8'),
  readFile(fileURLToPath(new URL('components/ThemeGallery.vue', root)), 'utf8'),
  readFile(fileURLToPath(new URL('services/bridge.ts', root)), 'utf8'),
  readFile(fileURLToPath(new URL('../src-tauri/src/rust_engine/config.rs', root)), 'utf8'),
])

test('Nivora ecosystem integration is clearly labelled, enabled by default, and gates remote catalogs', () => {
  assert.match(settings, /Интеграция с экосистемой Nivora/)
  assert.match(settings, /Отключение выключает удалённые каталоги плагинов и тем/)
  assert.match(bridge, /ecosystem_enabled: true/)
  assert.match(config, /"ecosystem_enabled": true/)
  assert.match(pluginCenter, /v-if="props\.ecosystemEnabled"/)
  assert.match(themeGallery, /v-if="props\.ecosystemEnabled !== false"/)
})

test('market popularity is identified as GitHub stars, not user reviews', () => {
  assert.match(pluginCatalog, /звёзд GitHub, не отзывы пользователей/)
  assert.match(pluginCatalog, /plugin-rating/)
  assert.match(pluginCatalog, /aria-label="Маркет плагинов сообщества"/)
})
