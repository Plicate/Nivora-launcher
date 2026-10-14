<script setup lang="ts">
import { ref } from 'vue'
import type { LoaderId } from '../types'

const props = withDefaults(
  defineProps<{
    loader: LoaderId
    size?: 'sm' | 'md' | 'lg'
  }>(),
  { size: 'md' },
)

const failed = ref(false)
const label = props.loader === 'vanilla' ? 'V' : props.loader.slice(0, 1).toUpperCase()
</script>

<template>
  <span class="loader-badge" :class="[`loader-badge--${size}`, `loader-badge--${loader}`]">
    <img
      v-if="!failed && loader !== 'vanilla'"
      :src="`/loaders/${loader}.png`"
      :alt="`${loader} logo`"
      @error="failed = true"
    />
    <span v-else class="loader-fallback">{{ label }}</span>
  </span>
</template>
