import { createApp } from 'vue'
import App from './App.vue'
import { vTooltip } from './directives/tooltip'
import { closeContextMenu, textEditMenu, openContextMenu } from './services/contextMenu'
import './styles.css'
import './visual-refresh.css'

window.addEventListener(
  'contextmenu',
  (event) => {
    const target = event.target as Element | null
    if (target) {
      const editable = target?.closest('input, textarea') as HTMLElement | null
      if (editable) {
        openContextMenu(event, textEditMenu(editable))
        return
      }
      const link = target?.closest('a[href]')
      if (link) {
        const href = link.getAttribute('href') || ''
        if (href.startsWith('http')) {
          openContextMenu(event, [{
            id: 'copy-link',
            label: 'Копировать ссылку',
            icon: 'copy',
            action: () => void navigator.clipboard?.writeText(href),
          }])
          return
        }
      }
    }
    // клик «по пустому месту» лаунчера: закрываем возможное открытое меню,
    // а системное не показываем вовсе.
    closeContextMenu()
    event.preventDefault()
  },
  { capture: true },
)

createApp(App).directive('tooltip', vTooltip).mount('#app')
