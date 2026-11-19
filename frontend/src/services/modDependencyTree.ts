import type { ModDependencyGraph } from '../types'

export interface ModTreeNode {
  id: string
  name: string
  x: number
  y: number
  disabled: boolean
  missing: boolean
  optional: boolean
}

export interface ModTreeEdge {
  key: string
  path: string
  from: string
  to: string
  optional: boolean
  missing: boolean
  shared: boolean
}

export interface ModTreeJunction {
  key: string
  path: string
  to: string
  from: string[]
  optional: boolean
  missing: boolean
}

export interface ModDependencyTree {
  nodes: ModTreeNode[]
  edges: ModTreeEdge[]
  junctions: ModTreeJunction[]
  width: number
  height: number
  nodeWidth: number
  nodeHeight: number
  isolatedCount: number
}

const NODE_WIDTH = 196
const NODE_HEIGHT = 42
const COLUMN_WIDTH = 252
const ROW_HEIGHT = 64
const PADDING = 24

interface Relation {
  from: string
  to: string
  optional: boolean
  missing: boolean
}

interface Position { x: number; y: number }

export function layoutModDependencyTree(graph?: ModDependencyGraph): ModDependencyTree {
  const empty: ModDependencyTree = {
    nodes: [], edges: [], junctions: [], width: 0, height: 0,
    nodeWidth: NODE_WIDTH, nodeHeight: NODE_HEIGHT, isolatedCount: 0,
  }
  if (!graph) return empty

  const nodeMap = new Map<string, ModDependencyGraph['nodes'][number]>()
  for (const node of graph.nodes) {
    const key = node.id.toLowerCase()
    const stem = node.file.replace(/\.(jar|zip)(\.disabled)?$/i, '')
    const named = Boolean(node.display_name) && node.display_name !== stem
    const existing = nodeMap.get(key)
    if (!existing) {
      nodeMap.set(key, node)
      continue
    }
    const existingStem = existing.file.replace(/\.(jar|zip)(\.disabled)?$/i, '')
    const existingNamed = Boolean(existing.display_name) && existing.display_name !== existingStem
    if (named && !existingNamed) nodeMap.set(key, node)
  }

  const canonicalId = (id: string) => nodeMap.get(id.toLowerCase())?.id ?? id
  const relationMap = new Map<string, Relation>()
  const addRelation = (fromValue: string, toValue: string, optional: boolean, missing: boolean) => {
    const from = canonicalId(fromValue)
    const to = canonicalId(toValue)
    const key = `${from.toLowerCase()}->${to.toLowerCase()}`
    const existing = relationMap.get(key)
    relationMap.set(key, {
      from,
      to,
      optional: existing ? existing.optional && optional : optional,
      missing: existing ? existing.missing || missing : missing,
    })
  }
  for (const edge of graph.edges) addRelation(edge.from, edge.to, edge.optional, false)

  const missingGroups = new Map<string, { id: string; entries: typeof graph.missing }>()
  for (const item of graph.missing) {
    const key = item.id.toLowerCase()
    const group = missingGroups.get(key) ?? { id: item.id, entries: [] }
    group.entries.push(item)
    missingGroups.set(key, group)
  }
  for (const group of missingGroups.values()) {
    if (nodeMap.has(group.id.toLowerCase())) continue
    for (const item of group.entries) addRelation(item.required_by, group.id, item.optional, true)
  }

  const connected = new Set<string>()
  for (const relation of relationMap.values()) {
    connected.add(relation.from.toLowerCase())
    connected.add(relation.to.toLowerCase())
  }
  const treeNodes = [...nodeMap.values()].filter((node) => connected.has(node.id.toLowerCase()))
  const isolatedCount = nodeMap.size - treeNodes.length
  if (!treeNodes.length && !missingGroups.size) return { ...empty, isolatedCount }

  const treeIds = new Set(treeNodes.map((node) => node.id.toLowerCase()))
  const relations = [...relationMap.values()]
  const ranks = new Map(treeNodes.map((node) => [node.id.toLowerCase(), 0]))
  for (let pass = 0; pass < treeNodes.length; pass += 1) {
    let changed = false
    for (const node of treeNodes) {
      const id = node.id.toLowerCase()
      const nextRank = relations.reduce((rank, relation) => {
        if (relation.from.toLowerCase() !== id) return rank
        if (!treeIds.has(relation.to.toLowerCase())) return Math.max(rank, 1)
        return Math.max(rank, (ranks.get(relation.to.toLowerCase()) ?? 0) + 1)
      }, 0)
      const boundedRank = Math.min(treeNodes.length - 1, nextRank)
      if (boundedRank > (ranks.get(id) ?? 0)) {
        ranks.set(id, boundedRank)
        changed = true
      }
    }
    if (!changed) break
  }

  const maxDepth = Math.max(1, ...ranks.values())
  const missingColumn = maxDepth + 1
  const missingNodes = [...missingGroups.values()].filter((group) => !nodeMap.has(group.id.toLowerCase()))
  const columnById = new Map(treeNodes.map((node) => [node.id.toLowerCase(), maxDepth - (ranks.get(node.id.toLowerCase()) ?? 0)]))
  const longRelations = relations
    .filter((relation) => Math.abs((columnById.get(relation.from.toLowerCase()) ?? missingColumn) - (columnById.get(relation.to.toLowerCase()) ?? missingColumn)) > 1)
    .sort((a, b) => a.to.localeCompare(b.to, 'ru') || a.from.localeCompare(b.from, 'ru'))
  const nodeTop = PADDING + longRelations.length * 12
  const longLaneY = new Map(longRelations.map((relation, index) => [`${relation.from.toLowerCase()}->${relation.to.toLowerCase()}`, PADDING / 2 + index * 12]))
  const columns = new Map<number, string[]>()
  for (const node of treeNodes) {
    const column = maxDepth - (ranks.get(node.id.toLowerCase()) ?? 0)
    const ids = columns.get(column) ?? []
    ids.push(node.id)
    columns.set(column, ids)
  }

  const positions = new Map<string, Position>()
  const nodes: ModTreeNode[] = []
  const displayOf = (id: string) => nodeMap.get(id.toLowerCase())?.display_name || id
  for (const [column, ids] of [...columns.entries()].sort(([a], [b]) => a - b)) {
    const barycenter = (id: string) => {
      const parentYs = relations
        .filter((relation) => relation.to.toLowerCase() === id.toLowerCase())
        .map((relation) => positions.get(relation.from.toLowerCase()))
        .filter((position): position is Position => Boolean(position))
        .map((position) => position.y + NODE_HEIGHT / 2)
      return parentYs.length ? parentYs.reduce((sum, y) => sum + y, 0) / parentYs.length : Number.POSITIVE_INFINITY
    }
    ids.sort((a, b) => barycenter(a) - barycenter(b) || displayOf(a).localeCompare(displayOf(b), 'ru'))
    ids.forEach((id, index) => {
      const node = nodeMap.get(id.toLowerCase())!
      const position = { x: PADDING + column * COLUMN_WIDTH, y: nodeTop + index * ROW_HEIGHT }
      positions.set(id.toLowerCase(), position)
      nodes.push({
        id: node.id,
        name: displayOf(id),
        ...position,
        disabled: !node.enabled,
        missing: false,
        optional: false,
      })
    })
  }

  missingNodes.sort((a, b) => a.id.localeCompare(b.id, 'ru'))
  missingNodes.forEach((group, index) => {
    const position = { x: PADDING + missingColumn * COLUMN_WIDTH, y: nodeTop + index * ROW_HEIGHT }
    positions.set(group.id.toLowerCase(), position)
    nodes.push({
      id: group.id,
      name: group.id,
      ...position,
      disabled: false,
      missing: true,
      optional: group.entries.every((item) => item.optional),
    })
  })

  const routedGroups = new Map<string, Array<{ relation: Relation; from: Position; to: Position; side: 'left' | 'right' }>>()
  for (const relation of relations) {
    const from = positions.get(relation.from.toLowerCase())
    const to = positions.get(relation.to.toLowerCase())
    if (!from || !to) continue
    const side = to.x > from.x ? 'left' : 'right'
    const key = `${relation.to.toLowerCase()}:${side}`
    const group = routedGroups.get(key) ?? []
    group.push({ relation, from, to, side })
    routedGroups.set(key, group)
  }

  const edges: ModTreeEdge[] = []
  const junctions: ModTreeJunction[] = []
  const curvePath = (x1: number, y1: number, x2: number, y2: number, bendAt: 'start' | 'end' = 'start') => {
    const direction = x2 < x1 ? -1 : 1
    const distance = Math.abs(x2 - x1)
    const reach = Math.min(96, distance * 0.42)
    const bend = y1 === y2 ? Math.min(32, distance * 0.32) : 0
    const firstControlY = y1 - (bendAt === 'start' ? bend : 0)
    const secondControlY = y2 - (bendAt === 'end' ? bend : 0)
    return `M ${x1} ${y1} C ${x1 + direction * reach} ${firstControlY}, ${x2 - direction * reach} ${secondControlY}, ${x2} ${y2}`
  }
  const laneCurvePath = (x1: number, y1: number, x2: number, y2: number, laneY: number) => {
    const direction = x2 < x1 ? -1 : 1
    const reach = Math.min(Math.abs(x2 - x1) * 0.3, 96)
    return `M ${x1} ${y1} C ${x1 + direction * reach} ${laneY}, ${x2 - direction * reach} ${laneY}, ${x2} ${y2}`
  }
  for (const group of routedGroups.values()) {
    group.sort((a, b) => a.from.y - b.from.y || a.relation.from.localeCompare(b.relation.from, 'ru'))
    const { to, side } = group[0]
    const targetX = side === 'left' ? to.x - 7 : to.x + NODE_WIDTH + 7
    const sourceX = (from: Position) => side === 'left' || from.x === to.x ? from.x + NODE_WIDTH + 4 : from.x - 4
    const targetY = to.y + NODE_HEIGHT / 2
    const shared = group.length > 1

    for (const { relation, from } of group) {
      const x1 = sourceX(from)
      const y1 = from.y + NODE_HEIGHT / 2
      const laneY = longLaneY.get(`${relation.from.toLowerCase()}->${relation.to.toLowerCase()}`)
      const path = laneY === undefined ? curvePath(x1, y1, targetX, targetY) : laneCurvePath(x1, y1, targetX, targetY, laneY)
      edges.push({
        key: `${relation.from}->${relation.to}`,
        path,
        from: relation.from,
        to: relation.to,
        optional: relation.optional,
        missing: relation.missing,
        shared,
      })
    }

  }

  const columnCount = missingNodes.length ? missingColumn + 1 : maxDepth + 1
  const tallestColumn = Math.max(...[...columns.values()].map((ids) => ids.length), missingNodes.length, 1)
  return {
    nodes,
    edges,
    junctions,
    isolatedCount,
    nodeWidth: NODE_WIDTH,
    nodeHeight: NODE_HEIGHT,
    width: PADDING * 2 + columnCount * COLUMN_WIDTH - (COLUMN_WIDTH - NODE_WIDTH),
    height: nodeTop + PADDING + tallestColumn * ROW_HEIGHT,
  }
}
