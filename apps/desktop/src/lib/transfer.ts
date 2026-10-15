import { pick, type Lang } from '@/lib/i18n'
import type { TransferProgress, TransferState } from '@/types'

// Long enough to read, short enough not to linger once a small copy finishes.
export const HIDE_AFTER_DONE_MS = 2500
export const HIDE_AFTER_CANCELLED_MS = 2000
export const HIDE_AFTER_FAILED_MS = 7000

/**
 * A pull that is running but has reported nothing for this long is taken to be
 * stuck. The Rust side gives up on a chunk that does not arrive after 30 s
 * (`READ_TIMEOUT` in geserdesk-clipboard) and reports it as failed, so this is
 * only the backstop for a report that never comes at all (the daemon gone, the
 * channel to it dead): longer than that timeout, so the real answer wins.
 */
export const STALL_AFTER_MS = 45_000

/**
 * A window that was raised but never heard a report (they went out while its page
 * was still loading, and there was nothing to ask for) hides itself after this.
 */
export const BLANK_HIDE_MS = 10_000

/** How long after a report the window should hide itself; `null` while the pull runs. */
export function hideDelay(state: TransferState): number | null {
  switch (state.kind) {
    case 'done':
      return HIDE_AFTER_DONE_MS
    case 'cancelled':
      return HIDE_AFTER_CANCELLED_MS
    case 'failed':
      return HIDE_AFTER_FAILED_MS
    case 'running':
      return null
  }
}

/** Whether nothing has been heard of a pull that was last reported at `lastReportAt`. */
export function isStalled(lastReportAt: number, now: number): boolean {
  return now - lastReportAt >= STALL_AFTER_MS
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`
  return `${(n / (1024 * 1024)).toFixed(1)} MB`
}

export function percent(done: number, total: number): number {
  return total > 0 ? Math.min(100, Math.round((100 * done) / total)) : 0
}

/** The window's heading for the latest report. */
export function heading(p: TransferProgress, stalled: boolean, cancelling: boolean, lang: Lang = 'id'): string {
  const t = (id: string, en: string) => pick(lang, id, en)
  switch (p.state.kind) {
    case 'done':
      return t('Selesai', 'Done')
    case 'cancelled':
      return t('Dibatalkan', 'Cancelled')
    case 'failed':
      return t('Gagal', 'Failed')
    case 'running':
      if (stalled) return t(`Tidak ada kabar dari ${p.peer}`, `Nothing heard from ${p.peer}`)
      return cancelling ? t('Membatalkan…', 'Cancelling…') : t(`Menerima dari ${p.peer}`, `Receiving from ${p.peer}`)
  }
}
