/** type-only contract for Nivora Launcher v3 plugins; it ships no runtime code */

import type {
	PluginActionApi,
	PluginActionDefinition,
	PluginCommandApi,
	PluginCommandDefinition,
	PluginConfigField,
	PluginDatabaseApi,
	PluginEventName,
	PluginEventPayloadMap,
	PluginI18nApi,
	PluginI18nDefinition,
	PluginKeyValueStore,
	PluginLauncherApi,
	PluginNetworkApi,
	PluginNotificationApi,
	PluginShortcutApi,
	PluginShortcutDefinition,
	PluginThemeApi,
	PluginThemeDefinition,
	PluginUiApi,
	PluginUiDefinition,
	PluginUnsubscribe,
} from "./api"

export type PluginApiVersion = "3"

export type PluginPermission =
	| "links:open"
	| "network:fetch"
	| "network:dns"
	| "network:ping"
	| "launcher:servers"
	| "launcher:dialogs"
	| "storage:workspace"
	| "storage:database"
	| "files:plugin"
	| "files:instance"
	| "ui:compose"
	| "events:launcher"
	| "runtime:observe"
	| "lifecycle:observe"
	| "process:observe"
	| "files:launcher-root-read"
	| "files:launcher-config-projected"
	| "network:http"
	| "network:https"
	| "network:websocket"
	| "network:tcp-client"
	| "network:udp-client"
	| "network:proxy-control"
	| "network:traffic-observe"
	| "network:traffic-intercept"
	| "network:tunnel"
	| "os:open-url"
	| "os:open-file"
	| "os:reveal-in-file-manager"
	| "os:clipboard-read"
	| "os:clipboard-write"
	| "os:spawn-external"
	| "os:browser-session"
	| "auth:oauth"
	| "auth:token-use"
	| "secrets:store"
	| "secrets:use"
	| "ai:invoke-cloud"
	| "ai:invoke-local"
	| "downloads:manage"
	| "updates:manage"
	| "dependencies:resolve"
	| "integrity:scan"
	| "metrics:emit"
	| "logs:write"
	| "presence:discord"
	| "scheduler:jobs"
	| "commands:register"
	| "shortcuts:register"
	| "instances:read"
	| "instances:mutate"
	| "mods:read"
	| "mods:install"
	| "packs:manage"
	| "accounts:projected-read"
	| "backup:export"
	| "backup:restore"
	| "sync:cloud"
	| "ipc:sidecar"
	| "policy:read"
	| "crypto:sign"
	| "crypto:verify"

export type JsonPrimitive = string | number | boolean | null
export type JsonValue = JsonPrimitive | readonly JsonValue[] | { readonly [key: string]: JsonValue }

export interface PluginMetadata {
	id: string
	name: string
	version: string
	author?: string
	description?: string
	api_version: PluginApiVersion
	apiVersion?: PluginApiVersion
}

export interface PluginManifestV3 {
	id: string
	name: string
	version: string
	api_version: PluginApiVersion
	entrypoint: string
	permissions: readonly PluginPermission[]
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
	update_channel?: string
	capabilities?: JsonValue
	consent_scopes?: JsonValue
	extension_points?: JsonValue
	dependency_graph?: JsonValue
	compatibility_matrix?: JsonValue
	host_requirements?: JsonValue
	integrity: { files: Readonly<Record<string, string>> }
	signature?: {
		algorithm: "ed25519"
		key_id: string
		public_key: string
		value: string
	}
}

export interface RuntimeInfo {
	plugin_id: string
	api_version: PluginApiVersion | string
	permissions: string[]
	capabilities: string[]
	sandbox_profile: string
	package_sha256: string
}

export interface RuntimeQuotaUsage {
  hostCalls?: { calls: number; bytes: number; callLimit: number; byteLimit: number; windowSeconds: number; enforced: boolean }
  operations?: { calls: number; bytes: number; callLimit: number; byteLimit: number; windowSeconds: number; enforced: boolean }
	plugin_id: string
	totals: {
		bytes: number
		files: number
	}
	directories: Record<string, {
		bytes: number
		files: number
	}>
}

export type FileScope = "plugin" | "instance" | "launcher-root-readonly"

export interface FileInfo {
	path: string
	size: number
	modified_at: number
}

export interface RuntimeApi {
	getInfo(): Promise<RuntimeInfo>
	getCapabilities(): Promise<string[]>
	getQuotaUsage(): Promise<RuntimeQuotaUsage>
	assertCapability(name: string): Promise<void>
}

export interface PermissionsSummary {
	plugin_id: string
	requested_permissions: string[]
	granted_permissions: string[]
	execution_allowed: boolean
	api_version: PluginApiVersion | string
}

export interface PermissionsApi {
	get(): Promise<PermissionsSummary>
	/** can only revoke existing grants; new grants require a user action in Plugin Center */
	set(permissions: string[]): Promise<PermissionsSummary>
}

export interface FilesApi {
	list(scope: FileScope, directory?: string): Promise<readonly FileInfo[]>
	readText(scope: FileScope, path: string): Promise<string>
	writeText(scope: Extract<FileScope, "plugin" | "instance">, path: string, content: string): Promise<void>
	remove(scope: Extract<FileScope, "plugin" | "instance">, path: string): Promise<boolean>
}

export interface ConfigApi {
	get<T extends JsonValue>(key: string, fallback?: T): Promise<T | undefined>
	set(key: string, value: JsonValue): Promise<void>
	remove(key: string): Promise<boolean>
	readProjected<T = Readonly<Record<string, JsonValue>>>(schemaId: string): Promise<{
		schema_id: string
		data: T
		removed_fields: string[]
	}>
}

export interface HttpRequest {
	url: string
	method?: "GET" | "POST" | "PUT" | "DELETE"
	headers?: Record<string, string>
	body?: string | JsonValue
	timeout_ms?: number
}

export interface HttpResponse {
	status: number
	url: string
	headers: Record<string, string>
	body_text: string
	body_json: unknown | null
	timeout_ms: number
}

export interface HttpApi {
	request(request: HttpRequest): Promise<HttpResponse>
}

export interface InstanceInfo {
	name: string
	path: string
	version_id: string
	minecraft_version: string
	loader: string
	display_name: string
	description: string
	group: string
	icon: string
	banner: string
	played_seconds: number
	kind: string
	server_core: string
	server_core_version: string
	server_port: number
	server_eula: boolean
	running: boolean
}

export interface ProcessMetricsResponse {
	instance: InstanceInfo
	running: boolean
	pid: number
	started_at: number
	metrics: Record<string, unknown>
}

export interface InstancesApi {
	list(): Promise<InstanceInfo[]>
	getProcessMetrics(instanceName: string): Promise<ProcessMetricsResponse>
}

export interface OsApi {
	openUrl(url: string): Promise<{ opened: boolean }>
}

export interface ProxyRouteMatch {
	hostEquals?: string[]
	hostSuffix?: string[]
	portIn?: number[]
	protocol?: Array<"tcp" | "udp" | "quic">
	sniEquals?: string[]
	processTag?: string[]
	instanceId?: string[]
}

export interface ProxyRouteAction {
	type: "DIRECT" | "PROXY" | "RELAY" | "DROP" | "MIRROR"
	sessionId?: string
	rewrite?: {
		targetHost?: string
		targetPort?: number
		overrideSni?: string
		addHeaders?: Record<string, string>
		removeHeaders?: string[]
	}
}

export interface ProxyRoutePolicy {
	id: string
	priority: number
	match: ProxyRouteMatch
	action: ProxyRouteAction
	failover?: {
		strategy: "none" | "next-route" | "direct-fallback" | "retry-same"
		maxAttempts?: number
	}
}

export interface ProxyApi {
	listRoutes(): Promise<ProxyRoutePolicy[]>
	saveRoutes(routes: ProxyRoutePolicy[]): Promise<ProxyRoutePolicy[]>
	addRoute(route: Omit<ProxyRoutePolicy, "id"> & { id?: string }): Promise<{ route: ProxyRoutePolicy; routes: ProxyRoutePolicy[] }>
	removeRoute(routeId: string): Promise<{ removed: boolean; routes: ProxyRoutePolicy[] }>
}

export interface PluginApi {
	readonly metadata: Readonly<PluginMetadata>
	readonly manifest: Readonly<PluginManifestV3>
	readonly ui: PluginUiApi
	readonly i18n: PluginI18nApi
	readonly theme: PluginThemeApi
	readonly actions: PluginActionApi
	readonly events: {
    listListeners(): readonly { event: string; count: number; listenerIds: readonly string[] }[]
		on<K extends PluginEventName>(event: K, listener: (payload: PluginEventPayloadMap[K]) => void): PluginUnsubscribe
	}
	readonly storage: PluginKeyValueStore
	readonly database: PluginDatabaseApi
	readonly network: PluginNetworkApi
	readonly launcher: PluginLauncherApi
	readonly notifications: PluginNotificationApi
	readonly commands: PluginCommandApi
	readonly shortcuts: PluginShortcutApi
	readonly runtime: RuntimeApi
	readonly permissions: PermissionsApi
	readonly files: FilesApi
	readonly config: ConfigApi
	readonly http: HttpApi
	readonly instances: InstancesApi
	readonly os: OsApi
	readonly proxy: ProxyApi
}

export interface TrialPlugin {
	activate(api: PluginApi): void | (() => void | Promise<void>) | Promise<void | (() => void | Promise<void>)>
}
