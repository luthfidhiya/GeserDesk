import { useRef, useState } from 'react'
import { deskRects, snap, union } from '@/lib/geometry'
import type { Draft, Point, Rect, Screen } from '@/types'

export const screenColor = (i: number) => `var(--c${i % 6})`

interface Props {
  screens: Screen[]
  draft: Draft
  selected: string | null
  bad: Set<string>
  onSelect: (name: string) => void
  onMove: (name: string, to: Point) => void
  onDragging: (dragging: boolean) => void
}

interface Drag {
  name: string
  startPt: Point
  start: Point
  /** The view is frozen while dragging, or it would rescale under the pointer. */
  vb: Rect
}

/** The desk: every machine's monitors as boxes you can drag. */
export function ScreenStage({ screens, draft, selected, bad, onSelect, onMove, onDragging }: Props) {
  const svg = useRef<SVGSVGElement>(null)
  const [drag, setDrag] = useState<Drag | null>(null)

  const u = union(screens.flatMap((s) => deskRects(s, draft[s.name])))
  // Generous margin so a machine can be dragged to any side of the others.
  const pad = Math.max(u.w * 0.25, u.h * 0.5) + 200
  const vb = drag?.vb ?? { x: u.x - pad, y: u.y - pad, w: u.w + pad * 2, h: u.h + pad * 2 }
  const fs = vb.w / 60

  const toDesk = (ev: React.PointerEvent): Point => {
    const el = svg.current!
    const pt = el.createSVGPoint()
    pt.x = ev.clientX
    pt.y = ev.clientY
    const p = pt.matrixTransform(el.getScreenCTM()!.inverse())
    return { x: p.x, y: p.y }
  }

  const gridX: number[] = []
  const gridY: number[] = []
  const step = 1000
  for (let g = Math.ceil(vb.x / step) * step; g < vb.x + vb.w; g += step) gridX.push(g)
  for (let g = Math.ceil(vb.y / step) * step; g < vb.y + vb.h; g += step) gridY.push(g)

  return (
    <svg
      ref={svg}
      role="img"
      aria-label="Susunan layar"
      viewBox={`${vb.x} ${vb.y} ${vb.w} ${vb.h}`}
      className="h-full w-full touch-none select-none"
      onPointerMove={(ev) => {
        if (!drag) return
        const p = toDesk(ev)
        const raw = {
          x: Math.round(drag.start.x + p.x - drag.startPt.x),
          y: Math.round(drag.start.y + p.y - drag.startPt.y),
        }
        const pxPerUnit = svg.current!.getBoundingClientRect().width / drag.vb.w
        onMove(drag.name, ev.altKey ? raw : snap(screens, draft, drag.name, raw, 24 / pxPerUnit))
      }}
      onPointerUp={() => {
        setDrag(null)
        onDragging(false)
      }}
      onPointerCancel={() => {
        setDrag(null)
        onDragging(false)
      }}
    >
      {gridX.map((x) => (
        <line key={`x${x}`} x1={x} y1={vb.y} x2={x} y2={vb.y + vb.h} stroke="var(--border)" strokeWidth={1} vectorEffect="non-scaling-stroke" />
      ))}
      {gridY.map((y) => (
        <line key={`y${y}`} x1={vb.x} y1={y} x2={vb.x + vb.w} y2={y} stroke="var(--border)" strokeWidth={1} vectorEffect="non-scaling-stroke" />
      ))}
      {screens.map((s, i) => {
        const color = bad.has(s.name) ? 'var(--bad)' : screenColor(i)
        const rects = deskRects(s, draft[s.name])
        const first = rects[0]
        return (
          <g
            key={s.name}
            className="cursor-grab"
            opacity={s.connected ? 1 : 0.55}
            onPointerDown={(ev) => {
              ev.preventDefault()
              svg.current!.setPointerCapture(ev.pointerId)
              onSelect(s.name)
              onDragging(true)
              setDrag({ name: s.name, startPt: toDesk(ev), start: { ...draft[s.name] }, vb })
            }}
          >
            {rects.map((m, k) => (
              <g key={k}>
                <rect
                  x={m.x} y={m.y} width={m.w} height={m.h} rx={fs / 3}
                  fill={color} fillOpacity={0.25} stroke={color}
                  strokeWidth={s.name === selected ? 3 : 1.5}
                  vectorEffect="non-scaling-stroke"
                />
                <text x={m.x + fs * 0.6} y={m.y + m.h - fs * 0.6} fontSize={fs * 0.8} fill="var(--muted-foreground)" pointerEvents="none">
                  {`${m.w}×${m.h}`}
                  {s.monitors.length > 1 ? ` · #${k + 1}` : ''}
                </text>
              </g>
            ))}
            <text x={first.x + fs * 0.6} y={first.y + fs * 1.5} fontSize={fs * 1.15} fontWeight={600} fill="var(--foreground)" pointerEvents="none">
              {s.name + (s.local ? ' (server)' : '')}
            </text>
          </g>
        )
      })}
    </svg>
  )
}
