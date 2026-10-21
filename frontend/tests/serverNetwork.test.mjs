import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { bridgeAllowList } from './helpers/registry.mjs'
import { publicIpv4, serverAddress, canMapServer } from '../src/services/serverNetwork.ts'

test('public server addresses reject private, CGNAT, malformed and reserved IPs', () => {
  for (const value of ['127.0.0.1', '10.0.0.1', '100.64.0.1', '100.127.255.254', '172.16.0.1', '192.0.0.9', '192.0.0.10', '192.168.1.2', '169.254.169.254', '224.0.0.1', '192.0.2.1', '198.51.100.1', '203.0.113.1', '1.2.3.999', '01.2.3.4', 'example.com', '::1']) {
    assert.equal(publicIpv4(value), false, value)
    assert.equal(serverAddress(value, 25565), '')
  }
  assert.equal(serverAddress('8.8.4.4', 25570), '8.8.4.4:25570')
  for (const port of [0, 65536, NaN, 2.5]) assert.equal(serverAddress('8.8.4.4', port), '')
})

test('UPnP action requires a running server, public router WAN and unrestricted bind', () => {
  const report = { upnp_available: true, bind_all: true, local_listening: true, running: true, nat_kind: 'public' }
  assert.equal(canMapServer(report, true), true)
  assert.equal(canMapServer(report, false), false)
  assert.equal(canMapServer(undefined, true), false)
  assert.equal(canMapServer({ ...report, upnp_available: false }, true), false)
  assert.equal(canMapServer({ ...report, bind_all: false }, true), false)
  assert.equal(canMapServer({ ...report, local_listening: false }, true), false)
  assert.equal(canMapServer({ ...report, running: false }, true), false)
  for (const nat_kind of ['direct', 'private', 'cgnat', 'double_nat', 'unknown']) {
    assert.equal(canMapServer({ ...report, nat_kind }, true), false)
  }
})

test('every network bridge operation is admitted by the desktop boundary', () => {
  const bridge = readFileSync(new URL('../src/services/bridge.ts', import.meta.url), 'utf8')
  const allowed = [...bridgeAllowList()].map((operation) => `"${operation}"`).join(' ')
  for (const operation of ['server_network_diagnose', 'server_network_status', 'server_network_open', 'server_network_close', 'server_network_prepare']) {
    assert.ok(bridge.includes(`'${operation}'`), operation)
    assert.ok(allowed.includes(`"${operation}"`), operation)
  }
})

test('network UI is opt-in and does not expose a false external reachability promise', () => {
  const panel = readFileSync(new URL('../src/components/ServerNetworkPanel.vue', import.meta.url), 'utf8')
  assert.ok(panel.includes('confirmDialog('))
  assert.ok(panel.includes('UPnP-проброс создан'))
  assert.ok(panel.includes('Доступность из интернета нужно проверить отдельно'))
  assert.ok(panel.includes('Ручные правила закрываются вручную'))
  assert.ok(panel.includes('const fresh = await bridge.serverNetworkDiagnose'))
  assert.ok(panel.includes('Сбросить на автоадрес'))
  const mounted = panel.slice(panel.indexOf('onMounted(() =>'), panel.indexOf('onBeforeUnmount(() =>'))
  assert.equal(mounted.includes('diagnose()'), false, 'opening a tab must not call the IP echo service automatically')
})
