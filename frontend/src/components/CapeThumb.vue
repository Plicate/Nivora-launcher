<script setup lang="ts">
// лицевая сторона плаща, как в Prism Launcher: из развёртки 64×32 (или HD)
// область плаща от (1,1) до (11,17) рисуется без сглаживания
import { onMounted, ref, watch } from 'vue'
import { capeTextureScale } from '../services/skinRenderer'

const props = defineProps<{ image: string }>()
const canvas = ref<HTMLCanvasElement | null>(null)
const failed = ref(false)

function draw() {
  const target = canvas.value
  if (!target || !props.image) return
  failed.value = false
  const image = new Image()
  image.decoding = 'async'
  image.onload = () => {
    const context = target.getContext('2d')
    if (!context) return
    const scale = capeTextureScale(image.naturalWidth, image.naturalHeight)
    // холст в пикселях текстуры, растягивается CSS-ом (image-rendering: pixelated)
    target.width = Math.max(1, Math.round(10 * scale))
    target.height = Math.max(1, Math.round(16 * scale))
    context.imageSmoothingEnabled = false
    context.clearRect(0, 0, target.width, target.height)
    context.drawImage(image, scale, scale, 10 * scale, 16 * scale, 0, 0, target.width, target.height)
  }
  image.onerror = () => { failed.value = true }
  image.src = props.image
}

onMounted(draw)
watch(() => props.image, draw)
</script>

<template>
  <span class="cape-thumb">
    <canvas v-show="!failed" ref="canvas" aria-label="Лицевая сторона плаща" />
    <span v-if="failed" class="cape-thumb-error">нет текстуры</span>
  </span>
</template>

<style scoped>
.cape-thumb { display: grid; width: 100%; height: 100%; place-items: center; }
.cape-thumb canvas { height: 100%; max-width: 100%; aspect-ratio: 10 / 16; image-rendering: pixelated; filter: drop-shadow(0 4px 8px rgba(0, 0, 0, .35)); }
.cape-thumb-error { color: var(--muted); font-size: 9px; }
</style>
