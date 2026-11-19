import test from 'node:test'
import assert from 'node:assert/strict'
import { chooseAllowedCatalog } from '../src/services/catalogSelection.ts'

test('restores a persisted FTB catalog after visiting the resource catalog', () => {
  const modpacks = ['modrinth', 'ftb', 'technic']
  const resources = ['modrinth', 'custom']
  const persisted = 'ftb'
  const resourceSelection = chooseAllowedCatalog(resources, persisted, 'ftb')
  assert.equal(resourceSelection, 'modrinth')
  assert.equal(persisted, 'ftb')
  assert.equal(chooseAllowedCatalog(modpacks, persisted, resourceSelection || ''), 'ftb')
})

test('prefers the saved catalog and safely falls back for unsupported views', () => {
  assert.equal(chooseAllowedCatalog(['modrinth', 'ftb', 'technic'], 'technic', 'modrinth'), 'technic')
  assert.equal(chooseAllowedCatalog(['modrinth'], 'missing', 'missing'), 'modrinth')
  assert.equal(chooseAllowedCatalog([], 'ftb', 'modrinth'), undefined)
})
