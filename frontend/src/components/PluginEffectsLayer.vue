<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import PluginViewRenderer from './PluginViewRenderer.vue'
import { pluginThemeEffects, pluginThemeEffectsStore } from '../services/pluginThemeEffects'
import { pluginRuntime } from '../services/pluginRuntime'
import type { PluginJsonValue, PluginUiView } from '../../../plugins/api'

const store = pluginThemeEffectsStore

const wallpaper = computed(() => (store.wallpapers.length ? store.wallpapers[store.wallpapers.length - 1] : null))
const overlay = computed(() => (store.effects.length ? store.effects[store.effects.length - 1] : null))
const hudWidgets = computed(() => store.hud)
const hasBehind = computed(() => store.emitters.some((emitter) => emitter.layer === 'behind'))
const hasAbove = computed(() => store.emitters.some((emitter) => emitter.layer === 'above'))

const wallpaperStyle = computed(() => {
  const entry = wallpaper.value
  if (!entry) return {}
  const fit = entry.fit === 'tile' ? 'repeat' : 'no-repeat'
  const size = entry.fit === 'tile' ? 'auto' : entry.fit
  return {
    backgroundImage: `url("${entry.image}")`,
    backgroundSize: size,
    backgroundRepeat: fit,
    backgroundPosition: 'center',
    filter: entry.blur ? `blur(${entry.blur}px)` : 'none',
  }
})

const overlayStyle = computed(() => {
  const entry = overlay.value
  if (!entry) return {}
  const spec = entry.spec
  const layers: string[] = []
  if (spec.vignette !== undefined) {
    layers.push(`radial-gradient(ellipse at center, transparent 45%, rgba(0, 0, 0, ${(0.65 * spec.vignette).toFixed(3)}) 100%)`)
  }
  if (spec.scanlines) {
    layers.push('repeating-linear-gradient(0deg, rgba(0, 0, 0, 0.16) 0px, rgba(0, 0, 0, 0.16) 1px, transparent 1px, transparent 3px)')
  }
  if (spec.noise !== undefined) {
    layers.push(`url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='160' height='160'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='2'/%3E%3C/filter%3E%3Crect width='160' height='160' filter='url(%23n)' opacity='0.5'/%3E%3C/svg%3E")`)
  }
  return {
    backgroundImage: layers.join(', '),
    opacity: '1',
    ...(spec.noise !== undefined ? { '--plugin-noise-opacity': String(spec.noise) } : {}),
  }
})

function widgetView(widget: (typeof store.hud)[number]): PluginUiView {
  return {
    id: widget.id,
    slot: 'sidebar_tab',
    title: '',
    root: widget.view as PluginUiView['root'],
  }
}

function onWidgetAction(widget: (typeof store.hud)[number], payload: { action: string; input: PluginJsonValue }): void {
  try {
    void pluginRuntime.runAction(widget.pluginId, payload.action, payload.input)
  } catch {
    // плагин мог остановиться к моменту нажатия
  }
}

function openWidgetLink(url: string): void {
  try { window.open(url, '_blank', 'noopener') } catch { /* браузер может запретить открытие */ }
}

function hudStyle(widget: (typeof store.hud)[number]): Record<string, string> {
  const style: Record<string, string> = { width: `${widget.width}px`, margin: `${widget.margin}px` }
  if (widget.position === 'top-center' || widget.position === 'bottom-center') style.margin = `0 ${widget.margin}px`
  return style
}

const behindRef = ref<HTMLCanvasElement | null>(null)
const aboveRef = ref<HTMLCanvasElement | null>(null)
let dpr = 1
let rafId = 0
let lastFrame = 0

interface ParticleState {
  x: number
  y: number
  vx: number
  vy: number
  angle: number
  factor: number
  size: number
  alpha: number
  color: string
  born: number
  life: number
  phase: number
  seed: number
}

const particlesByEmitter = new Map<string, ParticleState[]>()
const particlesByBurst = new Map<string, ParticleState[]>()
const imageCache = new Map<string, HTMLImageElement>()

function imageFor(dataUrl: string): HTMLImageElement | null {
  if (typeof document === 'undefined') return null
  let image = imageCache.get(dataUrl)
  if (!image) {
    image = new Image()
    image.src = dataUrl
    imageCache.set(dataUrl, image)
  }
  return image.complete && image.naturalWidth > 0 ? image : null
}

function zonePoint(zone: string, width: number, height: number): { x: number; y: number } {
  switch (zone) {
    case 'top': return { x: Math.random() * width, y: Math.random() * height * 0.3 }
    case 'bottom': return { x: Math.random() * width, y: height * (0.7 + Math.random() * 0.3) }
    case 'left': return { x: Math.random() * width * 0.3, y: Math.random() * height }
    case 'right': return { x: width * (0.7 + Math.random() * 0.3), y: Math.random() * height }
    case 'center': return { x: width * (0.25 + Math.random() * 0.5), y: height * (0.25 + Math.random() * 0.5) }
    default: return { x: Math.random() * width, y: Math.random() * height }
  }
}

function spawnParticle(emitter: (typeof store.emitters)[number], width: number, height: number): ParticleState {
  const point = zonePoint(emitter.zone, width, height)
  const spread = emitter.spread
  const angle = ((emitter.direction + (Math.random() - 0.5) * spread) * Math.PI) / 180
  const colors = emitter.colors.length ? emitter.colors : [emitter.color]
  return {
    x: point.x,
    y: point.y,
    vx: 0,
    vy: 0,
    angle,
    factor: 1 + (Math.random() - 0.5) * 2 * emitter.speedVariance,
    size: Math.max(0.6, emitter.size * (1 + (Math.random() - 0.5) * 2 * emitter.sizeVariance)),
    alpha: Math.max(0.05, Math.min(1, emitter.opacity * (1 + (Math.random() - 0.5) * 2 * emitter.opacityVariance))),
    color: colors[Math.floor(Math.random() * colors.length)] || emitter.color,
    born: performance.now(),
    life: emitter.lifetime > 0 ? emitter.lifetime * 1000 : 0,
    phase: Math.random() * Math.PI * 2,
    seed: Math.random(),
  }
}

function spawnBurstParticles(burst: (typeof store.bursts)[number], width: number, height: number): ParticleState[] {
  const list: ParticleState[] = []
  const centerX = burst.x * width
  const centerY = burst.y * height
  for (let index = 0; index < burst.count; index += 1) {
    const base = burst.spread >= 360 ? Math.random() * Math.PI * 2 : (-Math.PI / 2 + (Math.random() - 0.5) * (burst.spread * Math.PI) / 180)
    list.push({
      x: centerX,
      y: centerY,
      vx: 0,
      vy: 0,
      angle: base,
      factor: 0.4 + Math.random() * 0.6,
      size: Math.max(0.6, burst.size * (1 + (Math.random() - 0.5) * 2 * burst.sizeVariance)),
      alpha: Math.max(0.05, Math.min(1, 0.95)),
      color: burst.colors[index % burst.colors.length] || burst.color,
      born: performance.now(),
      life: burst.lifetime * 1000,
      phase: Math.random() * Math.PI * 2,
      seed: Math.random(),
    })
  }
  return list
}

function syncParticleArrays(): void {
  const liveIds = new Set(store.emitters.map((emitter) => emitter.id))
  for (const id of Array.from(particlesByEmitter.keys())) {
    if (!liveIds.has(id)) particlesByEmitter.delete(id)
  }
  for (const emitter of store.emitters) {
    if (!particlesByEmitter.has(emitter.id)) particlesByEmitter.set(emitter.id, [])
  }
  const liveBursts = new Set(store.bursts.map((burst) => burst.id))
  for (const id of Array.from(particlesByBurst.keys())) {
    if (!liveBursts.has(id)) particlesByBurst.delete(id)
  }
}

function drawShape(ctx: CanvasRenderingContext2D, shape: string, x: number, y: number, size: number, color: string, image?: string): void {
  if (shape === 'image' && image) {
    const bitmap = imageFor(image)
    if (bitmap) {
      ctx.drawImage(bitmap, x - size, y - size, size * 2, size * 2)
      return
    }
  }
  ctx.fillStyle = color
  switch (shape) {
    case 'square': {
      ctx.fillRect(x - size, y - size, size * 2, size * 2)
      return
    }
    case 'diamond': {
      ctx.beginPath()
      ctx.moveTo(x, y - size)
      ctx.lineTo(x + size, y)
      ctx.lineTo(x, y + size)
      ctx.lineTo(x - size, y)
      ctx.closePath()
      ctx.fill()
      return
    }
    case 'star': {
      ctx.beginPath()
      for (let point = 0; point < 10; point += 1) {
        const radius = point % 2 === 0 ? size : size * 0.45
        const angle = (Math.PI / 5) * point - Math.PI / 2
        const px = x + Math.cos(angle) * radius
        const py = y + Math.sin(angle) * radius
        if (point === 0) ctx.moveTo(px, py)
        else ctx.lineTo(px, py)
      }
      ctx.closePath()
      ctx.fill()
      return
    }
    case 'heart': {
      ctx.beginPath()
      const topCurveHeight = size * 0.4
      ctx.moveTo(x, y + size * 0.8)
      ctx.bezierCurveTo(x - size * 1.3, y + size * 0.1, x - size * 0.7, y - topCurveHeight, x, y - size * 0.2)
      ctx.bezierCurveTo(x + size * 0.7, y - topCurveHeight, x + size * 1.3, y + size * 0.1, x, y + size * 0.8)
      ctx.fill()
      return
    }
    default: {
      ctx.beginPath()
      ctx.arc(x, y, size, 0, Math.PI * 2)
      ctx.fill()
    }
  }
}

function step(now: number): void {
  rafId = requestAnimationFrame(step)
  const dt = Math.min(0.05, (now - lastFrame) / 1000) || 0.016
  lastFrame = now
  const width = window.innerWidth
  const height = window.innerHeight
  syncParticleArrays()

  const layers: Array<{ canvas: HTMLCanvasElement | null; layer: string; bursts: boolean }> = [
    { canvas: behindRef.value, layer: 'behind', bursts: false },
    { canvas: aboveRef.value, layer: 'above', bursts: true },
  ]

  for (const target of layers) {
    const canvas = target.canvas
    if (!canvas) continue
    const ctx = canvas.getContext('2d')
    if (!ctx) continue
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    ctx.clearRect(0, 0, width, height)

    for (const emitter of store.emitters) {
      if (emitter.layer !== target.layer) continue
      let list = particlesByEmitter.get(emitter.id)
      if (!list) {
        list = []
        particlesByEmitter.set(emitter.id, list)
      }
      let spawned = 0
      while (list.length < emitter.count && spawned < 8) {
        list.push(spawnParticle(emitter, width, height))
        spawned += 1
      }
      ctx.globalCompositeOperation = emitter.blend === 'lighter' ? 'lighter' : 'source-over'
      const keep: ParticleState[] = []
      for (const particle of list) {
        particle.phase += emitter.swaySpeed * Math.PI * 2 * dt
        particle.vy += emitter.gravity * dt
        particle.vx += emitter.wind * dt
        particle.x += (Math.cos(particle.angle) * emitter.speed * particle.factor + particle.vx + Math.sin(particle.phase) * emitter.sway) * dt
        particle.y += (Math.sin(particle.angle) * emitter.speed * particle.factor + particle.vy) * dt
        const age = now - particle.born
        const expired = particle.life > 0 && age > particle.life
        const offscreen = particle.x < -40 || particle.x > width + 40 || particle.y < -40 || particle.y > height + 40
        if (expired || offscreen) {
          if (emitter.respawn) {
            const fresh = spawnParticle(emitter, width, height)
            particle.x = fresh.x
            particle.y = fresh.y
            particle.born = now
            particle.phase = fresh.phase
            particle.color = fresh.color
            particle.size = fresh.size
            particle.alpha = fresh.alpha
            particle.angle = fresh.angle
            particle.factor = fresh.factor
            particle.vx = 0
            particle.vy = 0
          } else {
            continue
          }
        }
        let alpha = particle.alpha
        if (particle.life > 0) {
          const remaining = 1 - age / particle.life
          alpha *= Math.max(0, Math.min(1, remaining * 3))
        }
        ctx.globalAlpha = Math.max(0, Math.min(1, alpha))
        drawShape(ctx, emitter.shape, particle.x, particle.y, particle.size, particle.color, emitter.image)
        keep.push(particle)
      }
      list.length = 0
      for (const particle of keep) list.push(particle)
      ctx.globalCompositeOperation = 'source-over'
      ctx.globalAlpha = 1
    }

    if (target.bursts) {
      for (const burst of store.bursts) {
        let list = particlesByBurst.get(burst.id)
        if (!list) {
          list = spawnBurstParticles(burst, width, height)
          particlesByBurst.set(burst.id, list)
        }
        ctx.globalCompositeOperation = burst.blend === 'lighter' ? 'lighter' : 'source-over'
        const keep: ParticleState[] = []
        for (const particle of list) {
          particle.vy += burst.gravity * dt
          particle.x += (Math.cos(particle.angle) * burst.speed * particle.factor + particle.vx) * dt
          particle.y += (Math.sin(particle.angle) * burst.speed * particle.factor + particle.vy) * dt
          const age = now - particle.born
          const remaining = 1 - age / (burst.lifetime * 1000)
          if (remaining <= 0 || particle.y > height + 40) continue
          ctx.globalAlpha = Math.max(0, Math.min(1, particle.alpha * remaining))
          drawShape(ctx, burst.shape, particle.x, particle.y, particle.size, particle.color, burst.image)
          keep.push(particle)
        }
        list.length = 0
        for (const particle of keep) list.push(particle)
        ctx.globalCompositeOperation = 'source-over'
        ctx.globalAlpha = 1
        if (!keep.length) pluginThemeEffects.particlesRemoveBurst(burst.id)
      }
    }
  }
}

function syncCanvases(): void {
  if (typeof window === 'undefined') return
  dpr = Math.min(window.devicePixelRatio || 1, 2)
  for (const canvas of [behindRef.value, aboveRef.value]) {
    if (!canvas) continue
    canvas.width = Math.max(1, Math.round(window.innerWidth * dpr))
    canvas.height = Math.max(1, Math.round(window.innerHeight * dpr))
  }
}

onMounted(() => {
  syncCanvases()
  window.addEventListener('resize', syncCanvases)
  lastFrame = performance.now()
  rafId = requestAnimationFrame(step)
})

onBeforeUnmount(() => {
  cancelAnimationFrame(rafId)
  window.removeEventListener('resize', syncCanvases)
  particlesByEmitter.clear()
  particlesByBurst.clear()
})

watch(hasBehind, () => syncCanvases())
watch(hasAbove, () => syncCanvases())
</script>

<template>
  <div v-if="wallpaper" class="plugin-wallpaper-layer" aria-hidden="true" :style="wallpaperStyle">
    <div class="plugin-wallpaper-dim" :style="{ opacity: String(wallpaper.dim) }" />
  </div>
  <canvas v-show="hasBehind" ref="behindRef" class="plugin-particles-canvas plugin-particles-canvas--behind" aria-hidden="true" />
  <canvas v-show="hasAbove" ref="aboveRef" class="plugin-particles-canvas plugin-particles-canvas--above" aria-hidden="true" />
  <div v-if="overlay" class="plugin-effect-overlay" aria-hidden="true" :style="overlayStyle" />
  <div
    v-for="widget in hudWidgets"
    :key="widget.id"
    class="plugin-hud-widget"
    :class="`plugin-hud-widget--${widget.position}`"
    :style="hudStyle(widget)"
  >
    <PluginViewRenderer :view="widgetView(widget)" @action="onWidgetAction(widget, $event)" @open-link="openWidgetLink" />
  </div>
</template>

<style>
.plugin-wallpaper-layer { position: fixed; z-index: 0; inset: 0; overflow: hidden; pointer-events: none; }
.plugin-wallpaper-dim { position: absolute; inset: 0; background: #000; }
.plugin-particles-canvas { position: fixed; inset: 0; width: 100%; height: 100%; pointer-events: none; }
.plugin-particles-canvas--behind { z-index: 0; }
.plugin-particles-canvas--above { z-index: 50; }
.plugin-effect-overlay { position: fixed; z-index: 40; inset: 0; pointer-events: none; opacity: var(--plugin-noise-opacity, 1); }
.plugin-hud-widget { position: fixed; z-index: 24; pointer-events: auto; max-height: 70vh; overflow: auto; color: var(--text, #e8edf7); }
.plugin-hud-widget--top-left { top: 0; left: 0; }
.plugin-hud-widget--top-right { top: 0; right: 0; }
.plugin-hud-widget--bottom-left { bottom: 0; left: 0; }
.plugin-hud-widget--bottom-right { bottom: 0; right: 0; }
.plugin-hud-widget--top-center { top: 0; left: 50%; transform: translateX(-50%); }
.plugin-hud-widget--bottom-center { bottom: 0; left: 50%; transform: translateX(-50%); }
.plugin-slot-host { display: block; }
</style>
