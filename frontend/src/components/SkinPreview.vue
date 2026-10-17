<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { SkinRenderer, type BackEquipment } from '../services/skinRenderer'

const props = withDefaults(
  defineProps<{
    image?: string
    model?: 'classic' | 'slim'
    cape?: boolean
    capeImage?: string
    /** Показать текстуру плаща как элитры (как в Prism Launcher). */
    elytra?: boolean
  }>(),
  { image: '', model: 'classic', cape: false, capeImage: '', elytra: false },
)

const canvas = ref<HTMLCanvasElement | null>(null)
const webglSupported = ref(true)
const status = ref<string | null>('Загрузка текстуры…')

let renderer: SkinRenderer | undefined
let unsubscribe: (() => void) | undefined
let resizeObserver: ResizeObserver | undefined
let frame = 0
let lastFrame = 0
let lastInteraction = 0
let dragging = false
let lastPointerX = 0
let lastPointerY = 0
let yaw = -0.55
let pitch = 0.1
let zoom = 1
let width = 1
let height = 1

function back(): BackEquipment {
  if (!props.cape || !props.capeImage) return null
  return props.elytra ? 'elytra' : 'cape'
}

function render(now = performance.now()) {
  if (!renderer) return
  const elapsed = now - lastFrame
  lastFrame = now
  if (!dragging && now - lastInteraction > 900) yaw += elapsed * 0.00035
  renderer.render({ skin: props.image, model: props.model, cape: props.capeImage, back: back(), yaw, pitch, zoom }, width, height)
  status.value = renderer.skinStatus(props.image)
}

function tick(now: number) {
  // Без взаимодействия ~30 кадров/с и ничего во скрытой вкладке; при
  // перетаскивании — полная частота для плавного вращения.
  if (dragging || !document.hidden) {
    if (now - lastFrame >= (dragging ? 0 : 33)) render(now)
  }
  frame = window.requestAnimationFrame(tick)
}

function resize() {
  if (!canvas.value) return
  const bounds = canvas.value.getBoundingClientRect()
  const ratio = Math.min(2, window.devicePixelRatio || 1)
  width = Math.max(1, Math.round(bounds.width * ratio))
  height = Math.max(1, Math.round(bounds.height * ratio))
}

function pointerDown(event: PointerEvent) {
  dragging = true
  lastPointerX = event.clientX
  lastPointerY = event.clientY
  lastInteraction = performance.now()
  canvas.value?.setPointerCapture(event.pointerId)
}

function pointerMove(event: PointerEvent) {
  if (!dragging) return
  yaw += (event.clientX - lastPointerX) * 0.012
  pitch = Math.max(-0.8, Math.min(0.8, pitch + (event.clientY - lastPointerY) * 0.009))
  lastPointerX = event.clientX
  lastPointerY = event.clientY
  lastInteraction = performance.now()
}

function pointerUp() {
  dragging = false
  lastInteraction = performance.now()
}

function wheel(event: WheelEvent) {
  zoom = Math.max(0.78, Math.min(1.4, zoom * (event.deltaY > 0 ? 0.94 : 1.06)))
  lastInteraction = performance.now()
  event.preventDefault()
}

onMounted(() => {
  if (!canvas.value) return
  try {
    renderer = new SkinRenderer(canvas.value)
  } catch {
    webglSupported.value = false
    return
  }
  unsubscribe = renderer.onChange(() => render())
  resize()
  resizeObserver = new ResizeObserver(resize)
  resizeObserver.observe(canvas.value)
  lastInteraction = performance.now()
  frame = window.requestAnimationFrame(tick)
})

watch([() => props.image, () => props.model], () => render())
watch([() => props.capeImage, () => props.cape, () => props.elytra], () => {
  if (back()) {
    // Плащ и элитры видно сзади — разворачиваем модель спиной к зрителю.
    yaw = Math.PI - 0.5
    lastInteraction = performance.now()
  }
  render()
})

onBeforeUnmount(() => {
  window.cancelAnimationFrame(frame)
  resizeObserver?.disconnect()
  unsubscribe?.()
  renderer?.dispose()
  renderer = undefined
})
</script>

<template>
  <div class="skin-preview-stage" aria-label="Настоящий 3D preview скина">
    <canvas
      ref="canvas"
      class="skin-preview-canvas"
      aria-label="3D-модель персонажа Minecraft"
      @pointerdown="pointerDown"
      @pointermove="pointerMove"
      @pointerup="pointerUp"
      @pointercancel="pointerUp"
      @pointerleave="pointerUp"
      @wheel="wheel"
    />
    <span v-if="!webglSupported" class="skin-preview-fallback">WebGL недоступен</span>
    <span v-else-if="status" class="skin-preview-fallback">{{ status }}</span>
    <span class="skin-preview-hint">Потяните модель · колесо меняет масштаб</span>
    <div class="skin-preview-shadow" />
  </div>
</template>

<style scoped>
.skin-preview-stage { position: relative; display: grid; width: 260px; height: 286px; place-items: center; overflow: hidden; }
.skin-preview-canvas { position: absolute; inset: 0; width: 100%; height: 100%; cursor: grab; touch-action: none; }
.skin-preview-canvas:active { cursor: grabbing; }
.skin-preview-shadow { position: absolute; right: 50px; bottom: 28px; left: 50px; height: 22px; border-radius: 50%; background: rgba(0, 0, 0, .36); filter: blur(9px); pointer-events: none; }
.skin-preview-hint { position: absolute; right: 0; bottom: 0; left: 0; color: var(--muted); font-family: var(--font-mono); font-size: 8px; text-align: center; pointer-events: none; }
.skin-preview-fallback { position: absolute; top: 50%; padding: 4px 8px; border-radius: 6px; color: var(--muted); background: color-mix(in srgb, var(--panel) 80%, transparent); font-size: 10px; pointer-events: none; }
</style>
