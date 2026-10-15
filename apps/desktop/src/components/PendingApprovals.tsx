import { Button } from '@/components/ui/button'
import type { Pending } from '@/types'
import { useT } from '@/lib/i18n'

interface Props {
  pending: Pending[]
  onTrust: (p: Pending) => void
  onDismiss: (p: Pending) => void
}

/** The requests, one under the other with a line between: no boxes, the panel around them is the box. */
export function PendingApprovals({ pending, onTrust, onDismiss }: Props) {
  const t = useT()
  if (pending.length === 0) return null
  return (
    <div className="divide-y">
      <p className="pb-3 text-xs text-muted-foreground">
        {t(
          'Cocokkan sidik jari dengan yang tampil di komputer itu sebelum mempercayainya.',
          'Match the fingerprint with the one shown on that computer before trusting it.',
        )}
      </p>
      {pending.map((p) => (
        <div key={p.name + p.fingerprint} className="space-y-1 py-3 first:pt-3">
          <div className="font-semibold">{p.name}</div>
          {p.replaces ? (
            <p className="text-xs font-semibold" style={{ color: 'var(--bad)' }}>
              {t(
                `Sertifikat BERBEDA dari yang dipercaya sebelumnya untuk "${p.name}". Hanya setujui kalau Anda memang mengganti/menginstal ulang komputer itu.`,
                `The certificate DIFFERS from the one trusted before for "${p.name}". Only approve if you really replaced or reinstalled that computer.`,
              )}
            </p>
          ) : p.new_screen ? (
            <p className="text-xs text-muted-foreground">
              {t(
                `Komputer baru, belum ada di konfigurasi. Meminta tersambung sebagai "${p.name}" dari ${p.addr}. Kalau dipercayai, ia ditambahkan sebagai layar baru di sebelah kanan (bisa digeser di tab Monitor) dan konfigurasi diperbarui.`,
                `A new computer, not in the configuration yet. It asks to connect as "${p.name}" from ${p.addr}. If trusted, it is added as a new screen on the right (it can be moved in the Monitors tab) and the configuration is updated.`,
              )}
            </p>
          ) : (
            <p className="text-xs text-muted-foreground">
              {t(`Meminta tersambung sebagai "${p.name}" dari ${p.addr}.`, `Asks to connect as "${p.name}" from ${p.addr}.`)}
            </p>
          )}
          <code className="block break-all font-mono text-xs">{p.fingerprint}</code>
          <div className="flex gap-2 pt-1">
            <Button size="sm" onClick={() => onTrust(p)}>
              {p.new_screen ? t('Percayai dan tambahkan', 'Trust and add') : t('Percayai', 'Trust')}
            </Button>
            <Button size="sm" variant="outline" onClick={() => onDismiss(p)}>
              {t('Abaikan', 'Dismiss')}
            </Button>
          </div>
        </div>
      ))}
    </div>
  )
}
