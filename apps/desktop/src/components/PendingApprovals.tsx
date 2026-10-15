import { Button } from '@/components/ui/button'
import type { Pending } from '@/types'

interface Props {
  pending: Pending[]
  onTrust: (p: Pending) => void
  onDismiss: (p: Pending) => void
}

/** The requests, one under the other with a line between: no boxes, the panel around them is the box. */
export function PendingApprovals({ pending, onTrust, onDismiss }: Props) {
  if (pending.length === 0) return null
  return (
    <div className="divide-y">
      <p className="pb-3 text-xs text-muted-foreground">
        Cocokkan sidik jari dengan yang tampil di komputer itu sebelum mempercayainya.
      </p>
      {pending.map((p) => (
        <div key={p.name + p.fingerprint} className="space-y-1 py-3 first:pt-3">
          <div className="font-semibold">{p.name}</div>
          {p.replaces ? (
            <p className="text-xs font-semibold" style={{ color: 'var(--bad)' }}>
              Sertifikat BERBEDA dari yang dipercaya sebelumnya untuk &quot;{p.name}&quot;. Hanya setujui kalau Anda
              memang mengganti/menginstal ulang komputer itu.
            </p>
          ) : p.new_screen ? (
            <p className="text-xs text-muted-foreground">
              Komputer baru, belum ada di konfigurasi. Meminta tersambung sebagai &quot;{p.name}&quot; dari {p.addr}.
              Kalau dipercayai, ia ditambahkan sebagai layar baru di sebelah kanan (bisa digeser di tab susunan
              layar) dan konfigurasi diperbarui.
            </p>
          ) : (
            <p className="text-xs text-muted-foreground">
              Meminta tersambung sebagai &quot;{p.name}&quot; dari {p.addr}.
            </p>
          )}
          <code className="block break-all font-mono text-xs">{p.fingerprint}</code>
          <div className="flex gap-2 pt-1">
            <Button size="sm" onClick={() => onTrust(p)}>
              {p.new_screen ? 'Percayai dan tambahkan' : 'Percayai'}
            </Button>
            <Button size="sm" variant="outline" onClick={() => onDismiss(p)}>
              Abaikan
            </Button>
          </div>
        </div>
      ))}
    </div>
  )
}
