/** type-only contract for Nivora Launcher v2 plugins; it ships no runtime code */

export type PluginApiVersion = "2";

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
  | "events:launcher";

export type PluginJsonPrimitive = string | number | boolean | null;
export type PluginJsonValue =
  | PluginJsonPrimitive
  | readonly PluginJsonValue[]
  | { readonly [key: string]: PluginJsonValue };

export interface PluginIntegrity {
  files: Readonly<Record<string, string>>;
}

export interface PluginSignature {
  algorithm: "ed25519";
  key_id: string;
  public_key: string;
  value: string;
}

export type PluginUiSlot =
  | "sidebar_tab"
  | "instance_tab"
  | "settings_section"
  | "home_widget";

export type PluginUiTone = "default" | "muted" | "success" | "warning" | "danger";

export interface PluginUiOption {
  value: string;
  label: string;
  description?: string;
  disabled?: boolean;
}

export interface PluginUiListItem {
  id: string;
  label: string;
  detail?: string;
  badge?: string;
  action?: string;
}

export type PluginUiNode =
  | { type: "tabs"; id: string; tabs: readonly { id: string; label: string; content: PluginUiNode }[] }
  | { type: "accordion"; id: string; sections: readonly { id: string; label: string; content: PluginUiNode }[] }
  | { type: "table"; id: string; columns: readonly { key: string; label: string }[]; rows: readonly Record<string, string | number>[] }
  | { type: "chart"; id: string; label?: string; values: readonly number[] }
  | { type: "form"; id: string; fields: readonly PluginUiNode[]; submitAction: string }
  | { type: "include"; id: string; fragment: string }
  | { type: "text"; id?: string; text: string; tone?: PluginUiTone }
  | { type: "badge"; id?: string; label: string; tone?: PluginUiTone }
  | { type: "button"; id: string; label: string; action: string; disabled?: boolean; tone?: PluginUiTone }
  | {
      type: "input";
      id: string;
      label: string;
      value?: string;
      placeholder?: string;
      config_key?: string;
      action?: string;
      onChange?: string;
    }
  | {
      type: "textarea";
      id: string;
      label: string;
      value?: string;
      placeholder?: string;
      rows?: number;
      config_key?: string;
      action?: string;
      onChange?: string;
    }
  | {
      type: "number";
      id: string;
      label: string;
      value?: number;
      min?: number;
      max?: number;
      step?: number;
      unit?: string;
      config_key?: string;
      action?: string;
      onChange?: string;
    }
  | {
      type: "slider";
      id: string;
      label: string;
      value?: number;
      min: number;
      max: number;
      step?: number;
      unit?: string;
      config_key?: string;
      action?: string;
      onChange?: string;
    }
  | {
      type: "color";
      id: string;
      label: string;
      value?: string;
      config_key?: string;
      onChange?: string;
    }
  | {
      type: "datetime";
      id: string;
      label: string;
      value?: string;
      config_key?: string;
      action?: string;
      onChange?: string;
    }
  | {
      type: "select";
      id: string;
      label: string;
      options: readonly PluginUiOption[];
      value?: string;
      config_key?: string;
      action?: string;
      onChange?: string;
    }
  | { type: "checkbox"; id: string; label: string; checked?: boolean; config_key?: string; action?: string; onChange?: string }
  | { type: "switch"; id: string; label: string; checked?: boolean; hint?: string; config_key?: string; action?: string; onChange?: string }
  | { type: "progress"; id?: string; value: number; max?: number; label?: string }
  | { type: "metric"; id?: string; label: string; value: string; unit?: string; tone?: PluginUiTone }
  | { type: "alert"; id?: string; title?: string; text: string; tone?: PluginUiTone }
  | { type: "code"; id?: string; text: string }
  | { type: "image"; id?: string; url: string; alt?: string; height?: number }
  | { type: "list"; id?: string; items: readonly (string | PluginUiListItem)[] }
  | { type: "link"; id?: string; label: string; url: string; action?: string }
  | { type: "divider"; id?: string }
  | {
      type: "group";
      id?: string;
      label?: string;
      direction?: "vertical" | "horizontal";
      children: readonly PluginUiNode[];
    }
  | {
      type: "card";
      id?: string;
      label?: string;
      tone?: PluginUiTone;
      children: readonly PluginUiNode[];
    }
  | {
      type: "grid";
      id?: string;
      columns?: number;
      children: readonly PluginUiNode[];
    };

export interface PluginUiView {
  id: string;
  slot: PluginUiSlot;
  title: string;
  description?: string;
  icon?: string;
  order?: number;
  root: PluginUiNode;
}

export interface PluginUiDefinition {
  views: readonly PluginUiView[];
}

export interface PluginI18nDefinition {
  default_locale: string;
  messages: Readonly<Record<string, Readonly<Record<string, string>>>>;
}

export interface PluginThemeDefinition {
  name?: string;
  colors?: Readonly<Record<string, string>>;
  tokens?: Readonly<Record<string, string | number>>;
}

export interface PluginActionDefinition {
  id: string;
  label: string;
  description?: string;
  icon?: string;
  confirmation?: string;
}

export interface PluginCommandDefinition {
  id: string;
  label: string;
  description?: string;
  arguments?: readonly PluginCommandArgument[];
}

export interface PluginCommandArgument {
  name: string;
  description?: string;
  required?: boolean;
  choices?: readonly string[];
}

export interface PluginShortcutDefinition {
  id: string;
  label: string;
  default_key: string;
  description?: string;
}

export interface PluginConfigField {
  type: "string" | "number" | "boolean" | "select";
  label: string;
  description?: string;
  default?: string | number | boolean;
  minimum?: number;
  maximum?: number;
  options?: readonly PluginUiOption[];
}

export interface PluginManifestV2 {
  id: string;
  name: string;
  version: string;
  api_version: PluginApiVersion;
  entrypoint: string;
  author?: string;
  description?: string;
  icon?: string;
  category?: string;
  permissions: readonly PluginPermission[];
  network_hosts?: readonly string[];
  config?: Readonly<Record<string, PluginConfigField>>;
  ui?: PluginUiDefinition;
  i18n?: PluginI18nDefinition;
  theme?: PluginThemeDefinition;
  actions?: readonly PluginActionDefinition[];
  events?: readonly PluginEventName[];
  commands?: readonly PluginCommandDefinition[];
  shortcuts?: readonly PluginShortcutDefinition[];
  integrity?: PluginIntegrity;
  signature?: PluginSignature;
}

export interface PluginMetadata {
  id: string;
  name: string;
  version: string;
  author: string;
  description: string;
  api_version: PluginApiVersion;
}

export interface PluginInstanceRef {
  id: string;
  name: string;
  version?: string;
  loader?: string;
}

export interface PluginEventPayloadMap {
  "launcher.ready": { version: string };
  "instance.selected": { instance: PluginInstanceRef | null };
  "instance.beforeLaunch": { instance: PluginInstanceRef; operation_id?: string };
  "instance.afterExit": { instance: PluginInstanceRef; exit_code: number | null };
  "instance.crash": { instance: PluginInstanceRef; message?: string; exit_code?: number | null };
  "log.line": { instance?: PluginInstanceRef; level: "debug" | "info" | "warning" | "error"; line: string };
  "mods.beforeInstall": { instance: PluginInstanceRef; project_id: string; source: string; version?: string };
  "snapshot.created": {
    instance: PluginInstanceRef;
    snapshot_id: string;
    category: string;
    created_at: number;
  };
  "settings.changed": { keys: readonly string[] };
  "account.changed": { account_id: string | null };
  "plugin.enabled": { plugin_id: string };
}

export type PluginEventName = keyof PluginEventPayloadMap;
export type PluginEventListener<K extends PluginEventName> = (payload: PluginEventPayloadMap[K]) => void;
export type PluginUnsubscribe = () => void;
export type PluginDisposer = () => void | Promise<void>;
export type PluginActionHandler = (input?: PluginJsonValue) => void | PluginJsonValue | Promise<void | PluginJsonValue>;
export type PluginCommandHandler = (arguments_: Readonly<Record<string, string>>) => void | Promise<void>;

export interface PluginKeyValueStore {
  get<T extends PluginJsonValue>(key: string, fallback?: T): Promise<T | undefined>;
  set(key: string, value: PluginJsonValue): Promise<void>;
  remove(key: string): Promise<boolean>;
}

export interface PluginConfigStore {
  get<T extends PluginJsonValue>(key: string, fallback?: T): Promise<T | undefined>;
  set(key: string, value: PluginJsonValue): Promise<void>;
  remove(key: string): Promise<boolean>;
}

export interface PluginFileInfo {
  path: string;
  size: number;
  modified_at: number;
}

export interface PluginFilesApi {
  list(scope: "plugin" | "instance", directory?: string): Promise<readonly PluginFileInfo[]>;
  readText(scope: "plugin" | "instance", path: string): Promise<string>;
  writeText(scope: "plugin" | "instance", path: string, content: string): Promise<void>;
  remove(scope: "plugin" | "instance", path: string): Promise<boolean>;
}

export interface PluginDatabaseApi {
  query<Row = Readonly<Record<string, PluginJsonValue>>>(
    sql: string,
    parameters?: readonly PluginJsonValue[],
  ): Promise<readonly Row[]>;
  execute(sql: string, parameters?: readonly PluginJsonValue[]): Promise<{ changes: number }>;
}

export interface PluginNetworkRequest {
  method?: "GET" | "POST" | "PUT" | "DELETE";
  headers?: Readonly<Record<string, string>>;
  body?: PluginJsonValue;
  timeout_ms?: number;
}

export interface PluginNetworkAddress {
  family: "IPv4" | "IPv6" | string;
  address: string;
}

export interface PluginReachability {
  host: string;
  port: number;
  reachable: boolean;
  latency_ms: number;
  error: string;
}

export interface PluginNetworkApi {
  fetchJson<T = PluginJsonValue>(url: string, request?: PluginNetworkRequest): Promise<T>;
  resolve(host: string, port?: number): Promise<{ host: string; port: number | null; addresses: readonly PluginNetworkAddress[] }>;
  ping(host: string, port?: number, timeout_ms?: number): Promise<PluginReachability>;
  checkPort(host: string, port?: number, timeout_ms?: number): Promise<PluginReachability>;
}

export interface PluginServer {
  id: string;
  name: string;
  address: string;
  port: number;
  description: string;
  tags: readonly string[];
}

export interface PluginServerInput {
  id?: string;
  name: string;
  address: string;
  port?: number;
  description?: string;
  tags?: readonly string[];
}

export interface PluginServerListApi {
  list(): Promise<readonly PluginServer[]>;
  save(servers: readonly PluginServerInput[]): Promise<readonly PluginServer[]>;
  add(server: PluginServerInput): Promise<{ server: PluginServer; servers: readonly PluginServer[] }>;
  remove(id: string): Promise<{ removed: boolean; servers: readonly PluginServer[] }>;
}

export interface PluginDialogChoice {
  value: string;
  label: string;
  description?: string;
}

export interface PluginDialogsApi {
  alert(options: { title: string; message: string }): Promise<void>;
  confirm(options: { title: string; message: string; confirm_label?: string }): Promise<boolean>;
  prompt(options: { title: string; message: string; initial_value?: string }): Promise<string | null>;
  choose(options: { title: string; choices: readonly PluginDialogChoice[] }): Promise<string | null>;
}

export interface PluginNotificationApi {
  show(options: { title: string; message: string; tone?: PluginUiTone; id?: string }): Promise<string>;
  dismiss(id: string): Promise<boolean>;
}

export interface PluginActionApi {
  register(definition: PluginActionDefinition, handler: PluginActionHandler): PluginUnsubscribe;
  run(id: string, input?: PluginJsonValue): Promise<PluginJsonValue | void>;
}

export interface PluginCommandApi {
  register(definition: PluginCommandDefinition, handler: PluginCommandHandler): PluginUnsubscribe;
}

export interface PluginShortcutApi {
  register(definition: PluginShortcutDefinition, handler: () => void | Promise<void>): PluginUnsubscribe;
}

export interface PluginUiApi {
  registerView(view: PluginUiView): PluginUnsubscribe;
  updateView(view: PluginUiView): Promise<void>;
  removeView(id: string): Promise<boolean>;
  compose(node: PluginUiNode): Readonly<PluginUiNode>;
  registerFragment(id: string, node: PluginUiNode): PluginUnsubscribe;
  updateFragment(id: string, node: PluginUiNode): Promise<void>;
  bindData(viewId: string, nodeId: string, source: PluginDataSource): PluginUnsubscribe;
}

export interface PluginDataSource {
  type: "static" | "event" | "poll";
  event?: PluginEventName;
  intervalMs?: number;
  loader: () => Promise<PluginJsonValue>;
}

export interface PluginI18nApi {
  readonly locale: string;
  t(key: string, values?: Readonly<Record<string, string | number>>): string;
  availableLocales(): readonly string[];
}

export interface PluginThemeContext {
  readonly mode: "light" | "dark" | "glass";
  readonly accent: string;
  readonly tokens: Readonly<Record<string, string | number>>;
}

export interface PluginThemeApi {
  get(): Promise<PluginThemeContext>;
  register(definition: PluginThemeDefinition): PluginUnsubscribe;
}

export interface PluginLauncherApi {
  readonly dialogs: PluginDialogsApi;
  readonly servers: PluginServerListApi;
  openExternal(url: string): Promise<void>;
  selectedInstance(): Promise<PluginInstanceRef | null>;
  on<K extends PluginEventName>(event: K, listener: PluginEventListener<K>): PluginUnsubscribe;
}

export interface PluginApi {
  readonly metadata: Readonly<PluginMetadata>;
  readonly manifest: Readonly<PluginManifestV2>;
  readonly ui: PluginUiApi;
  readonly i18n: PluginI18nApi;
  readonly theme: PluginThemeApi;
  readonly actions: PluginActionApi;
  readonly events: {
    listListeners(): readonly { event: string; count: number; listenerIds: readonly string[] }[];
    on<K extends PluginEventName>(event: K, listener: PluginEventListener<K>): PluginUnsubscribe;
  };
  readonly storage: PluginKeyValueStore;
  readonly config: PluginConfigStore;
  readonly files: PluginFilesApi;
  readonly database: PluginDatabaseApi;
  readonly network: PluginNetworkApi;
  readonly launcher: PluginLauncherApi;
  readonly notifications: PluginNotificationApi;
  readonly commands: PluginCommandApi;
  readonly shortcuts: PluginShortcutApi;
}

export interface TrialPlugin {
  activate(api: PluginApi): void | PluginDisposer | Promise<void | PluginDisposer>;
}

export type PluginFactory = (api: PluginApi) => TrialPlugin | Promise<TrialPlugin>;
