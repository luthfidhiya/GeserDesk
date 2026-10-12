import type { Status } from '@/types'

export type Tone = 'ok' | 'warn' | 'bad' | 'idle'

export const TONE_COLOR: Record<Tone, string> = {
  ok: 'var(--ok)',
  warn: 'var(--warn)',
  bad: 'var(--bad)',
  idle: 'var(--muted-foreground)',
}

/** The one-line text and colour tone for a status. */
export function describe(s: Status): { text: string; tone: Tone } {
  switch (s.kind) {
    case 'stopped':
      return { text: 'Berhenti', tone: 'idle' }
    case 'server_running':
      return { text: 'Server berjalan', tone: 'ok' }
    case 'connecting':
      return { text: 'Menyambung…', tone: 'warn' }
    case 'connected':
      return { text: 'Tersambung', tone: 'ok' }
    case 'awaiting_approval':
      return { text: 'Menunggu persetujuan di server', tone: 'warn' }
    case 'retrying':
      return { text: `Terputus, mencoba lagi (${s.detail})`, tone: 'warn' }
    case 'fatal':
      return { text: `Gagal: ${s.detail}`, tone: 'bad' }
    case 'error':
      return { text: `Kesalahan: ${s.detail}`, tone: 'bad' }
  }
}
