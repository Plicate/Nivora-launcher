<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import Icon from './Icon.vue'
import type { InstanceInfo } from '../types'
import { externalJoinServer, isRatesJoinableInstance, serverAddress, type PlicateRatesApi, type PlicateRatesServer } from '../services/plicateRates'

const props = defineProps<{
  instances: InstanceInfo[]
  api?: PlicateRatesApi
}>()

const emit = defineEmits<{
  quickJoin: [payload: { server: PlicateRatesServer; instance: InstanceInfo; address: string }]
  navigateInstances: []
  toast: [message: string]
}>()

const servers = ref<PlicateRatesServer[]>([])
const categories = ref<string[]>([])
const loading = ref(false)
const loadError = ref('')
const refreshKey = ref(0)
const search = ref('')
const editionFilter = ref('all')
const categoryFilter = ref('')
const selectedServer = ref<PlicateRatesServer | null>(null)
const selectedInstanceName = ref('')
const joinDialog = ref<HTMLDialogElement | null>(null)
let requestId = 0

const visibleServers = computed(() => {
  const needle = search.value.trim().toLocaleLowerCase()
  return servers.value.filter((server) => {
    if (editionFilter.value !== 'all' && server.edition !== editionFilter.value) return false
    if (categoryFilter.value && server.category !== categoryFilter.value) return false
    if (!needle) return true
    return [server.name, server.host, server.version, server.category, ...(server.tags || [])]
      .some((value) => String(value || '').toLocaleLowerCase().includes(needle))
  })
})

const joinableInstances = computed(() => props.instances.filter(isRatesJoinableInstance))
const selectedInstance = computed(() => joinableInstances.value.find((instance) => instance.name === selectedInstanceName.value))

async function loadCatalog() {
  const currentRequest = ++requestId
  if (!props.api) {
    servers.value = []
    categories.value = []
    loadError.value = 'Каталог Plicate Rates пока не подключён.'
    loading.value = false
    return
  }
  loading.value = true
  loadError.value = ''
  try {
    const catalog = await props.api.listServers()
    if (currentRequest !== requestId) return
    servers.value = catalog.servers
    categories.value = catalog.categories
  } catch (error) {
    if (currentRequest !== requestId) return
    loadError.value = error instanceof Error ? error.message : 'Не удалось загрузить Plicate Rates.'
  } finally {
    if (currentRequest === requestId) loading.value = false
  }
}

watch(() => props.api, () => { void loadCatalog() })
watch(refreshKey, () => { void loadCatalog() })
onMounted(() => { void loadCatalog() })
onBeforeUnmount(() => { requestId += 1 })

function displayedAddress(server: PlicateRatesServer): string {
  return serverAddress(server)
}

async function copyAddress(server: PlicateRatesServer) {
  const address = displayedAddress(server)
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(address)
    } else {
      const input = document.createElement('textarea')
      input.value = address
      input.setAttribute('readonly', '')
      input.style.position = 'fixed'
      input.style.opacity = '0'
      document.body.append(input)
      input.select()
      const copied = document.execCommand('copy')
      input.remove()
      if (!copied) throw new Error('Браузер не разрешил скопировать адрес.')
    }
  } catch (error) {
    emit('toast', error instanceof Error ? error.message : 'Не удалось скопировать IP.')
    return
  }
  emit('toast', `Адрес ${address} скопирован.`)
  try {
    await props.api?.recordCopy(server.slug)
  } catch {
    // Копирование уже удалось; недоступность счётчика не должна отменять это действие.
  }
}

function openJoinChooser(server: PlicateRatesServer) {
  if (server.edition !== 'Java') {
    emit('toast', 'Быстрый вход пока доступен только для Java-серверов. Скопируйте IP и подключитесь через игру.')
    return
  }
  selectedServer.value = server
  selectedInstanceName.value = joinableInstances.value.find((instance) => !instance.running)?.name || ''
  joinDialog.value?.showModal()
}

function closeJoinChooser() {
  joinDialog.value?.close()
  selectedServer.value = null
}

function quickJoin() {
  const server = selectedServer.value
  const instance = selectedInstance.value
  if (!server || !instance || instance.running) return
  emit('quickJoin', { server, instance, address: displayedAddress(server) })
  closeJoinChooser()
}

/// открывает выбор экземпляра для сервера из ссылки nivora://join (сайт Plicate Rates)
function openExternalJoin(target: { host: string; port?: number | string; edition?: string; name?: string }) {
  const server = externalJoinServer(target)
  if (!server) return
  openJoinChooser(server)
}

defineExpose({ openExternalJoin })
</script>

<template>
  <section class="plicate-rates">
    <header class="rates-heading">
      <div class="rates-brand" aria-hidden="true"><Icon name="globe" :size="19" /></div>
      <div class="rates-heading-copy">
        <span class="rates-kicker">Экосистема Plicate</span>
        <h2>Рейтинг серверов</h2>
        <p>Проверенные Minecraft-серверы, актуальный онлайн и быстрый вход из лаунчера.</p>
      </div>
      <button class="button button--quiet rates-refresh" type="button" :disabled="loading" @click="refreshKey += 1">
        <Icon name="refresh" :size="14" />{{ loading ? 'Обновляю…' : 'Обновить' }}
      </button>
    </header>

    <div class="rates-toolbar">
      <label class="rates-search"><Icon name="search" :size="16" /><input v-model="search" type="search" placeholder="Сервер, адрес или режим" aria-label="Поиск серверов Plicate Rates" /></label>
      <label class="rates-filter"><span>Издание</span><select v-model="editionFilter" aria-label="Фильтр по изданию"><option value="all">Все</option><option value="Java">Java</option><option value="Bedrock">Bedrock</option></select></label>
      <label v-if="categories.length" class="rates-filter"><span>Режим</span><select v-model="categoryFilter" aria-label="Фильтр по режиму"><option value="">Все режимы</option><option v-for="category in categories" :key="category" :value="category">{{ category }}</option></select></label>
    </div>

    <p v-if="loadError" class="rates-state rates-state--error" role="alert">{{ loadError }} <button class="text-button" type="button" :disabled="loading" @click="refreshKey += 1">Повторить</button></p>
    <div v-else-if="loading && !servers.length" class="rates-state" role="status"><span class="status-dot status-dot--busy" />Загружаю каталог серверов…</div>
    <div v-else-if="!visibleServers.length" class="rates-state">
      <Icon name="server" :size="20" />
      <strong>{{ servers.length ? 'Серверы не найдены' : 'Пока нет подтверждённых серверов' }}</strong>
      <span>{{ servers.length ? 'Измените запрос или снимите фильтр.' : 'В Rates публикуются серверы с подтверждённым владением.' }}</span>
    </div>

    <div v-else class="rates-server-list">
      <article v-for="server in visibleServers" :key="server.slug" class="rates-server-card">
        <div class="rates-server-mark"><img v-if="server.icon" :src="server.icon" :alt="`Иконка ${server.name}`" /><Icon v-else name="server" :size="20" /></div>
        <div class="rates-server-main">
          <div class="rates-title-row"><span v-if="server.rank" class="rates-rank">#{{ server.rank }}</span><h3>{{ server.name }}</h3><span class="rates-edition">{{ server.edition }}</span></div>
          <p v-if="server.description" class="rates-description">{{ server.description }}</p>
          <p v-if="server.motd" class="rates-motd"><span>MOTD</span>{{ server.motd }}</p>
          <div class="rates-address-row"><code>{{ displayedAddress(server) }}</code><button class="text-button rates-copy" type="button" :aria-label="`Скопировать IP ${displayedAddress(server)}`" @click="copyAddress(server)"><Icon name="copy" :size="14" />Копировать IP</button></div>
          <div class="rates-meta"><span><Icon name="archive" :size="13" />{{ server.version || 'Версия не указана' }}</span><span><Icon name="mods" :size="13" />{{ server.category || 'Minecraft' }}</span><span v-if="server.reviews"><Icon name="star" :size="13" />{{ Number(server.rating || 0).toFixed(1) }} · {{ server.reviews }}</span></div>
          <div v-if="server.tags?.length" class="rates-tags"><span v-for="tag in server.tags" :key="tag">{{ tag }}</span></div>
        </div>
        <div class="rates-server-side">
          <span class="rates-online" :class="{ 'rates-online--unknown': server.online === null || !server.maximum }"><i v-if="server.online !== null && server.maximum" />{{ server.online === null || !server.maximum ? 'Нет ответа' : `${server.online} / ${server.maximum}` }}</span>
          <small>игроков онлайн</small>
          <button class="button button--accent rates-join" type="button" :disabled="server.edition !== 'Java'" :title="server.edition !== 'Java' ? 'Быстрый вход доступен для Java-серверов' : undefined" @click="openJoinChooser(server)"><Icon name="play" :size="14" />Быстрый вход</button>
        </div>
      </article>
    </div>

    <dialog ref="joinDialog" class="rates-join-dialog" aria-labelledby="rates-join-title" @close="selectedServer = null">
      <div class="rates-dialog-heading">
        <div><span class="rates-kicker">Быстрый вход</span><h2 id="rates-join-title">Выберите Java-экземпляр</h2><p v-if="selectedServer">Подключение к <strong>{{ selectedServer.name }}</strong> · {{ displayedAddress(selectedServer) }}</p></div>
        <button class="icon-button" type="button" aria-label="Закрыть" @click="closeJoinChooser"><Icon name="close" :size="16" /></button>
      </div>
      <div v-if="joinableInstances.length" class="rates-instance-options" role="radiogroup" aria-label="Экземпляр для подключения">
        <label v-for="instance in joinableInstances" :key="instance.name" class="rates-instance-option" :class="{ 'rates-instance-option--selected': selectedInstanceName === instance.name, 'rates-instance-option--disabled': instance.running }">
          <input v-model="selectedInstanceName" type="radio" name="rates-instance" :value="instance.name" :disabled="instance.running" />
          <span class="rates-instance-icon"><Icon :name="instance.running ? 'play' : 'instances'" :size="16" /></span>
          <span class="rates-instance-copy"><strong>{{ instance.display_name || instance.name }}</strong><small>{{ instance.minecraft_version || instance.version_id || 'Версия не указана' }} · {{ instance.loader || 'Vanilla' }}{{ instance.running ? ' · уже запущен' : '' }}</small></span>
        </label>
      </div>
      <div v-else class="rates-state rates-state--compact"><strong>Нет Java-экземпляров</strong><span>Создайте или импортируйте Java-экземпляр, чтобы подключиться через лаунчер.</span><button class="button button--quiet" type="button" @click="emit('navigateInstances'); closeJoinChooser()">Открыть экземпляры</button></div>
      <p v-if="joinableInstances.length && !selectedInstance" class="rates-inline-note">Выберите незапущенный Java-экземпляр. Уже работающую игру можно подключить к серверу вручную.</p>
      <footer class="rates-dialog-actions"><button class="button button--quiet" type="button" @click="closeJoinChooser">Отмена</button><button class="button button--accent" type="button" :disabled="!selectedInstance || selectedInstance.running" @click="quickJoin"><Icon name="play" :size="14" />{{ selectedInstance ? `Запустить ${selectedInstance.display_name || selectedInstance.name}` : 'Выберите экземпляр' }}</button></footer>
    </dialog>
  </section>
</template>

<style scoped>
.plicate-rates { display: grid; gap: 14px; }
.rates-heading { display: flex; align-items: center; gap: 13px; padding: 18px 20px; border: 1px solid var(--line); border-radius: var(--radius-card, 12px); background: linear-gradient(115deg, color-mix(in srgb, var(--blue) 10%, var(--panel)), var(--panel)); }
.rates-brand { display: grid; width: 42px; height: 42px; flex: 0 0 auto; place-items: center; border: 1px solid color-mix(in srgb, var(--blue) 30%, var(--line)); border-radius: 12px; background: color-mix(in srgb, var(--blue) 14%, var(--panel-raised)); color: var(--blue-bright); }
.rates-heading-copy { min-width: 0; flex: 1; }
.rates-kicker { color: var(--blue-bright); font-size: 10px; font-weight: 700; letter-spacing: .08em; text-transform: uppercase; }
.rates-heading h2 { margin: 2px 0 0; font-size: 20px; letter-spacing: -.035em; }
.rates-heading p { margin: 4px 0 0; color: var(--muted); font-size: 12px; }
.rates-refresh { flex: 0 0 auto; }
.rates-toolbar { display: flex; flex-wrap: wrap; align-items: end; gap: 10px; }
.rates-search { display: flex; min-width: min(100%, 250px); height: 40px; flex: 1; align-items: center; gap: 9px; padding: 0 11px; border: 1px solid var(--line); border-radius: var(--radius-control); background: var(--panel); color: var(--muted); }
.rates-search input { width: 100%; min-width: 0; border: 0; outline: 0; background: transparent; color: var(--text); font-size: 12px; }
.rates-filter { display: grid; min-width: 135px; gap: 4px; color: var(--muted); font-size: 10px; }
.rates-filter select { min-height: 40px; font-size: 12px; }
.rates-state { display: flex; min-height: 132px; flex-direction: column; align-items: center; justify-content: center; gap: 8px; padding: 18px; border: 1px dashed var(--line-strong); border-radius: var(--radius-card, 12px); color: var(--muted); text-align: center; font-size: 12px; }
.rates-state > strong { color: var(--text); font-size: 14px; }
.rates-state--error { min-height: 74px; flex-direction: row; flex-wrap: wrap; border-color: color-mix(in srgb, var(--danger) 50%, var(--line)); color: var(--text); }
.rates-server-list { display: grid; gap: 9px; }
.rates-server-card { display: grid; grid-template-columns: 40px minmax(0, 1fr) 150px; align-items: start; gap: 13px; padding: 15px; border: 1px solid var(--line); border-radius: var(--radius-card, 12px); background: var(--panel); transition: border-color .16s ease, background .16s ease; }
.rates-server-card:hover { border-color: var(--line-strong); background: color-mix(in srgb, var(--panel-raised) 42%, var(--panel)); }
.rates-server-mark { display: grid; width: 38px; height: 38px; place-items: center; overflow: hidden; border: 1px solid var(--line); border-radius: 11px; background: var(--panel-raised); color: var(--blue-bright); }
.rates-server-mark img { width: 100%; height: 100%; object-fit: cover; image-rendering: pixelated; }
.rates-motd { display: flex; flex-wrap: wrap; gap: 4px 7px; margin: 6px 0 0; color: var(--text); font-size: 10px; line-height: 1.4; overflow-wrap: anywhere; }
.rates-motd span { flex: 0 0 auto; color: var(--muted); font-size: 9px; font-weight: 700; letter-spacing: .05em; text-transform: uppercase; }
.rates-server-main { min-width: 0; }
.rates-title-row { display: flex; flex-wrap: wrap; align-items: center; gap: 7px; }
.rates-title-row h3 { min-width: 0; margin: 0; overflow: hidden; font-size: 15px; font-weight: 650; text-overflow: ellipsis; }
.rates-rank { color: var(--muted); font-size: 10px; font-variant-numeric: tabular-nums; }
.rates-edition { padding: 2px 7px; border: 1px solid var(--line); border-radius: 999px; color: var(--muted); font-size: 9px; }
.rates-description { display: -webkit-box; margin: 5px 0 0; overflow: hidden; color: var(--muted); font-size: 11px; -webkit-box-orient: vertical; -webkit-line-clamp: 2; }
.rates-address-row { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 9px; margin-top: 6px; }
.rates-address-row code { color: var(--quiet); font: 11px var(--font-mono, ui-monospace, monospace); overflow-wrap: anywhere; }
.text-button { display: inline-flex; align-items: center; gap: 5px; padding: 2px 0; border: 0; background: transparent; color: var(--blue-bright); cursor: pointer; font-size: 10px; }
.text-button:disabled { opacity: .5; cursor: wait; }
.rates-meta { display: flex; flex-wrap: wrap; gap: 4px 11px; margin-top: 8px; color: var(--muted); font-size: 10px; }
.rates-meta > span { display: inline-flex; align-items: center; gap: 4px; }
.rates-tags { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 7px; }
.rates-tags > span { padding: 2px 7px; border: 1px solid var(--line); border-radius: 999px; color: var(--muted); font-size: 9px; }
.rates-server-side { display: grid; justify-items: end; gap: 4px; padding-top: 2px; text-align: right; }
.rates-online { display: inline-flex; align-items: center; gap: 6px; color: var(--green); font-size: 13px; font-variant-numeric: tabular-nums; font-weight: 680; }
.rates-online i { width: 7px; height: 7px; border-radius: 50%; background: currentColor; box-shadow: 0 0 0 3px color-mix(in srgb, currentColor 16%, transparent); }
.rates-online--unknown { color: var(--muted); }
.rates-server-side > small { color: var(--muted); font-size: 9px; }
.rates-join { width: 100%; margin-top: 5px; }
.rates-join-dialog { width: min(540px, calc(100% - 28px)); max-height: min(80vh, 650px); padding: 20px; border: 1px solid var(--line-strong); border-radius: var(--radius-card, 12px); background: var(--panel); color: var(--text); box-shadow: 0 24px 80px #0008; }
.rates-join-dialog::backdrop { background: #0009; backdrop-filter: blur(3px); }
.rates-dialog-heading { display: flex; align-items: start; justify-content: space-between; gap: 12px; }
.rates-dialog-heading h2 { margin: 3px 0 0; font-size: 18px; }
.rates-dialog-heading p { margin: 5px 0 0; color: var(--muted); font-size: 11px; }
.rates-dialog-heading p strong { color: var(--text); }
.rates-instance-options { display: grid; max-height: 44vh; gap: 6px; margin-top: 16px; overflow: auto; }
.rates-instance-option { display: flex; align-items: center; gap: 10px; padding: 10px; border: 1px solid var(--line); border-radius: 9px; background: var(--panel-raised); cursor: pointer; }
.rates-instance-option--selected { border-color: var(--blue); background: color-mix(in srgb, var(--blue) 9%, var(--panel-raised)); }
.rates-instance-option--disabled { opacity: .55; cursor: not-allowed; }
.rates-instance-option input { accent-color: var(--blue); }
.rates-instance-icon { color: var(--blue-bright); }
.rates-instance-copy { display: grid; min-width: 0; gap: 2px; }
.rates-instance-copy strong { overflow: hidden; font-size: 12px; text-overflow: ellipsis; white-space: nowrap; }
.rates-instance-copy small { color: var(--muted); font-size: 10px; }
.rates-state--compact { min-height: 150px; margin-top: 15px; }
.rates-inline-note { margin: 12px 0 0; color: var(--muted); font-size: 10px; }
.rates-dialog-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; }
@media (max-width: 720px) {
  .rates-server-card { grid-template-columns: 36px minmax(0, 1fr); }
  .rates-server-mark { width: 34px; height: 34px; }
  .rates-server-side { grid-column: 2; grid-template-columns: auto 1fr; align-items: center; justify-items: start; gap: 5px 9px; padding: 0; text-align: left; }
  .rates-server-side > small { align-self: center; }
  .rates-join { grid-column: 1 / -1; width: auto; margin-top: 3px; }
  .rates-heading { align-items: flex-start; }
  .rates-refresh { padding-inline: 8px; }
}
@media (max-width: 470px) {
  .rates-heading { flex-wrap: wrap; padding: 14px; }
  .rates-heading-copy { flex-basis: calc(100% - 58px); }
  .rates-refresh { margin-left: auto; }
  .rates-toolbar { align-items: stretch; }
  .rates-search { flex-basis: 100%; }
  .rates-filter { flex: 1; }
  .rates-server-card { padding: 12px; }
}
</style>
