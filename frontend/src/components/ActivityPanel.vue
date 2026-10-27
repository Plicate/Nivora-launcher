<script setup lang="ts">
import { ref } from 'vue'
import Icon from './Icon.vue'
import QrCode from './QrCode.vue'
import { bridge } from '../services/bridge'
import type { ActivityState } from '../types'

const props = defineProps<{
  activity: ActivityState
}>()

const emit = defineEmits<{ close: []; cancel: [] }>()
// ссылки из встроенного окна открываем через бэкенд
const openFailed = ref(false)
const copyState = ref<'idle' | 'done' | 'failed'>('idle')
async function openVerificationUri(): Promise<void> {
  const url = props.activity.verification_uri
  if (!url) return
  openFailed.value = false
  try {
    await bridge.openExternal(url)
  } catch {
    openFailed.value = true
  }
}

/// быстрое копирование содержимого маркера работы: заголовок, статус и лог
async function copyActivitySummary(): Promise<void> {
  const text = [
    props.activity.title,
    props.activity.detail,
    ...(props.activity.verification_code ? [`Код Microsoft: ${props.activity.verification_code}`] : []),
    ...props.activity.logs,
  ].join('\n')
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
    } else {
      const area = document.createElement('textarea')
      area.value = text
      area.style.position = 'fixed'
      area.style.opacity = '0'
      document.body.appendChild(area)
      area.select()
      document.execCommand('copy')
      area.remove()
    }
    copyState.value = 'done'
  } catch {
    copyState.value = 'failed'
  }
  window.setTimeout(() => { copyState.value = 'idle' }, 1600)
}
</script>

<template>
  <Transition name="activity">
    <aside v-if="activity.open" class="activity-panel">
      <div class="activity-header">
        <div>
          
          <h2>{{ activity.title }}</h2>
        </div>
         <div class="activity-header-actions">
           <button
             class="icon-button"
             type="button"
             :aria-label="copyState === 'done' ? 'Скопировано' : 'Копировать содержимое'"
             :title="copyState === 'done' ? 'Скопировано' : copyState === 'failed' ? 'Не удалось скопировать' : 'Копировать содержимое'"
             @click="copyActivitySummary"
           >
             <Icon :name="copyState === 'done' ? 'check' : 'copy'" :size="16" />
           </button>
           <button v-if="activity.busy" class="button button--small button--outline" type="button" :disabled="activity.cancelling" @click="emit('cancel')">
             {{ activity.cancelling ? 'Останавливаю…' : 'Остановить' }}
           </button>
           <button class="icon-button" type="button" aria-label="Закрыть" @click="emit('close')">
             <Icon name="close" :size="18" />
           </button>
         </div>
      </div>
      <div class="activity-state">
        <span class="activity-pulse" :class="{ 'activity-pulse--done': !activity.busy }" />
        <div>
          <strong>{{ activity.busy ? 'Работаю над задачей' : 'Готово' }}</strong>
          <span>{{ activity.detail }}</span>
        </div>
      </div>
      <div class="progress-track" :class="{ 'progress-track--indeterminate': activity.busy && activity.progress <= 0 }">
        <span :style="{ width: `${activity.progress}%` }" />
      </div>
      <div v-if="activity.verification_code" class="verification-card">
        
        <strong>{{ activity.verification_code }}</strong>
        <div class="verification-qr">
          <QrCode :value="activity.verification_uri || 'https://microsoft.com/devicelogin'" :size="132" label="QR-код страницы входа Microsoft" />
          <small class="muted">Отсканируйте другим устройством и введите код выше.</small>
        </div>
        <button v-if="activity.verification_uri" class="button button--small button--outline" type="button" @click="openVerificationUri">
          {{ openFailed ? 'Открыть снова' : 'Открыть страницу входа' }}
        </button>
        <p v-if="openFailed && activity.verification_uri" class="muted verification-url">{{ activity.verification_uri }}</p>
      </div>
      <div class="activity-log">
        <p v-if="!activity.logs.length" class="muted">Подробности появятся здесь.</p>
        <p v-for="(line, index) in activity.logs" :key="`${index}-${line}`">{{ line }}</p>
      </div>
    </aside>
  </Transition>
</template>
