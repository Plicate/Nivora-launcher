import { createApp } from 'vue'
import { vTooltip } from './directives/tooltip'
import { overlayWindowRequested } from './services/overlayHelpers'
import './styles.css'

async function mountApp() {
  if (overlayWindowRequested()) {
    const { default: GameOverlay } = await import('./components/GameOverlay.vue')
    createApp(GameOverlay).directive('tooltip', vTooltip).mount('#app')
    return
  }

  const [{ default: App }, contextMenu] = await Promise.all([
    import('./App.vue'),
    import('./services/contextMenu'),
  ])
  await import('./visual-refresh.css')

  window.addEventListener(
    'contextmenu',
    (event) => {
      const target = event.target as Element | null
      if (target) {
        const editable = target.closest('input, textarea') as HTMLElement | null
        if (editable) {
          contextMenu.openContextMenu(event, contextMenu.textEditMenu(editable))
          return
        }
        const link = target.closest('a[href]')
        if (link) {
          const href = link.getAttribute('href') || ''
          if (href.startsWith('http')) {
            contextMenu.openContextMenu(event, [{
              id: 'copy-link',
              label: 'Копировать ссылку',
              icon: 'copy',
              action: () => void navigator.clipboard?.writeText(href),
            }])
            return
          }
        }
      }
      const selection = window.getSelection()
      if (selection && !selection.isCollapsed && selection.rangeCount > 0) {
        const range = selection.getRangeAt(0)
        if (target && range.intersectsNode(target as Node) && String(selection)) {
          const text = String(selection)
          contextMenu.openContextMenu(event, [{
            id: 'copy-selection',
            label: 'Копировать',
            icon: 'copy',
            action: () => void navigator.clipboard?.writeText(text),
          }])
          return
        }
      }
      contextMenu.closeContextMenu()
      event.preventDefault()
    },
    { capture: true },
  )

  createApp(App).directive('tooltip', vTooltip).mount('#app')
}

void mountApp()
