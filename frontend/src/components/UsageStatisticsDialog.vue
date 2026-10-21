<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import Icon from './Icon.vue'
import { bridge } from '../services/bridge'
import type { InstanceAsset, InstanceInfo, ModResourceUsage, ServerMetrics, SessionEvent } from '../types'

const props = defineProps<{
  instance: InstanceInfo
  metrics?: ServerMetrics
  metricHistory: ServerMetrics[]
  lastMetrics?: ServerMetrics | Record<string, unknown>
  assets: InstanceAsset[]
  running: boolean
}>()

const emit = defineEmits<{
  (event: 'close'): void
}>()

const liveMetrics = ref<ServerMetrics | undefined>(props.metrics)
const history = ref<ServerMetrics[]>([...props.metricHistory])
const lastMetrics = ref<ServerMetrics | Record<string, unknown> | undefined>(props.lastMetrics)
let pollTimer: number | undefined

const MAX_POINTS = 90

onMounted(() => {
  pollTimer = window.setInterval(async () => {
    if (document.hidden) return
    try {
      const result = await bridge.instanceLog(props.instance.name)
      if (result.metrics) liveMetrics.value = result.metrics
      if (result.last_metrics) lastMetrics.value = result.last_metrics
      history.value = [...history.value, result.metrics].slice(-MAX_POINTS)
    } catch {
      /* опрос лучший-усилие: следующая секунда повторит */
    }
  }, 1000)
})

onBeforeUnmount(() => {
  if (pollTimer) window.clearInterval(pollTimer)
})

function points(key: 'cpu_percent' | 'ram_bytes' | 'gpu'): string {
  const values = history.value.map((item) => {
    if (key === 'gpu') return Number(item.gpu?.util_percent || 0)
    return Number((item as unknown as Record<string, number>)[key] || 0)
  })
  if (!values.length) return ''
  const max = Math.max(...values, key === 'cpu_percent' || key === 'gpu' ? 1 : 0)
  const step = 100 / Math.max(1, values.length - 1)
  return values
    .map((value, index) => `${(index * step).toFixed(1)},${(42 - (value / max) * 40).toFixed(1)}`)
    .join(' ')
}

const currentGpu = computed(() => liveMetrics.value?.gpu)
const gpuAvailable = computed(() => currentGpu.value?.util_percent != null || currentGpu.value?.memory_bytes != null)

const previousRun = computed(() => {
  const value = lastMetrics.value as Partial<ServerMetrics> | undefined
  if (!value || !Object.keys(value).length) return null
  return value
})

const previousEvents = computed<SessionEvent[]>(() => previousRun.value?.session?.events || [])

const previousMods = computed<ModResourceUsage[]>(() => (previousRun.value?.session?.mods || []).slice(0, 12))

const liveEvents = computed<SessionEvent[]>(() => liveMetrics.value?.session?.events || [])

const liveMods = computed<ModResourceUsage[]>(() => (liveMetrics.value?.session?.mods || []).slice(0, 12))

const modNames = computed(() => new Map(
  props.assets
    .filter((asset) => asset.kind === 'mods')
    .map((asset) => [asset.name.replace(/\.jar$/i, '').toLowerCase(), asset.display_name || asset.name]),
))

const sessionStart = computed(() => {
  const events = liveEvents.value
  if (events.length) return events[0].at
  const started = Date.now() / 1000 - (liveMetrics.value?.uptime_seconds || 0)
  return started
})

const timelineEvents = computed(() => {
  const start = sessionStart.value
  return liveEvents.value.map((event) => ({
    ...event,
    offset: Math.max(0, Math.min(1, (event.at - start) / Math.max(30, liveMetrics.value?.uptime_seconds || 60))),
  }))
})

function formatSize(value?: number | null): string {
  const bytes = Number(value || 0)
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} ГБ`
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} МБ`
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} КБ`
  return `${bytes} Б`
}

function formatDuration(seconds: number): string {
  const total = Math.max(0, Math.round(seconds))
  const hours = Math.floor(total / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  if (hours) return `${hours} ч ${minutes} мин`
  if (minutes) return `${minutes} мин ${total % 60} с`
  return `${total} с`
}

function formatTime(at: number): string {
  return new Date(at * 1000).toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

function modLabel(mod: ModResourceUsage): string {
  const known = modNames.value.get(mod.mod) || modNames.value.get(mod.mod.toLowerCase())
  return known || mod.mod
}

function modLoad(mod: ModResourceUsage): number {
  return mod.errors * 8 + mod.warnings * 3 + Math.floor(mod.lines / 4)
}

function loadBarWidth(mod: ModResourceUsage): string {
  const heaviest = Math.max(...liveMods.value.map(modLoad), 1)
  return `${Math.max(6, Math.round((modLoad(mod) / heaviest) * 100))}%`
}
</script>

<template>
  <Teleport to="body">
    <Transition name="dialog">
      <div class="dialog-scrim" role="presentation" @click.self="emit('close')">
        <section class="dialog-card usage-statistics-card" role="dialog" tabindex="-1" aria-modal="true" aria-label="Статистика использования">
          <div class="dialog-icon"><Icon name="activity" :size="18" /></div>
          <div class="dialog-copy usage-statistics-copy">
            <div class="dialog-heading">
              <div>
                <span class="eyebrow">Экземпляр · телеметрия</span>
                <h2>Статистика использования</h2>
              </div>
              <button class="icon-button" type="button" aria-label="Закрыть" @click="emit('close')"><Icon name="close" :size="16" /></button>
            </div>

            <div class="usage-statistics-grid">
              <article class="usage-statistic">
                <span><Icon name="activity" :size="14" />CPU сейчас</span>
                <strong>{{ (liveMetrics?.cpu_percent || 0).toFixed(1) }}%</strong>
                <svg viewBox="0 0 100 42" preserveAspectRatio="none" aria-label="График CPU"><polyline :points="points('cpu_percent')" /></svg>
                <small>Среднее {{ liveMetrics?.avg_cpu_percent ?? '—' }}% · пик {{ liveMetrics?.peak_cpu_percent ?? '—' }}%</small>
              </article>
              <article class="usage-statistic">
                <span><Icon name="memory" :size="14" />RAM сейчас</span>
                <strong>{{ formatSize(liveMetrics?.ram_bytes) }}</strong>
                <svg viewBox="0 0 100 42" preserveAspectRatio="none" aria-label="График RAM"><polyline :points="points('ram_bytes')" /></svg>
                <small>Средняя {{ formatSize(liveMetrics?.avg_ram_bytes) }} · пик {{ formatSize(liveMetrics?.peak_ram_bytes) }}</small>
              </article>
              <article class="usage-statistic">
                <span><Icon name="image" :size="14" />GPU</span>
                <strong>{{ gpuAvailable ? `${(currentGpu?.util_percent || 0).toFixed(1)}%` : '—' }}</strong>
                <svg viewBox="0 0 100 42" preserveAspectRatio="none" aria-label="График GPU"><polyline :points="points('gpu')" /></svg>
                <small>{{ gpuAvailable ? `Видеопамять: ${formatSize(currentGpu?.memory_bytes)} · ${currentGpu?.source === 'nvidia' ? 'nvidia-smi' : currentGpu?.source === 'drm' ? 'DRM fdinfo' : 'нет данных'}` : 'Графика процесса недоступна для замера на этой системе' }}</small>
              </article>
              <article class="usage-statistic">
                <span><Icon name="activity" :size="14" />Сессия</span>
                <strong>{{ formatDuration(liveMetrics?.uptime_seconds || 0) }}</strong>
                <div class="usage-timeline" aria-label="События сессии">
                  <span
                    v-for="(event, index) in timelineEvents"
                    :key="`${index}-${event.title}`"
                    class="usage-timeline-point"
                    :style="{ left: `${event.offset * 100}%` }"
                    :title="`${formatTime(event.at)} · ${event.title}`"
                  />
                </div>
                <small>{{ running ? 'Игра запущена · события отмечены точками' : 'Последняя сессия' }}</small>
              </article>
            </div>

            <div class="usage-statistics-section">
              <h3><Icon name="mods" :size="14" />Моды по нагрузке</h3>
              <p v-if="!liveMods.length" class="muted">Пока нет данных: запустите игру с модами — лаунчер соберёт статистику из лога загрузки.</p>
              <div v-else class="usage-mods">
                <div v-for="mod in liveMods" :key="mod.mod" class="usage-mod">
                  <span class="usage-mod-name" :title="modLabel(mod)">{{ modLabel(mod) }}</span>
                  <span class="usage-mod-bar"><i :style="{ width: loadBarWidth(mod) }" /></span>
                  <span class="usage-mod-meta">{{ mod.errors ? `${mod.errors} ошибок · ` : '' }}{{ mod.warnings ? `${mod.warnings} предупреждений · ` : '' }}{{ mod.lines }} строк лога</span>
                </div>
              </div>
            </div>

            <div class="usage-statistics-section">
              <h3><Icon name="undo" :size="14" />Предыдущий запуск</h3>
              <p v-if="!previousRun" class="muted">Предыдущих запусков ещё не было — статистика появится после первой игровой сессии.</p>
              <div v-else class="usage-previous">
                <div class="usage-previous-stats">
                  <span><strong>{{ formatDuration(previousRun.uptime_seconds || 0) }}</strong><small>длительность</small></span>
                  <span><strong>{{ formatSize(previousRun.peak_ram_bytes) }}</strong><small>пик RAM</small></span>
                  <span><strong>{{ previousRun.peak_cpu_percent != null ? `${previousRun.peak_cpu_percent}%` : '—' }}</strong><small>пик CPU</small></span>
                  <span><strong>{{ previousRun.exit_code != null ? (previousRun.exit_code === 0 ? 'успешно' : `код ${previousRun.exit_code}`) : '—' }}</strong><small>завершение</small></span>
                </div>
                <div v-if="previousEvents.length" class="usage-timeline usage-timeline--history">
                  <span
                    v-for="(event, index) in previousEvents"
                    :key="`prev-${index}-${event.title}`"
                    class="usage-timeline-point"
                    :style="{ left: `${Math.min(100, (index / Math.max(1, previousEvents.length - 1)) * 100)}%` }"
                    :title="`${formatTime(event.at)} · ${event.title}`"
                  />
                </div>
                <div v-if="previousMods.length" class="usage-mods usage-mods--history">
                  <div v-for="mod in previousMods" :key="`prev-mod-${mod.mod}`" class="usage-mod">
                    <span class="usage-mod-name" :title="modLabel(mod)">{{ modLabel(mod) }}</span>
                    <span class="usage-mod-meta">{{ mod.errors ? `${mod.errors} ошибок · ` : '' }}{{ mod.warnings ? `${mod.warnings} предупреждений · ` : '' }}{{ mod.lines }} строк лога</span>
                  </div>
                </div>
              </div>
            </div>

            <div class="usage-statistics-section">
              <h3><Icon name="hard-drive" :size="14" />Диск</h3>
              <div class="usage-previous-stats">
                <span><strong>{{ formatSize(liveMetrics?.storage_bytes) }}</strong><small>файлы экземпляра</small></span>
                <span><strong>{{ props.assets.filter((asset) => asset.kind === 'mods').length }}</strong><small>модов установлено</small></span>
                <span><strong>{{ liveEvents.length }}</strong><small>событий сессии</small></span>
              </div>
            </div>

            <div class="dialog-actions">
              <button class="button button--outline" type="button" @click="emit('close')">Закрыть</button>
            </div>
          </div>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>
