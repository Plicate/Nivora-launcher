// WebGL-рендер персонажа Minecraft: скин (оба слоя), плащ и элитры.
//
// Геометрия и развёртки повторяют skinview3d (его использует Prism Launcher):
// каждый элемент — коробка со стандартной Minecraft-развёрткой (u, v, w, h, d).
// Плащ — объёмная коробка 10×16×1 под углом 10.8°, снаружи — область
// (1,1)–(11,17) текстуры 64×32; элитры — два крыла из области (22,0).
// Координаты: ноги внизу (y = −16), макушка сверху (y = 16), лицо смотрит
// в +z, левая рука персонажа — в +x.

export type SkinModel = 'classic' | 'slim'
export type BackEquipment = 'cape' | 'elytra' | null

export interface SkinScene {
  skin: string
  model: SkinModel
  cape?: string
  back?: BackEquipment
  yaw: number
  pitch: number
  zoom?: number
}

type Vec3 = [number, number, number]
type Mat4 = Float32Array
type FaceName = 'front' | 'back' | 'left' | 'right' | 'top' | 'bottom'
type UvBox = { u: number; v: number; w: number; h: number; d: number }
type TextureInfo = { texture: WebGLTexture; width: number; height: number; scale: number }
type TextureState = { status: 'loading' } | { status: 'ready'; info: TextureInfo } | { status: 'error'; message: string }
type Mesh = { positions: number[]; uvs: number[]; shades: number[] }

const FACE_SHADE: Record<FaceName, number> = { front: 1, back: 0.72, left: 0.82, right: 0.92, top: 1.1, bottom: 0.56 }
const FACES: FaceName[] = ['front', 'back', 'left', 'right', 'top', 'bottom']
const CAPE_ANGLE = (10.8 * Math.PI) / 180
const WING_ANGLE = 0.2617994
const SKIN_SIZES = [[64, 32], [64, 64], [128, 64], [128, 128], [256, 128], [256, 256], [512, 256], [512, 512]]
const MAX_TEXTURES = 96

// ── Матрицы (column-major, как в WebGL) ─────────────────────────────────

function identity(): Mat4 {
  return new Float32Array([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1])
}

function multiply(a: Mat4, b: Mat4): Mat4 {
  const result = new Float32Array(16)
  for (let column = 0; column < 4; column += 1) {
    for (let row = 0; row < 4; row += 1) {
      result[column * 4 + row] =
        a[row] * b[column * 4] + a[4 + row] * b[column * 4 + 1] + a[8 + row] * b[column * 4 + 2] + a[12 + row] * b[column * 4 + 3]
    }
  }
  return result
}

function chain(...matrices: Mat4[]): Mat4 {
  return matrices.reduce((result, matrix) => multiply(result, matrix), identity())
}

function translation(x: number, y: number, z: number): Mat4 {
  const value = identity()
  value[12] = x
  value[13] = y
  value[14] = z
  return value
}

function scaling(x: number, y: number, z: number): Mat4 {
  const value = identity()
  value[0] = x
  value[5] = y
  value[10] = z
  return value
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

function rotationZ(angle: number): Mat4 {
  const c = Math.cos(angle)
  const s = Math.sin(angle)
  return new Float32Array([c, s, 0, 0, -s, c, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1])
}

/** Порядок Эйлера XYZ, как у three.js. */
function euler(x: number, y: number, z: number): Mat4 {
  return chain(rotationX(x), rotationY(y), rotationZ(z))
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

function transform(matrix: Mat4, point: Vec3): Vec3 {
  const [x, y, z] = point
  return [
    matrix[0] * x + matrix[4] * y + matrix[8] * z + matrix[12],
    matrix[1] * x + matrix[5] * y + matrix[9] * z + matrix[13],
    matrix[2] * x + matrix[6] * y + matrix[10] * z + matrix[14],
  ]
}

// ── Геометрия ───────────────────────────────────────────────────────────

/** Области граней в стандартной развёртке Minecraft (в единицах 64-пиксельной текстуры). */
export function boxUvRects(box: UvBox): Record<FaceName, { x: number; y: number; width: number; height: number }> {
  const { u, v, w, h, d } = box
  return {
    top: { x: u + d, y: v, width: w, height: d },
    bottom: { x: u + d + w, y: v, width: w, height: d },
    left: { x: u, y: v + d, width: d, height: h },
    front: { x: u + d, y: v + d, width: w, height: h },
    right: { x: u + d + w, y: v + d, width: d, height: h },
    back: { x: u + d + w + d, y: v + d, width: w, height: h },
  }
}

function addBox(mesh: Mesh, matrix: Mat4, size: Vec3, box: UvBox, texture: TextureInfo) {
  const [width, height, depth] = size
  const x0 = -width / 2
  const x1 = width / 2
  const y0 = -height / 2
  const y1 = height / 2
  const z0 = -depth / 2
  const z1 = depth / 2
  const quads: Record<FaceName, Vec3[]> = {
    front: [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
    back: [[x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0]],
    left: [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
    right: [[x1, y0, z1], [x1, y0, z0], [x1, y1, z0], [x1, y1, z1]],
    top: [[x0, y1, z1], [x1, y1, z1], [x1, y1, z0], [x0, y1, z0]],
    bottom: [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
  }
  const rects = boxUvRects(box)
  for (const face of FACES) {
    const rect = rects[face]
    const u0 = (rect.x * texture.scale) / texture.width
    const u1 = ((rect.x + rect.width) * texture.scale) / texture.width
    const v0 = 1 - (rect.y * texture.scale) / texture.height
    const v1 = 1 - ((rect.y + rect.height) * texture.scale) / texture.height
    const uv = [[u0, v1], [u1, v1], [u1, v0], [u0, v0]]
    const corners = quads[face].map((corner) => transform(matrix, corner))
    for (const index of [0, 1, 2, 0, 2, 3]) {
      mesh.positions.push(...corners[index])
      mesh.uvs.push(...uv[index])
      mesh.shades.push(FACE_SHADE[face])
    }
  }
}

function skinMeshes(model: SkinModel, texture: TextureInfo): { inner: Mesh; outer: Mesh } {
  const inner: Mesh = { positions: [], uvs: [], shades: [] }
  const outer: Mesh = { positions: [], uvs: [], shades: [] }
  const legacy = texture.height * 2 <= texture.width // 64×32: нет второго слоя и отдельных левых конечностей
  const arm = model === 'slim' ? 3 : 4
  const armX = model === 'slim' ? 5.5 : 6
  const parts: Array<{ center: Vec3; size: Vec3; base: UvBox; overlay?: UvBox }> = [
    { center: [0, 12, 0], size: [8, 8, 8], base: { u: 0, v: 0, w: 8, h: 8, d: 8 }, overlay: { u: 32, v: 0, w: 8, h: 8, d: 8 } },
    { center: [0, 2, 0], size: [8, 12, 4], base: { u: 16, v: 16, w: 8, h: 12, d: 4 }, overlay: { u: 16, v: 32, w: 8, h: 12, d: 4 } },
    { center: [-armX, 2, 0], size: [arm, 12, 4], base: { u: 40, v: 16, w: arm, h: 12, d: 4 }, overlay: { u: 40, v: 32, w: arm, h: 12, d: 4 } },
    { center: [armX, 2, 0], size: [arm, 12, 4], base: legacy ? { u: 40, v: 16, w: arm, h: 12, d: 4 } : { u: 32, v: 48, w: arm, h: 12, d: 4 }, overlay: { u: 48, v: 48, w: arm, h: 12, d: 4 } },
    { center: [-1.9, -10, -0.1], size: [4, 12, 4], base: { u: 0, v: 16, w: 4, h: 12, d: 4 }, overlay: { u: 0, v: 32, w: 4, h: 12, d: 4 } },
    { center: [1.9, -10, -0.1], size: [4, 12, 4], base: legacy ? { u: 0, v: 16, w: 4, h: 12, d: 4 } : { u: 16, v: 48, w: 4, h: 12, d: 4 }, overlay: { u: 0, v: 48, w: 4, h: 12, d: 4 } },
  ]
  for (const [index, part] of parts.entries()) {
    const matrix = translation(...part.center)
    addBox(inner, matrix, part.size, part.base, texture)
    if (!legacy && part.overlay) {
      const grow = index === 0 ? 1 : 0.5
      addBox(outer, matrix, [part.size[0] + grow, part.size[1] + grow, part.size[2] + grow], part.overlay, texture)
    } else if (legacy && index === 0) {
      addBox(outer, matrix, [9, 9, 9], { u: 32, v: 0, w: 8, h: 8, d: 8 }, texture)
    }
  }
  return { inner, outer }
}

function capeMesh(texture: TextureInfo): Mesh {
  const mesh: Mesh = { positions: [], uvs: [], shades: [] }
  // Висит от плеч (y = 8) за спиной (z = −2), развёрнут наружной стороной назад.
  const matrix = chain(translation(0, 8, -2), rotationX(CAPE_ANGLE), rotationY(Math.PI), translation(0, -8, 0.5))
  addBox(mesh, matrix, [10, 16, 1], { u: 0, v: 0, w: 10, h: 16, d: 1 }, texture)
  return mesh
}

function elytraMesh(texture: TextureInfo): Mesh {
  const mesh: Mesh = { positions: [], uvs: [], shades: [] }
  const uv: UvBox = { u: 22, v: 0, w: 10, h: 20, d: 2 }
  const base = translation(0, 8, -2)
  const left = chain(base, translation(5, 0, 0), euler(WING_ANGLE, 0.01, WING_ANGLE), translation(-5, -10, -1))
  const right = chain(base, translation(-5, 0, 0), euler(WING_ANGLE, -0.01, -WING_ANGLE), translation(5, -10, -1), scaling(-1, 1, 1))
  addBox(mesh, left, [12, 22, 4], uv, texture)
  addBox(mesh, right, [12, 22, 4], uv, texture)
  return mesh
}

/** Масштаб пикселей текстуры плаща: 64×32 (и HD-кратные) или старый формат 22×17. */
export function capeTextureScale(width: number, height: number): number {
  if (width > 0 && height > 0 && Math.abs(width / height - 2) < 0.01) return width / 64
  if (width > 0 && height > 0 && Math.abs(width / height - 22 / 17) < 0.02) return width / 22
  return width / 64
}

/** Геометрия сцены без WebGL (для проверки развёрток и тестов). */
export function sceneMeshes(
  model: SkinModel,
  skin: { width: number; height: number },
  cape: { width: number; height: number } | null,
  back: BackEquipment,
): { skin: { inner: Mesh; outer: Mesh }; back: Mesh | null } {
  const fake = {} as WebGLTexture
  const skinInfo: TextureInfo = { texture: fake, width: skin.width, height: skin.height, scale: skin.width / 64 }
  const capeInfo: TextureInfo | null = cape ? { texture: fake, width: cape.width, height: cape.height, scale: capeTextureScale(cape.width, cape.height) } : null
  return {
    skin: skinMeshes(model, skinInfo),
    back: capeInfo && back === 'cape' ? capeMesh(capeInfo) : capeInfo && back === 'elytra' ? elytraMesh(capeInfo) : null,
  }
}

// ── Рендерер ─────────────────────────────────────────────────────────────

export class SkinRenderer {
  readonly canvas: HTMLCanvasElement
  private gl: WebGLRenderingContext
  private program: WebGLProgram
  private buffers: { position: WebGLBuffer; uv: WebGLBuffer; shade: WebGLBuffer }
  private attributes: { position: number; uv: number; shade: number }
  private uniforms: { matrix: WebGLUniformLocation | null; texture: WebGLUniformLocation | null }
  private textures = new Map<string, TextureState>()
  private listeners = new Set<() => void>()

  constructor(canvas: HTMLCanvasElement, options: { preserveDrawingBuffer?: boolean } = {}) {
    this.canvas = canvas
    const gl = canvas.getContext('webgl', { alpha: true, antialias: true, premultipliedAlpha: false, preserveDrawingBuffer: Boolean(options.preserveDrawingBuffer) })
    if (!gl) throw new Error('WebGL недоступен')
    this.gl = gl
    this.program = this.createProgram()
    const create = () => {
      const buffer = gl.createBuffer()
      if (!buffer) throw new Error('WebGL buffer')
      return buffer
    }
    this.buffers = { position: create(), uv: create(), shade: create() }
    this.attributes = {
      position: gl.getAttribLocation(this.program, 'aPosition'),
      uv: gl.getAttribLocation(this.program, 'aUv'),
      shade: gl.getAttribLocation(this.program, 'aShade'),
    }
    this.uniforms = { matrix: gl.getUniformLocation(this.program, 'uMatrix'), texture: gl.getUniformLocation(this.program, 'uTexture') }
    gl.enable(gl.DEPTH_TEST)
    gl.enable(gl.BLEND)
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA)
  }

  /** Вызывается, когда догрузилась какая-нибудь текстура — пора перерисовать. */
  onChange(listener: () => void): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  /** Состояние текстуры скина: null — готова, иначе текст для пользователя. */
  skinStatus(url: string): string | null {
    if (!url) return 'Нет активного скина'
    const state = this.request(url, 'skin')
    if (state.status === 'loading') return 'Загрузка текстуры…'
    if (state.status === 'error') return state.message
    return null
  }

  render(scene: SkinScene, width: number, height: number) {
    const { gl } = this
    if (this.canvas.width !== width || this.canvas.height !== height) {
      this.canvas.width = Math.max(1, width)
      this.canvas.height = Math.max(1, height)
    }
    gl.viewport(0, 0, this.canvas.width, this.canvas.height)
    gl.clearColor(0, 0, 0, 0)
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT)
    gl.useProgram(this.program)
    const zoom = scene.zoom ?? 1
    const projection = perspective(Math.PI / 4.2, this.canvas.width / Math.max(1, this.canvas.height), 0.1, 200)
    const view = translation(0, 0, -57 / zoom)
    const matrix = chain(projection, view, rotationX(-scene.pitch), rotationY(scene.yaw))
    const skin = this.request(scene.skin, 'skin')
    const capeState = scene.cape && scene.back ? this.request(scene.cape, 'cape') : undefined
    if (skin.status === 'ready') {
      const meshes = skinMeshes(scene.model, skin.info)
      this.draw(skin.info.texture, meshes.inner, matrix)
      if (capeState?.status === 'ready') this.drawBack(scene.back ?? null, capeState.info, matrix)
      this.draw(skin.info.texture, meshes.outer, matrix)
    } else if (capeState?.status === 'ready') {
      this.drawBack(scene.back ?? null, capeState.info, matrix)
    }
  }

  dispose() {
    const { gl } = this
    for (const state of this.textures.values()) if (state.status === 'ready') gl.deleteTexture(state.info.texture)
    this.textures.clear()
    gl.deleteBuffer(this.buffers.position)
    gl.deleteBuffer(this.buffers.uv)
    gl.deleteBuffer(this.buffers.shade)
    gl.deleteProgram(this.program)
    this.listeners.clear()
  }

  private drawBack(back: BackEquipment, texture: TextureInfo, matrix: Mat4) {
    if (back === 'elytra') {
      if (Math.abs(texture.width / texture.height - 2) < 0.01) this.draw(texture.texture, elytraMesh(texture), matrix)
    } else if (back === 'cape') {
      this.draw(texture.texture, capeMesh(texture), matrix)
    }
  }

  private request(url: string, kind: 'skin' | 'cape'): TextureState {
    const existing = this.textures.get(url)
    if (existing) return existing
    if (this.textures.size >= MAX_TEXTURES) {
      const [oldest, state] = this.textures.entries().next().value as [string, TextureState]
      if (state.status === 'ready') this.gl.deleteTexture(state.info.texture)
      this.textures.delete(oldest)
    }
    const loading: TextureState = { status: 'loading' }
    this.textures.set(url, loading)
    const image = new Image()
    image.crossOrigin = 'anonymous'
    image.decoding = 'async'
    image.referrerPolicy = 'no-referrer'
    image.onload = () => {
      if (this.textures.get(url) !== loading) return
      const width = image.naturalWidth
      const height = image.naturalHeight
      if (kind === 'skin' && !SKIN_SIZES.some(([w, h]) => w === width && h === height)) {
        this.textures.set(url, { status: 'error', message: 'Неподдерживаемый размер PNG-скина' })
      } else {
        try {
          const texture = this.upload(image)
          const scale = kind === 'skin' ? width / 64 : capeTextureScale(width, height)
          this.textures.set(url, { status: 'ready', info: { texture, width, height, scale } })
        } catch {
          this.textures.set(url, { status: 'error', message: 'Браузер не разрешил загрузить текстуру' })
        }
      }
      this.notify()
    }
    image.onerror = () => {
      if (this.textures.get(url) !== loading) return
      this.textures.set(url, { status: 'error', message: kind === 'skin' ? 'Не удалось загрузить текстуру скина' : 'Не удалось загрузить текстуру плаща' })
      this.notify()
    }
    image.src = url
    return loading
  }

  private notify() {
    for (const listener of [...this.listeners]) listener()
  }

  private upload(image: HTMLImageElement): WebGLTexture {
    const { gl } = this
    const texture = gl.createTexture()
    if (!texture) throw new Error('WebGL texture')
    gl.bindTexture(gl.TEXTURE_2D, texture)
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, 1)
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, image)
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST)
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST)
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)
    return texture
  }

  private draw(texture: WebGLTexture, mesh: Mesh, matrix: Mat4) {
    if (!mesh.positions.length) return
    const { gl, buffers, attributes } = this
    const bind = (buffer: WebGLBuffer, data: number[], location: number, size: number) => {
      if (location < 0) return
      gl.bindBuffer(gl.ARRAY_BUFFER, buffer)
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(data), gl.STREAM_DRAW)
      gl.enableVertexAttribArray(location)
      gl.vertexAttribPointer(location, size, gl.FLOAT, false, 0, 0)
    }
    bind(buffers.position, mesh.positions, attributes.position, 3)
    bind(buffers.uv, mesh.uvs, attributes.uv, 2)
    bind(buffers.shade, mesh.shades, attributes.shade, 1)
    gl.uniformMatrix4fv(this.uniforms.matrix, false, matrix)
    gl.activeTexture(gl.TEXTURE0)
    gl.bindTexture(gl.TEXTURE_2D, texture)
    gl.uniform1i(this.uniforms.texture, 0)
    gl.drawArrays(gl.TRIANGLES, 0, mesh.positions.length / 3)
  }

  private createProgram(): WebGLProgram {
    const { gl } = this
    const compile = (type: number, source: string) => {
      const shader = gl.createShader(type)
      if (!shader) throw new Error('WebGL shader')
      gl.shaderSource(shader, source)
      gl.compileShader(shader)
      if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
        const message = gl.getShaderInfoLog(shader) || 'Shader compilation failed'
        gl.deleteShader(shader)
        throw new Error(message)
      }
      return shader
    }
    const vertex = compile(gl.VERTEX_SHADER, `
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
    const fragment = compile(gl.FRAGMENT_SHADER, `
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
    const program = gl.createProgram()
    if (!program) throw new Error('WebGL program')
    gl.attachShader(program, vertex)
    gl.attachShader(program, fragment)
    gl.linkProgram(program)
    gl.deleteShader(vertex)
    gl.deleteShader(fragment)
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      const message = gl.getProgramInfoLog(program) || 'Program linking failed'
      gl.deleteProgram(program)
      throw new Error(message)
    }
    return program
  }
}

// Один WebGL-контекст на все миниатюры: браузер держит ограниченное число
// контекстов, и десяток превью в библиотеке скинов иначе терял бы часть из них.
let sharedThumbnailRenderer: SkinRenderer | null | undefined

export function thumbnailRenderer(): SkinRenderer | null {
  if (sharedThumbnailRenderer !== undefined) return sharedThumbnailRenderer
  try {
    sharedThumbnailRenderer = new SkinRenderer(document.createElement('canvas'), { preserveDrawingBuffer: true })
  } catch {
    sharedThumbnailRenderer = null
  }
  return sharedThumbnailRenderer
}
