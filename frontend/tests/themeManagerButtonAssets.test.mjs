import test from 'node:test'
import assert from 'node:assert/strict'
import { deflateSync } from 'node:zlib'
import { createServer } from 'vite'

let bridge
let createThemeManager
let vite

test.before(async () => {
  vite = await createServer({
    configFile: false,
    root: new URL('..', import.meta.url).pathname,
    optimizeDeps: { noDiscovery: true, include: [] },
    server: { middlewareMode: true },
    appType: 'custom',
  })
  ;({ bridge } = await vite.ssrLoadModule('/src/services/bridge.ts'))
  ;({ createThemeManager } = await vite.ssrLoadModule('/src/services/themeManager.ts'))
})

test.after(async () => {
  await vite?.close()
})

const tokens = {
  accent: '#3d8bfd',
  accent_strong: '#276fe0',
  background: '#101216',
  panel: '#181a20',
  panel_soft: '#20232b',
}

function crc32(bytes) {
  let crc = 0xffffffff
  for (const byte of bytes) {
    crc ^= byte
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0)
  }
  return (crc ^ 0xffffffff) >>> 0
}

function pngChunk(type, data) {
  const body = Buffer.concat([Buffer.from(type), data])
  const length = Buffer.alloc(4)
  length.writeUInt32BE(data.length)
  const checksum = Buffer.alloc(4)
  checksum.writeUInt32BE(crc32(body))
  return Buffer.concat([length, body, checksum])
}

function tinyPngDataUrl() {
  const signature = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])
  const header = Buffer.alloc(13)
  header.writeUInt32BE(1, 0)
  header.writeUInt32BE(1, 4)
  header[8] = 8
  header[9] = 6
  const pixels = deflateSync(Buffer.from([0, 60, 126, 255, 255]))
  const image = Buffer.concat([signature, pngChunk('IHDR', header), pngChunk('IDAT', pixels), pngChunk('IEND', Buffer.alloc(0))])
  return `data:image/png;base64,${image.toString('base64')}`
}

function tinyWavDataUrl() {
  const samples = Buffer.from([0, 0])
  const header = Buffer.alloc(44)
  header.write('RIFF', 0)
  header.writeUInt32LE(36 + samples.length, 4)
  header.write('WAVEfmt ', 8)
  header.writeUInt32LE(16, 16)
  header.writeUInt16LE(1, 20)
  header.writeUInt16LE(1, 22)
  header.writeUInt32LE(8000, 24)
  header.writeUInt32LE(16000, 28)
  header.writeUInt16LE(2, 32)
  header.writeUInt16LE(16, 34)
  header.write('data', 36)
  header.writeUInt32LE(samples.length, 40)
  const audio = Buffer.concat([header, samples])
  return `data:audio/wav;base64,${audio.toString('base64')}`
}

function managerDependencies(appearanceDraft) {
  return {
    state: {},
    appearanceDraft,
    settingsDraft: { font_family: 'system', language: 'ru' },
    localThemes: { value: [] },
    themeOperationBusy: { value: false },
    githubThemeInstallBusy: { value: false },
    activeThemeId: { value: '' },
    availableThemes: { value: [] },
    defaultColorTokens: tokens,
    lightColorTokens: tokens,
    defaultRadiusTokens: { card: 7, button: 8, control: 10 },
    defaultAnimationSettings: {},
    busy: { value: true },
    wallpaperPreviewUrl: { value: '' },
    wallpaperRenderBlur: { value: 0 },
    showToast() {},
    errorMessage: String,
    async saveSettings() { return true },
    releaseWallpaperPreview() {},
    setLanguageFromSettings() {},
  }
}

test('theme save and apply retain per-button labels and resource data', async () => {
  const originalThemeSave = bridge.themeSave
  let savedTheme
  let savedResourceData
  bridge.themeSave = async (theme, metadata, resourceData) => {
    savedTheme = { ...theme, ...metadata, source: 'local', version: '1.0.0', creators: [], source_url: '', resource_data: resourceData }
    savedResourceData = resourceData
    return { theme: savedTheme, themes: [savedTheme] }
  }

  try {
    const appearanceDraft = {
      color_tokens: { ...tokens },
      theme: 'dark',
      accent_color: tokens.accent,
      density: 'comfortable',
      sidebar_mode: 'expanded',
      button_customization: {
        width: 152,
        height: 38,
        fill: true,
        border: true,
        font_family: 'system',
        font_size: 12,
        outline: { enabled: false, width: 0, color: '#000000' },
        texture_fit: 'cover',
        crop_enabled: false,
        crop: { x: 0, y: 0, width: 100, height: 100 },
        texture_data: '',
        icon_data: '',
        sound_data: '',
        labels: { instances: '', accounts: '', mods: '', modpacks: '', themes: '', settings: '' },
        overrides: {
          'hero-play:preview-survival': { label: 'Запустить', width: 208, texture_data: tinyPngDataUrl() },
          'card-button:preview-survival%3Asettings': { sound_data: tinyWavDataUrl() },
          'nav-item:servers': { texture_data: tinyPngDataUrl() },
        },
      },
    }
    const manager = createThemeManager(managerDependencies(appearanceDraft))
    await manager.saveCurrentTheme({ name: 'Test theme', author: 'Test', description: 'Test assets' })

    assert.match(savedTheme.resources['hero-play:preview-survival-texture'], /^resources\/button-[a-f0-9]{16}\.png$/)
    assert.match(savedTheme.resources['card-button:preview-survival%3Asettings-sound'], /^resources\/button-[a-f0-9]{16}\.wav$/)
    assert.ok(savedResourceData['hero-play:preview-survival-texture'])
    assert.ok(savedResourceData['card-button:preview-survival%3Asettings-sound'])
    assert.ok(savedResourceData['nav-item:servers-texture'])
    assert.equal(savedTheme.button_customization.overrides['card-button:preview-survival%3Asettings'].sound, savedTheme.resources['card-button:preview-survival%3Asettings-sound'])
    assert.equal(savedTheme.button_customization.overrides['nav-item:servers'].texture, savedTheme.resources['nav-item:servers-texture'])
    assert.equal(savedTheme.button_customization.overrides['hero-play:preview-survival'].label, 'Запустить')
    assert.equal(savedTheme.button_customization.overrides['hero-play:preview-survival'].width, 208)

    manager.applyTheme(savedTheme)
    assert.equal(appearanceDraft.button_customization.overrides['hero-play:preview-survival'].texture_data, tinyPngDataUrl())
    assert.equal(appearanceDraft.button_customization.overrides['card-button:preview-survival%3Asettings'].sound_data, tinyWavDataUrl())
    assert.equal(appearanceDraft.button_customization.overrides['nav-item:servers'].texture_data, tinyPngDataUrl())
    assert.equal(appearanceDraft.button_customization.overrides['hero-play:preview-survival'].label, 'Запустить')
  } finally {
    bridge.themeSave = originalThemeSave
  }
})
