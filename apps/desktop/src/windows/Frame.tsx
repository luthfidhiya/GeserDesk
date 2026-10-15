import { getCurrentWindow } from '@tauri-apps/api/window'
import { Minus, Square, X } from 'lucide-react'
import type { ReactNode } from 'react'
import appIcon from '@/assets/app-icon.png'

const button = 'inline-flex h-8 w-[46px] items-center justify-center'

/**
 * A window's own title bar, in place of the system one: the app logo and the name
 * of what is open at the left, the window buttons at the right. The bar drags the
 * window and a double click maximises it.
 */
export default function Frame({ title, children }: { title: string; children: ReactNode }) {
  const win = getCurrentWindow()
  return (
    <div className="flex h-full flex-col">
      <header className="titlebar flex h-8 shrink-0 select-none items-center text-xs">
        <div data-tauri-drag-region className="flex h-full min-w-0 flex-1 items-center gap-2 pl-3">
          <img src={appIcon} alt="" className="pointer-events-none h-4 w-4 shrink-0" />
          <span className="pointer-events-none truncate">{title}</span>
        </div>
        <button className={button} aria-label="Perkecil" onClick={() => void win.minimize()}>
          <Minus size={14} />
        </button>
        <button className={button} aria-label="Perbesar" onClick={() => void win.toggleMaximize()}>
          <Square size={12} />
        </button>
        <button className={`${button} hover:!bg-red-600 hover:!text-white`} aria-label="Tutup" onClick={() => void win.close()}>
          <X size={14} />
        </button>
      </header>
      <div className="min-h-0 flex-1">{children}</div>
    </div>
  )
}
