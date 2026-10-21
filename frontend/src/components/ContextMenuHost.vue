<script setup lang="ts">
// хост контекстного меню: единственный экземпляр в App.vue, состояние —
import { computed, nextTick, ref, watch } from 'vue'
import Icon from './Icon.vue'
import { closeContextMenu, contextMenu } from '../services/contextMenu'

const menuElement = ref<HTMLElement | null>(null)
const flipped = ref(false)

watch(() => contextMenu.open, async (open) => {
  if (!open) return
  flipped.value = false
  await nextTick()
  const element = menuElement.value
  if (!element) return
  const bottom = contextMenu.y + element.offsetHeight + 12
  if (bottom > window.innerHeight) flipped.value = true
})

const style = computed(() => ({
  left: `${Math.min(contextMenu.x, Math.max(8, window.innerWidth - 280))}px`,
  top: `${contextMenu.y}px`,
}))

function run(action: () => void) {
  closeContextMenu()
  action()
}
</script>

<template>
  <Teleport to="body">
    <Transition name="dropdown">
      <nav v-if="contextMenu.open" ref="menuElement" class="context-menu" :class="{ 'context-menu--flipped': flipped }" :style="style" role="menu" @contextmenu.prevent>
        <div v-if="contextMenu.title" class="context-menu-title">{{ contextMenu.title }}</div>
        <button
          v-for="item in contextMenu.items"
          :key="item.id"
          type="button"
          class="context-menu-item"
          :class="{ 'context-menu-item--danger': item.danger }"
          role="menuitem"
          :disabled="item.disabled"
          @click="run(item.action)"
        >
          <Icon v-if="item.icon" :name="item.icon" :size="14" />
          <span>{{ item.label }}</span>
        </button>
      </nav>
    </Transition>
  </Teleport>
</template>
