<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import CustomSelect from './CustomSelect.vue'
import Icon from './Icon.vue'
import Collapsible from './Collapsible.vue'
import { bridge } from '../services/bridge'
import type { SelectOption } from '../types'

const props = defineProps<{
  name: string
  running: boolean
}>()

const emit = defineEmits<{ toast: [message: string] }>()

type WhitelistEntry = { name: string }
type OpEntry = { name: string; level: number }
type PropertyRow = { key: string; value: string; edited: boolean }
type Section = 'whitelist' | 'ops' | 'properties'

const section = ref<Section>('whitelist')
const loading = ref(false)
const busy = ref(false)
const whitelist = ref<WhitelistEntry[]>([])
const ops = ref<OpEntry[]>([])
const properties = ref<PropertyRow[]>([])
const newWhitelistName = ref('')
const newOpName = ref('')
const newOpLevel = ref(4)
const loadError = ref('')
const propertiesMissing = ref(false)
let loadGeneration = 0

const opLevelOptions: SelectOption[] = [
  { value: '1', label: '1', meta: 'обход спавн-защиты' },
  { value: '2', label: '2', meta: 'команды чита' },
  { value: '3', label: '3', meta: 'ban/kick и т.п.' },
  { value: '4', label: '4', meta: 'полный доступ' },
]

// ключи, которым даём понятные подписи; остальное показываем как есть
const propertyLabels: Record<string, string> = {
  'motd': 'MOTD',
  'max-players': 'Максимум игроков',
  'online-mode': 'Проверка лицензии (online-mode)',
  'white-list': 'Белый список',
  'enforce-whitelist': 'Принудительный белый список',
  'difficulty': 'Сложность',
  'gamemode': 'Режим игры',
  'pvp': 'PvP',
  'allow-flight': 'Разрешить полёт',
  'allow-nether': 'Разрешить Нижний мир',
  'view-distance': 'Дальность видимости',
  'simulation-distance': 'Дальность симуляции',
  'spawn-protection': 'Защита спавна, блоков',
  'enable-command-block': 'Командные блоки',
  'level-name': 'Имя мира',
  'level-seed': 'Сид мира',
  'level-type': 'Тип мира',
  'generate-structures': 'Генерировать структуры',
  'hardcore': 'Хардкор',
  'allow-cheats': 'Разрешить читы (Bedrock)',
  'player-idle-timeout': 'Кик простаивающих, минут',
  'enforce-secure-profile': 'Требовать безопасный профиль',
  'prevent-proxy-connections': 'Блокировать прокси-подключения',
  'sync-chunk-writes': 'Синхронная запись чанков',
  'network-compression-threshold': 'Порог сжатия сети',
}

const structuredKeys = computed(() => properties.value.filter((row) => propertyLabels[row.key]))
const otherKeys = computed(() => properties.value.filter((row) => !propertyLabels[row.key]))

const booleanOptions: SelectOption[] = [
  { value: 'true', label: 'Вкл' },
  { value: 'false', label: 'Выкл' },
]
const difficultyOptions: SelectOption[] = ['peaceful', 'easy', 'normal', 'hard'].map((value) => ({ value, label: value }))
const gamemodeOptions: SelectOption[] = ['survival', 'creative', 'adventure', 'spectator'].map((value) => ({ value, label: value }))
const levelTypeOptions: SelectOption[] = ['minecraft:normal', 'minecraft:flat', 'minecraft:large_biomes', 'minecraft:amplified', 'minecraft:single_biome_surface'].map((value) => ({ value, label: value.replace('minecraft:', '') }))

function propertyOptions(key: string): SelectOption[] | null {
  if (key === 'difficulty') return difficultyOptions
  if (key === 'gamemode') return gamemodeOptions
  if (key === 'level-type') return levelTypeOptions
  if (key === 'online-mode' || key === 'white-list' || key === 'enforce-whitelist' || key === 'pvp' || key === 'allow-flight' || key === 'allow-nether' || key === 'enable-command-block' || key === 'generate-structures' || key === 'hardcore' || key === 'allow-cheats' || key === 'enforce-secure-profile' || key === 'prevent-proxy-connections' || key === 'sync-chunk-writes') return booleanOptions
  return null
}

function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message
  return typeof error === 'string' ? error : 'Неизвестная ошибка'
}

function playerName(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const name = value.trim()
  return /^[A-Za-z0-9_]{3,16}$/.test(name) ? name : null
}

function parseJsonArray(content: string, path: string): unknown[] {
  let parsed: unknown
  try {
    parsed = JSON.parse(content)
  } catch {
    throw new Error(`${path}: некорректный JSON`)
  }
  if (!Array.isArray(parsed)) throw new Error(`${path}: ожидался JSON-массив`)
  return parsed
}

function parseWhitelist(content: string): WhitelistEntry[] {
  return parseJsonArray(content, 'whitelist.json').map((entry, index) => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) throw new Error(`whitelist.json: некорректная запись ${index + 1}`)
    const name = playerName((entry as Record<string, unknown>).name)
    if (!name) throw new Error(`whitelist.json: некорректное имя игрока в записи ${index + 1}`)
    return { name }
  })
}

function parseOps(content: string): OpEntry[] {
  return parseJsonArray(content, 'ops.json').map((entry, index) => {
    if (!entry || typeof entry !== 'object' || Array.isArray(entry)) throw new Error(`ops.json: некорректная запись ${index + 1}`)
    const record = entry as Record<string, unknown>
    const name = playerName(record.name)
    if (!name) throw new Error(`ops.json: некорректное имя игрока в записи ${index + 1}`)
    if (!Number.isInteger(record.level) || Number(record.level) < 1 || Number(record.level) > 4) throw new Error(`ops.json: некорректный уровень прав в записи ${index + 1}`)
    return { name, level: Number(record.level) }
  })
}

function parseProperties(content: string): PropertyRow[] {
  return content.split(/\r?\n/).filter((line) => line.trim() && !line.trimStart().startsWith('#')).map((line) => {
    const index = line.indexOf('=')
    const key = index === -1 ? line.trim() : line.slice(0, index).trim()
    const value = index === -1 ? '' : line.slice(index + 1)
    return { key, value, edited: false }
  })
}

function serializeProperties(rows: PropertyRow[]): string {
  return `${rows.map((row) => `${row.key}=${row.value}`).join('\n')}\n`
}

async function readOptional(serverName: string, path: string): Promise<string | null> {
  try {
    return (await bridge.serverFileRead(serverName, path)).content
  } catch (error) {
    const message = errorMessage(error)
    if (message !== 'Файл недоступен' && message !== 'Текстовый файл не найден или недоступен') throw error
    const { files } = await bridge.serverFileList(serverName)
    if (!files.some((file) => file.kind === 'file' && file.path === path)) return null
    throw error
  }
}

async function load() {
  const generation = ++loadGeneration
  const serverName = props.name
  loading.value = true
  loadError.value = ''
  propertiesMissing.value = false
  whitelist.value = []
  ops.value = []
  properties.value = []
  try {
    const [whitelistRaw, opsRaw, propertiesRaw] = await Promise.all([
      readOptional(serverName, 'whitelist.json'),
      readOptional(serverName, 'ops.json'),
      readOptional(serverName, 'server.properties'),
    ])
    const loadedWhitelist = parseWhitelist(whitelistRaw ?? '[]')
    const loadedOps = parseOps(opsRaw ?? '[]')
    const loadedProperties = propertiesRaw === null ? [] : parseProperties(propertiesRaw)
    if (generation !== loadGeneration || serverName !== props.name) return
    whitelist.value = loadedWhitelist
    ops.value = loadedOps
    properties.value = loadedProperties
    propertiesMissing.value = propertiesRaw === null
  } catch (error) {
    if (generation === loadGeneration && serverName === props.name) loadError.value = errorMessage(error)
  } finally {
    if (generation === loadGeneration && serverName === props.name) loading.value = false
  }
}

onMounted(load)
watch(() => props.name, load)

async function sendCommand(serverName: string, command: string): Promise<boolean> {
  try {
    await bridge.serverCommand(serverName, command)
    return true
  } catch (error) {
    if (props.name === serverName) emit('toast', errorMessage(error))
    return false
  }
}

async function addWhitelist() {
  const name = playerName(newWhitelistName.value)
  if (!name) {
    emit('toast', 'Введите корректный ник игрока (3–16 латинских букв, цифр или _)')
    return
  }
  const serverName = props.name
  const running = props.running
  busy.value = true
  try {
    if (running) {
      if (!(await sendCommand(serverName, `whitelist add ${name}`))) return
      if (props.name === serverName) whitelist.value = [...whitelist.value, { name }]
      if (!(await sendCommand(serverName, 'whitelist reload'))) return
    } else {
      if (whitelist.value.some((entry) => entry.name.toLowerCase() === name.toLowerCase())) {
        emit('toast', 'Этот игрок уже в списке')
        return
      }
      const nextWhitelist = [...whitelist.value, { name }]
      await bridge.serverFileWrite(serverName, 'whitelist.json', `${JSON.stringify(nextWhitelist.map((entry) => ({ name: entry.name })), null, 2)}\n`)
      if (props.name === serverName) whitelist.value = nextWhitelist
    }
    if (props.name === serverName) {
      newWhitelistName.value = ''
      emit('toast', running ? `Игрок ${name} добавлен в белый список` : `Белый список сохранён (${whitelist.value.length})`)
    }
  } finally {
    busy.value = false
  }
}

async function removeWhitelist(entry: WhitelistEntry) {
  const serverName = props.name
  const running = props.running
  if (!playerName(entry.name)) {
    emit('toast', 'Нельзя удалить игрока с некорректным ником')
    return
  }
  busy.value = true
  try {
    if (running) {
      if (!(await sendCommand(serverName, `whitelist remove ${entry.name}`))) return
      if (!(await sendCommand(serverName, 'whitelist reload'))) return
      if (props.name === serverName) whitelist.value = whitelist.value.filter((item) => item.name !== entry.name)
    } else {
      const nextWhitelist = whitelist.value.filter((item) => item.name !== entry.name)
      await bridge.serverFileWrite(serverName, 'whitelist.json', `${JSON.stringify(nextWhitelist.map((item) => ({ name: item.name })), null, 2)}\n`)
      if (props.name === serverName) whitelist.value = nextWhitelist
    }
    if (props.name === serverName) emit('toast', `Игрок ${entry.name} убран из белого списка`)
  } finally {
    busy.value = false
  }
}

async function toggleWhitelistEnabled() {
  const serverName = props.name
  const running = props.running
  const row = properties.value.find((item) => item.key === 'white-list')
  busy.value = true
  try {
    const next = row?.value === 'true' ? 'false' : 'true'
    if (running) {
      if (!(await sendCommand(serverName, `whitelist ${next === 'true' ? 'on' : 'off'}`))) return
    } else if (row) {
      if (props.name !== serverName) return
      row.value = next
      row.edited = true
      await saveProperties(serverName, running)
      return
    }
    if (props.name === serverName) {
      if (row) {
        row.value = next
        row.edited = false
      }
      emit('toast', next === 'true' ? 'Белый список включён' : 'Белый список выключен')
    }
  } finally {
    busy.value = false
  }
}

const whitelistEnabled = computed(() => properties.value.find((item) => item.key === 'white-list')?.value === 'true')

async function addOp() {
  const name = playerName(newOpName.value)
  if (!name) {
    emit('toast', 'Введите корректный ник игрока (3–16 латинских букв, цифр или _)')
    return
  }
  const serverName = props.name
  const running = props.running
  busy.value = true
  try {
    const nextOps = [...ops.value.filter((entry) => entry.name.toLowerCase() !== name.toLowerCase()), { name, level: newOpLevel.value }]
    if (running) {
      if (!(await sendCommand(serverName, `op ${name}`))) return
    } else {
      await bridge.serverFileWrite(serverName, 'ops.json', `${JSON.stringify(nextOps.map((entry) => ({ name: entry.name, level: entry.level })), null, 2)}\n`)
    }
    if (props.name === serverName) {
      ops.value = nextOps
      newOpName.value = ''
      emit('toast', `Права оператора выданы: ${name}`)
    }
  } finally {
    busy.value = false
  }
}

async function changeOpLevel(entry: OpEntry, level: number) {
  const serverName = props.name
  const running = props.running
  if (!playerName(entry.name)) {
    emit('toast', 'Нельзя изменить права игрока с некорректным ником')
    return
  }
  busy.value = true
  try {
    if (running) {
      if (!(await sendCommand(serverName, `op ${entry.name}`))) return
      if (props.name === serverName) entry.level = level
    } else {
      const nextOps = ops.value.map((item) => item.name === entry.name ? { ...item, level } : item)
      await bridge.serverFileWrite(serverName, 'ops.json', `${JSON.stringify(nextOps.map((item) => ({ name: item.name, level: item.level })), null, 2)}\n`)
      if (props.name === serverName) ops.value = nextOps
    }
    if (props.name === serverName) emit('toast', `Уровень ${entry.name}: ${level}`)
  } finally {
    busy.value = false
  }
}

async function removeOp(entry: OpEntry) {
  const serverName = props.name
  const running = props.running
  if (!playerName(entry.name)) {
    emit('toast', 'Нельзя снять права у игрока с некорректным ником')
    return
  }
  busy.value = true
  try {
    if (running) {
      if (!(await sendCommand(serverName, `deop ${entry.name}`))) return
      if (props.name === serverName) ops.value = ops.value.filter((item) => item.name !== entry.name)
    } else {
      const nextOps = ops.value.filter((item) => item.name !== entry.name)
      await bridge.serverFileWrite(serverName, 'ops.json', `${JSON.stringify(nextOps.map((item) => ({ name: item.name, level: item.level })), null, 2)}\n`)
      if (props.name === serverName) ops.value = nextOps
    }
    if (props.name === serverName) emit('toast', `Права оператора сняты: ${entry.name}`)
  } finally {
    busy.value = false
  }
}

function setProperty(row: PropertyRow, value: string) {
  row.value = value
  row.edited = true
}

async function saveProperties(serverName = props.name, running = props.running) {
  const rows = properties.value
  const content = serializeProperties(rows)
  busy.value = true
  try {
    await bridge.serverFileWrite(serverName, 'server.properties', content)
    if (props.name === serverName && properties.value === rows) {
      rows.forEach((row) => { row.edited = false })
      emit('toast', running ? 'server.properties сохранён; часть ключей применяется после рестарта' : 'server.properties сохранён')
    }
  } catch (error) {
    if (props.name === serverName) emit('toast', errorMessage(error))
  } finally {
    busy.value = false
  }
}

const propertiesDirty = computed(() => properties.value.some((row) => row.edited))
const sections: Array<{ id: Section; label: string }> = [
  { id: 'whitelist', label: 'Белый список' },
  { id: 'ops', label: 'Операторы' },
  { id: 'properties', label: 'server.properties' },
]
</script>

<template>
  <div class="server-access-panel">
    <div class="server-access-sections" role="tablist" aria-label="Управление доступом">
      <button v-for="item in sections" :key="item.id" class="server-access-tab" :class="{ 'server-access-tab--active': section === item.id }" type="button" role="tab" :aria-selected="section === item.id" @click="section = item.id">{{ item.label }}</button>
    </div>

    <p v-if="loading" class="muted">Читаю файлы сервера…</p>
    <p v-else-if="loadError" class="muted">{{ loadError }}</p>

    <template v-else>
      <div v-if="section === 'whitelist'" class="server-access-body">
        <div class="server-access-toolbar">
          <label class="field"><span>Ник игрока</span><input v-model="newWhitelistName" placeholder="Steve" @keyup.enter="addWhitelist" /></label>
          <button class="button button--small button--accent" type="button" :disabled="busy || loading || !newWhitelistName.trim()" @click="addWhitelist">Добавить</button>
          <button class="button button--small button--quiet" type="button" :disabled="busy || running" @click="toggleWhitelistEnabled">
            {{ whitelistEnabled ? 'Выключить белый список' : 'Включить белый список' }}
          </button>
        </div>
        <small class="field-help">{{ running ? 'Сервер работает: изменения применяются командами whitelist add/remove сразу.' : 'Сервер остановлен: имена пишутся в whitelist.json без UUID, сервер дополнит их при загрузке.' }}</small>
        <div class="server-access-list">
          <span v-for="entry in whitelist" :key="entry.name" class="server-access-chip">
            {{ entry.name }}
            <button class="text-button" type="button" :disabled="busy" @click="removeWhitelist(entry)">убрать</button>
          </span>
          <span v-if="!whitelist.length" class="muted">Список пуст — сервер примет всех, пока whitelist не включён.</span>
        </div>
      </div>

      <div v-else-if="section === 'ops'" class="server-access-body">
        <div class="server-access-toolbar">
          <label class="field"><span>Ник игрока</span><input v-model="newOpName" placeholder="Alex" @keyup.enter="addOp" /></label>
          <div class="field"><span>Уровень</span><CustomSelect :model-value="String(newOpLevel)" :options="opLevelOptions" @update:model-value="newOpLevel = Number($event)" /></div>
          <button class="button button--small button--accent" type="button" :disabled="busy || loading || !newOpName.trim()" @click="addOp">Выдать права</button>
        </div>
        <small class="field-help">{{ running ? 'Сервер работает: права выдаются командой op.' : 'Сервер остановлен: ops.json обновляется напрямую.' }}</small>
        <div class="server-access-list server-access-list--rows">
          <div v-for="entry in ops" :key="entry.name" class="server-access-row">
            <strong>{{ entry.name }}</strong>
            <CustomSelect :model-value="String(entry.level)" :options="opLevelOptions" :disabled="busy" @update:model-value="changeOpLevel(entry, Number($event))" />
            <button class="icon-button" type="button" aria-label="Снять права" :disabled="busy" @click="removeOp(entry)"><Icon name="trash" :size="14" /></button>
          </div>
          <span v-if="!ops.length" class="muted">Операторов нет.</span>
        </div>
      </div>

      <div v-else class="server-access-body">
        <p v-if="propertiesMissing" class="muted">server.properties ещё не создан: запустите сервер один раз, и файл появится.</p>
        <template v-else>
          <div class="server-access-toolbar">
            <button class="button button--small button--accent" type="button" :disabled="busy || loading || !properties.length || !propertiesDirty" @click="() => saveProperties()">{{ propertiesDirty ? 'Сохранить server.properties' : 'Изменений нет' }}</button>
            <small class="field-help">Порядок ключей и комментарии сохраняются, порт управляется из настроек сервера.</small>
          </div>
          <div v-if="properties.length" class="workspace-form-grid">
            <div v-for="row in structuredKeys" :key="row.key" class="field">
              <span>{{ propertyLabels[row.key] }}<i v-if="row.edited" class="server-access-edited">•</i></span>
              <CustomSelect v-if="propertyOptions(row.key)" :model-value="row.value" :options="propertyOptions(row.key)!" :disabled="busy" @update:model-value="setProperty(row, String($event))" />
              <input v-else :value="row.value" :disabled="busy" @input="setProperty(row, ($event.target as HTMLInputElement).value)" />
            </div>
          </div>
          <Collapsible v-if="otherKeys.length" class="server-access-other" :title="`Остальные ключи (${otherKeys.length})`">
            <div class="workspace-form-grid">
              <div v-for="row in otherKeys" :key="row.key" class="field">
                <span><code>{{ row.key }}</code><i v-if="row.edited" class="server-access-edited">•</i></span>
                <input :value="row.value" :disabled="busy" @input="setProperty(row, ($event.target as HTMLInputElement).value)" />
              </div>
            </div>
          </Collapsible>
        </template>
      </div>
    </template>
  </div>
</template>
