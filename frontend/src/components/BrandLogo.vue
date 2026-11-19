<script setup lang="ts">
import { computed } from 'vue'
import logo from '../assets/uni-black.png'

const props = withDefaults(defineProps<{ label?: string; color?: string }>(), { label: 'Nivora Launcher', color: '' })

const style = computed<Record<string, string>>(() => ({
  '--brand-logo-image': `url("${logo}")`,
  ...(props.color ? { '--brand-color': props.color } : {}),
}))
</script>

<template>
  <span class="brand-logo" :style="style" role="img" :aria-label="label" />
</template>
