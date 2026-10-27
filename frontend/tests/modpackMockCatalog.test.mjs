import test from 'node:test'
import assert from 'node:assert/strict'
import { demoModpacksForCatalog, searchDemoModpacks } from '../src/services/modpackMockCatalog.ts'

test('modpack preview results stay tied to the selected provider', () => {
  const ftb = demoModpacksForCatalog({ id: 'ftb', name: 'FTB', kind: 'ftb', api_url: '' })
  const technic = demoModpacksForCatalog({ id: 'technic', name: 'Technic', kind: 'technic', api_url: '' })

  assert.equal(ftb?.[0].source, 'ftb')
  assert.equal(ftb?.[0].catalog_id, 'ftb')
  assert.equal(technic?.[0].source, 'technic')
  assert.equal(technic?.[0].catalog_id, 'technic')
  assert.notEqual(ftb?.[0].project_id, technic?.[0].project_id)
})

test('unsupported providers do not silently become Modrinth', () => {
  assert.equal(demoModpacksForCatalog({ id: 'custom', name: 'Custom', kind: 'custom', api_url: '' }), null)
})

test('demo search filters and paginates selected-provider results', () => {
  const results = demoModpacksForCatalog({ id: 'ftb', name: 'FTB', kind: 'ftb', api_url: '' }) || []
  const filtered = searchDemoModpacks(results, 'stone', 0, 1)
  assert.equal(filtered.total, 1)
  assert.equal(filtered.results[0].project_id, 'preview-ftb-stone')
  assert.equal(filtered.has_more, false)
  assert.equal(searchDemoModpacks(results, '', 0, 1).has_more, true)
})
