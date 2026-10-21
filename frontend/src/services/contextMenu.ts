// контекстное меню лаунчера — своё, вместо системного WebView-меню
import { reactive } from 'vue'

export interface ContextMenuItem {
  id: string
  label: string
  icon?: string
  danger?: boolean
  disabled?: boolean
  separatorAfter?: boolean
  action: () => void
}

interface ContextMenuState {
  open: boolean
  x: number
  y: number
  title: string
  items: ContextMenuItem[]
}

export const contextMenu = reactive<ContextMenuState>({
  open: false,
  x: 0,
  y: 0,
  title: '',
  items: [],
})

let closeOnOutside: ((event: MouseEvent) => void) | null = null

function scheduleCloseOnOutside() {
  if (closeOnOutside) return
  closeOnOutside = (event: MouseEvent) => {
    const target = event.target as HTMLElement | null
    if (target?.closest('.context-menu')) return
    closeContextMenu()
  }
  // mousedown, а не click: меню должно закрываться и от клика по кнопке,
  // которая его открыла (иначе toggle выглядит залипанием).
  window.addEventListener('mousedown', closeOnOutside, { capture: true })
}

export function closeContextMenu() {
  contextMenu.open = false
  contextMenu.items = []
  contextMenu.title = ''
  if (closeOnOutside) {
    window.removeEventListener('mousedown', closeOnOutside, { capture: true })
    closeOnOutside = null
  }
}

/**
 * Открыть меню у курсора. Координаты — от события contextmenu; к краю окна
 * меню прижимается уже в компоненте, чтобы не вылезти за пределы.
 */
export function openContextMenu(event: MouseEvent, items: ContextMenuItem[], title = '') {
  if (!items.length) return
  event.preventDefault()
  event.stopPropagation()
  contextMenu.x = event.clientX
  contextMenu.y = event.clientY
  contextMenu.title = title
  contextMenu.items = items
  contextMenu.open = true
  scheduleCloseOnOutside()
}

/** Пункты «редактирования текста» для полей ввода — вместо системного меню. */
export function textEditMenu(target: HTMLElement): ContextMenuItem[] {
  const field = target.closest('input, textarea') as HTMLInputElement | HTMLTextAreaElement | null
  if (!field) return []
  const edit = (command: string) => () => {
    field.focus()
    document.execCommand(command)
  }
  const items: ContextMenuItem[] = [
    { id: 'cut', label: 'Вырезать', icon: 'close', action: edit('cut'), disabled: field.selectionStart === field.selectionEnd },
    { id: 'copy', label: 'Копировать', icon: 'copy', action: edit('copy'), disabled: field.selectionStart === field.selectionEnd },
    { id: 'paste', label: 'Вставить', icon: 'paste', action: edit('paste') },
    { id: 'select', label: 'Выделить всё', icon: 'check', action: () => field.select(), separatorAfter: true },
  ]
  if (field.type === 'password') {
    // в паролях вырезать/копировать не имеют смысла.
    return [items[3]]
  }
  return items
}
