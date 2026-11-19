import assert from 'node:assert/strict'
import test from 'node:test'
import { FIRST_RUN_EXPERIENCE_KEY, FIRST_RUN_SETUP_STAGES, initialFirstRunPhase, readFirstRunExperience, writeFirstRunExperience } from '../src/services/firstRunExperience.ts'

class MemoryStorage {
  values = new Map()
  getItem(key) { return this.values.get(key) ?? null }
  setItem(key, value) { this.values.set(key, value) }
}

test('fresh install shows setup and has not completed the tour', () => {
  assert.deepEqual(readFirstRunExperience(new MemoryStorage()), {
    setupCompleted: false,
    tourCompleted: false,
  })
})

test('the setup stages follow the requested order and resume the unfinished tour', () => {
  assert.deepEqual(FIRST_RUN_SETUP_STAGES.map((step) => step.phase), ['language', 'welcome', 'account', 'basics', 'finish'])
  assert.deepEqual(FIRST_RUN_SETUP_STAGES.map((step) => step.label), ['Язык интерфейса', 'Приветствие', 'Добавляем аккаунт', 'Базовая настройка', 'Завершение'])
  assert.equal(initialFirstRunPhase({ setupCompleted: false, tourCompleted: false }), 'language')
  assert.equal(initialFirstRunPhase({ setupCompleted: true, tourCompleted: false }), 'tour')
  assert.equal(initialFirstRunPhase({ setupCompleted: true, tourCompleted: true }), null)
})

test('setup and tour completion persist independently', () => {
  const storage = new MemoryStorage()
  assert.equal(writeFirstRunExperience({ setupCompleted: true, tourCompleted: false }, storage), true)
  assert.deepEqual(readFirstRunExperience(storage), { setupCompleted: true, tourCompleted: false })

  assert.equal(writeFirstRunExperience({ setupCompleted: true, tourCompleted: true }, storage), true)
  assert.deepEqual(readFirstRunExperience(storage), { setupCompleted: true, tourCompleted: true })
  assert.equal(storage.getItem(FIRST_RUN_EXPERIENCE_KEY)?.includes('"version":1'), true)
})

test('unknown or corrupt saved values safely restart the first-run experience', () => {
  const storage = new MemoryStorage()
  storage.setItem(FIRST_RUN_EXPERIENCE_KEY, '{broken')
  assert.deepEqual(readFirstRunExperience(storage), { setupCompleted: false, tourCompleted: false })
  storage.setItem(FIRST_RUN_EXPERIENCE_KEY, JSON.stringify({ version: 2, setupCompleted: true, tourCompleted: true }))
  assert.deepEqual(readFirstRunExperience(storage), { setupCompleted: false, tourCompleted: false })
  storage.setItem(FIRST_RUN_EXPERIENCE_KEY, JSON.stringify({ version: 1, setupCompleted: true, tourCompleted: false }))
  assert.deepEqual(readFirstRunExperience(storage), { setupCompleted: true, tourCompleted: false })
})

test('storage access errors are contained instead of blocking launcher startup', () => {
  const storage = { getItem() { throw new Error('blocked') }, setItem() { throw new Error('blocked') } }
  assert.deepEqual(readFirstRunExperience(storage), { setupCompleted: false, tourCompleted: false })
  assert.equal(writeFirstRunExperience({ setupCompleted: true, tourCompleted: true }, storage), false)
})
