import { describe, expect, it } from 'vitest'
import { autoArrange, neighbours, overlapping, snap, union } from './geometry'
import type { Draft } from '@/types'

const mon = (x: number, y: number, w: number, h: number) => ({ x, y, w, h })
const linux = { name: 'linux', local: true, monitors: [mon(0, 0, 1920, 1080)] }
const win = { name: 'win', local: false, monitors: [mon(0, 0, 1280, 1024)] }
const screens = [linux, win]

describe('overlapping', () => {
  it('lets machines touch', () => {
    const draft: Draft = { linux: { x: 0, y: 0 }, win: { x: 1920, y: 0 } }
    expect(overlapping(screens, draft).size).toBe(0)
  })
  it('flags both machines that share pixels', () => {
    const draft: Draft = { linux: { x: 0, y: 0 }, win: { x: 1000, y: 0 } }
    expect([...overlapping(screens, draft)].sort()).toEqual(['linux', 'win'])
  })
})

describe('neighbours', () => {
  it('names the side along a shared edge', () => {
    const draft: Draft = { linux: { x: 0, y: 0 }, win: { x: 1920, y: 100 } }
    expect(neighbours(screens, draft, 'linux')).toEqual(['win (kanan)'])
    expect(neighbours(screens, draft, 'win')).toEqual(['linux (kiri)'])
  })
  it('ignores machines that only touch at a corner or are apart', () => {
    expect(neighbours(screens, { linux: { x: 0, y: 0 }, win: { x: 1920, y: 1080 } }, 'linux')).toEqual([])
    expect(neighbours(screens, { linux: { x: 0, y: 0 }, win: { x: 3000, y: 0 } }, 'linux')).toEqual([])
  })
  it('knows above and below', () => {
    const draft: Draft = { linux: { x: 0, y: 0 }, win: { x: 100, y: 1080 } }
    expect(neighbours(screens, draft, 'linux')).toEqual(['win (bawah)'])
    expect(neighbours(screens, draft, 'win')).toEqual(['linux (atas)'])
  })
})

describe('snap', () => {
  const draft: Draft = { linux: { x: 0, y: 0 }, win: { x: 5000, y: 5000 } }
  it('pulls a nearby edge flush and aligns the tops', () => {
    expect(snap(screens, draft, 'win', { x: 1935, y: 12 }, 24)).toEqual({ x: 1920, y: 0 })
  })
  it('leaves a machine alone when nothing is within reach', () => {
    expect(snap(screens, draft, 'win', { x: 3000, y: 3000 }, 24)).toEqual({ x: 3000, y: 3000 })
  })
  it('snaps each axis on its own', () => {
    expect(snap(screens, draft, 'win', { x: 1925, y: 400 }, 24)).toEqual({ x: 1920, y: 400 })
  })
})

describe('autoArrange', () => {
  it('puts the server first and centres the others on it', () => {
    const out = autoArrange([win, linux])
    expect(out.linux).toEqual({ x: 0, y: 0 })
    expect(out.win).toEqual({ x: 1920, y: Math.round((1080 - 1024) / 2) })
  })
  it('makes a layout with no overlap even for offset monitors', () => {
    const odd = { name: 'odd', local: false, monitors: [mon(-1280, 0, 1280, 1024), mon(0, 0, 1920, 1080)] }
    const out = autoArrange([linux, odd])
    expect(overlapping([linux, odd], out).size).toBe(0)
    expect(union([mon(0, 0, 10, 10), mon(5, 5, 10, 10)])).toEqual(mon(0, 0, 15, 15))
  })
})
