<script setup lang="ts">
import { ref, watch } from 'vue'

const props = withDefaults(
  defineProps<{
    modelValue: number
    min?: number
    max?: number
    step?: number
    decimals?: number
    disabled?: boolean
    invalid?: boolean
    ariaLabel?: string
  }>(),
  { min: 0, max: 999999, step: 1, decimals: 0, disabled: false, invalid: false, ariaLabel: 'Числовое значение' },
)

const emit = defineEmits<{ 'update:modelValue': [value: number] }>()
const draft = ref('')

function clamp(value: number): number {
  const bounded = Math.min(props.max, Math.max(props.min, value))
  const multiplier = 10 ** props.decimals
  return Math.round(bounded * multiplier) / multiplier
}

function format(value: number): string {
  return Number.isFinite(value) ? value.toFixed(props.decimals).replace(/\.0+$/, '').replace(/(\.\d*?)0+$/, '$1') : ''
}

function sync(value: number) {
  draft.value = format(clamp(Number(value)))
}

function commit(raw: string) {
  const value = Number(raw.trim().replace(',', '.'))
  if (!Number.isFinite(value)) {
    sync(props.modelValue)
    return
  }
  const next = clamp(value)
  draft.value = format(next)
  if (next !== props.modelValue) emit('update:modelValue', next)
}

function adjust(delta: number) {
  commit(String(Number(props.modelValue || 0) + delta))
}

function updateDraft(event: Event) {
  draft.value = (event.target as HTMLInputElement).value
}

watch(() => props.modelValue, sync, { immediate: true })
</script>

<template>
  <div class="numeric-stepper" :class="{ 'numeric-stepper--invalid': invalid, 'numeric-stepper--disabled': disabled }">
    <button class="numeric-stepper-button" type="button" :disabled="disabled" aria-label="Уменьшить" @click="adjust(-step)">−</button>
    <input
      :value="draft"
      type="text"
      inputmode="decimal"
      :disabled="disabled"
      :aria-label="ariaLabel"
      :aria-valuemin="min"
      :aria-valuemax="max"
      :aria-valuenow="modelValue"
      @input="updateDraft"
      @blur="commit(draft)"
      @keydown.enter="commit(draft)"
    />
    <button class="numeric-stepper-button" type="button" :disabled="disabled" aria-label="Увеличить" @click="adjust(step)">+</button>
  </div>
</template>
