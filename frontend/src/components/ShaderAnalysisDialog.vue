<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import Icon from './Icon.vue'
import { bridge } from '../services/bridge'
import type { InstanceInfo } from '../types'

const props = defineProps<{
  instance: InstanceInfo
  shader: { name: string; size?: number }
}>()

const emit = defineEmits<{
  (event: 'close'): void
  (event: 'toast', message: string): void
}>()

interface AnalysisResult {
  analysis: {
    name: string
    format: string
    files: number
    programs: string[]
    features: string[]
    shadow_resolution: number
    size_bytes: number
    complexity: number
    estimated_vram_mb: number
    notes: string[]
    versions?: {
      requested: string
      verdict: VersionVerdict | null
      min_mc: string | null
      max_mc: string | null
      matrix: VersionVerdict[]
    }
  }
  gpu: {
    name: string
    vram_mb: number
    tier: string
    tier_label: string
    source: string
    recommendations: Array<{ name: string; detail: string }>
  }
  verdict: { label: string; tone: string }
  benchmark_available: boolean
}

interface VersionVerdict {
  version: string
  status: 'ok' | 'warn' | 'broken' | string
  label: string
  tone: string
  loader: string
  reason: string
}

interface BenchmarkState {
  phase: 'idle' | 'prepare' | 'baseline' | 'shader' | 'done' | 'failed'
  progress?: number
  message?: string
  error?: string
  baseline?: { cpu_percent?: number | null; ram_mb?: number; gpu_percent?: number | null; gpu_memory_mb?: number | null }
  shader?: { cpu_percent?: number | null; ram_mb?: number; gpu_percent?: number | null; gpu_memory_mb?: number | null }
  verdict?: { label: string; tone: string; gpu_delta_percent?: number; metric?: string }
}

const loading = ref(true)
const error = ref('')
const result = ref<AnalysisResult | null>(null)
const benchmark = ref<BenchmarkState>({ phase: 'idle' })
const benchmarkBusy = ref(false)
let pollTimer: number | undefined

async function load() {
  loading.value = true
  error.value = ''
  try {
    const response = await bridge.shaderAnalyze(props.instance.name, props.shader.name)
    if (!response) throw new Error('Ядро не ответило — анализ недоступен')
    result.value = response
  } catch (cause) {
    error.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    loading.value = false
  }
}

onMounted(load)
onBeforeUnmount(() => {
  if (pollTimer) window.clearInterval(pollTimer)
})

async function startBenchmark() {
  if (benchmarkBusy.value || !result.value?.benchmark_available) return
  if (benchmark.value.phase === 'baseline' || benchmark.value.phase === 'shader' || benchmark.value.phase === 'prepare') return
  benchmarkBusy.value = true
  benchmark.value = { phase: 'prepare', message: 'Готовлю запуск…' }
  try {
    await bridge.shaderBenchmarkStart(props.instance.name, props.shader.name)
    if (pollTimer) window.clearInterval(pollTimer)
    pollTimer = window.setInterval(async () => {
      if (document.hidden) return
      try {
        const state = await bridge.shaderBenchmarkStatus(props.instance.name)
        benchmark.value = state
        if (state.phase === 'done' || state.phase === 'failed') {
          if (pollTimer) window.clearInterval(pollTimer)
          if (state.phase === 'failed') emit('toast', state.error || 'Бенчмарк не удался')
          else emit('toast', 'Бенчмарк шейдера завершён')
        }
      } catch {
        /* следующий тик повторит */
      }
    }, 1000)
  } catch (cause) {
    benchmark.value = { phase: 'failed', error: cause instanceof Error ? cause.message : String(cause) }
    emit('toast', benchmark.value.error || 'Бенчмарк не удался')
  } finally {
    benchmarkBusy.value = false
  }
}

const complexityTone = computed(() => {
  const value = result.value?.analysis.complexity ?? 0
  if (value > 70) return 'danger'
  if (value > 45) return 'warning'
  return 'success'
})

const complexityBar = computed(() => `${Math.min(100, result.value?.analysis.complexity ?? 0)}%`)

const versionRangeLabel = computed(() => {
  const versions = result.value?.analysis.versions
  if (!versions) return ''
  const parts: string[] = []
  if (versions.min_mc) parts.push(`от ${versions.min_mc}`)
  if (versions.max_mc) parts.push(`до ${versions.max_mc}`)
  return parts.join(' ')
})

const phaseLabels: Record<string, string> = {
  prepare: 'Подготовка',
  baseline: 'Замер без шейдера',
  shader: 'Замер с шейдером',
  done: 'Готово',
  failed: 'Ошибка',
  idle: 'Ожидание',
}

function formatSize(value?: number): string {
  const bytes = Number(value || 0)
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} МБ`
  if (bytes >= 1024) return `${Math.round(bytes / 1024)} КБ`
  return `${bytes} Б`
}
</script>

<template>
  <Teleport to="body">
      <div class="dialog-scrim" role="presentation" @click.self="emit('close')">
        <section class="dialog-card shader-analysis-card" role="dialog" tabindex="-1" aria-modal="true" aria-label="Анализ шейдера">
          <div class="dialog-icon"><Icon name="image" :size="18" /></div>
          <div class="dialog-copy shader-analysis-copy">
            <div class="dialog-heading">
              <div>
                
                <h2>{{ shader.name }}</h2>
              </div>
              <button class="icon-button" type="button" aria-label="Закрыть" @click="emit('close')"><Icon name="close" :size="16" /></button>
            </div>

            <div v-if="loading" class="mod-audit-progress"><div class="progress-track progress-track--indeterminate"><span /></div><small>Разбираю архив шейдеров…</small></div>

            <div v-else-if="error" class="mod-audit-note mod-audit-note--danger"><Icon name="warning" :size="14" /><span>{{ error }}</span></div>

            <template v-else-if="result">
              <div class="shader-analysis-verdict" :class="`shader-analysis-verdict--${result.verdict.tone}`">
                <Icon :name="result.verdict.tone === 'success' ? 'check' : result.verdict.tone === 'danger' ? 'warning' : 'info'" :size="18" />
                <div>
                  <strong>{{ result.verdict.label }}</strong>
                  <small>Видеокарта: {{ result.gpu.name }} · уровень: {{ result.gpu.tier_label }}{{ result.gpu.vram_mb ? ` · ${Math.round(result.gpu.vram_mb / 1024)} ГБ VRAM` : '' }}</small>
                </div>
              </div>

              <div class="shader-analysis-stats">
                <span><strong>{{ result.analysis.complexity }}/100</strong><small>сложность шейдера</small></span>
                <span><strong>{{ result.analysis.features.length }}</strong><small>тяжёлых эффектов</small></span>
                <span><strong>{{ result.analysis.shadow_resolution || '—' }}</strong><small>разрешение теней</small></span>
                <span><strong>{{ result.analysis.estimated_vram_mb }} МБ</strong><small>оценка видеопамяти</small></span>
              </div>
              <div class="shader-analysis-bar"><i :class="`shader-analysis-bar--${complexityTone}`" :style="{ width: complexityBar }" /></div>

              <div v-if="result.analysis.versions" class="shader-analysis-versions">
                <h4><Icon name="check" :size="13" />Совместимость с версиями Minecraft</h4>
                <p v-if="versionRangeLabel" class="field-help">{{ versionRangeLabel }} · по фактическому коду шейдеров</p>
                <div class="shader-analysis-version-list">
                  <div
                    v-for="entry in result.analysis.versions.matrix"
                    :key="entry.version"
                    class="shader-analysis-version"
                    :class="`shader-analysis-version--${entry.tone}`"
                    :aria-current="entry.version === result.analysis.versions?.requested ? 'true' : undefined"
                  >
                    <span class="shader-analysis-version-name">{{ entry.version }}</span>
                    <span class="shader-analysis-version-status"><Icon :name="entry.tone === 'success' ? 'check' : entry.tone === 'danger' ? 'warning' : 'info'" :size="12" />{{ entry.label }}</span>
                    <span class="shader-analysis-version-loader">{{ entry.loader }}</span>
                    <small class="shader-analysis-version-reason">{{ entry.reason }}</small>
                  </div>
                </div>
              </div>

              <div class="shader-analysis-sections">
                <div>
                  <h4><Icon name="sparkle" :size="13" />Найденные эффекты</h4>
                  <div class="shader-analysis-tags">
                    <span v-for="feature in result.analysis.features" :key="feature">{{ feature }}</span>
                    <span v-if="!result.analysis.features.length" class="muted">Тяжёлых эффектов не найдено</span>
                  </div>
                </div>
                <div>
                  <h4><Icon name="list" :size="13" />Проходы рендера</h4>
                  <div class="shader-analysis-tags">
                    <span v-for="program in result.analysis.programs" :key="program">{{ program }}</span>
                  </div>
                  <p class="field-help">{{ result.analysis.files }} файлов шейдеров · {{ formatSize(result.analysis.size_bytes) }} · формат: {{ result.analysis.format === 'iris-properties' ? 'Iris/OptiFine (shaders.properties)' : result.analysis.format }}</p>
                </div>
                <div>
                  <h4><Icon name="check" :size="13" />Что тянет ваша карта</h4>
                  <div class="shader-analysis-recommendations">
                    <span v-for="item in result.gpu.recommendations" :key="item.name"><strong>{{ item.name }}</strong><small>{{ item.detail }}</small></span>
                  </div>
                </div>
                <div v-if="result.analysis.notes.length">
                  <h4><Icon name="info" :size="13" />Замечания</h4>
                  <ul class="shader-analysis-notes">
                    <li v-for="note in result.analysis.notes" :key="note">{{ note }}</li>
                  </ul>
                </div>
              </div>

              <div class="shader-analysis-benchmark">
                <div class="shader-analysis-benchmark-head">
                  <div>
                    <h4><Icon name="activity" :size="13" />Тест на железе</h4>
                    <p class="field-help">Лаунчер дважды запустит игру (без шейдера и с ним), подождёт главное меню, 15 секунд снимет нагрузку GPU/CPU и закроет игру. Настройки вернутся как были.</p>
                  </div>
                  <button
                    v-if="benchmark.phase !== 'baseline' && benchmark.phase !== 'shader' && benchmark.phase !== 'prepare'"
                    class="button button--accent"
                    type="button"
                    :disabled="benchmarkBusy || !result.benchmark_available"
                    @click="startBenchmark"
                  >
                    <Icon name="play" :size="14" />{{ benchmark.phase === 'done' ? 'Повторить тест' : 'Запустить тест' }}
                  </button>
                </div>
                <p v-if="!result.benchmark_available" class="field-help">Для теста нужен Iris — установите его из каталога модов: без Iris шейдеры не работают.</p>

                <div v-if="['prepare', 'baseline', 'shader'].includes(benchmark.phase)" class="shader-analysis-progress">
                  <div class="progress-track"><span :style="{ width: `${benchmark.progress || 0}%` }" /></div>
                  <small>{{ phaseLabels[benchmark.phase] }} · {{ benchmark.message }}</small>
                </div>

                <p v-if="benchmark.phase === 'failed' && benchmark.error" class="mod-audit-note mod-audit-note--danger"><Icon name="warning" :size="14" /><span>{{ benchmark.error }}</span></p>

                <div v-if="benchmark.phase === 'done' && benchmark.baseline && benchmark.shader" class="shader-analysis-results">
                  <div class="shader-analysis-result">
                    <strong>Без шейдера</strong>
                    <span>GPU: {{ benchmark.baseline.gpu_percent != null ? `${benchmark.baseline.gpu_percent}%` : 'нет данных' }}</span>
                    <span>CPU: {{ benchmark.baseline.cpu_percent != null ? `${benchmark.baseline.cpu_percent}%` : '—' }}</span>
                    <span>RAM: {{ benchmark.baseline.ram_mb ? `${Math.round(benchmark.baseline.ram_mb)} МБ` : '—' }}</span>
                  </div>
                  <div class="shader-analysis-result shader-analysis-result--with">
                    <strong>С шейдером</strong>
                    <span>GPU: {{ benchmark.shader.gpu_percent != null ? `${benchmark.shader.gpu_percent}%` : 'нет данных' }}</span>
                    <span>GPU-память: {{ benchmark.shader.gpu_memory_mb ? `${Math.round(benchmark.shader.gpu_memory_mb)} МБ` : '—' }}</span>
                    <span>CPU: {{ benchmark.shader.cpu_percent != null ? `${benchmark.shader.cpu_percent}%` : '—' }}</span>
                    <span>RAM: {{ benchmark.shader.ram_mb ? `${Math.round(benchmark.shader.ram_mb)} МБ` : '—' }}</span>
                  </div>
                </div>
                <div v-if="benchmark.phase === 'done' && benchmark.verdict" class="shader-analysis-verdict" :class="`shader-analysis-verdict--${benchmark.verdict.tone}`">
                  <Icon name="info" :size="18" />
                  <div>
                    <strong>{{ benchmark.verdict.label }}</strong>
                    <small>Разница нагрузки: +{{ benchmark.verdict.gpu_delta_percent }}% ({{ benchmark.verdict.metric === 'gpu_percent' ? 'видеокарта' : 'процессор' }})</small>
                  </div>
                </div>
              </div>

              <div class="dialog-actions">
                <button class="button button--outline" type="button" @click="emit('close')">Закрыть</button>
              </div>
            </template>
          </div>
        </section>
      </div>
  </Teleport>
</template>
