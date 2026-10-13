import { useEffect, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { EVENTS, on } from '@/lib/ipc'
import type { TransferProgress } from '@/types'

// Long enough to read, short enough not to linger once a small copy finishes.
const HIDE_AFTER_DONE_MS = 2500
const HIDE_AFTER_FAILED_MS = 7000

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

/** The content of the small always-on-top window `transfer_window.rs` opens.
 * It only ever shows the latest report; the Rust side decides when the window
 * itself is shown, this decides when it hides again. */
export default function TransferPopup() {
  const [p, setP] = useState<TransferProgress | null>(null)
  const hideTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(
    () =>
      on<TransferProgress>(EVENTS.transfer, (next) => {
        setP(next)
        if (hideTimer.current) clearTimeout(hideTimer.current)
        if (next.state.kind === 'done') {
          hideTimer.current = setTimeout(() => void getCurrentWindow().hide(), HIDE_AFTER_DONE_MS)
        } else if (next.state.kind === 'failed') {
          hideTimer.current = setTimeout(() => void getCurrentWindow().hide(), HIDE_AFTER_FAILED_MS)
        }
      }),
    [],
  )

  if (!p) return null

  const pct = p.bytes_total > 0 ? Math.min(100, Math.round((100 * p.bytes_done) / p.bytes_total)) : 0
  const title =
    p.state.kind === 'done'
      ? 'Selesai'
      : p.state.kind === 'failed'
        ? 'Gagal'
        : `Menerima dari ${p.peer}`

  return (
    <div
      className="flex h-full select-none flex-col gap-2 rounded-lg border p-3 text-sm"
      style={{ background: 'var(--popover)', color: 'var(--popover-foreground)', borderColor: 'var(--border)' }}
    >
      <div className="flex items-center justify-between">
        <span className="font-medium">{title}</span>
        <button
          aria-label="Tutup"
          className="opacity-60 hover:opacity-100"
          onClick={() => void getCurrentWindow().hide()}
        >
          ✕
        </button>
      </div>
      {p.state.kind === 'failed' ? (
        <p className="truncate" style={{ color: 'var(--bad)' }} title={p.state.detail}>
          {p.state.detail}
        </p>
      ) : (
        <>
          <p className="truncate" style={{ color: 'var(--muted-foreground)' }} title={p.file}>
            {p.file || '…'} {p.file_count > 1 && `(${p.file_index + 1}/${p.file_count})`}
          </p>
          <div className="h-2 overflow-hidden rounded-full" style={{ background: 'var(--muted)' }}>
            <div
              className="h-full rounded-full transition-[width]"
              style={{
                width: `${p.state.kind === 'done' ? 100 : pct}%`,
                background: p.state.kind === 'done' ? 'var(--ok)' : 'var(--primary)',
              }}
            />
          </div>
          <p className="text-xs" style={{ color: 'var(--muted-foreground)' }}>
            {formatBytes(p.bytes_done)} / {formatBytes(p.bytes_total)}
          </p>
        </>
      )}
    </div>
  )
}
