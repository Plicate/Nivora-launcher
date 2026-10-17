<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { skinMapForLayer } from '../skinUv'

type FaceName = 'front' | 'back' | 'left' | 'right' | 'top' | 'bottom'
type PartName = 'head' | 'body' | 'arm-left' | 'arm-right' | 'leg-left' | 'leg-right'
type Rect = { x: number; y: number; width: number; height: number }
type FaceMap = Record<FaceName, Rect>
type Vec3 = [number, number, number]
type Mat4 = Float32Array

const props = withDefaults(
  defineProps<{
    image?: string
    model?: 'classic' | 'slim'
    cape?: boolean
    capeImage?: string
  }>(),
  { image: '', model: 'classic', cape: false, capeImage: '' },
)

const canvas = ref<HTMLCanvasElement | null>(null)
const webglSupported = ref(true)
const textureLoaded = ref(false)
const textureError = ref('')
const faces: FaceName[] = ['front', 'back', 'left', 'right', 'top', 'bottom']
const textureWidth = ref(64)
const textureHeight = ref(64)
const skinScale = ref(1)

let gl: WebGLRenderingContext | undefined
let program: WebGLProgram | undefined
let skinTexture: WebGLTexture | undefined
let capeTexture: WebGLTexture | undefined
let positionBuffer: WebGLBuffer | undefined
let uvBuffer: WebGLBuffer | undefined
let shadeBuffer: WebGLBuffer | undefined
let attributeLocations: { position: number; uv: number; shade: number } | undefined
let uniformLocations: { matrix: WebGLUniformLocation | null; texture: WebGLUniformLocation | null } | undefined
let skinImage: HTMLImageElement | undefined
let capeImage: HTMLImageElement | undefined
let frame = 0
let lastFrame = 0
let resizeObserver: ResizeObserver | undefined
let dragging = false
let lastPointerX = 0
let lastPointerY = 0
let lastInteraction = 0
let yaw = -0.55
let pitch = -0.12
let zoom = 1

const head: FaceMap = {
  front: { x: 8, y: 8, width: 8, height: 8 },
  back: { x: 24, y: 8, width: 8, height: 8 },
  left: { x: 0, y: 8, width: 8, height: 8 },
  right: { x: 16, y: 8, width: 8, height: 8 },
  top: { x: 8, y: 0, width: 8, height: 8 },
  bottom: { x: 16, y: 0, width: 8, height: 8 },
}
const body: FaceMap = {
  front: { x: 20, y: 20, width: 8, height: 12 },
  back: { x: 32, y: 20, width: 8, height: 12 },
  left: { x: 16, y: 20, width: 4, height: 12 },
  right: { x: 28, y: 20, width: 4, height: 12 },
  top: { x: 20, y: 16, width: 8, height: 4 },
  bottom: { x: 28, y: 16, width: 8, height: 4 },
}
const rightLeg: FaceMap = {
  front: { x: 4, y: 20, width: 4, height: 12 },
  back: { x: 12, y: 20, width: 4, height: 12 },
  left: { x: 0, y: 20, width: 4, height: 12 },
  right: { x: 8, y: 20, width: 4, height: 12 },
  top: { x: 4, y: 16, width: 4, height: 4 },
  bottom: { x: 8, y: 16, width: 4, height: 4 },
}
const leftLeg: FaceMap = {
  front: { x: 20, y: 52, width: 4, height: 12 },
  back: { x: 28, y: 52, width: 4, height: 12 },
  left: { x: 16, y: 52, width: 4, height: 12 },
  right: { x: 24, y: 52, width: 4, height: 12 },
  top: { x: 20, y: 48, width: 4, height: 4 },
  bottom: { x: 24, y: 48, width: 4, height: 4 },
}

const rect = (x: number, y: number, width: number, height: number): Rect => ({ x, y, width, height })

function armMap(side: 'left' | 'right'): FaceMap {
  const left = side === 'left'
  const x = left ? 36 : 44
  const y = left ? 52 : 20
  const sideX = left ? 32 : 40
  const topY = left ? 48 : 16
  const width = props.model === 'slim' ? 3 : 4
  const backX = x + width + 4
  return {
    front: rect(x, y, width, 12),
    back: rect(backX, y, width, 12),
    left: rect(sideX, y, 4, 12),
    right: rect(x + width, y, 4, 12),
    top: rect(x, topY, width, 4),
    bottom: rect(x + width, topY, 4, 4),
  }
}

function shiftMap(value: FaceMap, dx: number, dy: number): FaceMap {
  return Object.fromEntries(faces.map((face) => [face, { ...value[face], x: value[face].x + dx, y: value[face].y + dy }])) as FaceMap
}

function overlayMap(part: PartName): FaceMap | undefined {
  if (textureHeight.value < 64 * skinScale.value) return undefined
  if (part === 'head') return shiftMap(head, 32, 0)
  if (part === 'body') return shiftMap(body, 0, 16)
  if (part === 'arm-right') return shiftMap(armMap('right'), 0, 16)
  if (part === 'arm-left') return shiftMap(armMap('left'), 16, 0)
  if (part === 'leg-right') return shiftMap(rightLeg, 0, 16)
  return shiftMap(rightLeg, 0, 32)
}

function baseMap(part: PartName): FaceMap {
  if (part === 'head') return head
  if (part === 'body') return body
  if (part === 'arm-right') return armMap('right')
  if (part === 'arm-left') return textureHeight.value < 64 * skinScale.value ? armMap('right') : armMap('left')
  if (part === 'leg-right') return rightLeg
  return textureHeight.value < 64 * skinScale.value ? rightLeg : leftLeg
}

function shader(type: number, source: string): WebGLShader {
  if (!gl) throw new Error('WebGL is not initialized')
  const value = gl.createShader(type)
  if (!value) throw new Error('Unable to create WebGL shader')
  gl.shaderSource(value, source)
  gl.compileShader(value)
  if (!gl.getShaderParameter(value, gl.COMPILE_STATUS)) {
    const message = gl.getShaderInfoLog(value) || 'Shader compilation failed'
    gl.deleteShader(value)
    throw new Error(message)
  }
  return value
}

function createProgram(): WebGLProgram {
  if (!gl) throw new Error('WebGL is not initialized')
  const vertex = shader(gl.VERTEX_SHADER, `
    attribute vec3 aPosition;
    attribute vec2 aUv;
    attribute float aShade;
    uniform mat4 uMatrix;
    varying vec2 vUv;
    varying float vShade;
    void main() {
      gl_Position = uMatrix * vec4(aPosition, 1.0);
      vUv = aUv;
      vShade = aShade;
    }
  `)
  const fragment = shader(gl.FRAGMENT_SHADER, `
    precision mediump float;
    uniform sampler2D uTexture;
    varying vec2 vUv;
    varying float vShade;
    void main() {
      vec4 color = texture2D(uTexture, vUv);
      if (color.a < 0.02) discard;
      gl_FragColor = vec4(color.rgb * vShade, color.a);
    }
  `)
  const value = gl.createProgram()
  if (!value) throw new Error('Unable to create WebGL program')
  gl.attachShader(value, vertex)
  gl.attachShader(value, fragment)
  gl.linkProgram(value)
  gl.deleteShader(vertex)
  gl.deleteShader(fragment)
  if (!gl.getProgramParameter(value, gl.LINK_STATUS)) {
    const message = gl.getProgramInfoLog(value) || 'Program linking failed'
    gl.deleteProgram(value)
    throw new Error(message)
  }
  return value
}

function createTexture(): WebGLTexture {
  if (!gl) throw new Error('WebGL is not initialized')
  const value = gl.createTexture()
  if (!value) throw new Error('Unable to create WebGL texture')
  gl.bindTexture(gl.TEXTURE_2D, value)
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST)
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST)
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)
  gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array([142, 163, 206, 255]))
  return value
}

function uploadTexture(texture: WebGLTexture, source?: TexImageSource, flipY = false) {
  if (!gl) return
  gl.bindTexture(gl.TEXTURE_2D, texture)
  gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, flipY ? 1 : 0)
  if (source) {
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, source)
  } else {
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, 1, 1, 0, gl.RGBA, gl.UNSIGNED_BYTE, new Uint8Array([142, 163, 206, 255]))
  }
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST)
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST)
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)
}

function loadImage(url: string, onLoad: (image: HTMLImageElement) => void): HTMLImageElement | undefined {
  if (!url) return undefined
  const image = new Image()
  image.crossOrigin = 'anonymous'
  image.decoding = 'async'
  image.referrerPolicy = 'no-referrer'
  image.onload = () => onLoad(image)
  image.src = url
  return image
}

function loadSkin() {
  if (!skinTexture) return
  textureWidth.value = 64
  textureHeight.value = 64
  skinScale.value = 1
  textureLoaded.value = false
  textureError.value = props.image ? 'Загрузка текстуры…' : 'Нет активного скина'
  uploadTexture(skinTexture)
  let image: HTMLImageElement | undefined
  image = loadImage(props.image, (loaded) => {
    if (skinImage !== image) return
    const width = loaded.naturalWidth || 0
    const height = loaded.naturalHeight || 0
    if (![[64, 32], [64, 64], [128, 64], [128, 128]].some(([expectedWidth, expectedHeight]) => width === expectedWidth && height === expectedHeight)) {
      textureError.value = 'Неподдерживаемый размер PNG-скина'
      return
    }
    textureWidth.value = width
    textureHeight.value = height
    skinScale.value = textureWidth.value / 64
    try {
      uploadTexture(skinTexture as WebGLTexture, loaded, true)
      textureLoaded.value = true
      textureError.value = ''
    } catch {
      textureError.value = 'Браузер не разрешил загрузить текстуру скина'
    }
  })
  skinImage = image
  if (image) image.onerror = () => {
    if (skinImage === image) textureError.value = 'Не удалось загрузить текстуру скина'
  }
  if (!props.image) {
    textureLoaded.value = false
  }
}

function loadCape() {
  if (!capeTexture) return
  let image: HTMLImageElement | undefined
  image = loadImage(props.capeImage, (loaded) => {
    if (capeImage === image) uploadTexture(capeTexture as WebGLTexture, loaded)
  })
  capeImage = image
  if (!props.capeImage) uploadTexture(capeTexture)
}

function multiply(a: Mat4, b: Mat4): Mat4 {
  const result = new Float32Array(16)
  for (let column = 0; column < 4; column += 1) {
    for (let row = 0; row < 4; row += 1) {
      result[column * 4 + row] =
        a[row] * b[column * 4] +
        a[4 + row] * b[column * 4 + 1] +
        a[8 + row] * b[column * 4 + 2] +
        a[12 + row] * b[column * 4 + 3]
    }
  }
  return result
}

function perspective(fov: number, aspect: number, near: number, far: number): Mat4 {
  const value = new Float32Array(16)
  const scale = 1 / Math.tan(fov / 2)
  value[0] = scale / aspect
  value[5] = scale
  value[10] = (far + near) / (near - far)
  value[11] = -1
  value[14] = (2 * far * near) / (near - far)
  return value
}

function lookAt(eye: Vec3, target: Vec3, up: Vec3): Mat4 {
  const z = normalize([eye[0] - target[0], eye[1] - target[1], eye[2] - target[2]])
  const x = normalize(cross(up, z))
  const y = cross(z, x)
  return new Float32Array([
    x[0], y[0], z[0], 0,
    x[1], y[1], z[1], 0,
    x[2], y[2], z[2], 0,
    -dot(x, eye), -dot(y, eye), -dot(z, eye), 1,
  ])
}

function rotationX(angle: number): Mat4 {
  const c = Math.cos(angle)
  const s = Math.sin(angle)
  return new Float32Array([1, 0, 0, 0, 0, c, s, 0, 0, -s, c, 0, 0, 0, 0, 1])
}

function rotationY(angle: number): Mat4 {
  const c = Math.cos(angle)
  const s = Math.sin(angle)
  return new Float32Array([c, 0, -s, 0, 0, 1, 0, 0, s, 0, c, 0, 0, 0, 0, 1])
}

function cross(a: Vec3, b: Vec3): Vec3 { return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]] }
function dot(a: Vec3, b: Vec3): number { return a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
function normalize(value: Vec3): Vec3 { const length = Math.hypot(...value) || 1; return [value[0] / length, value[1] / length, value[2] / length] }

function addFace(positions: number[], uvs: number[], shades: number[], center: Vec3, size: Vec3, face: FaceName, map: Rect, shade: number) {
  const [cx, cy, cz] = center
  const [width, height, depth] = size
  const x0 = cx - width / 2
  const x1 = cx + width / 2
  const y0 = cy - height / 2
  const y1 = cy + height / 2
  const z0 = cz - depth / 2
  const z1 = cz + depth / 2
  const quads: Record<FaceName, Vec3[]> = {
    front: [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
    back: [[x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0]],
    left: [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
    right: [[x1, y0, z1], [x1, y0, z0], [x1, y1, z0], [x1, y1, z1]],
    top: [[x0, y1, z1], [x1, y1, z1], [x1, y1, z0], [x0, y1, z0]],
    bottom: [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
  }
  const quad = quads[face]
  const order = [0, 1, 2, 0, 2, 3]
  const u0 = (map.x * skinScale.value) / textureWidth.value
  const u1 = ((map.x + map.width) * skinScale.value) / textureWidth.value
  const v0 = 1 - (map.y * skinScale.value) / textureHeight.value
  const v1 = 1 - ((map.y + map.height) * skinScale.value) / textureHeight.value
  const uv = [[u0, v1], [u1, v1], [u1, v0], [u0, v0]]
  for (const index of order) {
    positions.push(...quad[index])
    uvs.push(...uv[index])
    shades.push(shade)
  }
}

type PreparedGeometry = { positions: number[]; uvs: number[]; shades: number[]; cachedPositions: Float32Array; cachedUvs: Float32Array; cachedShades: Float32Array }
const geometryCache = new Map<string, PreparedGeometry>()

function prepareGeometry(key: string, build: () => { positions: number[]; uvs: number[]; shades: number[] }): PreparedGeometry {
  const cached = geometryCache.get(key)
  if (cached) return cached
  const geometry = build()
  const prepared: PreparedGeometry = {
    ...geometry,
    cachedPositions: new Float32Array(geometry.positions),
    cachedUvs: new Float32Array(geometry.uvs),
    cachedShades: new Float32Array(geometry.shades),
  }
  geometryCache.set(key, prepared)
  return prepared
}

function cubeGeometry(part: PartName, overlay = false): PreparedGeometry {
  const armWidth = props.model === 'slim' ? 3 : 4
  const armOffset = 4 + armWidth / 2
  const parts: Record<PartName, { center: Vec3; size: Vec3 }> = {
    // Keep a small neck gap so the head overlay cannot visually penetrate the torso.
    head: { center: [0, 28.6, 0], size: [8, 8, 8] },
    body: { center: [0, 18, 0], size: [8, 12, 4] },
    'arm-left': { center: [-armOffset, 18, 0], size: [armWidth, 12, 4] },
    'arm-right': { center: [armOffset, 18, 0], size: [armWidth, 12, 4] },
    'leg-left': { center: [-2, 6, 0], size: [4, 12, 4] },
    'leg-right': { center: [2, 6, 0], size: [4, 12, 4] },
  }
  const partValue = parts[part]
  const size: Vec3 = overlay ? partValue.size.map((value) => value + 0.42) as Vec3 : partValue.size
  const map = skinMapForLayer(baseMap(part), overlayMap(part), overlay)
  const positions: number[] = []
  const uvs: number[] = []
  const shades: number[] = []
  const faceShade: Record<FaceName, number> = { front: 1, back: .72, left: .82, right: .92, top: 1.1, bottom: .56 }
  for (const face of faces) addFace(positions, uvs, shades, partValue.center, size, face, map[face], faceShade[face])
  return prepareGeometry(`cube:${props.model}:${part}:${overlay}`, () => ({ positions, uvs, shades }))
}

function capeGeometry(): PreparedGeometry {
  return prepareGeometry(`cape:${props.capeImage ? 'a' : 'b'}`, () => ({
    positions: [-5, 23, -2.7, 5, 23, -2.7, 5, 7, -2.7, -5, 23, -2.7, 5, 7, -2.7, -5, 7, -2.7],
    uvs: [0, 0, 1, 0, 1, 1, 0, 0, 1, 1, 0, 1],
    shades: [1, 1, 1, 1, 1, 1],
  }))
}

function drawGeometry(texture: WebGLTexture, geometry: PreparedGeometry, matrix: Mat4) {
  if (!gl || !program || !positionBuffer || !uvBuffer || !shadeBuffer) return
  if (!attributeLocations || !uniformLocations) return
  gl.bindBuffer(gl.ARRAY_BUFFER, positionBuffer)
  gl.bufferData(gl.ARRAY_BUFFER, geometry.cachedPositions, gl.STREAM_DRAW)
  gl.enableVertexAttribArray(attributeLocations.position)
  gl.vertexAttribPointer(attributeLocations.position, 3, gl.FLOAT, false, 0, 0)
  gl.bindBuffer(gl.ARRAY_BUFFER, uvBuffer)
  gl.bufferData(gl.ARRAY_BUFFER, geometry.cachedUvs, gl.STREAM_DRAW)
  gl.enableVertexAttribArray(attributeLocations.uv)
  gl.vertexAttribPointer(attributeLocations.uv, 2, gl.FLOAT, false, 0, 0)
  gl.bindBuffer(gl.ARRAY_BUFFER, shadeBuffer)
  gl.bufferData(gl.ARRAY_BUFFER, geometry.cachedShades, gl.STREAM_DRAW)
  gl.enableVertexAttribArray(attributeLocations.shade)
  gl.vertexAttribPointer(attributeLocations.shade, 1, gl.FLOAT, false, 0, 0)
  gl.uniformMatrix4fv(uniformLocations.matrix, false, matrix)
  gl.activeTexture(gl.TEXTURE0)
  gl.bindTexture(gl.TEXTURE_2D, texture)
  gl.uniform1i(uniformLocations.texture, 0)
  gl.drawArrays(gl.TRIANGLES, 0, geometry.positions.length / 3)
}

function resize() {
  if (!canvas.value || !gl) return
  const bounds = canvas.value.getBoundingClientRect()
  const ratio = Math.min(2, window.devicePixelRatio || 1)
  canvas.value.width = Math.max(1, Math.round(bounds.width * ratio))
  canvas.value.height = Math.max(1, Math.round(bounds.height * ratio))
  gl.viewport(0, 0, canvas.value.width, canvas.value.height)
}

function render(now = performance.now()) {
  if (!gl || !program || !canvas.value || !skinTexture) return
  const elapsed = now - lastFrame
  lastFrame = now
  if (!dragging && now - lastInteraction > 900) yaw += elapsed * 0.00035
  gl.clearColor(0, 0, 0, 0)
  gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT)
  gl.useProgram(program)
  const aspect = canvas.value.width / Math.max(1, canvas.value.height)
  const projection = perspective(Math.PI / 4.2, aspect, .1, 160)
  const view = lookAt([0, 16, 57 / zoom], [0, 16, 0], [0, 1, 0])
  const model = multiply(rotationY(yaw), rotationX(pitch))
  const matrix = multiply(projection, multiply(view, model))
  if (props.cape && capeImage && capeTexture) drawGeometry(capeTexture, capeGeometry(), matrix)
  for (const part of ['leg-left', 'leg-right', 'body', 'arm-left', 'arm-right', 'head'] as PartName[]) {
    drawGeometry(skinTexture, cubeGeometry(part), matrix)
    if (overlayMap(part)) drawGeometry(skinTexture, cubeGeometry(part, true), matrix)
  }
}

function tick(now: number) {
  // Idle frames are throttled to ~30 fps and skipped entirely while the tab is hidden;
  // dragging still renders at full rate for smooth rotation.
  if (!dragging && document.hidden) {
    frame = window.requestAnimationFrame(tick)
    return
  }
  const minimumElapsed = dragging ? 0 : 33
  if (now - lastFrame >= minimumElapsed) render(now)
  frame = window.requestAnimationFrame(tick)
}

function pointerDown(event: PointerEvent) {
  dragging = true
  lastPointerX = event.clientX
  lastPointerY = event.clientY
  lastInteraction = performance.now()
  canvas.value?.setPointerCapture(event.pointerId)
}

function pointerMove(event: PointerEvent) {
  if (!dragging) return
  yaw += (event.clientX - lastPointerX) * 0.012
  pitch = Math.max(-.8, Math.min(.65, pitch + (event.clientY - lastPointerY) * 0.009))
  lastPointerX = event.clientX
  lastPointerY = event.clientY
  lastInteraction = performance.now()
}

function pointerUp() { dragging = false; lastInteraction = performance.now() }
function wheel(event: WheelEvent) { zoom = Math.max(.78, Math.min(1.32, zoom * (event.deltaY > 0 ? .94 : 1.06))); lastInteraction = performance.now(); event.preventDefault() }

function init() {
  const element = canvas.value
  if (!element) return
  gl = element.getContext('webgl', { alpha: true, antialias: true }) || undefined
  if (!gl) {
    webglSupported.value = false
    return
  }
  try {
    program = createProgram()
    skinTexture = createTexture()
    capeTexture = createTexture()
    positionBuffer = gl.createBuffer() || undefined
    uvBuffer = gl.createBuffer() || undefined
    shadeBuffer = gl.createBuffer() || undefined
    if (program) {
      attributeLocations = {
        position: gl.getAttribLocation(program, 'aPosition'),
        uv: gl.getAttribLocation(program, 'aUv'),
        shade: gl.getAttribLocation(program, 'aShade'),
      }
      uniformLocations = {
        matrix: gl.getUniformLocation(program, 'uMatrix'),
        texture: gl.getUniformLocation(program, 'uTexture'),
      }
    }
    gl.enable(gl.DEPTH_TEST)
    gl.enable(gl.BLEND)
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA)
    loadSkin()
    loadCape()
    resize()
    resizeObserver = new ResizeObserver(resize)
    resizeObserver.observe(element)
    lastInteraction = performance.now()
    frame = window.requestAnimationFrame(tick)
  } catch {
    webglSupported.value = false
  }
}

watch([() => props.image, () => props.capeImage, () => props.model], () => {
  if (gl) {
    loadSkin()
    loadCape()
  }
})

onMounted(init)
onBeforeUnmount(() => {
  window.cancelAnimationFrame(frame)
  resizeObserver?.disconnect()
  skinImage = undefined
  capeImage = undefined
  if (gl) {
    if (skinTexture) gl.deleteTexture(skinTexture)
    if (capeTexture) gl.deleteTexture(capeTexture)
    if (positionBuffer) gl.deleteBuffer(positionBuffer)
    if (uvBuffer) gl.deleteBuffer(uvBuffer)
    if (shadeBuffer) gl.deleteBuffer(shadeBuffer)
    if (program) gl.deleteProgram(program)
  }
})
</script>

<template>
  <div class="skin-preview-stage" aria-label="Настоящий 3D preview скина">
    <canvas
      ref="canvas"
      class="skin-preview-canvas"
      aria-label="3D-модель персонажа Minecraft"
      @pointerdown="pointerDown"
      @pointermove="pointerMove"
      @pointerup="pointerUp"
      @pointercancel="pointerUp"
      @pointerleave="pointerUp"
      @wheel="wheel"
    />
    <span v-if="!webglSupported" class="skin-preview-fallback">WebGL недоступен</span>
    <span v-else-if="textureError" class="skin-preview-fallback">{{ textureError }}</span>
    <span v-else-if="!textureLoaded" class="skin-preview-fallback">Загрузка текстуры…</span>
    <span class="skin-preview-hint">Потяните модель · колесо меняет масштаб</span>
    <div class="skin-preview-shadow" />
  </div>
</template>

<style scoped>
.skin-preview-stage { position: relative; display: grid; width: 260px; height: 286px; place-items: center; overflow: hidden; }
.skin-preview-canvas { position: absolute; inset: 0; width: 100%; height: 100%; cursor: grab; touch-action: none; }
.skin-preview-canvas:active { cursor: grabbing; }
.skin-preview-shadow { position: absolute; right: 50px; bottom: 28px; left: 50px; height: 22px; border-radius: 50%; background: rgba(0, 0, 0, .36); filter: blur(9px); pointer-events: none; }
.skin-preview-hint { position: absolute; right: 0; bottom: 0; left: 0; color: var(--muted); font-family: var(--font-mono); font-size: 8px; text-align: center; pointer-events: none; }
.skin-preview-fallback { position: absolute; top: 50%; color: var(--muted); font-size: 10px; }
</style>
