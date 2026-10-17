export function skinMapForLayer<T>(baseMap: T, overlayMap: T | undefined, overlay: boolean): T {
  return overlay ? (overlayMap ?? baseMap) : baseMap
}
