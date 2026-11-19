import assert from 'node:assert/strict'
import test from 'node:test'
import { boxRegions, regionAt, skinRegions } from '../src/services/skinRegions.ts'

test('head faces follow the Minecraft skin layout', () => {
  const head = boxRegions({ u: 0, v: 0, w: 8, h: 8, d: 8 })
  assert.deepEqual(head.top, { x: 8, y: 0, width: 8, height: 8 })
  assert.deepEqual(head.front, { x: 8, y: 8, width: 8, height: 8 })
  assert.deepEqual(head.back, { x: 24, y: 8, width: 8, height: 8 })
})

test('classic texture has 72 regions that never overlap and stay inside 64x64', () => {
  const regions = skinRegions('classic')
  assert.equal(regions.length, 6 * 6 * 2)
  const owner = new Map()
  for (const region of regions) {
    assert.ok(region.x >= 0 && region.y >= 0 && region.x + region.width <= 64 && region.y + region.height <= 64, region.id)
    for (let y = region.y; y < region.y + region.height; y++) {
      for (let x = region.x; x < region.x + region.width; x++) {
        const key = y * 64 + x
        assert.ok(!owner.has(key), `${region.id} overlaps ${owner.get(key)}`)
        owner.set(key, region.id)
      }
    }
  }
})

test('slim arms are three pixels wide', () => {
  const regions = skinRegions('slim')
  const front = regions.find((region) => region.id === 'rightArm.base.front')
  const left = regions.find((region) => region.id === 'rightArm.base.left')
  assert.deepEqual([front.x, front.width], [44, 3])
  assert.equal(left.x, 47)
})

test('regionAt resolves the side under a pixel', () => {
  const regions = skinRegions('classic')
  assert.equal(regionAt(regions, 10, 10)?.id, 'head.base.front')
  assert.equal(regionAt(regions, 42, 10)?.id, 'head.overlay.front')
  assert.equal(regionAt(regions, 22, 52)?.id, 'leftLeg.base.front')
  assert.equal(regionAt(regions, 0, 0), null)
})

test('legacy textures expose only regions that exist in 64x32', () => {
  const regions = skinRegions('classic', true)
  assert.ok(regions.every((region) => region.y + region.height <= 32))
  assert.ok(regions.some((region) => region.id === 'head.overlay.front'))
  assert.ok(!regions.some((region) => region.part === 'leftArm' || region.part === 'leftLeg'))
  assert.ok(!regions.some((region) => region.id === 'body.overlay.front'))
})
