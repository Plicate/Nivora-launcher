import type {
  PluginActionDefinition,
  PluginCommandDefinition,
  PluginConfigField,
  PluginEventName,
  PluginI18nDefinition,
  PluginShortcutDefinition,
  PluginThemeDefinition,
  PluginUiDefinition,
} from '../../../plugins/api'

export interface RuntimePluginManifest {
  id: string
  name: string
  version: string
  api_version: '2' | '3' | string
  entrypoint: string
  permissions: readonly string[]
  network_hosts?: readonly string[]
  author?: string
  description?: string
  icon?: string
  category?: string
  config?: Readonly<Record<string, PluginConfigField>>
  ui?: PluginUiDefinition
  i18n?: PluginI18nDefinition
  theme?: PluginThemeDefinition
  actions?: readonly PluginActionDefinition[]
  events?: readonly PluginEventName[]
  commands?: readonly PluginCommandDefinition[]
  shortcuts?: readonly PluginShortcutDefinition[]
  sandbox_profile?: string
  capabilities?: unknown
  compatibility_matrix?: unknown
  host_requirements?: unknown
  integrity?: { files: Readonly<Record<string, string>> }
  signature?: {
    algorithm: 'ed25519'
    key_id: string
    public_key: string
    value: string
  }
}
