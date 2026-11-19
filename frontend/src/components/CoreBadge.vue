<script setup lang="ts">
import paperLogo from '../assets/loaders/paper_official.png'
import foliaLogo from '../assets/loaders/folia.png'
import purpurLogo from '../assets/loaders/purpur.png'
import spigotLogo from '../assets/loaders/spigot.png'
import velocityLogo from '../assets/loaders/velocity.svg'
import leafLogo from '../assets/loaders/leaf.png'
import pufferfishLogo from '../assets/loaders/pufferfish.png'
import patinaLogo from '../assets/loaders/patina.png'

const props = withDefaults(defineProps<{ core?: string; name?: string; compact?: boolean }>(), { core: 'custom', name: '', compact: false })

const logos: Record<string, string> = {
  paper: paperLogo,
  folia: foliaLogo,
  purpur: purpurLogo,
  spigot: spigotLogo,
  velocity: velocityLogo,
  leaf: leafLogo,
  pufferfish: pufferfishLogo,
  patina: patinaLogo,
}

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
const logo = () => logos[(props.core || 'custom').toLowerCase()] || ''
</script>

<template>
  <span class="core-badge" :class="[current().className, { 'core-badge--compact': compact }]" role="img" v-tooltip="name || current().label" :aria-label="name || current().label">
    <span class="core-badge-mark">
      <img v-if="logo()" :src="logo()" alt="" loading="lazy" />
      <svg v-else viewBox="0 0 24 24" aria-hidden="true" focusable="false">
        <!-- закрытое и неизвестное ядра получают нейтральные метки -->
        <g v-if="core === 'waterfall'">
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
