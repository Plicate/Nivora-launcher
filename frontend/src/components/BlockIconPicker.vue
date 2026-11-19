<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import Icon from './Icon.vue'
import { composedBlockIcon } from '../services/blockIcon'
import type { InstanceIconBlock } from '../types'

const props = withDefaults(
  defineProps<{
    blocks: InstanceIconBlock[]
    modelValue: string
    loading?: boolean
  }>(),
  { loading: false },
)

const emit = defineEmits<{ 'update:modelValue': [value: string] }>()

const open = ref(false)
const root = ref<HTMLElement | null>(null)
const query = ref('')

const selected = computed(() => props.blocks.find((block) => block.id === props.modelValue))

const composedIcons = ref<Map<string, string>>(new Map())
let composeCancelled = false

async function composeIcons(blocks: InstanceIconBlock[]) {
  for (const block of blocks) {
    if (composeCancelled) return
    const url = await composedBlockIcon(block)
    if (composeCancelled) return
    const next = new Map(composedIcons.value)
    next.set(block.id, url)
    composedIcons.value = next
  }
}

watch(
  () => props.blocks,
  (blocks) => {
    void composeIcons(blocks)
  },
  { immediate: true },
)

function iconFor(block: InstanceIconBlock): string {
  return composedIcons.value.get(block.id) || block.data_url
}

const selectedIcon = computed(() => (selected.value ? iconFor(selected.value) : ''))
const filtered = computed(() => {
  const term = query.value.trim().toLowerCase()
  const list = term ? props.blocks.filter((block) => block.name.toLowerCase().includes(term) || block.id.includes(term)) : props.blocks
  return list.slice(0, 120)
})

function choose(blockId: string) {
  emit('update:modelValue', blockId)
  open.value = false
}

function toggle() {
  open.value = !open.value
  if (open.value) query.value = ''
}

function onDocumentPointer(event: MouseEvent) {
  if (root.value && !root.value.contains(event.target as Node)) open.value = false
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape' && open.value) {
    open.value = false
    event.stopPropagation()
  }
}

onMounted(() => document.addEventListener('mousedown', onDocumentPointer))
onBeforeUnmount(() => {
  document.removeEventListener('mousedown', onDocumentPointer)
  composeCancelled = true
})
</script>

<template>
  <div ref="root" class="block-icon-picker" @keydown="onKeydown">
    <button
      type="button"
      class="block-icon-trigger"
      :class="{ 'block-icon-trigger--open': open }"
      :aria-expanded="open"
      aria-haspopup="dialog"
      @click="toggle"
    >
      <span class="block-icon-trigger-mark">
        <img v-if="selected" :src="selectedIcon" :alt="selected.name" />
        <Icon v-else name="instances" :size="19" />
      </span>
      <span class="block-icon-trigger-copy">
        <strong>{{ selected ? selected.name : 'По умолчанию' }}</strong>
      </span>
      <Icon name="arrow-down" :size="14" class="block-icon-trigger-caret" :class="{ 'block-icon-trigger-caret--open': open }" />
    </button>

    <Transition name="block-icon-pop">
      <div v-if="open" class="block-icon-pop" role="dialog" aria-label="Выбор иконки-блока">
        <label class="block-icon-search">
          <Icon name="search" :size="13" />
          <input v-model="query" type="text" placeholder="Найти блок…" spellcheck="false" />
        </label>
        <div class="block-icon-grid" role="radiogroup" aria-label="Иконка экземпляра">
          <button
            type="button"
            class="block-icon-choice"
            :class="{ 'block-icon-choice--active': !modelValue }"
            role="radio"
            :aria-checked="!modelValue"
            @click="choose('')"
          >
            <span class="block-icon-default"><Icon name="instances" :size="18" /></span>
            <small>По умолчанию</small>
          </button>
          <button
            v-for="block in filtered"
            :key="block.id"
            type="button"
            class="block-icon-choice"
            :class="{ 'block-icon-choice--active': modelValue === block.id }"
            role="radio"
            :aria-checked="modelValue === block.id"
            :title="block.name"
            @click="choose(block.id)"
          >
            <img :src="iconFor(block)" :alt="block.name" loading="lazy" />
            <small>{{ block.name }}</small>
          </button>
        </div>
        <p v-if="loading" class="muted block-picker-hint">Читаю текстуры блоков…</p>
        <p v-else-if="!blocks.length" class="muted block-picker-hint">Установите Minecraft, чтобы загрузить текстуры блоков.</p>
        <p v-else-if="!filtered.length" class="muted block-picker-hint">Блоков по запросу «{{ query }}» не нашлось.</p>
      </div>
    </Transition>
  </div>
</template>
