import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { Button } from '@/components/ui/button'
import type { Mode } from '@/types'

export function FingerprintRow({ mode, fingerprint }: { mode: Mode; fingerprint: string }) {
  const what =
    mode === 'client'
      ? 'Sidik jari komputer ini (cocokkan dengan yang tampil di server)'
      : 'Sidik jari server ini (cocokkan dengan yang tampil di client)'
  return (
    <div className="space-y-1">
      <p className="text-xs text-muted-foreground">{what}</p>
      <div className="flex flex-wrap items-center gap-2">
        <code className="break-all font-mono text-xs">{fingerprint}</code>
        {fingerprint && (
          <Button variant="outline" size="sm" onClick={() => void writeText(fingerprint)}>
            Salin
          </Button>
        )}
      </div>
    </div>
  )
}
