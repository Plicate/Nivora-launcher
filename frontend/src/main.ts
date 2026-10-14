import { createApp } from 'vue'
import App from './App.vue'
import { vTooltip } from './directives/tooltip'
import './styles.css'
import './visual-refresh.css'

createApp(App).directive('tooltip', vTooltip).mount('#app')
