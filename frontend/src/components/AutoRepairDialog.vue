<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import Icon from './Icon.vue'
import { bridge } from '../services/bridge'
import { autoRepairState, closeAutoRepair } from '../services/autoRepair'
import { useModalFocus } from '../services/modalFocus'
import type { AutoRepairFix, AutoRepairResult, SettingsInfo } from '../types'

const emit = defineEmits<{
  (event: 'toast', message: string): void
  (event: 'settings', settings: SettingsInfo): void
  (event: 'repaired', instanceName: string): void
  (event: 'relaunch', instanceName: string): void
  (event: 'login'): void
}>()

type Phase = 'offer' | 'applying' | 'done'

const NO_FIX = ''
const phase = ref<Phase>('offer')
const selection = reactive<Record<string, string>>({})
const results = ref<AutoRepairResult[]>([])
const muting = ref(false)
const dialogElement = ref<HTMLElement | null>(null)

const report = computed(() => autoRepairState.report)
const open = computed(() => Boolean(report.value))
const issues = computed(() => report.value?.issues ?? [])
const heading = computed(() => {
  if (!issues.value.length) return 'Проблем не найдено'
  if (issues.value.length === 1) return issues.value[0].title
  return `Найдено проблем: ${issues.value.length}`
})
const subtitle = computed(() => {
  const value = report.value
  if (!value) return ''
  if (value.manual) return `Проверка экземпляра «${value.instance_name}»`
  if (value.exit_code !== null && value.exit_code !== undefined && value.exit_code !== 0) return `Игра завершилась с кодом ${value.exit_code} · «${value.instance_name}»`
  return `Запуск «${value.instance_name}» не удался`
})
const chosenFixes = computed<AutoRepairFix[]>(() => issues.value
  .map((issue) => issue.fixes.find((fix) => fix.id === selection[issue.id]))
  .filter((fix): fix is AutoRepairFix => Boolean(fix)))
const anySucceeded = computed(() => results.value.some((item) => item.ok))
const mutedCount = computed(() => report.value?.muted_issues?.length ?? 0)

watch(report, (value) => {
  phase.value = 'offer'
  results.value = []
  for (const key of Object.keys(selection)) delete selection[key]
  for (const issue of value?.issues ?? []) {
    selection[issue.id] = issue.fixes.find((fix) => fix.recommended)?.id ?? issue.fixes[0]?.id ?? NO_FIX
  }
})

function dismiss() {
  if (phase.value === 'applying') return
  closeAutoRepair()
}

const handleKeydown = useModalFocus(open, dialogElement, dismiss)

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : typeof error === 'string' ? error : 'Неизвестная ошибка'
}

function fixTitle(id: string): string {
  for (const issue of issues.value) {
    const fix = issue.fixes.find((item) => item.id === id)
    if (fix) return fix.label
  }
  return id
}

async function applySelected() {
  const value = report.value
  if (!value || !chosenFixes.value.length || phase.value !== 'offer') return
  phase.value = 'applying'
  try {
    const chosen = chosenFixes.value
    const response = await bridge.autoRepairApply(value.instance_name, chosen)
    results.value = response.results
    emit('repaired', value.instance_name)
    // Вход Microsoft интерактивный (код устройства) — его запускает приложение.
    const loginIds = new Set(chosen.filter((fix) => fix.action === 'login_microsoft').map((fix) => fix.id))
    if (response.results.some((item) => item.ok && loginIds.has(item.id))) emit('login')
    const failed = response.results.filter((item) => !item.ok).length
    emit('toast', failed ? `Починка завершена с ошибками: ${failed}` : 'Починка применена')
  } catch (error) {
    results.value = [{ id: 'error', ok: false, message: errorMessage(error) }]
  } finally {
    phase.value = 'done'
  }
}

async function muteShown() {
  const kinds = issues.value.map((issue) => issue.id)
  if (!kinds.length || muting.value) return
  muting.value = true
  try {
    const response = await bridge.autoRepairMute(kinds, true)
    emit('settings', response.settings)
    emit('toast', 'Больше не покажу такие предложения. Вернуть можно в Настройки → Авто-починка')
    closeAutoRepair()
  } catch (error) {
    emit('toast', errorMessage(error))
  } finally {
    muting.value = false
  }
}

function relaunch() {
  const name = report.value?.instance_name
  closeAutoRepair()
  if (name) emit('relaunch', name)
}
</script>

<template>
  <Teleport to="body">
    <Transition name="dialog">
      <div v-if="report" class="dialog-scrim" role="presentation" @click.self="dismiss">
        <section ref="dialogElement" class="dialog-card auto-repair-card" role="dialog" tabindex="-1" aria-modal="true" aria-labelledby="auto-repair-title" @keydown="handleKeydown">
          <div class="dialog-icon" :class="{ 'dialog-icon--danger': issues.length && phase !== 'done' }"><Icon :name="phase === 'done' ? 'check' : issues.length ? 'warning' : 'shield'" :size="18" /></div>
          <div class="dialog-copy">
            <div class="dialog-heading">
              <div>
                <span class="eyebrow">Авто-починка</span>
                <h2 id="auto-repair-title">{{ phase === 'done' ? 'Результат починки' : heading }}</h2>
                <small class="auto-repair-subtitle">{{ subtitle }}</small>
              </div>
              <button class="icon-button" type="button" aria-label="Закрыть" :disabled="phase === 'applying'" @click="dismiss"><Icon name="close" :size="16" /></button>
            </div>

            <template v-if="phase !== 'done'">
              <p v-if="!issues.length" class="auto-repair-empty">Лаунчер проверил моды, конфиги и параметры запуска и не нашёл ничего, что можно исправить автоматически.</p>
              <div v-else class="auto-repair-issues">
                <article v-for="issue in issues" :key="issue.id" class="auto-repair-issue">
                  <header>
                    <strong>{{ issue.title }}</strong>
                    <span>{{ issue.detail }}</span>
                  </header>
                  <details v-if="issue.evidence.length" class="auto-repair-evidence">
                    <summary>Что нашлось в логе</summary>
                    <pre>{{ issue.evidence.join('\n') }}</pre>
                  </details>
                  <div v-if="issue.fixes.length" class="auto-repair-fixes" role="radiogroup" :aria-label="`Как починить: ${issue.title}`">
                    <span class="auto-repair-label">Как починить</span>
                    <label v-for="fix in issue.fixes" :key="fix.id" class="dialog-choice auto-repair-fix" :class="{ 'dialog-choice--active': selection[issue.id] === fix.id }">
                      <input v-model="selection[issue.id]" class="visually-hidden" type="radio" :name="`auto-repair-${issue.id}`" :value="fix.id" :disabled="phase === 'applying'" />
                      <span class="dialog-choice-mark"><Icon v-if="selection[issue.id] === fix.id" name="check" :size="13" /></span>
                      <span class="dialog-choice-copy">
                        <strong>{{ fix.label }}<em v-if="fix.recommended" class="auto-repair-badge">рекомендуется</em></strong>
                        <ul class="auto-repair-steps"><li v-for="(step, index) in fix.steps" :key="index">{{ step }}</li></ul>
                      </span>
                    </label>
                    <label class="dialog-choice auto-repair-fix" :class="{ 'dialog-choice--active': selection[issue.id] === NO_FIX }">
                      <input v-model="selection[issue.id]" class="visually-hidden" type="radio" :name="`auto-repair-${issue.id}`" :value="NO_FIX" :disabled="phase === 'applying'" />
                      <span class="dialog-choice-mark"><Icon v-if="selection[issue.id] === NO_FIX" name="check" :size="13" /></span>
                      <span class="dialog-choice-copy"><strong>Не чинить</strong></span>
                    </label>
                  </div>
                  <small v-if="issue.manual" class="auto-repair-manual"><Icon name="info" :size="12" />{{ issue.manual }}</small>
                </article>
              </div>
              <small v-if="mutedCount" class="dialog-detail">Скрыто проблем: {{ mutedCount }} — их показ отключён в настройках авто-починки.</small>
              <div class="dialog-actions auto-repair-actions">
                <button v-if="issues.length && !report.manual" class="button button--quiet auto-repair-mute" type="button" :disabled="muting || phase === 'applying'" :title="`Не предлагать починку для: ${issues.map((issue) => issue.title).join(', ')}`" @click="muteShown">Больше не показывать</button>
                <button class="button button--quiet" type="button" :disabled="phase === 'applying'" @click="dismiss">{{ issues.length ? 'Не сейчас' : 'Закрыть' }}</button>
                <button v-if="issues.length" class="button button--accent" type="button" :disabled="!chosenFixes.length || phase === 'applying'" @click="applySelected">
                  <Icon name="refresh" :size="14" />{{ phase === 'applying' ? 'Чиню…' : chosenFixes.length > 1 ? `Починить (${chosenFixes.length})` : 'Починить' }}
                </button>
              </div>
            </template>

            <template v-else>
              <ul class="auto-repair-results">
                <li v-for="item in results" :key="item.id" :class="item.ok ? 'auto-repair-result--ok' : 'auto-repair-result--failed'">
                  <Icon :name="item.ok ? 'check' : 'warning'" :size="14" />
                  <span><strong>{{ item.id === 'error' ? 'Починка не удалась' : fixTitle(item.id) }}</strong><small>{{ item.message }}</small></span>
                </li>
              </ul>
              <div class="dialog-actions">
                <button class="button button--quiet" type="button" @click="dismiss">Закрыть</button>
                <button v-if="anySucceeded" class="button button--accent" type="button" @click="relaunch"><Icon name="play" :size="14" />Запустить снова</button>
              </div>
            </template>
          </div>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.auto-repair-card { width: min(640px, 100%); max-height: calc(100vh - 40px); overflow-y: auto; }
.auto-repair-subtitle { display: block; margin-top: 4px; color: var(--muted); font-size: 11px; }
.auto-repair-empty { margin-top: 16px; }
.auto-repair-issues { display: grid; gap: 12px; margin-top: 16px; }
.auto-repair-issue { display: grid; gap: 8px; padding: 12px; border: 1px solid var(--line); border-left: 3px solid color-mix(in srgb, var(--danger, #ff657a) 55%, transparent); border-radius: var(--radius-control, 10px); background: var(--panel-soft); }
.auto-repair-issue header { display: grid; gap: 3px; }
.auto-repair-issue header strong { font-size: 13px; }
.auto-repair-issue header span { color: var(--quiet); font-size: 12px; line-height: 1.55; }
.auto-repair-evidence summary { color: var(--muted); font-size: 11px; cursor: pointer; }
.auto-repair-evidence pre { margin: 6px 0 0; padding: 8px; max-height: 120px; overflow: auto; border-radius: 6px; color: var(--quiet); background: var(--panel); font-size: 10px; white-space: pre-wrap; word-break: break-word; }
.auto-repair-fixes { display: grid; gap: 4px; }
.auto-repair-label { color: var(--muted); font-size: 10px; letter-spacing: .06em; text-transform: uppercase; }
.auto-repair-fix { position: relative; }
.auto-repair-badge { margin-left: 8px; padding: 1px 6px; border-radius: 999px; color: var(--success, #9bac78); background: color-mix(in srgb, var(--success, #9bac78) 16%, transparent); font-size: 10px; font-style: normal; font-weight: 500; }
.auto-repair-steps { display: grid; gap: 2px; margin: 4px 0 0; padding-left: 16px; color: var(--muted); font-size: 11px; line-height: 1.45; }
.auto-repair-manual { display: flex; gap: 6px; align-items: flex-start; color: var(--muted); font-size: 11px; line-height: 1.5; }
.auto-repair-actions { flex-wrap: wrap; }
.auto-repair-mute { margin-right: auto; }
.auto-repair-results { display: grid; gap: 8px; margin: 16px 0 0; padding: 0; list-style: none; }
.auto-repair-results li { display: flex; gap: 8px; align-items: flex-start; padding: 8px 10px; border-radius: 8px; background: var(--panel-soft); }
.auto-repair-results li span { display: grid; gap: 2px; }
.auto-repair-results small { color: var(--muted); font-size: 11px; line-height: 1.45; }
.auto-repair-result--ok { color: var(--success, #9bac78); }
.auto-repair-result--failed { color: var(--danger, #ff657a); }
.auto-repair-results strong { color: var(--text); font-size: 12px; }
</style>
