export const OVERLAY_WINDOW_LABEL = 'nivora-overlay'

export const DEFAULT_OVERLAY_SETTINGS = {
  enabled: true,
  hotkey_overlay: 'Shift+Tab',
  hotkey_recording: 'F7',
  show_mods: true,
  show_metrics: true,
  recording_fps: 60,
  recording_quality: 23,
  recording_encoder: 'auto',
  recording_container: 'mp4',
  recording_audio: true,
  position: 'top-left',
  scale: 100,
  opacity: 96,
  compact: false,
  show_clock: true,
  show_system_metrics: true,
  metrics_gpu: true,
  metrics_cpu: true,
  metrics_ram: true,
  metrics_disk: true,
  metrics_temperatures: true,
  system_metrics_display: 'overlay',
  show_chat: true,
} as const

const MODIFIERS = new Set(['ctrl', 'control', 'alt', 'option', 'shift', 'meta', 'super', 'command', 'commandorcontrol'])
const SPECIAL_KEYS = new Set([
  'tab', 'space', 'enter', 'return', 'up', 'down', 'left', 'right',
  'insert', 'delete', 'home', 'end', 'pageup', 'pagedown',
  'capslock', 'numlock', 'printscreen', 'pause', 'backquote', 'escape',
])

function canonicalModifier(part: string): string | null {
  const value = part.toLowerCase()
  if (!MODIFIERS.has(value)) return null
  if (value === 'control') return 'Ctrl'
  if (value === 'alt' || value === 'option') return 'Alt'
  if (value === 'shift') return 'Shift'
  if (value === 'meta' || value === 'super' || value === 'command' || value === 'commandorcontrol') return 'Meta'
  return 'Ctrl'
}

const PUNCTUATION_KEYS = new Set([',', '.', '/', ';', "'", '[', ']', '\\', '-', '=', '`'])

function canonicalKey(part: string): string | null {
  const value = part.toLowerCase()
  if (value === ' ') return 'Space'
  if (value === 'arrowup') return 'Up'
  if (value === 'arrowdown') return 'Down'
  if (value === 'arrowleft') return 'Left'
  if (value === 'arrowright') return 'Right'
  if (value === 'comma') return ','
  if (/^[a-z]$/.test(value)) return value.toUpperCase()
  if (/^[0-9]$/.test(value)) return value
  if (/^f([1-9]|1[0-9]|2[0-4])$/.test(value)) return `F${Number(value.slice(1))}`
  if (PUNCTUATION_KEYS.has(value)) return value
  if (SPECIAL_KEYS.has(value)) {
    const names: Record<string, string> = { return: 'Enter', up: 'Up', down: 'Down', left: 'Left', right: 'Right' }
    return names[value] || value[0].toUpperCase() + value.slice(1)
  }
  return null
}

function isModifierOnly(part: string): boolean {
  return MODIFIERS.has(part.toLowerCase())
}

export function normalizeHotkey(value: string | undefined, fallback: string): string {
  const text = String(value || '').trim()
  if (!text) return fallback
  const parts = text.split('+').map((part) => part.trim()).filter(Boolean)
  if (parts.some((part) => !isModifierOnly(part) && !canonicalKey(part))) return fallback
  const modifiers: string[] = []
  const keys: string[] = []
  for (const part of parts) {
    const modifier = canonicalModifier(part)
    if (modifier) {
      if (!modifiers.includes(modifier)) modifiers.push(modifier)
      continue
    }
    const key = canonicalKey(part)
    if (!key) return fallback
    if (!keys.includes(key)) keys.push(key)
  }
  // требуется ровно одна клавиша помимо модификаторов
  if (keys.length !== 1) return fallback
  const order: Record<string, number> = { Ctrl: 0, Alt: 1, Shift: 2, Meta: 3 }
  modifiers.sort((a, b) => order[a] - order[b])
  return [...modifiers, keys[0]].join('+')
}

export function hotkeyModifiersFromEvent(event: { ctrlKey?: boolean; altKey?: boolean; shiftKey?: boolean; metaKey?: boolean; key: string }): string[] {
  const key = event.key.toLowerCase()
  return [
    ...(event.ctrlKey || key === 'control' ? ['Ctrl'] : []),
    ...(event.altKey || key === 'alt' ? ['Alt'] : []),
    ...(event.shiftKey || key === 'shift' ? ['Shift'] : []),
    ...(event.metaKey || key === 'meta' ? ['Meta'] : []),
  ]
}

export function hotkeyFromEvent(event: { ctrlKey?: boolean; altKey?: boolean; shiftKey?: boolean; metaKey?: boolean; key: string }): string {
  const key = canonicalKey(event.key || '')
  if (!key || isModifierOnly(event.key || '')) return ''
  return [...hotkeyModifiersFromEvent(event), key].join('+')
}

export function overlayWindowAction(visible: boolean, exists: boolean): 'create' | 'show' | 'hide' | 'noop' {
  if (visible) return exists ? 'show' : 'create'
  return exists ? 'hide' : 'noop'
}

export function overlayWindowRequested(): boolean {
  if (typeof window === 'undefined') return false
  // метка окна служит запасной проверкой, если URL потерялся при редиректе
  if (new URLSearchParams(window.location.search).get('window') === 'overlay') return true
  try {
    const internals = (window as Window & { __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } } }).__TAURI_INTERNALS__
    return internals?.metadata?.currentWindow?.label === OVERLAY_WINDOW_LABEL
  } catch {
    return false
  }
}

export function recordingFileName(file: string): string {
  if (!file) return ''
  const parts = file.split(/[\\/]/)
  return parts[parts.length - 1] || file
}

type OverlayWindowMode = 'x11' | 'wayland' | 'fallback'

interface OverlayWindowPlan {
  url: string
  transparent: boolean
  decorations: boolean
  shadow: boolean
  alwaysOnTop: boolean
  skipTaskbar: boolean
  center: boolean
  width: number
  height: number
  x?: number
  y?: number
}

export function overlayWindowPlan(
  mode: OverlayWindowMode,
  geometry: { width: number; height: number; x: number; y: number },
): OverlayWindowPlan {
  const compatible = mode !== 'x11'
  if (compatible) {
    return {
      // флаг убирает прозрачную вёрстку в непрозрачном окне Wayland
      url: 'index.html?window=overlay&opaque=1',
      transparent: false,
      decorations: true,
      shadow: false,
      alwaysOnTop: false,
      skipTaskbar: false,
      center: true,
      width: Math.min(Math.max(geometry.width, 640), 980),
      height: Math.min(Math.max(geometry.height, 480), 640),
    }
  }
  return {
    url: 'index.html?window=overlay',
    transparent: true,
    decorations: false,
    // тень WebKitGTK может давать артефакты на прозрачном окне
    shadow: false,
    alwaysOnTop: true,
    skipTaskbar: true,
    center: false,
    width: Math.max(geometry.width, 640),
    height: Math.max(geometry.height, 480),
    x: geometry.x,
    y: geometry.y,
  }
}
