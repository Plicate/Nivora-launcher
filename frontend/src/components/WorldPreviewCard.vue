<script setup lang="ts">
import { ref } from 'vue'
import { bridge } from '../services/bridge'

const props = defineProps<{ instance: string; hasMods?: boolean }>()

// прогрев идёт автоматически при открытии экземпляра; отдельной кнопки больше нет.
// сессия помнит прогретые экземпляры, чтобы не греть заново при каждом заходе на «Главную»
const warmedInstances = new Set<string>()

type AdvancementResult = Awaited<ReturnType<typeof bridge.instanceAdvancements>>
type PrewarmResult = Awaited<ReturnType<typeof bridge.instancePrewarm>>

const advancements = ref<AdvancementResult | null>(null)
const advancementsError = ref('')
const prewarm = ref<PrewarmResult | null>(null)
const prewarmBusy = ref(false)

async function loadAdvancements() {
  advancementsError.value = ''
  try {
    advancements.value = await bridge.instanceAdvancements(props.instance)
  } catch (error) {
    advancementsError.value = error instanceof Error ? error.message : String(error)
  }
}

async function warmInstance() {
  if (prewarmBusy.value || warmedInstances.has(props.instance)) return
  prewarmBusy.value = true
  try {
    prewarm.value = await bridge.instancePrewarm(props.instance)
    warmedInstances.add(props.instance)
  } catch (error) {
    prewarm.value = null
    prewarmError.value = error instanceof Error ? error.message : String(error)
  } finally {
    prewarmBusy.value = false
  }
}

const prewarmError = ref('')

loadAdvancements()
warmInstance()

function warmedLabel(bytes: number) {
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} ГиБ`
  return `${Math.round(bytes / 1024 / 1024)} МиБ`
}
</script>

<template>
  <section class="panel world-preview-card">
    <div class="panel-heading">
      <div>
        <h2>Запуск</h2>
      </div>
    </div>
    <div class="world-preview-body">
      <div v-if="prewarmBusy" class="world-prewarm-report">
        <span>Прогреваю кэш запуска…</span>
      </div>
      <div v-else-if="prewarm" class="world-prewarm-report">
        <span>{{ prewarm.client_jar_ready ? 'Версия готова' : 'Клиентский jar отсутствует' }} · библиотек: {{ prewarm.total_libraries - prewarm.missing_libraries }}/{{ prewarm.total_libraries }}<template v-if="prewarm.assets_total !== null && prewarm.assets_total !== undefined"> · ассетов: {{ prewarm.assets_total - (prewarm.assets_missing ?? 0) }}/{{ prewarm.assets_total }}</template></span>
        <small class="field-help">Прогрето в кэш ОС: {{ warmedLabel(prewarm.warmed_bytes) }}</small>
      </div>
      <small v-else-if="prewarmError" class="field-help">{{ prewarmError }}</small>
      <div v-if="advancements && hasMods && advancements.mods_total > 0" class="world-advancements">
        <span>Прогресс модпака: {{ advancements.mods_done }} из {{ advancements.mods_total }} достижений модов ({{ advancements.mods_percent }}%)</span>
        <div class="world-progress-track">
          <div class="world-progress-fill" :style="{ width: `${Math.min(100, advancements.mods_percent)}%` }"></div>
        </div>
        <small class="field-help">мир «{{ advancements.world }}»</small>
      </div>
      <small v-else-if="advancementsError" class="field-help">{{ advancementsError }}</small>
    </div>
  </section>
</template>

<style scoped>
.world-preview-card {
  margin-bottom: 14px;
}
.world-preview-body {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.world-prewarm-report,
.world-advancements {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 12.5px;
}
.world-progress-track {
  height: 6px;
  border-radius: 999px;
  background: var(--ds-surface-2);
  overflow: hidden;
}
.world-progress-fill {
  height: 100%;
  border-radius: 999px;
  background: var(--accent);
  transition: width 0.4s var(--ease-out-quart, ease);
}
</style>
