import assert from 'node:assert/strict'
import test from 'node:test'
import { skinMapForLayer } from '../frontend/src/skinUv.ts'

test('skin preview uses base UV for the base pass and overlay UV for the overlay pass', () => {
  const baseMap = { color: 'base' }
  const overlayMap = { color: 'overlay' }

  assert.equal(skinMapForLayer(baseMap, overlayMap, false), baseMap)
  assert.equal(skinMapForLayer(baseMap, overlayMap, true), overlayMap)
  assert.equal(skinMapForLayer(baseMap, undefined, true), baseMap)
})
