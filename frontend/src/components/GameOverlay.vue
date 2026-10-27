<script setup lang="ts">
// игровой оверлей Nivora (как оверлей Steam); открывается глобальной
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { emit } from '@tauri-apps/api/event'
import { bridge } from '../services/bridge'
import { toggleRecording, recordingFileName } from '../services/overlay'
import type { OverlayState } from '../types'

const snapshot = ref<OverlayState | null>(null)
const loadError = ref('')
const actionMessage = ref('')
const actionTone = ref<'ok' | 'error'>('ok')
const clockNow = ref(new Date())
const recordingBusy = ref(false)
const modsQuery = ref('')
const chatLines = ref<string[]>([])
const chatCursor = ref(0)
const chatInput = ref('')
const chatBusy = ref(false)
const chatOpen = ref(true)

let pollTimer: number | undefined
let clockTimer: number | undefined
let titleBeforeOverlay = ''

const instanceName = computed(() => snapshot.value?.instance?.display_name || snapshot.value?.instance?.name || '')

const clockLabel = computed(() => clockNow.value.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }))

const sessionLabel = computed(() => formatDuration(snapshot.value?.session_seconds || 0))
const totalLabel = computed(() => formatDuration(snapshot.value?.played_seconds || 0))

const visibleMods = computed(() => {
  const mods = snapshot.value?.mods || []
  const query = modsQuery.value.trim().toLowerCase()
  if (!query) return mods
  return mods.filter((mod) => mod.display_name.toLowerCase().includes(query) || mod.name.toLowerCase().includes(query))
})

const enabledModsCount = computed(() => (snapshot.value?.mods || []).filter((mod) => mod.enabled).length)

const metrics = computed(() => snapshot.value?.metrics || {})

const ramBarWidth = computed(() => {
  const bytes = Number(metrics.value.ram_bytes) || 0
  const limit = 8 * 1024 ** 3
  return `${Math.min(100, Math.round((bytes / limit) * 100))}%`
})

const diskBarWidth = computed(() => {
  const bytes = Number(metrics.value.storage_bytes) || 0
  const limit = 10 * 1024 ** 3
  return `${Math.min(100, Math.round((bytes / limit) * 100))}%`
})

const recording = computed(() => snapshot.value?.recording)

const overlaySettings = computed(() => snapshot.value?.overlay)

const systemMetrics = computed(() => snapshot.value?.system_metrics)

const isServerInstance = computed(() => snapshot.value?.instance?.kind === 'server')

const serverName = computed(() => snapshot.value?.instance?.name || '')

const chatVisible = computed(() => isServerInstance.value && overlaySettings.value?.show_chat !== false && chatOpen.value)

const overlayPosition = computed(() => overlaySettings.value?.position || 'top-left')

const rootClasses = computed(() => [
  `overlay-root--pos-${overlayPosition.value}`,
  { 'overlay-root--compact': overlaySettings.value?.compact === true },
])

const rootStyle = computed(() => ({
  zoom: String((overlaySettings.value?.scale || 100) / 100),
  '--overlay-tint': String(0.72 * ((overlaySettings.value?.opacity ?? 96) / 100)),
}))

function formatDuration(totalSeconds: number): string {
  const seconds = Math.max(0, Math.floor(totalSeconds))
  const days = Math.floor(seconds / 86400)
  const hours = Math.floor((seconds % 86400) / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  if (days > 0) return `${days} д ${hours} ч`
  if (hours > 0) return `${hours} ч ${minutes.toString().padStart(2, '0')} мин`
  if (minutes > 0) return `${minutes} мин`
  return `${seconds} с`
}

function formatBytes(bytes: number | undefined): string {
  const value = Math.max(0, Number(bytes) || 0)
  if (value >= 1024 ** 3) return `${(value / 1024 ** 3).toFixed(1)} ГБ`
  if (value >= 1024 ** 2) return `${(value / 1024 ** 2).toFixed(0)} МБ`
  if (value >= 1024) return `${(value / 1024).toFixed(0)} КБ`
  return `${value} Б`
}

function cpuPercent(): number {
  return Math.round(Number(metrics.value.cpu_percent) || 0)
}

async function refreshState() {
  try {
    snapshot.value = await bridge.overlayState()
    loadError.value = ''
  } catch (error) {
    loadError.value = error instanceof Error ? error.message : String(error)
  }
  if (chatVisible.value) void pollChat()
}

// хвост консоли сервера: те же операции, что и в панели серверов
async function pollChat() {
  const name = serverName.value
  if (!name || !isServerInstance.value) return
  try {
    const result = await bridge.serverLog(name, chatCursor.value)
    if (result.reset) chatLines.value = []
    if (result.lines.length) chatLines.value = [...chatLines.value, ...result.lines].slice(-40)
    if (typeof result.cursor === 'number') chatCursor.value = result.cursor
  } catch {
    // сервер мог остановиться; повторяем опрос на следующем тике
  }
}

async function sendChat() {
  const text = chatInput.value.trim()
  const name = serverName.value
  if (!text || !name || chatBusy.value) return
  chatBusy.value = true
  try {
    await bridge.serverCommand(name, text)
    chatInput.value = ''
    if (text.startsWith('/')) flash('Команда отправлена')
    await pollChat()
  } catch (error) {
    flash(error instanceof Error ? error.message : String(error), 'error')
  } finally {
    chatBusy.value = false
  }
}

function temperatureLabel(value: number | null | undefined): string {
  return value === null || value === undefined ? '—' : `${value.toFixed(0)}°C`
}

function flash(message: string, tone: 'ok' | 'error' = 'ok') {
  actionMessage.value = message
  actionTone.value = tone
}

async function handleRecordingClick() {
  if (recordingBusy.value) return
  recordingBusy.value = true
  try {
    const message = await toggleRecording(snapshot.value?.instance?.name || '')
    flash(message, 'ok')
    await refreshState()
  } catch (error) {
    flash(error instanceof Error ? error.message : String(error), 'error')
  } finally {
    recordingBusy.value = false
  }
}

function handleKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    closeOverlay()
  }
}

function startTimers() {
  stopTimers()
  pollTimer = window.setInterval(() => { void refreshState() }, 1000)
  clockTimer = window.setInterval(() => { clockNow.value = new Date() }, 1000)
}

function stopTimers() {
  if (pollTimer !== undefined) {
    window.clearInterval(pollTimer)
    pollTimer = undefined
  }
  if (clockTimer !== undefined) {
    window.clearInterval(clockTimer)
    clockTimer = undefined
  }
}

function handleVisibility() {
  if (document.hidden) stopTimers()
  else {
    startTimers()
    void refreshState()
  }
}

function closeOverlay() {
  // состояние видимости оверлея живёт в главном окне (там же глобальные
  // клавиши), поэтому просим его скрыть нас через событие
  emit('nivora-overlay-close').catch(() => undefined)
}

onMounted(() => {
  titleBeforeOverlay = document.title
  document.title = 'Nivora Overlay'
  document.body.classList.add('overlay-mode')
  window.addEventListener('keydown', handleKeydown)
  document.addEventListener('visibilitychange', handleVisibility)
  startTimers()
  void refreshState()
})

onBeforeUnmount(() => {
  // возврат того, что заняли: класс и заголовок; иначе после внутриоконного
  window.removeEventListener('keydown', handleKeydown)
  document.removeEventListener('visibilitychange', handleVisibility)
  stopTimers()
  document.body.classList.remove('overlay-mode', 'overlay-mode--opaque')
  if (titleBeforeOverlay) document.title = titleBeforeOverlay
})
</script>

<template>
  <div class="overlay-root" :class="rootClasses" :style="rootStyle">
    <header v-if="overlaySettings?.show_clock !== false" class="overlay-clock">
      <div class="overlay-clock__time">{{ clockLabel }}</div>
      <div class="overlay-clock__counters">
        <span v-tooltip="`Сессия «${instanceName}»`">Сессия {{ sessionLabel }}</span>
        <i class="overlay-clock__dot" aria-hidden="true" />
        <span v-tooltip="`Всего наиграно за «${instanceName}»`">Всего {{ totalLabel }}</span>
      </div>
    </header>

    <div v-if="snapshot && !snapshot.running" class="overlay-idle">
      <strong>Нет активной игры</strong>
      <small>Запустите Minecraft из Nivora — здесь появятся сессия, моды и метрики. Esc — закрыть.</small>
    </div>

    <section
      v-if="overlaySettings?.show_system_metrics && systemMetrics"
      class="overlay-system"
      :aria-label="`Системные метрики, позиция ${overlayPosition}`"
    >
      <div class="overlay-system__head">
        <strong>Система</strong>
        <span class="overlay-system__meta">{{ overlayPosition }}</span>
      </div>
      <div v-if="overlaySettings.metrics_cpu" class="overlay-system__row">
        <span class="overlay-system__label">CPU</span>
        <span class="overlay-system__value">{{ systemMetrics.cpu.percent !== null ? `${systemMetrics.cpu.percent.toFixed(0)}%` : '—' }}</span>
      </div>
      <div v-if="overlaySettings.metrics_ram" class="overlay-system__row">
        <span class="overlay-system__label">RAM</span>
        <span class="overlay-system__value">{{ systemMetrics.memory.percent !== null ? `${systemMetrics.memory.percent.toFixed(0)}%` : '—' }}</span>
        <div v-if="systemMetrics.memory.percent !== null" class="overlay-system__bar" aria-hidden="true"><i :style="{ width: `${Math.min(100, systemMetrics.memory.percent)}%` }" /></div>
      </div>
      <div v-for="gpu in overlaySettings.metrics_gpu ? systemMetrics.gpus : []" :key="gpu.index" class="overlay-system__row">
        <span class="overlay-system__label">GPU{{ systemMetrics.gpus.length > 1 ? gpu.index : '' }}</span>
        <span class="overlay-system__value">
          {{ gpu.utilization_percent !== null ? `${gpu.utilization_percent.toFixed(0)}%` : '—' }}
          <small v-if="gpu.memory_used_mib !== null && gpu.memory_total_mib !== null">{{ gpu.memory_used_mib }}/{{ gpu.memory_total_mib }} МБ</small>
        </span>
        <span v-if="overlaySettings.metrics_temperatures && gpu.temperature_c !== null" class="overlay-system__temp">{{ gpu.temperature_c.toFixed(0) }}°C</span>
      </div>
      <div v-for="disk in overlaySettings.metrics_disk ? systemMetrics.disks : []" :key="disk.mount" class="overlay-system__row">
        <span class="overlay-system__label" v-tooltip="disk.mount">{{ disk.mount }}</span>
        <span class="overlay-system__value">{{ disk.percent.toFixed(0) }}%<small>{{ formatBytes(disk.total_bytes - disk.free_bytes) }} / {{ formatBytes(disk.total_bytes) }}</small></span>
        <div class="overlay-system__bar" aria-hidden="true"><i :style="{ width: `${Math.min(100, disk.percent)}%` }" /></div>
      </div>
      <div v-if="overlaySettings.metrics_temperatures" class="overlay-system__row">
        <span class="overlay-system__label">°C</span>
        <span class="overlay-system__value">
          CPU {{ temperatureLabel(systemMetrics.temperatures.cpu_c) }}
          <small v-if="systemMetrics.temperatures.motherboard_c !== null">· плата {{ temperatureLabel(systemMetrics.temperatures.motherboard_c) }}</small>
        </span>
      </div>
    </section>

    <aside v-if="overlaySettings?.show_mods !== false" class="overlay-panel overlay-panel--mods">
      <div class="overlay-panel__head">
        <strong>Моды</strong>
        <span class="overlay-panel__meta">{{ enabledModsCount }}/{{ (snapshot?.mods || []).length }}</span>
      </div>
      <input v-model="modsQuery" class="overlay-panel__filter" type="search" placeholder="Поиск мода…" aria-label="Поиск мода" />
      <div v-if="snapshot?.instance?.game_edition === 'bedrock'" class="overlay-panel__empty">
        Моды доступны только для Java-экземпляров
      </div>
      <div v-else-if="visibleMods.length === 0" class="overlay-panel__empty">
        {{ (snapshot?.mods || []).length === 0 ? 'Модов нет' : 'Ничего не найдено' }}
      </div>
      <ul v-else class="overlay-mods">
        <li v-for="mod in visibleMods" :key="mod.name" class="overlay-mods__item" :class="{ 'overlay-mods__item--off': !mod.enabled }">
          <i class="overlay-mods__state" :class="mod.enabled ? 'overlay-mods__state--on' : 'overlay-mods__state--off'" aria-hidden="true" />
          <span class="overlay-mods__name" v-tooltip="mod.name">{{ mod.display_name }}</span>
        </li>
      </ul>
    </aside>

    <aside class="overlay-side">
      <section v-if="overlaySettings?.show_metrics !== false" class="overlay-panel overlay-panel--metrics">
        <div class="overlay-panel__head">
          <strong>Производительность</strong>
          <span class="overlay-panel__meta">{{ instanceName }}</span>
        </div>
        <div class="overlay-metrics">
          <div class="overlay-metric">
            <span class="overlay-metric__label">CPU</span>
            <div class="overlay-metric__bar" aria-hidden="true"><i :style="{ width: `${Math.min(100, cpuPercent())}%` }" /></div>
            <span class="overlay-metric__value">{{ cpuPercent() }}%</span>
          </div>
          <div class="overlay-metric">
            <span class="overlay-metric__label">RAM</span>
            <div class="overlay-metric__bar" aria-hidden="true"><i class="overlay-metric__bar-fill--ram" :style="{ width: ramBarWidth }" /></div>
            <span class="overlay-metric__value">{{ formatBytes(metrics.ram_bytes) }}</span>
          </div>
          <div class="overlay-metric">
            <span class="overlay-metric__label">Диск</span>
            <div class="overlay-metric__bar" aria-hidden="true"><i :style="{ width: diskBarWidth }" /></div>
            <span class="overlay-metric__value">{{ formatBytes(metrics.storage_bytes) }}</span>
          </div>
        </div>
        <dl class="overlay-peaks">
          <div><dt>Пик CPU</dt><dd>{{ Math.round(Number(metrics.peak_cpu_percent) || 0) }}%</dd></div>
          <div><dt>Пик RAM</dt><dd>{{ formatBytes(metrics.peak_ram_bytes) }}</dd></div>
        </dl>
      </section>

      <section class="overlay-panel overlay-panel--recording">
        <div class="overlay-panel__head">
          <strong>Запись</strong>
          <span class="overlay-recording__state" :class="recording?.active ? 'overlay-recording__state--on' : ''">
            {{ recording?.active ? `● ${formatDuration(recording.elapsed_seconds)}` : 'выключена' }}
          </span>
        </div>
        <p v-if="recording?.active" class="overlay-recording__file" v-tooltip="recording.file">{{ recordingFileName(recording.file) }} · {{ formatBytes(recording.size_bytes) }}</p>
        <p v-else class="overlay-recording__hint">Аппаратное кодирование, если доступно ({{ recording?.encoder || 'auto' }})</p>
        <div class="overlay-recording__actions">
          <button class="overlay-button" :class="recording?.active ? 'overlay-button--stop' : 'overlay-button--start'" type="button" :disabled="recordingBusy" @click="handleRecordingClick">
            {{ recordingBusy ? '…' : recording?.active ? 'Остановить запись' : 'Начать запись' }}
          </button>
        </div>
        <p v-if="actionMessage" class="overlay-recording__message" :class="`overlay-recording__message--${actionTone}`">{{ actionMessage }}</p>
      </section>

      <p v-if="loadError" class="overlay-error">{{ loadError }}</p>
    </aside>

    <section v-if="chatVisible" class="overlay-chat">
      <div class="overlay-chat__head">
        <strong>Консоль сервера</strong>
        <button class="overlay-chat__toggle" type="button" @click="chatOpen = false">Скрыть</button>
      </div>
      <ul class="overlay-chat__log">
        <li v-for="(line, index) in chatLines" :key="index">{{ line }}</li>
        <li v-if="!chatLines.length" class="overlay-chat__empty">Лог пуст: сервер, возможно, ещё стартует.</li>
      </ul>
      <form class="overlay-chat__form" @submit.prevent="sendChat">
        <input v-model="chatInput" placeholder="say Привет! или /whitelist add Steve" aria-label="Команда сервера" :disabled="chatBusy" />
        <button type="submit" :disabled="chatBusy || !chatInput.trim()">Отправить</button>
      </form>
    </section>
    <button v-else-if="isServerInstance && overlaySettings?.show_chat !== false" class="overlay-chat__open" type="button" @click="chatOpen = true; void pollChat()">Чат сервера</button>

    <footer class="overlay-hint">
      <kbd>{{ overlaySettings?.hotkey_overlay || 'Shift+Tab' }}</kbd> — закрыть оверлей
      <i aria-hidden="true">·</i>
      <kbd>{{ overlaySettings?.hotkey_recording || 'F7' }}</kbd> — запись
      <i aria-hidden="true">·</i>
      <kbd>Esc</kbd> — закрыть
    </footer>
  </div>
</template>

<style scoped>
.overlay-root {
  position: fixed;
  inset: 0;
  display: grid;
  grid-template-columns: minmax(0, 1fr) max-content minmax(0, 1fr);
  grid-template-rows: auto minmax(0, 1fr) auto auto;
  align-items: start;
  gap: 14px;
  padding: 22px 26px 18px;
  color: var(--text);
  font-family: var(--font-family);
  /* backdrop-filter не размывает чужое окно и снижает FPS */
  /* непрозрачная подложка нужна, если X11 или XWayland не отрисует прозрачность */
  background:
    linear-gradient(rgba(12, 13, 11, var(--overlay-tint, 0.72)), rgba(12, 13, 11, var(--overlay-tint, 0.72))),
    linear-gradient(#0c0d0b, #0c0d0b);
}

.overlay-root--compact { gap: 8px; padding: 14px 16px 12px; font-size: 13px; }
.overlay-root--compact .overlay-clock { padding: 6px 18px 8px; }
.overlay-root--compact .overlay-clock__time { font-size: 30px; }

.overlay-system {
  display: flex;
  flex-direction: column;
  gap: 6px;
  width: min(280px, 27vw);
  max-height: min(64vh, 560px);
  overflow: auto;
  overscroll-behavior: contain;
  touch-action: pan-y;
  padding: 10px 12px;
  border: 1px solid var(--line);
  border-radius: var(--radius-card, 10px);
  background: rgba(25, 26, 23, 0.85);
  box-shadow: 0 12px 36px rgba(0, 0, 0, 0.4);
  font-size: 12px;
  font-variant-numeric: tabular-nums;
}

.overlay-root--pos-top-left .overlay-system { grid-row: 1; grid-column: 1; justify-self: start; }
.overlay-root--pos-top-right .overlay-system { grid-row: 1; grid-column: 3; justify-self: end; }
.overlay-root--pos-bottom-left .overlay-system { grid-row: 3; grid-column: 1; align-self: end; justify-self: start; }
.overlay-root--pos-bottom-right .overlay-system { grid-row: 3; grid-column: 3; align-self: end; justify-self: end; }

.overlay-system__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding-bottom: 6px;
  border-bottom: 1px solid var(--line);
}

.overlay-system__meta { color: var(--muted); font-size: 10.5px; text-transform: uppercase; letter-spacing: 0.06em; }

.overlay-system__row { display: grid; grid-template-columns: 44px 1fr auto; align-items: center; gap: 8px; }

.overlay-system__label { color: var(--muted); font-size: 11px; letter-spacing: 0.04em; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

.overlay-system__value { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
.overlay-system__value small { color: var(--muted); font-size: 10px; }

.overlay-system__temp { color: var(--warning, #f5c451); font-size: 11px; }

.overlay-system__bar { grid-column: 2 / 4; width: 100%; height: 4px; border-radius: 999px; background: rgba(255, 255, 255, 0.12); overflow: hidden; }
.overlay-system__bar i { display: block; height: 100%; border-radius: 999px; background: var(--accent, #5b8dd9); }

.overlay-system__cores { display: flex; align-items: flex-end; gap: 2px; height: 18px; }
.overlay-system__cores i { flex: 1 1 0; min-width: 2px; max-width: 6px; border-radius: 1px; background: color-mix(in srgb, var(--accent, #5b8dd9) 75%, transparent); }

.overlay-chat {
  grid-row: 4;
  grid-column: 1;
  align-self: end;
  justify-self: start;
  display: flex;
  flex-direction: column;
  width: min(360px, 32vw);
  max-height: 40vh;
  border: 1px solid var(--line);
  border-radius: var(--radius-card, 10px);
  background: rgba(25, 26, 23, 0.88);
  box-shadow: 0 12px 36px rgba(0, 0, 0, 0.4);
  overflow: hidden;
}

.overlay-chat__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 12px;
  border-bottom: 1px solid var(--line);
}

.overlay-chat__head strong { font-size: 12.5px; }

.overlay-chat__toggle { border: none; background: none; color: var(--muted); font-size: 11px; cursor: pointer; }
/* hover-перекраска может заморозить оверлей WebKitGTK на NVIDIA */

.overlay-chat__log {
  flex: 1 1 auto;
  overflow: auto;
  overscroll-behavior: contain;
  touch-action: pan-y;
  margin: 0;
  padding: 8px 12px;
  list-style: none;
  font-family: var(--font-mono, monospace);
  font-size: 11px;
  line-height: 1.5;
  color: var(--quiet, #c7c7bb);
}

.overlay-chat__empty { color: var(--muted); }

.overlay-chat__form { display: flex; gap: 6px; padding: 8px; border-top: 1px solid var(--line); }

.overlay-chat__form input {
  flex: 1 1 auto;
  min-width: 0;
  padding: 6px 10px;
  border: 1px solid var(--line-strong);
  border-radius: 7px;
  background: rgba(12, 13, 11, 0.8);
  color: var(--text);
  font-size: 12px;
}

.overlay-chat__form button {
  padding: 6px 12px;
  border: 1px solid var(--line-strong);
  border-radius: 7px;
  background: var(--accent, #5b8dd9);
  color: #14140f;
  font-size: 12px;
  cursor: pointer;
}

.overlay-chat__form button:disabled { opacity: 0.5; cursor: default; }

.overlay-chat__open {
  grid-row: 4;
  grid-column: 1;
  align-self: end;
  justify-self: start;
  padding: 8px 14px;
  border: 1px solid var(--line);
  border-radius: 999px;
  background: rgba(25, 26, 23, 0.85);
  color: var(--text);
  font-size: 12px;
  cursor: pointer;
}

.overlay-idle {
  grid-row: 2;
  grid-column: 2;
  place-self: center;
  display: grid;
  gap: 6px;
  justify-items: center;
  max-width: min(380px, 90vw);
  padding: 18px 30px;
  border: 1px solid var(--line);
  border-radius: var(--radius-card, 10px);
  background: rgba(25, 26, 23, 0.86);
  box-shadow: 0 10px 34px rgba(0, 0, 0, 0.45);
  text-align: center;
  pointer-events: none;
}

.overlay-idle strong { font-size: 15px; }
.overlay-idle small { color: var(--muted); font-size: 11px; line-height: 1.5; }

.overlay-clock {
  grid-row: 1;
  grid-column: 2;
  justify-self: center;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 2px;
  max-width: 100%;
  padding: 10px 26px 12px;
  border: 1px solid var(--line);
  border-radius: var(--radius-card, 10px);
  background: rgba(25, 26, 23, 0.78);
  box-shadow: 0 10px 34px rgba(0, 0, 0, 0.45);
  user-select: none;
  pointer-events: none;
}

.overlay-clock__time {
  font-size: 42px;
  font-weight: 600;
  letter-spacing: 0.04em;
  line-height: 1;
  font-variant-numeric: tabular-nums;
}

.overlay-clock__counters {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--muted);
  font-size: 12.5px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.overlay-clock__dot {
  width: 3px;
  height: 3px;
  border-radius: 999px;
  background: currentColor;
  opacity: 0.6;
}

.overlay-panel {
  display: flex;
  flex-direction: column;
  min-width: 0;
  border: 1px solid var(--line);
  border-radius: var(--radius-card, 10px);
  background:
    linear-gradient(rgba(25, 26, 23, 0.82), rgba(25, 26, 23, 0.82)),
    linear-gradient(#0b1220, #0b1220);
  box-shadow: 0 12px 36px rgba(0, 0, 0, 0.4);
  overflow: hidden;
}

.overlay-panel__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 10px 14px;
  border-bottom: 1px solid var(--line);
}

.overlay-panel__head strong {
  font-size: 13px;
  letter-spacing: 0.02em;
}

.overlay-panel__meta {
  color: var(--muted);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.overlay-panel--mods {
  grid-row: 2;
  grid-column: 1;
  align-self: center;
  justify-self: start;
  width: min(300px, 27vw);
  max-height: min(56vh, 520px);
}

.overlay-panel__filter {
  margin: 8px 10px;
  padding: 7px 10px;
  border: 1px solid var(--line);
  border-radius: var(--radius-control, 8px);
  background: rgba(0, 0, 0, 0.25);
  color: var(--text);
  font-size: 12.5px;
}

.overlay-panel__filter:focus {
  outline: none;
  border-color: var(--blue-bright);
}

.overlay-mods {
  margin: 0;
  padding: 4px 6px 10px;
  list-style: none;
  overflow-y: auto;
  overscroll-behavior: contain;
}

.overlay-mods__item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 8px;
  border-radius: var(--radius-control, 8px);
  font-size: 12.5px;
}

.overlay-mods__item--off {
  opacity: 0.45;
}

.overlay-mods__state {
  flex: 0 0 auto;
  width: 7px;
  height: 7px;
  border-radius: 999px;
}

.overlay-mods__state--on {
  background: var(--green);
  box-shadow: 0 0 6px rgba(155, 172, 120, 0.7);
}

.overlay-mods__state--off {
  background: var(--muted);
}

.overlay-mods__name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.overlay-panel__empty {
  padding: 14px;
  color: var(--muted);
  font-size: 12.5px;
}

.overlay-side {
  grid-row: 2;
  grid-column: 3;
  align-self: center;
  justify-self: end;
  display: flex;
  flex-direction: column;
  gap: 12px;
  width: min(280px, 27vw);
}

.overlay-metrics {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 12px 14px 4px;
}

.overlay-metric {
  display: grid;
  grid-template-columns: 34px 1fr auto;
  align-items: center;
  gap: 10px;
}

.overlay-metric__label {
  color: var(--muted);
  font-size: 11px;
  font-weight: 600;
}

.overlay-metric__bar {
  height: 6px;
  border-radius: 999px;
  background: rgba(255, 255, 255, 0.08);
  overflow: hidden;
}

.overlay-metric__bar i {
  display: block;
  height: 100%;
  border-radius: 999px;
  background: linear-gradient(90deg, var(--green), var(--blue-bright));
  /* css-анимации могут остановить перерисовку слоёв оверлея WebKitGTK */
}

.overlay-metric__bar-fill--ram {
  background: linear-gradient(90deg, var(--blue-bright), var(--purple));
}

.overlay-metric__value {
  min-width: 52px;
  text-align: right;
  font-size: 12px;
  color: var(--quiet);
  font-variant-numeric: tabular-nums;
}

.overlay-peaks {
  display: flex;
  gap: 16px;
  margin: 0;
  padding: 8px 14px 12px;
  border-top: 1px dashed var(--line);
}

.overlay-peaks > div {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.overlay-peaks dt {
  color: var(--muted);
  font-size: 10.5px;
  font-weight: 600;
}

.overlay-peaks dd {
  margin: 0;
  font-size: 12.5px;
  font-variant-numeric: tabular-nums;
}

.overlay-panel--recording {
  padding-bottom: 12px;
}

.overlay-recording__state {
  color: var(--muted);
  font-size: 11.5px;
  font-variant-numeric: tabular-nums;
}

.overlay-recording__state--on {
  color: var(--danger);
}

.overlay-recording__file,
.overlay-recording__hint {
  margin: 8px 14px 0;
  color: var(--muted);
  font-size: 11.5px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.overlay-recording__actions {
  display: flex;
  padding: 10px 14px 0;
}

.overlay-button {
  flex: 1;
  padding: 9px 12px;
  border: 1px solid var(--line-strong);
  border-radius: var(--radius-button, 8px);
  color: var(--text);
  font-size: 12.5px;
  cursor: pointer;
  background: var(--panel-soft);
  /* transition кнопок может заморозить перерисовку оверлея WebKitGTK */
}

.overlay-button:active:not(:disabled) {

}

.overlay-button:disabled {
  opacity: 0.55;
  cursor: default;
}

.overlay-button--start {
  border-color: rgba(155, 172, 120, 0.55);
}

.overlay-button--stop {
  border-color: rgba(213, 138, 125, 0.6);
  background: rgba(213, 138, 125, 0.16);
}

.overlay-recording__message {
  margin: 8px 14px 0;
  font-size: 11.5px;
  color: var(--quiet);
}

.overlay-recording__message--error {
  color: var(--danger);
}

.overlay-error {
  margin: 0;
  padding: 8px 12px;
  border: 1px solid rgba(213, 138, 125, 0.5);
  border-radius: var(--radius-control, 8px);
  background: rgba(213, 138, 125, 0.12);
  color: var(--danger);
  font-size: 12px;
}

.overlay-hint {
  grid-row: 4;
  grid-column: 2;
  justify-self: center;
  align-self: end;
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 0;
  color: var(--muted);
  font-size: 11.5px;
  white-space: nowrap;
  user-select: none;
}

.overlay-hint kbd {
  padding: 2px 7px;
  border: 1px solid var(--line-strong);
  border-bottom-width: 2px;
  border-radius: 6px;
  background: rgba(0, 0, 0, 0.3);
  font-family: var(--font-family);
  font-size: 11px;
}

.overlay-hint i {
  opacity: 0.5;
}

@media (max-width: 1024px) {
  .overlay-root {
    grid-template-columns: minmax(0, 0.8fr) minmax(0, 1.2fr);
    grid-template-rows: auto auto auto minmax(0, 1fr) auto auto;
  }

  .overlay-clock { grid-row: 1; grid-column: 1 / -1; }
  .overlay-system { grid-row: 2 !important; grid-column: 1 / -1; justify-self: stretch; width: 100%; max-height: 18vh; }
  .overlay-panel--mods { grid-row: 3; grid-column: 1; align-self: start; width: min(280px, 33vw); }
  .overlay-side { grid-row: 3; grid-column: 2; align-self: start; width: min(320px, 49vw); }
  .overlay-idle { grid-row: 4; grid-column: 1 / -1; justify-self: center; }
  .overlay-chat, .overlay-chat__open { grid-row: 5; grid-column: 1 / -1; width: min(100%, 420px); max-height: 22vh; }
  .overlay-hint { grid-row: 6; grid-column: 1 / -1; justify-self: center; white-space: normal; }

  .overlay-clock__time {
    font-size: 32px;
  }
}
</style>
