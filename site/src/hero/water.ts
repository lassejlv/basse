/**
 * The hero's moving water, ported from site/dither/scene.py so the canvas and the
 * dith-rendered still agree. Each frame rebuilds the rows below the quay from the
 * pre-water source strip and dithers them in the exact palette with Floyd–Steinberg,
 * the algorithm dith used for the still above the water line. Error diffusion runs on
 * linear light, which matches dith's dot density for these two-colour passes.
 */

export interface SceneMeta {
  readonly width: number
  readonly height: number
  readonly unit: number
  readonly waterTop: number
  readonly lampX: number
  readonly sourceTop: number
}

type Rgb = readonly [number, number, number]

const TAU = Math.PI * 2

const hex = (value: number): Rgb => [(value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff]
const QUAY = hex(0x0e1113)
const FOG = hex(0xd8dee3)
const AMBER = hex(0xf5a524)

const linear = (c: number) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4)
const luminance = ([r, g, b]: Rgb) =>
  0.2126 * linear(r / 255) + 0.7152 * linear(g / 255) + 0.0722 * linear(b / 255)

/** Maps a 0..255 grey to its position between quay black (0) and the ink (1). */
const ramp = (ink: Rgb) => {
  const dark = luminance(QUAY)
  const span = luminance(ink) - dark
  return Float32Array.from({ length: 256 }, (_, v) => (linear(v / 255) - dark) / span)
}

const FOG_RAMP = ramp(FOG)
const AMBER_RAMP = ramp(AMBER)

export interface Water {
  readonly rows: number
  /** Renders the water for a phase in 0..1 (the motion loops at 1) into an RGBA buffer of width × rows. */
  readonly render: (phase: number, out: Uint8ClampedArray) => void
}

export const makeWater = (meta: SceneMeta, source: Uint8ClampedArray): Water => {
  const { width: W, height: H, unit: U, waterTop: top, sourceTop, lampX } = meta
  const rows = H - top
  const lum = new Uint8Array(W * rows)
  const amb = new Uint8Array(W * rows)
  const fog = new Uint8Array(W * rows)
  const amber = new Uint8Array(W * rows)
  const err = new Float32Array(W * (rows + 1) + 1)

  const diffuse = (field: Uint8Array, table: Float32Array, ink: Uint8Array) => {
    err.fill(0)
    for (let i = 0; i < W * rows; i++) {
      const x = i % W
      const value = table[field[i]!]! + err[i]!
      const on = value >= 0.5 ? 1 : 0
      ink[i] = on
      const e = value - on
      if (x + 1 < W) {
        err[i + 1]! += (e * 7) / 16
        err[i + W + 1]! += e / 16
      }
      if (x > 0) err[i + W - 1]! += (e * 3) / 16
      err[i + W]! += (e * 5) / 16
    }
  }

  const render = (phase: number, out: Uint8ClampedArray) => {
    for (let k = 0; k < rows; k++) {
      const depth = k / rows
      const row = Math.max(0, Math.max(0, Math.trunc(top - k * 0.9) - 2) - sourceTop) * W
      const band = Math.sin((k / U) * (0.45 - 0.25 * depth) + phase * TAU * 3)
      const shift = Math.trunc(6 * U * (1 + depth * 4) * Math.sin((k / U) * 0.21 + phase * TAU * 2))
      const fade = (1 - depth) ** 1.1
      const keep = (band > -0.2 ? 0.75 : 0.15) * fade
      const lit = (band > 0.1 ? 0.9 : 0) * fade
      for (let x = 0; x < W; x++) {
        const s = (row + ((((x - shift) % W) + W) % W)) * 4
        const i = k * W + x
        lum[i] = Math.round(255 * (0.02 + (source[s]! / 255 - 0.02) * keep))
        amb[i] = Math.round(source[s + 1]! * lit)
      }
      if (Math.sin((k / U) * 0.9 + phase * TAU * 4) < 0.25) continue
      const half = (9 + 45 * depth) * U * (0.6 + 0.4 * Math.sin(k * 0.37 + phase * TAU))
      const x0 = Math.max(0, Math.trunc(lampX - half + 4 * U * Math.sin(k * 0.13 + phase * TAU * 2)))
      const x1 = Math.min(W, Math.trunc(lampX + half))
      const streak = Math.round(255 * 0.75 * (1 - depth) ** 1.2)
      for (let x = x0; x < x1; x++) amb[k * W + x] = Math.max(amb[k * W + x]!, streak)
    }
    diffuse(lum, FOG_RAMP, fog)
    diffuse(amb, AMBER_RAMP, amber)
    for (let i = 0; i < W * rows; i++) {
      const [r, g, b] = amber[i] ? AMBER : fog[i] ? FOG : QUAY
      out[i * 4] = r
      out[i * 4 + 1] = g
      out[i * 4 + 2] = b
      out[i * 4 + 3] = 255
    }
  }

  return { rows, render }
}
