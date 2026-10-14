import { nextTick, onBeforeUnmount, watch, type Ref } from 'vue'

const FOCUSABLE = 'a[href], button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])'

export function useModalFocus(
  open: Readonly<Ref<boolean>>,
  dialog: Readonly<Ref<HTMLElement | null>>,
  onEscape?: () => void,
) {
  let returnFocusElement: HTMLElement | null = null
  let capturedFocus = false

  function restoreFocus() {
    if (!capturedFocus) return
    capturedFocus = false
    const target = returnFocusElement
    returnFocusElement = null
    if (target?.isConnected) target.focus()
  }

  watch([open, dialog], async ([active]) => {
    if (!active) {
      restoreFocus()
      return
    }
    if (!capturedFocus && typeof document !== 'undefined') {
      capturedFocus = true
      returnFocusElement = document.activeElement instanceof HTMLElement ? document.activeElement : null
    }
    await nextTick()
    if (!open.value) return
    const root = dialog.value
    if (!root) return
    const initialTarget = root.querySelector<HTMLElement>(FOCUSABLE)
    ;(initialTarget || root).focus()
  }, { flush: 'post', immediate: true })

  onBeforeUnmount(restoreFocus)

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault()
      onEscape?.()
      return
    }
    if (event.key !== 'Tab') return

    const root = dialog.value
    const focusable = Array.from(root?.querySelectorAll<HTMLElement>(FOCUSABLE) || [])
      .filter((element) => element.getAttribute('aria-hidden') !== 'true')
    if (!focusable.length) {
      event.preventDefault()
      root?.focus()
      return
    }

    const first = focusable[0]
    const last = focusable[focusable.length - 1]
    if (event.shiftKey && (document.activeElement === first || !root?.contains(document.activeElement))) {
      event.preventDefault()
      last.focus()
    } else if (!event.shiftKey && (document.activeElement === last || !root?.contains(document.activeElement))) {
      event.preventDefault()
      first.focus()
    }
  }

  return onKeydown
}
