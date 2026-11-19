import type { InstanceIconBlock } from '../types'

const FACE = 32
const LEFT_SHADE = 0.18
const RIGHT_SHADE = 0.38

const composedCache = new Map<string, string>()

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image()
    image.onload = () => resolve(image)
    image.onerror = () => reject(new Error(`texture failed: ${src.slice(0, 48)}`))
    image.src = src
  })
}

function shadedTile(image: HTMLImageElement, shade: number): HTMLCanvasElement {
  const tile = document.createElement('canvas')
  tile.width = FACE
  tile.height = FACE
  const ctx = tile.getContext('2d')
  if (!ctx) return tile
  ctx.imageSmoothingEnabled = false
  ctx.drawImage(image, 0, 0, FACE, FACE)
  if (shade > 0) {
    ctx.globalCompositeOperation = 'source-atop'
    ctx.fillStyle = `rgba(0, 0, 0, ${shade})`
    ctx.fillRect(0, 0, FACE, FACE)
  }
  return tile
}

async function composeInventoryBlock(topUrl: string, sideUrl: string): Promise<string> {
  const [top, side] = await Promise.all([loadImage(topUrl), loadImage(sideUrl)])
  const canvas = document.createElement('canvas')
  canvas.width = FACE * 2
  canvas.height = FACE * 2
  const ctx = canvas.getContext('2d')
  if (!ctx) throw new Error('canvas 2d unavailable')
  ctx.imageSmoothingEnabled = false
  const edge = 31 / 32
  ctx.setTransform(edge, 0.5, 0, 1, 1, 16)
  ctx.drawImage(shadedTile(side, LEFT_SHADE), 0, 0)
  ctx.setTransform(edge, -0.5, 0, 1, 32, 32)
  ctx.drawImage(shadedTile(side, RIGHT_SHADE), 0, 0)
  ctx.setTransform(edge, -0.5, edge, 0.5, 1, 16)
  ctx.drawImage(shadedTile(top, 0), 0, 0)
  ctx.setTransform(1, 0, 0, 1, 0, 0)
  return canvas.toDataURL('image/png')
}

export async function composedBlockIcon(block: InstanceIconBlock): Promise<string> {
  const top = block.top_data_url || block.data_url
  const side = block.side_data_url || block.data_url
  const cacheKey = `${block.id}\n${top}\n${side}`
  const cached = composedCache.get(cacheKey)
  if (cached) return cached
  let url: string
  try {
    url = top && side ? await composeInventoryBlock(top, side) : block.data_url
  } catch {
    url = block.data_url
  }
  composedCache.set(cacheKey, url)
  return url
}
