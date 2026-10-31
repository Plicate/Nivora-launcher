import test from 'node:test'
import assert from 'node:assert/strict'
import {
  DEFAULT_BUTTON_CUSTOMIZATION,
  cropBackgroundPosition,
  cropBackgroundSize,
  normalizeButtonCustomization,
  safeDataUrl,
  truncateButtonLabel,
} from '../src/services/buttonCustomization.ts'

test('button settings clamp size, typography, crop, and text to supported limits', () => {
  const settings = normalizeButtonCustomization({
    width: 999,
    height: 1,
    font_size: 30,
    crop: { x: -1, y: 102, width: 1, height: 200 },
    labels: { instances: 'Играть с друзьями и вместе', accounts: '😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀' },
  })
  assert.equal(settings.width, 280)
  assert.equal(settings.height, 28)
  assert.equal(settings.font_size, 20)
  assert.deepEqual(settings.crop, { x: 0, y: 0, width: 5, height: 100 })
  assert.equal(Array.from(settings.labels.instances).length, 16)
  assert.equal(Array.from(settings.labels.accounts).length, 16)
})

test('button defaults and invalid values stay safe', () => {
  const settings = normalizeButtonCustomization({ fill: false, font_family: 'url()' })
  assert.equal(settings.fill, false)
  assert.equal(settings.border, true)
  assert.equal(settings.font_family, 'system')
  assert.equal(settings.texture_fit, 'cover')
  assert.equal(settings.width, DEFAULT_BUTTON_CUSTOMIZATION.width)
})

test('custom truncation uses an ellipsis and preserves short labels', () => {
  assert.equal(truncateButtonLabel('Главная'), 'Главная')
  assert.equal(truncateButtonLabel('😀'.repeat(16), 8), `${'😀'.repeat(7)}…`)
})

test('asset data URLs allow only bounded raster images and audio formats', () => {
  assert.equal(safeDataUrl('data:image/png;base64,aGVsbG8=', 'image'), 'data:image/png;base64,aGVsbG8=')
  assert.equal(safeDataUrl('data:image/svg+xml;base64,PHN2Zz4=', 'image'), '')
  assert.equal(safeDataUrl('data:audio/ogg;base64,b2dn', 'audio'), 'data:audio/ogg;base64,b2dn')
  assert.equal(safeDataUrl('data:audio/html;base64,PHNjcmlwdD4=', 'audio'), '')
  assert.equal(safeDataUrl(`data:image/png;base64,${'a'.repeat(800_000)}`, 'image'), '')
})

test('texture crop expands the image and supports a custom position', () => {
  assert.equal(cropBackgroundSize({ x: 50, y: 50, width: 100, height: 100 }, 'cover'), 'cover')
  assert.equal(cropBackgroundSize({ x: 10, y: 25, width: 50, height: 25 }, 'cover'), '200% 400%')
  assert.equal(cropBackgroundPosition({ x: 10, y: 25, width: 50, height: 25 }), '20% 33.33%')
  assert.equal(cropBackgroundPosition({ x: 90, y: 99, width: 50, height: 25 }), `${100}% ${100}%`)
  assert.equal(cropBackgroundSize({ x: 0, y: 0, width: 5, height: 5 }, 'contain'), '2000% 2000%')
})
