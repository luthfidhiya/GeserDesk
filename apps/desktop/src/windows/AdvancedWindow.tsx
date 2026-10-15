import { useState } from 'react'
import { ServerForm } from '@/components/SettingsForm'
import { useSession } from '@/hooks/useSession'
import { useSettings } from '@/hooks/useSettings'
import { useTheme } from '@/hooks/useTheme'

/** The server config choice, port and switching settings, in a window of their own. */
export default function AdvancedWindow() {
  const { settings, update } = useSettings()
  const session = useSession()
  const [problem, setProblem] = useState<string | null>(null)
  useTheme(settings?.theme)
  if (!settings) return null

  // A running session decides the mode, as in the main window.
  const mode = session.running && session.daemon?.mode ? session.daemon.mode : settings.mode
  const shown = mode !== settings.mode ? { ...settings, mode } : settings

  return (
    <div className="flex h-full flex-col gap-3 overflow-y-auto p-4">
      {mode === 'server' ? (
        <ServerForm bare settings={shown} disabled={session.running} onChange={update} onProblem={setProblem} />
      ) : (
        <p className="text-sm text-muted-foreground">
          Komputer ini berjalan sebagai client. Pengaturan lanjutan (berkas konfigurasi, port, pindah layar,
          clipboard) milik server; ganti ke mode Server di jendela utama untuk mengubahnya.
        </p>
      )}
      {problem && (
        <p className="text-sm" style={{ color: 'var(--bad)' }}>
          {problem}
        </p>
      )}
    </div>
  )
}
