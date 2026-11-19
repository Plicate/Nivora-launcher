<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import Icon from './Icon.vue'
import { compareStoreVersions, isPresetStoreItem, presetStoreItems, type PlicateStoreApi, type PlicateStoreItem } from '../services/plicateStore'
import type { LauncherThemePreset } from '../types'

type InstalledResource = { id: string; version?: string; name?: string }
const props = defineProps<{
  api?: PlicateStoreApi
  installedThemes: InstalledResource[]
  installedPlugins: InstalledResource[]
  fallbackThemes?: LauncherThemePreset[]
  busy?: boolean
}>()
const emit = defineEmits<{
  installTheme: [payload: { name: string; data: string; replace: boolean }]
  installPlugin: [payload: { name: string; data: string; replace: boolean }]
  applyTheme: [theme: LauncherThemePreset]
  toast: [message: string]
}>()

const items = ref<PlicateStoreItem[]>([])
const loading = ref(false)
const error = ref('')
const filter = ref<'all' | 'theme' | 'plugin'>('all')
const query = ref('')
const activeItemId = ref('')
const installedById = computed(() => {
  const themes = props.installedThemes.map((item) => ({ ...item, kind: 'theme' as const }))
  const plugins = props.installedPlugins.map((item) => ({ ...item, kind: 'plugin' as const }))
  return new Map([...themes, ...plugins].map((item) => [`${item.kind}:${item.id}`, item]))
})
const matches = (item: PlicateStoreItem) => {
  const needle = query.value.trim().toLocaleLowerCase()
  return (filter.value === 'all' || item.kind === filter.value)
    && (!needle || `${item.name} ${item.author} ${item.description} ${item.id}`.toLocaleLowerCase().includes(needle))
}
const visibleItems = computed(() => items.value.filter(matches))
const builtInItems = computed(() => presetStoreItems(props.fallbackThemes || []))
const builtInVisible = computed(() => builtInItems.value.filter(matches))

async function refresh() {
  error.value = ''
  if (!props.api) {
    error.value = 'Адрес Plicate API не настроен для этой сборки.'
    return
  }
  loading.value = true
  try {
    items.value = await props.api.listCatalog()
  } catch (cause) {
    error.value = cause instanceof Error ? cause.message : 'Не удалось загрузить каталог Plicate.'
  } finally { loading.value = false }
}

function themeVars(item: PlicateStoreItem) {
  return {
    '--theme-bg': item.background || '#0f1115',
    '--theme-panel': item.panel || '#15181d',
    '--theme-soft': item.panel_soft || '#20242c',
    '--theme-accent': item.accent || '#3d8bfd',
    '--theme-strong': item.accent_strong || '#6ea8fe',
  }
}

function isPreset(item: PlicateStoreItem) {
  return isPresetStoreItem(item)
}

onMounted(() => { void refresh() })

function installed(item: PlicateStoreItem) {
  return installedById.value.get(`${item.kind}:${item.id}`)
}
function canUpdate(item: PlicateStoreItem) {
  const current = installed(item)
  return Boolean(current && current.version && compareStoreVersions(item.version, current.version) > 0)
}
function screenshotUrl(path: string) {
  try { return props.api?.assetUrl(path) || '' } catch { return '' }
}

function actionLabel(item: PlicateStoreItem) {
  if (isPreset(item)) return 'Применить'
  if (activeItemId.value === item.id) return 'Загружаю…'
  if (!installed(item)) return 'Установить'
  return canUpdate(item) ? 'Обновить' : 'Установлено'
}

function actionDisabled(item: PlicateStoreItem) {
  return Boolean(activeItemId.value) || Boolean(props.busy) || (!isPreset(item) && Boolean(installed(item)) && !canUpdate(item))
}

async function install(item: PlicateStoreItem) {
  if (isPreset(item)) {
    const theme = (props.fallbackThemes || []).find((candidate) => candidate.id === item.id)
    if (theme) emit('applyTheme', theme)
    else emit('toast', `Тема «${item.name}» недоступна в этой сборке.`)
    return
  }
  if (activeItemId.value || props.busy || (installed(item) && !canUpdate(item))) return
  activeItemId.value = item.id
  try {
    const archive = await props.api!.download(item)
    const payload = { ...archive, replace: Boolean(installed(item)) }
    if (item.kind === 'theme') emit('installTheme', payload)
    else emit('installPlugin', payload)
  } catch (cause) {
    emit('toast', cause instanceof Error ? cause.message : `Не удалось загрузить «${item.name}».`)
  } finally { activeItemId.value = '' }
}
</script>

<template>
  <section class="plicate-store">
    <header class="store-header">
      <div class="store-header-copy"><h2>Маркет</h2><p>Темы и плагины, которые устанавливаются прямо в Nivora Launcher.</p></div>
      <button class="button button--quiet" type="button" :disabled="loading" @click="void refresh()"><Icon name="refresh" :size="14" />{{ loading ? 'Обновляю…' : 'Обновить' }}</button>
    </header>

    <div class="store-toolbar">
      <label class="store-search"><Icon name="search" :size="16" /><input v-model="query" type="search" aria-label="Поиск в Магазине Plicate" placeholder="Найти тему или плагин" /></label>
      <div class="store-filters" role="tablist" aria-label="Тип ресурса">
        <button v-for="option in [{ id: 'all', label: 'Все' }, { id: 'theme', label: 'Темы' }, { id: 'plugin', label: 'Плагины' }]" :key="option.id" class="button button--small" :class="filter === option.id ? 'button--accent' : 'button--quiet'" type="button" role="tab" :aria-selected="filter === option.id" @click="filter = option.id as 'all' | 'theme' | 'plugin'">{{ option.label }}</button>
      </div>
    </div>

    <p v-if="loading && !items.length && !error" class="store-line" role="status">Загружаю каталог Plicate…</p>
    <p v-else-if="error" class="store-line" role="alert">{{ error }} <button class="text-button" type="button" :disabled="loading" @click="void refresh()">Повторить</button></p>
    <p v-else-if="!items.length" class="store-line" role="status">В каталоге Plicate пока ничего нет. Темы и плагины появятся здесь после публикации.</p>
    <p v-else-if="!visibleItems.length" class="store-line">Ничего не найдено по запросу.</p>
    <div v-else class="store-grid">
      <article v-for="item in visibleItems" :key="`${item.kind}:${item.id}`" class="store-card">
        <div class="store-card-visual">
          <img v-if="item.screenshots?.[0]?.url && api" :src="screenshotUrl(item.screenshots[0].url)" :alt="`Предпросмотр: ${item.name}`" loading="lazy" />
          <div v-else-if="item.kind === 'theme'" class="store-theme-preview" :style="themeVars(item)" aria-hidden="true">
            <span class="preview-sidebar"><i /><i /><i /></span>
            <span class="preview-main"><span class="preview-card" /><span class="preview-card" /><span class="preview-button" /></span>
          </div>
          <span v-else class="store-card-icon"><Icon name="plug" :size="27" /></span>
          <span class="store-kind">{{ item.kind === 'theme' ? 'Тема' : 'Плагин' }}</span>
        </div>
        <div class="store-card-body">
          <div class="store-card-title"><h3>{{ item.name }}</h3><span>v{{ item.version }}</span></div>
          <p class="store-card-author">{{ item.author }}</p>
          <p class="store-card-description">{{ item.description || 'Описание не добавлено.' }}</p>
          <footer class="store-card-footer"><span v-if="isPreset(item)" class="installed-label installed-label--muted"><Icon name="palette" :size="13" />Встроенная тема</span><span v-else-if="installed(item)" class="installed-label"><Icon name="check" :size="13" />{{ canUpdate(item) ? `Установлена v${installed(item)?.version || '—'}` : 'Установлено' }}</span><span v-else class="installed-label installed-label--muted">{{ item.kind === 'theme' ? 'Оформление лаунчера' : 'Расширение интерфейса' }}</span><button class="button button--small" :class="!isPreset(item) && installed(item) && !canUpdate(item) ? 'button--quiet' : 'button--accent'" type="button" :disabled="actionDisabled(item)" @click="void install(item)"><Icon :name="isPreset(item) ? 'palette' : installed(item) && canUpdate(item) ? 'refresh' : installed(item) ? 'check' : 'download'" :size="13" />{{ actionLabel(item) }}</button></footer>
        </div>
      </article>
    </div>

    <section v-if="builtInVisible.length" class="store-builtins" aria-labelledby="store-builtins-title">
      <h3 id="store-builtins-title">Встроенные темы</h3>
      <div class="store-grid">
        <article v-for="item in builtInVisible" :key="`builtin:${item.id}`" class="store-card">
          <div class="store-card-visual">
            <div class="store-theme-preview" :style="themeVars(item)" aria-hidden="true">
              <span class="preview-sidebar"><i /><i /><i /></span>
              <span class="preview-main"><span class="preview-card" /><span class="preview-card" /><span class="preview-button" /></span>
            </div>
            <span class="store-kind">Тема</span>
          </div>
          <div class="store-card-body">
            <div class="store-card-title"><h3>{{ item.name }}</h3><span>v{{ item.version }}</span></div>
            <p class="store-card-author">{{ item.author }}</p>
            <p class="store-card-description">{{ item.description || 'Описание не добавлено.' }}</p>
            <footer class="store-card-footer"><span class="installed-label installed-label--muted"><Icon name="palette" :size="13" />Встроенная тема</span><button class="button button--small button--accent" type="button" :disabled="actionDisabled(item)" @click="void install(item)"><Icon name="palette" :size="13" />Применить</button></footer>
          </div>
        </article>
      </div>
    </section>
  </section>
</template>

<style scoped>
.plicate-store { display: grid; gap: 14px; }
.store-header { display: flex; align-items: flex-end; gap: 12px; }
.store-header-copy { flex: 1; min-width: 0; }
.store-header h2 { margin: 0 0 6px; font-size: 20px; font-weight: 650; letter-spacing: -.02em; }
.store-header p { margin: 0; color: var(--muted); font-size: 13px; line-height: 1.5; }
.store-toolbar { display: flex; flex-wrap: wrap; align-items: center; gap: 9px; }
.store-search { display: flex; min-width: min(100%, 260px); height: 40px; flex: 1; align-items: center; gap: 9px; padding: 0 11px; border: 1px solid var(--line); border-radius: var(--radius-control); background: var(--panel); color: var(--muted); }
.store-search input { width: 100%; min-width: 0; border: 0; outline: 0; background: transparent; color: var(--text); font-size: 12px; }
.store-filters { display: flex; gap: 5px; }
.store-line { margin: 0; color: var(--muted); font-size: 13px; }
.store-builtins { display: grid; gap: 10px; }
.store-builtins h3 { margin: 0; color: var(--muted); font-size: 13px; font-weight: 600; }
.store-theme-preview { display: flex; width: 100%; height: 100%; min-height: 136px; gap: 6px; padding: 12px; background: var(--theme-bg, #0f1115); }
.preview-sidebar { display: grid; width: 44px; align-content: start; gap: 5px; padding: 7px; border-radius: 6px; background: var(--theme-panel, #15181d); }
.preview-sidebar i { height: 5px; border-radius: 3px; background: var(--theme-soft, #20242c); }
.preview-sidebar i:first-child { background: var(--theme-accent, #3d8bfd); }
.preview-main { display: grid; flex: 1; align-content: start; gap: 6px; }
.preview-card { height: 24px; border-radius: 6px; background: var(--theme-panel, #15181d); }
.preview-button { width: 62px; height: 11px; margin-top: 2px; border-radius: 999px; background: var(--theme-accent, #3d8bfd); }
.store-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 248px), 1fr)); gap: 12px; }
.store-card { display: grid; overflow: hidden; border: 1px solid var(--line); border-radius: var(--radius-card, 12px); background: var(--panel); transition: border-color .16s ease, transform .16s ease; }
.store-card:hover { border-color: var(--line-strong, var(--line)); transform: translateY(-1px); }
.store-card-visual { position: relative; display: grid; min-height: 136px; place-items: center; overflow: hidden; background: var(--panel-raised, var(--panel)); }
.store-card-visual img { width: 100%; height: 100%; max-height: 168px; object-fit: cover; }
.store-card-icon { display: grid; width: 58px; height: 58px; place-items: center; border: 1px solid var(--line); border-radius: 17px; background: var(--panel); color: var(--blue-bright); }
.store-kind { position: absolute; top: 9px; left: 9px; padding: 3px 7px; border: 1px solid var(--line); border-radius: 999px; background: var(--panel); color: var(--muted); font-size: 10px; }
.store-card-body { display: grid; gap: 5px; padding: 13px; }
.store-card-title { display: flex; align-items: start; justify-content: space-between; gap: 10px; }
.store-card-title h3 { margin: 0; font-size: 14px; font-weight: 650; }
.store-card-title > span { flex: 0 0 auto; color: var(--muted); font-size: 10px; }
.store-card-author { margin: 0; color: var(--muted); font-size: 10px; }
.store-card-description { display: -webkit-box; min-height: 38px; margin: 3px 0 5px; overflow: hidden; color: var(--muted); font-size: 11px; line-height: 1.5; -webkit-box-orient: vertical; -webkit-line-clamp: 2; }
.store-card-footer { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding-top: 9px; border-top: 1px solid var(--line); }
.installed-label { display: inline-flex; min-width: 0; align-items: center; gap: 4px; color: var(--green); font-size: 10px; }
.installed-label--muted { color: var(--muted); }
@media (max-width: 520px) { .store-header { align-items: flex-start; flex-wrap: wrap; } .store-header > button { margin-left: auto; } .store-toolbar { align-items: stretch; } .store-search { flex-basis: 100%; } .store-filters { justify-content: flex-end; } }
</style>
