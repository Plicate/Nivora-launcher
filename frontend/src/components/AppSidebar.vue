<script setup lang="ts">
import { computed } from 'vue'
import Icon from './Icon.vue'
import BrandLogo from './BrandLogo.vue'
import CustomSelect from './CustomSelect.vue'
import type { InstanceInfo, SelectOption, SidebarPosition, SidebarWidthMode, ViewName } from '../types'

const props = withDefaults(defineProps<{
  current: ViewName
  mobileOpen: boolean
  position: SidebarPosition
  widthMode: SidebarWidthMode
  cloudConfigured: boolean
  pluginTabs: Array<{ id: string; label: string }>
  activePluginTabId: string
  instances: InstanceInfo[]
  groups: string[]
  pinnedGroups?: string[]
  groupFilter: string
  groupBusy: boolean
  accountId: string
  accountInitial: string
  accountOptions: SelectOption[]
}>(), { pinnedGroups: () => [] })

const emit = defineEmits<{
  navigate: [view: ViewName]
  pluginTab: [pluginId: string]
  groupFilter: [filter: string]
  createGroup: []
  account: [accountId: string]
  close: []
  resizeStart: [event: PointerEvent]
}>()

const libraryItems: Array<{ view: ViewName; label: string; icon: string }> = [
  { view: 'instances', label: 'Экземпляры', icon: 'instances' },
  { view: 'accounts', label: 'Аккаунты', icon: 'accounts' },
  { view: 'mods', label: 'Каталог', icon: 'mods' },
  { view: 'modpacks', label: 'Модпаки', icon: 'archive' },
]

const toolItems: Array<{ view: ViewName; label: string; icon: string }> = [
  { view: 'plugins', label: 'Плагины', icon: 'plug' },
  { view: 'themes', label: 'Студия тем', icon: 'palette' },
]

const groupNames = computed(() => Array.from(new Set([
  ...props.groups.filter((group) => group.trim()),
  ...props.instances.map((instance) => instance.group).filter((group) => Boolean(group?.trim())),
])))
const orderedGroupNames = computed(() => [
  ...props.pinnedGroups.filter((group) => groupNames.value.includes(group)),
  ...groupNames.value.filter((group) => !props.pinnedGroups.includes(group)),
])
const groupFilters = computed(() => [
  { value: 'all', label: 'Все экземпляры', icon: 'instances', count: props.instances.length },
  { value: 'ungrouped', label: 'Без группы', icon: 'folder', count: props.instances.filter((instance) => !instance.group?.trim()).length },
  ...orderedGroupNames.value.map((group) => ({
    value: `group:${group}`,
    label: group,
    icon: 'folder',
    count: props.instances.filter((instance) => instance.group === group).length,
  })),
])
</script>

<template>
  <div v-if="mobileOpen" class="sidebar-scrim" @click="emit('close')" />
  <aside class="sidebar" :class="{ 'sidebar--open': mobileOpen }" :data-position="position">
    <div class="brand-lockup">
      <BrandLogo class="brand-mark" label="Nivora Launcher" />
      <div class="brand-copy"><strong>Nivora Launcher</strong><span>Лаунчер Minecraft</span></div>
    </div>

    <nav class="nav-list" aria-label="Основная навигация">
      <span class="nav-section-label">Библиотека</span>
      <button
        v-for="item in libraryItems"
        :key="item.view"
        class="nav-item"
        :class="{ 'nav-item--active': current === item.view }"
        :data-tour="item.view"
        type="button"
        v-tooltip="item.label"
        @click="emit('navigate', item.view)"
      >
        <Icon class="nav-item-icon" :name="item.icon" :size="17" />
        <span>{{ item.label }}</span>
      </button>
      <span class="nav-section-label nav-section-label--tools">Инструменты</span>
      <button
        v-for="item in toolItems"
        :key="item.view"
        class="nav-item"
        :class="{ 'nav-item--active': current === item.view && !(item.view === 'plugins' && activePluginTabId) }"
        :data-tour="item.view"
        type="button"
        v-tooltip="item.label"
        @click="emit('navigate', item.view)"
      >
        <Icon class="nav-item-icon" :name="item.icon" :size="17" />
        <span>{{ item.label }}</span>
      </button>
      <span v-if="pluginTabs.length" class="nav-section-label nav-section-label--tools">Вкладки плагинов</span>
      <button
        v-for="plugin in pluginTabs"
        :key="plugin.id"
        class="nav-item"
        :class="{ 'nav-item--active': activePluginTabId === plugin.id }"
        type="button"
        v-tooltip="plugin.label"
        @click="emit('pluginTab', plugin.id)"
      >
        <Icon class="nav-item-icon" name="plug" :size="17" />
        <span>{{ plugin.label }}</span>
      </button>
      <button
        class="nav-item nav-item--settings"
        data-tour="settings"
        :class="{ 'nav-item--active': current === 'settings' }"
        type="button"
        v-tooltip="'Настройки'"
        @click="emit('navigate', 'settings')"
      >
        <Icon class="nav-item-icon" name="settings" :size="17" />
        <span>Настройки</span>
      </button>
    </nav>

    <div class="sidebar-group-shortcuts" aria-label="Подборки экземпляров">
      <div class="sidebar-group-heading">
        <span class="nav-section-label">Подборки</span>
        <button class="sidebar-group-add" type="button" :disabled="groupBusy" aria-label="Создать подборку" v-tooltip="'Создать подборку'" @click="emit('createGroup')">
          <Icon name="plus" :size="15" />
        </button>
      </div>
      <button
        v-for="item in groupFilters"
        :key="item.value"
        class="sidebar-group-item"
        :class="{ 'sidebar-group-item--active': groupFilter === item.value }"
        type="button"
        :aria-label="`${item.label}: ${item.count} экземпляров`"
        :aria-pressed="groupFilter === item.value"
        v-tooltip="`${item.label} · ${item.count}`"
        @click="emit('groupFilter', item.value)"
      >
        <Icon :name="item.icon" :size="15" />
        <span>{{ item.label }}</span>
        <small>{{ item.count }}</small>
      </button>
    </div>

    <div class="sidebar-spacer" />
    <div class="sidebar-account">
      <span class="sidebar-account-caption">Запуск от имени</span>
      <div class="sidebar-account-control">
        <span class="sidebar-account-avatar" aria-hidden="true">{{ accountInitial || 'A' }}</span>
        <CustomSelect
          :model-value="accountId"
          :options="accountOptions"
          aria-label="Аккаунт для запуска игры"
          v-tooltip="'Выбрать аккаунт для запуска'"
          @update:model-value="emit('account', $event)"
        />
      </div>
    </div>
    <div class="sidebar-footer">
      <Icon class="sidebar-footer-icon" :name="cloudConfigured ? 'cloud' : 'hard-drive'" :size="14" />
      <span>{{ cloudConfigured ? 'Облако подключено' : 'Только локально' }}</span>
      <small>1.0</small>
    </div>
    <div
      v-if="widthMode === 'manual' && (position === 'left' || position === 'right')"
      class="sidebar-resize-handle"
      aria-label="Изменить ширину сайдбара"
      @pointerdown="emit('resizeStart', $event)"
    />
  </aside>
</template>
