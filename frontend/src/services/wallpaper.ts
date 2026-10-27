interface WallpaperRenderer {
  /** null означает устаревший запрос, пустая строка означает отсутствие обоев */
  render(source: string, blur: number): Promise<string | null>
  release(): void
}

/** результат подготовки к сохранению: готовые байты WebP в base64 */
interface PreparedWallpaper {
  fileName: string
  data: string
  previewUrl: string
}

const STORED_MAX_SIDE = 1600

export function createWallpaperRenderer(): WallpaperRenderer {
  let request = 0
  let objectUrl = ''

  function release() {
    if (objectUrl) {
      URL.revokeObjectURL(objectUrl)
      objectUrl = ''
    }
  }

  async function render(source: string, blur: number): Promise<string | null> {
    const current = ++request
    if (!source) {
      release()
      return ''
    }
    const radius = Math.min(32, Math.max(0, Math.round(blur)))
    if (!radius) {
      release()
      return source
    }

    const image = new Image()
    image.decoding = 'async'
    image.src = source
    try {
      await image.decode()
      if (current !== request) return null

      const viewportWidth = Math.max(1, window.innerWidth)
      const viewportHeight = Math.max(1, window.innerHeight)
      const scale = Math.min(0.5, 960 / Math.max(viewportWidth, viewportHeight))
      const width = Math.max(1, Math.round(viewportWidth * scale))
      const height = Math.max(1, Math.round(viewportHeight * scale))
      const scaledRadius = radius * scale
      const padding = Math.ceil(scaledRadius * 3)

      const renderCanvas = document.createElement('canvas')
      renderCanvas.width = width + padding * 2
      renderCanvas.height = height + padding * 2
      const renderContext = renderCanvas.getContext('2d')
      const outputCanvas = document.createElement('canvas')
      outputCanvas.width = width
      outputCanvas.height = height
      const outputContext = outputCanvas.getContext('2d')
      if (!renderContext || !outputContext) throw new Error('Canvas unavailable')

      const cover = Math.max(renderCanvas.width / image.naturalWidth, renderCanvas.height / image.naturalHeight)
      const drawWidth = image.naturalWidth * cover
      const drawHeight = image.naturalHeight * cover
      renderContext.filter = `blur(${scaledRadius}px)`
      renderContext.drawImage(image, (renderCanvas.width - drawWidth) / 2, (renderCanvas.height - drawHeight) / 2, drawWidth, drawHeight)
      // запас по краям не даёт blur затемнить границы кадра
      outputContext.drawImage(renderCanvas, padding, padding, width, height, 0, 0, width, height)

      const blob = await new Promise<Blob | null>((resolve) => outputCanvas.toBlob(resolve, 'image/webp', 0.86))
      if (current !== request) return null
      if (!blob) throw new Error('Canvas unavailable')

      const previousUrl = objectUrl
      objectUrl = URL.createObjectURL(blob)
      if (previousUrl) URL.revokeObjectURL(previousUrl)
      return objectUrl
    } catch {
      // если изображение недоступно, показываем исходник без размытия
      if (current === request) return source
      return null
    }
  }

  return { render, release }
}

/** уменьшает изображение и сохраняет его в WebP; null означает ошибку подготовки */
export async function prepareWallpaperForStorage(source: string): Promise<PreparedWallpaper | null> {
  const image = new Image()
  image.decoding = 'async'
  image.src = source
  try {
    await image.decode()
  } catch {
    return null
  }

  const longest = Math.max(image.naturalWidth, image.naturalHeight)
  if (!longest) return null
  const scale = Math.min(1, STORED_MAX_SIDE / longest)
  const width = Math.max(1, Math.round(image.naturalWidth * scale))
  const height = Math.max(1, Math.round(image.naturalHeight * scale))

  const canvas = document.createElement('canvas')
  canvas.width = width
  canvas.height = height
  const context = canvas.getContext('2d')
  if (!context) return null
  context.imageSmoothingQuality = 'high'
  context.drawImage(image, 0, 0, width, height)

  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, 'image/webp', 0.86))
  if (!blob) return null

  const dataUrl = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result || ''))
    reader.onerror = () => reject(new Error('read failed'))
    reader.readAsDataURL(blob)
  })
  const base64 = dataUrl.replace(/^data:[^;]*;base64,/, '')
  if (!base64) return null

  return {
    fileName: 'wallpaper.webp',
    data: base64,
    previewUrl: URL.createObjectURL(blob),
  }
}
