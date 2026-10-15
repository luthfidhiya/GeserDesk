import { useEffect, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { EVENTS, ipc, on } from '@/lib/ipc'
import {
  BLANK_HIDE_MS,
  HIDE_AFTER_FAILED_MS,
  STALL_AFTER_MS,
  formatBytes,
  heading,
  hideDelay,
  percent,
} from '@/lib/transfer'
import { useLang, useT } from '@/lib/i18n'
import type { TransferProgress } from '@/types'

const hideWindow = () => void getCurrentWindow().hide()

/** The content of the small always-on-top window `transfer_window.rs` opens.
 * It only ever shows the latest report; the Rust side decides when the window
 * itself is shown, this decides when it hides again: a little after a pull
 * ends, and also when a running one has gone quiet for too long. */
export default function TransferPopup() {
  const t = useT()
  const lang = useLang()
  const [p, setP] = useState<TransferProgress | null>(null)
  const [stalled, setStalled] = useState(false)
  /** The `seq` the user pressed Cancel on, until the daemon answers. */
  const [cancelling, setCancelling] = useState<number | null>(null)
  const hideTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const stallTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const heardEvent = useRef(false)

  useEffect(() => {
    const clear = () => {
      if (hideTimer.current) clearTimeout(hideTimer.current)
      if (stallTimer.current) clearTimeout(stallTimer.current)
    }
    const receive = (next: TransferProgress) => {
      clear()
      setP(next)
      setStalled(false)
      const delay = hideDelay(next.state)
      if (delay !== null) {
        setCancelling(null)
        hideTimer.current = setTimeout(hideWindow, delay)
      } else {
        // Running: if nothing more is heard, say so and get out of the way. A
        // later report raises the window again (the Rust side shows it on every
        // running report).
        stallTimer.current = setTimeout(() => {
          setStalled(true)
          hideTimer.current = setTimeout(hideWindow, HIDE_AFTER_FAILED_MS)
        }, STALL_AFTER_MS)
      }
    }
    const off = on<TransferProgress>(EVENTS.transfer, (next) => {
      heardEvent.current = true
      receive(next)
    })
    // The reports sent while this page was loading are gone: ask for the latest.
    // One that arrives in the meantime is newer, so only an empty window takes it.
    void ipc.latestTransfer().then((latest) => {
      if (latest && !heardEvent.current) receive(latest)
    }).catch(() => {})
    // And if there is nothing to show at all, do not stay up empty.
    hideTimer.current = setTimeout(hideWindow, BLANK_HIDE_MS)
    return () => {
      clear()
      off()
    }
  }, [])

  if (!p) return null

  const running = p.state.kind === 'running'
  const isCancelling = cancelling === p.seq
  const pct = percent(p.bytes_done, p.bytes_total)

  const cancel = () => {
    setCancelling(p.seq)
    ipc.cancelTransfer().catch(() => setCancelling(null))
  }

  return (
    <div
      className="flex h-full select-none flex-col gap-2 rounded-lg border p-3 text-sm"
      style={{ background: 'var(--popover)', color: 'var(--popover-foreground)', borderColor: 'var(--border)' }}
    >
      <div className="flex items-center justify-between">
        <span className="font-medium">{heading(p, stalled, isCancelling, lang)}</span>
        <button
          aria-label={t('Sembunyikan', 'Hide')}
          title={running ? t('Sembunyikan (transfer tetap berjalan)', 'Hide (the transfer keeps going)') : t('Tutup', 'Close')}
          className="opacity-60 hover:opacity-100"
          onClick={hideWindow}
        >
          ✕
        </button>
      </div>
      {p.state.kind === 'failed' ? (
        <p className="truncate" style={{ color: 'var(--bad)' }} title={p.state.detail}>
          {p.state.detail}
        </p>
      ) : p.state.kind === 'cancelled' ? (
        <p style={{ color: 'var(--muted-foreground)' }}>{t('Berkas sementara dihapus.', 'The temporary files were removed.')}</p>
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
                background: p.state.kind === 'done' ? 'var(--ok)' : stalled ? 'var(--warn)' : 'var(--primary)',
              }}
            />
          </div>
          <div className="flex items-center justify-between">
            <p className="text-xs" style={{ color: 'var(--muted-foreground)' }}>
              {formatBytes(p.bytes_done)} / {formatBytes(p.bytes_total)}
            </p>
            {running && (
              <button
                className="rounded-md border px-2 py-0.5 text-xs disabled:opacity-50"
                style={{ borderColor: 'var(--border)' }}
                disabled={isCancelling}
                onClick={cancel}
              >
                {t('Batal', 'Cancel')}
              </button>
            )}
          </div>
        </>
      )}
    </div>
  )
}
