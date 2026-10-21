<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import Icon from './Icon.vue'
import NumericStepper from './NumericStepper.vue'
import { bridge } from '../services/bridge'
import { confirmDialog } from '../services/dialogs'
import { canMapServer, natLabels, serverAddress } from '../services/serverNetwork'
import type { ServerNetworkMapping, ServerNetworkReport } from '../types'

const props = defineProps<{ name: string; port: number; running: boolean; core: string }>()
const emit = defineEmits<{ (event: 'toast', message: string): void }>()
const externalPort = ref(props.port)
const manualIp = ref('')
const report = ref<ServerNetworkReport>()
const reportFresh = ref(false)
const reportCheckedAt = ref(0)
const mapping = ref<ServerNetworkMapping>({ active: false, error: '' })
const busy = ref('')
const error = ref('')
const statusBusy = ref(false)
let epoch = 0
let timer: ReturnType<typeof setInterval> | undefined
const address = computed(() => mapping.value.active
  ? mapping.value.address || ''
  : serverAddress(manualIp.value || (reportFresh.value ? report.value?.public_ip || '' : ''), externalPort.value))
const readyForUpnp = computed(() => reportFresh.value
  && canMapServer(report.value, props.running)
  && !mapping.value.active
  && !mapping.value.closing)
const checkedAt = computed(() => reportCheckedAt.value ? new Date(reportCheckedAt.value).toLocaleTimeString() : '')
const proxyCore = computed(() => ['velocity', 'waterfall'].includes(props.core))

function message(value: unknown): string {
  return value instanceof Error ? value.message : String(value)
}

async function syncMapping() {
  if (statusBusy.value || busy.value) return
  statusBusy.value = true
  const current = epoch
  try {
    const result = await bridge.serverNetworkStatus(props.name)
    if (current === epoch) mapping.value = result.mapping
  } catch {
    // do not turn a disconnected backend into a claim that the port is closed.
  } finally {
    statusBusy.value = false
  }
}

async function diagnose() {
  if (busy.value) return
  busy.value = 'diagnose'
  reportFresh.value = false
  reportCheckedAt.value = 0
  error.value = ''
  const current = epoch
  const name = props.name
  const port = externalPort.value
  const configuredPort = props.port
  const core = props.core
  try {
    const result = await bridge.serverNetworkDiagnose(name, port)
    if (current !== epoch || name !== props.name || port !== externalPort.value || configuredPort !== props.port || core !== props.core) return
    report.value = result
    mapping.value = result.mapping
    reportFresh.value = true
    reportCheckedAt.value = Date.now()
  } catch (value) {
    if (current === epoch) error.value = message(value)
  } finally {
    if (current === epoch) busy.value = ''
  }
}

async function prepare() {
  if (busy.value || props.running || proxyCore.value) return
  if (!(await confirmDialog('Разрешить входящие подключения к игровому серверу? Лаунчер очистит server-ip в server.properties и применит сохранённый игровой порт. Online-mode и white-list не изменятся. Сам проброс на роутере пока не создаётся.', { danger: true, confirmLabel: 'Подготовить сервер' }))) return
  busy.value = 'prepare'
  error.value = ''
  try {
    await bridge.serverNetworkPrepare(props.name)
    if (report.value) { report.value.bind_all = true; report.value.bind_address = '' }
    reportFresh.value = false
    emit('toast', 'server.properties подготовлен. Теперь запустите сервер')
  } catch (value) {
    error.value = message(value)
  } finally {
    busy.value = ''
  }
}

async function open() {
  if (busy.value || !readyForUpnp.value) return
  const current = epoch
  const name = props.name
  const port = props.port
  const core = props.core
  const wasRunning = props.running
  const selectedExternalPort = externalPort.value
  const sameContext = () => current === epoch
    && name === props.name
    && port === props.port
    && core === props.core
    && wasRunning === props.running
    && selectedExternalPort === externalPort.value
  if (!(await confirmDialog(`Открыть TCP ${selectedExternalPort} → ${report.value?.local_ip || 'этот компьютер'}:${port} для подключений из интернета? Публикуется только игровой порт, не RCON и не панель лаунчера. Используйте online-mode=true и whitelist. Правило временное; лаунчер удалит его при остановке сервера.`, { danger: true, confirmLabel: 'Открыть игровой порт' }))) return
  if (current !== epoch || name !== props.name || port !== props.port || core !== props.core || wasRunning !== props.running || !readyForUpnp.value) return
  if (selectedExternalPort !== externalPort.value) {
    error.value = 'Внешний порт изменился. Повторите публикацию и подтверждение.'
    return
  }
  busy.value = 'open'
  error.value = ''
  try {
    const fresh = await bridge.serverNetworkDiagnose(name, selectedExternalPort)
    if (!sameContext()) return
    report.value = fresh
    mapping.value = fresh.mapping
    reportFresh.value = true
    reportCheckedAt.value = Date.now()
    if (!canMapServer(fresh, props.running)) {
      error.value = !fresh.local_listening
        ? 'Игровой TCP-порт сейчас не отвечает. Запустите сервер и проверьте порт.'
        : 'Состояние сети изменилось. Проверьте диагностику и повторите публикацию.'
      return
    }
    const result = await bridge.serverNetworkOpen(name, selectedExternalPort, fresh.observed_ip || '')
    if (!sameContext()) {
      if (result.mapping.active) await bridge.serverNetworkClose(name)
      return
    }
    mapping.value = result.mapping
    if (!mapping.value.active) throw new Error(mapping.value.error || 'Роутер не подтвердил активный проброс')
    emit('toast', 'UPnP-проброс создан. Доступность из интернета нужно проверить отдельно')
  } catch (value) {
    if (sameContext()) error.value = message(value)
  } finally {
    if (current === epoch) busy.value = ''
  }
}

async function close() {
  if (busy.value) return
  busy.value = 'close'
  error.value = ''
  try {
    mapping.value = (await bridge.serverNetworkClose(props.name)).mapping
    emit('toast', 'Правило UPnP удалено. Ручные правила роутера и firewall не изменены')
  } catch (value) {
    error.value = message(value)
  } finally {
    busy.value = ''
  }
}

async function copyAddress() {
  if (!address.value) return
  try {
    await navigator.clipboard.writeText(address.value)
    emit('toast', 'Адрес для подключения скопирован')
  } catch {
    emit('toast', 'Выделите и скопируйте адрес вручную')
  }
}

watch(() => props.name, () => {
  epoch += 1
  busy.value = ''
  report.value = undefined
  reportFresh.value = false
  reportCheckedAt.value = 0
  error.value = ''
  mapping.value = { active: false, error: '' }
  manualIp.value = ''
  externalPort.value = props.port
  void syncMapping()
})
watch(() => props.port, (value) => {
  if (!mapping.value.active) externalPort.value = value
  report.value = undefined
  reportFresh.value = false
  reportCheckedAt.value = 0
})
watch(() => props.running, () => { reportFresh.value = false })
watch(() => props.core, () => { reportFresh.value = false })
onMounted(() => {
  void syncMapping()
  timer = setInterval(() => void syncMapping(), 5000)
})
onBeforeUnmount(() => {
  epoch += 1
  if (timer) clearInterval(timer)
  // navigation away is not a request to close a running server.
})
</script>

<template>
  <section class="workspace-card network-panel">
    <div class="workspace-card-heading">
      <div><span class="eyebrow">Публичный доступ · Java TCP</span><h3>Открыть сервер в интернет</h3><span class="muted">Белый IPv4 напрямую или временный UPnP-проброс на роутере. Без VPN и relay.</span></div>
      <span class="tag" :class="{ 'tag--success': mapping.active && !mapping.error }">{{ mapping.active ? (mapping.error ? 'Проброс требует проверки' : 'UPnP-проброс создан') : 'Автопроброс выключен' }}</span>
    </div>
    <div class="network-intro"><Icon name="shield" :size="20" /><p>В интернете сервер доступен посторонним. Используйте доверенное ядро, обновления, <code>online-mode=true</code> и whitelist. Лаунчер не отключает firewall, не открывает RCON и не меняет настройки авторизации.</p></div>
    <p v-if="proxyCore" class="network-note">Для {{ core }} игровой listener задаётся в конфигурации прокси. Подготовка server.properties к нему не применяется; внутренний порт в настройках лаунчера должен совпадать с listener прокси.</p>
    <div class="network-controls">
      <div class="field"><span>Внутренний игровой порт</span><strong class="network-port">{{ port }} / TCP</strong><small>Берётся из сохранённых настроек сервера.</small></div>
      <div class="field"><span>Внешний порт</span><NumericStepper v-model="externalPort" :min="1" :max="65535" :disabled="Boolean(busy || mapping.active || mapping.closing)" aria-label="Внешний игровой TCP-порт" /><small>Может отличаться от внутреннего.</small></div>
      <button class="button button--outline" type="button" :disabled="Boolean(busy)" @click="diagnose"><Icon name="refresh" :size="15" />{{ busy === 'diagnose' ? 'Проверяю сеть…' : 'Проверить сеть' }}</button>
    </div>
    <p class="network-note">Проверка отправляет HTTPS-запрос к api.ipify.org для определения вашего внешнего IPv4 и ищет UPnP-роутер в локальной сети. Данные аккаунта не передаются.</p>
    <div v-if="error || mapping.error" class="network-error" role="alert">{{ error || mapping.error }}<span v-if="mapping.uncertain_lease">Роутер не подтвердил конечную аренду. Проверьте и удалите правило вручную в роутере; автоматическое истечение не гарантируется.</span><span v-else-if="mapping.closing">Продление выключено. Если правило не удалось удалить, оно истечёт по окончании аренды.</span></div>
    <div v-if="report" class="network-report" aria-live="polite">
      <div><span>Тип сети</span><strong>{{ natLabels[report.nat_kind] }}</strong></div>
      <div><span>Локальный IPv4</span><strong>{{ report.local_ip || 'Не определён' }}</strong></div>
      <div><span>WAN роутера</span><strong>{{ report.wan_ip || 'Не определён' }}</strong></div>
      <div><span>Внешний IPv4</span><strong>{{ report.observed_ip || report.public_ip || 'Не определён' }}</strong></div>
      <div><span>UPnP IGD</span><strong>{{ report.upnp_available ? report.router_name : 'Недоступен' }}</strong></div>
      <div><span>Локальный игровой TCP</span><strong>{{ report.local_listening ? 'Отвечает' : report.running ? 'Ещё не отвечает' : 'Сервер остановлен' }}</strong></div>
    </div>
    <p v-if="report" class="network-note">{{ reportFresh ? `Проверено в ${checkedAt}.` : 'Результаты устарели: повторите проверку перед созданием проброса или использованием автоадреса.' }}</p>
    <p v-if="report?.nat_kind === 'double_nat'" class="network-note">Внешний адрес и WAN роутера различаются. Автоадрес скрыт, чтобы не предлагать адрес, который может быть недоступен. Введите адрес внешнего маршрутизатора только если знаете, что на нём настроен проброс.</p>
    <ul v-if="report?.warnings.length" class="network-warnings"><li v-for="warning in report.warnings" :key="warning">{{ warning }}</li></ul>
    <div class="network-actions">
      <button v-if="!running && !proxyCore" class="button button--outline" type="button" :disabled="Boolean(busy)" @click="prepare"><Icon name="settings" :size="15" />Подготовить server.properties</button>
      <button v-if="!mapping.active && !mapping.closing" class="button button--accent" type="button" :disabled="Boolean(busy) || !readyForUpnp" @click="open"><Icon name="globe" :size="15" />{{ busy === 'open' ? 'Создаю проброс…' : 'Открыть через UPnP' }}</button>
      <button v-else class="button button--outline" type="button" :disabled="Boolean(busy)" @click="close"><Icon name="lock" :size="15" />{{ busy === 'close' ? 'Удаляю правило…' : 'Закрыть UPnP-проброс' }}</button>
    </div>
    <p class="network-note">Для UPnP нужны запущенный сервер, белый WAN роутера и включённый UPnP IGD. Аренда — до 10 минут, продление во время работы сервера; при остановке правило удаляется автоматически. После аварийного закрытия лаунчера оно истечёт без продления.</p>
    <div class="network-address">
      <div class="network-manual-ip">
        <label class="field"><span>Свой белый IPv4 (необязательно)</span><input v-model="manualIp" placeholder="Если автоопределение недоступно" inputmode="decimal" maxlength="15" :disabled="mapping.active" /></label>
        <button v-if="manualIp" class="button button--small button--quiet" type="button" :disabled="mapping.active" @click="manualIp = ''">Сбросить на автоадрес</button>
        <small v-if="manualIp">Используется введённый вручную адрес.</small>
      </div>
      <div class="network-address-output"><span>Адрес для друзей</span><strong>{{ address || 'Сначала определите белый IPv4' }}</strong><button class="button button--small button--quiet" type="button" :disabled="!address" @click="copyAddress"><Icon name="copy" :size="14" />Скопировать</button></div>
    </div>
    <div class="network-manual">
      <h4>Если UPnP недоступен</h4>
      <ol>
        <li>При белом IP прямо на компьютере проброс не нужен. Разрешите входящий TCP {{ port }} в firewall.</li>
        <li>За роутером создайте Port Forwarding / Virtual Server: TCP {{ externalPort }} → {{ report?.local_ip || 'локальный IP компьютера' }}:{{ port }}. Закрепите локальный IP в DHCP роутера.</li>
        <li>При CGNAT / сером WAN закажите белый IPv4 у провайдера. Если роутеров два, настройте оба или переведите внешний в bridge. UPnP не обходит NAT провайдера.</li>
        <li v-if="proxyCore">Проверьте listener прокси и совпадение его порта с {{ port }}.</li>
      </ol>
      <div v-for="help in report?.firewall_help || []" :key="help.platform" class="network-command"><span>{{ help.platform }} · выполнить самостоятельно, только если этот firewall используется</span><code>{{ help.command }}</code><small>Отменить:</small><code>{{ help.undo }}</code></div>
    </div>
    <div class="network-verification"><Icon name="info" :size="18" /><p>{{ report?.verification_note || 'Проброс на роутере не гарантирует доступность из интернета. Проверьте адрес с другого подключения, например через мобильный интернет.' }} NAT loopback может быть недоступен: друзья подключаются по внешнему адресу, вы — по локальному IP. Ручные правила закрываются вручную.</p></div>
  </section>
</template>

<style scoped>
.network-panel { display: grid; gap: 18px; }
.network-panel h3 { margin: 7px 0; }
.network-intro, .network-verification { display: flex; align-items: flex-start; gap: 12px; padding: 14px; border: 1px solid var(--line); border-radius: calc(var(--radius-control, 10px) * 1.2); background: var(--panel-soft); }
.network-intro .icon, .network-verification .icon { flex: 0 0 auto; color: var(--accent); }
.network-intro p, .network-verification p { margin: 0; color: var(--muted); font-size: 12px; line-height: 1.65; }
.network-controls { display: grid; grid-template-columns: 1fr 1fr auto; gap: 16px; align-items: end; }
.network-controls small, .network-note { margin: 0; color: var(--muted); font-size: 11px; line-height: 1.6; }
.network-port { display: block; padding: 10px 0; color: var(--text); font-size: 16px; }
.network-error { display: grid; gap: 6px; padding: 13px; border: 1px solid color-mix(in srgb, var(--danger, #ff8c8c) 45%, var(--line)); border-radius: var(--radius-control, 10px); color: var(--danger, #ff8c8c); font-size: 12px; overflow-wrap: anywhere; }
.network-report { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 10px; }
.network-report > div { display: grid; gap: 7px; padding: 13px; border: 1px solid var(--line); border-radius: var(--radius-control, 10px); }
.network-report span, .network-address-output > span { color: var(--muted); font-size: 10px; }
.network-report strong { color: var(--text); font-size: 12px; overflow-wrap: anywhere; }
.network-warnings { display: grid; gap: 7px; margin: 0; padding: 13px 13px 13px 30px; border-radius: var(--radius-control, 10px); color: var(--muted); background: var(--panel-raised); font-size: 12px; line-height: 1.6; }
.network-actions { display: flex; flex-wrap: wrap; gap: 9px; }
.network-address { display: grid; grid-template-columns: 1fr 1.3fr; gap: 16px; padding: 16px; border: 1px solid var(--line); border-radius: calc(var(--radius-control, 10px) * 1.2); }
.network-address-output { display: grid; gap: 9px; align-content: start; justify-items: start; }
.network-manual-ip { display: grid; justify-items: start; gap: 8px; }
.network-manual-ip small { color: var(--muted); font-size: 10px; }
.network-address-output strong { font-family: ui-monospace, monospace; font-size: 16px; overflow-wrap: anywhere; user-select: text; }
.network-manual { display: grid; gap: 13px; }
.network-manual h4 { margin: 0; font-size: 14px; }
.network-manual ol { display: grid; gap: 8px; margin: 0; padding-left: 20px; color: var(--muted); font-size: 12px; line-height: 1.6; }
.network-command { display: grid; gap: 7px; }
.network-command span, .network-command small { color: var(--muted); font-size: 10px; }
.network-command code { display: block; padding: 10px 12px; border: 1px solid var(--line); border-radius: var(--radius-button, 8px); color: var(--text); background: var(--panel); font-size: 11px; overflow-wrap: anywhere; white-space: pre-wrap; user-select: text; }
@media (max-width: 1000px) { .network-controls { grid-template-columns: 1fr 1fr; } .network-report { grid-template-columns: 1fr 1fr; } }
@media (max-width: 600px) { .network-controls, .network-report, .network-address { grid-template-columns: 1fr; } }
</style>
