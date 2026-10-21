// карта UV-областей текстуры скина Minecraft для редактора скинов:
export type SkinFace = 'top' | 'bottom' | 'right' | 'front' | 'left' | 'back'
export type SkinPartId = 'head' | 'body' | 'rightArm' | 'leftArm' | 'rightLeg' | 'leftLeg'
export type SkinLayer = 'base' | 'overlay'

export interface SkinRegion {
  id: string
  part: SkinPartId
  partLabel: string
  face: SkinFace
  faceLabel: string
  layer: SkinLayer
  x: number
  y: number
  width: number
  height: number
}

export interface SkinRegionBox {
  u: number
  v: number
  w: number
  h: number
  d: number
}

const FACE_LABELS: Record<SkinFace, string> = {
  top: 'верх',
  bottom: 'низ',
  right: 'правая',
  front: 'перед',
  left: 'левая',
  back: 'зад',
}

const PART_LABELS: Record<SkinPartId, string> = {
  head: 'Голова',
  body: 'Торс',
  rightArm: 'Правая рука',
  leftArm: 'Левая рука',
  rightLeg: 'Правая нога',
  leftLeg: 'Левая нога',
}

/** Прямоугольники сторон одной «коробки» развёртки (как в skinRenderer). */
export function boxRegions(box: SkinRegionBox): Record<SkinFace, { x: number; y: number; width: number; height: number }> {
  const { u, v, w, h, d } = box
  return {
    top: { x: u + d, y: v, width: w, height: d },
    bottom: { x: u + d + w, y: v, width: w, height: d },
    right: { x: u, y: v + d, width: d, height: h },
    front: { x: u + d, y: v + d, width: w, height: h },
    left: { x: u + d + w, y: v + d, width: d, height: h },
    back: { x: u + d + w + d, y: v + d, width: w, height: h },
  }
}

interface PartLayout {
  part: SkinPartId
  base: SkinRegionBox
  overlay?: SkinRegionBox
}

function partLayouts(model: 'classic' | 'slim'): PartLayout[] {
  const arm = model === 'slim' ? 3 : 4
  return [
    { part: 'head', base: { u: 0, v: 0, w: 8, h: 8, d: 8 }, overlay: { u: 32, v: 0, w: 8, h: 8, d: 8 } },
    { part: 'body', base: { u: 16, v: 16, w: 8, h: 12, d: 4 }, overlay: { u: 16, v: 32, w: 8, h: 12, d: 4 } },
    { part: 'rightArm', base: { u: 40, v: 16, w: arm, h: 12, d: 4 }, overlay: { u: 40, v: 32, w: arm, h: 12, d: 4 } },
    { part: 'leftArm', base: { u: 32, v: 48, w: arm, h: 12, d: 4 }, overlay: { u: 48, v: 48, w: arm, h: 12, d: 4 } },
    { part: 'rightLeg', base: { u: 0, v: 16, w: 4, h: 12, d: 4 }, overlay: { u: 0, v: 32, w: 4, h: 12, d: 4 } },
    { part: 'leftLeg', base: { u: 16, v: 48, w: 4, h: 12, d: 4 }, overlay: { u: 0, v: 48, w: 4, h: 12, d: 4 } },
  ]
}

/**
 * Все области текстуры 64×64. `legacy` — формат 64×32: размечаем только то,
 * что в нём есть (базовый слой, правые конечности и шляпа).
 */
export function skinRegions(model: 'classic' | 'slim', legacy = false): SkinRegion[] {
  const regions: SkinRegion[] = []
  for (const layout of partLayouts(model)) {
    for (const face of Object.keys(boxRegions(layout.base)) as SkinFace[]) {
      const rect = boxRegions(layout.base)[face]
      regions.push({
        id: `${layout.part}.base.${face}`,
        part: layout.part,
        partLabel: PART_LABELS[layout.part],
        face,
        faceLabel: FACE_LABELS[face],
        layer: 'base',
        ...rect,
      })
      if (layout.overlay) {
        const overlayRect = boxRegions(layout.overlay)[face]
        regions.push({
          id: `${layout.part}.overlay.${face}`,
          part: layout.part,
          partLabel: PART_LABELS[layout.part],
          face,
          faceLabel: FACE_LABELS[face],
          layer: 'overlay',
          ...overlayRect,
        })
      }
    }
  }
  return legacy ? regions.filter((region) => region.y + region.height <= 32) : regions
}

/** Область, в которую попадает пиксель текстуры, или null. */
export function regionAt(regions: SkinRegion[], x: number, y: number): SkinRegion | null {
  for (const region of regions) {
    if (x >= region.x && x < region.x + region.width && y >= region.y && y < region.y + region.height) return region
  }
  return null
}
