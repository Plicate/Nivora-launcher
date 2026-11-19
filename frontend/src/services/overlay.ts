import { WebviewWindow } from '@tauri-apps/api/webviewWindow'
import { PhysicalPosition, PhysicalSize, currentMonitor, getCurrentWindow, primaryMonitor } from '@tauri-apps/api/window'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { isRegistered, register, unregisterAll } from '@tauri-apps/plugin-global-shortcut'
import { bridge, inTauri } from './bridge'
import type { OverlaySettings } from '../types'

export { OVERLAY_WINDOW_LABEL, DEFAULT_OVERLAY_HOTKEY, DEFAULT_OVERLAY_SETTINGS, OVERLAY_MISSED_POLLS_BEFORE_DETACH, consumeOverlayCloseKey, normalizeHotkey, normalizeOverlayHotkey, overlayWindowPlan, overlayWindowRequested, recordingFileName, shouldDetachOverlayAfterMisses } from './overlayHelpers'

import { OVERLAY_WINDOW_LABEL, normalizeHotkey, normalizeOverlayHotkey, overlayWindowAction, overlayWindowPlan, recordingFileName } from './overlayHelpers'

let overlayWindowPromise: Promise<WebviewWindow> | null = null
let overlayVisible = false
let overlayVisibilityRequest = 0
let hotkeysBound = false
let closeListener: UnlistenFn | null = null
let lastOverlayError = ''
let overlayOpaqueFallback = false

export function overlayLastError(): string {
  return lastOverlayError
}

let waylandSession = false

export function setOverlayWaylandMode(value: boolean): void {
  waylandSession = value
}

export function overlayWaylandMode(): boolean {
  return waylandSession
}

let overlayOpaqueForced = false

export function setOverlayOpaqueWindow(value: boolean): void {
  overlayOpaqueForced = value
}

function overlayWindowMode(): 'x11' | 'wayland' | 'fallback' {
  if (overlayOpaqueFallback || overlayOpaqueForced) return 'fallback'
  if (waylandSession) return 'wayland'
  return 'x11'
}

async function overlayAttemptLog(entry: string): Promise<{ path: string; tail: string } | null> {
  if (!inTauri()) return null
  try {
    return await invoke<{ path: string; tail: string }>('overlay_attempt_log', { entry })
  } catch {
    return null
  }
}

async function createOverlayWindow(): Promise<WebviewWindow> {
  const existing = await WebviewWindow.getByLabel(OVERLAY_WINDOW_LABEL).catch(() => null)
  if (existing) return existing
  const mode = overlayWindowMode()
  const geometry = await initialOverlayGeometry()
  const plan = overlayWindowPlan(mode, geometry)
  const reason = mode === 'fallback' && overlayOpaqueForced && !overlayOpaqueFallback
    ? ' · причина: NVIDIA + форсированный X11 в Wayland-сессии (прозрачное окно на XWayland перерисовывается только под курсором)'
    : ''
  void overlayAttemptLog(`создание окна: режим=${mode} ${plan.width}x${plan.height}@${plan.x ?? 'центр'},${plan.y ?? ''} transparent=${plan.transparent} alwaysOnTop=${plan.alwaysOnTop}${reason}`)
  return new Promise<WebviewWindow>((resolve, reject) => {
    const overlay = new WebviewWindow(OVERLAY_WINDOW_LABEL, {
      ...plan,
      title: 'Nivora Overlay',
      resizable: false,
      visible: false,
      focus: false,
      maximized: false,
      fullscreen: false,
    })
    let settled = false
    let timeout: number | undefined
    const settle = (error?: Error) => {
      if (settled) return
      settled = true
      if (timeout !== undefined) window.clearTimeout(timeout)
      if (error) {
        void overlayAttemptLog(`окно НЕ создано: ${error.message}`)
        reject(error)
      } else {
        resolve(overlay)
      }
    }
    timeout = window.setTimeout(() => settle(new Error('окно оверлея не ответило за 9 с')), 9000)
    overlay.once('tauri://created', () => settle())
    overlay.once('tauri://error', ({ payload }) => {
      lastOverlayError = String((payload as { message?: string } | string) || '') || 'окно оверлея не создано'
      settle(new Error(`Не удалось создать окно оверлея: ${lastOverlayError}`))
    })
    overlay.onCloseRequested(() => {
      overlayVisible = false
      overlayWindowPromise = null
    }).catch(() => undefined)
  })
}

async function ensureOverlayWindow(): Promise<WebviewWindow> {
  if (!overlayWindowPromise) {
    overlayWindowPromise = (async () => {
      try {
        return await createOverlayWindow()
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        if (overlayWindowMode() === 'fallback') throw error
        lastOverlayError = `${message} → пробую совместимое непрозрачное окно`
        overlayOpaqueFallback = true
        try {
          return await createOverlayWindow()
        } catch (retryError) {
          const retryMessage = retryError instanceof Error ? retryError.message : String(retryError)
          lastOverlayError = `${message}; совместимое окно тоже не создалось: ${retryMessage}`
          throw retryError
        }
      }
    })()
    overlayWindowPromise.catch(() => {
      overlayWindowPromise = null
    })
  }
  return overlayWindowPromise
}

async function initialOverlayGeometry(): Promise<{ width: number; height: number; x: number; y: number }> {
  try {
    const monitor = (await currentMonitor()) || (await primaryMonitor())
    if (monitor) {
      const scale = monitor.scaleFactor && monitor.scaleFactor > 0 ? monitor.scaleFactor : 1
      return {
        width: Math.max(640, Math.round(monitor.size.width / scale)),
        height: Math.max(480, Math.round(monitor.size.height / scale)),
        x: Math.round(monitor.position.x / scale),
        y: Math.round(monitor.position.y / scale),
      }
    }
  } catch {
  }
  return { width: 1280, height: 800, x: 0, y: 0 }
}

async function coverActiveMonitor(overlay: WebviewWindow): Promise<void> {
  try {
    const monitor = await currentMonitor()
    if (!monitor) return
    await overlay.setSize(new PhysicalSize(monitor.size.width, monitor.size.height))
    await overlay.setPosition(new PhysicalPosition(monitor.position.x, monitor.position.y))
  } catch {
  }
}

export async function initOverlayEventBridge(onTrayOpen?: () => void, onInAppClose?: () => void): Promise<void> {
  if (!inTauri() || closeListener) return
  try {
    closeListener = await listen('nivora-overlay-close', () => {
      void setOverlayVisible(false)
      if (onInAppClose) onInAppClose()
    })
  } catch {
    closeListener = null
  }
  try {
    await listen('nivora-overlay-open', async () => {
      const mainWindow = getCurrentWindow()
      await mainWindow.show().catch(() => undefined)
      await mainWindow.unminimize().catch(() => undefined)
      await mainWindow.setFocus().catch(() => undefined)
      if (onTrayOpen) onTrayOpen()
      else void toggleOverlay()
    })
  } catch {
  }
}

export async function setOverlayVisible(visible: boolean): Promise<boolean> {
  const request = ++overlayVisibilityRequest
  overlayVisible = visible
  if (!inTauri()) return visible
  try {
    const existing = overlayWindowPromise
      ? await overlayWindowPromise
      : await WebviewWindow.getByLabel(OVERLAY_WINDOW_LABEL).catch(() => null)
    if (request !== overlayVisibilityRequest) return overlayVisible
    const action = overlayWindowAction(visible, Boolean(existing))
    if (action === 'noop') return true
    if (action === 'hide') {
      await existing?.hide()
      if (request !== overlayVisibilityRequest && overlayVisible) await existing?.show().catch(() => undefined)
      return overlayVisible
    }
    const overlay = existing ?? await ensureOverlayWindow()
    if (request !== overlayVisibilityRequest) {
      if (!overlayVisible) await overlay.hide().catch(() => undefined)
      return overlayVisible
    }
    if (visible) {
      const started = Date.now()
      await coverActiveMonitor(overlay)
      lastOverlayError = ''
      await overlay.setAlwaysOnTop(true).catch((error) => {
        lastOverlayError = `Окно создано, но режим поверх окон не подтверждён: ${String(error)}`
        void overlayAttemptLog(lastOverlayError)
      })
      await overlay.show()
      if (request !== overlayVisibilityRequest) {
        if (!overlayVisible) await overlay.hide().catch(() => undefined)
        return overlayVisible
      }
      let shown = await overlay.isVisible().catch(() => true)
      if (!shown) {
        await new Promise((resolve) => setTimeout(resolve, 250))
        if (request !== overlayVisibilityRequest) return overlayVisible
        await overlay.show().catch(() => undefined)
        shown = await overlay.isVisible().catch(() => true)
      }
      if (!shown) throw new Error('окно оверлея не отобразилось')
      if (request !== overlayVisibilityRequest) {
        if (!overlayVisible) await overlay.hide().catch(() => undefined)
        return overlayVisible
      }
      if (!lastOverlayError) void overlayAttemptLog(`окно показано за ${Date.now() - started} мс (режим ${overlayWindowMode()})`)
      return true
    }
    return true
  } catch (error) {
    if (request !== overlayVisibilityRequest) return overlayVisible
    overlayVisible = false
    lastOverlayError = error instanceof Error ? error.message : String(error)
    void overlayAttemptLog(`показ НЕ удался: ${lastOverlayError}`)
    overlayWindowPromise = null
    return false
  }
}

export async function toggleOverlay(): Promise<boolean> {
  await setOverlayVisible(!overlayVisible)
  return overlayVisible
}

let hotkeyBindChain: Promise<unknown> = Promise.resolve()

export async function bindGameHotkeys(
  settings: OverlaySettings,
  onOverlayToggle: () => void,
  onRecordingToggle: () => void,
): Promise<string[]> {
  if (!inTauri()) return []
  const run = hotkeyBindChain.catch(() => undefined).then(async () => {
    await unbindGameHotkeys()
  const bindings: Array<[string, () => void]> = [
    [normalizeOverlayHotkey(settings.hotkey_overlay), onOverlayToggle],
    [normalizeHotkey(settings.hotkey_recording, 'F7'), onRecordingToggle],
  ]
  if (waylandSession) return bindings.map(([combination]) => combination)
  const failed: string[] = []
  for (const [combination, handler] of bindings) {
    try {
      await register(combination, (event) => {
        if (event.state === 'Pressed') handler()
      })
      hotkeysBound = true
    } catch {
      failed.push(combination)
    }
  }
  return failed
  })
  hotkeyBindChain = run
  return run as Promise<string[]>
}

export async function unbindGameHotkeys(): Promise<void> {
  if (!inTauri()) return
  if (hotkeysBound) {
    try {
      await unregisterAll()
    } catch {
    }
    hotkeysBound = false
  }
}

export async function toggleRecording(instanceName: string): Promise<string> {
  let status = null
  try {
    status = await bridge.recordingStatus()
  } catch {
  }
  if (status?.active) {
    const stopped = await bridge.recordingStop()
    const name = recordingFileName(stopped.file)
    return name ? `Запись сохранена: ${name}` : 'Запись остановлена'
  }
  const started = await bridge.recordingStart(instanceName)
  if (!started.active) throw new Error('Запись не началась — смотрите лог recordings/last-recording.log')
  return `Запись начата · ${started.encoder}`
}

function describeOverlayWindowMode(): string {
  const mode = overlayWindowMode()
  if (mode === 'x11') return 'обычный (прозрачный, поверх всех)'
  if (mode === 'wayland') return 'прозрачное полноэкранное окно — Wayland compositor может скрыть его за полноэкранной игрой'
  if (overlayOpaqueForced) return 'совместимый (непрозрачный), принудительно: NVIDIA + форсированный X11 в Wayland-сессии — прозрачное окно на XWayland перерисовывалось бы только под курсором'
  return 'совместимый (непрозрачный) — прозрачное окно не создалось с первой попытки'
}

export interface OverlaySelfTestStep {
  name: string
  ok: boolean
  detail: string
}

export async function overlaySelfTest(settings: Pick<OverlaySettings, 'hotkey_overlay' | 'hotkey_recording'>): Promise<OverlaySelfTestStep[]> {
  const wasOverlayVisible = overlayVisible
  const steps: OverlaySelfTestStep[] = []
  const desktop = inTauri()
  steps.push({
    name: 'Среда',
    ok: desktop,
    detail: desktop
      ? `сессия: ${overlayWaylandMode() ? 'нативный Wayland' : 'X11/XWayland'}; режим окна: ${describeOverlayWindowMode()}`
      : 'запущено вне Tauri (браузер?) — оверлей доступен только в приложении',
  })
  let monitorAvailable = false
  let monitorInfo = 'API мониторов недоступно — окно создастся 1280×800'
  try {
    const monitor = (await currentMonitor()) || (await primaryMonitor())
    if (monitor) {
      monitorAvailable = true
      monitorInfo = `${monitor.size.width}×${monitor.size.height}, масштаб ${monitor.scaleFactor}`
    }
  } catch {
  }
  steps.push({ name: 'Экран', ok: monitorAvailable, detail: monitorInfo })
  const attemptLog = await overlayAttemptLog('запуск самодиагностики')
  steps.push({
    name: 'Журнал попыток',
    ok: true,
    detail: attemptLog ? attemptLog.path : 'журнал недоступен (запуск вне приложения?)',
  })
  if (!desktop) return steps
  const started = Date.now()
  let created = false
  try {
    await ensureOverlayWindow()
    created = true
  } catch {
  }
  const shown = created ? await setOverlayVisible(true) : false
  steps.push({
    name: 'Окно',
    ok: shown,
    detail: shown
      ? `создано и показано за ${Date.now() - started} мс${overlayWindowMode() === 'fallback' ? ` (${describeOverlayWindowMode()})` : ''}`
      : overlayLastError() || 'окно не создалось',
  })
  if (created) {
    if (overlayWaylandMode()) {
      steps.push({
        name: 'Поверх всех',
        ok: false,
        detail: 'запрос always-on-top не гарантирует порядок окон в Wayland; его определяет композитор, и поверх fullscreen-игры это не подтверждено',
      })
    } else {
      try {
        const overlay = await ensureOverlayWindow()
        await overlay.setAlwaysOnTop(true)
        steps.push({
          name: 'Поверх всех',
          ok: true,
          detail: 'запрос принят; фактическое отображение поверх эксклюзивной fullscreen-игры не проверено',
        })
      } catch (error) {
        steps.push({ name: 'Поверх всех', ok: false, detail: `не удалось: ${error instanceof Error ? error.message : String(error)}` })
      }
    }
  }
  const combinations = [
    normalizeOverlayHotkey(settings.hotkey_overlay),
    normalizeHotkey(settings.hotkey_recording, 'F7'),
  ]
  if (overlayWaylandMode()) {
    steps.push({ name: 'Глобальные клавиши', ok: false, detail: `${combinations.join(', ')} не перехватываются в нативном Wayland; используйте кнопку «Оверлей» или пункт в меню трея (панель откроется в окне лаунчера)` })
  } else {
    const states = await Promise.all(combinations.map(async (combination) => {
      try {
        return [combination, await isRegistered(combination)] as const
      } catch {
        return [combination, false] as const
      }
    }))
    const registeredCount = states.filter(([, registered]) => registered).length
    steps.push({
      name: 'Глобальные клавиши',
      ok: registeredCount > 0,
      detail: registeredCount === combinations.length
        ? `${combinations.join(', ')} зарегистрированы`
        : registeredCount === 0
          ? `${combinations.join(', ')} не зарегистрированы (кнопка «Оверлей» и трей работают всегда)`
          : `${registeredCount} из ${combinations.length} зарегистрированы (${combinations.join(', ')})`,
    })
  }
  if (!wasOverlayVisible) await setOverlayVisible(false)
  steps.push({
    name: 'Если окна всё равно не видно',
    ok: true,
    detail: 'если окно создано, но его не видно поверх игры: Wayland-композитор или эксклюзивный полноэкранный режим может держать его позади. В нативном Wayland используйте панель в окне лаунчера; для отдельного окна переключите Minecraft в оконный/безрамочный режим (F11).',
  })
  return steps
}
