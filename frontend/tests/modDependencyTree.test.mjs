import assert from 'node:assert/strict'
import test from 'node:test'
import { layoutModDependencyTree } from '../src/services/modDependencyTree.ts'

const assertCurvedPath = (path) => {
  const commands = path.match(/[A-Za-z]/g) ?? []
  assert.equal(commands[0], 'M')
  assert.ok(commands.slice(1).length > 0)
  assert.ok(commands.slice(1).every((command) => command === 'C'))

  const values = (path.match(/-?\d+(?:\.\d+)?/g) ?? []).map(Number)
  let x = values[0]
  let y = values[1]
  for (let index = 2; index < values.length; index += 6) {
    const [control1X, control1Y, control2X, control2Y, endX, endY] = values.slice(index, index + 6)
    const dx = endX - x
    const dy = endY - y
    const cross1 = (control1X - x) * dy - (control1Y - y) * dx
    const cross2 = (control2X - x) * dy - (control2Y - y) * dx
    assert.ok(Math.abs(cross1) + Math.abs(cross2) > 0, `straight Bézier segment: ${path}`)
    x = endX
    y = endY
  }
}

const node = (id, name = id) => ({
  id,
  file: `${id}.jar`,
  display_name: name,
  version: '1.0',
  enabled: true,
  source: 'fabric',
  loader: 'fabric',
  dependencies: [],
})

test('shared dependencies use one uninterrupted curved edge per mod', () => {
  const tree = layoutModDependencyTree({
    nodes: [node('alpha', 'Alpha Mod'), node('beta', 'Beta Mod'), node('core', 'Shared Core')],
    edges: [
      { from: 'alpha', to: 'core', optional: false, constraint: '*' },
      { from: 'beta', to: 'core', optional: false, constraint: '*' },
    ],
    missing: [],
  })

  assert.equal(tree.nodes.length, 3)
  assert.equal(tree.edges.length, 2)
  assert.ok(tree.edges.every((edge) => edge.shared))
  assert.equal(tree.junctions.length, 0)
  for (const path of tree.edges.map((edge) => edge.path)) {
    assertCurvedPath(path)
    assert.doesNotMatch(path, /[LHV]/)
  }
  assert.notEqual(tree.edges[0].path, tree.edges[1].path)
})

test('single dependencies and missing nodes use curved connectors', () => {
  const tree = layoutModDependencyTree({
    nodes: [node('alpha'), node('beta'), node('gamma'), node('loose')],
    edges: [{ from: 'alpha', to: 'beta', optional: false, constraint: '*' }],
    missing: [
      { id: 'missing-api', required_by: 'alpha', optional: false },
      { id: 'missing-api', required_by: 'gamma', optional: true },
    ],
  })

  const direct = tree.edges.find((edge) => edge.to === 'beta')
  assert.ok(direct)
  assert.equal(direct.shared, false)
  assertCurvedPath(direct.path)
  const missingEdges = tree.edges.filter((edge) => edge.to === 'missing-api')
  assert.equal(missingEdges.length, 2)
  assert.ok(missingEdges.every((edge) => edge.shared))
  for (const edge of missingEdges) assertCurvedPath(edge.path)
  assert.equal(tree.junctions.length, 0)
  assert.equal(tree.nodes.find((entry) => entry.id === 'missing-api')?.missing, true)
  assert.equal(tree.isolatedCount, 1)
})

test('cyclic dependencies still receive unique positions and routed edges', () => {
  const tree = layoutModDependencyTree({
    nodes: [node('alpha'), node('beta'), node('gamma')],
    edges: [
      { from: 'alpha', to: 'beta', optional: false, constraint: '*' },
      { from: 'beta', to: 'alpha', optional: false, constraint: '*' },
      { from: 'beta', to: 'gamma', optional: false, constraint: '*' },
    ],
    missing: [],
  })

  assert.equal(new Set(tree.nodes.map(({ x, y }) => `${x},${y}`)).size, tree.nodes.length)
  const sameLayerEdge = tree.edges.find((edge) => tree.nodes.find((node) => node.id === edge.from)?.x === tree.nodes.find((node) => node.id === edge.to)?.x)
  assert.ok(sameLayerEdge)
  const source = tree.nodes.find((node) => node.id === sameLayerEdge.from)
  assert.ok(sameLayerEdge.path.startsWith(`M ${source.x + tree.nodeWidth + 4} `))
  assert.ok(tree.edges.every((edge) => edge.path.startsWith('M ')))
  assert.ok(tree.edges.every((edge) => edge.path.split(/\s+/).length >= 7))
})
