<script setup lang="ts">
// Миниатюрная 3D-модель скина. Рисуется общим WebGL-рендерером (один
// контекст на все миниатюры) и копируется в собственный 2D-canvas; модель
// крутится перетаскиванием, как основной предпросмотр.
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { thumbnailRenderer } from '../services/skinRenderer'

const props = withDefaults(defineProps<{ image: string; model?: 'classic' | 'slim'; capeImage?: string }>(), { model: 'classic', capeImage: '' })

const canvas = ref<HTMLCanvasElement | null>(null)
const unavailable = ref(false)
const failed = ref(false)
let yaw = -0.5
let pitch = 0.12
let dragging = false
let moved = false
let lastX = 0
let lastY = 0
let pending = 0
let unsubscribe: (() => void) | undefined
let visible = true
let observer: IntersectionObserver | undefined

function draw() {
  pending = 0
  const target = canvas.value
  const renderer = thumbnailRenderer()
  if (!target || !renderer) {
    unavailable.value = !renderer
    return
  }
  if (!visible) return
  const bounds = target.getBoundingClientRect()
  const ratio = Math.min(2, window.devicePixelRatio || 1)
  const width = Math.max(1, Math.round(bounds.width * ratio))
  const height = Math.max(1, Math.round(bounds.height * ratio))
  if (target.width !== width || target.height !== height) {
    target.width = width
    target.height = height
  }
  renderer.render({ skin: props.image, model: props.model, cape: props.capeImage, back: props.capeImage ? 'cape' : null, yaw, pitch, zoom: 1.08 }, width, height)
  // Текстура не загрузилась (например, удалённый https-адрес без CORS) —
  // показываем обычную картинку вместо пустого места.
  const status = renderer.skinStatus(props.image)
  failed.value = status !== null && !status.startsWith('Загрузка')
  const context = target.getContext('2d')
  if (!context) return
  context.clearRect(0, 0, width, height)
  context.drawImage(renderer.canvas, 0, 0, width, height)
}

function schedule() {
  if (!pending) pending = window.requestAnimationFrame(draw)
}

function pointerDown(event: PointerEvent) {
  dragging = true
  moved = false
  lastX = event.clientX
  lastY = event.clientY
  canvas.value?.setPointerCapture(event.pointerId)
}

function pointerMove(event: PointerEvent) {
  if (!dragging) return
  const dx = event.clientX - lastX
  const dy = event.clientY - lastY
  if (Math.abs(dx) + Math.abs(dy) > 2) moved = true
  yaw += dx * 0.02
  pitch = Math.max(-0.8, Math.min(0.8, pitch + dy * 0.012))
  lastX = event.clientX
  lastY = event.clientY
  schedule()
}

function pointerUp() {
  dragging = false
}

// Вращение не должно выбирать карточку: клик после перетаскивания гасим.
function click(event: MouseEvent) {
  if (moved) {
    event.stopPropagation()
    event.preventDefault()
    moved = false
  }
}

onMounted(() => {
  const renderer = thumbnailRenderer()
  unsubscribe = renderer?.onChange(schedule)
  if (canvas.value && 'IntersectionObserver' in window) {
    observer = new IntersectionObserver((entries) => {
      visible = entries.some((entry) => entry.isIntersecting)
      if (visible) schedule()
    })
    observer.observe(canvas.value)
  }
  schedule()
})

watch(() => [props.image, props.model, props.capeImage], schedule)

onBeforeUnmount(() => {
  if (pending) window.cancelAnimationFrame(pending)
  unsubscribe?.()
  observer?.disconnect()
})
</script>

<template>
  <span class="skin-thumb-3d" :title="unavailable || failed ? '' : 'Потяните, чтобы повернуть'">
    <canvas
      ref="canvas"
      aria-label="3D-миниатюра скина"
      @pointerdown.stop="pointerDown"
      @pointermove="pointerMove"
      @pointerup="pointerUp"
      @pointercancel="pointerUp"
      @click.capture="click"
    />
    <img v-if="(unavailable || failed) && image" :src="image" alt="" class="skin-thumb-fallback" />
  </span>
</template>

<style scoped>
.skin-thumb-3d { position: relative; display: block; width: 100%; height: 100%; }
.skin-thumb-3d canvas { display: block; width: 100%; height: 100%; cursor: grab; touch-action: none; }
.skin-thumb-3d canvas:active { cursor: grabbing; }
.skin-thumb-fallback { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: contain; image-rendering: pixelated; }
</style>
