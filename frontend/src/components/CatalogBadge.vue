<script setup lang="ts">
import modrinthLogo from '../assets/modrinth.svg'

const props = withDefaults(defineProps<{ kind?: string; name?: string; compact?: boolean }>(), { kind: 'custom', name: '', compact: false })

const meta: Record<string, { label: string; className: string }> = {
  modrinth: { label: 'M', className: 'catalog-badge--modrinth' },
  ftb: { label: 'FTB', className: 'catalog-badge--ftb' },
  technic: { label: 'T', className: 'catalog-badge--technic' },
  custom: { label: 'API', className: 'catalog-badge--custom' },
}

const current = () => meta[props.kind || 'custom'] || meta.custom
</script>

<template>
  <span class="catalog-badge" :class="[current().className, { 'catalog-badge--compact': compact }]" v-tooltip="name || current().label">
    <img v-if="kind === 'modrinth'" :src="modrinthLogo" alt="" />
    <span v-else class="catalog-badge-mark">{{ current().label }}</span>
    <span v-if="!compact && name" class="catalog-badge-name">{{ name }}</span>
  </span>
</template>
