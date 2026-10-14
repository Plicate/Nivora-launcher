// Чистые helpers оверлея без зависимостей: используются и в окне лаунчера,
// и в окне оверлея, и в node-тестах.

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
  metrics_cpu: true,
  metrics_ram: true,
  metrics_gpu: true,
  metrics_disk: true,
  metrics_temperatures: true,
  system_metrics_display: 'overlay',
  show_chat: true,
} as const

// Модификаторы и клавиши, которые реально умеет регистрировать системный
// слой глобальных горячих клавиш. Всему прочему (пунктуация, «е» из русской
// раскладки, имя окна) здесь не место — раньше поле было свободным текстом,
// и мусор в нём молча убивал оверлей.
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

// Точечный список пунктуационных клавиш: без него «Ctrl+,» (настройки по
// умолчанию) нельзя ни записать рекордером, ни сохранить — canonicalKey
// возвращал null и normalizeHotkey затирал значение фолбэком.
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
  // комбинация без обычной клавиши («Ctrl+Shift») — не комбинация
  if (keys.length !== 1) return fallback
  const order: Record<string, number> = { Ctrl: 0, Alt: 1, Shift: 2, Meta: 3 }
  modifiers.sort((a, b) => order[a] - order[b])
  return [...modifiers, keys[0]].join('+')
}

// Сбор комбинации из события клавиатуры: пока нажаты только модификаторы,
// ждём основную клавишу. Неподдерживаемую клавишу игнорируем.
export function hotkeyFromEvent(event: { ctrlKey?: boolean; altKey?: boolean; shiftKey?: boolean; metaKey?: boolean; key: string }): string {
  const key = canonicalKey(event.key || '')
  if (!key || isModifierOnly(event.key || '')) return ''
  const parts: string[] = []
  if (event.ctrlKey) parts.push('Ctrl')
  if (event.altKey) parts.push('Alt')
  if (event.shiftKey) parts.push('Shift')
  if (event.metaKey) parts.push('Meta')
  parts.push(key)
  return parts.join('+')
}

export function overlayWindowRequested(): boolean {
  if (typeof window === 'undefined') return false
  // Два независимых признака. Параметр URL может потеряться при редиректах
  // и особенностях протокола webview, а вот метка окна зашита в рантайм
  // Tauri — её потерять нельзя. Раньше при потере параметра окно оверлея
  // рисовало ВЕСЬ лаунчер заново (второй bootstrap, «чудо с уменьшенным
  // окном»), вместо самого оверлея.
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

// Параметры создания окна оверлея. Три режима:
//  - 'x11'      — прозрачное бордерлесс-окно на весь монитор, поверх всех
//                 (обычный путь для X11 и XWayland-сборок пакета);
//  - 'wayland'  — нативный Wayland: клиент не может ставить себя поверх
//                 других окон и позиционировать себя, поэтому окно обычное,
//                 непрозрачное, с рамкой, по центру и не больше 980×640;
//  - 'fallback' — совместимое окно вместо прозрачного: либо прозрачное
//                 НЕ создалось (аварийно, NVIDIA/XWayland и т.п.), либо
//                 среда заведомо рискованная и непрозрачность включена
//                 превентивно (см. setOverlayOpaqueWindow в overlay.ts):
//                 там прозрачное окно перерисовывается только под курсором.
//                 Лучше непрозрачный работающий оверлей, чем «кнопка
//                 ничего не делает».
export type OverlayWindowMode = 'x11' | 'wayland' | 'fallback'

export interface OverlayWindowPlan {
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
      // &opaque=1 сообщает окну оверлея, что оно живёт в непрозрачном окне:
      // иначе полупрозрачная вёрстка ляжет на белый фон.
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
    // Тень у бордерлесс-окна на WebKitGTK — источник артефактов, оверлею
    // она не нужна.
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
