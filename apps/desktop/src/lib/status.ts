import { pick, type Lang } from '@/lib/i18n'
import type { Status } from '@/types'

export type Tone = 'ok' | 'warn' | 'bad' | 'idle'

export const TONE_COLOR: Record<Tone, string> = {
  ok: 'var(--ok)',
  warn: 'var(--warn)',
  bad: 'var(--bad)',
  idle: 'var(--muted-foreground)',
}

/** The one-line text and colour tone for a status. */
export function describe(s: Status, lang: Lang): { text: string; tone: Tone } {
  const t = (id: string, en: string) => pick(lang, id, en)
  switch (s.kind) {
    case 'stopped':
      return { text: t('Berhenti', 'Stopped'), tone: 'idle' }
    case 'server_running':
      return { text: t('Server berjalan', 'Server running'), tone: 'ok' }
    case 'connecting':
      return { text: t('Menyambung…', 'Connecting…'), tone: 'warn' }
    case 'connected':
      return { text: t('Tersambung', 'Connected'), tone: 'ok' }
    case 'awaiting_approval':
      return { text: t('Menunggu persetujuan di server', 'Waiting for approval on the server'), tone: 'warn' }
    case 'retrying':
      return { text: t(`Terputus, mencoba lagi (${s.detail})`, `Disconnected, retrying (${s.detail})`), tone: 'warn' }
    case 'fatal':
      return { text: t(`Gagal: ${s.detail}`, `Failed: ${s.detail}`), tone: 'bad' }
    case 'error':
      return { text: t(`Kesalahan: ${s.detail}`, `Error: ${s.detail}`), tone: 'bad' }
  }
}
