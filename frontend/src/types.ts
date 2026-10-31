import type { ButtonCustomization, ButtonCustomizationTarget } from './services/buttonCustomization'

export type ViewName = 'instances' | 'instance-settings' | 'accounts' | 'mods' | 'modpacks' | 'plugins' | 'themes' | 'settings'
export type LoaderId = 'vanilla' | 'fabric' | 'forge' | 'neoforge' | 'quilt'
export type ProjectType = 'mod' | 'resourcepack' | 'shader' | 'datapack' | 'plugin' | 'modpack'
export type CatalogSort = 'relevance' | 'downloads' | 'newest' | 'updated'
export type ThemeMode = 'system' | 'light' | 'dark' | 'glass'
export type ThemeSource = 'builtin' | 'local' | 'github' | 'shared'
export type PageTheme = 'inherit' | 'light' | 'dark' | 'glass'
export type PageLayout = 'compact' | 'balanced' | 'wide'
export type SidebarPosition = 'left' | 'right' | 'top' | 'bottom'
export type SidebarMode = 'expanded' | 'adaptive' | 'compact'
export type SidebarWidthMode = 'full' | 'content' | 'manual'
export type SidebarVisibility = 'always' | 'hover'
export type AnimationMode = 'full' | 'reduced' | 'off'
export type AnimationStyle = 'fade' | 'slide' | 'scale' | 'none'
export type DensityMode = 'compact' | 'comfortable' | 'spacious'
export type SecurityProfile = 'compat' | 'off' | 'manual' | 'strict'
export type SecurityPermission =
  | 'network:game'
  | 'files:instance'
  | 'desktop:display'
  | 'desktop:audio'
  | 'files:host-read'
  | 'files:host-write'
  | 'process:spawn'
  | 'chat:read'
  | 'input:capture'
  | 'native:load'

export interface LauncherProfile {
  id: string
  name: string
  memory: string
  java_path: string
  graphics_mode: 'auto' | 'nvidia' | 'mesa' | 'software' | string
  account_id: string
  window_width: number
  window_height: number
  fullscreen: boolean
  quick_play_world: string
  quick_play_server: string
  jvm_arguments: string[]
  game_arguments: string[]
  jvm_preset?: string
}

export interface AnimationSettings {
  page_style: AnimationStyle
  page_duration_ms: number
  surface_duration_ms: number
  surface_stagger_ms: number
  page_styles: Record<ViewName, AnimationStyle | ''>
  page_durations_ms: Record<ViewName, number>
}

export interface OverlaySettings {
  enabled: boolean
  hotkey_overlay: string
  hotkey_recording: string
  show_mods: boolean
  show_metrics: boolean
  recording_fps: number
  recording_quality: number
  recording_encoder: 'auto' | 'nvenc' | 'vaapi' | 'x264' | string
  recording_container: 'mp4' | 'mkv' | string
  recording_audio: boolean
  position: 'top-left' | 'top-right' | 'bottom-left' | 'bottom-right' | string
  scale: number
  opacity: number
  compact: boolean
  show_clock: boolean
  show_system_metrics: boolean
  metrics_gpu: boolean
  metrics_cpu: boolean
  metrics_ram: boolean
  metrics_disk: boolean
  metrics_temperatures: boolean
  system_metrics_display: 'overlay' | 'always' | string
  show_chat: boolean
}

export interface RecordingStatus {
  active: boolean
  file: string
  size_bytes: number
  started_at: number
  elapsed_seconds: number
  encoder: string
  instance: string
}

export interface OverlayModEntry {
  name: string
  display_name: string
  enabled: boolean
}

export interface OverlayMetrics {
  cpu_percent?: number | null
  ram_bytes?: number | null
  peak_cpu_percent?: number | null
  peak_ram_bytes?: number | null
  storage_bytes?: number | null
}

export interface OverlayState {
  running: boolean
  instance: {
    name: string
    display_name: string
    game_edition: string
    kind?: string
    minecraft_version: string
    loader: string
  } | null
  session_seconds: number
  played_seconds: number
  metrics: OverlayMetrics
  system_metrics?: SystemMetricsSnapshot
  mods: OverlayModEntry[]
  recording: RecordingStatus
  overlay: OverlaySettings
}

export interface InstanceInfo {
  name: string
  display_name: string
  description: string
  group: string
  icon: string
  banner: string
  played_seconds: number
  version_id: string
  minecraft_version: string
  loader: LoaderId
  loader_version?: string
  path: string
  kind?: 'client' | 'server' | string
  game_edition?: 'java' | 'bedrock' | string
  bedrock_profile?: string
  server_core?: string
  server_core_version?: string
  server_port?: number
  server_eula?: boolean
  running?: boolean
  notes?: string
  tags?: string[]
}

export type BedrockCategory = 'addons' | 'resourcepacks' | 'worlds' | 'skins'

export interface SettingsProfile {
  name: string
  created_at: number
  updated_at: number
  key_count: number
}

export interface WorldBackup {
  id: string
  created_at: number
  size_bytes: number
  destination: 'local' | 'cloud' | string
}

export interface BackupSchedule {
  enabled: boolean
  interval_hours: number
  keep: number
  destination: 'local' | 'cloud' | string
  last_run?: number
  last_status?: string
  last_error?: string
  override?: boolean
}

export interface SystemMetricsSnapshot {
  timestamp: number
  cpu: { percent: number | null }
  memory: { percent: number | null }
  disks: Array<{ mount: string; total_bytes: number; free_bytes: number; percent: number }>
  gpus: Array<{ index: number; name: string; utilization_percent: number | null; memory_used_mib: number | null; memory_total_mib: number | null; temperature_c: number | null }>
  temperatures: { cpu_c: number | null; gpu_c: number | null; motherboard_c: number | null }
  source: { psutil: boolean; nvidia_smi: boolean }
}

export interface CrashHint {
  kind: string
  title: string
  detail: string
}

export interface CrashReport {
  exit_code: number | null
  title: string
  hints: CrashHint[]
  suspected_mods: Array<{ name: string; file?: string; reason?: string }>
  excerpt: string[]
}

export type AutoRepairAction =
  | 'disable_mods'
  | 'install_dependencies'
  | 'set_memory'
  | 'set_java'
  | 'reset_jvm_arguments'
  | 'repair_version'
  | 'backup_configs'
  | 'set_graphics_mode'
  | 'update_loader'
  | 'set_account'
  | 'login_microsoft'
  | 'set_security'

export interface AutoRepairFix {
  id: string
  action: AutoRepairAction
  label: string
  steps: string[]
  params: Record<string, unknown>
  recommended: boolean
}

export interface AutoRepairIssue {
  id: string
  title: string
  detail: string
  evidence: string[]
  fixes: AutoRepairFix[]
  manual: string
}

export interface AutoRepairReport {
  instance_name: string
  exit_code: number | null
  issues: AutoRepairIssue[]
  muted_issues: string[]
  manual?: boolean
  disabled?: boolean
}

export interface AutoRepairResult {
  id: string
  ok: boolean
  message: string
}

export interface ModDependencyGraph {
  nodes: Array<{ id: string; file: string; display_name: string; version: string; enabled: boolean; source: string; loader: string; dependencies: Array<{ id: string; constraint: string; optional: boolean }> }>
  edges: Array<{ from: string; to: string; optional: boolean; constraint: string }>
  missing: Array<{ id: string; required_by: string; optional: boolean }>
}

export interface BedrockRuntimeStatus {
  available: boolean
  supported: boolean
  version: string
  message: string
  store_signed_in?: boolean
  gamertag?: string
  engine_ready?: boolean
  current_edition?: string
  current_version?: string
  /** канал Bedrock: linux-bedrockonlinux | windows-official | unsupported */
  platform?: 'linux-bedrockonlinux' | 'windows-official' | 'unsupported' | string
  /** true означает официальный Minecraft for Windows из Microsoft Store */
  official?: boolean
  /** отчёт о зависимостях среды (устанавливаются автоматически) */
  dependencies?: { name: string; ok: boolean; detail: string }[]
}

export interface BedrockGameVersion {
  edition: 'release' | 'preview'
  version: string
  installed: boolean
}

export interface BedrockVersionsResult {
  versions: BedrockGameVersion[]
  current_edition: 'release' | 'preview' | ''
  current_version: string
}

export interface BedrockSetupStatus {
  started: boolean
  running: boolean
  finished: boolean
  cancelled?: boolean
  error?: string
  logs_tail?: string[]
  result?: {
    exit_code: number
    logs?: string[]
    logs_tail?: string[]
    error_hint?: string
    requires_login: boolean
    installed?: boolean
    failed?: boolean
    current_edition?: string
    current_version?: string
  }
}

export interface BedrockContentItem {
  name: string
  scope: string
  scope_label: string
  category: BedrockCategory
  directory: string
  item_kind: 'file' | 'directory'
  size: number
  modified_at: number
}

/** read_path относителен корню операций экземпляра и читается через instance_file_read */
export interface BedrockScreenshotItem {
  name: string
  read_path: string
  scope_label: string
  size: number
  modified_at: number
}

export interface ExternalInstanceCandidate {
  launcher: string
  display_name: string
  suggested_name: string
  source_path: string
  game_path: string
  version_id: string
  minecraft_version: string
  loader: LoaderId | string
  notes: string
}

export interface AccountInfo {
  account_id: string
  kind: 'offline' | 'microsoft' | string
  username: string
  alias: string
  display_name: string
  expires_at: number
  limit_seconds: number
  used_seconds: number
  is_default: boolean
}

export interface AppearanceSettings {
  sidebar_texture_data: string
  theme: ThemeMode
  active_theme_id: string
  wallpaper_path: string
  wallpaper_data: string
  wallpaper_blur: number
  wallpaper_opacity: number
  accent_color: string
  sidebar_position: SidebarPosition
  sidebar_mode: SidebarMode
  sidebar_width_mode: SidebarWidthMode
  sidebar_width: number
  sidebar_floating: boolean
  sidebar_visibility: SidebarVisibility
  animations: AnimationMode
  animation_settings: AnimationSettings
  density: DensityMode
  instance_layout: InstanceLayout
  ui_scale: number
  radius_tokens: Record<string, number>
  page_themes: Record<ViewName, PageTheme>
  page_layouts: Record<ViewName, PageLayout>
  widget_visibility: Record<string, boolean>
  color_tokens: Record<string, string>
  button_customization: ButtonCustomization
}

/** формат списка экземпляров: обычные строки или плотные плитки */
export type InstanceLayout = 'default' | 'compact' | string

/** информация о последнем релизе из GitHub (фолбэк проверки обновлений) */
export interface RemoteReleaseInfo {
  update_available: boolean
  latest_version: string
  current_version: string
  release_url: string
  published_at: string
  notes: string
  assets: Array<{ name: string; url: string }>
}

export interface SkinExportResult {
  path: string
  folder: string
  opened: boolean
}

export interface SettingsInfo {
  sidebar_texture_data: string
  game_dir: string
  java_path: string
  security_profile: 'manual' | 'strict' | 'off' | string
  security_permissions: string[]
  memory: string
  memory_kib: number
  memory_total_kib: number
  memory_recommended_kib: number
  max_downloads: number
  startup_view: ViewName
  last_view: ViewName
  remember_last_view: boolean
  default_version_filter: 'all' | 'release' | 'snapshot' | 'old_beta' | 'old_alpha' | string
  default_loader: LoaderId
  default_account_id: string
  open_last_instance: boolean
  launch_after_install: boolean
  auto_refresh_metadata: boolean
  metadata_refresh_hours: number
  download_timeout_seconds: number
  download_retries: number
  confirm_destructive_actions: boolean
  auto_repair_enabled: boolean
  auto_repair_muted: string[]
  auto_repair_kinds?: Array<{ id: string; title: string }>
  show_activity_panel: boolean
  show_toasts: boolean
  minimize_on_launch: boolean
  language: string | null
  font_family: string
  file_explorer: 'builtin' | 'system' | string
  file_editor: 'builtin' | 'system' | string
  keep_logs_days: number
  telemetry: boolean
  telemetry_endpoint: string
  keyboard_shortcuts: Record<string, string>
  graphics_mode: 'auto' | 'nvidia' | 'mesa' | 'software' | string
  launcher_profiles: LauncherProfile[]
  active_launcher_profile_id: string
  http_proxy: string
  download_source?: 'auto' | 'official' | 'mirror'
  has_client_id: boolean
  developer_mode: boolean
  theme: ThemeMode
  active_theme_id: string
  wallpaper_path: string
  wallpaper_data: string
  wallpaper_blur: number
  wallpaper_opacity: number
  accent_color: string
  sidebar_position: SidebarPosition
  sidebar_mode: SidebarMode
  sidebar_width_mode: SidebarWidthMode
  sidebar_width: number
  sidebar_floating: boolean
  sidebar_visibility: SidebarVisibility
  animations: AnimationMode
  animation_settings: AnimationSettings
  overlay_settings: OverlaySettings
  backup_schedule: BackupSchedule
  density: DensityMode
  instance_layout: InstanceLayout
  ui_scale: number
  radius_tokens: Record<string, number>
  page_themes: Record<ViewName, PageTheme>
  page_layouts: Record<ViewName, PageLayout>
  widget_visibility: Record<string, boolean>
  color_tokens: Record<string, string>
  button_customization: ButtonCustomization
  instance_groups: string[]
  pinned_instance_groups: string[]
  login_source: 'microsoft' | 'disabled' | string
  login_api_url: string
  catalog_source: 'modrinth' | 'custom' | string
  catalog_api_url: string
  data_source: 'mojang' | 'custom' | string
  data_api_url: string
  resource_catalogs: ResourceCatalog[]
  active_catalog_id: string
  plugin_catalogs: PluginCatalog[]
  active_plugin_catalog_id: string
  cloud_url: string
  cloud_username: string
  cloud_remote_path: string
  cloud_include_accounts: boolean
  cloud_last_sync: number
  cloud_configured: boolean
}

export interface CloudFile {
  key: string
  size: number
  modified_at: number
}

export interface CloudStatus {
  provider: 'webdav' | string
  configured: boolean
  endpoint: string
  username: string
  remote_path: string
  include_accounts: boolean
  last_sync: number
}

export interface ResourceCatalog {
  id: string
  name: string
  kind: 'modrinth' | 'ftb' | 'technic' | 'custom' | string
  api_url: string
}

export interface PluginCatalog {
  id: string
  name: string
  kind: 'modrinth' | 'spigot' | 'hangar' | 'custom' | string
  api_url: string
  enabled?: boolean
}

export interface ServerCore {
  id: string
  name: string
  family: string
  description: string
}

export interface ServerConfig {
  core: string
  version: string
  build: string
  port: number
  max_players: number
  motd: string
  memory: string
  java_path: string
  security_profile: SecurityProfile
  security_permissions: SecurityPermission[]
  eula_accepted: boolean
  auto_restart: boolean
  restart_schedule: { enabled: boolean; time: string; warn_minutes: number; last_run: string }
  snapshot_retention_hours: number
  snapshot_max_count: number
}

export interface ServerStatus {
  start_error?: string
  exit_code?: number | null
  crash?: CrashReport
  running: boolean
  pid: number
  started_at: number
  players: number
  max_players: number
  tps: number
  uptime_seconds: number
  metrics: ServerMetrics
  last_metrics: ServerMetrics | Record<string, unknown>
}

export interface ServerMetrics {
  network_available?: boolean
  network_source?: string
  network_scope?: string
  network_error?: string
  network_rx_bps?: number | null
  network_tx_bps?: number | null
  network_rx_bytes?: number
  network_tx_bytes?: number
  timestamp: number
  pid: number
  running: boolean
  uptime_seconds: number
  ram_bytes: number
  cpu_percent: number
  storage_bytes: number
  players: number
  max_players: number
  tps: number
  peak_ram_bytes?: number
  peak_cpu_percent?: number
  peak_gpu_percent?: number | null
  avg_cpu_percent?: number | null
  avg_ram_bytes?: number | null
  avg_gpu_percent?: number | null
  gpu?: ProcessGpuSample | null
  session?: SessionTelemetry | null
  exit_code?: number | null
  ended_at?: number
}

export interface ProcessGpuSample {
  util_percent?: number | null
  memory_bytes?: number | null
  source?: string
}

export interface SessionTelemetry {
  events?: SessionEvent[]
  mods?: ModResourceUsage[]
}

export interface SessionEvent {
  at: number
  title: string
}

export interface ModResourceUsage {
  mod: string
  lines: number
  warnings: number
  errors: number
  first_seen: number
}

export interface ServerFile {
  name: string
  path: string
  kind: 'file' | 'directory' | string
  size: number
  modified_at: number
  editable: boolean
}

export interface ModAuditCheck {
  id: string
  title: string
}

export interface ModAuditStructure {
  platform: string
  declared: Record<string, unknown>
  classes: number
  strings: number
  references: number
  natives: string[]
  nested_jars: string[]
  skipped_entries: string[]
  signed: boolean
  sources_bytes: number
  source_lines: number
}

export interface ModAuditPrepareResult {
  session_id: string
  state: string
  source: { path: string; label: string; kind: string; sha256: string; size: number }
  structure: ModAuditStructure
  estimate: {
    seconds: number
    label: string
    source_lines: number
    measured_lines_per_second: number
    cpu_mib_per_second: number
  }
  checks: ModAuditCheck[]
  checks_count: number
}

export interface ModAuditFinding {
  id: string
  category: string
  severity: 'critical' | 'warning' | 'notice' | 'info'
  title: string
  detail: string
  recommendation: string
  evidence: { class: string; value: string }[]
}

export interface ModAuditReport {
  session_id: string
  file: string
  label: string
  kind: string
  sha256: string
  size_bytes: number
  platform: string
  declared: Record<string, unknown>
  findings: ModAuditFinding[]
  counts: Record<string, number>
  risk_score: number
  verdict: 'dangerous' | 'suspicious' | 'review' | 'clean'
  verdict_label: string
  categories: string[]
  scanned_files: number
  duration_seconds: number
  sources_wiped: boolean
  scanned_at: number
}

export interface ServerSnapshot {
  id: string
  category: 'mods' | 'plugins' | 'resourcepacks' | 'shaders' | 'saves' | string
  archive: string
  created_at: number
  file_count: number
  source_bytes: number
  retention_hours: number
  target?: string
}

export interface ServerNetworkMapping {
  active: boolean
  method?: 'upnp'
  internal_port?: number
  external_port?: number
  local_ip?: string
  public_ip?: string
  address?: string
  expires_at?: number
  lease_seconds?: number
  error: string
  closing?: boolean
  uncertain_lease?: boolean
}

export interface ServerNetworkReport {
  internal_port: number
  external_port: number
  local_ip: string
  observed_ip: string
  wan_ip: string
  public_ip: string
  address: string
  nat_kind: 'direct' | 'public' | 'double_nat' | 'private' | 'cgnat' | 'unknown'
  upnp_available: boolean
  router_name: string
  router_ip: string
  running: boolean
  local_listening: boolean
  bind_address: string
  bind_all: boolean
  online_mode: boolean
  whitelist: boolean
  external_verified: boolean
  mapping: ServerNetworkMapping
  warnings: string[]
  firewall_help: Array<{ platform: string; command: string; undo: string }>
  verification_note: string
}

export interface ServerDetails {
  server: InstanceInfo
  config: ServerConfig
  memory_total_kib: number
  memory_limit_kib: number
  status: ServerStatus
  files: ServerFile[]
  plugin_files?: ServerFile[]
  logs: { lines: string[]; running: boolean; cursor?: number; reset?: boolean }
  snapshots: { mods: ServerSnapshot[]; plugins: ServerSnapshot[] }
}

export interface ServerPluginResult {
  source: string
  project_id: string
  title: string
  description: string
  author: string
  icon_url: string
  downloads: number
  version: string
  version_id?: string
  download_url: string
  file_name: string
  installed?: boolean
  created_at?: string
  updated_at?: string
  dependencies?: PluginDependency[]
  project_type?: ProjectType
}

export interface PluginDependency {
  project_id?: string
  version_id?: string
  dependency_type?: string
  required?: boolean
  [key: string]: unknown
}

export interface ServerPluginUpdate {
  source: string
  catalog_id: string
  catalog_name?: string
  project_id: string
  file_name: string
  path: string
  current_version: string
  current_version_id: string
  version: string
  version_id: string
  updated_at: string
  trusted_hash: boolean
  dependencies: PluginDependency[]
}

export interface ResourceUpdate {
  source: string
  catalog: string
  catalog_id: string
  project_id: string
  project_type: Exclude<ProjectType, 'plugin' | 'modpack'>
  name: string
  current_version: string
  current_version_id: string
  version: string
  version_id: string
  game_version: string
  loader: string
  path: string
  world_name?: string
  world_title?: string
}

export interface GitHubPluginRepository {
  owner: string
  repo: string
  full_name: string
  name: string
  description: string
  repository_url: string
  stars: number
  updated_at: string
  topics: string[]
}

export interface GitHubPluginManifest {
  id: string
  name: string
  version: string
  author: string
  description: string
  icon: string
  category: string
  api_version: string
  entrypoint: string
  permissions: PluginPermission[]
  sidebar_tab?: { label: string }
  network_hosts: string[]
  featured: boolean
}

export interface GitHubPluginDetails extends GitHubPluginRepository {
  manifest: GitHubPluginManifest
  latest_release: { tag_name: string; name: string; published_at: string; url: string }
  download: { owner: string; repo: string; full_name: string; tag_name: string; version: string; file_name: string; download_url: string; size: number; sha256: string }
}

export interface GitHubThemeRepository {
  owner: string
  repo: string
  full_name: string
  name: string
  description: string
  url: string
  stars: number
  updated_at: string
  topics: string[]
}

export interface ThemeArchiveMetadata {
  author: string
  description: string
  version: string
  creators: string[]
}

export interface ThemeSaveRequest extends ThemeArchiveMetadata {
  name: string
}

export interface AppState {
  build_stamp?: string
  instances: InstanceInfo[]
  accounts: AccountInfo[]
  settings: SettingsInfo
  plugins?: PluginManifest[]
  server_cores?: ServerCore[]
  plugin_catalogs?: PluginCatalog[]
  active_plugin_catalog_id?: string
  system: {
    platform: string
    machine: string
    python: string
    session: string
    gdk_backend?: string
    display: string
    nvidia_smi: boolean
    security?: SecurityStatus
  }
}

export interface SecurityStatus {
  platform: string
  sandbox?: 'bubblewrap' | 'appcontainer' | 'none'
  bubblewrap: boolean
  bubblewrap_path: string
  strict_available: boolean
  profiles: SecurityProfile[]
  permissions: SecurityPermission[]
}

export interface VersionInfo {
  id: string
  type: string
  release_time: string
}

export interface LoaderChoice {
  loader: LoaderId
  version: string
  installer_url: string
}

export interface ModpackModPreviewItem {
  name: string
  source: string
  optional?: boolean
}

export interface ModpackModPreview {
  total: number
  mods: ModpackModPreviewItem[]
  truncated?: boolean
}

export interface ModpackDependencyRequirements {
  minecraft?: string
  loader?: LoaderId
  loader_version?: string
  compatible?: boolean
  issues?: string[]
  /** список модов, которые окажутся в экземпляре после импорта */
  mods?: ModpackModPreview
}

export interface ModResult {
  project_id?: string
  id?: string
  title?: string
  name?: string
  description?: string
  icon_url?: string
  downloads?: number
  author?: string
  slug?: string
  installed?: boolean
  source?: string
  version?: string
  download_url?: string
  file_name?: string
  project_type?: ProjectType
  game_versions?: string[]
  date_created?: string
  date_modified?: string
  categories?: string[]
  /** категории без платформ из display_categories ответа /v2/search */
  display_categories?: string[]
  /** число подписчиков проекта в API Modrinth */
  follows?: number
  /** поддержка среды запуска: required, optional, unsupported или unknown */
  client_side?: string
  server_side?: string
  license?: { id?: string; name?: string; url?: string } | string
  versions?: string[]
  dependencies?: PluginDependency[]
  version_id?: string
  catalog_id?: string
}

export interface InstanceAsset {
  name: string
  display_name?: string
  path: string
  size: number
  modified_at: number
  kind: string
  enabled?: boolean
  world_id?: string
  version?: string
  author?: string
  source?: string
  project_id?: string
  icon_url?: string
  icon_data?: string
}

export interface InstanceWorld {
  id: string
  name: string
  path: string
  size: number
  last_played: number | null
  icon: string
  datapacks?: InstanceAsset[]
}

export interface InstanceServer {
  id: string
  name: string
  address: string
  accept_textures: boolean
}

export interface InstanceSettings {
  memory: string
  java_path: string
  graphics_mode: 'auto' | 'nvidia' | 'mesa' | 'software' | string
  window_width: number
  window_height: number
  fullscreen: boolean
  quick_play_world: string
  quick_play_server: string
  jvm_arguments: string[]
  game_arguments: string[]
  jvm_preset: string
  console_colors: InstanceConsoleColors
  security_profile: SecurityProfile
  security_permissions: SecurityPermission[]
  env_variables?: Record<string, string>
  launch_wrapper?: string
  pre_launch_command?: string
  post_launch_command?: string
}

export interface InstanceConsoleColors {
  error: string
  info: string
  warning: string
}

export interface InstanceFile {
  name: string
  path: string
  kind: 'file' | 'directory'
  size: number
  modified_at: number
  editable: boolean
}

export interface InstanceFileReadResult {
  content: string
  encoding: 'text' | 'base64'
  mime_type?: string
}

export interface InstanceLogArchive {
  id: string
  modified_at: number
  size: number
}

export interface InstanceLogArchiveReadResult {
  content: string
  next_offset: number
  eof: boolean
}

export interface InstanceDetails {
  instance: InstanceInfo
  settings: InstanceSettings
  assets: Record<string, InstanceAsset[]>
  worlds: InstanceWorld[]
  servers: InstanceServer[]
  snapshots: Record<string, ServerSnapshot[]>
}

// сравнение экземпляров

export interface InstanceDiffEntry {
  path: string
  name: string
  version: string
  loader: string
  mod_id: string
  size: number
  sha256: string
  enabled: boolean
  enabled_changed?: boolean
  from_version?: string
  to_version?: string
  from_path?: string
  to_path?: string
}

export interface InstanceDiffBucket {
  label: string
  added: InstanceDiffEntry[]
  removed: InstanceDiffEntry[]
  changed: InstanceDiffEntry[]
}

export interface InstanceComparison {
  source: InstanceInfo
  target: InstanceInfo
  report: {
    mods: InstanceDiffBucket
    resourcepacks: InstanceDiffBucket
    shaderpacks: InstanceDiffBucket
    configs: InstanceDiffBucket
    options: { changed: Array<{ key: string; from: string; to: string }> }
    meta: Record<string, { from: string; to: string }>
    stats: { source: InstanceManifestStats; target: InstanceManifestStats }
    summary: {
      mods_added: number
      mods_removed: number
      mods_changed: number
      configs_changed: number
      packs_changed: number
    }
  }
  source_stats: InstanceManifestStats
  target_stats: InstanceManifestStats
}

export interface InstanceManifestStats {
  mod_count: number
  mod_bytes: number
  class_count: number
  unpacked_bytes: number
  shader_count: number
  shader_programs: number
  resourcepack_count: number
}

export interface PortChangeItem {
  kind: 'mod' | 'resourcepack' | 'shaderpack' | 'config'
  path: string
}

export interface InstancePortResult {
  copied: string[]
  failed: Array<{ path: string; error: string }>
  snapshotted: string[]
  target: InstanceInfo
  details: InstanceDetails
}

// автонастройка экземпляра

/** режим автонастройки: что важнее — картинка или кадры */
export type AutotuneMode = 'balanced' | 'quality' | 'performance'

export interface AutotunePlanChange {
  key: string
  category?: string
  label: string
  from: string
  to: string
  reason: string
}

export interface AutotuneRecommendation {
  title: string
  detail: string
}

/** профиль оборудования для подбора настроек Minecraft */
export interface AutotuneHardware {
  tier: 'low' | 'balanced' | 'high'
  tier_title: string
  cpu_model: string
  cpu_cores: number
  memory_mib: number
  gpu: string
  gpu_vendor: string
  gpu_discrete: boolean
  /** класс видеокарты: потолок настроек отрисовки */
  gpu_class?: 'software' | 'entry' | 'mid' | 'high' | 'enthusiast'
  gpu_class_title?: string
  /** объём видеопамяти, МиБ (0 — неизвестен) */
  vram_mib?: number
  /** разрешение основного дисплея (0 — неизвестно) */
  display_width?: number
  display_height?: number
  /** частота обновления основного дисплея, Гц (0 — неизвестна) */
  display_hz?: number
  reasons: string[]
}

export interface AutotunePlan {
  /** выбранный режим: сбалансированный / качество / оптимизация */
  mode?: AutotuneMode
  mode_title?: string
  mode_description?: string
  heap_mib: number
  memory: string
  jvm_preset: string
  jvm_arguments: string[]
  graphics_mode: string
  java_path: string
  java_major_required?: number
  launch_wrapper?: string
  changes: AutotunePlanChange[]
  recommendations?: AutotuneRecommendation[]
  hardware?: AutotuneHardware
  mod_buckets?: Record<string, number>
  stats: InstanceManifestStats
  system_memory_kib: number
  notes: string[]
  instance_name?: string
  /** правки options.txt: ключ → значение (применяются кнопкой «Применить») */
  options_changes?: Record<string, string>
}

export interface AutotuneResult {
  plan: AutotunePlan
  applied: boolean
  settings: InstanceSettings
  /** фактически записанные в options.txt ключи (после apply) */
  minecraft_options?: Record<string, string>
  /** ключи, не записанные в options.txt: файла ещё нет (игра его создаст при первом запуске) */
  skipped_minecraft_options?: string[]
}

// настройки автора плагинов

export interface PluginVerifyResult {
  id: string
  name: string
  version: string
  signed: boolean
}

export interface PluginKeypairResult {
  private_pem: string
  public_pem: string
}

// закрепления и безопасное обновление

export interface InstancePin {
  kind: 'world' | 'mod' | 'resourcepack' | 'shader' | 'server'
  value: string
}

export interface SafeUpdateSession {
  id: string
  created_at: number
  snapshots: Record<string, string>
  applied: Array<{ project_id: string; name: string; version: string; version_id: string }>
  failed: Array<{ name: string; error: string }>
  rolled_back: boolean
  rolled_back_at?: number
}

export interface SafeUpdateResult {
  session: SafeUpdateSession
  applied: SafeUpdateSession['applied']
  failed: SafeUpdateSession['failed']
  snapshots: Record<string, string>
  errors?: string[]
  sessions: SafeUpdateSession[]
  details?: InstanceDetails
  instance_name?: string
}

export interface ShaderAnalyzeResult {  analysis: {
    name: string
    format: string
    files: number
    programs: string[]
    features: string[]
    shadow_resolution: number
    size_bytes: number
    complexity: number
    estimated_vram_mb: number
    notes: string[]
  }
  gpu: {
    name: string
    vram_mb: number
    tier: string
    tier_label: string
    source: string
    recommendations: Array<{ name: string; detail: string }>
  }
  verdict: { label: string; tone: string }
  benchmark_available: boolean
}

export interface ShaderBenchmarkState {
  phase: 'idle' | 'prepare' | 'baseline' | 'shader' | 'done' | 'failed'
  progress?: number
  message?: string
  error?: string
  baseline?: { cpu_percent?: number | null; ram_mb?: number; gpu_percent?: number | null; gpu_memory_mb?: number | null }
  shader?: { cpu_percent?: number | null; ram_mb?: number; gpu_percent?: number | null; gpu_memory_mb?: number | null }
  verdict?: { label: string; tone: string; gpu_delta_percent?: number; metric?: string }
}

export interface InstanceVersionComponent {
  id: string
  title: string
  version: string
  meta: string
  changeable: boolean
  removable: boolean
  overridden?: string
}

export interface InstanceVersionsPayload {
  components: InstanceVersionComponent[]
  minecraft_version: string
  loader: string
  loader_version: string
  lwjgl_versions: string[]
  library_overrides: Record<string, string>
}

export interface InstanceLog {
  lines: string[]
  running: boolean
  metrics: ServerMetrics
  last_metrics?: ServerMetrics | Record<string, unknown>
}

export interface SkinInfo {
  skin_id: string
  name: string
  model: string
  path: string
  image: string
}

export interface SkinProfile {
  id?: string
  name?: string
  skins?: Array<{ id?: string; state?: string; url?: string; variant?: string }>
  capes?: Array<{ id?: string; state?: string; name?: string; alias?: string; url?: string }>
}

export interface ActivityState {
  open: boolean
  title: string
  detail: string
  progress: number
  busy: boolean
  cancelling: boolean
  logs: string[]
  verification_code?: string
  verification_uri?: string
}

export interface InstanceIconBlock {
  id: string
  name: string
  /** плоская текстура боковой грани для совместимости */
  data_url: string
  /** верхняя грань; у блоков с одинаковой текстурой совпадает с боковой */
  top_data_url?: string
  /** боковая грань; отсутствует, если совпадает с data_url */
  side_data_url?: string
}

export interface SelectOption {
  value: string
  label: string
  meta?: string
}

export type PluginPermission =
  | 'links:open'
  | 'network:fetch'
  | 'network:dns'
  | 'network:ping'
  | 'network:listen'
  | 'launcher:servers'
  | 'launcher:dialogs'
  | 'network:game'
  | 'storage:workspace'
  | 'storage:database'
  | 'files:plugin'
  | 'files:instance'
  | 'files:host-read'
  | 'files:host-write'
  | 'process:spawn'
  | 'chat:read'
  | 'input:capture'
  | 'native:load'
  | 'ui:compose'
  | 'events:launcher'
  | 'runtime:observe'
  | 'lifecycle:observe'
  | 'process:observe'
  | 'files:launcher-root-read'
  | 'files:launcher-config-projected'
  | 'network:http'
  | 'network:https'
  | 'network:websocket'
  | 'network:tcp-client'
  | 'network:udp-client'
  | 'network:proxy-control'
  | 'network:traffic-observe'
  | 'network:traffic-intercept'
  | 'network:tunnel'
  | 'os:open-url'
  | 'os:open-file'
  | 'os:reveal-in-file-manager'
  | 'os:clipboard-read'
  | 'os:clipboard-write'
  | 'os:spawn-external'
  | 'os:browser-session'
  | 'auth:oauth'
  | 'auth:token-use'
  | 'secrets:store'
  | 'secrets:use'
  | 'ai:invoke-cloud'
  | 'ai:invoke-local'
  | 'downloads:manage'
  | 'updates:manage'
  | 'dependencies:resolve'
  | 'integrity:scan'
  | 'metrics:emit'
  | 'logs:write'
  | 'presence:discord'
  | 'scheduler:jobs'
  | 'commands:register'
  | 'shortcuts:register'
  | 'instances:read'
  | 'instances:mutate'
  | 'mods:read'
  | 'mods:install'
  | 'packs:manage'
  | 'accounts:projected-read'
  | 'backup:export'
  | 'backup:restore'
  | 'sync:cloud'
  | 'ipc:sidecar'
  | 'policy:read'
  | 'crypto:sign'
  | 'crypto:verify'

export type PluginTrust = 'verified' | 'community' | 'local'

export interface PluginManifest {
  id: string
  name: string
  version: string
  author: string
  description: string
  icon: string
  category: string
  trust: PluginTrust
  permissions: PluginPermission[]
  api_version?: string
  entrypoint?: string
  sidebar_tab?: { label: string }
  network_hosts?: string[]
  enabled: boolean
  installed: boolean
  featured?: boolean
  last_updated?: string
  built_in?: boolean
  runtime?: 'active' | 'archive-only' | string
  signed?: boolean
  signature_valid?: boolean
  publisher_key_id?: string | null
  publisher_public_key?: string | null
  publisher_fingerprint?: string | null
  publisher_pinned?: boolean
  publisher_revocation_authority?: boolean
  publisher_trusted?: boolean
  package_valid?: boolean
  package_error?: string | null
  revoked?: boolean
  revocation_reason?: string | null
  requested_permissions?: PluginPermission[]
  granted_permissions?: PluginPermission[]
  local_untrusted_execution_allowed?: boolean
  execution_allowed?: boolean
  execution_blocked_reason?: string | null
  source_url?: string
  source_entrypoint?: string
  source_text?: string
  license_name?: string
  license_text?: string
  publisher?: string
  published_at?: string
}

export type PluginUiFragment =
  | { kind: 'text'; text: string }
  | { kind: 'input'; id: string; label: string; value?: string; placeholder?: string }
  | { kind: 'button'; id: string; label: string; action: string }
  | { kind: 'form'; id: string; title?: string; children: PluginUiFragment[] }
  | { kind: 'list'; id: string; items: string[] }

export type ThemeButtonCustomizationOverride = Partial<Pick<ButtonCustomization,
  'width' | 'height' | 'fill' | 'border' | 'font_family' | 'font_size' | 'texture_fit' | 'crop_enabled' | 'crop'
>> & { outline?: Partial<ButtonCustomization['outline']>; label?: string }

export type ThemeButtonCustomization = Pick<ButtonCustomization,
  'width' | 'height' | 'fill' | 'border' | 'font_family' | 'font_size' | 'outline' | 'texture_fit' | 'crop_enabled' | 'crop' | 'labels'
> & { overrides?: Partial<Record<ButtonCustomizationTarget, ThemeButtonCustomizationOverride>> }

export interface LauncherThemePreset {
  id: string
  schema_version: 1 | 2
  name: string
  author: string
  source: ThemeSource
  description: string
  mode: ThemeMode
  accent: string
  accent_strong: string
  background: string
  panel: string
  panel_soft: string
  density: DensityMode
  sidebar_mode: SidebarMode
  featured?: boolean
  version?: string
  creators?: string[]
  source_url?: string
  locale?: 'ru' | 'en'
  button_customization?: ThemeButtonCustomization
  radius_tokens?: Record<string, number>
  resources?: Record<string, string>
  resource_data?: Record<string, string>
  plugin?: { id: string; name: string; version: string; api_version: '3'; entrypoint: string; sources?: string[] }
}

export type MemoryUnit = 'KiB' | 'MiB' | 'GiB'

export interface BackendEvent {
  kind: 'log' | 'download' | 'status' | string
  operation_id?: string
  instance_name?: string
  code?: string
  verification_uri?: string
  line?: string
  message?: string
  event?: string
  destination?: string
  current?: number
  total?: number | null
  metrics?: ServerMetrics
  launch_id?: string
  log_id?: string
  sequence?: number
}

export interface JvmPreset {
  id: string
  name: string
  description: string
  flags: string[]
  recommended_for: string[]
}

export interface OptimizationSystem {
  ram_kib: number
  ram_gib: number
  cpu_count: number
  os: string
  gpu_vendor: string
  gpu_renderer: string
  gpu_driver: string
  gpu_vram_mib: number
  gpu_is_hybrid: boolean
}

export interface OptimizationInstance {
  loader: string
  mc_version: string
  mod_count: number
  weight: 'vanilla' | 'light' | 'medium' | 'heavy' | 'legacy'
  java_required?: number | null
}

export interface OptimizerMod {
  name: string
  mod_id: string
  description: string
  url: string
  category: 'fps' | 'memory' | 'loading' | 'network'
  required_side: 'client' | 'server' | 'both'
  already_installed: boolean
}

export interface OptimizationRecommendations {
  memory_mib: number
  preset_id: string
  preset_name: string
  preset_flags: string[]
  optimizer_mods: OptimizerMod[]
  warnings: string[]
}

export interface OptimizationCurrentState {
  memory_mib: number
  preset_id: string
}

export interface OptimizationReport {
  system: OptimizationSystem
  instance: OptimizationInstance
  recommendations: OptimizationRecommendations
  current: OptimizationCurrentState
}

export interface LaunchHistoryEntry {
  launch_id: string
  started_at: number
  loader: string
  preset: string
  memory: string
  mc_version: string
  exit_code: number | null
  elapsed_seconds: number | null
  time_to_window_seconds: number | null
}

export interface CacheStats {
  total_bytes: number
  total_mib: number
  file_count: number
}
