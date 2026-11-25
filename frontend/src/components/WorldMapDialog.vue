<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { bridge } from '../services/bridge'
import Icon from './Icon.vue'

const props = defineProps<{ instance: string; worldFolder: string; worldTitle: string }>()
const emit = defineEmits<{ (event: 'close'): void }>()

type PreviewResult = Awaited<ReturnType<typeof bridge.worldPreview>>
type WorldHit = { name: string; x: number; y: number; z: number }
type WorldMarker = { name: string; x: number; y: number; z: number }
type MarkerKind = 'structure' | 'biome' | 'ore' | 'marker' | 'player' | 'bed' | 'death'
type FlatMarker = {
  key: string
  name: string
  x: number
  y?: number
  z: number
  kind: MarkerKind
  system: boolean
}

const KIND_LABELS: Record<MarkerKind, string> = {
  structure: 'Структуры',
  biome: 'Биомы',
  ore: 'Руды',
  marker: 'Метки',
  player: 'Игрок',
  bed: 'Кровать',
  death: 'Смерти',
}

const mode = ref<'map' | 'iso'>('map')
const previewPath = ref('')
const imgError = ref('')
const meta = ref<PreviewResult | null>(null)
// точное положение спавна на итоговой картинке: бэкенд присылает его в пикселях
// изображения, маркер ставится по процентам, а не по центру картинки
const spawnStyle = computed(() => {
  const metaValue = meta.value
  const px = metaValue?.spawn_px
  if (!metaValue || !px) return null
  const left = (px.x / metaValue.width) * 100
  const top = (px.y / metaValue.height) * 100
  if (left < -2 || left > 102 || top < -2 || top > 102) return null
  return { left: `${left}%`, top: `${top}%` }
})
const busy = ref(false)
const error = ref('')
const zoom = ref(1)
const panX = ref(0)
const panY = ref(0)
const viewport = ref<HTMLElement | null>(null)
const image = ref<HTMLImageElement | null>(null)
let dragging = false
let lastX = 0
let lastY = 0
let downX = 0
let downY = 0
let draggedFar = false

const query = ref('')
const hitKinds = ref<Record<'structure' | 'biome' | 'ore', boolean>>({
  structure: true,
  biome: true,
  ore: true,
})
const searchHits = ref<{ structure: WorldHit[]; biome: WorldHit[]; ore: WorldHit[] }>({
  structure: [],
  biome: [],
  ore: [],
})
const userMarkers = ref<WorldMarker[]>([])
const playerPos = ref<{ x: number; y: number; z: number } | null>(null)
const bedPos = ref<{ x: number; y: number; z: number } | null>(null)
const deathMarkers = ref<{ name: string; x: number; y: number; z: number }[]>([])
const mapOrigin = ref<{ x: number; z: number } | null>(null)
const markMode = ref(false)
const selected = ref<{ marker: FlatMarker; left: number; top: number } | null>(null)
const searchBusy = ref(false)

const worldToPx = (x: number, z: number) => {
  const metaValue = meta.value
  if (!metaValue) return null
  const block = (metaValue as { block_px?: number }).block_px ?? 16
  // после обрезки карты до контента спавн может остаться за кадром —
  // привязка идёт к левому верхнему пикселю, а не к спавну
  if (mapOrigin.value) {
    return { x: (x - mapOrigin.value.x) * block, y: (z - mapOrigin.value.z) * block }
  }
  if (!metaValue.spawn_px) return null
  return {
    x: metaValue.spawn_px.x + (x - metaValue.spawn.x) * block,
    y: metaValue.spawn_px.y + (z - metaValue.spawn.z) * block,
  }
}
const pxToWorld = (px: number, py: number) => {
  const metaValue = meta.value
  if (!metaValue) return null
  const block = (metaValue as { block_px?: number }).block_px ?? 16
  if (mapOrigin.value) {
    return { x: mapOrigin.value.x + px / block, z: mapOrigin.value.z + py / block }
  }
  if (!metaValue.spawn_px) return null
  return {
    x: metaValue.spawn.x + (px - metaValue.spawn_px.x) / block,
    z: metaValue.spawn.z + (py - metaValue.spawn_px.y) / block,
  }
}

const visibleMarkers = computed<FlatMarker[]>(() => {
  const metaValue = meta.value
  if (!metaValue || mode.value !== 'map' || (!metaValue.spawn_px && !mapOrigin.value)) return []
  const needle = query.value.trim().toLowerCase()
  const flat: FlatMarker[] = []
  const push = (kind: MarkerKind, hit: WorldHit, system: boolean, enabled: boolean) => {
    if (!enabled) return
    if (needle && !hit.name.toLowerCase().includes(needle)) return
    flat.push({
      key: `${kind}:${hit.name}:${hit.x}:${hit.z}`,
      name: hit.name,
      x: hit.x,
      y: hit.y,
      z: hit.z,
      kind,
      system,
    })
  }
  searchHits.value.structure.forEach((hit) => push('structure', hit, true, hitKinds.value.structure))
  searchHits.value.biome.forEach((hit) => push('biome', hit, true, hitKinds.value.biome))
  searchHits.value.ore.forEach((hit) => push('ore', hit, true, hitKinds.value.ore))
  userMarkers.value.forEach((marker) => push('marker', marker, false, true))
  if (playerPos.value)
    push('player', { name: 'Игрок', ...playerPos.value }, true, true)
  if (bedPos.value) push('bed', { name: 'Кровать', ...bedPos.value }, true, true)
  deathMarkers.value.forEach((hit) => push('death', hit, true, true))
  return flat.slice(0, 250)
})

const markerStyle = (marker: FlatMarker) => {
  const target = worldToPx(marker.x, marker.z)
  const metaValue = meta.value
  if (!target || !metaValue) return null
  const left = (target.x / metaValue.width) * 100
  const top = (target.y / metaValue.height) * 100
  if (left < -2 || left > 102 || top < -2 || top > 102) return null
  return { left: `${left}%`, top: `${top}%` }
}

async function runSearch() {
  searchBusy.value = true
  try {
    const result = await bridge.worldSearch(props.instance, props.worldFolder)
    searchHits.value = {
      structure: result.structures ?? [],
      biome: result.biomes ?? [],
      ore: result.ores ?? [],
    }
  } catch {
    searchHits.value = { structure: [], biome: [], ore: [] }
  } finally {
    searchBusy.value = false
  }
}

async function loadMarkers() {
  try {
    const result = await bridge.worldMarkersGet(props.instance, props.worldFolder)
    userMarkers.value = result.markers ?? []
    playerPos.value = result.player ?? null
    bedPos.value = result.bed ?? null
  } catch {
    userMarkers.value = []
  }
}

async function saveMarkers() {
  try {
    await bridge.worldMarkersSet(props.instance, props.worldFolder, userMarkers.value)
  } catch {
    /* метки остаются только в текущей сессии */
  }
}

function addMarkerAt(event: PointerEvent) {
  const target = eventWorld(event)
  if (!target) return
  userMarkers.value.push({
    name: `Метка ${userMarkers.value.length + 1}`,
    x: Math.round(target.x),
    y: 0,
    z: Math.round(target.z),
  })
  void saveMarkers()
}

function eventWorld(event: PointerEvent) {
  const img = image.value
  const metaValue = meta.value
  if (!img || !metaValue) return null
  const rect = img.getBoundingClientRect()
  if (!rect.width || !rect.height) return null
  const px = ((event.clientX - rect.left) / rect.width) * metaValue.width
  const py = ((event.clientY - rect.top) / rect.height) * metaValue.height
  return pxToWorld(px, py)
}

function onMarkerClick(event: PointerEvent, marker: FlatMarker) {
  event.stopPropagation()
  const box = viewport.value?.getBoundingClientRect()
  if (!box) return
  selected.value = {
    marker,
    left: Math.min(Math.max(event.clientX - box.left, 12), Math.max(box.width - 220, 12)),
    top: Math.min(Math.max(event.clientY - box.top, 12), Math.max(box.height - 150, 12)),
  }
}

async function copyTeleport(marker: FlatMarker) {
  const y = marker.y ? Math.round(marker.y) : null
  const command = y === null ? `/tp ${marker.x} ~ ${marker.z}` : `/tp ${marker.x} ${y} ${marker.z}`
  try {
    await navigator.clipboard.writeText(command)
  } catch {
    /* буфер недоступен */
  }
  selected.value = null
}

function removeMarker(marker: FlatMarker) {
  userMarkers.value = userMarkers.value.filter(
    (item) => !(item.x === marker.x && item.z === marker.z && item.name === marker.name),
  )
  selected.value = null
  void saveMarkers()
}

function jumpTo(marker: FlatMarker) {
  const target = worldToPx(marker.x, marker.z)
  const img = image.value
  const box = viewport.value?.getBoundingClientRect()
  if (!target || !img || !box) return
  const fit = Math.min((box.width - 56) / img.naturalWidth, (box.height - 56) / img.naturalHeight)
  zoom.value = Math.min(4, Math.max(1.2, fit))
  panX.value = -(target.x / img.naturalWidth - 0.5) * img.naturalWidth * zoom.value
  panY.value = -(target.y / img.naturalHeight - 0.5) * img.naturalHeight * zoom.value
  selected.value = null
}

function resetView() {
  const box = viewport.value?.getBoundingClientRect()
  const img = image.value
  if (!box || !img || !img.naturalWidth) {
    zoom.value = 1
    panX.value = 0
    panY.value = 0
    return
  }
  const fit = Math.min((box.width - 56) / img.naturalWidth, (box.height - 56) / img.naturalHeight)
  zoom.value = Math.min(1.6, Math.max(0.25, fit))
  panX.value = 0
  panY.value = 0
}

async function build() {
  if (busy.value) return
  busy.value = true
  error.value = ''
  try {
    const result = await bridge.worldPreview(props.instance, props.worldFolder, mode.value)
    meta.value = result
    mapOrigin.value = result.map_origin ?? null
    deathMarkers.value = (result.deaths ?? []).map((death) => ({
      name: death.cause ? `Смерть: ${death.cause}` : 'Смерть',
      x: death.x,
      y: death.y,
      z: death.z,
    }))
    // картинка приходит прямо с бэкенда (base64): не зависит от файлового протокола,
    // который на десктопе мог молча отказывать
    previewPath.value = result.png_base64
      ? `data:image/png;base64,${result.png_base64}`
      : bridge.fileUrl(result.png_path)
    imgError.value = ''
    await nextTick()
    window.setTimeout(resetView, 60)
  } catch (caught) {
    error.value = caught instanceof Error ? caught.message : String(caught)
  } finally {
    busy.value = false
  }
}

function onImgError() {
  imgError.value = `Изображение карты не загрузилось (${previewPath.value.slice(0, 80)})`
}

function zoomBy(factor: number) {
  zoom.value = Math.min(12, Math.max(0.25, zoom.value * factor))
}

function onWheel(event: WheelEvent) {
  event.preventDefault()
  const rect = viewport.value?.getBoundingClientRect()
  if (!rect) return
  const cx = event.clientX - rect.left - rect.width / 2
  const cy = event.clientY - rect.top - rect.height / 2
  const before = zoom.value
  const after = Math.min(12, Math.max(0.25, before * (event.deltaY < 0 ? 1.15 : 1 / 1.15)))
  panX.value = cx - ((cx - panX.value) * after) / before
  panY.value = cy - ((cy - panY.value) * after) / before
  zoom.value = after
}

function onPointerDown(event: PointerEvent) {
  if (event.button !== 0) return
  dragging = true
  draggedFar = false
  downX = event.clientX
  downY = event.clientY
  lastX = event.clientX
  lastY = event.clientY
  event.preventDefault()
}

function onPointerMove(event: PointerEvent) {
  if (!dragging) return
  if (Math.abs(event.clientX - downX) + Math.abs(event.clientY - downY) > 5) draggedFar = true
  panX.value += event.clientX - lastX
  panY.value += event.clientY - lastY
  lastX = event.clientX
  lastY = event.clientY
}

function onPointerUp(event: PointerEvent) {
  const wasClick = dragging && !draggedFar
  dragging = false
  if (!wasClick) return
  if (markMode.value && mode.value === 'map') {
    addMarkerAt(event)
    return
  }
  if (selected.value) selected.value = null
}

function onKey(event: KeyboardEvent) {
  if (event.key === 'Escape') emit('close')
}

watch(mode, () => {
  selected.value = null
  if (previewPath.value || error.value) build()
})

onMounted(() => {
  window.addEventListener('keydown', onKey)
  window.addEventListener('pointermove', onPointerMove)
  window.addEventListener('pointerup', onPointerUp)
  window.addEventListener('pointercancel', onPointerUp)
  build()
  void runSearch()
  void loadMarkers()
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKey)
  window.removeEventListener('pointermove', onPointerMove)
  window.removeEventListener('pointerup', onPointerUp)
  window.removeEventListener('pointercancel', onPointerUp)
})
</script>

<template>
  <div class="wmap-scrim" role="presentation" @click.self="emit('close')">
    <section class="wmap-card" role="dialog" aria-modal="true" :aria-label="`Карта мира ${worldTitle}`">
      <header class="wmap-head">
        <div>
          <h2>Карта · {{ worldTitle }}</h2>
          <small class="wmap-hint">Колесо — масштаб, удерживайте кнопку мыши — перемещение, двойной клик — сброс</small>
        </div>
        <div class="wmap-head-actions">
          <span class="wmap-beta">
            <span class="wmap-beta-badge">BETA</span>
            <span
              class="wmap-beta-info"
              v-tooltip="'Функция в стадии разработки'"
            ><Icon name="info" :size="14" /></span>
          </span>
          <button class="button button--small" :class="{ 'button--accent': mode === 'map' }" type="button" @click="mode = 'map'">Карта</button>
          <button class="button button--small" :class="{ 'button--accent': mode === 'iso' }" type="button" @click="mode = 'iso'">Изометрия</button>
          <button class="icon-button" type="button" aria-label="Закрыть" @click="emit('close')"><Icon name="close" :size="16" /></button>
        </div>
      </header>
      <div class="wmap-search" :class="{ 'wmap-search--disabled': mode === 'iso' }">
        <div class="wmap-search-field">
          <Icon name="search" :size="14" />
          <input v-model="query" type="text" placeholder="Поиск по миру: структуры, биомы, руды…" />
        </div>
        <button
          v-for="(label, kind) in KIND_LABELS"
          v-show="kind !== 'marker' && kind !== 'player' && kind !== 'bed' && kind !== 'death'"
          :key="kind"
          class="wmap-chip"
          :class="{ 'wmap-chip--on': hitKinds[kind as 'structure' | 'biome' | 'ore'] }"
          type="button"
          @click="hitKinds[kind as 'structure' | 'biome' | 'ore'] = !hitKinds[kind as 'structure' | 'biome' | 'ore']"
        >{{ label }}</button>
        <button
          class="button button--small"
          :class="{ 'button--accent': markMode }"
          type="button"
          :disabled="mode !== 'map'"
          :title="mode === 'iso' ? 'Метки доступны в режиме «Карта»' : 'Поставить метку кликом по карте'"
          @click="markMode = !markMode"
        >Метка</button>
      </div>
      <div ref="viewport" class="wmap-viewport" :class="{ 'wmap-viewport--dragging': dragging, 'wmap-viewport--marking': markMode && mode === 'map' }" @wheel="onWheel" @pointerdown="onPointerDown" @dragstart.prevent @dblclick="resetView">
        <span v-if="busy && !previewPath" class="wmap-status">Строю карту…</span>
        <p v-else-if="error && !previewPath" class="wmap-error">{{ error }}</p>
        <template v-else-if="previewPath">
          <div class="wmap-stage" :style="{ transform: `translate(calc(-50% + ${panX}px), calc(-50% + ${panY}px)) scale(${zoom})` }">
            <img ref="image" :key="previewPath" :src="previewPath" :alt="`Карта мира ${worldTitle}`" class="wmap-img" :class="{ 'wmap-img--pixelated': mode === 'map' }" draggable="false" @error="onImgError" />
            <span v-if="imgError" class="wmap-error">{{ imgError }}</span>
            <span v-if="mode === 'map'" class="wmap-spawn" :style="spawnStyle ?? undefined" aria-hidden="true"></span>
            <template v-if="mode === 'map'">
              <button
                v-for="marker in visibleMarkers"
                :key="marker.key"
                class="wmap-marker"
                :class="`wmap-marker--${marker.kind}`"
                :style="markerStyle(marker) ?? undefined"
                type="button"
                :title="`${marker.name} · ${marker.x}, ${marker.z}`"
                @pointerdown.stop
                @pointerup.stop="onMarkerClick($event, marker)"
              ></button>
            </template>
          </div>
          <div v-if="mode === 'map' && query.trim()" class="wmap-results">
            <div v-if="!visibleMarkers.length" class="wmap-results-empty">Ничего не найдено</div>
            <button
              v-for="marker in visibleMarkers.slice(0, 60)"
              :key="`row-${marker.key}`"
              class="wmap-result"
              type="button"
              @click="jumpTo(marker)"
            >
              <span class="wmap-dot" :class="`wmap-dot--${marker.kind}`"></span>
              <span class="wmap-result-name">{{ marker.name }}</span>
              <span class="wmap-result-coords">{{ marker.x }}, {{ marker.z }}</span>
            </button>
          </div>
          <div
            v-if="selected"
            class="wmap-popup"
            :style="{ left: `${selected.left}px`, top: `${selected.top}px` }"
          >
            <strong class="wmap-popup-name">{{ selected.marker.name }}</strong>
            <small class="wmap-popup-coords">{{ selected.marker.x }}<template v-if="selected.marker.y">, {{ Math.round(selected.marker.y) }}</template>, {{ selected.marker.z }}</small>
            <div class="wmap-popup-actions">
              <button class="button button--small" type="button" @click="copyTeleport(selected.marker)">Скопировать /tp</button>
              <button v-if="!selected.marker.system" class="button button--small button--quiet" type="button" @click="removeMarker(selected.marker)">Удалить</button>
              <button class="button button--small button--outline" type="button" @click="jumpTo(selected.marker)">Показать</button>
            </div>
          </div>
        </template>
      </div>
      <footer class="wmap-foot">
        <button class="icon-button" type="button" aria-label="Приблизить" @click="zoomBy(1.25)"><Icon name="plus" :size="15" /></button>
        <button class="icon-button" type="button" aria-label="Отдалить" @click="zoomBy(1 / 1.25)"><Icon name="minus" :size="15" /></button>
        <button class="button button--small button--quiet" type="button" @click="resetView">Сбросить вид</button>
        <button class="button button--small button--outline" type="button" :disabled="busy" @click="build">{{ busy ? 'Строю…' : 'Обновить' }}</button>
        <span v-if="markMode && mode === 'map'" class="wmap-mark-hint">Клик по карте ставит метку</span>
        <small v-if="meta" class="wmap-meta">{{ meta.world }} · разобрано колонок: {{ meta.columns }} · спавн {{ meta.spawn.x }}, {{ meta.spawn.z }}<template v-if="searchBusy"> · поиск…</template></small>
        <small v-else-if="error" class="wmap-meta wmap-error">{{ error }}</small>
      </footer>
    </section>
  </div>
</template>

<style scoped>
.wmap-scrim {
  position: fixed;
  inset: 0;
  z-index: 90;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(0, 0, 0, 0.45);
  padding: 14px;
  animation: wmap-scrim-in 0.24s ease both;
}
.wmap-card {
  display: flex;
  flex-direction: column;
  gap: 10px;
  width: min(1900px, 97%);
  height: min(1060px, 97%);
  border-radius: var(--ds-radius-dialog, 16px);
  background: var(--ds-surface-1);
  border: 1px solid var(--ds-border);
  box-shadow: var(--ds-shadow-dialog, 0 24px 64px rgba(0, 0, 0, 0.35));
  padding: 14px 16px;
  animation: wmap-card-in 0.34s cubic-bezier(0.22, 1, 0.36, 1) both;
  --wmap-biome: var(--wmap-biome); --wmap-ore: var(--wmap-ore); --wmap-marker: var(--wmap-marker); --wmap-player: var(--wmap-player);
}
.wmap-head {
  animation: wmap-fade-up 0.4s cubic-bezier(0.22, 1, 0.36, 1) 0.1s both;
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}
.wmap-head h2 {
  margin: 0;
  font-size: 15.5px;
  font-weight: 650;
}
.wmap-hint {
  color: var(--muted, inherit);
  font-size: 11.5px;
}
.wmap-beta {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  margin-right: 4px;
  align-self: center;
}
.wmap-beta-badge {
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.08em;
  line-height: 1;
  padding: 3px 7px;
  border-radius: 999px;
  border: 1px solid var(--warning, var(--muted, inherit));
  color: var(--warning, var(--muted, inherit));
}
.wmap-beta-info {
  color: var(--muted, inherit);
  display: inline-flex;
  cursor: help;
}
.wmap-head-actions {
  display: flex;
  gap: 6px;
  align-items: center;
}
.wmap-search {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  animation: wmap-fade-up 0.4s cubic-bezier(0.22, 1, 0.36, 1) 0.14s both;
}
.wmap-search--disabled {
  opacity: 0.45;
  pointer-events: none;
}
.wmap-search-field {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 0 1 420px;
  min-width: 240px;
  height: 34px;
  padding: 0 10px;
  border-radius: 10px;
  border: 1px solid var(--ds-border);
  background: var(--ds-surface-2);
  color: var(--muted, inherit);
}
.wmap-search-field input {
  flex: 1;
  min-width: 0;
  border: none;
  outline: none;
  background: transparent;
  color: var(--text, inherit);
  font-size: 13px;
}
.wmap-chip {
  height: 30px;
  padding: 0 12px;
  border-radius: 999px;
  border: 1px solid var(--ds-border);
  background: var(--ds-surface-2);
  color: var(--muted, inherit);
  font-size: 12px;
  cursor: pointer;
}
.wmap-chip--on {
  color: var(--accent);
  border-color: color-mix(in srgb, var(--accent) 55%, transparent);
}
.wmap-viewport {
  position: relative;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  border-radius: 12px;
  border: 1px solid var(--ds-border);
  background: var(--ds-surface-2);
  cursor: grab;
  touch-action: none;
}
.wmap-viewport--dragging {
  cursor: grabbing;
}
.wmap-viewport--marking {
  cursor: crosshair;
}
.wmap-stage {
  position: absolute;
  left: 50%;
  top: 50%;
  max-width: none;
  transform-origin: center center;
  animation: wmap-img-in 0.44s cubic-bezier(0.22, 1, 0.36, 1) both;
}
.wmap-img {
  display: block;
  max-width: none;
  user-select: none;
}
.wmap-img--pixelated {
  image-rendering: pixelated;
}
.wmap-spawn {
  position: absolute;
  width: 10px;
  height: 10px;
  margin: -5px 0 0 -5px;
  border-radius: 50%;
  background: var(--accent);
  box-shadow: 0 0 0 2px var(--ds-surface-1);
  pointer-events: none;
  animation: wmap-spawn-pulse 2.2s ease-out infinite;
}
.wmap-marker {
  position: absolute;
  width: 12px;
  height: 12px;
  margin: -6px 0 0 -6px;
  border-radius: 50%;
  border: 2px solid var(--ds-surface-1);
  cursor: pointer;
  padding: 0;
}
.wmap-marker--structure {
  background: var(--accent);
}
.wmap-marker--biome {
  background: var(--wmap-biome);
}
.wmap-marker--ore {
  background: var(--wmap-ore);
}
.wmap-marker--marker {
  background: var(--wmap-marker);
}
.wmap-marker--player {
  background: var(--wmap-player);
}
.wmap-marker--bed {
  background: transparent;
  border-color: var(--wmap-player);
}
.wmap-marker--death {
  background: var(--danger, #e5484d);
}
.wmap-results {
  position: absolute;
  left: 10px;
  top: 10px;
  max-width: 320px;
  max-height: 55%;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 6px;
  border-radius: 10px;
  border: 1px solid var(--ds-border);
  background: var(--ds-surface-1);
  z-index: 3;
}
.wmap-results-empty {
  color: var(--muted, inherit);
  font-size: 12px;
  padding: 6px 8px;
}
.wmap-result {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 8px;
  border: none;
  border-radius: 8px;
  background: transparent;
  color: var(--text, inherit);
  font-size: 12.5px;
  cursor: pointer;
  text-align: left;
}
.wmap-result:hover {
  background: var(--ds-surface-2);
}
.wmap-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex: none;
}
.wmap-dot--structure {
  background: var(--accent);
}
.wmap-dot--biome {
  background: var(--wmap-biome);
}
.wmap-dot--ore {
  background: var(--wmap-ore);
}
.wmap-dot--marker {
  background: var(--wmap-marker);
}
.wmap-dot--player {
  background: var(--wmap-player);
}
.wmap-dot--bed {
  background: transparent;
  border: 1px solid var(--wmap-player);
}
.wmap-dot--death {
  background: var(--danger, #e5484d);
}
.wmap-result-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.wmap-result-coords {
  color: var(--muted, inherit);
  font-variant-numeric: tabular-nums;
}
.wmap-popup {
  position: absolute;
  z-index: 4;
  width: 210px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 10px;
  border-radius: 10px;
  border: 1px solid var(--ds-border);
  background: var(--ds-surface-1);
}
.wmap-popup-name {
  font-size: 13px;
}
.wmap-popup-coords {
  color: var(--muted, inherit);
}
.wmap-popup-actions {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}
.wmap-status,
.wmap-error {
  color: var(--muted, inherit);
  font-size: 12.5px;
}
.wmap-error {
  color: var(--danger, #e5484d);
}
.wmap-foot {
  animation: wmap-fade-up 0.4s cubic-bezier(0.22, 1, 0.36, 1) 0.18s both;
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.wmap-mark-hint {
  color: var(--accent);
  font-size: 12px;
}
.wmap-meta {
  color: var(--muted, inherit);
  margin-left: auto;
}
@keyframes wmap-scrim-in {
  from { opacity: 0; }
  to { opacity: 1; }
}
@keyframes wmap-card-in {
  from { opacity: 0; transform: translateY(14px) scale(0.955); }
  to { opacity: 1; transform: translateY(0) scale(1); }
}
@keyframes wmap-img-in {
  from { opacity: 0; transform: translate(calc(-50% + var(--pan-x, 0px)), calc(-50% + var(--pan-y, 0px))) scale(calc(var(--zoom, 1) * 0.985)); }
  to { opacity: 1; }
}
@keyframes wmap-spawn-pulse {
  0% { box-shadow: 0 0 0 2px var(--ds-surface-1), 0 0 0 0 color-mix(in srgb, var(--accent) 55%, transparent); }
  70% { box-shadow: 0 0 0 2px var(--ds-surface-1), 0 0 0 14px transparent; }
  100% { box-shadow: 0 0 0 2px var(--ds-surface-1), 0 0 0 0 transparent; }
}
@keyframes wmap-fade-up {
  from { opacity: 0; transform: translateY(8px); }
  to { opacity: 1; transform: translateY(0); }
}
@media (prefers-reduced-motion: reduce) {
  .wmap-scrim, .wmap-card, .wmap-head, .wmap-search, .wmap-foot, .wmap-stage {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
  }
}
</style>
