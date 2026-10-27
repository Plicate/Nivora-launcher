<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import Icon from './Icon.vue'
import CustomSelect from './CustomSelect.vue'
import { useModalFocus } from '../services/modalFocus'
import type { ProjectType, SelectOption } from '../types'

interface CompatibleVersion {
  id: string
  version_number: string
  name: string
  date_published: string
  game_versions: string[]
  loaders: string[]
}

const props = withDefaults(defineProps<{
  details?: Record<string, unknown>
  versions?: CompatibleVersion[]
  projectType: ProjectType
  catalogName: string
  loading?: boolean
  error?: string
  canInstall?: boolean
  installed?: boolean
  busy?: boolean
}>(), { loading: false, error: '', canInstall: false, installed: false, busy: false })

const emit = defineEmits<{
  close: []
  install: [versionId: string]
}>()

const selectedVersionId = ref('')
const dialogElement = ref<HTMLElement | null>(null)
const dialogOpen = ref(true)
const failedScreenshots = ref<string[]>([])
const versions = computed(() => props.versions || [])
const details = computed(() => props.details || {})
const title = computed(() => String(details.value.title || details.value.name || details.value.project_id || 'Подробности проекта'))
const description = computed(() => String(details.value.body || details.value.description || 'Описание не предоставлено.'))
const screenshots = computed(() => {
  const gallery = Array.isArray(details.value.gallery)
    ? details.value.gallery as unknown[]
    : Array.isArray(details.value.screenshots) ? details.value.screenshots as unknown[] : []
  return gallery.flatMap((entry, index): Array<{ url: string; alt: string }> => {
    const image = typeof entry === 'string' ? { url: entry } : entry && typeof entry === 'object' ? entry as Record<string, unknown> : {}
    const url = String(image.url || image.image || image.src || '')
    try {
      if (new URL(url).protocol !== 'https:') return []
    } catch {
      return []
    }
    if (failedScreenshots.value.includes(url)) return []
    return [{ url, alt: String(image.title || image.description || `${title.value} · скриншот ${index + 1}`) }]
  })
})
const categories = computed(() => Array.isArray(details.value.categories) ? details.value.categories.map(String).slice(0, 24) : [])
const gameVersions = computed(() => Array.isArray(details.value.game_versions) ? details.value.game_versions.map(String).slice(0, 24) : [])
const dependencies = computed(() => Array.isArray(details.value.dependencies) ? details.value.dependencies.filter((value) => value && typeof value === 'object') as Array<Record<string, unknown>> : [])
const licenseName = computed(() => {
  const value = details.value.license
  if (typeof value === 'string') return value
  if (value && typeof value === 'object') {
    const license = value as Record<string, unknown>
    return String(license.name || license.id || '')
  }
  return ''
})
const selectedVersion = computed(() => versions.value.find((item) => item.id === selectedVersionId.value) || versions.value[0])
const versionOptions = computed<SelectOption[]>(() => versions.value.map((version) => ({
  value: version.id,
  label: version.version_number || version.name || version.id,
  meta: formatDate(version.date_published),
})))

watch(versions, (values) => {
  if (!values.some((item) => item.id === selectedVersionId.value)) selectedVersionId.value = values[0]?.id || ''
}, { immediate: true })
watch(() => props.details, () => { failedScreenshots.value = [] }, { deep: true })

function hideScreenshot(url: string) {
  failedScreenshots.value = [...failedScreenshots.value, url]
}

function formatDate(value: unknown) {
  const date = new Date(String(value || ''))
  return Number.isNaN(date.getTime()) ? '' : new Intl.DateTimeFormat(undefined, { year: 'numeric', month: 'short', day: 'numeric' }).format(date)
}

function formatCount(value: unknown) {
  const number = Number(value)
  return Number.isFinite(number) && number > 0 ? new Intl.NumberFormat().format(number) : ''
}

function startInstall() {
  emit('install', selectedVersion.value?.id || '')
}

function closeDialog() {
  dialogOpen.value = false
  emit('close')
}

const handleDialogKeydown = useModalFocus(dialogOpen, dialogElement, closeDialog)
</script>

<template>
  <div class="catalog-details-scrim" role="presentation" @click.self="closeDialog">
    <section ref="dialogElement" class="catalog-details-dialog" role="dialog" tabindex="-1" aria-modal="true" :aria-label="`Подробности: ${title}`" @keydown="handleDialogKeydown">
      <header class="catalog-details-header">
         <div class="catalog-details-mark"><img v-if="details.icon_url || details.icon" :src="String(details.icon_url || details.icon)" alt="" /><Icon v-else :name="projectType === 'plugin' ? 'plug' : 'mods'" :size="24" /></div>
        <div class="catalog-details-title"><h2>{{ title }}</h2><p>{{ String(details.author || details.owner || '') }}</p></div>
         <button class="icon-button" type="button" aria-label="Закрыть подробности" @click="closeDialog"><Icon name="close" :size="18" /></button>
      </header>

      <div class="catalog-details-body">
        <div v-if="loading" class="catalog-details-state"><span class="status-dot status-dot--busy" />Загружаю полные сведения и совместимые версии…</div>
        <p v-if="error" class="inline-error" role="alert">{{ error }}</p>
        <template v-if="!loading || details.project_id || details.id">
          <p class="catalog-details-description">{{ description }}</p>
          <section v-if="screenshots.length" class="catalog-details-section catalog-details-gallery">
            <h3>Скриншоты</h3>
            <div class="catalog-details-gallery-grid">
              <figure v-for="image in screenshots" :key="image.url">
                <img :src="image.url" :alt="image.alt" loading="lazy" referrerpolicy="no-referrer" @error="hideScreenshot(image.url)" />
                <figcaption>{{ image.alt }}</figcaption>
              </figure>
            </div>
          </section>
          <div class="catalog-details-facts">
            <div v-if="formatCount(details.downloads)"><span>Загрузки</span><strong>{{ formatCount(details.downloads) }}</strong></div>
            <div v-if="formatCount(details.follows)"><span>В избранном</span><strong>{{ formatCount(details.follows) }}</strong></div>
            <div v-if="licenseName"><span>Лицензия</span><strong>{{ licenseName }}</strong></div>
            <div v-if="formatDate(details.date_created || details.created_at || details.datePublished)"><span>Опубликовано</span><strong>{{ formatDate(details.date_created || details.created_at || details.datePublished) }}</strong></div>
            <div v-if="formatDate(details.date_modified || details.updated_at || details.lastUpdated)"><span>Обновлено</span><strong>{{ formatDate(details.date_modified || details.updated_at || details.lastUpdated) }}</strong></div>
            <div v-if="details.client_side || details.server_side"><span>Сторона игры</span><strong>{{ String(details.client_side || 'unknown') }} / {{ String(details.server_side || 'unknown') }}</strong></div>
          </div>
          <section v-if="projectType === 'plugin' && (details.version || details.sha256)" class="catalog-details-section">
            <h3>Проверенный GitHub Release</h3>
            <div class="catalog-details-facts">
              <div v-if="details.version"><span>Версия архива</span><strong>{{ String(details.version) }}</strong></div>
              <div v-if="details.archive_size"><span>Размер ZIP</span><strong>{{ Number(details.archive_size).toLocaleString() }} bytes</strong></div>
              <div v-if="details.sha256" class="catalog-details-hash"><span>SHA-256</span><strong>{{ String(details.sha256) }}</strong></div>
            </div>
            <div class="catalog-details-links"><a v-if="details.release_url" :href="String(details.release_url)" target="_blank" rel="noreferrer">Открыть Release</a><a v-if="details.repository_url" :href="String(details.repository_url)" target="_blank" rel="noreferrer">Открыть репозиторий</a></div>
            <p class="catalog-details-note">Архив будет сохранён выключенным. JavaScript-код плагина не запускается.</p>
          </section>
          <div v-if="categories.length" class="catalog-details-section"><h3>Категории</h3><div class="catalog-details-chips"><span v-for="category in categories" :key="category">{{ category }}</span></div></div>
          <div v-if="gameVersions.length" class="catalog-details-section"><h3>Версии Minecraft</h3><div class="catalog-details-chips"><span v-for="version in gameVersions" :key="version">{{ version }}</span></div></div>
          <div v-if="dependencies.length" class="catalog-details-section"><h3>Зависимости</h3><ul><li v-for="(dependency, index) in dependencies" :key="String(dependency.project_id || dependency.id || index)">{{ String(dependency.project_id || dependency.id || dependency.name || 'Зависимость') }}<small v-if="dependency.dependency_type"> · {{ String(dependency.dependency_type) }}</small></li></ul><p class="catalog-details-note">Обязательные зависимости будут установлены вместе с ресурсом, если каталог публикует их ID.</p></div>
          <div v-if="versions.length" class="catalog-details-section"><h3>Совместимая версия</h3><CustomSelect v-model="selectedVersionId" :options="versionOptions" /><div class="catalog-details-chips"><span v-for="value in selectedVersion?.loaders || []" :key="`loader-${value}`">{{ value }}</span><span v-for="value in selectedVersion?.game_versions || []" :key="`game-${value}`">{{ value }}</span></div></div>
        </template>
      </div>

      <footer class="catalog-details-footer">
         <span>{{ projectType === 'plugin' ? (installed ? 'Локальный архив будет заменён' : 'ZIP будет проверен и сохранён выключенным') : installed ? 'Уже установлен' : 'Проверьте совместимость перед установкой' }}</span>
         <button v-if="canInstall" class="button button--accent" type="button" :disabled="busy || (installed && projectType !== 'plugin')" @click="startInstall"><Icon name="download" :size="15" />{{ busy ? (projectType === 'plugin' ? 'Импортирую…' : 'Устанавливаю…') : installed ? (projectType === 'plugin' ? 'Сохранить новую версию' : 'Установлено') : (projectType === 'plugin' ? 'Импортировать ZIP' : 'Установить') }}</button>
      </footer>
    </section>
  </div>
</template>

<style scoped>
.catalog-details-scrim { position: fixed; z-index: 90; inset: 0; display: grid; place-items: center; padding: 24px; background: rgb(3 7 14 / 74%); }
.catalog-details-dialog { display: flex; width: min(760px, 100%); max-height: min(84vh, 850px); flex-direction: column; overflow: hidden; border: 1px solid var(--line-strong); border-radius: min(calc(var(--radius-card, 7px) * 3.143), 30px); color: var(--text); background: var(--panel); box-shadow: 0 28px 100px rgb(0 0 0 / 40%); }
.catalog-details-header { display: flex; align-items: flex-start; gap: 16px; padding: 24px; border-bottom: 1px solid var(--line); }
.catalog-details-mark { display: grid; width: 54px; height: 54px; flex: 0 0 54px; place-items: center; overflow: hidden; border: 1px solid var(--line); border-radius: min(calc(var(--radius-card, 7px) * 2.286), 24px); background: var(--panel-soft); color: var(--accent); }
.catalog-details-mark img { width: 100%; height: 100%; object-fit: cover; }
.catalog-details-title { min-width: 0; flex: 1; }
.catalog-details-title h2 { margin: 5px 0; font-size: 1.4rem; line-height: 1.15; }
.catalog-details-title p { margin: 0; color: var(--muted); }
.catalog-details-body { min-height: 0; overflow: auto; padding: 22px 24px; }
.catalog-details-description { margin: 0 0 22px; color: var(--quiet); line-height: 1.65; white-space: pre-wrap; overflow-wrap: anywhere; }
.catalog-details-state { display: flex; align-items: center; gap: 10px; margin-bottom: 16px; color: var(--muted); }
.catalog-details-facts { display: grid; grid-template-columns: repeat(auto-fit, minmax(145px, 1fr)); gap: 10px; }
.catalog-details-facts > div { display: grid; gap: 5px; padding: 12px; border: 1px solid var(--line); border-radius: calc(var(--radius-card, 7px) * 1.857); background: var(--panel-soft); }
.catalog-details-facts span, .catalog-details-section h3 { color: var(--muted); font-size: .75rem; letter-spacing: .02em; }
.catalog-details-facts strong { overflow-wrap: anywhere; }
.catalog-details-hash { grid-column: 1 / -1; }
.catalog-details-hash strong { font-family: var(--font-mono); font-size: .72rem; }
.catalog-details-links { display: flex; flex-wrap: wrap; gap: 12px; margin-top: 10px; }
.catalog-details-links a { color: var(--accent); font-size: .82rem; }
.catalog-details-section { margin-top: 22px; }
.catalog-details-section h3 { margin: 0 0 10px; }
.catalog-details-gallery-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 225px), 1fr)); gap: 10px; }
.catalog-details-gallery-grid figure { min-width: 0; overflow: hidden; margin: 0; border: 1px solid var(--line); border-radius: calc(var(--radius-control, 10px) * 1.2); background: var(--panel-soft); }
.catalog-details-gallery-grid img { display: block; width: 100%; aspect-ratio: 16 / 9; object-fit: cover; background: var(--panel-raised); }
.catalog-details-gallery-grid figcaption { display: -webkit-box; overflow: hidden; padding: 8px 10px; color: var(--muted); font-size: .75rem; line-height: 1.4; -webkit-box-orient: vertical; -webkit-line-clamp: 2; }
.catalog-details-section ul { display: grid; gap: 7px; margin: 0; padding-left: 20px; }
.catalog-details-section li { overflow-wrap: anywhere; }
.catalog-details-section li small { color: var(--muted); }
.catalog-details-chips { display: flex; flex-wrap: wrap; gap: 7px; }
.catalog-details-chips span { padding: 5px 9px; border: 1px solid var(--line); border-radius: 999px; background: var(--panel-soft); color: var(--quiet); font-size: .78rem; }
.catalog-details-note { margin: 9px 0 0; color: var(--muted); font-size: .82rem; line-height: 1.5; }
.catalog-details-section > .custom-select + .catalog-details-chips { margin-top: 10px; }
.catalog-details-footer { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 16px 24px; border-top: 1px solid var(--line); color: var(--muted); font-size: .84rem; }
@media (max-width: 600px) { .catalog-details-scrim { align-items: end; padding: 8px; } .catalog-details-dialog { max-height: 92vh; border-radius: min(calc(var(--radius-card, 7px) * 2.714), 26px); } .catalog-details-header, .catalog-details-body { padding: 18px; } .catalog-details-footer { padding: 14px 18px; } .catalog-details-footer span { max-width: 46%; } .catalog-details-gallery-grid { grid-template-columns: 1fr; } }
</style>
