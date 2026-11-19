import assert from 'node:assert/strict'
import test from 'node:test'
import { modCatalogSearchContext } from '../src/services/modCatalogSearch.ts'

test('modpack search uses the new-instance selections instead of an existing profile', () => {
  assert.deepEqual(modCatalogSearchContext({
    view: 'modpacks',
    selectedInstanceName: 'Old profile',
    selectedInstanceVersion: '1.19.2',
    selectedInstanceLoader: 'forge',
    selectedVersion: '1.21.1',
    selectedLoader: 'fabric',
  }), {
    instance_name: '',
    game_version: '1.21.1',
    loader: 'fabric',
  })
})

test('regular mod search keeps the selected profile context', () => {
  assert.deepEqual(modCatalogSearchContext({
    view: 'mods',
    selectedInstanceName: 'Current profile',
    selectedInstanceVersion: '1.20.1',
    selectedInstanceLoader: 'quilt',
    selectedVersion: '1.21.1',
    selectedLoader: 'fabric',
  }), {
    instance_name: 'Current profile',
    game_version: '1.20.1',
    loader: 'quilt',
  })
})
