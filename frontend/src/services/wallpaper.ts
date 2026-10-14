// Конвейер отрисовки обоев: исходная картинка → canvas-размытие → blob-URL.
// Живёт отдельно от App.vue, потому что здесь есть свои тонкости:
//  * запросы нумеруются, и устаревший результат (пользователь дёрнул слайдер
//    размытия) отбрасывается, а не перезаписывает свежий;
//  * blob-URL предыдущего кадра освобождается только после того, как новый
//    уже записан, иначе на миг пропадает фон.
// toBlob вместо toDataURL: кодирование WebP уходит из главного потока.

export interface WallpaperRenderer {
  /** null — запрос устарел, значение не трогаем; '' — обоев нет. */
  render(source: string, blur: number): Promise<string | null>
  release(): void
}

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

      // Рисуем в уменьшенном масштабе: blur на маленьком холсте дешевле,
      // а растянутый результат всё равно стоит за полупрозрачным интерфейсом.
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
      // Кадр без полей: blur по краям холста затемняет картинку, поэтому
      // берём центральную часть с запасом на радиус.
      outputContext.drawImage(renderCanvas, padding, padding, width, height, 0, 0, width, height)

      const blob = await new Promise<Blob | null>((resolve) => outputCanvas.toBlob(resolve, 'image/webp', 0.86))
      if (current !== request) return null
      if (!blob) throw new Error('Canvas unavailable')

      const previousUrl = objectUrl
      objectUrl = URL.createObjectURL(blob)
      if (previousUrl) URL.revokeObjectURL(previousUrl)
      return objectUrl
    } catch {
      // повреждённый файл, недоступный asset — показываем как есть
      if (current === request) return source
      return null
    }
  }

  return { render, release }
}
