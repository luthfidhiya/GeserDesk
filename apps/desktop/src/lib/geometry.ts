// Pure geometry for the screen-arrangement stage: no DOM, no React.

import type { Draft, Point, Rect, Screen } from '@/types'

type Placed = Pick<Screen, 'name' | 'monitors'>

/** `s`'s monitors on the shared desk when its origin is `o`. */
export function deskRects(s: Pick<Screen, 'monitors'>, o: Point): Rect[] {
  return s.monitors.map((m) => ({ x: m.x + o.x, y: m.y + o.y, w: m.w, h: m.h }))
}

export function union(rects: Rect[]): Rect {
  let l = Infinity
  let t = Infinity
  let r = -Infinity
  let b = -Infinity
  for (const m of rects) {
    l = Math.min(l, m.x)
    t = Math.min(t, m.y)
    r = Math.max(r, m.x + m.w)
    b = Math.max(b, m.y + m.h)
  }
  return { x: l, y: t, w: r - l, h: b - t }
}

/** Whether two rectangles share any area (touching edges do not count). */
export function hit(a: Rect, b: Rect): boolean {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/** Names of the screens that overlap another one. */
export function overlapping(screens: Placed[], draft: Draft): Set<string> {
  const bad = new Set<string>()
  const all = screens.map((s) => [s.name, deskRects(s, draft[s.name])] as const)
  for (let i = 0; i < all.length; i++) {
    for (let j = i + 1; j < all.length; j++) {
      if (all[i][1].some((a) => all[j][1].some((b) => hit(a, b)))) {
        bad.add(all[i][0])
        bad.add(all[j][0])
      }
    }
  }
  return bad
}

const overlap = (a0: number, a1: number, b0: number, b1: number) =>
  Math.min(a1, b1) - Math.max(a0, b0) > 0

/** Which machines touch `name` along a shared stretch of edge, and on which side. */
export function neighbours(screens: Placed[], draft: Draft, name: string): string[] {
  const self = screens.find((s) => s.name === name)
  if (!self) return []
  const me = deskRects(self, draft[name])
  const found = new Map<string, string>()
  for (const s of screens) {
    if (s.name === name) continue
    for (const b of deskRects(s, draft[s.name])) {
      for (const a of me) {
        let side: string | null = null
        if (overlap(a.y, a.y + a.h, b.y, b.y + b.h)) {
          if (a.x + a.w === b.x) side = 'kanan'
          else if (b.x + b.w === a.x) side = 'kiri'
        }
        if (!side && overlap(a.x, a.x + a.w, b.x, b.x + b.w)) {
          if (a.y + a.h === b.y) side = 'bawah'
          else if (b.y + b.h === a.y) side = 'atas'
        }
        if (side) found.set(s.name + side, `${s.name} (${side})`)
      }
    }
  }
  return [...found.values()]
}

/** Move `o` so `name`'s monitors touch or align with the others' within `thr`. */
export function snap(screens: Placed[], draft: Draft, name: string, o: Point, thr: number): Point {
  const self = screens.find((s) => s.name === name)
  if (!self) return o
  const me = deskRects(self, o)
  const others = screens.filter((s) => s.name !== name).flatMap((s) => deskRects(s, draft[s.name]))
  let bestX: number | null = null
  let bestY: number | null = null
  const consider = (cur: number | null, d: number) =>
    Math.abs(d) <= thr && (cur === null || Math.abs(d) < Math.abs(cur)) ? d : cur
  for (const a of me) {
    for (const b of others) {
      for (const d of [b.x + b.w - a.x, b.x - (a.x + a.w), b.x - a.x, b.x + b.w - (a.x + a.w)])
        bestX = consider(bestX, d)
      for (const d of [b.y + b.h - a.y, b.y - (a.y + a.h), b.y - a.y, b.y + b.h - (a.y + a.h)])
        bestY = consider(bestY, d)
    }
  }
  return { x: o.x + (bestX ?? 0), y: o.y + (bestY ?? 0) }
}

/** Server first, then the others, side by side; vertically centred on the server. */
export function autoArrange(screens: (Placed & { local: boolean })[]): Draft {
  const order = [...screens].sort((a, b) => Number(b.local) - Number(a.local))
  const origin = { x: 0, y: 0 }
  const boxes = order.map((s) => union(deskRects(s, origin)))
  const base = boxes[0]?.h ?? 0
  const out: Draft = {}
  let x = 0
  order.forEach((s, i) => {
    const u = boxes[i]
    out[s.name] = { x: x - u.x, y: Math.round((base - u.h) / 2) - u.y }
    x += u.w
  })
  return out
}
