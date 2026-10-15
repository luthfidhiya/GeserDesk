import { X } from 'lucide-react'
import { useEffect } from 'react'
import appIcon from '@/assets/app-icon.png'
import { PendingApprovals } from '@/components/PendingApprovals'
import type { Pending } from '@/types'

interface Props {
  pending: Pending[]
  problem: string | null
  onTrust: (p: Pending) => void
  onDismiss: (p: Pending) => void
  onClose: () => void
}

/**
 * Computers asking to connect, over the main window: a plain header (the logo and the
 * title at the left, close at the right) on the panel's own background, and the
 * requests below it, which are plain text with lines between them (no boxes). A click
 * outside or Escape closes it.
 */
export function ApprovalsPanel({ pending, problem, onTrust, onDismiss, onClose }: Props) {
  useEffect(() => {
    const esc = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', esc)
    return () => window.removeEventListener('keydown', esc)
  }, [onClose])

  return (
    <div className="fixed inset-x-0 bottom-0 top-8 z-40 flex items-start justify-center bg-black/50 p-6" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-label="Permintaan sambung"
        className="flex max-h-full w-full max-w-lg flex-col overflow-hidden rounded-md border bg-card shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="flex h-8 shrink-0 select-none items-center gap-2 pl-3 pr-2 text-xs text-muted-foreground">
          <img src={appIcon} alt="" className="h-4 w-4 shrink-0" />
          <span className="flex-1 truncate font-medium">Permintaan sambung</span>
          <button
            className="inline-flex size-6 items-center justify-center rounded hover:bg-accent hover:text-foreground"
            aria-label="Tutup"
            onClick={onClose}
          >
            <X size={14} />
          </button>
        </header>
        <div className="min-h-0 flex-1 space-y-3 overflow-y-auto px-5 pb-4">
          <PendingApprovals pending={pending} onTrust={onTrust} onDismiss={onDismiss} />
          {problem && (
            <p className="text-sm" style={{ color: 'var(--bad)' }}>
              {problem}
            </p>
          )}
        </div>
      </div>
    </div>
  )
}
