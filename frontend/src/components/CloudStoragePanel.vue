<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue'
import Icon from './Icon.vue'
import { bridge } from '../services/bridge'
import { confirmDialog } from '../services/dialogs'
import type { CloudFile, CloudStatus } from '../types'

const props = defineProps<{
  url: string
  username: string
  remotePath: string
  includeAccounts: boolean
}>()

const emit = defineEmits<{
  saved: [settings: { cloud_url: string; cloud_username: string; cloud_remote_path: string; cloud_include_accounts: boolean; cloud_configured: boolean }]
  toast: [message: string]
  changed: []
}>()

const form = reactive({
  url: '',
  username: '',
  remotePath: 'nivora-launcher',
  includeAccounts: false,
})
const password = ref('')
const files = ref<CloudFile[]>([])
const status = ref<CloudStatus>({
  provider: 'webdav',
  configured: false,
  endpoint: '',
  username: '',
  remote_path: 'nivora-launcher',
  include_accounts: false,
  last_sync: 0,
})
const busyAction = ref('')
const message = ref('')
const messageIsError = ref(false)
const connectionState = ref<'unknown' | 'available' | 'unavailable'>('unknown')

const statusLabel = computed(() => {
  if (busyAction.value) return busyAction.value
  if (connectionState.value === 'available') return 'Соединение проверено'
  if (connectionState.value === 'unavailable') return 'Проверка не пройдена'
  if (status.value.configured) return 'Настроено'
  return 'Только локально'
})

watch(
  () => [props.url, props.username, props.remotePath, props.includeAccounts],
  () => {
    if (busyAction.value) return
    form.url = props.url
    form.username = props.username
    form.remotePath = props.remotePath || 'nivora-launcher'
    form.includeAccounts = props.includeAccounts
    status.value = {
      ...status.value,
      endpoint: form.url,
      username: form.username,
      remote_path: form.remotePath,
      include_accounts: form.includeAccounts,
      configured: status.value.configured && Boolean(form.url && form.username),
    }
  },
  { immediate: true },
)

function syncForm(settings: { cloud_url: string; cloud_username: string; cloud_remote_path: string; cloud_include_accounts: boolean; cloud_configured: boolean }) {
  form.url = settings.cloud_url
  form.username = settings.cloud_username
  form.remotePath = settings.cloud_remote_path
  form.includeAccounts = settings.cloud_include_accounts
  status.value.configured = settings.cloud_configured
  emit('saved', settings)
}

function applyStatus(next: CloudStatus, reachable?: boolean) {
  status.value = next
  form.url = next.endpoint
  form.username = next.username
  form.remotePath = next.remote_path || 'nivora-launcher'
  form.includeAccounts = next.include_accounts
  if (reachable === true) connectionState.value = 'available'
  else if (reachable === false) connectionState.value = 'unknown'
}

function setMessage(value: string, isError = false) {
  message.value = value
  messageIsError.value = isError
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

async function configure() {
  busyAction.value = 'Сохраняю подключение…'
  setMessage('')
  try {
    const result = await bridge.cloudConfigure({
      url: form.url.trim(),
      username: form.username.trim(),
      remote_path: form.remotePath.trim(),
      include_accounts: form.includeAccounts,
      ...(password.value ? { password: password.value } : {}),
    })
    applyStatus(result.status, false)
    syncForm({
      cloud_url: result.settings.cloud_url,
      cloud_username: result.settings.cloud_username,
      cloud_remote_path: result.settings.cloud_remote_path,
      cloud_include_accounts: result.settings.cloud_include_accounts,
      cloud_configured: result.settings.cloud_configured,
    })
    password.value = ''
    setMessage('Настройки облака сохранены локально.')
    emit('toast', 'Настройки облака сохранены')
  } catch (error) {
    setMessage(errorText(error), true)
  } finally {
    busyAction.value = ''
  }
}

async function testConnection() {
  busyAction.value = 'Проверяю WebDAV…'
  setMessage('')
  try {
    const result = await bridge.cloudTest()
    applyStatus(result.status, true)
    files.value = result.files
    setMessage(`Подключение работает. Файлов в облаке: ${result.files.length}.`)
  } catch (error) {
    connectionState.value = 'unavailable'
    setMessage(errorText(error), true)
  } finally {
    busyAction.value = ''
  }
}

async function refreshFiles() {
  busyAction.value = 'Получаю список файлов…'
  setMessage('')
  try {
    const result = await bridge.cloudList()
    applyStatus(result.status, true)
    files.value = result.files
  } catch (error) {
    connectionState.value = 'unavailable'
    setMessage(errorText(error), true)
  } finally {
    busyAction.value = ''
  }
}

async function syncNow() {
  busyAction.value = 'Сохраняю файлы…'
  setMessage('')
  try {
    const result = await bridge.cloudSync()
    applyStatus(result.status, true)
    files.value = result.files
    setMessage(`Сохранено файлов: ${result.result.uploaded}. ${formatBytes(result.result.bytes)}.`)
    emit('toast', 'Файлы сохранены в облако')
  } catch (error) {
    connectionState.value = 'unavailable'
    setMessage(errorText(error), true)
  } finally {
    busyAction.value = ''
  }
}

async function restoreNow() {
  busyAction.value = 'Проверяю изменения…'
  setMessage('')
  try {
    const preview = await bridge.cloudRestorePreview()
    const changes = preview.result.changes || []
    const listed = changes.slice(0, 18).map((item) => `${item.action === 'create' ? '+' : item.action === 'replace' ? '~' : '↔'} ${item.key}`).join('\n')
    const more = changes.length > 18 ? `\n… и ещё ${changes.length - 18}` : ''
    const detail = `${listed || 'Нет разрешённых файлов'}${more}\n\nПропущено защищённых файлов: ${preview.result.skipped}. Размер: ${formatBytes(preview.result.bytes)}.`
    if (!(await confirmDialog('Подтвердить восстановление из облака?', { danger: true, confirmLabel: 'Восстановить', detail }))) return
    busyAction.value = 'Восстанавливаю файлы…'
    const result = await bridge.cloudRestore(preview.result.manifest_id)
    applyStatus(result.status, true)
    emit('changed')
    setMessage(`Восстановлено файлов: ${result.result.restored}. ${formatBytes(result.result.bytes)}.`)
    emit('toast', 'Файлы восстановлены из облака')
  } catch (error) {
    connectionState.value = 'unavailable'
    setMessage(errorText(error), true)
  } finally {
    busyAction.value = ''
  }
}

async function deleteFile(file: CloudFile) {
  if (!(await confirmDialog(`Удалить «${file.key}» из облака?`, { danger: true, confirmLabel: 'Удалить' }))) return
  busyAction.value = 'Удаляю файл…'
  setMessage('')
  try {
    const result = await bridge.cloudDelete(file.key)
    applyStatus(result.status, true)
    files.value = result.files
    setMessage('Файл удалён из облака.')
  } catch (error) {
    connectionState.value = 'unavailable'
    setMessage(errorText(error), true)
  } finally {
    busyAction.value = ''
  }
}

async function disconnect() {
  if (!(await confirmDialog('Отключить облако и удалить сохранённый пароль с этого устройства?', { danger: true, confirmLabel: 'Отключить' }))) return
  busyAction.value = 'Отключаю облако…'
  setMessage('')
  try {
    const result = await bridge.cloudDisconnect()
    applyStatus(result.status, false)
    files.value = []
    password.value = ''
    syncForm({
      cloud_url: result.settings.cloud_url,
      cloud_username: result.settings.cloud_username,
      cloud_remote_path: result.settings.cloud_remote_path,
      cloud_include_accounts: result.settings.cloud_include_accounts,
      cloud_configured: result.settings.cloud_configured,
    })
    setMessage('Облако отключено. Удалённые файлы не затронуты.')
  } catch (error) {
    setMessage(errorText(error), true)
  } finally {
    busyAction.value = ''
  }
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} Б`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} КиБ`
  if (value < 1024 * 1024 * 1024) return `${(value / 1024 / 1024).toFixed(1)} МиБ`
  return `${(value / 1024 / 1024 / 1024).toFixed(1)} ГиБ`
}

function formatDate(value: number): string {
  return value ? new Date(value * 1000).toLocaleString('ru-RU') : '—'
}

onMounted(async () => {
  try {
    const result = await bridge.cloudStatus()
    applyStatus(result.status)
  } catch {
    // The settings form remains usable when the desktop bridge is unavailable.
  }
})
</script>

<template>
  <section class="panel settings-card settings-card--wide cloud-card">
    <div class="panel-heading">
      <div>
        <span class="eyebrow">Синхронизация</span>
        <h2><Icon name="cloud" :size="20" /> Файлы в облаке</h2>
          <p>WebDAV сохраняет разрешённые файлы лаунчера, экземпляров, скинов и настроек. Плагины и служебные данные исключены. Пароль и ключ проверки манифеста остаются на этом устройстве: восстановить такую копию на другом устройстве сейчас нельзя.</p>
      </div>
      <span class="cloud-status-pill" :class="{ 'cloud-status-pill--active': connectionState === 'available', 'cloud-status-pill--error': connectionState === 'unavailable' }"><span class="status-dot" :class="{ 'status-dot--green': connectionState === 'available', 'status-dot--red': connectionState === 'unavailable' }" />{{ statusLabel }}</span>
    </div>
    <div class="cloud-layout">
      <div class="settings-form">
        <label class="field field--wide"><span>WebDAV endpoint <small>адрес сервиса или личной папки</small></span><input v-model="form.url" autocomplete="url" placeholder="https://cloud.example.com/remote.php/dav/files/user" /></label>
        <div class="field-grid">
          <label class="field"><span>Логин</span><input v-model="form.username" autocomplete="username" placeholder="user@example.com" /></label>
          <label class="field"><span>Пароль или app token <small>не показывается повторно</small></span><input v-model="password" type="password" autocomplete="new-password" placeholder="Оставьте пустым, чтобы сохранить прежний" /></label>
        </div>
        <label class="field field--wide"><span>Удалённая папка <small>создаётся автоматически</small></span><input v-model="form.remotePath" placeholder="nivora-launcher" /></label>
        <div class="cloud-actions">
          <button class="button button--accent" type="button" :disabled="Boolean(busyAction)" @click="configure"><Icon name="save" :size="15" />Сохранить подключение</button>
          <button class="button button--outline" type="button" :disabled="Boolean(busyAction) || !status.configured" @click="testConnection"><Icon name="plug" :size="15" />Проверить</button>
          <button class="button button--quiet" type="button" :disabled="Boolean(busyAction) || !status.configured" @click="disconnect"><Icon name="unlink" :size="15" />Отключить</button>
        </div>
        <p v-if="message" class="cloud-message" :class="{ 'cloud-message--error': messageIsError }" :role="messageIsError ? 'alert' : 'status'">{{ message }}</p>
      </div>
      <div class="cloud-files-panel">
        <div class="cloud-files-heading"><div><strong>Удалённые файлы</strong><small>{{ files.length }} файлов<span v-if="status.last_sync"> · синхронизация {{ formatDate(status.last_sync) }}</span></small></div><button class="icon-button" type="button" aria-label="Обновить список файлов" :disabled="Boolean(busyAction) || !status.configured" @click="refreshFiles"><Icon name="refresh" :size="16" /></button></div>
        <div v-if="files.length" class="cloud-file-list">
          <div v-for="file in files" :key="file.key" class="cloud-file-row"><span class="cloud-file-icon"><Icon name="file" :size="15" /></span><span class="cloud-file-copy"><strong v-tooltip="file.key">{{ file.key }}</strong><small>{{ formatBytes(file.size) }} · {{ formatDate(file.modified_at) }}</small></span><button class="icon-button icon-button--danger" type="button" aria-label="Удалить файл" :disabled="Boolean(busyAction)" @click="deleteFile(file)"><Icon name="trash" :size="14" /></button></div>
        </div>
        <div v-else class="cloud-files-empty"><Icon name="cloud-off" :size="22" /><span>{{ status.configured ? 'Список ещё не загружен.' : 'Подключите WebDAV, чтобы увидеть файлы.' }}</span></div>
        <div class="cloud-file-actions"><button class="button button--small button--quiet" type="button" :disabled="Boolean(busyAction) || !status.configured" @click="refreshFiles"><Icon name="folder" :size="14" />Список</button><button class="button button--small button--accent" type="button" :disabled="Boolean(busyAction) || !status.configured" @click="syncNow"><Icon name="upload" :size="14" />Синхронизировать</button><button class="button button--small button--outline" type="button" :disabled="Boolean(busyAction) || !status.configured" @click="restoreNow"><Icon name="download" :size="14" />Восстановить</button></div>
      </div>
    </div>
  </section>
</template>
