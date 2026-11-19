<script setup lang="ts">
import { computed } from 'vue'
import PluginViewRenderer from './PluginViewRenderer.vue'
import { pluginThemeEffectsStore } from '../services/pluginThemeEffects'
import type { PluginUiView } from '../../../plugins/api'

const props = defineProps<{ slotName: string }>()

const entry = computed(() => {
  const rows = pluginThemeEffectsStore.slots.filter((slot) => slot.slot === props.slotName)
  return rows.length ? rows[rows.length - 1] : null
})

const view = computed<PluginUiView | null>(() => {
  if (!entry.value) return null
  return {
    id: `plugin-slot-${entry.value.slot}`,
    slot: 'sidebar_tab',
    title: '',
    root: entry.value.view as PluginUiView['root'],
  }
})
</script>

<template>
  <div v-if="view" class="plugin-slot-host" :data-plugin-slot="slotName">
    <PluginViewRenderer :view="view" />
  </div>
</template>
