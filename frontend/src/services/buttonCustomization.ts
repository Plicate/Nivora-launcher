export const BUTTON_LABEL_KEYS = ['instances', 'accounts', 'mods', 'modpacks', 'themes', 'settings'] as const

export type ButtonLabelKey = (typeof BUTTON_LABEL_KEYS)[number]
export type ButtonCrop = { x: number; y: number; width: number; height: number }
export type ButtonCustomization = {
  width: number
  height: number
  fill: boolean
  border: boolean
  font_family: 'system' | 'inter' | 'manrope' | 'mono'
  font_size: number
  outline: { enabled: boolean; width: number; color: string }
  texture_fit: 'cover' | 'contain'
  crop_enabled: boolean
  crop: ButtonCrop
  texture: string
  icon: string
  sound: string
  texture_data: string
  icon_data: string
  sound_data: string
  labels: Record<ButtonLabelKey, string>
}

export const DEFAULT_BUTTON_CUSTOMIZATION: ButtonCustomization = {
  width: 152,
  height: 38,
  fill: true,
  border: true,
  font_family: 'system',
  font_size: 12,
  outline: { enabled: false, width: 1, color: '#111111' },
  texture_fit: 'cover',
  crop_enabled: false,
  crop: { x: 50, y: 50, width: 100, height: 100 },
  texture: '',
  icon: '',
  sound: '',
  texture_data: '',
  icon_data: '',
  sound_data: '',
  labels: { instances: '', accounts: '', mods: '', modpacks: '', themes: '', settings: '' },
}

const fontFamilies = new Set(['system', 'inter', 'manrope', 'mono'])
const colors = /^#[\da-f]{6}$/i

function boundedNumber(value: unknown, min: number, max: number, fallback: number): number {
  const numeric = Number(value)
  return Number.isFinite(numeric) ? Math.round(Math.max(min, Math.min(max, numeric))) : fallback
}

function boundedLabel(value: unknown): string {
  return Array.from(String(value ?? '').trim()).slice(0, 16).join('')
}

export function normalizeButtonCustomization(value: unknown): ButtonCustomization {
  const raw = value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}
  const outline = raw.outline && typeof raw.outline === 'object' && !Array.isArray(raw.outline)
    ? raw.outline as Record<string, unknown>
    : {}
  const crop = raw.crop && typeof raw.crop === 'object' && !Array.isArray(raw.crop)
    ? raw.crop as Record<string, unknown>
    : {}
  const rawLabels = raw.labels && typeof raw.labels === 'object' && !Array.isArray(raw.labels)
    ? raw.labels as Record<string, unknown>
    : {}
  const family = String(raw.font_family ?? DEFAULT_BUTTON_CUSTOMIZATION.font_family)
  const color = String(outline.color ?? DEFAULT_BUTTON_CUSTOMIZATION.outline.color).toLowerCase()
  const cropWidth = boundedNumber(crop.width, 5, 100, 100)
  const cropHeight = boundedNumber(crop.height, 5, 100, 100)
  const normalizedCrop = {
    x: Math.min(boundedNumber(crop.x, 0, 100, 50), 100 - cropWidth),
    y: Math.min(boundedNumber(crop.y, 0, 100, 50), 100 - cropHeight),
    width: cropWidth,
    height: cropHeight,
  }

  return {
    ...DEFAULT_BUTTON_CUSTOMIZATION,
    width: boundedNumber(raw.width, 120, 280, DEFAULT_BUTTON_CUSTOMIZATION.width),
    height: boundedNumber(raw.height, 28, 64, DEFAULT_BUTTON_CUSTOMIZATION.height),
    fill: raw.fill !== false,
    border: raw.border !== false,
    font_family: fontFamilies.has(family) ? family as ButtonCustomization['font_family'] : 'system',
    font_size: boundedNumber(raw.font_size, 9, 20, DEFAULT_BUTTON_CUSTOMIZATION.font_size),
    outline: {
      enabled: outline.enabled === true,
      width: boundedNumber(outline.width, 0, 3, DEFAULT_BUTTON_CUSTOMIZATION.outline.width),
      color: colors.test(color) ? color : DEFAULT_BUTTON_CUSTOMIZATION.outline.color,
    },
    texture_fit: raw.texture_fit === 'contain' ? 'contain' : 'cover',
    crop_enabled: raw.crop_enabled === true,
    crop: normalizedCrop,
    texture: typeof raw.texture === 'string' ? raw.texture.slice(0, 160) : '',
    icon: typeof raw.icon === 'string' ? raw.icon.slice(0, 160) : '',
    sound: typeof raw.sound === 'string' ? raw.sound.slice(0, 160) : '',
    texture_data: safeDataUrl(raw.texture_data, 'image'),
    icon_data: safeDataUrl(raw.icon_data, 'image'),
    sound_data: safeDataUrl(raw.sound_data, 'audio'),
    labels: Object.fromEntries(BUTTON_LABEL_KEYS.map((key) => [key, boundedLabel(rawLabels[key])])) as Record<ButtonLabelKey, string>,
  }
}

export function safeDataUrl(value: unknown, kind: 'image' | 'audio'): string {
  if (typeof value !== 'string') return ''
  const limit = kind === 'image' ? 720_000 : 1_400_000
  if (value.length > limit) return ''
  const accepted = kind === 'image'
    ? /^data:image\/(?:png|jpeg|webp);base64,[a-z\d+/]+=*$/i
    : /^data:audio\/(?:mpeg|ogg|wav|webm);base64,[a-z\d+/]+=*$/i
  return accepted.test(value) ? value : ''
}

export function truncateButtonLabel(value: string, limit = 16): string {
  const chars = Array.from(value)
  if (chars.length <= limit) return value
  return `${chars.slice(0, Math.max(0, limit - 1)).join('')}…`
}

export function buttonFontStack(font: ButtonCustomization['font_family']): string {
  if (font === 'inter') return 'Inter, system-ui, sans-serif'
  if (font === 'manrope') return 'Manrope, system-ui, sans-serif'
  if (font === 'mono') return 'DM Mono, ui-monospace, monospace'
  return 'system-ui, sans-serif'
}

export function cropBackgroundPosition(crop: ButtonCrop): string {
  const width = boundedNumber(crop.width, 5, 100, 100)
  const height = boundedNumber(crop.height, 5, 100, 100)
  const x = Math.min(boundedNumber(crop.x, 0, 100, 50), 100 - width)
  const y = Math.min(boundedNumber(crop.y, 0, 100, 50), 100 - height)
  const positionX = width === 100 ? 0 : Math.round(x / (100 - width) * 10_000) / 100
  const positionY = height === 100 ? 0 : Math.round(y / (100 - height) * 10_000) / 100
  return `${positionX}% ${positionY}%`
}

export function cropBackgroundSize(crop: ButtonCrop, fit: 'cover' | 'contain'): string {
  const width = boundedNumber(crop.width, 5, 100, 100)
  const height = boundedNumber(crop.height, 5, 100, 100)
  if (width === 100 && height === 100) return fit
  return `${Math.max(100, 10000 / width)}% ${Math.max(100, 10000 / height)}%`
}
