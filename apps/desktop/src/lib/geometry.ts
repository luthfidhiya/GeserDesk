// Pure geometry for the screen-arrangement stage: no DOM, no React.

import type { Draft, Placement, Point, Rect, Screen } from '@/types'

type Placed = Pick<Screen, 'name' | 'monitors'>

/** Smallest width or height a machine may be shrunk to on the desk. */
export const MIN_DESK_SIZE = 64

/**
 * `s`'s monitors on the shared desk when it is placed at `o`. A machine given a
 * size is stretched about the top-left of its monitors, the way the server does.
 */
export function deskRects(s: Pick<Screen, 'monitors'>, o: Placement): Rect[] {
  if (s.monitors.length === 0) return []
  const n = union(s.monitors)
  const sx = o.w && n.w > 0 ? o.w / n.w : 1
  const sy = o.h && n.h > 0 ? o.h / n.h : 1
  const edge = (v: number, base: number, origin: number, k: number) => origin + base + Math.round((v - base) * k)
  return s.monitors.map((m) => {
    const x = edge(m.x, n.x, o.x, sx)
    const y = edge(m.y, n.y, o.y, sy)
    return { x, y, w: edge(m.x + m.w, n.x, o.x, sx) - x, h: edge(m.y + m.h, n.y, o.y, sy) - y }
  })
}

/** Whether `o` gives the machine a desk size different from its real one. */
export function isStretched(s: Pick<Screen, 'monitors'>, o: Placement | undefined): boolean {
  if (!o || s.monitors.length === 0) return false
  const n = union(s.monitors)
  return (o.w !== undefined && o.w !== n.w) || (o.h !== undefined && o.h !== n.h)
}

/** `o` without a desk size: the machine is its real size again. */
export function unstretched(o: Placement): Placement {
  return { x: o.x, y: o.y }
}

/** The desk size to send for `s`: `null` for its real size. */
export function deskSize(s: Pick<Screen, 'monitors'>, o: Placement): { w: number; h: number } | null {
  if (!isStretched(s, o)) return null
  const n = union(s.monitors)
  return { w: o.w ?? n.w, h: o.h ?? n.h }
}

export type Grip = { fx: -1 | 0 | 1; fy: -1 | 0 | 1 }

/**
 * `s` placed at `start`, with the side(s) named by `grip` dragged by `(dx, dy)`.
 * The opposite sides stay where they are. A side near an edge of another machine
 * (within `thr`) is pulled onto it; `thr` of 0 turns that off.
 */
export function resized(
  s: Pick<Screen, 'monitors'>,
  start: Placement,
  grip: Grip,
  dx: number,
  dy: number,
  others: Rect[],
  thr: number,
): Placement {
  const n = union(s.monitors)
  const box = union(deskRects(s, start))
  const pull = (v: number, edges: number[]) => {
    let best = v
    for (const e of edges) if (Math.abs(e - v) <= thr && (best === v || Math.abs(e - v) < Math.abs(best - v))) best = e
    return best
  }
  const xs = others.flatMap((r) => [r.x, r.x + r.w])
  const ys = others.flatMap((r) => [r.y, r.y + r.h])
  let [l, t, r, b] = [box.x, box.y, box.x + box.w, box.y + box.h]
  if (grip.fx === 1) r = Math.max(l + MIN_DESK_SIZE, pull(Math.round(r + dx), xs))
  if (grip.fx === -1) l = Math.min(r - MIN_DESK_SIZE, pull(Math.round(l + dx), xs))
  if (grip.fy === 1) b = Math.max(t + MIN_DESK_SIZE, pull(Math.round(b + dy), ys))
  if (grip.fy === -1) t = Math.min(b - MIN_DESK_SIZE, pull(Math.round(t + dy), ys))
  // The box's top-left sits at origin + the monitors' top-left, so give that back.
  return { x: l - n.x, y: t - n.y, w: r - l, h: b - t }
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
  const me = deskRects(self, { ...draft[name], ...o })
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
