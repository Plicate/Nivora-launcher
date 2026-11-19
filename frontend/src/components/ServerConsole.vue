<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import Icon from './Icon.vue'
const props = defineProps<{ logs: string[]; running: boolean; command: string; busy?: boolean; expanded?: boolean }>()
const emit = defineEmits<{ (event: 'update:command', value: string): void; (event: 'send'): void }>()
const viewport = ref<HTMLElement | null>(null)
const follow = ref(true)
function scrolled() {
  if (viewport.value) follow.value = viewport.value.scrollHeight - viewport.value.scrollTop - viewport.value.clientHeight < 24
}
async function scrollToBottom() {
  await nextTick()
  if (viewport.value && follow.value) viewport.value.scrollTop = viewport.value.scrollHeight
}
watch(() => props.logs, () => void scrollToBottom(), { immediate: true })
function change(event: Event) { emit('update:command', (event.target as HTMLInputElement).value) }
</script>
<template>
  <section class="workspace-card server-console" :class="{ 'server-console--expanded': expanded }">
    <div class="workspace-card-heading"><div><h3>Консоль сервера</h3><span class="muted">Запуск, ошибки и команды — на главной странице сервера</span></div><span class="tag" :class="{ 'tag--success': running }">{{ running ? 'Процесс работает' : 'Остановлен' }}</span></div>
    <div class="console-viewport"><pre ref="viewport" aria-live="off" @scroll="scrolled">{{ logs.length ? logs.join('\n') : 'Вывод сервера появится здесь после запуска. Ошибки запуска также сохраняются в консоли.' }}</pre><button v-if="logs.length && !follow" class="console-jump-button" type="button" @click="follow = true; void scrollToBottom()"><Icon name="arrow-down" :size="14" />К последней строке</button></div>
    <form class="server-command" @submit.prevent="emit('send')"><input :value="command" :disabled="!running || busy" placeholder="Команда сервера: list, say Привет, whitelist add Ник" aria-label="Команда сервера" @input="change" /><button class="button button--small button--accent" type="submit" :disabled="!running || busy || !command.trim()">Отправить</button></form>
  </section>
</template>
<style scoped>
.server-console--expanded pre { min-height: 440px; max-height: 65vh; }
</style>
