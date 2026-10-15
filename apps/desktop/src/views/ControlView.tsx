import { getVersion } from '@tauri-apps/api/app'
import { useEffect, useState } from 'react'
import { ClientForm, ServerForm } from '@/components/SettingsForm'
import { FingerprintRow } from '@/components/FingerprintRow'
import { LogPanel } from '@/components/LogPanel'
import { StatusBar } from '@/components/StatusBar'
import { Button } from '@/components/ui/button'
import { ipc } from '@/lib/ipc'
import type { DaemonInfo, Settings, Status } from '@/types'

interface Props {
  settings: Settings
  onChange: (change: (s: Settings) => Settings) => void
  daemon: DaemonInfo | null
  status: Status
  running: boolean
  problem: string | null
  setProblem: (text: string | null) => void
  onStart: () => void
  onStop: () => void
  onShutdownDaemon: () => void
}

/** Choose client or server, set it up, Start/Stop, and watch the log. */
export function ControlView({ settings, daemon, onChange, status, running, problem, setProblem, onStart, onStop, onShutdownDaemon }: Props) {
  const [fingerprint, setFingerprint] = useState('')
  const mode = settings.mode
  const [appVersion, setAppVersion] = useState('')
  useEffect(() => {
    void getVersion().then(setAppVersion)
  }, [])

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
      {daemon && (
        <p className="text-xs text-muted-foreground">
          Terhubung ke daemon (PID {daemon.pid}, dimulai oleh {daemon.started_by === 'cli' ? 'CLI' : 'aplikasi'}). Menutup
          jendela tidak menghentikan sesi.
          {daemon.compatible && appVersion && daemon.app_version !== appVersion && (
            <span style={{ color: 'var(--warn)' }}>
              {' '}
              Versi daemon {daemon.app_version} berbeda dari aplikasi {appVersion}.
            </span>
          )}
        </p>
      )}
      {daemon && !daemon.compatible && (
        <div className="flex items-center gap-3 rounded-lg border p-3 text-sm" style={{ borderColor: 'var(--bad)', color: 'var(--bad)' }}>
          <span className="flex-1">
            Daemon ini memakai protokol {daemon.protocol}, aplikasi ini memakai yang lain. Hentikan dulu daemon lama agar sesi
            baru bisa dijalankan.
          </span>
          <Button variant="destructive" size="sm" onClick={onShutdownDaemon}>
            Hentikan daemon lama
          </Button>
        </div>
      )}
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

      <LogPanel daemonPid={daemon?.pid} />
    </div>
  )
}
