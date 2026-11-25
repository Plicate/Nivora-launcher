<script setup lang="ts">
import { ref } from 'vue'
import { bridge } from '../services/bridge'

const props = defineProps<{ instance: string; hasMods?: boolean }>()

// прогрев идёт автоматически при открытии экземпляра; сессия помнит прогретые
// экземпляры, чтобы не греть заново при каждом заходе на «Главную»
const warmedInstances = new Set<string>()

type AdvancementResult = Awaited<ReturnType<typeof bridge.instanceAdvancements>>
type PrewarmResult = Awaited<ReturnType<typeof bridge.instancePrewarm>>

const advancements = ref<AdvancementResult | null>(null)
const advancementsError = ref('')
const prewarm = ref<PrewarmResult | null>(null)
const prewarmBusy = ref(false)
const prewarmError = ref('')

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

loadAdvancements()
warmInstance()

function warmedLabel(bytes: number) {
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} ГиБ`
  return `${Math.round(bytes / 1024 / 1024)} МиБ`
}
</script>

<template>
  <div class="ov-status">
    <span class="ov-status-dot" :class="prewarmBusy ? 'ov-status-dot--busy' : prewarm && prewarm.client_jar_ready ? 'ov-status-dot--ok' : 'ov-status-dot--idle'" aria-hidden="true"></span>
    <span class="ov-status-text">
      <template v-if="prewarmBusy">Прогреваю кэш запуска…</template>
      <template v-else-if="prewarm">{{ prewarm.client_jar_ready ? 'Версия готова' : 'Клиентский jar отсутствует' }} · библиотек: {{ prewarm.total_libraries - prewarm.missing_libraries }}/{{ prewarm.total_libraries }}<template v-if="prewarm.assets_total !== null && prewarm.assets_total !== undefined"> · ассетов: {{ prewarm.assets_total - (prewarm.assets_missing ?? 0) }}/{{ prewarm.assets_total }}</template></template>
      <template v-else-if="prewarmError">{{ prewarmError }}</template>
      <template v-else>Проверяю готовность…</template>
    </span>
    <div v-if="advancements && hasMods && advancements.mods_total > 0" class="ov-status-progress">
      <div class="ov-status-track"><div class="ov-status-fill" :style="{ width: `${Math.min(100, advancements.mods_percent)}%` }"></div></div>
      <span class="ov-status-note">модпак {{ advancements.mods_done }}/{{ advancements.mods_total }} ({{ advancements.mods_percent }}%) · «{{ advancements.world }}»</span>
    </div>
    <span v-else-if="prewarm" class="ov-status-note">прогрето в кэш ОС: {{ warmedLabel(prewarm.warmed_bytes) }}</span>
    <span v-else-if="advancementsError" class="ov-status-note">{{ advancementsError }}</span>
  </div>
</template>

<style scoped>
.ov-status { display: flex; flex-wrap: wrap; min-width: 0; align-items: center; gap: 8px 10px; min-height: 26px; padding: 2px 2px 0; }
.ov-status-dot { flex: 0 0 auto; width: 7px; height: 7px; border-radius: 50%; background: var(--quiet); }
.ov-status-dot--busy { background: var(--muted); animation: ov-status-pulse 1.2s ease-in-out infinite; }
.ov-status-dot--ok { background: var(--accent); }
@keyframes ov-status-pulse { 0%, 100% { opacity: 1; } 50% { opacity: .35; } }
.ov-status-text { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--muted); font-size: 12px; }
.ov-status-progress { display: flex; align-items: center; gap: 8px; flex: 0 0 auto; max-width: 46%; }
.ov-status-track { width: 120px; height: 5px; border-radius: 999px; background: var(--ds-surface-2); overflow: hidden; }
.ov-status-fill { height: 100%; border-radius: 999px; background: var(--accent); transition: width 0.4s var(--ease-out-quart, ease); }
.ov-status-note { flex: 1 1 auto; min-width: 0; color: var(--quiet); font-size: 11.5px; overflow-wrap: anywhere; }
@media (max-width: 900px) { .ov-status { flex-wrap: wrap; } .ov-status-progress { max-width: 100%; } }
</style>
