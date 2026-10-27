<script setup lang="ts">
import type { LoaderId } from '../types'
// официальные глифы загрузчиков из дизайн-системы Modrinth (/v2/tag/loader):
// каждый лоадер имеет настоящий логотип, монохром подстраивается под тему
import fabricLogo from '../assets/loaders/fabric.svg'
import forgeLogo from '../assets/loaders/forge.svg'
import neoforgeLogo from '../assets/loaders/neoforge.svg'
import quiltLogo from '../assets/loaders/quilt.svg'
import liteloaderLogo from '../assets/loaders/liteloader.svg'
import riftLogo from '../assets/loaders/rift.svg'
import modloaderLogo from '../assets/loaders/modloader.svg'
import bukkitLogo from '../assets/loaders/bukkit.svg'
import spigotLogo from '../assets/loaders/spigot.svg'
import paperLogo from '../assets/loaders/paper.svg'
import purpurLogo from '../assets/loaders/purpur.svg'
import foliaLogo from '../assets/loaders/folia.svg'
import spongeLogo from '../assets/loaders/sponge.svg'
import velocityLogo from '../assets/loaders/velocity.svg'
import waterfallLogo from '../assets/loaders/waterfall.svg'
import bungeecordLogo from '../assets/loaders/bungeecord.svg'
import irisLogo from '../assets/loaders/iris.svg'
import optifineLogo from '../assets/loaders/optifine.svg'
import canvasLogo from '../assets/loaders/canvas.svg'
import geyserLogo from '../assets/loaders/geyser.svg'
import nilloaderLogo from '../assets/loaders/nilloader.svg'
import ornitheLogo from '../assets/loaders/ornithe.svg'
import legacyFabricLogo from '../assets/loaders/legacy-fabric.svg'
import babricLogo from '../assets/loaders/babric.svg'
import btaBabricLogo from '../assets/loaders/bta-babric.svg'

const props = withDefaults(
  defineProps<{
    loader: LoaderId | string
    size?: 'sm' | 'md' | 'lg'
  }>(),
  { size: 'md' },
)

// для Vanilla нет официального логотипа, поэтому показываем букву
const logos: Record<string, string> = {
  fabric: fabricLogo,
  forge: forgeLogo,
  neoforge: neoforgeLogo,
  quilt: quiltLogo,
  liteloader: liteloaderLogo,
  rift: riftLogo,
  modloader: modloaderLogo,
  bukkit: bukkitLogo,
  spigot: spigotLogo,
  paper: paperLogo,
  purpur: purpurLogo,
  folia: foliaLogo,
  sponge: spongeLogo,
  velocity: velocityLogo,
  waterfall: waterfallLogo,
  bungeecord: bungeecordLogo,
  iris: irisLogo,
  optifine: optifineLogo,
  canvas: canvasLogo,
  geyser: geyserLogo,
  nilloader: nilloaderLogo,
  ornithe: ornitheLogo,
  'legacy-fabric': legacyFabricLogo,
  babric: babricLogo,
  'bta-babric': btaBabricLogo,
}

const logoFor = (loader: string): string => logos[loader.toLowerCase()] || ''
const labelFor = (loader: string): string => loader.slice(0, 1).toUpperCase()
</script>

<template>
  <span class="loader-badge" :class="[`loader-badge--${size}`, `loader-badge--${props.loader}`]" :aria-label="props.loader" v-tooltip="props.loader === 'vanilla' ? 'Vanilla' : props.loader">
    <img v-if="logoFor(props.loader)" class="loader-badge-logo" :src="logoFor(props.loader)" alt="" loading="lazy" />
    <span v-else class="loader-fallback">{{ labelFor(props.loader) }}</span>
  </span>
</template>
