<script setup lang="ts">
import { ref, watch } from 'vue'
import QRCode from 'qrcode'

const props = withDefaults(defineProps<{ value: string; size?: number; label?: string }>(), {
  size: 168,
  label: 'QR-код ссылки',
})

const svg = ref('')

watch(
  () => props.value,
  async (value) => {
    if (!value) {
      svg.value = ''
      return
    }
    try {
      svg.value = await QRCode.toString(value, {
        type: 'svg',
        errorCorrectionLevel: 'M',
        margin: 2,
        color: { dark: '#0b1220', light: '#ffffff' },
      })
    } catch {
      svg.value = ''
    }
  },
  { immediate: true },
)
</script>

<template>
  <div
    v-if="svg"
    class="qr-code"
    role="img"
    :aria-label="label"
    :style="{ width: `${size}px`, height: `${size}px` }"
    v-html="svg"
  />
</template>

<style scoped>
.qr-code {
  flex: none;
  padding: 6px;
  border-radius: 10px;
  background: #ffffff;
  box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.08);
}
.qr-code :deep(svg) {
  display: block;
  width: 100%;
  height: 100%;
}
</style>
