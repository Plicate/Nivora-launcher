<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import Icon from './Icon.vue'
import { bridge } from '../services/bridge'
import type { ModAuditFinding, ModAuditPrepareResult, ModAuditReport } from '../types'

type AuditPhase = 'idle' | 'preparing' | 'confirm' | 'scanning' | 'report' | 'error'

export interface ModAuditRequest {
  instanceName: string
  path: string
  fileName: string
}

const emit = defineEmits<{
  (event: 'toast', message: string): void
  (event: 'finished', report: ModAuditReport): void
}>()

const phase = ref<AuditPhase>('idle')
const fileName = ref('')
const error = ref('')
const prepared = ref<ModAuditPrepareResult | null>(null)
const report = ref<ModAuditReport | null>(null)
const scanningSeconds = ref(0)
const showAllChecks = ref(false)
const expandedFindings = ref<Set<string>>(new Set())
let scanTimer: number | undefined
let activeSessionId = ''

const severityOrder: ModAuditFinding['severity'][] = ['critical', 'warning', 'notice', 'info']
const severityLabels: Record<ModAuditFinding['severity'], string> = {
  critical: 'Опасно',
  warning: 'Подозрительно',
  notice: 'Замечание',
  info: 'Информация',
}
const platformLabels: Record<string, string> = {
  bukkit: 'Bukkit/Spigot',
  paper: 'Paper',
  bungee: 'BungeeCord',
  fabric: 'Fabric',
  quilt: 'Quilt',
  forge: 'Forge',
  neoforge: 'NeoForge',
  unknown: 'неизвестно',
}

const verdictTone = computed(() => {
  if (report.value?.verdict === 'dangerous') return 'danger'
  if (report.value?.verdict === 'suspicious') return 'warning'
  if (report.value?.verdict === 'review') return 'notice'
  return 'success'
})

const groupedFindings = computed(() => {
  if (!report.value) return []
  return severityOrder
    .map((severity) => ({
      severity,
      label: severityLabels[severity],
      findings: report.value?.findings.filter((finding) => finding.severity === severity) ?? [],
    }))
    .filter((group) => group.findings.length > 0)
})

const visibleChecks = computed(() => {
  const checks = prepared.value?.checks ?? []
  return showAllChecks.value ? checks : checks.slice(0, 8)
})

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : typeof error === 'string' ? error : 'Неизвестная ошибка'
}

function formatSize(value: number): string {
  if (value >= 1024 * 1024) return `${(value / (1024 * 1024)).toFixed(1)} MiB`
  if (value >= 1024) return `${Math.round(value / 1024)} KiB`
  return `${value} Б`
}

function reset(): void {
  if (scanTimer) {
    window.clearInterval(scanTimer)
    scanTimer = undefined
  }
  activeSessionId = ''
  prepared.value = null
  report.value = null
  error.value = ''
  scanningSeconds.value = 0
  showAllChecks.value = false
  expandedFindings.value = new Set()
}

async function cancelAudit(): Promise<void> {
  const sessionId = activeSessionId
  reset()
  phase.value = 'idle'
  if (sessionId) {
    try {
      await bridge.modAuditCancel(sessionId)
      emit('toast', 'Проверка отклонена: декомпилированные исходники уничтожены без анализа')
    } catch {
      // истёкшие исходники удаляются по таймауту
    }
  }
}

function closeDialog(): void {
  if (phase.value === 'preparing' || phase.value === 'scanning') return
  void cancelAudit()
}

function toggleFinding(id: string): void {
  const next = new Set(expandedFindings.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  expandedFindings.value = next
}

async function acceptAudit(): Promise<void> {
  const sessionId = activeSessionId
  if (!sessionId) return
  phase.value = 'scanning'
  scanningSeconds.value = 0
  scanTimer = window.setInterval(() => {
    scanningSeconds.value += 1
  }, 1000)
  try {
    const result = await bridge.modAuditAccept(sessionId)
    activeSessionId = ''
    report.value = result.report
    phase.value = 'report'
    emit('finished', result.report)
  } catch (auditError) {
    phase.value = 'error'
    error.value = errorMessage(auditError)
    activeSessionId = ''
  } finally {
    if (scanTimer) {
      window.clearInterval(scanTimer)
      scanTimer = undefined
    }
  }
}

const lastRequest: ModAuditRequest = { instanceName: '', path: '', fileName: '' }

async function start(request: ModAuditRequest): Promise<void> {
  lastRequest.instanceName = request.instanceName
  lastRequest.path = request.path
  lastRequest.fileName = request.fileName
  reset()
  fileName.value = request.fileName
  phase.value = 'preparing'
  try {
    const payload = await bridge.modAuditPrepare(request.instanceName, request.path)
    activeSessionId = payload.session_id
    prepared.value = payload
    phase.value = 'confirm'
  } catch (prepareError) {
    phase.value = 'error'
    error.value = errorMessage(prepareError)
  }
}

function retry(): void {
  void start(lastRequest)
}

function handleKeydown(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    event.preventDefault()
    closeDialog()
  }
}

onBeforeUnmount(() => {
  if (scanTimer) window.clearInterval(scanTimer)
  if (activeSessionId) void bridge.modAuditCancel(activeSessionId).catch(() => undefined)
})

defineExpose({ start })
</script>

<template>
  <Teleport to="body">
    <Transition name="dialog">
      <div v-if="phase !== 'idle'" class="dialog-scrim mod-audit-scrim" role="presentation" @click.self="closeDialog">
        <section class="dialog-card mod-audit-card" role="dialog" tabindex="-1" aria-modal="true" aria-label="Проверка безопасности мода" @keydown="handleKeydown">
          <div class="dialog-icon" :class="{ 'dialog-icon--danger': phase === 'error' || verdictTone === 'danger' }">
            <Icon :name="phase === 'error' ? 'warning' : phase === 'report' ? (verdictTone === 'success' ? 'check' : 'warning') : 'shield'" :size="18" />
          </div>
          <div class="dialog-copy mod-audit-copy">
            <div class="dialog-heading">
              <div>
                
                <h2>Проверка «{{ fileName }}»</h2>
              </div>
              <button class="icon-button" type="button" aria-label="Закрыть" :disabled="phase === 'preparing' || phase === 'scanning'" @click="closeDialog"><Icon name="close" :size="16" /></button>
            </div>

            <template v-if="phase === 'preparing'">
              <p>Лаунчер декомпилирует файл во временную изолированную папку. Код плагина не запускается — читаются только байты классов.</p>
              <div class="mod-audit-progress"><div class="progress-track progress-track--indeterminate"><span /></div><small>Декомпиляция…</small></div>
            </template>

            <template v-else-if="phase === 'confirm' && prepared">
              <p>Файл декомпилирован. Проверка займёт <strong>{{ prepared.estimate.label }}</strong> и затронет {{ prepared.checks_count }} категорий. Оценка рассчитана по структуре плагина ({{ prepared.structure.source_lines.toLocaleString('ru-RU') }} строк исходников) и скорости этого ПК ({{ prepared.estimate.measured_lines_per_second.toLocaleString('ru-RU') }} строк/с).</p>
              <div class="mod-audit-stats">
                <span><strong>{{ prepared.structure.classes.toLocaleString('ru-RU') }}</strong><small>классов</small></span>
                <span><strong>{{ formatSize(prepared.source.size) }}</strong><small>размер файла</small></span>
                <span><strong>{{ platformLabels[prepared.structure.platform] || prepared.structure.platform }}</strong><small>платформа</small></span>
                <span><strong>{{ prepared.structure.source_lines.toLocaleString('ru-RU') }}</strong><small>строк исходников</small></span>
              </div>
              <div v-if="prepared.structure.natives.length" class="mod-audit-note mod-audit-note--danger"><Icon name="warning" :size="14" /><span>В архиве есть нативные библиотеки ({{ prepared.structure.natives.slice(0, 3).join(', ') }}{{ prepared.structure.natives.length > 3 ? '…' : '' }}) — их код проверке не поддаётся.</span></div>
              <div class="mod-audit-checks">
                <strong>Что будет проверяться:</strong>
                <ul>
                  <li v-for="check in visibleChecks" :key="check.id">{{ check.title }}</li>
                </ul>
                <button v-if="prepared.checks.length > visibleChecks.length || (showAllChecks && prepared.checks.length > 8)" class="button button--small button--quiet" type="button" @click="showAllChecks = !showAllChecks">
                  {{ showAllChecks ? 'Свернуть' : `Показать все ${prepared.checks_count}` }}
                </button>
              </div>
              <small class="dialog-detail">Если откажетесь — все декомпилированные исходники будут немедленно уничтожены без проверки. Они также уничтожаются сразу после проверки.</small>
              <div class="dialog-actions">
                <button class="button button--quiet" type="button" @click="cancelAudit">Отменить и уничтожить исходники</button>
                <button class="button button--accent" type="button" @click="acceptAudit">Начать проверку ({{ prepared.estimate.label }})</button>
              </div>
            </template>

            <template v-else-if="phase === 'scanning'">
              <p>Идёт проверка декомпилированных исходников: {{ prepared?.checks_count }} категорий (кейлогеры, окна, файлы, сеть, запуск команд и другие).</p>
              <div class="mod-audit-progress"><div class="progress-track progress-track--indeterminate"><span /></div><small>Проверка… {{ scanningSeconds }} с</small></div>
              <small class="dialog-detail">По завершении проверки исходники будут уничтожены — останется только отчёт.</small>
            </template>

            <template v-else-if="phase === 'report' && report">
              <div class="mod-audit-verdict" :class="`mod-audit-verdict--${verdictTone}`">
                <strong>{{ report.verdict_label }}</strong>
                <span>Уровень риска: {{ report.risk_score }}/100 · опасных: {{ report.counts.critical || 0 }} · подозрительных: {{ report.counts.warning || 0 }} · замечаний: {{ report.counts.notice || 0 }}</span>
              </div>
              <div v-if="report.findings.length" class="mod-audit-findings">
                <article v-for="group in groupedFindings" :key="group.severity" class="mod-audit-group" :class="`mod-audit-group--${group.severity}`">
                  <header><strong>{{ group.label }}</strong><small>{{ group.findings.length }}</small></header>
                  <button v-for="finding in group.findings" :key="finding.id" class="mod-audit-finding" type="button" @click="toggleFinding(finding.id)">
                    <span class="mod-audit-finding-head">
                      <strong>{{ finding.title }}</strong>
                      <Icon :name="expandedFindings.has(finding.id) ? 'close' : 'info'" :size="13" />
                    </span>
                    <span v-if="expandedFindings.has(finding.id)" class="mod-audit-finding-body">
                      <small>{{ finding.detail }}</small>
                      <small v-if="finding.evidence.length" class="mod-audit-evidence">Где: {{ finding.evidence.slice(0, 4).map((item) => item.class).join(', ') }}{{ finding.evidence.length > 4 ? '…' : '' }}</small>
                      <small class="mod-audit-recommendation">{{ finding.recommendation }}</small>
                    </span>
                  </button>
                </article>
              </div>
              <p v-else>Сигналов, требующих внимания, не найдено.</p>
              <small class="dialog-detail">Проверено {{ report.scanned_files.toLocaleString('ru-RU') }} файлов исходников за {{ report.duration_seconds.toFixed(1) }} с. Декомпилированные исходники уничтожены.</small>
              <div class="dialog-actions">
                <button class="button button--accent" type="button" @click="closeDialog">Закрыть</button>
              </div>
            </template>

            <template v-else-if="phase === 'error'">
              <p>{{ error || 'Не удалось выполнить проверку' }}</p>
              <div class="dialog-actions">
                <button class="button button--quiet" type="button" @click="closeDialog">Закрыть</button>
                <button class="button button--accent" type="button" @click="retry">Повторить</button>
              </div>
            </template>
          </div>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.mod-audit-scrim { z-index: 2300; }
.mod-audit-card { width: min(640px, 100%); align-items: flex-start; }
.mod-audit-copy { max-height: min(72vh, 680px); overflow-y: auto; padding-right: 4px; }
.mod-audit-progress { display: grid; gap: 8px; margin-top: 14px; }
.mod-audit-progress small { color: var(--muted); font-size: 10px; }
.mod-audit-stats { display: grid; grid-template-columns: repeat(auto-fit, minmax(110px, 1fr)); gap: 8px; margin-top: 14px; }
.mod-audit-stats span { display: grid; gap: 3px; padding: 10px; border: 1px solid var(--line); border-radius: calc(var(--radius-control, 10px) * 1.2); background: var(--panel-soft); }
.mod-audit-stats strong { font-size: 13px; }
.mod-audit-stats small { color: var(--muted); font-size: 10px; }
.mod-audit-note { display: flex; align-items: flex-start; gap: 8px; margin-top: 12px; padding: 9px 11px; border-radius: var(--radius-control, 10px); font-size: 11px; line-height: 1.5; color: var(--quiet); background: var(--panel-soft); }
.mod-audit-note--danger { border: 1px solid color-mix(in srgb, var(--danger) 40%, transparent); color: var(--danger); background: color-mix(in srgb, var(--danger) 10%, transparent); }
.mod-audit-checks { margin-top: 14px; display: grid; gap: 8px; }
.mod-audit-checks strong { font-size: 11px; color: var(--quiet); }
.mod-audit-checks ul { display: grid; gap: 4px; margin: 0; padding: 0 0 0 16px; color: var(--muted); font-size: 11px; line-height: 1.5; }
.mod-audit-checks .button { justify-self: start; }
.mod-audit-verdict { display: grid; gap: 5px; margin-top: 14px; padding: 12px 14px; border-radius: calc(var(--radius-control, 10px) * 1.2); border: 1px solid var(--line); }
.mod-audit-verdict strong { font-size: 14px; }
.mod-audit-verdict span { color: var(--quiet); font-size: 11px; }
.mod-audit-verdict--danger { border-color: color-mix(in srgb, var(--danger) 45%, transparent); background: color-mix(in srgb, var(--danger) 12%, transparent); }
.mod-audit-verdict--warning { border-color: color-mix(in srgb, #e8b34b 45%, transparent); background: color-mix(in srgb, #e8b34b 10%, transparent); }
.mod-audit-verdict--notice { border-color: color-mix(in srgb, var(--accent, var(--blue)) 35%, transparent); background: color-mix(in srgb, var(--accent, var(--blue)) 8%, transparent); }
.mod-audit-verdict--success { border-color: color-mix(in srgb, var(--green) 40%, transparent); background: color-mix(in srgb, var(--green) 10%, transparent); }
.mod-audit-findings { display: grid; gap: 10px; margin-top: 14px; }
.mod-audit-group { display: grid; gap: 6px; }
.mod-audit-group > header { display: flex; align-items: center; justify-content: space-between; padding: 0 2px; }
.mod-audit-group > header strong { font-size: 11px; letter-spacing: .02em; }
.mod-audit-group > header small { color: var(--muted); font-size: 10px; }
.mod-audit-group--critical > header strong { color: var(--danger); }
.mod-audit-group--warning > header strong { color: #e8b34b; }
.mod-audit-group--notice > header strong { color: var(--quiet); }
.mod-audit-group--info > header strong { color: var(--muted); }
.mod-audit-finding { display: grid; gap: 6px; width: 100%; padding: 10px 12px; border: 1px solid var(--line); border-radius: calc(var(--radius-control, 10px) * 1.1); color: var(--text); text-align: left; background: var(--panel-soft); cursor: pointer; transition: border-color .16s ease, background-color .16s ease; }
.mod-audit-finding:hover { border-color: var(--line-strong); background: var(--panel-raised); }
.mod-audit-finding-head { display: flex; align-items: center; justify-content: space-between; gap: 10px; }
.mod-audit-finding-head strong { font-size: 12px; }
.mod-audit-group--critical .mod-audit-finding { border-color: color-mix(in srgb, var(--danger) 35%, var(--line)); }
.mod-audit-group--warning .mod-audit-finding { border-color: color-mix(in srgb, #e8b34b 30%, var(--line)); }
.mod-audit-finding-body { display: grid; gap: 6px; }
.mod-audit-finding-body small { color: var(--quiet); font-size: 11px; line-height: 1.55; }
.mod-audit-evidence { color: var(--muted); font-family: ui-monospace, monospace; font-size: 10px; word-break: break-all; }
.mod-audit-recommendation { color: var(--text); }
</style>
