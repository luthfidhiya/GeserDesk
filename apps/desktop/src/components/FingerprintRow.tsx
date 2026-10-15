import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { Button } from '@/components/ui/button'
import type { Mode } from '@/types'
import { useT } from '@/lib/i18n'

export function FingerprintRow({ mode, fingerprint }: { mode: Mode; fingerprint: string }) {
  const t = useT()
  const what =
    mode === 'client'
      ? t('Sidik jari komputer ini (cocokkan dengan yang tampil di server)', "This computer's fingerprint (match it with the one shown on the server)")
      : t('Sidik jari server ini (cocokkan dengan yang tampil di client)', "This server's fingerprint (match it with the one shown on the client)")
  return (
    <div className="space-y-1">
      <p className="text-xs text-muted-foreground">{what}</p>
      <div className="flex flex-wrap items-center gap-2">
        <code className="break-all font-mono text-xs">{fingerprint}</code>
        {fingerprint && (
          <Button variant="outline" size="sm" onClick={() => void writeText(fingerprint)}>
            {t('Salin', 'Copy')}
          </Button>
        )}
      </div>
    </div>
  )
}
