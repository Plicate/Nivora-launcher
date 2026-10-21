<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { openContextMenu, type ContextMenuItem } from '../services/contextMenu'
import CoreBadge from './CoreBadge.vue'
import CustomSelect from './CustomSelect.vue'
import Icon from './Icon.vue'
import LoaderBadge from './LoaderBadge.vue'
import type { InstanceInfo, SelectOption } from '../types'

const props = withDefaults(defineProps<{
  instances: InstanceInfo[]
  groups: string[]
  pinnedGroups?: string[]
  busy?: boolean
  groupBusy?: boolean
  activeFilter: string
  searchShortcut?: string
  layout?: string
}>(), { pinnedGroups: () => [], busy: false, groupBusy: false, searchShortcut: 'Ctrl+K', layout: 'default' })

const emit = defineEmits<{
  'update:activeFilter': [filter: string]
  'update:layout': [layout: string]
  'new-instance': []
  'create-group': []
  'rename-group': [group: string]
  'delete-group': [group: string]
  'toggle-pin': [group: string]
  'move-group': [instance: InstanceInfo, group: string]
  play: [instance: InstanceInfo]
  stop: [instance: InstanceInfo]
  settings: [instance: InstanceInfo]
  duplicate: [instance: InstanceInfo]
  'delete-instance': [instance: InstanceInfo]
}>()

const searchQuery = ref('')
const activeTag = ref('')
const tagNames = computed(() => Array.from(new Set(props.instances.flatMap((instance) => instance.tags || []))).sort((a, b) => a.localeCompare(b)))
function toggleTag(tag: string) {
  activeTag.value = activeTag.value === tag ? '' : tag
}
const groupNames = computed(() => Array.from(new Set([
  ...props.groups.filter((group) => group.trim()),
  ...props.instances.map((instance) => instance.group).filter((group) => Boolean(group?.trim())),
])))
const pinnedGroupNames = computed(() => props.pinnedGroups.filter((group) => groupNames.value.includes(group)).slice(0, 3))
const orderedGroupNames = computed(() => [
  ...pinnedGroupNames.value,
  ...groupNames.value.filter((group) => !pinnedGroupNames.value.includes(group)),
])
const isPinned = (group: string) => pinnedGroupNames.value.includes(group)
const groupOptions = computed<SelectOption[]>(() => [
  { value: '', label: 'Без группы', meta: 'Не входит в подборку' },
  ...groupNames.value.map((group) => ({ value: group, label: group })),
])
const filterOptions = computed<SelectOption[]>(() => [
  { value: 'all', label: 'Все экземпляры', meta: `${props.instances.length}` },
  { value: 'ungrouped', label: 'Без группы', meta: `${props.instances.filter((instance) => !instance.group?.trim()).length}` },
  ...orderedGroupNames.value.map((group) => ({
    value: `group:${group}`,
    label: group,
    meta: `${props.instances.filter((instance) => instance.group === group).length}`,
  })),
])
const filteredInstances = computed(() => {
  const query = searchQuery.value.trim().toLocaleLowerCase()
  return props.instances.filter((instance) => {
    if (activeTag.value && !(instance.tags || []).includes(activeTag.value)) return false
    if (!query) return true
    return [
      instance.display_name,
      instance.name,
      instance.description,
      instance.group,
      instance.game_edition,
      instance.kind === 'server' ? instance.server_core : instance.loader,
      instance.server_core_version,
      instance.minecraft_version,
      instance.version_id,
      ...(instance.tags || []),
    ].some((value) => String(value || '').toLocaleLowerCase().includes(query))
  })
})
const visibleSections = computed(() => {
  const sectionFor = (group: string) => ({
    key: group ? `group:${group}` : 'ungrouped',
    name: group || 'Без группы',
    group,
    instances: filteredInstances.value.filter((instance) => (instance.group || '') === group),
    totalCount: props.instances.filter((instance) => (instance.group || '') === group).length,
  })

  if (props.activeFilter === 'all') {
    const searchActive = Boolean(searchQuery.value.trim())
    const sections = orderedGroupNames.value.map(sectionFor).filter((section) => section.instances.length || !searchActive)
    const ungrouped = sectionFor('')
    if (ungrouped.instances.length || (!sections.length && !searchActive)) sections.push(ungrouped)
    return sections
  }
  if (props.activeFilter === 'ungrouped') return [sectionFor('')]
  const group = props.activeFilter.slice('group:'.length)
  return groupNames.value.includes(group) ? [sectionFor(group)] : []
})
const showFirstRunEmpty = computed(() => props.activeFilter === 'all' && !props.instances.length)
const showSearchEmpty = computed(() => props.instances.length > 0 && !filteredInstances.value.length)

watch(groupNames, (groups) => {
  if (props.activeFilter.startsWith('group:') && !groups.includes(props.activeFilter.slice('group:'.length))) {
    emit('update:activeFilter', 'all')
  }
})

function changeGroup(instance: InstanceInfo, group: string) {
  if ((instance.group || '') !== group) emit('move-group', instance, group)
}

const compact = computed(() => props.layout === 'compact')
// плитка компактного режима, у которой раскрыта панель «Дополнительно».
const expandedInstance = ref('')

function toggleMore(instance: InstanceInfo) {
  expandedInstance.value = expandedInstance.value === instance.name ? '' : instance.name
}

function instanceMeta(instance: InstanceInfo): string {
  if (instance.game_edition === 'bedrock') return 'Bedrock'
  const core = instance.kind === 'server' ? (instance.server_core || 'Сервер') : (instance.loader || 'Vanilla')
  const version = instance.kind === 'server'
    ? (instance.server_core_version || instance.minecraft_version || instance.version_id || '')
    : (instance.minecraft_version || instance.version_id || '')
  return version ? `${core} · ${version}` : core
}

/** Собственное меню правого клика по экземпляру — как у десктопных приложений. */
function instanceMenu(event: MouseEvent, instance: InstanceInfo) {
  const title = instance.display_name || instance.name
  const items: ContextMenuItem[] = [
    {
      id: 'play',
      label: instance.running ? 'Остановить' : instance.kind === 'server' ? 'Запустить сервер' : 'Играть',
      icon: instance.running ? 'stop' : 'play',
      action: () => toggleInstance(instance),
    },
    {
      id: 'settings',
      label: 'Рабочая область',
      icon: 'settings',
      action: () => emit('settings', instance),
    },
  ]
  if (instance.game_edition !== 'bedrock') {
    items.push({ id: 'duplicate', label: 'Дублировать', icon: 'copy', action: () => emit('duplicate', instance) })
  }
  items.push({ id: 'delete', label: 'Удалить', icon: 'trash', danger: true, action: () => emit('delete-instance', instance) })
  openContextMenu(event, items, title)
}

function toggleInstance(instance: InstanceInfo) {
  if (instance.running) emit('stop', instance)
  else emit('play', instance)
}

function emptyMessage(group: string): string {
  if (group) return 'Назначьте подборку в строке нужного экземпляра.'
  if (props.activeFilter !== 'all') return 'Выберите «Без группы» у нужного экземпляра.'
  return 'Здесь будут профили, которым не назначена подборка.'
}

function formatPlayedTime(value: number): string {
  const seconds = Math.max(0, Math.round(value || 0))
  const hours = Math.floor(seconds / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  if (hours) return `${hours} ч ${minutes} мин`
  return `${minutes} мин`
}
</script>

<template>
  <section class="instance-groups" aria-label="Группы экземпляров">
    <div class="instance-groups-toolbar">
      <label class="instance-search">
        <Icon name="search" :size="16" />
        <input v-model="searchQuery" type="search" placeholder="Найти экземпляр" aria-label="Найти экземпляр" />
        <kbd>{{ searchShortcut }}</kbd>
      </label>
      <div class="instance-filter-fallback">
        <CustomSelect
          :model-value="activeFilter"
          :options="filterOptions"
          aria-label="Фильтр по подборкам"
          @update:model-value="emit('update:activeFilter', $event)"
        />
      </div>
      <span class="instance-result-count">{{ filteredInstances.length }} из {{ instances.length }}</span>
      <div class="segmented-control instance-layout-switch" role="group" aria-label="Вид списка экземпляров">
        <button type="button" :class="{ active: !compact }" :aria-pressed="!compact" aria-label="Обычный список" v-tooltip="'Обычный список'" @click="emit('update:layout', 'default')"><Icon name="list" :size="15" /></button>
        <button type="button" :class="{ active: compact }" :aria-pressed="compact" aria-label="Компактный режим" v-tooltip="'Компактный режим'" @click="emit('update:layout', 'compact')"><Icon name="grid" :size="15" /></button>
      </div>
      <button class="button button--outline instance-groups-create-fallback" type="button" :disabled="groupBusy" @click="emit('create-group')">
        <Icon name="plus" :size="15" />Новая подборка
      </button>
    </div>

    <div v-if="tagNames.length" class="instance-tag-bar" aria-label="Фильтр по тегам">
      <button
        v-for="tag in tagNames"
        :key="tag"
        class="instance-tag-chip"
        :class="{ 'instance-tag-chip--active': activeTag === tag }"
        type="button"
        :aria-pressed="activeTag === tag"
        @click="toggleTag(tag)"
      >#{{ tag }}<small>{{ instances.filter((instance) => (instance.tags || []).includes(tag)).length }}</small></button>
      <button v-if="activeTag" class="text-button" type="button" @click="activeTag = ''">Сбросить тег</button>
    </div>

    <div v-if="showFirstRunEmpty" class="empty-card">
      <span class="empty-orb empty-orb--large"><Icon name="instances" :size="24" /></span>
      <h3>Профилей пока нет</h3>
      <p>Создайте экземпляр Minecraft, чтобы выбрать версию и загрузчик.</p>
      <button class="button button--accent" type="button" @click="emit('new-instance')">
        <Icon name="plus" :size="17" />Создать экземпляр
      </button>
    </div>

    <div v-else-if="showSearchEmpty" class="instance-search-empty">
      <span>Нет совпадений по запросу «{{ searchQuery.trim() }}».</span>
      <button class="text-button" type="button" @click="searchQuery = ''; emit('update:activeFilter', 'all')">Сбросить поиск и фильтр</button>
    </div>

    <div v-else class="instance-group-sections">
      <section v-for="section in visibleSections" :key="section.key" class="instance-group-section">
        <header class="instance-group-heading">
          <div class="instance-group-heading-copy">
            <Icon class="instance-group-heading-icon" :name="section.group ? 'folder' : 'instances'" :size="15" />
            <h3>{{ section.name }}</h3>
            <span class="instance-group-count">{{ section.instances.length }}</span>
          </div>
          <div class="instance-group-heading-actions">
            <button
              v-if="section.group"
              class="icon-button"
              type="button"
              :disabled="groupBusy"
              :aria-label="`Переименовать группу «${section.group}»`"
              v-tooltip="'Переименовать группу (экземпляры остаются в ней)'"
              @click="emit('rename-group', section.group)"
            >
              <Icon name="edit" :size="15" />
            </button>
            <button
              v-if="section.group"
              class="icon-button instance-group-pin"
              :class="{ 'instance-group-pin--active': isPinned(section.group) }"
              type="button"
              :disabled="groupBusy || (!isPinned(section.group) && pinnedGroupNames.length >= 3)"
              :aria-label="isPinned(section.group) ? `Открепить группу «${section.group}»` : `Закрепить группу «${section.group}»`"
              :aria-pressed="isPinned(section.group)"
              v-tooltip="isPinned(section.group) ? 'Открепить группу' : pinnedGroupNames.length >= 3 ? 'Можно закрепить не более трёх групп' : 'Закрепить группу сверху'"
              @click="emit('toggle-pin', section.group)"
            >
              <Icon name="star" :size="15" />
            </button>
            <button
              v-if="section.group"
              class="icon-button icon-button--danger instance-group-delete"
              type="button"
              :disabled="groupBusy || section.totalCount > 0"
              :aria-label="`Удалить группу «${section.group}»`"
              v-tooltip="section.totalCount ? 'Сначала переместите все экземпляры из этой группы' : 'Удалить пустую группу'"
              @click="emit('delete-group', section.group)"
            >
              <Icon name="trash" :size="16" />
            </button>
          </div>
        </header>

        <div v-if="section.instances.length && compact" class="instance-tiles">
          <article
            v-for="instance in section.instances"
            :key="instance.name"
            class="instance-tile"
            :class="{ 'instance-tile--open': expandedInstance === instance.name, 'instance-tile--running': instance.running }"
            @contextmenu="instanceMenu($event, instance)"
          >
            <div class="instance-tile-actions">
              <button
                class="button button--small instance-tile-play"
                :class="instance.running ? 'button--outline' : 'button--accent'"
                type="button"
                :disabled="busy"
                :aria-label="`${instance.running ? 'Остановить' : (instance.kind === 'server' ? 'Запустить' : 'Играть')}: ${instance.display_name || instance.name}`"
                v-tooltip="instance.running ? 'Остановить' : (instance.kind === 'server' ? 'Запустить' : 'Играть')"
                @click="toggleInstance(instance)"
              ><Icon :name="instance.running ? 'stop' : 'play'" :size="15" /></button>
              <button class="icon-button" type="button" :aria-label="`Настройки экземпляра ${instance.display_name || instance.name}`" v-tooltip="'Настройки'" @click="emit('settings', instance)"><Icon name="settings" :size="15" /></button>
              <button
                class="icon-button"
                :class="{ 'icon-button--active': expandedInstance === instance.name }"
                type="button"
                :aria-expanded="expandedInstance === instance.name"
                :aria-label="`Дополнительно: ${instance.display_name || instance.name}`"
                v-tooltip="'Дополнительно'"
                @click="toggleMore(instance)"
              ><Icon name="more" :size="15" /></button>
            </div>
            <button class="instance-tile-body" type="button" @click="emit('settings', instance)">
              <span class="instance-tile-mark">
                <img v-if="instance.icon" :src="instance.icon" alt="" />
                <CoreBadge v-else-if="instance.kind === 'server'" :core="instance.server_core" compact />
                <span v-else-if="instance.game_edition === 'bedrock'" class="instance-edition-badge">B</span>
                <LoaderBadge v-else :loader="instance.loader" size="md" />
              </span>
              <strong class="instance-tile-name">{{ instance.display_name || instance.name }}</strong>
              <small class="instance-tile-meta"><i v-if="instance.running" class="instance-tile-dot" />{{ instanceMeta(instance) }}</small>
            </button>
            <div v-if="expandedInstance === instance.name" class="instance-tile-more">
              <div class="instance-tile-more-row">
                <span>Подборка</span>
                <CustomSelect
                  :model-value="instance.group || ''"
                  :options="groupOptions"
                  :disabled="busy || groupBusy"
                  :aria-label="`Подборка экземпляра ${instance.display_name || instance.name}`"
                  @update:model-value="changeGroup(instance, $event)"
                />
              </div>
              <p class="instance-tile-more-info">Наиграно: {{ formatPlayedTime(instance.played_seconds) }}<template v-if="instance.kind === 'server'"> · порт {{ instance.server_port || 25565 }}</template></p>
              <div class="instance-tile-more-actions">
                <button v-if="instance.game_edition !== 'bedrock'" class="button button--small button--quiet" type="button" @click="emit('duplicate', instance)"><Icon name="copy" :size="14" />Дублировать</button>
                <button class="button button--small button--quiet instance-tile-delete" type="button" @click="emit('delete-instance', instance)"><Icon name="trash" :size="14" />Удалить</button>
              </div>
            </div>
          </article>
        </div>
        <div v-else-if="section.instances.length" class="instance-list">
          <article v-for="instance in section.instances" :key="instance.name" class="instance-row" @contextmenu="instanceMenu($event, instance)">
            <div class="instance-row-primary">
              <span class="instance-row-mark">
                <img v-if="instance.icon" :src="instance.icon" alt="" />
                <CoreBadge v-else-if="instance.kind === 'server'" :core="instance.server_core" compact />
                <span v-else-if="instance.game_edition === 'bedrock'" class="instance-edition-badge">B</span>
                <LoaderBadge v-else :loader="instance.loader" size="md" />
              </span>
              <div class="instance-row-copy">
                <div class="instance-row-title">
                  <h3>{{ instance.display_name || instance.name }}</h3>
                  <span v-if="instance.running" class="instance-running-state"><i />Запущен</span>
                </div>
                <p>
                  {{ instance.game_edition === 'bedrock' ? 'Bedrock Edition' : (instance.kind === 'server' ? (instance.server_core || 'Сервер') : (instance.loader || 'Vanilla')) }}
                  <span>·</span>
                  {{ instance.game_edition === 'bedrock' ? 'Microsoft Store лицензия' : (instance.kind === 'server' ? (instance.server_core_version || instance.minecraft_version || instance.version_id || 'версия не указана') : (instance.minecraft_version || instance.version_id || 'версия не указана')) }}
                  <template v-if="instance.kind === 'server'"><span>·</span>:{{ instance.server_port || 25565 }}</template>
                  <span>·</span>
                  {{ formatPlayedTime(instance.played_seconds) }}
                </p>
                 <small v-if="instance.display_name && instance.display_name !== instance.name">{{ instance.name }}</small>
                  <small v-else-if="instance.game_edition === 'bedrock'">Отдельный профиль Bedrock</small>
                 <small v-else-if="instance.description">{{ instance.description }}</small>
                 <span v-if="(instance.tags || []).length" class="instance-row-tags"><i v-for="tag in instance.tags" :key="tag">#{{ tag }}</i></span>
                 <small v-if="instance.notes" class="instance-row-notes" v-tooltip="instance.notes">{{ instance.notes.slice(0, 90) }}{{ instance.notes.length > 90 ? '…' : '' }}</small>
              </div>
            </div>
            <div class="instance-row-controls">
              <div class="instance-row-group">
                <span>Подборка</span>
                <CustomSelect
                  :model-value="instance.group || ''"
                  :options="groupOptions"
                  :disabled="busy || groupBusy"
                  :aria-label="`Подборка экземпляра ${instance.display_name || instance.name}`"
                  @update:model-value="changeGroup(instance, $event)"
                />
              </div>
              <div class="instance-row-actions">
                <button class="button button--small" :class="instance.running ? 'button--outline' : 'button--accent'" type="button" :disabled="busy" @click="toggleInstance(instance)">
                  <Icon :name="instance.running ? 'stop' : 'play'" :size="14" />{{ instance.running ? 'Остановить' : (instance.kind === 'server' ? 'Запустить' : 'Играть') }}
                </button>
                <button class="icon-button" type="button" :aria-label="`Настройки экземпляра ${instance.display_name || instance.name}`" v-tooltip="'Настройки'" @click="emit('settings', instance)"><Icon name="settings" :size="15" /></button>
                <button v-if="instance.game_edition !== 'bedrock'" class="icon-button" type="button" :aria-label="`Дублировать ${instance.display_name || instance.name}`" v-tooltip="'Дублировать'" @click="emit('duplicate', instance)"><Icon name="copy" :size="15" /></button>
                <button class="icon-button icon-button--danger" type="button" :aria-label="`Удалить ${instance.display_name || instance.name}`" v-tooltip="'Удалить'" @click="emit('delete-instance', instance)"><Icon name="trash" :size="15" /></button>
              </div>
            </div>
          </article>
        </div>
        <div v-else class="instance-group-empty">
          <span class="empty-orb"><Icon :name="section.group ? 'folder' : 'instances'" :size="20" /></span>
          <div>
            <strong>{{ section.group ? 'В этой группе пока пусто' : 'Нет экземпляров без группы' }}</strong>
            <p>{{ emptyMessage(section.group) }}</p>
            <button v-if="activeFilter !== 'all'" class="text-button" type="button" @click="emit('update:activeFilter', 'all')">Показать все экземпляры</button>
          </div>
        </div>
      </section>
    </div>
  </section>
</template>
