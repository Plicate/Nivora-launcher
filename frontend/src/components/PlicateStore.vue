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
const source = ref<'live' | 'default'>('live')
const filter = ref<'all' | 'theme' | 'plugin'>('all')
const query = ref('')
const activeItemId = ref('')
const installedById = computed(() => {
  const themes = props.installedThemes.map((item) => ({ ...item, kind: 'theme' as const }))
  const plugins = props.installedPlugins.map((item) => ({ ...item, kind: 'plugin' as const }))
  return new Map([...themes, ...plugins].map((item) => [`${item.kind}:${item.id}`, item]))
})
const visibleItems = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase()
  return items.value.filter((item) => (filter.value === 'all' || item.kind === filter.value)
    && (!needle || `${item.name} ${item.author} ${item.description} ${item.id}`.toLocaleLowerCase().includes(needle)))
})

async function refresh() {
  error.value = ''
  if (!props.api) {
    if (useFallbackCatalog()) return
    error.value = 'Адрес Plicate API не настроен для этой сборки.'
    return
  }
  loading.value = true
  try {
    items.value = await props.api.listCatalog()
    source.value = 'live'
  } catch (cause) {
    if (useFallbackCatalog()) return
    error.value = cause instanceof Error ? cause.message : 'Не удалось загрузить Plicate Store.'
  } finally { loading.value = false }
}

function useFallbackCatalog() {
  const presets = presetStoreItems(props.fallbackThemes || [])
  if (!presets.length) return false
  items.value = presets
  source.value = 'default'
  return true
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
    <header class="store-hero">
      <span class="store-mark"><Icon name="archive" :size="21" /></span>
      <div class="store-hero-copy"><span class="store-kicker">Экосистема Plicate</span><h2>Маркет Plicate</h2><p>Темы и плагины, которые устанавливаются прямо в Nivora Launcher.</p></div>
      <button class="button button--quiet" type="button" :disabled="loading" @click="void refresh()"><Icon name="refresh" :size="14" />{{ loading ? 'Обновляю…' : 'Обновить' }}</button>
    </header>

    <div class="store-toolbar">
      <label class="store-search"><Icon name="search" :size="16" /><input v-model="query" type="search" aria-label="Поиск в Магазине Plicate" placeholder="Найти тему или плагин" /></label>
      <div class="store-filters" role="tablist" aria-label="Тип ресурса">
        <button v-for="option in [{ id: 'all', label: 'Все' }, { id: 'theme', label: 'Темы' }, { id: 'plugin', label: 'Плагины' }]" :key="option.id" class="button button--small" :class="filter === option.id ? 'button--accent' : 'button--quiet'" type="button" role="tab" :aria-selected="filter === option.id" @click="filter = option.id as 'all' | 'theme' | 'plugin'">{{ option.label }}</button>
      </div>
    </div>

    <p v-if="source === 'default' && !loading" class="store-note" role="status">Показаны встроенные темы: Plicate API недоступен. <button class="text-button" type="button" :disabled="loading" @click="void refresh()">Обновить</button></p>

    <div v-if="error" class="store-state store-state--error" role="alert"><Icon name="cloud-off" :size="19" /><strong>Маркет пока недоступен</strong><span>{{ error }}</span><button class="button button--small button--quiet" type="button" @click="void refresh()">Повторить</button></div>
    <div v-else-if="loading && !items.length" class="store-state" role="status"><span class="status-dot status-dot--busy" />Загружаю каталог Plicate…</div>
    <div v-else-if="!visibleItems.length" class="store-state"><Icon name="archive" :size="21" /><strong>{{ items.length ? 'Ничего не найдено' : 'В каталоге пока пусто' }}</strong><span>{{ items.length ? 'Измените запрос или выберите другой раздел.' : 'Новые публичные темы и плагины появятся здесь после публикации.' }}</span></div>
    <div v-else class="store-grid">
      <article v-for="item in visibleItems" :key="`${item.kind}:${item.id}`" class="store-card">
        <div class="store-card-visual">
          <img v-if="item.screenshots?.[0]?.url && api" :src="screenshotUrl(item.screenshots[0].url)" :alt="`Предпросмотр: ${item.name}`" loading="lazy" />
          <span v-else class="store-card-icon"><Icon :name="item.kind === 'theme' ? 'palette' : 'plug'" :size="27" /></span>
          <span class="store-kind">{{ item.kind === 'theme' ? 'ТЕМА' : 'ПЛАГИН' }}</span>
        </div>
        <div class="store-card-body">
          <div class="store-card-title"><h3>{{ item.name }}</h3><span>v{{ item.version }}</span></div>
          <p class="store-card-author">{{ item.author }}</p>
          <p class="store-card-description">{{ item.description || 'Описание не добавлено.' }}</p>
          <footer class="store-card-footer"><span v-if="isPreset(item)" class="installed-label installed-label--muted"><Icon name="palette" :size="13" />Встроенная тема</span><span v-else-if="installed(item)" class="installed-label"><Icon name="check" :size="13" />{{ canUpdate(item) ? `Установлена v${installed(item)?.version || '—'}` : 'Установлено' }}</span><span v-else class="installed-label installed-label--muted">{{ item.kind === 'theme' ? 'Оформление лаунчера' : 'Расширение интерфейса' }}</span><button class="button button--small" :class="!isPreset(item) && installed(item) && !canUpdate(item) ? 'button--quiet' : 'button--accent'" type="button" :disabled="actionDisabled(item)" @click="void install(item)"><Icon :name="isPreset(item) ? 'palette' : installed(item) && canUpdate(item) ? 'refresh' : installed(item) ? 'check' : 'download'" :size="13" />{{ actionLabel(item) }}</button></footer>
        </div>
      </article>
    </div>
  </section>
</template>

<style scoped>
.plicate-store { display: grid; gap: 14px; }
.store-hero { display: flex; align-items: center; gap: 13px; padding: 18px 20px; border: 1px solid var(--line); border-radius: var(--radius-card, 12px); background: linear-gradient(115deg, color-mix(in srgb, var(--blue) 10%, var(--panel)), var(--panel)); }
.store-mark { display: grid; width: 42px; height: 42px; flex: 0 0 auto; place-items: center; border: 1px solid color-mix(in srgb, var(--blue) 30%, var(--line)); border-radius: 12px; background: color-mix(in srgb, var(--blue) 14%, var(--panel-raised)); color: var(--blue-bright); }
.store-hero-copy { flex: 1; min-width: 0; }
.store-kicker { color: var(--blue-bright); font-size: 10px; font-weight: 700; letter-spacing: .08em; text-transform: uppercase; }
.store-hero h2 { margin: 2px 0 0; font-size: 20px; letter-spacing: -.035em; }
.store-hero p { margin: 4px 0 0; color: var(--muted); font-size: 12px; }
.store-toolbar { display: flex; flex-wrap: wrap; align-items: center; gap: 9px; }
.store-note { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; margin: 0; padding: 9px 12px; border: 1px solid var(--line); border-radius: var(--radius-control, 10px); color: var(--muted); font-size: 12px; }
.store-search { display: flex; min-width: min(100%, 260px); height: 40px; flex: 1; align-items: center; gap: 9px; padding: 0 11px; border: 1px solid var(--line); border-radius: var(--radius-control); background: var(--panel); color: var(--muted); }
.store-search input { width: 100%; min-width: 0; border: 0; outline: 0; background: transparent; color: var(--text); font-size: 12px; }
.store-filters { display: flex; gap: 5px; }
.store-state { display: flex; min-height: 160px; flex-direction: column; align-items: center; justify-content: center; gap: 9px; padding: 22px; border: 1px dashed var(--line-strong); border-radius: var(--radius-card, 12px); color: var(--muted); text-align: center; font-size: 12px; }
.store-state strong { color: var(--text); font-size: 14px; }
.store-state--error { color: var(--danger); }
.store-state--error span { max-width: 560px; color: var(--muted); }
.store-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 248px), 1fr)); gap: 12px; }
.store-card { display: grid; overflow: hidden; border: 1px solid var(--line); border-radius: var(--radius-card, 12px); background: var(--panel); transition: border-color .16s ease, transform .16s ease; }
.store-card:hover { border-color: var(--line-strong, var(--line)); transform: translateY(-1px); }
.store-card-visual { position: relative; display: grid; min-height: 136px; place-items: center; overflow: hidden; background: linear-gradient(135deg, color-mix(in srgb, var(--blue) 12%, var(--panel-raised)), var(--panel)); }
.store-card-visual img { width: 100%; height: 100%; max-height: 168px; object-fit: cover; }
.store-card-icon { display: grid; width: 58px; height: 58px; place-items: center; border: 1px solid var(--line); border-radius: 17px; background: var(--panel); color: var(--blue-bright); }
.store-kind { position: absolute; top: 9px; left: 9px; padding: 3px 7px; border: 1px solid var(--line); border-radius: 999px; background: var(--panel); color: var(--muted); font-size: 9px; font-weight: 700; letter-spacing: .05em; }
.store-card-body { display: grid; gap: 5px; padding: 13px; }
.store-card-title { display: flex; align-items: start; justify-content: space-between; gap: 10px; }
.store-card-title h3 { margin: 0; font-size: 14px; font-weight: 650; }
.store-card-title > span { flex: 0 0 auto; color: var(--muted); font-size: 10px; }
.store-card-author { margin: 0; color: var(--blue-bright); font-size: 10px; }
.store-card-description { display: -webkit-box; min-height: 38px; margin: 3px 0 5px; overflow: hidden; color: var(--muted); font-size: 11px; line-height: 1.5; -webkit-box-orient: vertical; -webkit-line-clamp: 2; }
.store-card-footer { display: flex; align-items: center; justify-content: space-between; gap: 8px; padding-top: 9px; border-top: 1px solid var(--line); }
.installed-label { display: inline-flex; min-width: 0; align-items: center; gap: 4px; color: var(--green); font-size: 10px; }
.installed-label--muted { color: var(--muted); }
@media (max-width: 520px) { .store-hero { align-items: flex-start; flex-wrap: wrap; padding: 14px; } .store-hero-copy { flex-basis: calc(100% - 58px); } .store-hero > button { margin-left: auto; } .store-toolbar { align-items: stretch; } .store-search { flex-basis: 100%; } .store-filters { justify-content: flex-end; } }
</style>
