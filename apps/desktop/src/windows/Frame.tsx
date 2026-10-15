import { getCurrentWindow } from '@tauri-apps/api/window'
import { Minus, Square, X } from 'lucide-react'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import appIcon from '@/assets/app-icon.png'
import { useT } from '@/lib/i18n'

export interface MenuItem {
  label: string
  run: () => void
}
export interface Menu {
  label: string
  items: MenuItem[]
}

const button = 'inline-flex h-8 w-[46px] items-center justify-center'

/** A menu in the title bar: opens on click, and moves to a neighbour on hover once one is open. */
function MenuBar({ menus }: { menus: Menu[] }) {
  const [open, setOpen] = useState<number | null>(null)
  const box = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (open === null) return
    const away = (e: MouseEvent) => !box.current?.contains(e.target as Node) && setOpen(null)
    const esc = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(null)
    window.addEventListener('mousedown', away)
    window.addEventListener('keydown', esc)
    return () => {
      window.removeEventListener('mousedown', away)
      window.removeEventListener('keydown', esc)
    }
  }, [open])
  return (
    <div ref={box} className="flex h-full items-center">
      {menus.map((m, i) => (
        <div key={m.label} className="relative h-full">
          <button
            className="menu-top h-full px-2.5"
            data-open={open === i || undefined}
            onClick={() => setOpen(open === i ? null : i)}
            onMouseEnter={() => open !== null && setOpen(i)}
          >
            {m.label}
          </button>
          {open === i && (
            <div className="menu-pop absolute left-0 top-full z-50 min-w-48 rounded-md border py-1 shadow-lg">
              {m.items.map((it) => (
                <button
                  key={it.label}
                  className="menu-item block w-full px-3 py-1.5 text-left"
                  onClick={() => {
                    setOpen(null)
                    it.run()
                  }}
                >
                  {it.label}
                </button>
              ))}
            </div>
          )}
        </div>
      ))}
    </div>
  )
}

/**
 * The window's own title bar, in place of the system one: the app logo, then the
 * menus or the name of what is open, and the window buttons at the right. The bar
 * drags the window and a double click maximises it.
 */
export default function Frame({ title, menus, children }: { title?: string; menus?: Menu[]; children: ReactNode }) {
  const win = getCurrentWindow()
  const t = useT()
  return (
    <div className="flex h-full flex-col">
      <header className="titlebar flex h-8 shrink-0 select-none items-center text-xs">
        <img src={appIcon} alt="" data-tauri-drag-region className="h-4 w-4 shrink-0 mx-3" />
        {menus && <MenuBar menus={menus} />}
        <div data-tauri-drag-region className="flex h-full min-w-0 flex-1 items-center">
          {title && <span className="pointer-events-none truncate">{title}</span>}
        </div>
        <button className={button} aria-label={t('Perkecil', 'Minimize')} onClick={() => void win.minimize()}>
          <Minus size={14} />
        </button>
        <button className={button} aria-label={t('Perbesar', 'Maximize')} onClick={() => void win.toggleMaximize()}>
          <Square size={12} />
        </button>
        <button className={`${button} hover:!bg-red-600 hover:!text-white`} aria-label={t('Tutup', 'Close')} onClick={() => void win.close()}>
          <X size={14} />
        </button>
      </header>
      <div className="min-h-0 flex-1">{children}</div>
    </div>
  )
}
