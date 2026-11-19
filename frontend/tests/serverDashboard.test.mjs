import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { bridgeAllowList } from './helpers/registry.mjs'
const workspace = readFileSync(new URL('../src/components/ServerWorkspace.vue', import.meta.url), 'utf8')
const bridge = readFileSync(new URL('../src/services/bridge.ts', import.meta.url), 'utf8')

test('server overview includes the same usable console as the console tab', () => {
  const overview = workspace.slice(workspace.indexOf('v-else-if="tab === \'overview\'"'), workspace.indexOf('v-else-if="tab === \'console\'"'))
  assert.ok(overview.includes('<ServerConsole'))
  assert.ok(overview.includes('@send="sendCommand"'))
  assert.ok(workspace.includes("tab.value === 'overview' || tab.value === 'console'"))
})

test('incoming and outgoing server traffic have separate charts and honest unavailable states', () => {
  assert.ok(workspace.includes("metricPoints('network_rx_bps')"))
  assert.ok(workspace.includes("metricPoints('network_tx_bps')"))
  assert.ok(workspace.includes('network_available'))
  assert.ok(workspace.includes('не полный сетевой биллинг'))
  assert.ok(workspace.includes('network_rx_bytes'))
  assert.ok(workspace.includes('network_tx_bytes'))
})

test('restart crosses the desktop boundary and exposes its busy state', () => {
  const allowed = [...bridgeAllowList()].map((operation) => `"${operation}"`).join(' ')
  assert.ok(allowed.includes('"server_restart"'))
  assert.ok(bridge.includes("request<ServerStatus>('server_restart'"))
  assert.ok(workspace.includes('bridge.restartServer('))
  assert.ok(workspace.includes("activeAction === 'restart'"))
  assert.ok(workspace.includes('server-uptime-label'))
})

test('browser preview rejects real server controls instead of faking a successful start', () => {
  assert.ok(bridge.includes("['server_start', 'server_stop', 'server_restart', 'server_command'].includes(operation)"))
  assert.ok(bridge.includes('браузерный preview не запускает Java'))
  assert.ok(workspace.includes('server-launch-error'))
})
