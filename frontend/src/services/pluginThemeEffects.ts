import { reactive } from 'vue'
import type {
  PluginCursorSpec,
  PluginEffectSpec,
  PluginHudOptions,
  PluginHudPosition,
  PluginParticleBlend,
  PluginParticleBurstSpec,
  PluginParticleLayer,
  PluginParticleZone,
  PluginParticleSpec,
  PluginSoundOptions,
  PluginSlotName,
  PluginStylePatch,
  PluginStyleScope,
  PluginTransitionPreset,
  PluginWallpaperSpec,
} from '../../../plugins/api'
import { safeThemeColor, safeThemeKey, safeThemeToken } from './pluginThemeContext.ts'

const MAX_TOTAL_EMITTERS = 24
const MAX_PLUGIN_EMITTERS = 8
const MAX_BURSTS = 16
const MAX_PLUGIN_SOUNDS = 12
const MAX_PLUGIN_FONTS = 4
const MAX_PLUGIN_SLOTS = 3
const MAX_PLUGIN_STYLES = 7
const MAX_PLUGIN_HUD = 8
const MAX_PLUGIN_TOKENS = 64
const MAX_DATA_URL = 900_000
const MAX_WALLPAPER_DATA_URL = 1_800_000
const MAX_FONT_DATA_URL = 2_400_000
const MAX_VIEW_JSON = 24_000

const SLOTS: readonly PluginSlotName[] = ['sidebar.footer', 'instances.top', 'settings.footer']
const STYLE_SCOPES: readonly PluginStyleScope[] = ['window', 'sidebar', 'cards', 'buttons', 'inputs', 'header', 'scrollbars']
const HUD_POSITIONS: readonly PluginHudPosition[] = ['top-left', 'top-right', 'bottom-left', 'bottom-right', 'top-center', 'bottom-center']
const TRANSITIONS: readonly PluginTransitionPreset[] = ['instant', 'swift', 'smooth', 'cinematic']

export interface PluginParticleRuntime {
  id: string
  pluginId: string
  shape: string
  image?: string
  color: string
  colors: string[]
  count: number
  size: number
  sizeVariance: number
  speed: number
  speedVariance: number
  direction: number
  spread: number
  gravity: number
  wind: number
  sway: number
  swaySpeed: number
  opacity: number
  opacityVariance: number
  lifetime: number
  blend: PluginParticleBlend
  zone: PluginParticleZone
  layer: PluginParticleLayer
  respawn: boolean
  label: string
}

export interface PluginParticleBurstRuntime {
  id: string
  pluginId: string
  x: number
  y: number
  shape: string
  image?: string
  color: string
  colors: string[]
  count: number
  speed: number
  size: number
  sizeVariance: number
  spread: number
  gravity: number
  lifetime: number
  blend: PluginParticleBlend
  layer: PluginParticleLayer
  label: string
  createdAt: number
}

export interface PluginStyleRuntime {
  pluginId: string
  scope: PluginStyleScope
  css: Record<string, string>
}

export interface PluginSlotRuntime {
  pluginId: string
  slot: PluginSlotName
  view: Record<string, unknown>
}

export interface PluginSoundRuntime {
  pluginId: string
  name: string
  audio: string
}

export interface PluginCursorRuntime {
  pluginId: string
  image: string
  hotspotX: number
  hotspotY: number
  size: number
}

export interface PluginWallpaperRuntime {
  pluginId: string
  image: string
  blur: number
  dim: number
  fit: string
}

export interface PluginEffectRuntime {
  pluginId: string
  spec: {
    panelGlow?: string
    panelBlur?: number
    gradient?: { from: string; to: string; angle: number }
    vignette?: number
    noise?: number
    scanlines?: boolean
    shimmer?: boolean
  }
}

export interface PluginHudRuntime {
  pluginId: string
  id: string
  view: Record<string, unknown>
  position: PluginHudPosition
  width: number
  margin: number
}

export interface PluginFontRuntime {
  pluginId: string
  name: string
  family: string
  font: string
}

export interface PluginTokenRuntime {
  pluginId: string
  vars: Record<string, string | number>
}

const PARTICLE_PRESETS: Record<string, Partial<PluginParticleRuntime>> = {
  snow: { shape: 'circle', color: '#ffffff', count: 140, size: 3.5, sizeVariance: 0.6, speed: 24, direction: 90, spread: 12, sway: 16, swaySpeed: 0.4, gravity: 3, opacity: 0.85, opacityVariance: 0.2, zone: 'full', layer: 'behind' },
  rain: { shape: 'circle', color: '#9fd0ff', count: 160, size: 2, sizeVariance: 0.2, speed: 320, direction: 92, spread: 4, sway: 2, gravity: 120, opacity: 0.5, opacityVariance: 0.1, zone: 'full', layer: 'behind' },
  embers: { shape: 'circle', color: '#ff9d45', count: 60, size: 3, sizeVariance: 0.5, speed: 34, direction: 270, spread: 26, sway: 10, gravity: -22, opacity: 0.8, opacityVariance: 0.3, lifetime: 6, blend: 'lighter', zone: 'bottom', layer: 'behind' },
  bubbles: { shape: 'circle', color: '#bfe8ff', count: 40, size: 6, sizeVariance: 0.6, speed: 18, direction: 270, spread: 16, sway: 18, gravity: -6, opacity: 0.35, opacityVariance: 0.2, zone: 'bottom', layer: 'behind' },
  stars: { shape: 'circle', color: '#ffffff', count: 70, size: 2, sizeVariance: 0.4, speed: 0.4, direction: 90, spread: 180, sway: 0, opacity: 0.9, opacityVariance: 0.35, zone: 'top', layer: 'behind' },
  fireflies: { shape: 'circle', color: '#ffe98a', count: 26, size: 3, sizeVariance: 0.4, speed: 8, direction: 90, spread: 180, sway: 22, swaySpeed: 0.3, gravity: 0, opacity: 0.9, opacityVariance: 0.4, blend: 'lighter', zone: 'full', layer: 'above' },
  confetti: { shape: 'square', color: '#ffffff', colors: ['#ff5d5d', '#ffd166', '#7bd88f', '#5dc8ff', '#c084fc'], count: 90, size: 6, sizeVariance: 0.6, speed: 70, direction: 90, spread: 24, sway: 12, gravity: 28, opacity: 0.95, opacityVariance: 0.1, lifetime: 8, zone: 'top', layer: 'above' },
  sparks: { shape: 'circle', color: '#ffd166', count: 50, size: 2, sizeVariance: 0.4, speed: 60, direction: 270, spread: 60, sway: 4, gravity: 60, opacity: 0.9, opacityVariance: 0.3, lifetime: 3, blend: 'lighter', zone: 'bottom', layer: 'above' },
  dust: { shape: 'circle', color: '#ffffff', count: 50, size: 1.6, sizeVariance: 0.5, speed: 5, direction: 90, spread: 180, sway: 6, swaySpeed: 0.2, gravity: 0, opacity: 0.25, opacityVariance: 0.15, zone: 'full', layer: 'behind' },
  hearts: { shape: 'heart', color: '#ff6b8a', count: 22, size: 8, sizeVariance: 0.4, speed: 16, direction: 270, spread: 24, sway: 12, gravity: -8, opacity: 0.8, opacityVariance: 0.2, zone: 'full', layer: 'above' },
}

const DEFAULT_PARTICLE: PluginParticleRuntime = {
  id: '',
  pluginId: '',
  shape: 'circle',
  color: '#ffffff',
  colors: [],
  count: 80,
  size: 4,
  sizeVariance: 1,
  speed: 30,
  speedVariance: 0.4,
  direction: 90,
  spread: 20,
  gravity: 0,
  wind: 0,
  sway: 0,
  swaySpeed: 0.6,
  opacity: 0.9,
  opacityVariance: 0.25,
  lifetime: 0,
  blend: 'normal',
  zone: 'full',
  layer: 'behind',
  respawn: true,
  label: '',
}

const STYLE_PROPERTIES: Record<string, (value: string) => string | null> = {
  'background': colorLikeValue,
  'background-color': colorLikeValue,
  'color': colorLikeValue,
  'border-color': colorLikeValue,
  'border-radius': lengthListValue,
  'border-width': lengthListValue,
  'padding': lengthListValue,
  'gap': lengthListValue,
  'font-size': lengthListValue,
  'letter-spacing': lengthListValue,
  'opacity': opacityValue,
  'box-shadow': boxShadowValue,
  'backdrop-filter': backdropFilterValue,
  'font-family': fontFamilyValue,
  'background-image': backgroundImageValue,
  'background-size': sizePositionValue,
  'background-position': sizePositionValue,
}

const SCOPE_SELECTORS: Record<PluginStyleScope, string> = {
  window: '.app-shell',
  sidebar: '.sidebar, .sidebar-footer',
  cards: '.panel',
  buttons: '.button, .icon-button',
  inputs: '.text-input, .field input, input[type="text"], input[type="password"], input[type="number"], textarea, select',
  header: '.topbar',
  scrollbars: '.app-shell *::-webkit-scrollbar-thumb, .app-shell *::-webkit-scrollbar-track',
}

const KNOWN_FONT_FAMILIES = /^(system|inter|manrope|mono)$/i

const state = reactive({
  emitters: [] as PluginParticleRuntime[],
  bursts: [] as PluginParticleBurstRuntime[],
  styles: [] as PluginStyleRuntime[],
  slots: [] as PluginSlotRuntime[],
  sounds: [] as PluginSoundRuntime[],
  cursor: null as PluginCursorRuntime | null,
  wallpapers: [] as PluginWallpaperRuntime[],
  effects: [] as PluginEffectRuntime[],
  hud: [] as PluginHudRuntime[],
  transitions: null as { pluginId: string; preset: PluginTransitionPreset } | null,
  fonts: [] as PluginFontRuntime[],
  tokens: [] as PluginTokenRuntime[],
})

let nextId = 1
const nextIdValue = (prefix: string) => `${prefix}-${nextId++}`
const audioElements = new Map<string, HTMLAudioElement>()

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function stringArg(value: unknown, max: number, label: string, pattern?: RegExp): string {
  const text = typeof value === 'string' ? value.trim() : ''
  if (!text || text.length > max || (pattern && !pattern.test(text))) throw new TypeError(`${label} must be a short string`)
  return text
}

function optionalStringArg(value: unknown, max: number, label: string, pattern?: RegExp): string | undefined {
  if (value === undefined || value === null || value === '') return undefined
  return stringArg(value, max, label, pattern)
}

function boundedNumber(value: unknown, min: number, max: number, fallback: number): number {
  const numeric = Number(value)
  return Number.isFinite(numeric) ? Math.max(min, Math.min(max, numeric)) : fallback
}

function dataUrlArg(value: unknown, mimes: RegExp, max: number, label: string): string {
  if (typeof value !== 'string' || value.length > max) throw new TypeError(`${label} must be a base64 data URL up to ${max} characters`)
  if (!/^data:([a-z0-9.+-]+\/[a-z0-9.+-]+);base64,[a-z0-9+/=\s]+$/i.test(value) || !mimes.test(value)) {
    throw new TypeError(`${label} must use an allowed data URL media type`)
  }
  return value
}

function colorArg(value: unknown, label: string): string {
  const color = safeThemeColor(value)
  if (!color) throw new TypeError(`${label} must be an #rgb or #rrggbb hex color`)
  return color
}

function optionalColorArg(value: unknown, label: string): string | undefined {
  if (value === undefined || value === null || value === '') return undefined
  return colorArg(value, label)
}

function colorLikeValue(value: string): string | null {
  if (value === 'transparent' || value === 'none') return value
  if (safeThemeColor(value)) return value.toLowerCase()
  const gradient = /^linear-gradient\((-?\d+(\.\d+)?)deg(\s*,\s*#[0-9a-fA-F]{3,8}){2,3}\)$/.exec(value)
  if (gradient) return value
  return null
}

function lengthListValue(value: string): string | null {
  if (!/^(-?\d+(\.\d+)?(px|rem|em|%))(\s+-?\d+(\.\d+)?(px|rem|em|%)){0,3}$/.test(value)) return null
  return value
}

function opacityValue(value: string): string | null {
  const numeric = Number(value)
  return Number.isFinite(numeric) && numeric >= 0 && numeric <= 1 ? String(numeric) : null
}

function boxShadowValue(value: string): string | null {
  if (value === 'none') return value
  if (!/^(-?\d+(\.\d+)?px)(\s+-?\d+(\.\d+)?px)*(\s+#[0-9a-fA-F]{3,8})?$/.test(value)) return null
  return value
}

function backdropFilterValue(value: string): string | null {
  if (value === 'none') return value
  if (!/^blur\((\d+(\.\d+)?)px\)$/.test(value)) return null
  return value
}

function fontFamilyValue(value: string): string | null {
  if (KNOWN_FONT_FAMILIES.test(value)) return value
  if (/^plugin-[a-z0-9-]{1,80}$/.test(value)) return value
  return null
}

function backgroundImageValue(value: string): string | null {
  if (!/^data:image\/(png|jpeg|webp|gif);base64,[a-z0-9+/=\s]+$/i.test(value)) return null
  if (value.length > MAX_DATA_URL) return null
  return `url("${value.replace(/\s+/g, '')}")`
}

function sizePositionValue(value: string): string | null {
  return /^(cover|contain|center|top|bottom|left|right|\d+(\.\d+)?%?)(\s+(cover|contain|center|top|bottom|left|right|\d+(\.\d+)?%?))?$/.test(value) ? value : null
}

function stylePatchArg(value: unknown, label: string): Record<string, string> {
  if (!isRecord(value)) throw new TypeError(`${label} must be an object of CSS values`)
  const keys = Object.keys(value)
  if (keys.length > 24) throw new TypeError(`${label} supports up to 24 CSS properties`)
  const result: Record<string, string> = {}
  for (const key of keys) {
    if (!Object.prototype.hasOwnProperty.call(STYLE_PROPERTIES, key)) throw new TypeError(`Unsupported theme style property: ${key}`)
    const raw = value[key]
    if (raw === undefined || raw === null) continue
    if (typeof raw !== 'string' && typeof raw !== 'number') throw new TypeError(`Style value for ${key} must be a string or number`)
    const normalized = String(raw).trim()
    const validated = STYLE_PROPERTIES[key](normalized)
    if (!validated) throw new TypeError(`Invalid value for theme style property ${key}`)
    result[key] = validated
  }
  if (!Object.keys(result).length) throw new TypeError(`${label} must set at least one CSS property`)
  return result
}

function viewNodeArg(value: unknown, label: string): Record<string, unknown> {
  if (!isRecord(value)) throw new TypeError(`${label} must be a UI node object`)
  const type = value.type ?? value.kind
  if (typeof type !== 'string' || type.length > 32 || !/^[a-z][a-z0-9_-]*$/i.test(type)) {
    throw new TypeError(`${label}.type must be a short node type string`)
  }
  if (value.id !== undefined && (typeof value.id !== 'string' || value.id.length > 96)) throw new TypeError(`${label}.id must be a short string`)
  const json = JSON.stringify(value)
  if (!json || json.length > MAX_VIEW_JSON) throw new TypeError(`${label} is too large`)
  assertViewDepth(value, 0)
  return value
}

function assertViewDepth(node: unknown, depth: number): void {
  if (depth > 8) throw new TypeError('UI node tree is nested too deep')
  if (!isRecord(node)) return
  const children = node.children
  if (Array.isArray(children)) {
    if (children.length > 40) throw new TypeError('UI node has too many children')
    for (const child of children) assertViewDepth(child, depth + 1)
  }
}

function normalizeParticleSpec(pluginId: string, raw: unknown): PluginParticleRuntime {
  if (!isRecord(raw)) throw new TypeError('Particle spec must be an object')
  const preset = optionalStringArg(raw.preset, 24, 'Particle preset')
  const base: PluginParticleRuntime = { ...DEFAULT_PARTICLE, pluginId, ...(preset ? (PARTICLE_PRESETS[preset] || {}) : {}) }
  if (preset && !PARTICLE_PRESETS[preset]) throw new TypeError(`Unknown particle preset: ${preset}`)
  const image = optionalDataUrl(raw.image)
  const shape = optionalStringArg(raw.shape, 16, 'Particle shape', /^(circle|square|star|heart|diamond)$/)
  const colorsRaw = Array.isArray(raw.colors) ? raw.colors : []
  const colors = colorsRaw.slice(0, 8).map((entry) => colorArg(entry, 'Particle color'))
  return {
    ...base,
    shape: image ? 'image' : (shape || base.shape),
    image: image || undefined,
    color: optionalColorArg(raw.color, 'Particle color') || (colors[0] || base.color),
    colors: colors.length ? colors : (Array.isArray(base.colors) ? base.colors.slice() : []),
    count: boundedNumber(raw.count, 1, 400, base.count),
    size: boundedNumber(raw.size, 1, 64, base.size),
    sizeVariance: boundedNumber(raw.sizeVariance, 0, 4, base.sizeVariance),
    speed: boundedNumber(raw.speed, 0, 1200, base.speed),
    speedVariance: boundedNumber(raw.speedVariance, 0, 1, base.speedVariance),
    direction: boundedNumber(raw.direction, 0, 360, base.direction),
    spread: boundedNumber(raw.spread, 0, 360, base.spread),
    gravity: boundedNumber(raw.gravity, -400, 400, base.gravity),
    wind: boundedNumber(raw.wind, -300, 300, base.wind),
    sway: boundedNumber(raw.sway, 0, 80, base.sway),
    swaySpeed: boundedNumber(raw.swaySpeed, 0, 4, base.swaySpeed),
    opacity: boundedNumber(raw.opacity, 0.02, 1, base.opacity),
    opacityVariance: boundedNumber(raw.opacityVariance, 0, 0.8, base.opacityVariance),
    lifetime: boundedNumber(raw.lifetime, 0, 300, base.lifetime),
    blend: optionalStringArg(raw.blend, 12, 'Particle blend', /^(normal|lighter)$/) as PluginParticleBlend || base.blend,
    zone: optionalStringArg(raw.zone, 12, 'Particle zone', /^(full|top|bottom|left|right|center)$/) as PluginParticleZone || base.zone,
    layer: optionalStringArg(raw.layer, 8, 'Particle layer', /^(behind|above)$/) as PluginParticleLayer || base.layer,
    respawn: typeof raw.respawn === 'boolean' ? raw.respawn : base.respawn,
    label: optionalStringArg(raw.label, 64, 'Particle label') || '',
  }
}

function normalizeBurstSpec(pluginId: string, raw: unknown): PluginParticleBurstRuntime {
  if (!isRecord(raw)) throw new TypeError('Particle burst spec must be an object')
  const x = Number(raw.x)
  const y = Number(raw.y)
  if (!Number.isFinite(x) || x < 0 || x > 1 || !Number.isFinite(y) || y < 0 || y > 1) {
    throw new TypeError('Particle burst x and y must be fractions between 0 and 1')
  }
  const image = optionalDataUrl(raw.image)
  const shape = optionalStringArg(raw.shape, 16, 'Particle shape', /^(circle|square|star|heart|diamond)$/)
  const colorsRaw = Array.isArray(raw.colors) ? raw.colors : []
  const colors = colorsRaw.slice(0, 8).map((entry) => colorArg(entry, 'Particle color'))
  const color = optionalColorArg(raw.color, 'Particle color') || colors[0] || '#ffd166'
  return {
    id: nextIdValue('burst'),
    pluginId,
    x,
    y,
    shape: image ? 'image' : (shape || 'circle'),
    image: image || undefined,
    color,
    colors: colors.length ? colors : [color],
    count: boundedNumber(raw.count, 1, 200, 40),
    speed: boundedNumber(raw.speed, 10, 1200, 160),
    size: boundedNumber(raw.size, 1, 64, 4),
    sizeVariance: boundedNumber(raw.sizeVariance, 0, 4, 0.5),
    spread: boundedNumber(raw.spread, 0, 360, 360),
    gravity: boundedNumber(raw.gravity, -400, 400, 220),
    lifetime: boundedNumber(raw.lifetime, 0.2, 12, 1.6),
    blend: optionalStringArg(raw.blend, 12, 'Particle blend', /^(normal|lighter)$/) as PluginParticleBlend || 'normal',
    layer: optionalStringArg(raw.layer, 8, 'Particle layer', /^(behind|above)$/) as PluginParticleLayer || 'above',
    label: optionalStringArg(raw.label, 64, 'Particle label') || '',
    createdAt: Date.now(),
  }
}

function optionalDataUrl(value: unknown): string | undefined {
  if (value === undefined || value === null || value === '') return undefined
  return dataUrlArg(value, /^data:image\/(png|jpeg|webp|gif);base64,/i, MAX_DATA_URL, 'Particle image')
}

function cursorSpecArg(raw: unknown): { image: string; hotspotX: number; hotspotY: number; size: number } {
  if (!isRecord(raw)) throw new TypeError('Cursor spec must be an object')
  const image = dataUrlArg(raw.image, /^data:image\/(png|gif|webp);base64,/i, 400_000, 'Cursor image')
  const size = Math.round(boundedNumber(raw.size, 16, 128, 32))
  return {
    image,
    hotspotX: Math.round(boundedNumber(raw.hotspotX, 0, size, 0)),
    hotspotY: Math.round(boundedNumber(raw.hotspotY, 0, size, 0)),
    size,
  }
}

function wallpaperSpecArg(raw: unknown): { image: string; blur: number; dim: number; fit: string } {
  if (!isRecord(raw)) throw new TypeError('Wallpaper spec must be an object')
  const image = dataUrlArg(raw.image, /^data:image\/(png|jpeg|webp|gif);base64,/i, MAX_WALLPAPER_DATA_URL, 'Wallpaper image')
  const fit = optionalStringArg(raw.fit, 8, 'Wallpaper fit', /^(cover|contain|tile)$/) || 'cover'
  return {
    image,
    blur: boundedNumber(raw.blur, 0, 40, 0),
    dim: boundedNumber(raw.dim, 0, 0.9, 0),
    fit,
  }
}

function effectSpecArg(raw: unknown): PluginEffectRuntime['spec'] {
  if (!isRecord(raw)) throw new TypeError('Effects spec must be an object')
  const spec: PluginEffectRuntime['spec'] = {}
  const panelGlow = optionalColorArg(raw.panelGlow, 'Panel glow color')
  if (panelGlow) spec.panelGlow = panelGlow
  if (raw.panelBlur !== undefined && raw.panelBlur !== null) spec.panelBlur = boundedNumber(raw.panelBlur, 0, 24, 0)
  if (raw.gradient !== undefined && raw.gradient !== null) {
    if (!isRecord(raw.gradient)) throw new TypeError('Gradient must be an object')
    spec.gradient = {
      from: colorArg(raw.gradient.from, 'Gradient from'),
      to: colorArg(raw.gradient.to, 'Gradient to'),
      angle: boundedNumber(raw.gradient.angle, 0, 360, 160),
    }
  }
  if (raw.vignette !== undefined && raw.vignette !== null) spec.vignette = boundedNumber(raw.vignette, 0, 1, 0)
  if (raw.noise !== undefined && raw.noise !== null) spec.noise = boundedNumber(raw.noise, 0, 1, 0)
  if (raw.scanlines !== undefined && raw.scanlines !== null) {
    if (typeof raw.scanlines !== 'boolean') throw new TypeError('Scanlines must be a boolean')
    spec.scanlines = raw.scanlines
  }
  if (raw.shimmer !== undefined && raw.shimmer !== null) {
    if (typeof raw.shimmer !== 'boolean') throw new TypeError('Shimmer must be a boolean')
    spec.shimmer = raw.shimmer
  }
  if (!Object.keys(spec).length) throw new TypeError('Effects spec must set at least one effect')
  return spec
}

function hudOptionsArg(raw: unknown): { position: PluginHudPosition; width: number; margin: number } {
  if (raw === undefined || raw === null) return { position: 'top-right', width: 260, margin: 16 }
  if (!isRecord(raw)) throw new TypeError('HUD options must be an object')
  const position = optionalStringArg(raw.position, 16, 'HUD position', /^(top-left|top-right|bottom-left|bottom-right|top-center|bottom-center)$/) as PluginHudPosition
  return {
    position: position || 'top-right',
    width: Math.round(boundedNumber(raw.width, 120, 480, 260)),
    margin: Math.round(boundedNumber(raw.margin, 8, 96, 16)),
  }
}

function tokenOverridesArg(raw: unknown): Record<string, string | number> {
  if (!isRecord(raw)) throw new TypeError('Token overrides must be an object')
  const keys = Object.keys(raw)
  if (keys.length > MAX_PLUGIN_TOKENS) throw new TypeError(`Token overrides support up to ${MAX_PLUGIN_TOKENS} entries`)
  const result: Record<string, string | number> = {}
  for (const key of keys) {
    if (!safeThemeKey(key)) throw new TypeError(`Invalid token name: ${key}`)
    const token = safeThemeToken(raw[key])
    if (token === undefined) throw new TypeError(`Invalid token value for ${key}`)
    result[key] = token
  }
  if (!Object.keys(result).length) throw new TypeError('Token overrides must set at least one token')
  return result
}

function pluginCount<T extends { pluginId: string }>(rows: T[], pluginId: string): number {
  return rows.filter((row) => row.pluginId === pluginId).length
}

function findEmitter(pluginId: string, id: string): PluginParticleRuntime | undefined {
  return state.emitters.find((emitter) => emitter.pluginId === pluginId && emitter.id === id)
}

export function pluginThemeEffectsSnapshot() {
  return {
    emitters: state.emitters.map((emitter) => ({ ...emitter })),
    bursts: state.bursts.map((burst) => ({ ...burst })),
    styles: state.styles.map((style) => ({ ...style, css: { ...style.css } })),
    slots: state.slots.map((slot) => ({ ...slot })),
    sounds: state.sounds.map((sound) => ({ ...sound })),
    cursor: state.cursor ? { ...state.cursor } : null,
    wallpapers: state.wallpapers.map((wallpaper) => ({ ...wallpaper })),
    effects: state.effects.map((effect) => ({ ...effect, spec: { ...effect.spec } })),
    hud: state.hud.map((widget) => ({ ...widget })),
    transitions: state.transitions ? { ...state.transitions } : null,
    fonts: state.fonts.map((font) => ({ ...font })),
    tokens: state.tokens.map((entry) => ({ ...entry, vars: { ...entry.vars } })),
  }
}

function spawnEmitter(runtime: PluginParticleRuntime): { id: string } {
  if (pluginCount(state.emitters, runtime.pluginId) >= MAX_PLUGIN_EMITTERS) throw new Error(`A plugin may keep up to ${MAX_PLUGIN_EMITTERS} active emitters`)
  if (state.emitters.length >= MAX_TOTAL_EMITTERS) throw new Error(`The launcher keeps up to ${MAX_TOTAL_EMITTERS} active emitters across plugins`)
  runtime.id = nextIdValue('emitter')
  state.emitters.push(runtime)
  return { id: runtime.id }
}

function playSound(pluginId: string, name: string, options: PluginSoundOptions): void {
  const sound = state.sounds.find((entry) => entry.pluginId === pluginId && entry.name === name)
  if (!sound) throw new Error(`No plugin sound is registered as ${name}`)
  if (typeof document === 'undefined' || typeof Audio === 'undefined') return
  const key = `${pluginId}:${name}`
  let element = audioElements.get(key)
  if (!element || !options.loop) {
    element = new Audio(sound.audio)
    audioElements.set(key, element)
  }
  element.volume = boundedNumber(options.volume, 0, 1, 1)
  element.loop = options.loop === true
  element.playbackRate = boundedNumber(options.rate, 0.25, 4, 1)
  const playback = element.play()
  if (playback && typeof playback.catch === 'function') playback.catch(() => {})
  if (!element.loop) {
    element.addEventListener('ended', () => audioElements.delete(key), { once: true })
  }
}

function registerFont(pluginId: string, name: string, font: string): { family: string } {
  const slug = (value: string) => value.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '') || 'font'
  const family = `plugin-${slug(pluginId).slice(0, 40)}-${slug(name).slice(0, 40)}`
  const record: PluginFontRuntime = { pluginId, name, family, font }
  if (typeof document !== 'undefined' && typeof FontFace !== 'undefined') {
    const base64 = font.slice(font.indexOf(',') + 1).replace(/\s+/g, '')
    const bytes = Uint8Array.from(atob(base64), (char) => char.charCodeAt(0))
    const face = new FontFace(family, bytes.buffer)
    const loaded = face.load()
    if (loaded && typeof loaded.then === 'function') {
      loaded.then(() => {
        try { document.fonts.add(face) } catch { /* браузер может отказать в регистрации шрифта */ }
      }).catch(() => {})
    }
  }
  state.fonts.push(record)
  return { family }
}

function unregisterFont(pluginId: string, name: string): boolean {
  const index = state.fonts.findIndex((entry) => entry.pluginId === pluginId && entry.name === name)
  if (index < 0) return false
  const [removed] = state.fonts.splice(index, 1)
  if (typeof document !== 'undefined' && document.fonts) {
    try { document.fonts.delete(new FontFace(removed.family, new ArrayBuffer(0))) } catch { /* шрифт мог не загрузиться */ }
  }
  return true
}

function effectiveCursor(): PluginCursorRuntime | null {
  return state.cursor
}

function effectiveWallpaper(): PluginWallpaperRuntime | null {
  return state.wallpapers.length ? state.wallpapers[state.wallpapers.length - 1] : null
}

function effectiveEffects(): PluginEffectRuntime | null {
  return state.effects.length ? state.effects[state.effects.length - 1] : null
}

function effectiveTransition(): { pluginId: string; preset: PluginTransitionPreset } | null {
  return state.transitions
}

function effectiveStyles(): PluginStyleRuntime[] {
  return state.styles
}

function effectiveTokens(): PluginTokenRuntime[] {
  return state.tokens
}

function mergeEffectVars(effects: PluginEffectRuntime | null): Record<string, string> {
  const vars: Record<string, string> = {}
  if (!effects) return vars
  const spec = effects.spec
  if (spec.panelGlow) vars['--plugin-panel-glow'] = `0 0 18px ${spec.panelGlow}`
  if (spec.panelBlur !== undefined) vars['--plugin-panel-blur'] = `${spec.panelBlur}px`
  if (spec.gradient) vars['--plugin-gradient'] = `linear-gradient(${spec.gradient.angle}deg, ${spec.gradient.from}, ${spec.gradient.to})`
  return vars
}

function effectCss(effects: PluginEffectRuntime | null): string {
  if (!effects) return ''
  const rules: string[] = []
  const spec = effects.spec
  if (spec.panelGlow) rules.push(`.panel { box-shadow: var(--plugin-panel-glow, none) !important; }`)
  if (spec.panelBlur !== undefined) {
    rules.push(`.panel { backdrop-filter: blur(${spec.panelBlur}px) !important; }`)
  }
  if (spec.gradient) {
    rules.push(`.app-shell { background-image: var(--plugin-gradient, none) !important; background-attachment: fixed !important; }`)
  }
  if (spec.shimmer) {
    rules.push('@keyframes plugin-effect-shimmer { 0% { background-position: -180% 0; } 100% { background-position: 180% 0; } }')
    rules.push('.app-shell::after { content: ""; position: fixed; inset: 0; pointer-events: none; background: linear-gradient(105deg, transparent 40%, rgba(255,255,255,0.06) 50%, transparent 60%); background-size: 220% 100%; animation: plugin-effect-shimmer 6s linear infinite; z-index: 2; }')
  }
  return rules.join('\n')
}

function styleCss(styles: PluginStyleRuntime[]): string {
  const parts: string[] = []
  for (const style of styles) {
    const selector = SCOPE_SELECTORS[style.scope]
    if (!selector) continue
    const declarations = Object.entries(style.css).map(([property, value]) => `  ${property}: ${value} !important;`)
    parts.push(`${selector} {\n${declarations.join('\n')}\n}`)
  }
  return parts.join('\n\n')
}

function transitionCss(preset: PluginTransitionPreset | null): string {
  if (!preset || preset === 'instant') return ''
  const durations: Record<Exclude<PluginTransitionPreset, 'instant'>, string> = { swift: '90ms', smooth: '170ms', cinematic: '320ms' }
  const timing = preset === 'cinematic' ? 'cubic-bezier(0.22, 1, 0.36, 1)' : 'ease'
  return `.app-shell * { transition-duration: ${durations[preset]} !important; transition-timing-function: ${timing} !important; }`
}

function tokenVars(tokens: PluginTokenRuntime[]): Record<string, string> {
  const vars: Record<string, string> = {}
  for (const entry of tokens) {
    for (const [key, value] of Object.entries(entry.vars)) {
      vars[`--plugin-token-${key}`] = String(value)
    }
  }
  return vars
}

function applyStyleVars(vars: Record<string, string>): void {
  if (typeof document === 'undefined') return
  const root = document.documentElement
  for (const [key, value] of Object.entries(vars)) root.style.setProperty(key, value)
}

function syncInjectedStyles(): void {
  if (typeof document === 'undefined') return
  let style = document.getElementById('plugin-theme-style-overrides') as HTMLStyleElement | null
  const css = [effectCss(effectiveEffects()), styleCss(state.styles), transitionCss(state.transitions ? state.transitions.preset : null)].filter(Boolean).join('\n\n')
  if (!css) {
    if (style) style.remove()
    return
  }
  if (!style) {
    style = document.createElement('style')
    style.id = 'plugin-theme-style-overrides'
    document.head.appendChild(style)
  }
  style.textContent = css
}

function syncCursor(): void {
  if (typeof document === 'undefined') return
  const cursor = effectiveCursor()
  const body = document.body
  if (!body) return
  if (!cursor) {
    if (body.dataset.pluginCursorApplied === '1') {
      body.style.removeProperty('cursor')
      document.documentElement.style.removeProperty('cursor')
      delete body.dataset.pluginCursorApplied
    }
    return
  }
  const value = `url("${cursor.image}") ${cursor.hotspotX} ${cursor.hotspotY}, auto`
  body.style.setProperty('cursor', value, 'important')
  document.documentElement.style.setProperty('cursor', value, 'important')
  body.dataset.pluginCursorApplied = '1'
}

export const pluginThemeEffects = {
  particlesSpawn(pluginId: string, args: Record<string, unknown>): { id: string } {
    return spawnEmitter(normalizeParticleSpec(pluginId, args.spec))
  },
  particlesBurst(pluginId: string, args: Record<string, unknown>): { id: string } {
    const burst = normalizeBurstSpec(pluginId, args.spec)
    if (state.bursts.length >= MAX_BURSTS) state.bursts.shift()
    state.bursts.push(burst)
    return { id: burst.id }
  },
  particlesStop(pluginId: string, id: string): boolean {
    const index = state.emitters.findIndex((emitter) => emitter.pluginId === pluginId && emitter.id === id)
    if (index < 0) return false
    state.emitters.splice(index, 1)
    return true
  },
  particlesStopAll(pluginId: string): number {
    const before = state.emitters.length
    state.emitters = state.emitters.filter((emitter) => emitter.pluginId !== pluginId) as never
    return before - state.emitters.length
  },
  particlesActive(pluginId: string): string[] {
    return state.emitters.filter((emitter) => emitter.pluginId === pluginId).map((emitter) => emitter.id)
  },
  particlesRemoveEmitter(id: string): void {
    const index = state.emitters.findIndex((emitter) => emitter.id === id)
    if (index >= 0) state.emitters.splice(index, 1)
  },
  particlesRemoveBurst(id: string): void {
    const index = state.bursts.findIndex((burst) => burst.id === id)
    if (index >= 0) state.bursts.splice(index, 1)
  },
  styleSet(pluginId: string, args: Record<string, unknown>): void {
    const scope = stringArg(args.scope, 24, 'Style scope', new RegExp(`^(${STYLE_SCOPES.join('|')})$`)) as PluginStyleScope
    const css = stylePatchArg(args.patch, 'Style patch')
    const existing = state.styles.find((style) => style.pluginId === pluginId && style.scope === scope)
    if (existing) existing.css = css
    else {
      if (pluginCount(state.styles, pluginId) >= MAX_PLUGIN_STYLES) throw new Error(`A plugin may restyle up to ${MAX_PLUGIN_STYLES} scopes`)
      state.styles.push({ pluginId, scope, css })
    }
    syncInjectedStyles()
  },
  styleReset(pluginId: string, args: Record<string, unknown>): void {
    const scope = optionalStringArg(args.scope, 24, 'Style scope', new RegExp(`^(${STYLE_SCOPES.join('|')})$`))
    state.styles = state.styles.filter((style) => style.pluginId !== pluginId || (scope !== undefined && style.scope !== scope)) as never
    syncInjectedStyles()
  },
  replaceSet(pluginId: string, args: Record<string, unknown>): void {
    const slot = stringArg(args.slot, 32, 'Slot name', new RegExp(`^(${SLOTS.join('|').replace(/\./g, '\\.')})$`)) as PluginSlotName
    const view = viewNodeArg(args.view, 'Replacement view')
    const existing = state.slots.find((entry) => entry.pluginId === pluginId && entry.slot === slot)
    if (existing) existing.view = view
    else {
      if (pluginCount(state.slots, pluginId) >= MAX_PLUGIN_SLOTS) throw new Error(`A plugin may replace up to ${MAX_PLUGIN_SLOTS} slots`)
      state.slots.push({ pluginId, slot, view })
    }
  },
  replaceReset(pluginId: string, args: Record<string, unknown>): void {
    const slot = optionalStringArg(args.slot, 32, 'Slot name', new RegExp(`^(${SLOTS.join('|').replace(/\./g, '\\.')})$`)) as PluginSlotName | undefined
    state.slots = state.slots.filter((entry) => entry.pluginId !== pluginId || (slot !== undefined && entry.slot !== slot)) as never
  },
  soundsRegister(pluginId: string, args: Record<string, unknown>): void {
    const name = stringArg(args.name, 64, 'Sound name', /^[a-z][a-z0-9_-]{0,63}$/i)
    const audio = dataUrlArg(args.audio, /^data:audio\/(mpeg|ogg|wav|x-wav|webm|mp4);base64,/i, MAX_DATA_URL, 'Sound audio')
    const existing = state.sounds.find((sound) => sound.pluginId === pluginId && sound.name === name)
    if (existing) existing.audio = audio
    else {
      if (pluginCount(state.sounds, pluginId) >= MAX_PLUGIN_SOUNDS) throw new Error(`A plugin may register up to ${MAX_PLUGIN_SOUNDS} sounds`)
      state.sounds.push({ pluginId, name, audio })
    }
  },
  soundsPlay(pluginId: string, args: Record<string, unknown>): void {
    const name = stringArg(args.name, 64, 'Sound name', /^[a-z][a-z0-9_-]{0,63}$/i)
    const options = isRecord(args.options) ? args.options as PluginSoundOptions : {}
    playSound(pluginId, name, options)
  },
  soundsList(pluginId: string): string[] {
    return state.sounds.filter((sound) => sound.pluginId === pluginId).map((sound) => sound.name)
  },
  soundsUnregister(pluginId: string, args: Record<string, unknown>): boolean {
    const name = stringArg(args.name, 64, 'Sound name', /^[a-z][a-z0-9_-]{0,63}$/i)
    const index = state.sounds.findIndex((sound) => sound.pluginId === pluginId && sound.name === name)
    if (index < 0) return false
    state.sounds.splice(index, 1)
    const key = `${pluginId}:${name}`
    const element = audioElements.get(key)
    if (element) {
      element.pause()
      audioElements.delete(key)
    }
    return true
  },
  cursorSet(pluginId: string, args: Record<string, unknown>): void {
    state.cursor = { pluginId, ...cursorSpecArg(args.spec) }
    syncCursor()
  },
  cursorReset(pluginId: string): void {
    if (state.cursor && state.cursor.pluginId !== pluginId) return
    state.cursor = null
    syncCursor()
  },
  wallpaperSet(pluginId: string, args: Record<string, unknown>): void {
    const spec = wallpaperSpecArg(args.spec)
    const existing = state.wallpapers.find((entry) => entry.pluginId === pluginId)
    if (existing) Object.assign(existing, spec)
    else state.wallpapers.push({ pluginId, ...spec })
  },
  wallpaperReset(pluginId: string): void {
    state.wallpapers = state.wallpapers.filter((entry) => entry.pluginId !== pluginId) as never
  },
  effectsSet(pluginId: string, args: Record<string, unknown>): void {
    const spec = effectSpecArg(args.spec)
    const existing = state.effects.find((entry) => entry.pluginId === pluginId)
    if (existing) existing.spec = spec
    else state.effects.push({ pluginId, spec })
    applyStyleVars(mergeEffectVars(effectiveEffects()))
    syncInjectedStyles()
  },
  effectsReset(pluginId: string): void {
    state.effects = state.effects.filter((entry) => entry.pluginId !== pluginId) as never
    applyStyleVars(mergeEffectVars(effectiveEffects()))
    syncInjectedStyles()
  },
  hudAdd(pluginId: string, args: Record<string, unknown>): { id: string } {
    const view = viewNodeArg(args.view, 'HUD view')
    const options = hudOptionsArg(args.options)
    if (pluginCount(state.hud, pluginId) >= MAX_PLUGIN_HUD) throw new Error(`A plugin may keep up to ${MAX_PLUGIN_HUD} HUD widgets`)
    const id = nextIdValue('hud')
    state.hud.push({ pluginId, id, view, position: options.position, width: options.width, margin: options.margin })
    return { id }
  },
  hudRemove(pluginId: string, args: Record<string, unknown>): boolean {
    const id = stringArg(args.id, 96, 'HUD widget id')
    const index = state.hud.findIndex((widget) => widget.pluginId === pluginId && widget.id === id)
    if (index < 0) return false
    state.hud.splice(index, 1)
    return true
  },
  hudList(pluginId: string): Array<{ id: string; position: PluginHudPosition }> {
    return state.hud.filter((widget) => widget.pluginId === pluginId).map((widget) => ({ id: widget.id, position: widget.position }))
  },
  transitionsSet(pluginId: string, args: Record<string, unknown>): void {
    const preset = stringArg(args.preset, 16, 'Transition preset', new RegExp(`^(${TRANSITIONS.join('|')})$`)) as PluginTransitionPreset
    state.transitions = { pluginId, preset }
    syncInjectedStyles()
  },
  transitionsReset(pluginId: string): void {
    if (state.transitions && state.transitions.pluginId !== pluginId) return
    state.transitions = null
    syncInjectedStyles()
  },
  fontsRegister(pluginId: string, args: Record<string, unknown>): { family: string } {
    const name = stringArg(args.name, 48, 'Font name', /^[a-z][a-z0-9_-]{0,47}$/i)
    const font = dataUrlArg(args.font, /^data:font\/(woff2?|ttf|otf);base64,/i, MAX_FONT_DATA_URL, 'Font file')
    if (pluginCount(state.fonts, pluginId) >= MAX_PLUGIN_FONTS) throw new Error(`A plugin may register up to ${MAX_PLUGIN_FONTS} fonts`)
    return registerFont(pluginId, name, font)
  },
  fontsUnregister(pluginId: string, args: Record<string, unknown>): boolean {
    return unregisterFont(pluginId, stringArg(args.name, 48, 'Font name', /^[a-z][a-z0-9_-]{0,47}$/i))
  },
  fontsList(pluginId: string): Array<{ name: string; family: string }> {
    return state.fonts.filter((font) => font.pluginId === pluginId).map((font) => ({ name: font.name, family: font.family }))
  },
  tokensSet(pluginId: string, args: Record<string, unknown>): void {
    const vars = tokenOverridesArg(args.overrides)
    const existing = state.tokens.find((entry) => entry.pluginId === pluginId)
    if (existing) existing.vars = vars
    else state.tokens.push({ pluginId, vars })
    applyStyleVars(tokenVars(state.tokens))
  },
  tokensReset(pluginId: string): void {
    state.tokens = state.tokens.filter((entry) => entry.pluginId !== pluginId) as never
    applyStyleVars(tokenVars(state.tokens))
  },
  clearPlugin(pluginId: string): void {
    state.emitters = state.emitters.filter((emitter) => emitter.pluginId !== pluginId) as never
    state.bursts = state.bursts.filter((burst) => burst.pluginId !== pluginId) as never
    state.styles = state.styles.filter((style) => style.pluginId !== pluginId) as never
    state.slots = state.slots.filter((slot) => slot.pluginId !== pluginId) as never
    for (const sound of state.sounds.filter((entry) => entry.pluginId === pluginId)) {
      const element = audioElements.get(`${pluginId}:${sound.name}`)
      if (element) {
        element.pause()
        audioElements.delete(`${pluginId}:${sound.name}`)
      }
    }
    state.sounds = state.sounds.filter((sound) => sound.pluginId !== pluginId) as never
    if (state.cursor && state.cursor.pluginId === pluginId) {
      state.cursor = null
      syncCursor()
    }
    state.wallpapers = state.wallpapers.filter((entry) => entry.pluginId !== pluginId) as never
    state.effects = state.effects.filter((entry) => entry.pluginId !== pluginId) as never
    state.hud = state.hud.filter((widget) => widget.pluginId !== pluginId) as never
    if (state.transitions && state.transitions.pluginId === pluginId) state.transitions = null
    for (const font of state.fonts.filter((entry) => entry.pluginId === pluginId)) {
      if (typeof document !== 'undefined' && document.fonts) {
        try { document.fonts.delete(new FontFace(font.family, new ArrayBuffer(0))) } catch { /* шрифт мог не загрузиться */ }
      }
    }
    state.fonts = state.fonts.filter((font) => font.pluginId !== pluginId) as never
    state.tokens = state.tokens.filter((entry) => entry.pluginId !== pluginId) as never
    applyStyleVars(mergeEffectVars(effectiveEffects()))
    applyStyleVars(tokenVars(state.tokens))
    syncInjectedStyles()
  },
}

export function applyPluginThemeOperation(operation: string, pluginId: string, args: Record<string, unknown>): unknown {
  switch (operation) {
    case 'theme.style.set': return pluginThemeEffects.styleSet(pluginId, args)
    case 'theme.style.reset': return pluginThemeEffects.styleReset(pluginId, args)
    case 'theme.slots.set': return pluginThemeEffects.replaceSet(pluginId, args)
    case 'theme.slots.reset': return pluginThemeEffects.replaceReset(pluginId, args)
    case 'theme.particles.spawn': return pluginThemeEffects.particlesSpawn(pluginId, args)
    case 'theme.particles.burst': return pluginThemeEffects.particlesBurst(pluginId, args)
    case 'theme.particles.stop': return { stopped: pluginThemeEffects.particlesStop(pluginId, stringArg(args.id, 96, 'Emitter id')) }
    case 'theme.particles.stopAll': return { removed: pluginThemeEffects.particlesStopAll(pluginId) }
    case 'theme.particles.active': return { ids: pluginThemeEffects.particlesActive(pluginId) }
    case 'theme.sounds.register': return pluginThemeEffects.soundsRegister(pluginId, args)
    case 'theme.sounds.play': return pluginThemeEffects.soundsPlay(pluginId, args)
    case 'theme.sounds.list': return { names: pluginThemeEffects.soundsList(pluginId) }
    case 'theme.sounds.unregister': return { removed: pluginThemeEffects.soundsUnregister(pluginId, args) }
    case 'theme.cursor.set': return pluginThemeEffects.cursorSet(pluginId, args)
    case 'theme.cursor.reset': return pluginThemeEffects.cursorReset(pluginId)
    case 'theme.wallpaper.set': return pluginThemeEffects.wallpaperSet(pluginId, args)
    case 'theme.wallpaper.reset': return pluginThemeEffects.wallpaperReset(pluginId)
    case 'theme.effects.set': return pluginThemeEffects.effectsSet(pluginId, args)
    case 'theme.effects.reset': return pluginThemeEffects.effectsReset(pluginId)
    case 'theme.hud.add': return pluginThemeEffects.hudAdd(pluginId, args)
    case 'theme.hud.remove': return { removed: pluginThemeEffects.hudRemove(pluginId, args) }
    case 'theme.hud.list': return { widgets: pluginThemeEffects.hudList(pluginId) }
    case 'theme.transitions.set': return pluginThemeEffects.transitionsSet(pluginId, args)
    case 'theme.transitions.reset': return pluginThemeEffects.transitionsReset(pluginId)
    case 'theme.fonts.register': return pluginThemeEffects.fontsRegister(pluginId, args)
    case 'theme.fonts.unregister': return { removed: pluginThemeEffects.fontsUnregister(pluginId, args) }
    case 'theme.fonts.list': return { fonts: pluginThemeEffects.fontsList(pluginId) }
    case 'theme.tokens.set': return pluginThemeEffects.tokensSet(pluginId, args)
    case 'theme.tokens.reset': return pluginThemeEffects.tokensReset(pluginId)
    default: throw new TypeError(`Unsupported theme operation: ${operation}`)
  }
}

export const pluginThemeEffectsStore = state

export const pluginThemeEffectsState = {
  cursor: () => effectiveCursor(),
  wallpaper: () => effectiveWallpaper(),
  effects: () => effectiveEffects(),
  transition: () => effectiveTransition(),
  styles: () => effectiveStyles(),
  tokens: () => effectiveTokens(),
}
