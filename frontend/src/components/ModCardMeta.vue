<script setup lang="ts">
import { computed } from 'vue'
import type { ModResult } from '../types'
import Icon from './Icon.vue'
import LoaderBadge from './LoaderBadge.vue'

const props = defineProps<{ mod: ModResult }>()

// платформенные категории из результатов поиска Modrinth
const LOADER_TAGS = new Set(['fabric', 'forge', 'neoforge', 'quilt', 'liteloader', 'rift', 'modloader', 'bukkit', 'spigot', 'paper', 'purpur', 'folia', 'sponge', 'velocity', 'waterfall', 'bungeecord', 'iris', 'optifine', 'canvas', 'geyser', 'nilloader', 'ornithe', 'legacy-fabric', 'babric', 'bta-babric'])

// показываем поддерживаемые платформы как логотипы
const loaders = computed(() =>
  ((props.mod.categories || []) as unknown[])
    .filter((tag): tag is string => typeof tag === 'string' && LOADER_TAGS.has(tag.toLowerCase()))
    .slice(0, 3),
)

// оставляем не более двух категорий, чтобы карточка не разрасталась
const categories = computed(() => {
  const raw = (props.mod.display_categories && props.mod.display_categories.length ? props.mod.display_categories : props.mod.categories) || []
  return ((raw as unknown[]).filter((tag): tag is string => typeof tag === 'string' && !LOADER_TAGS.has(tag.toLowerCase())) as string[]).slice(0, 2)
})

// значения required и optional означают поддержку соответствующей стороны
const environment = computed<{ label: string; icon: string } | null>(() => {
  const client = String(props.mod.client_side || '').toLowerCase()
  const server = String(props.mod.server_side || '').toLowerCase()
  const clientSupported = client === 'required' || client === 'optional'
  const serverSupported = server === 'required' || server === 'optional'
  if (clientSupported && serverSupported) return { label: 'Клиент и сервер', icon: 'globe' }
  if (clientSupported) return { label: 'Клиентский', icon: 'monitor' }
  if (serverSupported) return { label: 'Серверный', icon: 'server' }
  return null
})

const visible = computed(() => Boolean(environment.value || loaders.value.length || categories.value.length))
</script>

<template>
  <div v-if="visible" class="mod-card-tags">
    <span v-if="environment" class="mod-tag mod-tag--env"><Icon :name="environment.icon" :size="12" />{{ environment.label }}</span>
    <span v-for="category in categories" :key="`category-${category}`" class="mod-tag">{{ category }}</span>
    <span v-for="loader in loaders" :key="`loader-${loader}`" class="mod-tag mod-tag--loader"><LoaderBadge :loader="loader" size="sm" />{{ loader }}</span>
  </div>
</template>
