<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import Icon from './Icon.vue'
import CustomSelect from './CustomSelect.vue'
import { bridge } from '../services/bridge'
import type { InstanceComparison, InstanceDiffBucket, InstanceDiffEntry, InstanceInfo, PortChangeItem } from '../types'

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  if (typeof error === 'string') return error
  try {
    return JSON.stringify(error)
  } catch {
    return 'Неизвестная ошибка'
  }
}

const props = defineProps<{
  instances: InstanceInfo[]
  initialSource?: string
  initialTarget?: string
}>()

const emit = defineEmits<{
  (event: 'close'): void
  (event: 'toast', message: string): void
}>()

const javaInstances = computed(() => props.instances.filter((item) => item.game_edition !== 'bedrock' && item.kind !== 'server'))
const source = ref(props.initialSource || javaInstances.value[0]?.name || '')
const target = ref(props.initialTarget || javaInstances.value.find((item) => item.name !== source.value)?.name || '')
const instanceOptions = computed(() => javaInstances.value.map((instance) => ({ value: instance.name, label: instance.display_name || instance.name })))
const loading = ref(false)
const porting = ref(false)
const error = ref('')
const comparison = ref<InstanceComparison | null>(null)
const section = ref<'mods' | 'packs' | 'configs' | 'settings'>('mods')
const selected = ref(new Set<string>())
const ready = computed(() => Boolean(source.value && target.value && source.value !== target.value))

const sectionTabs = computed(() => {
  const report = comparison.value?.report
  if (!report) return []
  return [
    { id: 'mods' as const, label: 'Моды', count: report.summary.mods_added + report.summary.mods_removed + report.summary.mods_changed },
    { id: 'packs' as const, label: 'Ресурсы и шейдеры', count: report.summary.packs_changed },
    { id: 'configs' as const, label: 'Конфиги', count: report.summary.configs_changed },
    { id: 'settings' as const, label: 'Версия и настройки', count: Object.keys(report.meta).length + report.options.changed.length },
  ]
})

const selectedItems = computed<PortChangeItem[]>(() => {
  const items: PortChangeItem[] = []
  for (const key of selected.value) {
    const [kind, path] = key.split('::', 2)
    if (kind && path) items.push({ kind: kind as PortChangeItem['kind'], path })
  }
  return items
})

function entryKey(kind: string, path: string): string {
  return `${kind}::${path}`
}

function toggleEntry(kind: string, entry: InstanceDiffEntry): void {
  const key = entryKey(kind, entry.path)
  if (selected.value.has(key)) selected.value.delete(key)
  else selected.value.add(key)
  selected.value = new Set(selected.value)
}

function selectAllVisible(): void {
  const report = comparison.value?.report
  if (!report) return
  const next = new Set(selected.value)
  const add = (kind: string, bucket: InstanceDiffBucket) => {
    for (const entry of [...bucket.added, ...bucket.changed]) next.add(entryKey(kind, entry.path))
  }
  add('mod', report.mods)
  add('resourcepack', report.resourcepacks)
  add('shaderpack', report.shaderpacks)
  add('config', report.configs)
  selected.value = next
}

function clearSelection(): void {
  selected.value = new Set()
}

function swap(): void {
  const previousSource = source.value
  source.value = target.value
  target.value = previousSource
  void load()
}

async function load(): Promise<void> {
  if (!ready.value || loading.value) return
  loading.value = true
  error.value = ''
  comparison.value = null
  selected.value = new Set()
  try {
    comparison.value = await bridge.instanceCompare(source.value, target.value)
    // обычно нужно добавить новые моды из исходного экземпляра
    const next = new Set<string>()
    for (const entry of comparison.value.report.mods.added) next.add(entryKey('mod', entry.path))
    for (const entry of comparison.value.report.configs.changed) next.add(entryKey('config', entry.path))
    selected.value = next
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    loading.value = false
  }
}

async function portSelected(): Promise<void> {
  if (!ready.value || porting.value || !selectedItems.value.length || !comparison.value) return
  porting.value = true
  error.value = ''
  try {
    const result = await bridge.instancePortChanges(source.value, target.value, selectedItems.value)
    const failed = result.failed?.length || 0
    emit('toast', failed
      ? `Перенесено ${result.copied.length}, не удалось ${failed}: ${result.failed[0].error}`
      : `Перенесено файлов: ${result.copied.length}. Цель обновлена.`)
    await load()
  } catch (cause) {
    error.value = errorMessage(cause)
  } finally {
    porting.value = false
  }
}

function formatSize(value: number): string {
  const bytes = Number(value || 0)
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} МБ`
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} КБ`
  return `${bytes} Б`
}

function instanceLabel(name: string): string {
  const instance = javaInstances.value.find((item) => item.name === name)
  return instance ? (instance.display_name || instance.name) : name
}

const totalDifferences = computed(() => {
  const report = comparison.value?.report
  if (!report) return 0
  return report.summary.mods_added + report.summary.mods_removed + report.summary.mods_changed
    + report.summary.configs_changed + report.summary.packs_changed
    + Object.keys(report.meta).length
})

onMounted(load)
</script>

<template>
  <Teleport to="body">
      <div class="dialog-scrim" role="presentation" @click.self="emit('close')">
        <section class="dialog-card compare-card" role="dialog" tabindex="-1" aria-modal="true" aria-label="Сравнение экземпляров">
          <div class="dialog-icon"><Icon name="globe" :size="18" /></div>
          <div class="dialog-copy">
            <div class="dialog-heading">
              <div>
                
                <h2>Чем отличаются сборки</h2>
              </div>
              <button class="icon-button" type="button" aria-label="Закрыть" @click="emit('close')"><Icon name="close" :size="16" /></button>
            </div>

            <div class="compare-pickers">
              <label class="field">
                <span>Исходная сборка</span>
                <CustomSelect v-model="source" :options="instanceOptions" :disabled="loading" aria-label="Исходная сборка" @update:model-value="load" />
              </label>
              <button class="icon-button" type="button" aria-label="Поменять сборки местами" :disabled="loading || !ready" @click="swap"><Icon name="refresh" :size="14" /></button>
              <label class="field">
                <span>Сравнивается с</span>
                <CustomSelect v-model="target" :options="instanceOptions" :disabled="loading" aria-label="Сравнивается с" @update:model-value="load" />
              </label>
            </div>

            <div v-if="!ready" class="compare-note"><Icon name="info" :size="14" /><span>Выберите два разных экземпляра Java Edition.</span></div>
            <div v-else-if="loading" class="mod-audit-progress"><div class="progress-track progress-track--indeterminate"><span /></div><small>Сканирую моды, конфиги и версии…</small></div>
            <div v-else-if="error" class="compare-note compare-note--danger"><Icon name="warning" :size="14" /><span>{{ error }}</span></div>

            <template v-else-if="comparison">
              <div class="compare-verdict" :class="totalDifferences ? '' : 'compare-verdict--same'">
                <Icon :name="totalDifferences ? 'list' : 'check'" :size="18" />
                <div>
                  <strong>{{ totalDifferences ? `Различий: ${totalDifferences}` : 'Сборки идентичны' }}</strong>
                  <small>
                    «{{ instanceLabel(source) }}»: {{ comparison.source_stats.mod_count }} модов · {{ comparison.source_stats.class_count.toLocaleString('ru-RU') }} классов
                    — «{{ instanceLabel(target) }}»: {{ comparison.target_stats.mod_count }} модов · {{ comparison.target_stats.class_count.toLocaleString('ru-RU') }} классов
                  </small>
                </div>
              </div>

              <div v-if="totalDifferences" class="compare-tabs" role="tablist">
                <button
                  v-for="tab in sectionTabs"
                  :key="tab.id"
                  class="compare-tab"
                  :class="{ 'compare-tab--active': section === tab.id }"
                  type="button"
                  role="tab"
                  :aria-selected="section === tab.id"
                  @click="section = tab.id"
                >{{ tab.label }}<em v-if="tab.count">{{ tab.count }}</em></button>
              </div>

              <div v-if="totalDifferences" class="compare-body">
                <template v-if="section === 'mods'">
                  <div v-for="group in [['added', 'Есть только в исходной'], ['changed', 'Версии разошлись'], ['removed', 'Есть только во второй']] as const" :key="group[0]">
                    <template v-if="comparison.report.mods[group[0]].length">
                      <h4 class="compare-group">{{ group[1] }}<small>{{ comparison.report.mods[group[0]].length }}</small></h4>
                      <div class="compare-list">
                        <label
                          v-for="entry in comparison.report.mods[group[0]]"
                          :key="entry.path + (entry.from_path || '')"
                          class="compare-row"
                          :class="{ 'compare-row--selected': selected.has(entryKey('mod', entry.path)) }"
                        >
                          <input
                            v-if="group[0] !== 'removed'"
                            type="checkbox"
                            :checked="selected.has(entryKey('mod', entry.path))"
                            @change="toggleEntry('mod', entry)"
                          >
                          <Icon v-else name="close" :size="12" />
                          <div class="compare-row-copy">
                            <span class="compare-row-name">{{ entry.name }}<em v-if="entry.mod_id && entry.mod_id !== entry.name">{{ entry.mod_id }}</em></span>
                            <small>
                              <template v-if="group[0] === 'changed'">
                                <template v-if="entry.from_version || entry.to_version">{{ entry.from_version || '—' }} → {{ entry.to_version || '—' }}</template>
                                <template v-if="entry.enabled_changed"> · вкл/выкл</template>
                              </template>
                              <template v-else>{{ entry.loader || 'мод' }}{{ entry.version ? ` · ${entry.version}` : '' }}</template>
                              · {{ formatSize(entry.size) }}
                            </small>
                          </div>
                        </label>
                      </div>
                    </template>
                  </div>
                </template>

                <template v-else-if="section === 'packs'">
                  <div v-for="bucket in [['resourcepacks', 'Ресурспаки'], ['shaderpacks', 'Шейдерпаки']] as const" :key="bucket[0]">
                    <h4 class="compare-group">{{ bucket[1] }}<small>{{ comparison.report[bucket[0]].added.length + comparison.report[bucket[0]].removed.length + comparison.report[bucket[0]].changed.length }}</small></h4>
                    <div v-if="comparison.report[bucket[0]].added.length + comparison.report[bucket[0]].removed.length + comparison.report[bucket[0]].changed.length" class="compare-list">
                      <label
                        v-for="entry in [...comparison.report[bucket[0]].added, ...comparison.report[bucket[0]].changed]"
                        :key="entry.path"
                        class="compare-row"
                        :class="{ 'compare-row--selected': selected.has(entryKey(bucket[0] === 'resourcepacks' ? 'resourcepack' : 'shaderpack', entry.path)) }"
                      >
                        <input type="checkbox" :checked="selected.has(entryKey(bucket[0] === 'resourcepacks' ? 'resourcepack' : 'shaderpack', entry.path))" @change="toggleEntry(bucket[0] === 'resourcepacks' ? 'resourcepack' : 'shaderpack', entry)">
                        <div class="compare-row-copy">
                          <span class="compare-row-name">{{ entry.name }}</span>
                          <small>{{ bucket[0] === 'resourcepacks' ? 'ресурспак' : 'шейдерпак' }} · {{ formatSize(entry.size) }}</small>
                        </div>
                      </label>
                      <div v-for="entry in comparison.report[bucket[0]].removed" :key="entry.path" class="compare-row compare-row--removed">
                        <Icon name="close" :size="12" />
                        <div class="compare-row-copy">
                          <span class="compare-row-name">{{ entry.name }}</span>
                          <small>нет в исходной · {{ formatSize(entry.size) }}</small>
                        </div>
                      </div>
                    </div>
                    <p v-else class="muted compare-empty">Различий нет</p>
                  </div>
                </template>

                <template v-else-if="section === 'configs'">
                  <div v-for="group in [['added', 'Новые конфиги'], ['changed', 'Изменившиеся конфиги'], ['removed', 'Отсутствующие в исходной']] as const" :key="group[0]">
                    <template v-if="comparison.report.configs[group[0]].length">
                      <h4 class="compare-group">{{ group[1] }}<small>{{ comparison.report.configs[group[0]].length }}</small></h4>
                      <div class="compare-list">
                        <label
                          v-for="entry in comparison.report.configs[group[0]]"
                          :key="entry.path"
                          class="compare-row"
                          :class="{ 'compare-row--selected': selected.has(entryKey('config', entry.path)) }"
                        >
                          <input v-if="group[0] !== 'removed'" type="checkbox" :checked="selected.has(entryKey('config', entry.path))" @change="toggleEntry('config', entry)">
                          <Icon v-else name="close" :size="12" />
                          <div class="compare-row-copy">
                            <span class="compare-row-name compare-row-name--mono">{{ entry.path }}</span>
                            <small>{{ formatSize(entry.size) }}</small>
                          </div>
                        </label>
                      </div>
                    </template>
                  </div>
                  <p v-if="!comparison.report.configs.added.length && !comparison.report.configs.changed.length && !comparison.report.configs.removed.length" class="muted compare-empty">Различий нет</p>
                </template>

                <template v-else>
                  <div class="compare-kv">
                    <div v-for="(value, key) in comparison.report.meta" :key="key" class="compare-kv-row">
                      <span>{{ key === 'version_id' ? 'Версия сборки' : key === 'minecraft_version' ? 'Версия Minecraft' : key }}</span>
                      <small>{{ value.from || '—' }} → {{ value.to }}</small>
                    </div>
                    <div v-for="item in comparison.report.options.changed" :key="item.key" class="compare-kv-row">
                      <span>options.txt · {{ item.key }}</span>
                      <small>{{ item.from || '—' }} → {{ item.to }}</small>
                    </div>
                    <p v-if="!Object.keys(comparison.report.meta).length && !comparison.report.options.changed.length" class="muted compare-empty">Различий нет</p>
                  </div>
                </template>
              </div>

              <div v-if="totalDifferences" class="compare-actions">
                <span class="muted">{{ selectedItems.length ? `Выбрано: ${selectedItems.length}` : 'Отметьте, что переносить' }}</span>
                <div class="compare-actions-buttons">
                  <button class="button button--quiet button--small" type="button" :disabled="porting" @click="selectAllVisible">Выбрать всё</button>
                  <button class="button button--quiet button--small" type="button" :disabled="porting || !selectedItems.length" @click="clearSelection">Снять выбор</button>
                  <button class="button button--accent button--small" type="button" :disabled="porting || !selectedItems.length" @click="portSelected">
                    <Icon name="upload" :size="14" />{{ porting ? 'Переношу…' : `Перенести в «${instanceLabel(target)}»` }}
                  </button>
                </div>
              </div>
              <p class="field-help compare-help">Перед перезаписью цель получает снапшот затронутой категории — откат в один клик из вкладки «Снапшоты».</p>
            </template>
          </div>
        </section>
      </div>
  </Teleport>
</template>

<style scoped>
.compare-card { max-width: 640px; }
.compare-pickers { display: flex; gap: 10px; align-items: flex-end; }
.compare-pickers .field { flex: 1; margin: 0; }
.compare-pickers .icon-button { margin-bottom: 8px; }
.compare-note { display: flex; gap: 8px; align-items: center; padding: 10px 12px; border-radius: 10px; background: var(--background-secondary, rgba(127, 127, 127, 0.12)); color: var(--text-secondary, #9aa0a6); font-size: 13px; }
.compare-note--danger { background: rgba(240, 71, 71, 0.12); color: #f0666a; }
.compare-verdict { display: flex; gap: 10px; align-items: center; padding: 12px; border-radius: 12px; background: rgba(127, 127, 127, 0.1); }
.compare-verdict small { display: block; color: var(--text-secondary, #9aa0a6); }
.compare-verdict--same { background: rgba(80, 200, 120, 0.12); }
.compare-tabs { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 12px; }
.compare-tab { border: 1px solid var(--border, rgba(127, 127, 127, 0.3)); background: transparent; color: var(--text-secondary, #9aa0a6); border-radius: 999px; padding: 5px 12px; font-size: 12px; cursor: pointer; display: inline-flex; gap: 6px; align-items: center; }
.compare-tab em { font-style: normal; font-weight: 600; }
.compare-tab--active { color: var(--text-primary, #fff); border-color: var(--accent, #7aa2f7); }
.compare-body { margin-top: 10px; max-height: 320px; overflow-y: auto; padding-right: 4px; }
.compare-group { display: flex; justify-content: space-between; align-items: baseline; font-size: 12px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--text-secondary, #9aa0a6); margin: 12px 0 6px; }
.compare-group small { font-size: 11px; }
.compare-list { display: flex; flex-direction: column; gap: 4px; }
.compare-row { display: flex; gap: 10px; align-items: center; padding: 7px 10px; border-radius: 9px; border: 1px solid transparent; cursor: pointer; }
.compare-row:hover { background: rgba(127, 127, 127, 0.08); }
.compare-row--selected { border-color: var(--accent, #7aa2f7); background: rgba(122, 162, 247, 0.08); }
.compare-row--removed { cursor: default; opacity: 0.7; }
.compare-row-copy { min-width: 0; flex: 1; }
.compare-row-name { font-weight: 600; font-size: 13px; display: flex; gap: 6px; align-items: baseline; }
.compare-row-name em { font-style: normal; font-weight: 400; font-size: 11px; color: var(--text-secondary, #9aa0a6); }
.compare-row-name--mono { font-family: var(--mono, monospace); font-size: 12px; }
.compare-row-copy small { color: var(--text-secondary, #9aa0a6); font-size: 11px; }
.compare-empty { padding: 6px 2px; font-size: 12px; }
.compare-kv-row { display: flex; justify-content: space-between; gap: 12px; padding: 7px 10px; border-radius: 8px; font-size: 13px; }
.compare-kv-row:nth-child(odd) { background: rgba(127, 127, 127, 0.07); }
.compare-kv-row small { color: var(--text-secondary, #9aa0a6); font-family: var(--mono, monospace); font-size: 11px; }
.compare-actions { display: flex; justify-content: space-between; align-items: center; gap: 10px; margin-top: 14px; flex-wrap: wrap; }
.compare-actions-buttons { display: flex; gap: 8px; }
.compare-help { margin-top: 8px; }
@media (max-width: 640px) { .compare-pickers { flex-direction: column; align-items: stretch; } }
</style>
