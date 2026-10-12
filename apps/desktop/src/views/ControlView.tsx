import { useEffect, useState } from 'react'
import { ClientForm, ServerForm } from '@/components/SettingsForm'
import { FingerprintRow } from '@/components/FingerprintRow'
import { LogPanel } from '@/components/LogPanel'
import { StatusBar } from '@/components/StatusBar'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Label } from '@/components/ui/label'
import { ipc } from '@/lib/ipc'
import type { Settings, Status } from '@/types'

interface Props {
  settings: Settings
  onChange: (change: (s: Settings) => Settings) => void
  status: Status
  running: boolean
  problem: string | null
  setProblem: (text: string | null) => void
  onStart: () => void
  onStop: () => void
}

/** Choose client or server, set it up, Start/Stop, and watch the log. */
export function ControlView({ settings, onChange, status, running, problem, setProblem, onStart, onStop }: Props) {
  const [fingerprint, setFingerprint] = useState('')
  const mode = settings.mode

  useEffect(() => {
    let current = true
    void ipc.ownFingerprint(mode).then((f) => current && setFingerprint(f ?? ''))
    return () => {
      current = false
    }
  }, [mode])

  return (
    <div className="flex h-full min-h-0 flex-col gap-4">
      {mode === 'client' ? (
        <ClientForm settings={settings} disabled={running} onChange={onChange} onProblem={setProblem} />
      ) : (
        <ServerForm settings={settings} disabled={running} onChange={onChange} onProblem={setProblem} />
      )}

      <FingerprintRow mode={mode} fingerprint={fingerprint} />

      <div className="flex flex-wrap items-center gap-3">
        {running ? (
          <Button variant="destructive" className="w-28" onClick={onStop}>
            Stop
          </Button>
        ) : (
          <Button className="w-28" onClick={onStart}>
            Start
          </Button>
        )}
        <StatusBar status={status} />
      </div>
      {problem && (
        <p className="text-sm" style={{ color: 'var(--bad)' }}>
          {problem}
        </p>
      )}
      {status.kind === 'awaiting_approval' && (
        <p className="text-xs" style={{ color: 'var(--warn)' }}>
          Buka tab Pengaturan monitor di komputer server, lalu setujui komputer ini. ({status.detail})
        </p>
      )}

      <div className="flex items-center gap-2">
        <Checkbox
          id="tray"
          checked={settings.close_to_tray}
          onCheckedChange={(v) => onChange((s) => ({ ...s, close_to_tray: v === true }))}
        />
        <Label htmlFor="tray">Tutup jendela = sembunyikan ke tray (tetap berjalan)</Label>
      </div>

      <LogPanel />
    </div>
  )
}
