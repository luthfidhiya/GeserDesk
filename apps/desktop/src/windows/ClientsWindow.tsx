import { useCallback, useEffect, useState } from 'react'
import { RevokeButton } from '@/components/ScreenList'
import { Badge } from '@/components/ui/badge'
import { useDesk } from '@/hooks/useDesk'
import { useSession } from '@/hooks/useSession'
import { useSettings } from '@/hooks/useSettings'
import { useTheme } from '@/hooks/useTheme'
import { errorText, ipc } from '@/lib/ipc'
import type { TrustedClient } from '@/types'

const POLL_MS = 2000

/**
 * The computers this server trusts, read from its config file, so it works with no
 * server running. While one runs, the ones connected now are marked, and a computer
 * connected without being trusted (TLS off) is listed too.
 */
export default function ClientsWindow() {
  const { settings } = useSettings()
  const session = useSession()
  const [trusted, setTrusted] = useState<TrustedClient[] | null>(null)
  const [problem, setProblem] = useState<string | null>(null)
  useTheme(settings?.theme)
  // A running session decides the mode, as in the main window.
  const mode = session.running && session.daemon?.mode ? session.daemon.mode : settings?.mode
  const { desk, refresh: refreshDesk } = useDesk(session.running && mode === 'server', false)

  const load = useCallback(async () => {
    try {
      setTrusted(await ipc.trustedClients())
      setProblem(null)
    } catch (e) {
      setProblem(errorText(e))
    }
  }, [])

  useEffect(() => {
    void load()
    const id = window.setInterval(() => void load(), POLL_MS)
    return () => window.clearInterval(id)
  }, [load])

  if (!settings) return null

  const revoke = async (name: string) => {
    try {
      await ipc.revokeClient(name)
      await Promise.all([load(), refreshDesk()])
    } catch (e) {
      setProblem(`Gagal: ${errorText(e)}`)
    }
  }

  const connected = new Set((desk?.screens ?? []).filter((s) => !s.local && s.connected).map((s) => s.name))
  const names = [...new Set([...(trusted ?? []).map((c) => c.name), ...connected])]
  const monitors = (name: string) => desk?.screens.find((s) => s.name === name)?.monitors ?? []
  const fingerprint = (name: string) => trusted?.find((c) => c.name === name)?.fingerprint
  // Connected ones first, then by name.
  names.sort((a, b) => Number(connected.has(b)) - Number(connected.has(a)) || a.localeCompare(b))

  return (
    <div className="flex h-full flex-col gap-3 overflow-y-auto p-4">
      {trusted && names.length === 0 && (
        <p className="text-sm text-muted-foreground">Belum ada client yang dipercaya.</p>
      )}
      <div className="space-y-2">
        {names.map((name) => {
          const fp = fingerprint(name)
          const sizes = monitors(name).map((m) => `${m.w}×${m.h}`).join(', ')
          return (
            <div key={name} className="rounded-lg border p-3">
              <div className="flex items-center gap-2 font-semibold">
                {name}
                {fp && (
                  <Badge variant="outline" title={fp} style={{ color: 'var(--ok)' }}>
                    terpercaya
                  </Badge>
                )}
                <span
                  className="ml-auto size-2.5 rounded-full"
                  title={connected.has(name) ? 'terhubung' : 'tidak terhubung'}
                  style={{ background: connected.has(name) ? 'var(--ok)' : 'var(--muted-foreground)' }}
                />
              </div>
              <p className="mb-1 text-xs text-muted-foreground">
                {connected.has(name) ? `Terhubung · ${monitors(name).length} monitor: ${sizes}` : 'Tidak terhubung'}
              </p>
              {fp && <RevokeButton onConfirm={() => void revoke(name)} />}
            </div>
          )
        })}
      </div>
      {problem && (
        <p className="text-sm" style={{ color: 'var(--bad)' }}>
          {problem}
        </p>
      )}
    </div>
  )
}
