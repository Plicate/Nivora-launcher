<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'

const props = withDefaults(defineProps<{ modelValue: string }>(), { modelValue: '#ffffff' })
const emit = defineEmits<{ 'update:modelValue': [value: string] }>()

const hue = ref(0)
const saturation = ref(0)
const value = ref(1)
const hexText = ref('#ffffff')
const wheel = ref<HTMLElement | null>(null)
const square = ref<HTMLElement | null>(null)
let dragging: 'hue' | 'square' | undefined

function clamp(input: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, input))
}

function parseHex(input: string): [number, number, number] | undefined {
  const normalized = input.trim().replace(/^#/, '')
  if (!/^[0-9a-f]{6}$/i.test(normalized)) return undefined
  return [Number.parseInt(normalized.slice(0, 2), 16) / 255, Number.parseInt(normalized.slice(2, 4), 16) / 255, Number.parseInt(normalized.slice(4, 6), 16) / 255]
}

function syncFromHex(input: string) {
  const rgb = parseHex(input)
  if (!rgb) return
  const [red, green, blue] = rgb
  const maximum = Math.max(red, green, blue)
  const minimum = Math.min(red, green, blue)
  const delta = maximum - minimum
  let nextHue = 0
  if (delta) {
    if (maximum === red) nextHue = ((green - blue) / delta) % 6
    else if (maximum === green) nextHue = (blue - red) / delta + 2
    else nextHue = (red - green) / delta + 4
    nextHue *= 60
    if (nextHue < 0) nextHue += 360
  }
  hue.value = nextHue
  saturation.value = maximum ? delta / maximum : 0
  value.value = maximum
  hexText.value = `#${input.trim().replace(/^#/, '').toLowerCase()}`
}

function hsvHex(): string {
  const saturationValue = saturation.value * value.value
  const chroma = saturationValue
  const huePart = hue.value / 60
  const x = chroma * (1 - Math.abs((huePart % 2) - 1))
  const match = huePart < 1 ? [chroma, x, 0] : huePart < 2 ? [x, chroma, 0] : huePart < 3 ? [0, chroma, x] : huePart < 4 ? [0, x, chroma] : huePart < 5 ? [x, 0, chroma] : [chroma, 0, x]
  const offset = value.value - chroma
  return `#${match.map((channel) => Math.round((channel + offset) * 255).toString(16).padStart(2, '0')).join('')}`
}

function emitColor() {
  const next = hsvHex()
  hexText.value = next
  if (next !== props.modelValue.toLowerCase()) emit('update:modelValue', next)
}

function updateHue(event: PointerEvent) {
  const bounds = wheel.value?.getBoundingClientRect()
  if (!bounds) return
  const x = event.clientX - (bounds.left + bounds.width / 2)
  const y = event.clientY - (bounds.top + bounds.height / 2)
  hue.value = (Math.atan2(y, x) * 180 / Math.PI + 450) % 360
  emitColor()
}

function updateSquare(event: PointerEvent) {
  const bounds = square.value?.getBoundingClientRect()
  if (!bounds) return
  saturation.value = clamp((event.clientX - bounds.left) / bounds.width, 0, 1)
  value.value = clamp(1 - (event.clientY - bounds.top) / bounds.height, 0, 1)
  emitColor()
}

function startDrag(kind: 'hue' | 'square', event: PointerEvent) {
  dragging = kind
  ;(event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId)
  if (kind === 'hue') updateHue(event)
  else updateSquare(event)
  window.addEventListener('pointermove', moveDrag)
  window.addEventListener('pointerup', stopDrag, { once: true })
}

function moveDrag(event: PointerEvent) {
  if (dragging === 'hue') updateHue(event)
  else if (dragging === 'square') updateSquare(event)
}

function stopDrag() {
  dragging = undefined
  window.removeEventListener('pointermove', moveDrag)
}

function applyText() {
  if (!parseHex(hexText.value)) {
    hexText.value = hsvHex()
    return
  }
  syncFromHex(hexText.value)
  emitColor()
}

const hueColor = computed(() => `hsl(${hue.value} 100% 50%)`)
const hueMarkerStyle = computed(() => {
  const angle = (hue.value - 90) * Math.PI / 180
  return { left: `${50 + Math.cos(angle) * 40}%`, top: `${50 + Math.sin(angle) * 40}%` }
})
const squareMarkerStyle = computed(() => ({ left: `${saturation.value * 100}%`, top: `${(1 - value.value) * 100}%` }))

watch(() => props.modelValue, (next) => syncFromHex(next), { immediate: true })
onBeforeUnmount(stopDrag)
</script>

<template>
  <div class="color-picker">
    <div ref="wheel" class="color-picker-wheel" @pointerdown="startDrag('hue', $event)">
      <span class="color-picker-hue-marker" :style="hueMarkerStyle" />
      <div ref="square" class="color-picker-square" :style="{ background: `linear-gradient(to top, #000, transparent), linear-gradient(to right, #fff, ${hueColor})` }" @pointerdown.stop="startDrag('square', $event)">
        <span class="color-picker-square-marker" :style="squareMarkerStyle" />
      </div>
    </div>
    <div class="color-picker-footer">
      <label><span>HEX</span><input v-model="hexText" maxlength="7" spellcheck="false" @change="applyText" @keyup.enter="applyText" /></label>
      <span class="color-picker-swatch" :style="{ background: hsvHex() }" />
    </div>
  </div>
</template>
