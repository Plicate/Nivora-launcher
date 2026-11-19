<script setup lang="ts">
import { computed, ref } from 'vue'

const props = withDefaults(
  defineProps<{
    modelValue: number
    min?: number
    max?: number
    step?: number
    disabled?: boolean
    ariaLabel?: string
  }>(),
  { min: 0, max: 100, step: 1, disabled: false, ariaLabel: '' },
)
const emit = defineEmits<{ 'update:modelValue': [value: number]; change: [value: number] }>()

const root = ref<HTMLElement | null>(null)
const dragging = ref(false)

const clamped = computed(() => {
  const value = Number.isFinite(props.modelValue) ? props.modelValue : props.min
  return Math.min(props.max, Math.max(props.min, value))
})
const percent = computed(() => {
  const span = props.max - props.min
  return span > 0 ? ((clamped.value - props.min) / span) * 100 : 0
})

function commit(next: number) {
  const stepped = Math.round((next - props.min) / props.step) * props.step
  const value = Math.min(props.max, Math.max(props.min, Number((props.min + stepped).toFixed(6))))
  if (value !== clamped.value) {
    emit('update:modelValue', value)
    emit('change', value)
  }
}

function valueAt(clientX: number) {
  const rect = root.value?.getBoundingClientRect()
  if (!rect || rect.width <= 0) return clamped.value
  const ratio = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width))
  return props.min + ratio * (props.max - props.min)
}

function onPointerDown(event: PointerEvent) {
  if (props.disabled) return
  dragging.value = true
  ;(event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId)
  commit(valueAt(event.clientX))
}
function onPointerMove(event: PointerEvent) {
  if (!dragging.value || props.disabled) return
  commit(valueAt(event.clientX))
}
function onPointerUp() {
  dragging.value = false
}

function onKeydown(event: KeyboardEvent) {
  if (props.disabled) return
  const big = Math.max(props.step, (props.max - props.min) / 10)
  const keys: Record<string, number> = {
    ArrowRight: clamped.value + props.step,
    ArrowUp: clamped.value + props.step,
    ArrowLeft: clamped.value - props.step,
    ArrowDown: clamped.value - props.step,
    PageUp: clamped.value + big,
    PageDown: clamped.value - big,
    Home: props.min,
    End: props.max,
  }
  if (event.key in keys) {
    commit(keys[event.key])
    event.preventDefault()
  }
}
</script>

<template>
  <div
    ref="root"
    class="custom-slider"
    :class="{ 'custom-slider--disabled': disabled, 'custom-slider--dragging': dragging }"
    role="slider"
    :tabindex="disabled ? -1 : 0"
    :aria-label="ariaLabel || undefined"
    :aria-valuemin="min"
    :aria-valuemax="max"
    :aria-valuenow="clamped"
    :aria-disabled="disabled || undefined"
    @pointerdown="onPointerDown"
    @pointermove="onPointerMove"
    @pointerup="onPointerUp"
    @pointercancel="onPointerUp"
    @keydown="onKeydown"
  >
    <span class="custom-slider-track" />
    <span class="custom-slider-fill" :style="{ width: `${percent}%` }" />
    <span class="custom-slider-thumb" :style="{ left: `${percent}%` }" />
  </div>
</template>

<style scoped>
.custom-slider { position: relative; height: 22px; flex: 1 1 auto; min-width: 0; touch-action: none; cursor: pointer; outline: none; }
.custom-slider-track { position: absolute; left: 0; right: 0; top: 50%; height: 5px; transform: translateY(-50%); border-radius: 999px; background: color-mix(in srgb, var(--text, #fff) 10%, transparent); }
.custom-slider-fill { position: absolute; left: 0; top: 50%; height: 5px; transform: translateY(-50%); border-radius: 999px; background: var(--accent, var(--blue, #3d8bfd)); }
.custom-slider-thumb { position: absolute; top: 50%; width: 14px; height: 14px; transform: translate(-50%, -50%); border-radius: 999px; background: #fff; box-shadow: 0 1px 4px rgba(0, 0, 0, .35); transition: transform .15s var(--ease-spring, ease); }
.custom-slider:hover .custom-slider-thumb { transform: translate(-50%, -50%) scale(1.12); }
.custom-slider:focus-visible .custom-slider-thumb { box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent, var(--blue, #3d8bfd)) 30%, transparent), 0 1px 4px rgba(0, 0, 0, .35); }
.custom-slider--disabled { opacity: .45; cursor: not-allowed; }
</style>
