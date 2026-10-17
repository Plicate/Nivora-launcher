import type { PluginManifest } from '../types'
import { createPluginLanSessions, NIVORA_PLUGIN_API_VERSION, type PluginLanSessionRpc } from './api'

export const lanRoomPluginManifest: PluginManifest = {
  id: 'nivora-lan-room',
  name: 'LAN комнаты',
  version: '1.0.0',
  author: 'Nivora Launcher',
  description: 'Создавайте защищённые комнаты для друзей в одной локальной сети.',
  icon: 'globe',
  category: 'Сеть',
  trust: 'verified',
  permissions: ['network:listen'],
  api_version: NIVORA_PLUGIN_API_VERSION,
  entrypoint: 'builtin:lan-room',
  enabled: true,
  installed: true,
  featured: true,
  built_in: true,
  runtime: 'active',
}

export function connectLanRoomPlugin(rpc: PluginLanSessionRpc) {
  return createPluginLanSessions(lanRoomPluginManifest, rpc)
}
