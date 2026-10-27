<script setup lang="ts">
// раскрывающийся блок вместо нативного <details>: у системного нет ни
// анимации раскрытия, ни стилизованной стрелки
import { ref } from 'vue'
import Icon from './Icon.vue'

withDefaults(
  defineProps<{
    title: string
    hint?: string
  }>(),
  { hint: '' },
)

const expanded = ref(false)
</script>

<template>
  <div class="collapsible" :class="{ 'collapsible--open': expanded }">
    <button class="collapsible__head" type="button" :aria-expanded="expanded" @click="expanded = !expanded">
      <span class="collapsible__title"><strong>{{ title }}</strong><small v-if="hint">{{ hint }}</small></span>
      <Icon name="chevron" :size="16" />
    </button>
    <div class="collapsible__body">
      <div class="collapsible__inner"><slot /></div>
    </div>
  </div>
</template>

<style scoped>
.collapsible { border: 1px solid var(--line); border-radius: var(--radius-control, 10px); background: var(--panel-soft); overflow: hidden; }
.collapsible__head {
  display: flex;
  width: 100%;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 9px 12px;
  border: 0;
  background: transparent;
  color: var(--text);
  cursor: pointer;
  transition: background-color .15s ease;
}
.collapsible__head:hover { background: var(--panel-raised); }
.collapsible__head:focus-visible { outline: none; box-shadow: inset 0 0 0 2px var(--blue); }
.collapsible__title { display: flex; align-items: baseline; gap: 8px; min-width: 0; flex-wrap: wrap; }
.collapsible__title strong { font-size: 12.5px; font-weight: 600; }
.collapsible__title small { color: var(--muted); font-size: 11px; }
.collapsible__head svg { flex: 0 0 auto; color: var(--blue-bright); transition: transform .22s ease; }
.collapsible--open .collapsible__head svg { transform: rotate(180deg); }
.collapsible__body {
  display: grid;
  grid-template-rows: 0fr;
  transition: grid-template-rows .24s ease;
}
.collapsible--open .collapsible__body { grid-template-rows: 1fr; }
.collapsible__inner { min-height: 0; overflow: hidden; padding: 0 12px; }
.collapsible--open .collapsible__inner { padding: 4px 12px 12px; }
</style>
