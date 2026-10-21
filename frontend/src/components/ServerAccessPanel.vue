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

const opLevelOptions: SelectOption[] = [
  { value: '1', label: '1', meta: 'обход спавн-защиты' },
  { value: '2', label: '2', meta: 'команды чита' },
  { value: '3', label: '3', meta: 'ban/kick и т.п.' },
  { value: '4', label: '4', meta: 'полный доступ' },
]

// ключи, которым даём понятные подписи; остальное показываем как есть.
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

function parseJsonArray(content: string): unknown[] {
  try {
    const parsed = JSON.parse(content || '[]')
    return Array.isArray(parsed) ? parsed : []
  } catch {
    return []
  }
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

async function readOptional(path: string): Promise<string | null> {
  try {
    return (await bridge.serverFileRead(props.name, path)).content
  } catch {
    return null
  }
}

async function load() {
  loading.value = true
  loadError.value = ''
  try {
    const [whitelistRaw, opsRaw, propertiesRaw] = await Promise.all([
      readOptional('whitelist.json'),
      readOptional('ops.json'),
      readOptional('server.properties'),
    ])
    whitelist.value = parseJsonArray(whitelistRaw || '[]')
      .map((entry) => ({ name: String((entry as Record<string, unknown>).name || '').trim() }))
      .filter((entry) => entry.name)
    ops.value = parseJsonArray(opsRaw || '[]')
      .map((entry) => {
        const record = entry as Record<string, unknown>
        const level = Number(record.level)
        return { name: String(record.name || '').trim(), level: Number.isFinite(level) ? Math.min(4, Math.max(1, Math.round(level))) : 4 }
      })
      .filter((entry) => entry.name)
    properties.value = propertiesRaw === null ? [] : parseProperties(propertiesRaw)
    if (propertiesRaw === null && section.value === 'properties') loadError.value = 'server.properties ещё не создан: запустите сервер один раз, и файл появится.'
  } catch (error) {
    loadError.value = errorMessage(error)
  } finally {
    loading.value = false
  }
}

onMounted(load)
watch(() => props.name, load)

async function sendCommand(command: string): Promise<boolean> {
  try {
    await bridge.serverCommand(props.name, command)
    return true
  } catch (error) {
    emit('toast', errorMessage(error))
    return false
  }
}

async function addWhitelist() {
  const name = newWhitelistName.value.trim()
  if (!name) return
  busy.value = true
  try {
    if (props.running) {
      if (!(await sendCommand(`whitelist add ${name}`))) return
      whitelist.value = [...whitelist.value, { name }]
      if (!(await sendCommand('whitelist reload'))) return
    } else {
      if (whitelist.value.some((entry) => entry.name.toLowerCase() === name.toLowerCase())) {
        emit('toast', 'Этот игрок уже в списке')
        return
      }
      whitelist.value = [...whitelist.value, { name }]
      await bridge.serverFileWrite(props.name, 'whitelist.json', `${JSON.stringify(whitelist.value.map((entry) => ({ name: entry.name })), null, 2)}\n`)
    }
    newWhitelistName.value = ''
    emit('toast', props.running ? `Игрок ${name} добавлен в белый список` : `Белый список сохранён (${whitelist.value.length})`)
  } finally {
    busy.value = false
  }
}

async function removeWhitelist(entry: WhitelistEntry) {
  busy.value = true
  try {
    if (props.running) {
      if (!(await sendCommand(`whitelist remove ${entry.name}`))) return
      if (!(await sendCommand('whitelist reload'))) return
    }
    whitelist.value = whitelist.value.filter((item) => item.name !== entry.name)
    if (!props.running) {
      await bridge.serverFileWrite(props.name, 'whitelist.json', `${JSON.stringify(whitelist.value.map((item) => ({ name: item.name })), null, 2)}\n`)
    }
    emit('toast', `Игрок ${entry.name} убран из белого списка`)
  } finally {
    busy.value = false
  }
}

async function toggleWhitelistEnabled() {
  const row = properties.value.find((item) => item.key === 'white-list')
  busy.value = true
  try {
    const next = row?.value === 'true' ? 'false' : 'true'
    if (props.running) {
      if (!(await sendCommand(`whitelist ${next === 'true' ? 'on' : 'off'}`))) return
    } else if (row) {
      row.value = next
      row.edited = true
      await saveProperties()
      return
    }
    if (row) {
      row.value = next
      row.edited = false
    }
    emit('toast', next === 'true' ? 'Белый список включён' : 'Белый список выключен')
  } finally {
    busy.value = false
  }
}

const whitelistEnabled = computed(() => properties.value.find((item) => item.key === 'white-list')?.value === 'true')

async function addOp() {
  const name = newOpName.value.trim()
  if (!name) return
  busy.value = true
  try {
    if (props.running) {
      if (!(await sendCommand(`op ${name}`))) return
      ops.value = [...ops.value.filter((entry) => entry.name.toLowerCase() !== name.toLowerCase()), { name, level: newOpLevel.value }]
    } else {
      ops.value = [...ops.value.filter((entry) => entry.name.toLowerCase() !== name.toLowerCase()), { name, level: newOpLevel.value }]
      await bridge.serverFileWrite(props.name, 'ops.json', `${JSON.stringify(ops.value.map((entry) => ({ name: entry.name, level: entry.level })), null, 2)}\n`)
    }
    newOpName.value = ''
    emit('toast', `Права оператора выданы: ${name}`)
  } finally {
    busy.value = false
  }
}

async function changeOpLevel(entry: OpEntry, level: number) {
  busy.value = true
  try {
    entry.level = level
    if (props.running) {
      const command = level >= 1 ? `op ${entry.name}` : `deop ${entry.name}`
      if (!(await sendCommand(command))) return
    } else {
      await bridge.serverFileWrite(props.name, 'ops.json', `${JSON.stringify(ops.value.map((item) => ({ name: item.name, level: item.level })), null, 2)}\n`)
    }
    emit('toast', `Уровень ${entry.name}: ${level}`)
  } finally {
    busy.value = false
  }
}

async function removeOp(entry: OpEntry) {
  busy.value = true
  try {
    if (props.running) {
      if (!(await sendCommand(`deop ${entry.name}`))) return
    }
    ops.value = ops.value.filter((item) => item.name !== entry.name)
    if (!props.running) {
      await bridge.serverFileWrite(props.name, 'ops.json', `${JSON.stringify(ops.value.map((item) => ({ name: item.name, level: item.level })), null, 2)}\n`)
    }
    emit('toast', `Права оператора сняты: ${entry.name}`)
  } finally {
    busy.value = false
  }
}

function setProperty(row: PropertyRow, value: string) {
  row.value = value
  row.edited = true
}

async function saveProperties() {
  busy.value = true
  try {
    await bridge.serverFileWrite(props.name, 'server.properties', serializeProperties(properties.value))
    properties.value.forEach((row) => { row.edited = false })
    emit('toast', props.running ? 'server.properties сохранён; часть ключей применяется после рестарта' : 'server.properties сохранён')
  } catch (error) {
    emit('toast', errorMessage(error))
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
        <div class="server-access-toolbar">
          <button class="button button--small button--accent" type="button" :disabled="busy || loading || !properties.length || !propertiesDirty" @click="saveProperties">{{ propertiesDirty ? 'Сохранить server.properties' : 'Изменений нет' }}</button>
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
      </div>
    </template>
  </div>
</template>
