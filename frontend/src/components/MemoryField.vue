<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import CustomSelect from './CustomSelect.vue'
import NumericStepper from './NumericStepper.vue'
import type { MemoryUnit, SelectOption } from '../types'

const props = withDefaults(defineProps<{ modelValue: string; maxKib?: number; disabled?: boolean }>(), { maxKib: 0, disabled: false })
const emit = defineEmits<{ 'update:modelValue': [value: string] }>()

const value = ref(1)
const unit = ref<MemoryUnit>('GiB')
const automatic = ref(false)
const multipliers: Record<MemoryUnit, number> = { KiB: 1, MiB: 1024, GiB: 1024 * 1024 }
const unitOptions: SelectOption[] = [
  { value: 'KiB', label: 'KiB', meta: '1024 байта' },
  { value: 'MiB', label: 'MiB', meta: '1024 KiB' },
  { value: 'GiB', label: 'GiB', meta: '1024 MiB' },
]

const maximum = computed(() => props.maxKib > 0 ? Math.max(1, Math.floor(props.maxKib / multipliers[unit.value])) : 8192)

function sync(raw: string) {
  const text = String(raw || '').trim()
  if (!text || text.toLowerCase() === 'auto' || text.toLowerCase() === 'automatic') {
    automatic.value = true
    return
  }
  const match = text.match(/^(\d+(?:[.,]\d+)?)\s*(KiB|MiB|GiB|K|M|G)$/i)
  if (!match) {
    automatic.value = true
    return
  }
  const suffix = match[2].toLowerCase()
  unit.value = suffix.startsWith('g') ? 'GiB' : suffix.startsWith('k') ? 'KiB' : 'MiB'
  value.value = Math.max(1, Number(match[1].replace(',', '.')))
  automatic.value = false
}

function emitValue() {
  if (automatic.value) {
    emit('update:modelValue', 'auto')
    return
  }
  emit('update:modelValue', `${value.value}${unit.value}`)
}

function setAutomatic(next: boolean) {
  automatic.value = next
  if (!next && !Number.isFinite(value.value)) value.value = 1
  emitValue()
}

function changeUnit(next: string) {
  const previous = multipliers[unit.value]
  const nextUnit = next as MemoryUnit
  const total = value.value * previous
  unit.value = nextUnit
  value.value = Math.max(1, Math.round((total / multipliers[nextUnit]) * 100) / 100)
  emitValue()
}

function changeValue(next: number) {
  value.value = next
  emitValue()
}

watch(() => props.modelValue, sync, { immediate: true })
</script>

<template>
  <div class="memory-field">
    <div class="memory-field-controls">
      <NumericStepper :model-value="value" :min="1" :max="maximum" :step="unit === 'GiB' ? 1 : 128" :decimals="unit === 'GiB' ? 2 : 0" :disabled="disabled || automatic" aria-label="Объём памяти" @update:model-value="changeValue" />
      <CustomSelect :model-value="unit" :options="unitOptions" :disabled="disabled || automatic" @update:model-value="changeUnit" />
      <button class="memory-auto-toggle" :class="{ active: automatic }" type="button" :disabled="disabled" @click="setAutomatic(!automatic)">{{ automatic ? 'Авто' : 'Авто' }}</button>
    </div>
    <small class="field-help">{{ automatic ? 'Размер подбирается автоматически.' : `От 1 до ${maximum} ${unit}.` }}</small>
  </div>
</template>
