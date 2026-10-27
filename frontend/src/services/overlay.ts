import { WebviewWindow } from '@tauri-apps/api/webviewWindow'
import { PhysicalPosition, PhysicalSize, currentMonitor, primaryMonitor } from '@tauri-apps/api/window'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { isRegistered, register, unregisterAll } from '@tauri-apps/plugin-global-shortcut'
import { bridge, inTauri } from './bridge'
import type { OverlaySettings } from '../types'

export { OVERLAY_WINDOW_LABEL, DEFAULT_OVERLAY_SETTINGS, normalizeHotkey, overlayWindowPlan, overlayWindowRequested, recordingFileName } from './overlayHelpers'

import { OVERLAY_WINDOW_LABEL, normalizeHotkey, overlayWindowAction, overlayWindowPlan, recordingFileName } from './overlayHelpers'

let overlayWindowPromise: Promise<WebviewWindow> | null = null
let overlayVisible = false
let hotkeysBound = false
let closeListener: UnlistenFn | null = null
let lastOverlayError = ''
// если прозрачное окно не создалось, повторяем попытку в непрозрачном режиме
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
  if (waylandSession) return 'wayland'
  return overlayOpaqueFallback || overlayOpaqueForced ? 'fallback' : 'x11'
}

async function waitForWindowIpc(overlay: WebviewWindow, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      await overlay.isVisible()
      return
    } catch {
      // повторяем проверку после короткой паузы
    }
    await new Promise((resolve) => setTimeout(resolve, 150))
  }
  throw new Error(`окно не ответило за ${Math.round(timeoutMs / 1000)} с (событие создания не пришло)`)
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
  // записываем причину принудительного непрозрачного режима в журнал
  const reason = mode === 'fallback' && overlayOpaqueForced && !overlayOpaqueFallback
    ? ' · причина: NVIDIA + форсированный X11 в Wayland-сессии (прозрачное окно на XWayland перерисовывается только под курсором)'
    : ''
  void overlayAttemptLog(`создание окна: режим=${mode} ${plan.width}x${plan.height}@${plan.x ?? 'центр'},${plan.y ?? ''} transparent=${plan.transparent} alwaysOnTop=${plan.alwaysOnTop}${reason}`)
  return new Promise<WebviewWindow>((resolve, reject) => {
    const overlay = new WebviewWindow(OVERLAY_WINDOW_LABEL, {
      ...plan,
      title: 'Nivora Overlay',
      resizable: false,
      visible: true,
      focus: true,
      maximized: false,
      fullscreen: false,
    })
    let settled = false
    const settle = (error?: Error) => {
      if (settled) return
      settled = true
      if (error) {
        void overlayAttemptLog(`окно НЕ создано: ${error.message}`)
        reject(error)
      } else {
        resolve(overlay)
      }
    }
    overlay.once('tauri:created', () => settle())
    overlay.once('tauri:error', ({ payload }) => {
      lastOverlayError = String((payload as { message?: string } | string) || '') || 'окно оверлея не создано'
      settle(new Error(`Не удалось создать окно оверлея: ${lastOverlayError}`))
    })
    void waitForWindowIpc(overlay, 9000).then(() => settle(), (error: Error) => settle(error))
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
        if (overlayWindowMode() !== 'x11') throw error
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

// используем геометрию текущего или основного монитора
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
    // если API мониторов недоступно, используем запасной размер
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
      // внутренний оверлей также закрывается клавишей Esc
      if (onInAppClose) onInAppClose()
    })
  } catch {
    closeListener = null
  }
  // событие из меню трея работает и без глобальных клавиш Wayland
  try {
    await listen('nivora-overlay-open', () => {
      if (onTrayOpen) onTrayOpen()
      else void toggleOverlay()
    })
  } catch {
  }
}

export async function setOverlayVisible(visible: boolean): Promise<boolean> {
  overlayVisible = visible
  if (!inTauri()) return visible
  const wayland = overlayWaylandMode()
  try {
    const existing = overlayWindowPromise
      ? await overlayWindowPromise
      : await WebviewWindow.getByLabel(OVERLAY_WINDOW_LABEL).catch(() => null)
    const action = overlayWindowAction(visible, Boolean(existing))
    if (action === 'noop') return true
    if (action === 'hide') {
      await existing?.hide()
      return true
    }
    const overlay = existing ?? await ensureOverlayWindow()
    if (visible) {
      const started = Date.now()
      await overlay.show()
      let shown = await overlay.isVisible().catch(() => true)
      if (!shown) {
        // вебвью WebKitGTK иногда показывает окно с задержкой
        await new Promise((resolve) => setTimeout(resolve, 250))
        await overlay.show().catch(() => undefined)
        shown = await overlay.isVisible().catch(() => true)
      }
      if (!shown) throw new Error('окно оверлея не отобразилось')
      if (!wayland) {
        await coverActiveMonitor(overlay)
        await overlay.setAlwaysOnTop(true).catch(() => undefined)
      }
      await overlay.setFocus().catch(() => undefined)
      lastOverlayError = ''
      void overlayAttemptLog(`окно показано за ${Date.now() - started} мс (режим ${overlayWindowMode()})`)
      return true
    }
    return true
  } catch (error) {
    overlayVisible = false
    lastOverlayError = error instanceof Error ? error.message : String(error)
    void overlayAttemptLog(`показ НЕ удался: ${lastOverlayError}`)
    // окно могло закрыться снаружи, поэтому сбрасываем его кэш
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
    [normalizeHotkey(settings.hotkey_overlay, 'Shift+Tab'), onOverlayToggle],
    [normalizeHotkey(settings.hotkey_recording, 'F7'), onRecordingToggle],
  ]
  // wayland не поддерживает глобальный перехват клавиш
  if (waylandSession) return bindings.map(([combination]) => combination)
  const failed: string[] = []
  for (const [combination, handler] of bindings) {
    try {
      await register(combination, (event) => {
        if (event.state === 'Pressed') handler()
      })
      hotkeysBound = true
    } catch {
      // другое приложение могло занять это сочетание
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
  await setOverlayVisible(false)
}

export async function toggleRecording(instanceName: string): Promise<string> {
  let status = null
  try {
    status = await bridge.recordingStatus()
  } catch {
    // при недоступном статусе пробуем запустить запись
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
  if (mode === 'wayland') return 'совместимый (непрозрачный) — нативный Wayland'
  if (overlayOpaqueForced) return 'совместимый (непрозрачный), принудительно: NVIDIA + форсированный X11 в Wayland-сессии — прозрачное окно на XWayland перерисовывалось бы только под курсором'
  return 'совместимый (непрозрачный) — прозрачное окно не создалось с первой попытки'
}

export interface OverlaySelfTestStep {
  name: string
  ok: boolean
  detail: string
}

export async function overlaySelfTest(settings: Pick<OverlaySettings, 'hotkey_overlay' | 'hotkey_recording'>): Promise<OverlaySelfTestStep[]> {
  const steps: OverlaySelfTestStep[] = []
  const desktop = inTauri()
  steps.push({
    name: 'Среда',
    ok: desktop,
    detail: desktop
      ? `сессия: ${overlayWaylandMode() ? 'нативный Wayland' : 'X11/XWayland'}; режим окна: ${describeOverlayWindowMode()}`
      : 'запущено вне Tauri (браузер?) — оверлей доступен только в приложении',
  })
  let monitorInfo = 'API мониторов недоступно — окно создастся 1280×800'
  try {
    const monitor = (await currentMonitor()) || (await primaryMonitor())
    if (monitor) monitorInfo = `${monitor.size.width}×${monitor.size.height}, масштаб ${monitor.scaleFactor}`
  } catch {
  }
  steps.push({ name: 'Экран', ok: true, detail: monitorInfo })
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
  if (created && !overlayWaylandMode()) {
    try {
      const overlay = await ensureOverlayWindow()
      await overlay.setAlwaysOnTop(true)
      steps.push({ name: 'Поверх всех', ok: true, detail: 'always-on-top применён' })
    } catch (error) {
      steps.push({ name: 'Поверх всех', ok: false, detail: `не удалось: ${error instanceof Error ? error.message : String(error)}` })
    }
  } else if (overlayWaylandMode()) {
    steps.push({ name: 'Поверх всех', ok: true, detail: 'не применяется в нативном Wayland — окно переключается как обычное' })
  }
  const combinations = [
    normalizeHotkey(settings.hotkey_overlay, 'Shift+Tab'),
    normalizeHotkey(settings.hotkey_recording, 'F7'),
  ]
  if (overlayWaylandMode()) {
    steps.push({ name: 'Глобальные клавиши', ok: true, detail: `${combinations.join(', ')} — в нативном Wayland перехват чужих клавиш невозможен; работают при фокусе лаунчера, кнопка «Оверлей» и значок в трее` })
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
  steps.push({
    name: 'Если окна всё равно не видно',
    ok: true,
    detail: 'все пункты зелёные, но поверх игры оверлея нет — игра в эксклюзивном полноэкранном режиме, который рисуется поверх любых окон. Переключите игру в оконный/безрамочный режим (F11 в Minecraft).',
  })
  return steps
}
