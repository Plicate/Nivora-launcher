<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from 'vue'

const props = withDefaults(
  defineProps<{
    modelValue: string
    presets?: string[]
    ariaLabel?: string
  }>(),
  {
    presets: () => ['#000000', '#404040', '#808080', '#c0c0c0', '#ffffff', '#8b0000', '#ff5555', '#ffaa00', '#ffff55', '#55ff55', '#00aa00', '#55ffff', '#00aaaa', '#5555ff', '#0000aa', '#ff55ff', '#aa00aa', '#8b653f'],
    ariaLabel: '',
  },
)
const emit = defineEmits<{ 'update:modelValue': [value: string] }>()

const paletteOpen = ref(false)
const editing = ref(false)
const draftText = ref('')

function isHex(value: string) {
  return /^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.test(value.trim())
}

function startEdit() {
  draftText.value = props.modelValue
  editing.value = true
}
function commitEdit() {
  const value = draftText.value.trim()
  if (isHex(value)) emit('update:modelValue', value.toLowerCase())
  editing.value = false
}
function pick(preset: string) {
  emit('update:modelValue', preset)
  paletteOpen.value = false
}

function onDocPointer(event: PointerEvent) {
  const target = event.target as HTMLElement | null
  if (!target?.closest?.('.custom-color')) paletteOpen.value = false
}
watch(paletteOpen, (open) => {
  if (open) document.addEventListener('pointerdown', onDocPointer)
  else document.removeEventListener('pointerdown', onDocPointer)
})
onBeforeUnmount(() => document.removeEventListener('pointerdown', onDocPointer))
</script>

<template>
  <div class="custom-color">
    <button
      type="button"
      class="custom-color-swatch"
      :style="{ background: modelValue }"
      :aria-label="ariaLabel || 'Выбрать цвет'"
      @click="paletteOpen = !paletteOpen"
    />
    <input
      v-if="editing"
      v-model="draftText"
      class="custom-color-input"
      :class="{ 'custom-color-input--invalid': !isHex(draftText) }"
      spellcheck="false"
      maxlength="7"
      @keydown.enter.prevent="commitEdit"
      @blur="commitEdit"
    />
    <code
      v-else
      class="custom-color-code"
      role="button"
      tabindex="0"
      @click="startEdit"
      @keydown.enter.prevent="startEdit"
    >{{ modelValue }}</code>
    <div v-if="paletteOpen" class="custom-color-palette">
      <button
        v-for="preset in presets"
        :key="preset"
        type="button"
        class="custom-color-pick"
        :class="{ 'custom-color-pick--active': preset.toLowerCase() === modelValue.toLowerCase() }"
        :style="{ background: preset }"
        :aria-label="preset"
        @click="pick(preset)"
      />
    </div>
  </div>
</template>

<style scoped>
.custom-color { position: relative; display: inline-flex; align-items: center; gap: 8px; }
.custom-color-swatch { width: 30px; height: 30px; flex: 0 0 30px; padding: 0; border: 1px solid var(--line-strong, rgba(255, 255, 255, .13)); border-radius: 8px; cursor: pointer; transition: transform .15s var(--ease-spring, ease); }
.custom-color-swatch:hover { transform: scale(1.06); }
.custom-color-code { cursor: pointer; font-family: var(--font-mono, monospace); font-size: 12px; color: var(--muted, #7d8591); }
.custom-color-input { width: 86px; height: 30px; padding: 0 8px; border: 1px solid var(--line, rgba(255, 255, 255, .07)); border-radius: 8px; background: var(--panel-raised, #1a1e24); color: var(--text, #fff); font-family: var(--font-mono, monospace); font-size: 12px; outline: none; }
.custom-color-input--invalid { border-color: var(--danger, #e5484d); }
.custom-color-palette { position: absolute; top: calc(100% + 6px); left: 0; z-index: 60; display: grid; grid-template-columns: repeat(9, 20px); gap: 5px; padding: 8px; border: 1px solid var(--line-strong, rgba(255, 255, 255, .13)); border-radius: 10px; background: var(--panel-raised, #1a1e24); box-shadow: 0 16px 40px rgba(0, 0, 0, .4); }
.custom-color-pick { width: 20px; height: 20px; padding: 0; border: 1px solid rgba(0, 0, 0, .25); border-radius: 6px; cursor: pointer; }
.custom-color-pick--active { outline: 2px solid var(--accent, var(--blue, #3d8bfd)); outline-offset: 1px; }
</style>
