<script setup lang="ts">
import { computed, ref } from 'vue'
import GitHubPluginCatalog from './GitHubPluginCatalog.vue'
import Icon from './Icon.vue'
import Collapsible from './Collapsible.vue'
import PluginViewRenderer from './PluginViewRenderer.vue'
import { permissionDescriptions, permissionLabels } from '../services/launcherExtensions'
import type { PluginManifest, PluginPermission } from '../types'
import type { PluginCommandDefinition, PluginJsonValue, PluginShortcutDefinition, PluginUiView } from '../../../plugins/api'

type RuntimeView = { pluginId: string; pluginName: string; view: PluginUiView }
type RuntimeCommand = { pluginId: string; pluginName: string; definition: PluginCommandDefinition }
type RuntimeShortcut = { pluginId: string; pluginName: string; definition: PluginShortcutDefinition }

const props = defineProps<{
  plugins: PluginManifest[]
  runtimeViews: RuntimeView[]
  runtimeCommands: RuntimeCommand[]
  runtimeShortcuts: RuntimeShortcut[]
  githubInstallBusy: boolean
  active: boolean
}>()
const emit = defineEmits<{
  toggle: [plugin: PluginManifest]
  installArchive: [payload: { name: string; data: string }]
  uninstall: [plugin: PluginManifest]
  installGithub: [payload: { owner: string; repo: string; replace: boolean }]
  updateGrants: [payload: { pluginId: string; permissions: PluginPermission[] }]
  allowLocalExecution: [payload: { pluginId: string; allowed: boolean }]
  pinPublisher: [plugin: PluginManifest]
  unpinPublisher: [plugin: PluginManifest]
  setRevocationAuthority: [payload: { plugin: PluginManifest; enabled: boolean }]
  importRevocations: [documentJson: string]
  revoke: [plugin: PluginManifest]
  unrevoke: [plugin: PluginManifest]
  runViewAction: [payload: { pluginId: string; action: string; input: PluginJsonValue }]
  saveViewConfig: [payload: { pluginId: string; key: string; value: PluginJsonValue }]
  openViewLink: [payload: { pluginId: string; url: string }]
  runCommand: [payload: { pluginId: string; commandId: string }]
  toast: [message: string]
}>()

const archiveInput = ref<HTMLInputElement>()
const revocationInput = ref<HTMLInputElement>()
const installedPlugins = computed(() => props.plugins.filter((plugin) => plugin.installed))
const canImportRevocations = computed(() => installedPlugins.value.some((plugin) =>
  plugin.publisher_pinned && plugin.publisher_trusted && plugin.publisher_revocation_authority,
))

function commandsFor(pluginId: string) {
  return props.runtimeCommands.filter((entry) => entry.pluginId === pluginId)
}

function shortcutsFor(pluginId: string) {
  return props.runtimeShortcuts.filter((entry) => entry.pluginId === pluginId)
}

function updateGrant(plugin: PluginManifest, permission: PluginPermission, event: Event) {
  const checked = (event.currentTarget as HTMLInputElement).checked
  const grants = new Set(plugin.granted_permissions || [])
  if (checked) grants.add(permission)
  else grants.delete(permission)
  emit('updateGrants', { pluginId: plugin.id, permissions: [...grants] })
}

function allowLocalExecution(plugin: PluginManifest, event: Event) {
  emit('allowLocalExecution', {
    pluginId: plugin.id,
    allowed: (event.currentTarget as HTMLInputElement).checked,
  })
}

function setRevocationAuthority(plugin: PluginManifest, event: Event) {
  emit('setRevocationAuthority', {
    plugin,
    enabled: (event.currentTarget as HTMLInputElement).checked,
  })
}

function executionStatus(plugin: PluginManifest): string {
  if (plugin.package_valid === false) return plugin.package_error || 'Пакет не прошёл проверку.'
  if (plugin.revoked) return `Плагин заблокирован${plugin.revocation_reason ? `: ${plugin.revocation_reason}` : ' на этом устройстве.'}`
  if (plugin.execution_allowed) return 'QuickJS runtime разрешён для этого устройства.'
  if (plugin.signed && !plugin.publisher_trusted) return 'Чтобы запускать код, сначала закрепите ключ издателя.'
  if (!plugin.signed && !plugin.local_untrusted_execution_allowed) return 'Нужно отдельно разрешить локальный запуск неподписанного кода.'
  if (!plugin.enabled) return 'Плагин выключен.'
  return 'Runtime сейчас недоступен.'
}

function pickArchive() {
  archiveInput.value?.click()
}

function pickRevocationDocument() {
  if (canImportRevocations.value) revocationInput.value?.click()
}

async function importArchive(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file) return
  if (file.size > 64 * 1024 * 1024) {
    emit('toast', 'Архив плагина больше 64 MiB')
    return
  }
  try {
    const bytes = new Uint8Array(await file.arrayBuffer())
    let binary = ''
    for (let offset = 0; offset < bytes.length; offset += 0x8000) binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000))
    emit('installArchive', { name: file.name, data: btoa(binary) })
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : 'Не удалось прочитать ZIP-архив')
  }
}

async function importRevocationDocument(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file) return
  if (file.size > 1024 * 1024) {
    emit('toast', 'Список отзыва больше 1 MiB')
    return
  }
  try {
    emit('importRevocations', await file.text())
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : 'Не удалось прочитать список отзыва')
  }
}
</script>

<template>
  <section class="plugin-center">
    <div class="section-intro plugin-catalog-intro">
      <div>
        <span class="eyebrow">Каталог · плагины лаунчера</span>
        <h2>Плагины лаунчера</h2>
        <p>Импортируйте архивы, проверяйте разрешения и отдельно решайте, каким издателям доверять. Списки отзыва принимаются только от явно назначенного ключа.</p>
      </div>
      <span class="count-badge">{{ installedPlugins.length }} на устройстве</span>
    </div>

    <GitHubPluginCatalog :active="props.active" :plugins="props.plugins" :install-busy="props.githubInstallBusy" @install="emit('installGithub', $event)">
      <template #import-control>
        <input ref="archiveInput" class="workspace-file-input" type="file" accept=".zip,application/zip" @change="importArchive" />
        <button class="button button--outline" type="button" :disabled="props.githubInstallBusy" @click="pickArchive"><Icon name="upload" :size="15" />Импортировать ZIP</button>
        <input ref="revocationInput" class="workspace-file-input" type="file" accept=".json,application/json" @change="importRevocationDocument" />
        <button class="button button--outline" type="button" :disabled="!canImportRevocations" v-tooltip="'Требуется закреплённый ключ с полномочием на списки отзыва'" @click="pickRevocationDocument"><Icon name="upload" :size="15" />Импортировать список отзыва</button>
      </template>
    </GitHubPluginCatalog>

    <section v-if="props.runtimeViews.length" class="panel plugin-runtime-panel" aria-labelledby="plugin-runtime-title">
      <div class="panel-heading plugin-library-heading">
        <div><span class="eyebrow">Изолированный QuickJS</span><h2 id="plugin-runtime-title">Интерфейс активных плагинов</h2><p>Эти страницы построены из проверенных декларативных компонентов, без HTML-кода плагина.</p></div>
        <span class="count-badge">{{ props.runtimeViews.length }}</span>
      </div>
      <article v-for="entry in props.runtimeViews" :key="`${entry.pluginId}:${entry.view.id}`" class="plugin-runtime-view">
        <div class="plugin-runtime-view__source">{{ entry.pluginName }} <span>· {{ entry.view.slot }}</span></div>
        <PluginViewRenderer
          :view="entry.view"
          @action="(payload) => emit('runViewAction', { pluginId: entry.pluginId, ...payload })"
          @config="(payload) => emit('saveViewConfig', { pluginId: entry.pluginId, key: payload.key, value: payload.value })"
          @open-link="(url) => emit('openViewLink', { pluginId: entry.pluginId, url })"
        />
      </article>
    </section>

    <section v-if="installedPlugins.length" class="panel plugin-library-panel" aria-labelledby="installed-plugin-title">
      <div class="panel-heading plugin-library-heading">
          <div><span class="eyebrow">Локальная библиотека</span><h2 id="installed-plugin-title">Установленные плагины</h2><p>API v1 остаётся статическим. API v2/v3 запускаются только после проверки доверия и выданных grants.</p></div>
        <span class="count-badge">{{ installedPlugins.length }}</span>
      </div>
      <div class="plugin-installed-list">
        <article v-for="plugin in installedPlugins" :key="plugin.id" class="plugin-installed-card">
          <div class="plugin-installed-row">
            <span class="plugin-installed-icon"><Icon :name="plugin.icon || 'plug'" :size="19" /></span>
            <div class="plugin-installed-copy">
              <div class="plugin-installed-title"><strong>{{ plugin.name }}</strong><span>v{{ plugin.version }}</span></div>
              <p>{{ plugin.description || plugin.id }}</p>
              <small>{{ ['2', '3'].includes(String(plugin.api_version || '')) ? executionStatus(plugin) : plugin.sidebar_tab?.label ? `Статическая вкладка: ${plugin.sidebar_tab.label}` : 'Архивный плагин · код не запускается' }}</small>
            </div>
            <span class="plugin-status" :class="plugin.enabled ? 'plugin-status--enabled' : 'plugin-status--disabled'"><i />{{ plugin.enabled ? 'Включён' : 'Выключен' }}</span>
            <button class="button button--small button--outline" type="button" @click="emit('toggle', plugin)">{{ plugin.enabled ? 'Выключить' : 'Включить' }}</button>
            <button class="icon-button" type="button" :aria-label="`Удалить ${plugin.name}`" @click="emit('uninstall', plugin)"><Icon name="trash" :size="15" /></button>
          </div>
          <div v-if="commandsFor(plugin.id).length || shortcutsFor(plugin.id).length" class="plugin-registered-tools">
            <button
              v-for="command in commandsFor(plugin.id)"
              :key="`${plugin.id}:command:${command.definition.id}`"
              class="button button--small button--outline"
              type="button"
              v-tooltip="command.definition.description || command.definition.label"
              @click="emit('runCommand', { pluginId: plugin.id, commandId: command.definition.id })"
            >{{ command.definition.label }}</button>
            <span v-for="shortcut in shortcutsFor(plugin.id)" :key="`${plugin.id}:shortcut:${shortcut.definition.id}`" class="plugin-shortcut-hint">
              {{ shortcut.definition.label }} <kbd>{{ shortcut.definition.default_key }}</kbd>
            </span>
          </div>
          <Collapsible v-if="['2', '3'].includes(String(plugin.api_version || ''))" class="plugin-security" title="Доверие и разрешения" :hint="`${plugin.granted_permissions?.length || 0} / ${plugin.requested_permissions?.length || plugin.permissions.length} выдано`">
            <div class="plugin-security__body">
              <p class="plugin-security__status">{{ executionStatus(plugin) }}</p>
              <div v-if="plugin.requested_permissions?.length" class="plugin-security__permissions">
                <label v-for="permission in plugin.requested_permissions" :key="permission" class="plugin-security__permission">
                  <input
                    type="checkbox"
                    :checked="plugin.granted_permissions?.includes(permission) || false"
                    :disabled="plugin.package_valid === false"
                    @change="updateGrant(plugin, permission, $event)"
                  />
                  <span><strong>{{ permissionLabels[permission] || permission }}</strong><small>{{ permissionDescriptions[permission] || permission }}</small></span>
                </label>
              </div>
              <label v-if="plugin.package_valid && !plugin.signed" class="plugin-security__local">
                <input type="checkbox" :checked="plugin.local_untrusted_execution_allowed || false" @change="allowLocalExecution(plugin, $event)" />
                <span><strong>Разрешить локальный запуск</strong><small>Согласие привязано к текущему хешу пакета и сбросится после его изменения.</small></span>
              </label>
              <label v-if="plugin.package_valid && plugin.signed && plugin.publisher_key_id && plugin.publisher_pinned" class="plugin-security__local">
                <input type="checkbox" :checked="plugin.publisher_revocation_authority || false" @change="setRevocationAuthority(plugin, $event)" />
                <span><strong>Разрешить ключу подписывать списки отзыва</strong><small>Подписанные документы смогут блокировать указанные плагины и издателей; каждый импорт проверяет подпись и новую последовательность.</small></span>
              </label>
              <div class="plugin-security__actions">
                <button v-if="plugin.package_valid && plugin.signed && plugin.publisher_key_id && !plugin.publisher_pinned" class="button button--small button--outline" type="button" @click="emit('pinPublisher', plugin)">Закрепить ключ издателя</button>
                <button v-if="plugin.publisher_pinned" class="button button--small button--quiet" type="button" @click="emit('unpinPublisher', plugin)">Отозвать доверие к издателю</button>
                <button v-if="plugin.package_valid && !plugin.revoked" class="button button--small button--quiet" type="button" @click="emit('revoke', plugin)">Заблокировать локально</button>
                <button v-if="plugin.revoked" class="button button--small button--quiet" type="button" @click="emit('unrevoke', plugin)">Снять локальную блокировку</button>
              </div>
            </div>
          </Collapsible>
        </article>
      </div>
    </section>
  </section>
</template>

<style scoped>
.plugin-center { display: grid; min-width: 0; gap: 14px; }
.plugin-catalog-intro { margin-bottom: 0; }
.plugin-library-panel { display: grid; gap: 14px; padding: 20px; }
.plugin-runtime-panel { display: grid; gap: 14px; padding: 20px; }
.plugin-library-heading { align-items: center; margin: 0; }
.plugin-library-heading h2 { margin-top: 5px; font-size: 19px; }
.plugin-library-heading p { margin: 5px 0 0; color: var(--muted); font-size: 10px; line-height: 1.5; }
.plugin-installed-list { display: grid; gap: 8px; }
.plugin-installed-card { display: grid; min-width: 0; gap: 8px; padding: 9px; border: 1px solid var(--line); border-radius: calc(var(--radius-control, 10px) * 1.2); background: var(--panel-soft); }
.plugin-installed-row { display: flex; min-width: 0; align-items: center; gap: 12px; padding: 11px 12px; border: 1px solid var(--line); border-radius: calc(var(--radius-control, 10px) * 1.1); background: var(--panel-soft); }
.plugin-installed-icon { display: grid; width: 38px; height: 38px; flex: 0 0 38px; place-items: center; border: 1px solid color-mix(in srgb, var(--accent) 28%, var(--line)); border-radius: var(--radius-control, 10px); color: var(--accent); background: color-mix(in srgb, var(--accent) 10%, var(--panel)); }
.plugin-installed-copy { min-width: 0; flex: 1; }
.plugin-installed-title { display: flex; min-width: 0; align-items: baseline; flex-wrap: wrap; gap: 6px; }
.plugin-installed-title strong { overflow: hidden; font-size: 11px; text-overflow: ellipsis; white-space: nowrap; }
.plugin-installed-title span, .plugin-installed-copy small { color: var(--muted); font-size: 9px; }
.plugin-installed-copy p { overflow: hidden; margin: 3px 0; color: var(--muted); font-size: 9px; text-overflow: ellipsis; white-space: nowrap; }
.plugin-status { display: inline-flex; flex: 0 0 auto; align-items: center; gap: 6px; color: var(--muted); font-size: 9px; }
.plugin-status i { width: 7px; height: 7px; border-radius: 50%; background: var(--muted); }
.plugin-status--enabled { color: var(--green); }
.plugin-status--enabled i { background: var(--green); }
.plugin-status--disabled i { background: var(--warning); }
.plugin-registered-tools { display: flex; min-width: 0; align-items: center; flex-wrap: wrap; gap: 7px; padding: 0 2px 2px; }
.plugin-shortcut-hint { display: inline-flex; align-items: center; gap: 6px; color: var(--muted); font-size: 9px; }
.plugin-shortcut-hint kbd { padding: 3px 6px; border: 1px solid var(--line); border-radius: calc(var(--radius-button, 8px) * .625); color: var(--text); background: var(--panel); font: inherit; }
.plugin-runtime-view { display: grid; min-width: 0; gap: 12px; padding: 15px; border: 1px solid var(--line); border-radius: calc(var(--radius-control, 10px) * 1.2); background: var(--panel-soft); }
.plugin-runtime-view__source { color: var(--quiet); font-size: 11px; font-weight: 600; }
.plugin-runtime-view__source span { color: var(--muted); font-weight: 400; }
.plugin-security { min-width: 0; background: var(--panel); }
.plugin-security__body { display: grid; gap: 12px; padding-top: 4px; }
.plugin-security__status { margin: 0; color: var(--muted); font-size: 10px; line-height: 1.5; overflow-wrap: anywhere; }
.plugin-security__permissions { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 240px), 1fr)); gap: 7px; }
.plugin-security__permission,
.plugin-security__local { display: flex; min-width: 0; align-items: flex-start; gap: 8px; padding: 8px; border: 1px solid var(--line); border-radius: var(--radius-button, 8px); color: var(--quiet); cursor: pointer; }
.plugin-security__permission input,
.plugin-security__local input { flex: 0 0 auto; margin-top: 2px; accent-color: var(--accent); }
.plugin-security__permission span,
.plugin-security__local span { display: grid; min-width: 0; gap: 3px; }
.plugin-security__permission strong,
.plugin-security__local strong { color: var(--text); font-size: 10px; font-weight: 600; }
.plugin-security__permission small,
.plugin-security__local small { color: var(--muted); font-size: 9px; line-height: 1.45; overflow-wrap: anywhere; }
.plugin-security__actions { display: flex; flex-wrap: wrap; gap: 7px; }
@media (max-width: 760px) {
  .plugin-catalog-intro { margin-bottom: 0; }
  .plugin-library-heading { align-items: flex-start; }
  .plugin-installed-row { flex-wrap: wrap; }
  .plugin-installed-copy { flex-basis: calc(100% - 54px); }
  .plugin-status { margin-left: 50px; }
  .plugin-installed-row > .button { flex: 1; }
}
</style>
