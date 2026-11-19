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
  // в capture phase закрытие сработает на клике, который открыл меню
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

/** открывает меню у курсора; компонент ограничивает его окном */
export function openContextMenu(event: MouseEvent, items: ContextMenuItem[], title = '') {
  openContextMenuAt(event.clientX, event.clientY, items, title)
  event.preventDefault()
  event.stopPropagation()
}

/** открывает меню в заданной точке, например при выборе клавишей Enter */
export function openContextMenuAt(x: number, y: number, items: ContextMenuItem[], title = '') {
  if (!items.length) return
  contextMenu.x = x
  contextMenu.y = y
  contextMenu.title = title
  contextMenu.items = items
  contextMenu.open = true
  scheduleCloseOnOutside()
}

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
    // не показываем операции чтения и изменения для пароля
    return [items[3]]
  }
  return items
}
