<script setup lang="ts">
// палитра команд: единая поверхность Ctrl+K — разделы, действия и
// глобальный поиск по экземплярам в одном окне
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import Icon from './Icon.vue'
import { bridge } from '../services/bridge'
import type { PaletteCommand } from '../types'

const props = defineProps<{ items: PaletteCommand[] }>()

const emit = defineEmits<{
  (event: 'close'): void
  (event: 'toast', message: string): void
  (event: 'open-instance', name: string): void
  (event: 'open-path', path: string): void
  (event: 'update-mods', instance: string): void
}>()

interface SearchEntry {
  instance: string
  instance_kind: string
  game_edition?: string
  minecraft_version: string
  name: string
  path: string
  enabled?: boolean
  size_bytes?: number
  kind: string
  mod_id?: string
  version?: string
  loader?: string
  file_name?: string
}

interface SearchResponse {
  query: string
  total: number
  results: {
    mods: SearchEntry[]
    worlds: SearchEntry[]
    resourcepacks: SearchEntry[]
    shaderpacks: SearchEntry[]
  }
}

interface PaletteRow {
  key: string
  group: string
  title: string
  subtitle?: string
  icon: string
  hint?: string
  folderable?: boolean
  entry?: SearchEntry
  command?: PaletteCommand
  action?: 'toggle-mod' | 'update-mods'
}

const inputElement = ref<HTMLInputElement | null>(null)
const cardElement = ref<HTMLElement | null>(null)
const query = ref('')
const activeIndex = ref(0)

const response = ref<SearchResponse | null>(null)
const searching = ref(false)
let searchTimer: number | undefined
let activeSearch: AbortController | null = null
let searchGeneration = 0

const searchGroupTitles: Record<string, string> = {
  mods: 'Моды и плагины',
  worlds: 'Миры',
  resourcepacks: 'Ресурспаки',
  shaderpacks: 'Шейдерпаки',
}

const searchGroupIcons: Record<string, string> = {
  mods: 'archive',
  worlds: 'globe',
  resourcepacks: 'image',
  shaderpacks: 'sparkle',
}

// «мод:» — режим управления модами: поиск только по модам + действия
const MOD_PREFIX_RE = /^(мод|mod)\s*[:：]\s*/i
const modPrefix = computed(() => MOD_PREFIX_RE.test(query.value))

function entryTitle(entry: SearchEntry): string {
  if (entry.kind === 'plugin') return `${entry.name} · плагин`
  if (entry.kind === 'mod' && entry.mod_id) {
    const version = entry.version && entry.version !== '-' ? ` ${entry.version}` : ''
    const suffix = entry.enabled === false ? ' · выключен' : ''
    return `${entry.name} · ${entry.mod_id}${version}${suffix}`
  }
  if (entry.kind === 'mod' && entry.enabled === false) return `${entry.name} · выключен`
  return entry.name
}

const commandRows = computed<PaletteRow[]>(() => {
  if (modPrefix.value) return []
  const needle = query.value.trim().toLowerCase()
  return props.items
    .filter((item) => {
      if (!needle) return true
      const haystack = `${item.title} ${item.subtitle || ''} ${item.keywords || ''}`.toLowerCase()
      return haystack.includes(needle)
    })
    .map((item) => ({
      key: item.id,
      group: item.group,
      title: item.title,
      subtitle: item.subtitle,
      icon: item.icon,
      command: item,
    }))
})

const firstModInstance = computed(() => response.value?.results.mods?.[0]?.instance || '')

const searchRows = computed<PaletteRow[]>(() => {
  if (!response.value) return []
  const rows: PaletteRow[] = []
  const prefixMode = modPrefix.value
  let updateRowAdded = false
  const keys = prefixMode ? (['mods'] as const) : (['mods', 'worlds', 'resourcepacks', 'shaderpacks'] as const)
  for (const key of keys) {
    const entries = response.value.results[key] || []
    for (const entry of entries) {
      rows.push({
        key: `search:${entry.instance}:${entry.path}`,
        group: searchGroupTitles[key] || 'Результаты',
        title: entryTitle(entry),
        icon: searchGroupIcons[key] || 'file',
        hint: `${entry.instance}${entry.minecraft_version ? ' · ' + entry.minecraft_version : ''}`,
        folderable: true,
        entry,
      })
      if (prefixMode && key === 'mods' && entry.kind === 'mod') {
        const disabled = entry.enabled === false
        rows.push({
          key: `modtoggle:${entry.instance}:${entry.path}`,
          group: searchGroupTitles.mods,
          title: `${disabled ? 'Включить' : 'Выключить'} «${entry.name}»`,
          icon: 'power',
          hint: entry.instance,
          entry,
          action: 'toggle-mod',
        })
      }
      if (prefixMode && key === 'mods' && !updateRowAdded) {
        updateRowAdded = true
        const instance = firstModInstance.value || entry.instance
        rows.push({
          key: 'modupdate-all',
          group: searchGroupTitles.mods,
          title: `Обновить все моды «${instance}»`,
          subtitle: 'безопасный режим',
          icon: 'refresh',
          action: 'update-mods',
        })
      }
    }
  }
  return rows
})

const rows = computed<PaletteRow[]>(() => [...commandRows.value, ...searchRows.value])

const activeRowKey = computed(() => rows.value[activeIndex.value]?.key || '')

const groups = computed(() => {
  const ordered: Array<{ title: string; rows: PaletteRow[] }> = []
  const byTitle = new Map<string, { title: string; rows: PaletteRow[] }>()
  for (const row of rows.value) {
    let group = byTitle.get(row.group)
    if (!group) {
      group = { title: row.group, rows: [] }
      byTitle.set(row.group, group)
      ordered.push(group)
    }
    group.rows.push(row)
  }
  return ordered
})

watch([query, () => props.items], () => {
  activeIndex.value = 0
})

watch(activeIndex, async () => {
  await nextTick()
  const active = cardElement.value?.querySelector('.palette-row--active')
  active?.scrollIntoView({ block: 'nearest' })
})

function cancelSearch() {
  if (searchTimer !== undefined) {
    window.clearTimeout(searchTimer)
    searchTimer = undefined
  }
  activeSearch?.abort()
  activeSearch = null
  searchGeneration += 1
}

function scheduleSearch() {
  cancelSearch()
  const trimmed = query.value.replace(MOD_PREFIX_RE, '').trim()
  if (trimmed.length < (modPrefix.value ? 1 : 2)) {
    response.value = null
    searching.value = false
    return
  }
  response.value = null
  searching.value = true
  const generation = searchGeneration
  searchTimer = window.setTimeout(async () => {
    searchTimer = undefined
    const controller = new AbortController()
    activeSearch = controller
    try {
      const nextResponse = await bridge.globalSearch(trimmed, controller.signal)
      if (generation === searchGeneration && !controller.signal.aborted) response.value = nextResponse
    } catch {
      if (generation === searchGeneration && !controller.signal.aborted) response.value = null
    } finally {
      if (generation === searchGeneration) {
        searching.value = false
        if (activeSearch === controller) activeSearch = null
      }
    }
  }, 250)
}

watch(query, scheduleSearch, { flush: 'sync' })

function close() {
  cancelSearch()
  searching.value = false
  emit('close')
}

function activate(row: PaletteRow) {
  if (row.command) {
    row.command.run()
    close()
    return
  }
  if (row.entry && row.action === 'toggle-mod') {
    void toggleEntryMod(row.entry)
    return
  }
  if (row.action === 'update-mods') {
    const instance = firstModInstance.value
    if (instance) {
      emit('update-mods', instance)
      close()
    }
    return
  }
  if (row.entry) {
    emit('open-instance', row.entry.instance)
    close()
  }
}

async function toggleEntryMod(entry: SearchEntry) {
  const enabling = entry.enabled === false
  try {
    await bridge.toggleMod(entry.instance, entry.path, enabling)
    emit('toast', `«${entry.name}» ${enabling ? 'включён' : 'выключен'}`)
  } catch {
    emit('toast', `Не удалось переключить «${entry.name}»`)
  }
  scheduleSearch()
}

function openFolder(row: PaletteRow) {
  if (!row.entry) return
  const parent = row.entry.path.replace(/[\\/][^\\/]+$/, '')
  emit('open-path', parent || row.entry.path)
  close()
}

function onKeydown(event: KeyboardEvent) {
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k') {
    event.preventDefault()
    close()
    return
  }
  if (event.key === 'Escape') {
    event.preventDefault()
    close()
    return
  }
  if (event.key === 'ArrowDown') {
    event.preventDefault()
    if (rows.value.length) activeIndex.value = (activeIndex.value + 1) % rows.value.length
    return
  }
  if (event.key === 'ArrowUp') {
    event.preventDefault()
    if (rows.value.length) activeIndex.value = (activeIndex.value + rows.value.length - 1) % rows.value.length
    return
  }
  if (event.key === 'Home') {
    event.preventDefault()
    activeIndex.value = 0
    return
  }
  if (event.key === 'End') {
    event.preventDefault()
    activeIndex.value = Math.max(0, rows.value.length - 1)
    return
  }
  if (event.key === 'Enter') {
    event.preventDefault()
    const row = rows.value[activeIndex.value]
    if (row) activate(row)
  }
}

function hoverRow(row: PaletteRow) {
  const index = rows.value.indexOf(row)
  if (index >= 0) activeIndex.value = index
}

const emptyState = computed(() => !searching.value && !rows.value.length)

onMounted(async () => {
  await nextTick()
  inputElement.value?.focus()
})

onBeforeUnmount(() => {
  cancelSearch()
  searching.value = false
})
</script>

<template>
      <div class="dialog-scrim" role="presentation" @click.self="close">
      <section
        ref="cardElement"
        class="dialog-card palette-card"
        role="dialog"
        tabindex="-1"
        aria-modal="true"
        aria-label="Палитра команд"
        @keydown="onKeydown"
      >
        <div class="palette-head">
          <Icon name="search" :size="17" />
          <input
            ref="inputElement"
            v-model="query"
            class="palette-input"
            type="text"
            placeholder="Команда, «мод:» или поиск…"
            autocomplete="off"
            spellcheck="false"
          />
          <span class="palette-kbd">Ctrl</span>
          <span class="palette-kbd">K</span>
          <button class="icon-button" type="button" aria-label="Закрыть" @click="close">
            <Icon name="close" :size="16" />
          </button>
        </div>

        <div class="palette-body">
          <div v-if="emptyState" class="palette-state">{{ modPrefix ? 'Введите название мода после «мод:»…' : 'Ничего не нашлось. Попробуйте короче.' }}</div>
          <template v-else>
            <div v-for="group in groups" :key="group.title" class="palette-group">
              <h4>{{ group.title }}<span>{{ group.rows.length }}</span></h4>
              <button
                v-for="row in group.rows"
                :key="row.key"
                class="palette-row"
                :class="{ 'palette-row--active': row.key === activeRowKey }"
                type="button"
                @click="activate(row)"
                @mousemove="hoverRow(row)"
              >
                <Icon :name="row.icon" :size="15" />
                <span class="palette-row-title">
                  {{ row.title }}
                  <small v-if="row.subtitle"> · {{ row.subtitle }}</small>
                </span>
                <span v-if="row.hint" class="palette-row-hint">{{ row.hint }}</span>
                <span
                  v-if="row.folderable"
                  class="palette-row-folder"
                  role="button"
                  tabindex="-1"
                  aria-label="Показать во встроенном проводнике"
                  @click.stop="openFolder(row)"
                  @keydown.enter.stop.prevent="openFolder(row)"
                >
                  <Icon name="folder" :size="14" />
                </span>
              </button>
            </div>
            <div v-if="searching" class="palette-state palette-state--live">Ищу по всем экземплярам…</div>
            <div
              v-else-if="query.trim().length >= 2 && !searchRows.length && commandRows.length"
              class="palette-state"
            >По содержимому экземпляров ничего не нашлось.</div>
          </template>
        </div>

        <footer class="palette-foot">
          <span><span class="palette-kbd">↑↓</span> выбор</span>
          <span><span class="palette-kbd">Enter</span> выполнить</span>
          <span><span class="palette-kbd">Esc</span> закрыть</span>
        </footer>
      </section>
    </div>
  </template>

<style scoped>
.palette-card {
  width: min(580px, calc(100vw - 48px));
  max-height: min(64vh, 560px);
  display: flex;
  flex-direction: column;
  padding: 0;
  overflow: hidden;
  animation: palette-card-in 0.18s cubic-bezier(0.2, 0.9, 0.3, 1);
}
.palette-head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 14px;
  border-bottom: 1px solid var(--ds-border);
  color: var(--muted);
}
.palette-input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: inherit;
  font: inherit;
  font-size: 14.5px;
}
.palette-input::placeholder {
  color: var(--muted);
}
.palette-kbd {
  flex: none;
  padding: 2px 6px;
  border: 1px solid var(--ds-border);
  border-radius: 6px;
  background: var(--ds-surface-2);
  color: var(--muted);
  font-size: 10.5px;
  line-height: 1.5;
}
.palette-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  padding: 6px 8px 10px;
}
.palette-group h4 {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin: 10px 6px 4px;
  font-size: 11px;
  font-weight: 650;
  color: var(--muted);
}
.palette-group h4 span {
  font-weight: 500;
  font-size: 10.5px;
}
.palette-row {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
  padding: 8px 10px;
  border: none;
  border-radius: 9px;
  background: transparent;
  color: inherit;
  font: inherit;
  font-size: 13.5px;
  text-align: left;
  cursor: pointer;
  transition: background 0.12s ease, color 0.12s ease;
}
.palette-row + .palette-row {
  margin-top: 1px;
}
.palette-row > .icon {
  flex: none;
  color: var(--muted);
  transition: color 0.12s ease;
}
.palette-row--active {
  background: color-mix(in srgb, var(--accent) 15%, transparent);
}
.palette-row--active > .icon {
  color: var(--accent);
}
.palette-row-title {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.palette-row-title small {
  color: var(--muted);
  font-size: 12px;
}
.palette-row-hint {
  flex: none;
  max-width: 40%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--muted);
  font-size: 11.5px;
}
.palette-row-folder {
  flex: none;
  width: 26px;
  height: 26px;
  display: grid;
  place-items: center;
  border-radius: 7px;
  color: var(--muted);
  transition: background 0.12s ease, color 0.12s ease;
}
.palette-row-folder:hover {
  background: var(--panel-soft, var(--ds-surface-2));
  color: inherit;
}
.palette-state {
  padding: 18px 12px;
  color: var(--muted);
  font-size: 12.5px;
  line-height: 1.55;
}
.palette-state--live {
  padding: 10px 12px 2px;
  animation: palette-pulse 1.1s ease-in-out infinite;
}
.palette-foot {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 8px 14px;
  border-top: 1px solid var(--ds-border);
  color: var(--muted);
  font-size: 11px;
}
@keyframes palette-card-in {
  from {
    opacity: 0;
    transform: translateY(-8px) scale(0.985);
  }
  to {
    opacity: 1;
    transform: none;
  }
}
@keyframes palette-pulse {
  0%,
  100% {
    opacity: 0.55;
  }
  50% {
    opacity: 1;
  }
}
@media (prefers-reduced-motion: reduce) {
  .palette-card,
  .palette-state--live {
    animation: none;
  }
}
</style>
