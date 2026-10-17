<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import CatalogDetailsDialog from './CatalogDetailsDialog.vue'
import CustomSelect from './CustomSelect.vue'
import Icon from './Icon.vue'
import { bridge } from '../services/bridge'
import { confirmDialog } from '../services/dialogs'
import type { GitHubPluginDetails, GitHubPluginRepository, PluginManifest, SelectOption } from '../types'

const props = withDefaults(defineProps<{ plugins: PluginManifest[]; installBusy: boolean; active?: boolean }>(), { active: true })
const emit = defineEmits<{
  install: [payload: { owner: string; repo: string; replace: boolean }]
}>()

const query = ref('')
const sortMode = ref<'updated' | 'stars'>('updated')
const results = ref<GitHubPluginRepository[]>([])
const page = ref(1)
const loading = ref(false)
const catalogError = ref('')
const selectedRepository = ref<GitHubPluginRepository>()
const selected = ref<GitHubPluginDetails>()
const detailsOpen = ref(false)
const detailsLoading = ref(false)
const detailsError = ref('')
const installBusy = ref(false)
let detailsRequestId = 0
let automaticSearchStarted = false

const sortOptions: SelectOption[] = [
  { value: 'updated', label: 'Недавно обновлены' },
  { value: 'stars', label: 'По популярности' },
]

watch(() => props.installBusy, (busy) => {
  if (!busy) installBusy.value = false
})

const visibleResults = computed(() => [...results.value].sort((left, right) => sortMode.value === 'stars'
  ? right.stars - left.stars
  : Date.parse(right.updated_at) - Date.parse(left.updated_at)))
const canLoadMore = computed(() => results.value.length >= page.value * 30 && page.value < 10)
const installedPlugin = computed(() => props.plugins.find((plugin) => plugin.id === selected.value?.manifest.id))
const detailsForDialog = computed<Record<string, unknown> | undefined>(() => {
  if (!selected.value) return undefined
  return {
    title: selected.value.manifest.name,
    author: selected.value.manifest.author,
    description: selected.value.manifest.description || selected.value.description,
    date_modified: selected.value.latest_release.published_at,
    version: selected.value.download.version,
    archive_size: selected.value.download.size,
    sha256: selected.value.download.sha256,
    release_url: selected.value.latest_release.url,
    repository_url: selected.value.repository_url,
  }
})

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return typeof error === 'string' ? error : 'Неизвестная ошибка'
}

async function searchGithub(reset = true) {
  if (loading.value) return
  loading.value = true
  catalogError.value = ''
  if (reset) {
    detailsRequestId += 1
    detailsOpen.value = false
    selected.value = undefined
    selectedRepository.value = undefined
    detailsLoading.value = false
    detailsError.value = ''
    results.value = []
    page.value = 1
  }
  try {
    const nextPage = reset ? 1 : page.value + 1
    const response = await bridge.githubPluginSearch(query.value.trim(), nextPage)
    page.value = response.page
    results.value = reset ? response.results : [...results.value, ...response.results]
  } catch (error) {
    catalogError.value = errorMessage(error)
  } finally {
    loading.value = false
  }
}

async function showDetails(repository: GitHubPluginRepository) {
  const requestId = ++detailsRequestId
  selectedRepository.value = repository
  selected.value = undefined
  detailsError.value = ''
  detailsLoading.value = true
  detailsOpen.value = true
  try {
    const details = await bridge.githubPluginDetails(repository.owner, repository.repo)
    if (requestId === detailsRequestId) selected.value = details
  } catch (error) {
    if (requestId === detailsRequestId) detailsError.value = errorMessage(error)
  } finally {
    if (requestId === detailsRequestId) detailsLoading.value = false
  }
}

function closeDetails() {
  detailsRequestId += 1
  detailsOpen.value = false
  detailsLoading.value = false
  detailsError.value = ''
  selected.value = undefined
  selectedRepository.value = undefined
}

async function installSelected() {
  if (!selected.value || installBusy.value || props.installBusy) return
  const replace = Boolean(installedPlugin.value)
  if (replace && !(await confirmDialog(`Заменить локальный архив «${installedPlugin.value?.name}» версией ${selected.value.download.version}? Код архива не будет запущен.`, { danger: true, confirmLabel: 'Заменить архив' }))) return
  installBusy.value = true
  emit('install', { owner: selected.value.owner, repo: selected.value.repo, replace })
}

function formatDate(value: string): string {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '' : date.toLocaleDateString()
}

function formatStars(value: number): string {
  return new Intl.NumberFormat().format(value)
}

watch(() => props.active, (active) => {
  if (!active || automaticSearchStarted) return
  automaticSearchStarted = true
  void searchGithub()
}, { immediate: true })
</script>

<template>
  <section class="plugin-catalog" aria-label="Каталог плагинов сообщества">
    <section class="panel search-panel">
      <div class="search-bar">
        <Icon name="search" :size="19" />
        <input v-model="query" maxlength="120" placeholder="Название или тема плагина" aria-label="Поиск плагинов" @keyup.enter="searchGithub()" />
        <CustomSelect v-model="sortMode" :options="sortOptions" aria-label="Сортировка плагинов" />
        <slot name="import-control" />
        <button class="button button--accent" type="button" :disabled="loading" @click="searchGithub()">{{ loading ? 'Ищу…' : 'Найти плагины' }}</button>
      </div>
      <div class="search-context"><span>Источник: GitHub Releases</span><span class="context-divider" /><span>ZIP проверяется перед импортом · код не запускается</span></div>
      <p v-if="catalogError" class="inline-error">{{ catalogError }}</p>
    </section>

    <div v-if="loading && !results.length" class="empty-card empty-card--short" role="status" aria-live="polite"><span class="status-dot status-dot--busy" /><h3>Ищу плагины…</h3><p>Ищу публичные GitHub-репозитории каталога.</p></div>
    <div v-else-if="visibleResults.length" class="mod-grid">
      <article v-for="repository in visibleResults" :key="repository.full_name" class="mod-card">
        <div class="mod-icon"><Icon name="plug" :size="23" /></div>
        <div class="mod-card-copy">
          <span class="plugin-repository-owner">{{ repository.full_name }}</span>
          <h3>{{ repository.name }}</h3>
          <p>{{ repository.description || 'Автор не добавил описание.' }}</p>
          <div class="mod-card-footer">
            <span class="mod-downloads">{{ formatStars(repository.stars) }} ★ · {{ formatDate(repository.updated_at) || 'GitHub' }}</span>
            <a class="button button--small button--quiet" :href="repository.repository_url" target="_blank" rel="noreferrer">GitHub</a>
            <button class="button button--small button--accent" type="button" :aria-label="`Открыть карточку плагина ${repository.full_name}`" :disabled="installBusy || props.installBusy" @click="showDetails(repository)"><Icon name="download" :size="14" />Подробнее / импорт</button>
          </div>
        </div>
      </article>
    </div>
    <div v-else-if="catalogError" class="empty-card empty-card--short"><span class="empty-orb empty-orb--large"><Icon name="plug" :size="27" /></span><h3>Каталог недоступен</h3><p>Проверьте подключение и попробуйте снова.</p></div>
    <div v-else class="empty-card empty-card--short"><span class="empty-orb empty-orb--large"><Icon name="plug" :size="27" /></span><h3>{{ query ? 'Ничего не найдено' : 'Каталог пока пуст' }}</h3><p>{{ query ? 'Измените запрос и попробуйте снова.' : 'Публичные плагины появятся после публикации репозитория с topic nivora-launcher-plugin.' }}</p></div>

    <button v-if="canLoadMore" class="button button--quiet plugin-catalog-more" type="button" :disabled="loading" @click="searchGithub(false)">{{ loading ? 'Загружаю…' : 'Показать ещё' }}</button>

    <CatalogDetailsDialog
      v-if="detailsOpen"
      :details="detailsForDialog"
      :versions="[]"
      project-type="plugin"
      :catalog-name="selectedRepository?.full_name || 'GitHub'"
      :loading="detailsLoading"
      :error="detailsError"
      :can-install="Boolean(selected)"
      :installed="Boolean(installedPlugin)"
      :busy="installBusy || props.installBusy"
      @close="closeDetails"
      @install="installSelected"
    />
  </section>
</template>

<style scoped>
.plugin-catalog { display: grid; min-width: 0; gap: 14px; }
.plugin-repository-owner { display: block; overflow: hidden; margin-bottom: 4px; color: var(--muted); font-size: 9px; text-overflow: ellipsis; white-space: nowrap; }
.plugin-catalog-more { justify-self: center; }
@media (max-width: 760px) {
  .search-context { align-items: flex-start; flex-wrap: wrap; }
  .search-context .context-divider { display: none; }
}
</style>
