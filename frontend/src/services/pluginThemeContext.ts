import type { PluginThemeContext, PluginThemeDefinition, PluginThemeFont } from '../../../plugins/api'
import type { AppearanceSettings } from '../types'
const themeFonts: readonly PluginThemeFont[] = ['system', 'inter', 'manrope', 'mono']
const themeColorPattern = /^#[\da-f]{6}$/i
const themeTokenKeyPattern = /^[a-z][a-z0-9_.-]{0,255}$/i

export function safeThemeToken(value: unknown): string | number | undefined {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string' && value.length <= 256 && !/[\u0000-\u001f\u007f]/.test(value)) return value
  return undefined
}

export function safeThemeKey(key: string): boolean {
  return themeTokenKeyPattern.test(key) && !['__proto__', 'constructor', 'prototype'].includes(key.toLowerCase())
}

export function safeThemeColor(value: unknown): string | undefined {
  return typeof value === 'string' && themeColorPattern.test(value) ? value.toLowerCase() : undefined
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function boundedNumber(value: unknown, min: number, max: number, fallback: number): number {
  const numeric = Number(value)
  return Number.isFinite(numeric) ? Math.round(Math.max(min, Math.min(max, numeric))) : fallback
}

function boundedLabel(value: unknown): string {
  return Array.from(String(value ?? '').trim()).slice(0, 16).join('')
}

export function buildPluginThemeContext(
  appearance: AppearanceSettings,
  mode: PluginThemeContext['mode'],
  currentLocale: string,
  uiFont: string,
): PluginThemeContext {
  const colors: Record<string, string> = {}
  const tokens: Record<string, string | number> = {}
  for (const [key, rawColor] of Object.entries(appearance.color_tokens || {})) {
    const color = safeThemeColor(rawColor)
    if (!safeThemeKey(key) || !color) continue
    colors[key] = color
    tokens[key] = color
  }
  const accent = safeThemeColor(appearance.accent_color) || colors.accent || '#3d8bfd'
  colors.accent = accent
  tokens.accent = accent
  const rawButton: Record<string, unknown> = isRecord(appearance.button_customization) ? appearance.button_customization : {}
  const rawOutline: Record<string, unknown> = isRecord(rawButton.outline) ? rawButton.outline : {}
  const rawCrop: Record<string, unknown> = isRecord(rawButton.crop) ? rawButton.crop : {}
  const rawLabels: Record<string, unknown> = isRecord(rawButton.labels) ? rawButton.labels : {}
  const buttonFont = themeFonts.includes(String(rawButton.font_family) as PluginThemeFont)
    ? String(rawButton.font_family) as PluginThemeFont
    : 'system'
  const cropWidth = boundedNumber(rawCrop.width, 5, 100, 100)
  const cropHeight = boundedNumber(rawCrop.height, 5, 100, 100)
  const buttonStyle = Object.freeze({
    width: boundedNumber(rawButton.width, 120, 280, 152),
    height: boundedNumber(rawButton.height, 28, 64, 38),
    fill: rawButton.fill !== false,
    border: rawButton.border !== false,
    font_family: buttonFont,
    font_size: boundedNumber(rawButton.font_size, 9, 20, 12),
    outline: Object.freeze({
      enabled: rawOutline.enabled === true,
      width: boundedNumber(rawOutline.width, 0, 3, 1),
      color: safeThemeColor(rawOutline.color) || '#111111',
    }),
    texture_fit: rawButton.texture_fit === 'contain' ? 'contain' : 'cover',
    crop_enabled: rawButton.crop_enabled === true,
    crop: Object.freeze({
      x: Math.min(boundedNumber(rawCrop.x, 0, 100, 50), 100 - cropWidth),
      y: Math.min(boundedNumber(rawCrop.y, 0, 100, 50), 100 - cropHeight),
      width: cropWidth,
      height: cropHeight,
    }),
  })
  const buttonLabels = Object.freeze({
    instances: boundedLabel(rawLabels.instances),
    accounts: boundedLabel(rawLabels.accounts),
    mods: boundedLabel(rawLabels.mods),
    modpacks: boundedLabel(rawLabels.modpacks),
    themes: boundedLabel(rawLabels.themes),
    settings: boundedLabel(rawLabels.settings),
  })
  const fonts = Object.freeze({
    ui: themeFonts.includes(uiFont as PluginThemeFont) ? uiFont as PluginThemeFont : 'system',
    button: buttonFont,
    available: Object.freeze([...themeFonts]),
  })
  return Object.freeze({
    mode,
    accent,
    tokens: Object.freeze(tokens),
    colors: Object.freeze(colors),
    locale: currentLocale === 'en' ? 'en' : 'ru',
    buttonStyle,
    buttonLabels,
    fonts,
  })
}

interface PluginThemeHostContext {
  theme(): PluginThemeContext
  authorize(permission: 'ui:compose'): Promise<void>
  registerTheme(id: string, definition: PluginThemeDefinition): void
  unregisterTheme(id: string): void
}

function stringArg(value: Record<string, unknown>, key: string, max: number): string {
  const field = value[key]
  if (typeof field !== 'string' || field.length > max) throw new TypeError(`${key} must be a string of at most ${max} characters`)
  return field
}

function themeDefinition(value: unknown): PluginThemeDefinition {
  if (!isRecord(value) || Object.keys(value).some((key) => !['name', 'colors', 'tokens'].includes(key))) {
    throw new TypeError('theme definition must contain only name, colors, and tokens')
  }
  const name = value.name === undefined ? '' : stringArg(value, 'name', 80)
  const colors: Record<string, string> = {}
  const tokens: Record<string, string | number> = {}
  if (value.colors !== undefined) {
    if (!isRecord(value.colors) || Object.keys(value.colors).length > 64) throw new TypeError('theme colors must be an object of at most 64 entries')
    for (const [key, color] of Object.entries(value.colors)) {
      if (!/^[a-z][a-z0-9_.-]{0,63}$/i.test(key) || typeof color !== 'string' || !/^#[0-9a-f]{6}$/i.test(color)) {
        throw new TypeError(`Invalid theme color: ${key}`)
      }
      colors[key] = color.toLowerCase()
    }
  }
  if (value.tokens !== undefined) {
    if (!isRecord(value.tokens) || Object.keys(value.tokens).length > 64) throw new TypeError('theme tokens must be an object of at most 64 entries')
    for (const [key, token] of Object.entries(value.tokens)) {
      if (!/^[a-z][a-z0-9_.-]{0,63}$/i.test(key)) throw new TypeError(`Invalid theme token: ${key}`)
      if (typeof token === 'string' && token.length <= 256) {
        tokens[key] = token
      } else if (typeof token === 'number' && Number.isFinite(token)) {
        tokens[key] = token
      } else {
        throw new TypeError(`Invalid theme token: ${key}`)
      }
    }
  }
  return {
    ...(name ? { name } : {}),
    ...(value.colors !== undefined ? { colors: Object.freeze(colors) } : {}),
    ...(value.tokens !== undefined ? { tokens: Object.freeze(tokens) } : {}),
  }
}

export async function dispatchPluginThemeHostCall(
  operation: string,
  args: Record<string, unknown>,
  context: PluginThemeHostContext,
): Promise<unknown> {
  switch (operation) {
    case 'theme.get':
      return context.theme()
    case 'theme.register': {
      await context.authorize('ui:compose')
      context.registerTheme(stringArg(args, 'registration_id', 96), themeDefinition(args.definition))
      return undefined
    }
    case 'theme.unregister':
      context.unregisterTheme(stringArg(args, 'registration_id', 96))
      return undefined
    default:
      throw new Error(`Unsupported plugin theme host operation: ${operation}`)
  }
}
