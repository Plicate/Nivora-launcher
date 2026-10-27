<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import Icon from './Icon.vue'
import { bridge } from '../services/bridge'
import type { AutotuneMode, AutotunePlanChange, AutotuneResult, InstanceInfo } from '../types'

const props = defineProps<{
  instance: InstanceInfo
}>()

const emit = defineEmits<{
  (event: 'close'): void
  (event: 'applied', settings: AutotuneResult['settings']): void
  (event: 'toast', message: string): void
}>()

const loading = ref(true)
const applying = ref(false)
const error = ref('')
const result = ref<AutotuneResult | null>(null)
/** режим автонастройки: что важнее — картинка или кадры */
const mode = ref<AutotuneMode>('balanced')
const MODES: Array<{ id: AutotuneMode; title: string; hint: string; icon: string }> = [
  {
    id: 'performance',
    title: 'Упор в оптимизацию',
    hint: 'Максимум кадров: дистанции и эффекты урезаются до безопасного минимума',
    icon: 'speed',
  },
  {
    id: 'balanced',
    title: 'Сбалансированный',
    hint: 'Настройки под железо без крайностей',
    icon: 'sparkle',
  },
  {
    id: 'quality',
    title: 'Упор в качество',
    hint: 'Максимум картинки, который тянет железо, без просадок FPS',
    icon: 'image',
  },
]

const CATEGORY_ORDER = ['Память', 'Java', 'JVM', 'Графика', 'Запуск', 'Minecraft']
const CATEGORY_ICONS: Record<string, string> = {
  'Память': 'memory',
  'Java': 'monitor',
  'JVM': 'settings',
  'Графика': 'image',
  'Запуск': 'play',
  'Minecraft': 'mods',
}

const groupedChanges = computed<Array<{ category: string; icon: string; items: AutotunePlanChange[] }>>(() => {
  const changes = result.value?.plan.changes || []
  const groups = new Map<string, AutotunePlanChange[]>()
  for (const change of changes) {
    const category = change.category || 'Прочее'
    if (!groups.has(category)) groups.set(category, [])
    groups.get(category)!.push(change)
  }
  return [...groups.entries()]
    .sort(([a], [b]) => (CATEGORY_ORDER.indexOf(a) + 99) - (CATEGORY_ORDER.indexOf(b) + 99))
    .map(([category, items]) => ({ category, icon: CATEGORY_ICONS[category] || 'settings', items }))
})
const recommendations = computed(() => result.value?.plan.recommendations || [])
const notes = computed(() => result.value?.plan.notes || [])
const buckets = computed(() => result.value?.plan.mod_buckets || {})
const javaMajor = computed(() => result.value?.plan.java_major_required || 0)
const heapLabel = computed(() => {
  const heap = result.value?.plan.heap_mib || 0
  return heap >= 1024 ? `${(heap / 1024).toFixed(heap % 1024 === 0 ? 0 : 1)} ГБ` : `${heap} МБ`
})
const systemLabel = computed(() => {
  const kib = result.value?.plan.system_memory_kib || 0
  return kib >= 1024 * 1024 ? `${Math.round(kib / (1024 * 1024))} ГБ` : `${Math.round(kib / 1024)} МБ`
})
const hardware = computed(() => result.value?.plan.hardware)
const hardwareLine = computed(() => {
  const hw = hardware.value
  if (!hw) return ''
  const parts: string[] = []
  if (hw.cpu_model) parts.push(hw.cpu_model)
  else if (hw.cpu_cores > 0) parts.push(`${hw.cpu_cores} потоков CPU`)
  parts.push(hw.memory_mib >= 1024 ? `${(hw.memory_mib / 1024).toFixed(hw.memory_mib % 1024 === 0 ? 0 : 1)} ГБ ОЗУ` : `${hw.memory_mib} МБ ОЗУ`)
  if (hw.gpu && hw.gpu !== 'не определена') parts.push(hw.gpu)
  if (hw.gpu_class_title) parts.push(`класс GPU: ${hw.gpu_class_title}`)
  if (hw.vram_mib && hw.vram_mib > 0) parts.push(`${(hw.vram_mib / 1024).toFixed(hw.vram_mib % 1024 === 0 ? 0 : 1)} ГБ VRAM`)
  if (hw.display_width && hw.display_height) {
    const hz = hw.display_hz ? `@${hw.display_hz} Гц` : ''
    parts.push(`дисплей ${hw.display_width}×${hw.display_height}${hz}`)
  }
  return parts.join(' · ')
})

async function load(): Promise<void> {
  loading.value = true
  error.value = ''
  try {
    result.value = await bridge.instanceAutotune(props.instance.name, false, 0, mode.value)
  } catch (cause) {
    error.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    loading.value = false
  }
}

/** смена режима пересчитывает план, ничего не применяя */
async function pickMode(next: AutotuneMode): Promise<void> {
  if (applying.value || mode.value === next) return
  mode.value = next
  await load()
}

async function apply(): Promise<void> {
  if (applying.value || !result.value) return
  applying.value = true
  error.value = ''
  try {
    const applied = await bridge.instanceAutotune(props.instance.name, true, 0, mode.value)
    emit('applied', applied.settings)
    const minecraftKeys = Object.keys(applied.minecraft_options || {})
    const skipped = applied.skipped_minecraft_options || []
    const minecraftNote = minecraftKeys.length ? `, Minecraft: ${minecraftKeys.join(', ')}` : ''
    const skippedNote = skipped.length
      ? `. Настройки Minecraft (${skipped.length}) будут применены после первого запуска: options.txt ещё не создан игрой`
      : ''
    emit('toast', `«${props.instance.display_name || props.instance.name}» настроен: память ${applied.plan.memory}, пресет ${applied.plan.jvm_preset}${minecraftNote}${skippedNote}`)
    emit('close')
  } catch (cause) {
    error.value = cause instanceof Error ? cause.message : String(cause)
  } finally {
    applying.value = false
  }
}

onMounted(load)
</script>

<template>
  <Teleport to="body">
      <div class="dialog-scrim" role="presentation" @click.self="emit('close')">
        <section class="dialog-card autotune-card" role="dialog" tabindex="-1" aria-modal="true" aria-label="Автонастройка экземпляра">
          <div class="dialog-icon"><Icon name="sparkle" :size="18" /></div>
          <div class="dialog-copy">
            <div class="dialog-heading">
              <div>
                
                <h2>Полный разбор и настройка запуска</h2>
              </div>
              <button class="icon-button" type="button" aria-label="Закрыть" @click="emit('close')"><Icon name="close" :size="16" /></button>
            </div>

            <div class="autotune-modes" role="group" aria-label="Режим автонастройки">
              <button
                v-for="item in MODES"
                :key="item.id"
                type="button"
                class="autotune-mode"
                :class="{ 'autotune-mode--active': mode === item.id }"
                :aria-pressed="mode === item.id"
                :disabled="applying || loading"
                @click="pickMode(item.id)"
              >
                <Icon :name="item.icon" :size="15" />
                <span>
                  <strong>{{ item.title }}</strong>
                  <small>{{ item.hint }}</small>
                </span>
              </button>
            </div>

            <div v-if="loading" class="mod-audit-progress"><div class="progress-track progress-track--indeterminate"><span /></div><small>Анализирую моды, шейдеры, Java и железо…</small></div>
            <div v-else-if="error" class="compare-note compare-note--danger"><Icon name="warning" :size="14" /><span>{{ error }}</span></div>

            <template v-else-if="result">
              <div class="autotune-stats">
                <span class="autotune-stat"><strong>{{ result.plan.stats.mod_count }}</strong>модов</span>
                <span class="autotune-stat"><strong>{{ result.plan.stats.class_count.toLocaleString('ru-RU') }}</strong>классов</span>
                <span class="autotune-stat"><strong>{{ result.plan.stats.shader_count }}</strong>шейдерпаков</span>
                <span v-if="buckets.worldgen" class="autotune-stat autotune-stat--accent"><strong>{{ buckets.worldgen }}</strong>worldgen</span>
                <span v-if="buckets.heavy" class="autotune-stat autotune-stat--accent"><strong>{{ buckets.heavy }}</strong>тяжёлых</span>
                <span class="autotune-stat"><strong>{{ systemLabel }}</strong>ОЗУ</span>
                <span v-if="javaMajor" class="autotune-stat"><strong>Java {{ javaMajor }}+</strong>нужна</span>
              </div>

              <div v-if="hardware" class="autotune-hardware">
                <Icon name="monitor" :size="16" />
                <div>
                  <strong>Железо: {{ hardware.tier_title }}</strong>
                  <small>{{ hardwareLine }} — по этому профилю подобраны настройки Minecraft</small>
                </div>
              </div>

              <div class="autotune-heap">
                <Icon name="sparkle" :size="20" />
                <div>
                  <strong>{{ heapLabel }}</strong>
                  <small>оптимальная куча JVM вместо текущей настройки «{{ result.settings.memory }}»</small>
                </div>
              </div>

              <div class="autotune-changes">
                <section v-for="group in groupedChanges" :key="group.category" class="autotune-group">
                  <header class="autotune-group-heading"><Icon :name="group.icon" :size="13" />{{ group.category }}</header>
                  <div v-for="change in group.items" :key="change.key" class="autotune-change">
                    <span class="autotune-change-label">{{ change.label }}</span>
                    <span class="autotune-change-values">
                      <template v-if="change.from && change.from !== change.to"><s>{{ change.from }}</s> → </template><strong>{{ change.to || '—' }}</strong>
                    </span>
                    <small class="autotune-change-reason">{{ change.reason }}</small>
                  </div>
                </section>
              </div>

              <section v-if="recommendations.length" class="autotune-recommendations">
                <header class="autotune-group-heading"><Icon name="sparkle" :size="13" />Рекомендации — применяются вручную</header>
                <div v-for="item in recommendations" :key="item.title" class="autotune-recommendation">
                  <strong>{{ item.title }}</strong>
                  <small>{{ item.detail }}</small>
                </div>
              </section>

              <ul v-if="notes.length" class="autotune-notes">
                <li v-for="note in notes" :key="note">{{ note }}</li>
              </ul>

              <div class="autotune-actions">
                <button class="button button--quiet" type="button" :disabled="applying" @click="emit('close')">Позже</button>
                <button class="button button--accent" type="button" :disabled="applying" @click="apply">
                  <Icon name="sparkle" :size="14" />{{ applying ? 'Применяю…' : 'Настроить одной кнопкой' }}
                </button>
              </div>
              <p class="field-help">Настройки записываются в экземпляр и применяются при следующем запуске; ручные правки не пропадают — автонастройка меняет только перечисленные поля.</p>
            </template>
          </div>
        </section>
      </div>
  </Teleport>
</template>

<style scoped>
.autotune-card { max-width: 680px; }
.autotune-modes { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; margin-top: 12px; }
.autotune-mode { display: flex; gap: 8px; align-items: flex-start; padding: 10px; border: 1px solid var(--line); border-radius: 10px; background: var(--panel-soft); color: var(--muted); text-align: left; cursor: pointer; transition: border-color .15s, color .15s, background .15s; }
.autotune-mode strong { display: block; color: var(--text); font-size: 12px; }
.autotune-mode small { display: block; margin-top: 2px; font-size: 10px; line-height: 1.45; }
.autotune-mode--active { border-color: color-mix(in srgb, var(--accent) 55%, var(--line)); background: color-mix(in srgb, var(--accent) 12%, var(--panel)); color: var(--quiet); }
.autotune-mode:disabled { opacity: .6; cursor: default; }
@media (max-width: 620px) { .autotune-modes { grid-template-columns: 1fr; } }
.compare-note { display: flex; gap: 8px; align-items: center; padding: 10px 12px; border-radius: 10px; background: var(--panel-soft); color: var(--quiet); font-size: 13px; }
.compare-note--danger { background: color-mix(in srgb, var(--danger) 14%, transparent); color: var(--danger); }
.autotune-stats { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 14px; }
.autotune-stat { display: inline-flex; align-items: baseline; gap: 5px; padding: 5px 10px; border: 1px solid var(--line); border-radius: 999px; color: var(--muted); font-size: 11px; }
.autotune-stat strong { color: var(--text); font-size: 12px; font-variant-numeric: tabular-nums; }
.autotune-stat--accent { border-color: color-mix(in srgb, var(--accent) 45%, var(--line)); color: var(--quiet); }
.autotune-heap { display: flex; gap: 12px; align-items: center; margin-top: 12px; padding: 14px; border: 1px solid color-mix(in srgb, var(--accent) 30%, var(--line)); border-radius: 12px; background: color-mix(in srgb, var(--accent) 9%, var(--panel)); }
.autotune-heap strong { font-size: 22px; font-variant-numeric: tabular-nums; }
.autotune-heap small { display: block; color: var(--muted); }
.autotune-hardware { display: flex; gap: 10px; align-items: center; margin-top: 12px; padding: 10px 12px; border: 1px solid var(--line); border-radius: 10px; color: var(--muted); }
.autotune-hardware strong { color: var(--text); }
.autotune-hardware small { display: block; }
.autotune-changes { margin-top: 12px; display: flex; flex-direction: column; gap: 12px; max-height: 300px; overflow-y: auto; }
.autotune-group { display: flex; flex-direction: column; gap: 6px; }
.autotune-group-heading { display: flex; align-items: center; gap: 6px; color: var(--muted); font-size: 10px; font-weight: 700; letter-spacing: .08em; text-transform: uppercase; }
.autotune-change { padding: 9px 12px; border-radius: 10px; border: 1px solid var(--line); }
.autotune-change-label { font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted); display: block; }
.autotune-change-values { font-size: 14px; display: block; margin-top: 2px; }
.autotune-change-values s { opacity: 0.6; margin-right: 2px; }
.autotune-change-reason { color: var(--muted); display: block; margin-top: 3px; }
.autotune-recommendations { display: flex; flex-direction: column; gap: 6px; margin-top: 12px; }
.autotune-recommendation { padding: 9px 12px; border: 1px dashed color-mix(in srgb, var(--warning, #e0b45c) 45%, var(--line)); border-radius: 10px; }
.autotune-recommendation strong { display: block; font-size: 12px; }
.autotune-recommendation small { display: block; margin-top: 2px; color: var(--muted); line-height: 1.5; }
.autotune-notes { margin: 12px 0 0; padding-left: 18px; color: var(--muted); font-size: 12px; }
.autotune-actions { display: flex; justify-content: flex-end; gap: 10px; margin-top: 14px; }
</style>
