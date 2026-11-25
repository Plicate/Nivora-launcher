<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import Icon from './Icon.vue'
import { collapsePoint, parseCatalogBody, type CatalogBlock, type CatalogInline } from '../services/catalogMarkdown'

const props = defineProps<{ body?: string; description?: string }>()

const blocks = computed<CatalogBlock[]>(() => {
  const source = props.body || props.description || ''
  if (!source) return []
  return parseCatalogBody(source)
})

const limit = computed(() => collapsePoint(blocks.value))
const expanded = ref(false)
watch(() => props.body, () => { expanded.value = false })

const visibleBlocks = computed(() => (expanded.value ? blocks.value : blocks.value.slice(0, limit.value)))
const collapsible = computed(() => blocks.value.length > limit.value)

const failedMedia = ref<string[]>([])
watch(() => props.body, () => { failedMedia.value = [] })

function badgeVisible(url: string) {
  return !failedMedia.value.includes(url)
}
function hideMedia(url: string) {
  failedMedia.value = [...failedMedia.value, url]
}
</script>

<template>
  <div class="cat-desc">
    <template v-for="(block, index) in visibleBlocks" :key="`block-${index}`">
      <h3 v-if="block.type === 'heading' && block.level <= 3" class="cat-heading">
        <template v-for="(part, partIndex) in block.inline" :key="partIndex">
          <a v-if="part.kind === 'link'" :href="part.url" target="_blank" rel="noreferrer">{{ part.text }}</a>
          <code v-else-if="part.kind === 'code'">{{ part.text }}</code>
          <strong v-else-if="part.kind === 'strong'">{{ part.text }}</strong>
          <em v-else-if="part.kind === 'em'">{{ part.text }}</em>
          <template v-else>{{ part.text }}</template>
        </template>
      </h3>
      <h4 v-else-if="block.type === 'heading'" class="cat-subheading">
        <template v-for="(part, partIndex) in block.inline" :key="partIndex">
          <a v-if="part.kind === 'link'" :href="part.url" target="_blank" rel="noreferrer">{{ part.text }}</a>
          <code v-else-if="part.kind === 'code'">{{ part.text }}</code>
          <strong v-else-if="part.kind === 'strong'">{{ part.text }}</strong>
          <em v-else-if="part.kind === 'em'">{{ part.text }}</em>
          <template v-else>{{ part.text }}</template>
        </template>
      </h4>

      <p v-else-if="block.type === 'paragraph'" class="cat-paragraph">
        <template v-for="(part, partIndex) in block.inline" :key="partIndex">
          <a v-if="part.kind === 'link'" :href="part.url" target="_blank" rel="noreferrer">{{ part.text }}<Icon name="link" :size="11" /></a>
          <code v-else-if="part.kind === 'code'">{{ part.text }}</code>
          <strong v-else-if="part.kind === 'strong'">{{ part.text }}</strong>
          <em v-else-if="part.kind === 'em'">{{ part.text }}</em>
          <template v-else>{{ part.text }}</template>
        </template>
      </p>

      <div v-else-if="block.type === 'badges'" class="cat-badges">
        <template v-for="(item, badgeIndex) in block.items" :key="`badge-${badgeIndex}`">
          <a v-if="badgeVisible(item.url)" class="cat-badge" :href="item.href" target="_blank" rel="noreferrer" :aria-label="item.alt || 'Ссылка'">
            <img :src="item.url" :alt="item.alt" loading="lazy" referrerpolicy="no-referrer" @error="hideMedia(item.url)" />
          </a>
        </template>
      </div>

      <component
        :is="block.href ? 'a' : 'figure'"
        v-else-if="block.type === 'media' && badgeVisible(block.url)"
        class="cat-media"
        :class="{ 'cat-media--video': block.video }"
        :href="block.href ? block.href : undefined"
        target="_blank"
        rel="noreferrer"
      >
        <img :src="block.url" :alt="block.alt" loading="lazy" referrerpolicy="no-referrer" @error="hideMedia(block.url)" />
        <span v-if="block.video" class="cat-media-play" aria-hidden="true"><Icon name="play" :size="22" /></span>
      </component>

      <blockquote v-else-if="block.type === 'quote'" class="cat-quote">
        <p v-for="(paragraph, paragraphIndex) in block.paragraphs" :key="paragraphIndex">
          <template v-for="(part, partIndex) in paragraph" :key="partIndex">
            <a v-if="part.kind === 'link'" :href="part.url" target="_blank" rel="noreferrer">{{ part.text }}<Icon name="link" :size="11" /></a>
            <strong v-else-if="part.kind === 'strong'">{{ part.text }}</strong>
            <em v-else-if="part.kind === 'em'">{{ part.text }}</em>
            <code v-else-if="part.kind === 'code'">{{ part.text }}</code>
            <template v-else>{{ part.text }}</template>
          </template>
        </p>
      </blockquote>

      <ul v-else-if="block.type === 'list' && !block.ordered" class="cat-list">
        <li v-for="(item, itemIndex) in block.items" :key="itemIndex">
          <template v-for="(part, partIndex) in item" :key="partIndex">
            <a v-if="part.kind === 'link'" :href="part.url" target="_blank" rel="noreferrer">{{ part.text }}</a>
            <strong v-else-if="part.kind === 'strong'">{{ part.text }}</strong>
            <em v-else-if="part.kind === 'em'">{{ part.text }}</em>
            <code v-else-if="part.kind === 'code'">{{ part.text }}</code>
            <template v-else>{{ part.text }}</template>
          </template>
        </li>
      </ul>
      <ol v-else-if="block.type === 'list'" class="cat-list cat-list--ordered">
        <li v-for="(item, itemIndex) in block.items" :key="itemIndex">
          <template v-for="(part, partIndex) in item" :key="partIndex">
            <a v-if="part.kind === 'link'" :href="part.url" target="_blank" rel="noreferrer">{{ part.text }}</a>
            <strong v-else-if="part.kind === 'strong'">{{ part.text }}</strong>
            <em v-else-if="part.kind === 'em'">{{ part.text }}</em>
            <code v-else-if="part.kind === 'code'">{{ part.text }}</code>
            <template v-else>{{ part.text }}</template>
          </template>
        </li>
      </ol>

      <pre v-else-if="block.type === 'code'" class="cat-code"><code>{{ block.text }}</code></pre>

      <div v-else-if="block.type === 'table'" class="cat-table-wrap">
        <table class="cat-table">
          <thead><tr><th v-for="(cell, cellIndex) in block.head" :key="cellIndex">
            <template v-for="(part, partIndex) in cell" :key="partIndex">{{ part.text }}</template>
          </th></tr></thead>
          <tbody>
            <tr v-for="(row, rowIndex) in block.rows" :key="rowIndex">
              <td v-for="(cell, cellIndex) in row" :key="cellIndex">
                <template v-for="(part, partIndex) in cell" :key="partIndex">
                  <a v-if="part.kind === 'link'" :href="part.url" target="_blank" rel="noreferrer">{{ part.text }}</a>
                  <template v-else>{{ part.text }}</template>
                </template>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <hr v-else-if="block.type === 'hr'" class="cat-rule" />
    </template>

    <button v-if="collapsible && !expanded" class="button button--small button--quiet cat-expand" type="button" @click="expanded = true">
      <Icon name="chevron" :size="14" />Показать описание полностью
    </button>
    <button v-else-if="collapsible && expanded" class="button button--small button--quiet cat-expand" type="button" @click="expanded = false">
      <Icon name="minus" :size="14" />Свернуть описание
    </button>
  </div>
</template>

<style scoped>
.cat-desc { display: grid; gap: 12px; }
.cat-heading { margin: 6px 0 0; color: var(--text); font-size: .95rem; font-weight: 650; letter-spacing: -.01em; }
.cat-subheading { margin: 2px 0 0; color: var(--quiet); font-size: .84rem; font-weight: 600; }
.cat-paragraph { margin: 0; color: var(--quiet); font-size: .86rem; line-height: 1.6; }
.cat-paragraph strong { color: var(--text); }
.cat-desc a { display: inline-flex; align-items: center; gap: 3px; color: var(--accent); }
.cat-desc a:hover { text-decoration: underline; }
.cat-desc code { padding: 1px 5px; border: 1px solid var(--line); border-radius: 5px; background: var(--panel-soft); color: var(--text); font-family: var(--font-mono); font-size: .78em; }
.cat-badges { display: flex; flex-wrap: wrap; gap: 8px; }
.cat-badge { display: inline-flex; overflow: hidden; border: 1px solid var(--line); border-radius: 9px; background: var(--panel-soft); transition: border-color .16s ease, transform .16s ease; }
.cat-badge:hover { border-color: var(--accent); transform: translateY(-1px); }
.cat-badge img { display: block; height: 34px; object-fit: contain; }
.cat-media { position: relative; display: block; overflow: hidden; margin: 2px 0; border: 1px solid var(--line); border-radius: calc(var(--radius-control, 10px) * 1.2); background: var(--panel-soft); }
.cat-media img { display: block; width: 100%; max-height: 300px; object-fit: cover; }
.cat-media--video img { aspect-ratio: 16 / 9; }
.cat-media-play { position: absolute; top: 50%; left: 50%; display: grid; width: 52px; height: 52px; place-items: center; color: var(--text); background: rgb(8 10 14 / 62%); border-radius: 999px; transform: translate(-50%, -50%); }
.cat-media--video:hover img { opacity: .88; }
.cat-quote { display: grid; gap: 8px; margin: 0; padding: 12px 14px; border-left: 2px solid var(--accent); border-radius: calc(var(--radius-control, 10px) * .8); background: var(--panel-soft); }
.cat-quote p { margin: 0; color: var(--quiet); font-size: .84rem; line-height: 1.55; }
.cat-quote strong { color: var(--text); }
.cat-list { display: grid; gap: 6px; margin: 0; padding-left: 20px; color: var(--quiet); font-size: .85rem; line-height: 1.55; }
.cat-list strong { color: var(--text); }
.cat-code { overflow-x: auto; margin: 0; padding: 12px; border: 1px solid var(--line); border-radius: calc(var(--radius-control, 10px) * .8); background: var(--panel-soft); color: var(--quiet); font-family: var(--font-mono); font-size: .78rem; line-height: 1.5; }
.cat-table-wrap { overflow-x: auto; }
.cat-table { width: 100%; border-collapse: collapse; font-size: .82rem; }
.cat-table th, .cat-table td { padding: 7px 10px; border: 1px solid var(--line); text-align: left; }
.cat-table th { color: var(--muted); font-weight: 600; background: var(--panel-soft); }
.cat-table td { color: var(--quiet); }
.cat-rule { margin: 4px 0; border: none; border-top: 1px solid var(--line); }
.cat-expand { justify-self: start; margin-top: 2px; }
</style>
