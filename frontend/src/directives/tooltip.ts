// кастомные тултипы вместо нативных title: системные подсказки рисуются
import type { Directive } from 'vue'

let tooltipEl: HTMLElement | null = null
let showTimer: number | undefined
let hideTimer: number | undefined
let currentAnchor: HTMLElement | null = null
let currentText = ''

function ensureTooltip(): HTMLElement {
  if (!tooltipEl) {
    tooltipEl = document.createElement('div')
    tooltipEl.className = 'app-tooltip'
    tooltipEl.setAttribute('role', 'tooltip')
    tooltipEl.style.display = 'none'
    document.body.appendChild(tooltipEl)
  }
  return tooltipEl
}

function clearTimers(): void {
  window.clearTimeout(showTimer)
  window.clearTimeout(hideTimer)
}

function place(anchor: HTMLElement): void {
  const tooltip = ensureTooltip()
  const rect = anchor.getBoundingClientRect()
  tooltip.style.left = '0px'
  tooltip.style.top = '0px'
  const box = tooltip.getBoundingClientRect()
  const margin = 10
  let left = rect.left + rect.width / 2 - box.width / 2
  left = Math.max(margin, Math.min(left, window.innerWidth - box.width - margin))
  const fitsBelow = rect.bottom + box.height + margin + 4 < window.innerHeight
  const top = fitsBelow ? rect.bottom + 8 : Math.max(margin, rect.top - box.height - 8)
  tooltip.style.left = `${Math.round(left)}px`
  tooltip.style.top = `${Math.round(top)}px`
  tooltip.classList.toggle('app-tooltip--above', !fitsBelow)
}

function show(anchor: HTMLElement, text: string): void {
  clearTimers()
  currentAnchor = anchor
  currentText = text
  showTimer = window.setTimeout(() => {
    if (!currentAnchor || !currentText) return
    const tooltip = ensureTooltip()
    tooltip.textContent = currentText
    tooltip.style.display = 'block'
    place(currentAnchor)
    tooltip.classList.add('app-tooltip--visible')
  }, 240)
}

function hideSoon(): void {
  clearTimers()
  hideTimer = window.setTimeout(() => {
    const tooltip = tooltipEl
    if (!tooltip) return
    tooltip.classList.remove('app-tooltip--visible')
    window.setTimeout(() => {
      if (tooltipEl && !tooltipEl.classList.contains('app-tooltip--visible')) tooltipEl.style.display = 'none'
    }, 130)
    currentAnchor = null
    currentText = ''
  }, 60)
}

function text(value: unknown): string {
  return typeof value === 'string' ? value.trim() : ''
}

export const vTooltip: Directive<HTMLElement, string | undefined> = {
  mounted(element, binding) {
    let current = text(binding.value)
    if (!current) return
    const enter = () => {
      if (!current) return
      show(element, current)
    }
    const leave = () => hideSoon()
    // binding в этом замыкании мог устареть — актуальное значение приходит
    // с событием из updated()
    const update = (event: Event) => {
      current = text((event as CustomEvent).detail)
      if (currentAnchor === element) {
        if (!current) {
          hideSoon()
        } else if (tooltipEl && tooltipEl.style.display !== 'none') {
          currentText = current
          tooltipEl.textContent = current
          place(element)
        }
      }
    }
    element.addEventListener('mouseenter', enter)
    element.addEventListener('mouseleave', leave)
    element.addEventListener('focusin', enter)
    element.addEventListener('focusout', leave)
    element.addEventListener('update-tooltip', update)
  },
  updated(element, binding) {
    element.dispatchEvent(new CustomEvent('update-tooltip', { detail: binding.value }))
  },
  unmounted(element) {
    if (currentAnchor === element) hideSoon()
  },
}
