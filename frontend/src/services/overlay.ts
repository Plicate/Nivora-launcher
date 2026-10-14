// Управление игровым оверлеем Nivora (как оверлей Steam).
//
// Главное окно лаунчера на время игровой сессии регистрирует глобальные
// горячие клавиши (Shift+Tab — оверлей, F7 — запись) и создаёт отдельное
// прозрачное окно поверх игры. Само окно оверлея рендерит GameOverlay.vue
// и опрашивает overlay_state; здесь только «тумблеры» и жизненный цикл.
import { WebviewWindow } from '@tauri-apps/api/webviewWindow'
import { PhysicalPosition, PhysicalSize, currentMonitor, primaryMonitor } from '@tauri-apps/api/window'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { isRegistered, register, unregisterAll } from '@tauri-apps/plugin-global-shortcut'
import { bridge, inTauri } from './bridge'
import type { OverlaySettings } from '../types'

export { OVERLAY_WINDOW_LABEL, DEFAULT_OVERLAY_SETTINGS, normalizeHotkey, overlayWindowPlan, overlayWindowRequested, recordingFileName } from './overlayHelpers'

import { OVERLAY_WINDOW_LABEL, normalizeHotkey, overlayWindowPlan, recordingFileName } from './overlayHelpers'

let overlayWindowPromise: Promise<WebviewWindow> | null = null
let overlayVisible = false
let hotkeysBound = false
let closeListener: UnlistenFn | null = null
// Ошибки окна оверлея раньше глотались: «оверлей не открывается» без единого
// сообщения. Теперь последний сбой доступен UI для тоста/уведомления.
let lastOverlayError = ''
// Прозрачное поверх-всех окно хоть раз не создалось (NVIDIA/XWayland и др.)?
// Тогда до конца сессии используем совместимую непрозрачную форму окна.
let overlayOpaqueFallback = false

export function overlayLastError(): string {
  return lastOverlayError
}

// Окно намеренно НЕ делаем fullscreen: эксклюзивный полноэкранный режим
// игры всё равно рисуется поверх topmost-окон, а transparent+fullscreen
// на WebView2 местами даёт чёрный кадр. Вместо этого — бордерлесс-окно
// размером в текущий монитор: тот же эффект, меньше сюрпризов.
//
// Геометрию задаём уже при создании (с запасом 1280×800): если API мониторов
// недоступен (нет разрешения, драйвер, HDR-переход), окно всё равно
// открывается приличного размера, а не «уменьшенное» 800×600 в углу.
//
// Wayland: клиенты не могут ни позиционировать себя, ни ставить себя поверх
// других окон, а прозрачность в webkit2gtk работает не везде. Поэтому в
// Wayland-сессии окно создаётся обычным, непрозрачным и по центру — это
// честно работает; «поверх игры» на Wayland не даст ни один нормальный
// клиент (оверлей Steam работает иначе — он внедряется в процесс игры).
let waylandSession = false

// Истинный тип сессии знает бэкенд (XDG_SESSION_TYPE): App.vue сообщает его
// сюда после bootstrap. Никаких эвристик по User-Agent — они зачеркнули бы
// и обычные X11-сборки на Linux.
export function setOverlayWaylandMode(value: boolean): void {
  waylandSession = value
}

export function overlayWaylandMode(): boolean {
  return waylandSession
}

// Прозрачность окна — решение, отдельное от хоткеев. Пакетный враппер может
// форсировать GDK_BACKEND=x11 в Wayland-сессии: тогда глобальные клавиши и
// «поверх всех» работают (waylandSession=false), но прозрачное окно
// webkit2gtk на XWayland + NVIDIA перерисовывается только там, где движется
// курсор («двинешь мышкой — пропадает»). App.vue включает этот флаг
// превентивно для связки NVIDIA + Wayland-сессия с форсированным X11.
let overlayOpaqueForced = false

export function setOverlayOpaqueWindow(value: boolean): void {
  overlayOpaqueForced = value
}

export function overlayOpaqueWindowForced(): boolean {
  return overlayOpaqueForced
}

// Режим, в котором будет создано окно. Нативный Wayland и рискованные
// окружения получают совместимый непрозрачный план сразу, а не после
// неудачной попытки с прозрачным окном.
function overlayWindowMode(): 'x11' | 'wayland' | 'fallback' {
  if (waylandSession) return 'wayland'
  return overlayOpaqueFallback || overlayOpaqueForced ? 'fallback' : 'x11'
}

// Включён ли аварийный непрозрачный режим окна (создание прозрачного не удалось).
export function overlayUsingFallback(): boolean {
  return overlayOpaqueFallback
}

// Событие tauri:created может НЕ прийти вовсе (особенность WebKitGTK и/или
// гонка: слушатель подключается позже, чем окно уже создано). Раньше промис
// создания висел вечно — клик по кнопке «Оверлей» «ничего не делал» без
// единого сообщения. Теперь параллельно опрашиваем само окно: первый же
// успешный ответ IPC означает, что окно живо.
async function waitForWindowIpc(overlay: WebviewWindow, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      await overlay.isVisible()
      return
    } catch {
      // окно ещё не существует — пробуем ещё раз через 150 мс
    }
    await new Promise((resolve) => setTimeout(resolve, 150))
  }
  throw new Error(`окно не ответило за ${Math.round(timeoutMs / 1000)} с (событие создания не пришло)`)
}

// Каждая попытка открыть оверлей дописывается в журнал на диске
// (data_home/nivora-launcher/logs/overlay.log) — команда возвращает путь
// и хвост, чтобы отчёт «Диагностика оверлея» показывал факты.
export async function overlayAttemptLog(entry: string): Promise<{ path: string; tail: string } | null> {
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
  // При превентивно-непрозрачном режиме пишем причину в журнал: без неё
  // строка «работает в совместимом режиме» в отчёте необъяснима.
  const reason = mode === 'fallback' && overlayOpaqueForced && !overlayOpaqueFallback
    ? ' · причина: NVIDIA + форсированный X11 в Wayland-сессии (прозрачное окно на XWayland перерисовывается только под курсором)'
    : ''
  void overlayAttemptLog(`создание окна: режим=${mode} ${plan.width}x${plan.height}@${plan.x ?? 'центр'},${plan.y ?? ''} transparent=${plan.transparent} alwaysOnTop=${plan.alwaysOnTop}${reason}`)
  return new Promise<WebviewWindow>((resolve, reject) => {
    // Окно создаётся сразу видимым: у WebKitGTK связка «создать скрытым,
    // потом показать» иногда оставляет пустой кадр или теряет событие
    // показа — а «мигнуть» оверлеем не страшно.
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
    // Страховка от молчаливого зависания (см. waitForWindowIpc).
    void waitForWindowIpc(overlay, 9000).then(() => settle(), (error: Error) => settle(error))
    // На режимах с рамкой у окна есть крестик: после такого закрытия само
    // окно уничтожено, и если не сбросить состояние, следующее нажатие
    // горячей клавиши «прятало» уже несуществующее окно.
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
        // В нативном Wayland и при превентивно-непрозрачном режиме окно уже
        // создаётся максимально совместимым — повторять нечем. Иначе одна
        // попытка с непрозрачным окном всё же лучше, чем «никак»:
        // прозрачность на XWayland/NVIDIA подводит.
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

// Геометрия для создания окна: текущий монитор →primary → запасные 1280×800.
async function initialOverlayGeometry(): Promise<{ width: number; height: number; x: number; y: number }> {
  try {
    const monitor = (await currentMonitor()) || (await primaryMonitor())
    if (monitor) {
      // size/position монитора — физические пиксели, а width/height/x/y при
      // создании окна — логические. Без деления на scale HiDPI-экран
      // получал окно в полтора-два раза больше самого монитора.
      const scale = monitor.scaleFactor && monitor.scaleFactor > 0 ? monitor.scaleFactor : 1
      return {
        width: Math.max(640, Math.round(monitor.size.width / scale)),
        height: Math.max(480, Math.round(monitor.size.height / scale)),
        x: Math.round(monitor.position.x / scale),
        y: Math.round(monitor.position.y / scale),
      }
    }
  } catch {
    // без разрешения на API мониторов окно создаётся с запасным размером
  }
  return { width: 1280, height: 800, x: 0, y: 0 }
}

// Перед показом растягиваем окно на текущий монитор. Дешёвая операция,
// зато оверлей не «застревает» в размере монитора, который был на момент
// создания окна (перетащили игру на другой экран — размер подтянется).
async function coverActiveMonitor(overlay: WebviewWindow): Promise<void> {
  try {
    const monitor = await currentMonitor()
    if (!monitor) return
    await overlay.setSize(new PhysicalSize(monitor.size.width, monitor.size.height))
    await overlay.setPosition(new PhysicalPosition(monitor.position.x, monitor.position.y))
  } catch {
    // остались при прежней геометрии — не критично
  }
}

export function overlayIsVisible(): boolean {
  return overlayVisible
}

// Окно оверлея — отдельный webview со своим JS-контекстом. Esc внутри него
// приходит событием, а прячет окно главный контекст: только он знает,
// «видим» ли оверлей сейчас.
export async function initOverlayEventBridge(onTrayOpen?: () => void, onInAppClose?: () => void): Promise<void> {
  if (!inTauri() || closeListener) return
  try {
    closeListener = await listen('nivora-overlay-close', () => {
      void setOverlayVisible(false)
      // Внутрилаунчерный оверлей (Wayland-костыль) закрывается тем же Esc.
      if (onInAppClose) onInAppClose()
    })
  } catch {
    closeListener = null
  }
  // Пункт «Игровой оверлей» в меню трея: работает даже там, где глобальные
  // клавиши не поддерживаются (Wayland-сессии).
  try {
    await listen('nivora-overlay-open', () => {
      if (onTrayOpen) onTrayOpen()
      else void toggleOverlay()
    })
  } catch {
    // пункт меню останется без действия — не критично
  }
}

// Возвращает true, если окно действительно показалось/спряталось. Ошибки
// больше не глотаем молча: текст доступен через overlayLastError().
export async function setOverlayVisible(visible: boolean): Promise<boolean> {
  overlayVisible = visible
  if (!inTauri()) return visible
  const wayland = overlayWaylandMode()
  try {
    const overlay = await ensureOverlayWindow()
    if (visible) {
      const started = Date.now()
      await overlay.show()
      let shown = await overlay.isVisible().catch(() => true)
      if (!shown) {
        // Повтор после паузы: WebKitGTK иногда «догоняет» показ окна.
        await new Promise((resolve) => setTimeout(resolve, 250))
        await overlay.show().catch(() => undefined)
        shown = await overlay.isVisible().catch(() => true)
      }
      if (!shown) throw new Error('окно оверлея не отобразилось')
      // Дальше — только улучшения позиции и поведения. Любое из них может
      // не поддерживаться окружением; сбой не должен «закрывать» окно,
      // которое уже открыто и работает.
      if (!wayland) {
        await coverActiveMonitor(overlay)
        await overlay.setAlwaysOnTop(true).catch(() => undefined)
      }
      await overlay.setFocus().catch(() => undefined)
      lastOverlayError = ''
      void overlayAttemptLog(`окно показано за ${Date.now() - started} мс (режим ${overlayWindowMode()})`)
      return true
    }
    await overlay.hide()
    return true
  } catch (error) {
    overlayVisible = false
    lastOverlayError = error instanceof Error ? error.message : String(error)
    void overlayAttemptLog(`показ НЕ удался: ${lastOverlayError}`)
    // окно могло закрыться снаружи — сбросим кэш и пересоздадим при
    // следующем открытии
    overlayWindowPromise = null
    return false
  }
}

export async function toggleOverlay(): Promise<boolean> {
  await setOverlayVisible(!overlayVisible)
  return overlayVisible
}

// Привязка/перепривязка горячих клавиш сериализуется: глубокий вачер
// настроек вызывает bindGameHotkeys на каждое изменение, и параллельные
// вызовы успевали зарегистрировать И старую, И новую комбинацию — оверлей
// закрывался обеими.
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
  // В Wayland-сессии глобальный перехват клавиш (XGrabKey) не работает:
  // compositor не отдаёт события чужим клиентам. Оверлей Steam работает,
  // потому что внедряется в процесс игры — для нас этот путь закрыт.
  // Не дёргаем register впустую, а честно отдаём комбинации как
  // «недоступные глобально»: их по-прежнему ловит окно лаунчера (см.
  // handleGlobalKeydown в App.vue) и пункт в меню трея.
  if (waylandSession) return bindings.map(([combination]) => combination)
  const failed: string[] = []
  for (const [combination, handler] of bindings) {
    try {
      await register(combination, (event) => {
        if (event.state === 'Pressed') handler()
      })
      hotkeysBound = true
    } catch {
      // комбинация может быть занята другой программой — раньше это
      // молчало и выглядело как «оверлей не открывается»; теперь сообщаем
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
      // ignore
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
    // нет ответа — попробуем просто запустить
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

// Человекочитаемое имя режима для самодиагностики: важно не только
// «непрозрачный», но и почему — превентивно (NVIDIA + XWayland) или после
// неудачной попытки создать прозрачное окно.
function describeOverlayWindowMode(): string {
  const mode = overlayWindowMode()
  if (mode === 'x11') return 'обычный (прозрачный, поверх всех)'
  if (mode === 'wayland') return 'совместимый (непрозрачный) — нативный Wayland'
  if (overlayOpaqueForced) return 'совместимый (непрозрачный), принудительно: NVIDIA + форсированный X11 в Wayland-сессии — прозрачное окно на XWayland перерисовывалось бы только под курсором'
  return 'совместимый (непрозрачный) — прозрачное окно не создалось с первой попытки'
}

// Пошаговая самопроверка оверлея: превращает «он не открывается» в
// конкретный отчёт, который можно прочитать и переслать. Каждый шаг
// независим — сбой одного не мешает проверить остальные.
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
    // оставляем запасной текст
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
    // текст ошибки — в шаге «Показ»
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
