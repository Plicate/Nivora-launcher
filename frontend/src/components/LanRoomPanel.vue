<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import Icon from './Icon.vue'
import NumericStepper from './NumericStepper.vue'
import { bridge } from '../services/bridge'
import { connectLanRoomPlugin } from '../plugins/lan-room'
import type { LanRoomCredentials, LanRoomStatus } from '../types'

const emit = defineEmits<{ toast: [message: string] }>()
const api = connectLanRoomPlugin({
  lanSessionCreate: bridge.lanSessionCreate,
  lanSessionJoin: bridge.lanSessionJoin,
  lanSessionStatus: bridge.lanSessionStatus,
  lanSessionLeave: bridge.lanSessionLeave,
})
const roomName = ref('Комната друзей')
const nickname = ref('Player')
const password = ref('')
const gamePort = ref(25565)
const hostControlPort = ref(0)
const joinAddress = ref('')
const joinControlPort = ref(0)
const status = ref<LanRoomStatus>()
const sessionCredentials = ref<LanRoomCredentials>()
const busy = ref(false)
const statusBusy = ref(false)
const error = ref('')
const isHost = computed(() => status.value?.local_role === 'host')
let statusTimer: number | undefined

function errorMessage(value: unknown): string {
  if (value instanceof Error) return value.message
  return typeof value === 'string' ? value : 'Неизвестная ошибка'
}

function credentials(controlPort: number, hostAddress?: string): LanRoomCredentials {
  return {
    room_name: roomName.value.trim(),
    password: password.value,
    player_nickname: nickname.value.trim(),
    host_control_port: Number(controlPort),
    minecraft_game_port: Number(gamePort.value),
    ...(hostAddress ? { host_address: hostAddress.trim() } : {}),
  }
}

async function createRoom() {
  if (busy.value) return
  busy.value = true
  error.value = ''
  const payload = credentials(hostControlPort.value)
  try {
    status.value = await api.create(payload)
    sessionCredentials.value = { ...payload, host_control_port: status.value.host_control_port }
    password.value = ''
    hostControlPort.value = status.value.host_control_port
    emit('toast', 'LAN-комната создана')
    scheduleStatusRefresh()
  } catch (value) {
    error.value = errorMessage(value)
  } finally {
    busy.value = false
  }
}

async function joinRoom() {
  if (busy.value) return
  busy.value = true
  error.value = ''
  const payload = credentials(joinControlPort.value, joinAddress.value)
  try {
    status.value = await api.join(payload)
    sessionCredentials.value = payload
    password.value = ''
    emit('toast', 'Вы подключились к LAN-комнате')
    scheduleStatusRefresh()
  } catch (value) {
    error.value = errorMessage(value)
  } finally {
    busy.value = false
  }
}

function scheduleStatusRefresh(delay = 2500) {
  if (statusTimer) window.clearTimeout(statusTimer)
  if (!sessionCredentials.value) return
  statusTimer = window.setTimeout(() => void refreshStatus(), delay)
}

function returnToConnectionForm(payload: LanRoomCredentials, message: string) {
  const wasHost = status.value?.local_role === 'host'
  if (statusTimer) window.clearTimeout(statusTimer)
  statusTimer = undefined
  roomName.value = payload.room_name
  nickname.value = payload.player_nickname
  gamePort.value = payload.minecraft_game_port
  if (wasHost) {
    hostControlPort.value = payload.host_control_port
  } else {
    joinAddress.value = payload.host_address || joinAddress.value
    joinControlPort.value = payload.host_control_port
  }
  status.value = undefined
  sessionCredentials.value = undefined
  error.value = `${message} ${wasHost ? 'Создайте комнату заново' : 'Подключитесь заново'}, повторно введя пароль.`
}

async function refreshStatus() {
  const payload = sessionCredentials.value
  if (!payload || statusBusy.value) return
  statusBusy.value = true
  try {
    status.value = await api.status(payload)
    error.value = ''
  } catch (value) {
    returnToConnectionForm(payload, errorMessage(value))
  } finally {
    statusBusy.value = false
    scheduleStatusRefresh()
  }
}

async function leaveRoom() {
  const payload = sessionCredentials.value
  if (!payload || busy.value) return
  busy.value = true
  if (statusTimer) window.clearTimeout(statusTimer)
  statusTimer = undefined
  try {
    await api.leave(payload)
    status.value = undefined
    sessionCredentials.value = undefined
    password.value = ''
    error.value = ''
    emit('toast', 'Вы вышли из LAN-комнаты')
  } catch (value) {
    returnToConnectionForm(payload, errorMessage(value))
  } finally {
    busy.value = false
  }
}

async function copyRoomInfo() {
  if (!status.value) return
  const value = [
    `Комната: ${status.value.room_name}`,
    `Адрес комнаты: ${status.value.host_address}`,
    `Порт комнаты: ${status.value.host_control_port}`,
    `Порт Minecraft: ${status.value.minecraft_game_port}`,
  ].join('\n')
  try {
    await navigator.clipboard.writeText(value)
    emit('toast', 'Адреса комнаты скопированы без пароля')
  } catch {
    error.value = 'Не удалось скопировать данные комнаты. Передайте адрес хоста и порты вручную.'
  }
}

onBeforeUnmount(() => {
  if (statusTimer) window.clearTimeout(statusTimer)
})
</script>

<template>
  <section class="panel lan-room-panel">
      <div class="lan-room-heading">
      <span class="lan-room-icon"><Icon name="globe" :size="20" /></span>
      <div><span class="eyebrow">Встроенная сетевая функция</span><h3>LAN-комнаты</h3><p>Туннель работает только между устройствами с прямой доступностью по локальной сети и передаёт один игровой TCP-порт. Это не VPN и не соединяет разные сети через интернет.</p></div>
      <span class="plugin-trust plugin-trust--verified">Встроенный</span>
    </div>

    <template v-if="!status">
      <div class="lan-room-common-fields">
        <label class="field"><span>Название комнаты</span><input v-model="roomName" maxlength="64" placeholder="Например, Minecraft вечер" /></label>
        <label class="field"><span>Ваш ник</span><input v-model="nickname" maxlength="32" autocomplete="nickname" placeholder="Игровой ник" /></label>
        <label class="field"><span>Пароль комнаты</span><input v-model="password" type="password" minlength="8" maxlength="256" autocomplete="new-password" placeholder="Не менее 8 символов" /></label>
        <div class="field"><span>Порт Minecraft</span><NumericStepper v-model="gamePort" :min="1" :max="65535" aria-label="Порт Minecraft" /></div>
      </div>
      <p v-if="error" class="inline-error lan-room-error">{{ error }}</p>
      <div class="lan-room-actions">
        <article class="lan-room-action-card">
          <div><strong>Создать комнату</strong><p>Участникам нужна доступность к этому компьютеру по локальному адресу. Разрешите порт комнаты во входящих правилах firewall.</p></div>
          <div class="field"><span>Порт комнаты · 0 = автоматически</span><NumericStepper v-model="hostControlPort" :min="0" :max="65535" aria-label="Порт комнаты для создания" /></div>
          <button class="button button--accent" type="button" :disabled="busy || !roomName.trim() || !nickname.trim() || password.length < 8 || gamePort < 1 || gamePort > 65535" @click="createRoom"><Icon name="plus" :size="15" />{{ busy ? 'Создаю…' : 'Создать комнату' }}</button>
        </article>
        <article class="lan-room-action-card">
          <div><strong>Подключиться к комнате</strong><p>Введите частный IP-адрес компьютера хоста. Устройства должны иметь маршрут друг к другу в локальной сети.</p></div>
          <label class="field"><span>Адрес комнаты</span><input v-model="joinAddress" placeholder="192.168.1.20" autocomplete="off" /></label>
          <div class="field"><span>Порт комнаты</span><NumericStepper v-model="joinControlPort" :min="1" :max="65535" aria-label="Порт комнаты для подключения" /></div>
          <button class="button button--outline" type="button" :disabled="busy || !roomName.trim() || !nickname.trim() || password.length < 8 || !joinAddress.trim() || joinControlPort < 1 || joinControlPort > 65535 || gamePort < 1 || gamePort > 65535" @click="joinRoom"><Icon name="plug" :size="15" />{{ busy ? 'Подключаюсь…' : 'Подключиться' }}</button>
        </article>
      </div>
    </template>

    <div v-else class="lan-room-active">
      <div class="lan-room-active-heading"><div><span class="tag tag--success">{{ isHost ? 'Вы хост' : 'Подключено' }}</span><h4>{{ status.room_name }}</h4><p>{{ status.player_count }} {{ status.player_count === 1 ? 'участник' : 'участников' }} · {{ statusBusy ? 'обновляю список…' : 'состав комнаты обновляется автоматически' }}</p></div><div class="workspace-card-actions"><button class="button button--small button--quiet" type="button" :disabled="statusBusy" @click="refreshStatus"><Icon name="refresh" :size="14" />Обновить</button><button v-if="isHost" class="button button--small button--outline" type="button" @click="copyRoomInfo"><Icon name="copy" :size="14" />Скопировать адрес</button><button class="button button--small button--danger" type="button" :disabled="busy" @click="leaveRoom"><Icon name="close" :size="14" />{{ busy ? 'Выхожу…' : 'Выйти' }}</button></div></div>
      <p v-if="error" class="inline-error lan-room-error">{{ error }}</p>
      <div class="lan-room-address"><span><small>Адрес Minecraft</small><strong>{{ status.host_reachable_minecraft_address }}</strong></span><span><small>Адрес комнаты</small><strong>{{ status.host_address }}:{{ status.host_control_port }}</strong></span></div>
      <div v-if="(status.alternative_hosts || []).length" class="lan-room-alt-hosts">
        <small>Если основной адрес не отвечает, попробуйте другие IPv4 этого компьютера:</small>
        <code v-for="host in status.alternative_hosts" :key="host" v-tooltip="`Minecraft: ${host}:${status.minecraft_game_port}`">{{ host }}:{{ status.minecraft_game_port }}</code>
      </div>
      <div class="lan-room-members"><strong>Участники</strong><span v-for="member in status.members" :key="`${member.nickname}-${member.role}`"><Icon name="accounts" :size="14" />{{ member.nickname }}<small>{{ member.role === 'host' ? 'хост' : 'игрок' }}</small></span></div>
      <p class="lan-room-note">{{ status.game_traffic_tunnel ? 'Это незашифрованный локальный TCP-прокси только для игрового порта, а не виртуальная LAN: требуется прямой доступ к адресу хоста; разные сети и интернет через него не соединяются. Для входа используйте адрес выше. Sandbox-профилю нужен доступ network:game.' : 'Комната слушает локальный адрес хоста. Участникам нужна прямая доступность к нему; после подключения у них появится локальный адрес TCP-прокси для Minecraft.' }}</p>
    </div>
  </section>
</template>
