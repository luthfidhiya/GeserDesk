import { X } from 'lucide-react'
import { useEffect, useRef, useState, type PointerEvent, type ReactNode } from 'react'
import appIcon from '@/assets/app-icon.png'

/** Where a panel may be dragged to, as an offset from where it opens: always with its header in reach. */
function clamp(x: number, y: number) {
  const reachX = window.innerWidth / 2 - 80
  return {
    x: Math.min(Math.max(x, -reachX), reachX),
    y: Math.min(Math.max(y, -16), window.innerHeight - 120),
  }
}

/**
 * A panel over the main window: a plain header (the logo and the title at the left,
 * close at the right) on the panel's own background, the content below it. Drag it by
 * the header; a click outside or Escape closes it.
 */
export function Panel({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  const [offset, setOffset] = useState({ x: 0, y: 0 })
  const grab = useRef<{ x: number; y: number; from: { x: number; y: number } } | null>(null)

  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', esc)
    return () => window.removeEventListener('keydown', esc)
  }, [onClose])

  const start = (e: PointerEvent<HTMLElement>) => {
    if ((e.target as HTMLElement).closest('button')) return
    e.currentTarget.setPointerCapture(e.pointerId)
    grab.current = { x: e.clientX, y: e.clientY, from: offset }
  }
  const move = (e: PointerEvent<HTMLElement>) => {
    const g = grab.current
    if (g) setOffset(clamp(g.from.x + e.clientX - g.x, g.from.y + e.clientY - g.y))
  }
  const end = () => {
    grab.current = null
  }

  return (
    <div className="fixed inset-x-0 bottom-0 top-8 z-40 flex items-start justify-center bg-black/50 p-6" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-label={title}
        className="flex max-h-full w-full max-w-lg flex-col overflow-hidden rounded-md border bg-card shadow-2xl"
        style={{ transform: `translate(${offset.x}px, ${offset.y}px)` }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header
          className="flex h-8 shrink-0 cursor-grab select-none items-center gap-2 pl-3 pr-2 text-xs text-muted-foreground active:cursor-grabbing"
          onPointerDown={start}
          onPointerMove={move}
          onPointerUp={end}
          onPointerCancel={end}
          onDoubleClick={() => setOffset({ x: 0, y: 0 })}
        >
          <img src={appIcon} alt="" className="pointer-events-none h-4 w-4 shrink-0" />
          <span className="flex-1 truncate font-medium">{title}</span>
          <button
            className="inline-flex size-6 items-center justify-center rounded hover:bg-accent hover:text-foreground"
            aria-label="Tutup"
            onClick={onClose}
          >
            <X size={14} />
          </button>
        </header>
        <div className="min-h-0 flex-1 space-y-3 overflow-y-auto px-5 pb-4">{children}</div>
      </div>
    </div>
  )
}
