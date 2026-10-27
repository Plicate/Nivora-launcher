import test from 'node:test'
import assert from 'node:assert/strict'
import { shouldOpenActivityAfterSessionStart } from '../src/services/activityVisibility.ts'

test('session-start only opens Activity when enabled and not dismissed for this run', () => {
  assert.equal(shouldOpenActivityAfterSessionStart(true, false), true)
  assert.equal(shouldOpenActivityAfterSessionStart(true, true), false)
  assert.equal(shouldOpenActivityAfterSessionStart(false, false), false)
})
