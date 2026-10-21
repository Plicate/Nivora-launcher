<script setup lang="ts">
// глобальный поиск по всем экземплярам: где установлен мод, в каком
// экземпляре мир или ресурспак. Ctrl+K или кнопка в верхней панели
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import Icon from './Icon.vue'
import { bridge } from '../services/bridge'

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

const emit = defineEmits<{
  (event: 'close'): void
  (event: 'open-instance', name: string): void
  (event: 'open-path', path: string): void
}>()

const inputElement = ref<HTMLInputElement | null>(null)
const query = ref('')
const response = ref<SearchResponse | null>(null)
const searching = ref(false)
let searchTimer: number | undefined

const groupTitles: Record<string, string> = {
  mods: 'Моды и плагины',
  worlds: 'Миры',
  resourcepacks: 'Ресурспаки',
  shaderpacks: 'Шейдерпаки',
}

const groupIcons: Record<string, string> = {
  mods: 'archive',
  worlds: 'globe',
  resourcepacks: 'image',
  shaderpacks: 'sparkle',
}

const groups = computed(() => {
  if (!response.value) return []
  const entries = response.value.results
  return (['mods', 'worlds', 'resourcepacks', 'shaderpacks'] as const)
    .map((key) => ({ key, title: groupTitles[key], items: entries[key] || [] }))
    .filter((group) => group.items.length > 0)
})

function scheduleSearch() {
  if (searchTimer) window.clearTimeout(searchTimer)
  const trimmed = query.value.trim()
  if (trimmed.length < 2) {
    response.value = null
    searching.value = false
    return
  }
  searching.value = true
  searchTimer = window.setTimeout(async () => {
    try {
      response.value = await bridge.globalSearch(trimmed)
    } catch {
      response.value = null
    } finally {
      searching.value = false
    }
  }, 250)
}

watch(query, scheduleSearch)

function choose(entry: SearchEntry) {
  emit('open-instance', entry.instance)
  emit('close')
}

function showInFolder(entry: SearchEntry) {
  const parent = entry.path.replace(/[\\/][^\\/]+$/, '')
  emit('open-path', parent || entry.path)
}

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

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    emit('close')
    return
  }
  if (event.key === 'Enter') {
    const first = groups.value[0]?.items[0]
    if (first) {
      event.preventDefault()
      choose(first)
    }
  }
}

onMounted(async () => {
  await nextTick()
  inputElement.value?.focus()
})

onBeforeUnmount(() => {
  if (searchTimer) window.clearTimeout(searchTimer)
})
</script>

<template>
  <Teleport to="body">
    <Transition name="dialog">
      <div class="dialog-scrim" role="presentation" @click.self="emit('close')">
        <section class="dialog-card global-search-card" role="dialog" tabindex="-1" aria-modal="true" aria-label="Глобальный поиск">
          <div class="global-search-head">
            <Icon name="search" :size="17" />
            <input
              ref="inputElement"
              v-model="query"
              class="global-search-input"
              type="text"
              placeholder="Где установлен мод? Мир, ресурспак, шейдер…"
              autocomplete="off"
              spellcheck="false"
              @keydown="onKeydown"
            />
            <button class="icon-button" type="button" aria-label="Закрыть" @click="emit('close')"><Icon name="close" :size="16" /></button>
          </div>

          <div v-if="searching" class="global-search-state">Ищу по всем экземплярам…</div>
          <div v-else-if="query.trim().length < 2" class="global-search-state">Введите хотя бы две буквы. Поиск видит моды (включая выключенные и их внутренние имена из jar-манифестов), плагины серверов, миры, ресурспаки и шейдеры во всех экземплярах.</div>
          <div v-else-if="!groups.length" class="global-search-state">Ничего не нашлось. Попробуйте короче — по подстроке имени.</div>

          <div v-else class="global-search-groups">
            <div v-for="group in groups" :key="group.key" class="global-search-group">
              <h4>{{ group.title }}<span>{{ group.items.length }}</span></h4>
              <button
                v-for="entry in group.items"
                :key="`${entry.instance}:${entry.path}`"
                class="global-search-row"
                type="button"
                @click="choose(entry)"
              >
                <Icon :name="groupIcons[group.key] || 'file'" :size="14" />
                <span class="global-search-row-name">{{ entryTitle(entry) }}</span>
                <span class="global-search-row-instance">{{ entry.instance }}<small v-if="entry.minecraft_version"> · {{ entry.minecraft_version }}</small></span>
                <span
                  class="global-search-row-folder"
                  role="button"
                  tabindex="-1"
                  aria-label="Показать во встроенном проводнике"
                  @click.stop="showInFolder(entry)"
                  @keydown.enter.stop.prevent="showInFolder(entry)"
                ><Icon name="folder" :size="14" /></span>
              </button>
            </div>
          </div>

          <footer class="global-search-foot">Ctrl+K — открыть поиск · Enter — первый результат · клик по папке — проводник</footer>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>
