<script setup lang="ts">
import paperLogo from '../assets/papermc_logo.min.svg'

const props = withDefaults(defineProps<{ core?: string; name?: string; compact?: boolean }>(), { core: 'custom', name: '', compact: false })

const meta: Record<string, { label: string; className: string }> = {
  paper: { label: 'Paper', className: 'core-badge--paper' },
  purpur: { label: 'Purpur', className: 'core-badge--purpur' },
  folia: { label: 'Folia', className: 'core-badge--folia' },
  spigot: { label: 'Spigot', className: 'core-badge--spigot' },
  leaf: { label: 'Leaf', className: 'core-badge--leaf' },
  pufferfish: { label: 'Pufferfish', className: 'core-badge--pufferfish' },
  patina: { label: 'Patina', className: 'core-badge--patina' },
  velocity: { label: 'Velocity', className: 'core-badge--velocity' },
  waterfall: { label: 'Waterfall', className: 'core-badge--waterfall' },
  custom: { label: 'Custom server JAR', className: 'core-badge--custom' },
}

const current = () => meta[props.core || 'custom'] || meta.custom
</script>

<template>
  <span class="core-badge" :class="[current().className, { 'core-badge--compact': compact }]" role="img" v-tooltip="name || current().label" :aria-label="name || current().label">
    <span class="core-badge-mark">
      <img v-if="core === 'paper'" :src="paperLogo" alt="" />
      <svg v-else viewBox="0 0 24 24" aria-hidden="true" focusable="false">
        <g v-if="core === 'purpur'">
          <path d="M12 2.8 20.5 7.6v8.8L12 21.2 3.5 16.4V7.6L12 2.8Z" />
          <path d="m3.8 7.8 8.2 4.7 8.2-4.7M12 12.5v8.2M12 3v9.5" />
        </g>
        <g v-else-if="core === 'folia'">
          <path d="M12 11.8c-1.8-5.2.8-8.2 5.4-8.8 1 4.4-.4 7.9-5.4 8.8ZM12.2 12c5.2-1.8 8.2.8 8.8 5.4-4.4 1-7.9-.4-8.8-5.4ZM12 12.2c1.8 5.2-.8 8.2-5.4 8.8-1-4.4.4-7.9 5.4-8.8ZM11.8 12c-5.2 1.8-8.2-.8-8.8-5.4 4.4-1 7.9.4 8.8 5.4Z" />
          <path d="M12 12v8" />
        </g>
        <g v-else-if="core === 'spigot'">
          <path d="M4 5v4m0-2h9.5a3 3 0 0 1 3 3v1H20v4h-7v-2H7v3.5a3 3 0 1 0 6 0V16" />
          <path d="M2.5 5h3m-3 2h3" />
        </g>
        <g v-else-if="core === 'leaf'">
          <path d="M20.5 4.2C11.3 3.4 5.1 6.6 5.1 13.2c0 3.4 2 5.7 5.5 5.7 6.4 0 10-5.6 9.9-14.7Z" />
          <path d="M4 20c3.3-5.3 7.2-8.4 12.3-10.7" />
        </g>
        <g v-else-if="core === 'pufferfish'">
          <path d="m7 7-2-3m7 2V3m5 6 3-3M6 10l-3-1m3 5-3 2m5 2-1 3m6-3 1 3m3-8 3 1m-3-5 3-2" />
          <path d="M7 7c-2 1-3 3-3 5 0 4 3 7 7 7 3 0 5-2 6-4l3-2-3-2c-1-3-3-5-7-5-1 0-2 0-3 1Z" />
          <circle class="core-badge-eye" cx="13.5" cy="10.5" r=".75" />
        </g>
        <g v-else-if="core === 'patina'">
          <path d="M12 3 19 6v5c0 4-2.8 7.3-7 10-4.2-2.7-7-6-7-10V6z" />
          <path d="M8.5 10.5c.5-1 1.3-1.5 2.4-1.5m.2 5c1.6 1 3.1.8 4.4-.4" />
          <circle cx="15.4" cy="8.7" r=".65" />
        </g>
        <g v-else-if="core === 'velocity'">
          <path d="m13.5 2.5-8 10.2h5l-1 8.8 9-12h-5l1-7Z" />
          <path d="M3 8h3m-4 4h3m-2 4h3" />
        </g>
        <g v-else-if="core === 'waterfall'">
          <path d="M4 5c2 0 2 2 4 2s2-2 4-2M4 11c2 0 2 2 4 2s2-2 4-2M4 17c2 0 2 2 4 2s2-2 4-2M17 4v10a3 3 0 0 0 6 0" />
        </g>
        <g v-else>
          <path d="M9 3h6m-5 0v4l-4 3v10h12V10l-4-3V3M6 13h12" />
        </g>
      </svg>
    </span>
    <span v-if="!compact && name" class="core-badge-name">{{ name }}</span>
  </span>
</template>
