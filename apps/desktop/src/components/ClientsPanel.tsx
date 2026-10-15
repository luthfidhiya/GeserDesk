import { useEffect, useState } from 'react'
import { Panel } from '@/components/Panel'
import { PendingApprovals } from '@/components/PendingApprovals'
import { RevokeButton } from '@/components/ScreenList'
import { ScreenSwitch } from '@/components/ScreenSwitch'
import { Badge } from '@/components/ui/badge'
import { errorText, ipc } from '@/lib/ipc'
import type { DeskSnapshot, Pending, TrustedClient } from '@/types'

const POLL_MS = 2000

interface Props {
  /** The running server's desk, to mark who is connected; none when no server runs. */
  desk: DeskSnapshot | null
  refresh: () => Promise<void>
  /** Answers to the requests waiting at the top, where they can be found again after the panel for them was closed. */
  onTrust: (p: Pending) => void
  onDismiss: (p: Pending) => void
  problem: string | null
  onClose: () => void
}

/**
 * The computers waiting to be trusted, then the ones this server trusts, read from its
 * config file, so the list works with no server running. While one runs, the ones connected now are marked, and a computer
 * connected without being trusted (TLS off) is listed too.
 */
export function ClientsPanel({ desk, refresh, onTrust, onDismiss, problem: answering, onClose }: Props) {
  const [trusted, setTrusted] = useState<TrustedClient[] | null>(null)
  const [problem, setProblem] = useState<string | null>(null)

  const load = () =>
    ipc.trustedClients().then(
      (list) => {
        setTrusted(list)
        setProblem(null)
      },
      (e) => setProblem(errorText(e)),
    )

  useEffect(() => {
    void load()
    const id = window.setInterval(() => void load(), POLL_MS)
    return () => window.clearInterval(id)
  }, [])

  const revoke = async (name: string) => {
    try {
      await ipc.revokeClient(name)
      await Promise.all([load(), refresh()])
    } catch (e) {
      setProblem(`Gagal: ${errorText(e)}`)
    }
  }

  const setOff = async (name: string, disabled: boolean) => {
    try {
      await ipc.setScreenDisabled(name, disabled)
      await Promise.all([load(), refresh()])
    } catch (e) {
      setProblem(`Gagal: ${errorText(e)}`)
    }
  }

  const connected = new Set((desk?.screens ?? []).filter((s) => !s.local && s.connected).map((s) => s.name))
  const names = [...new Set([...(trusted ?? []).map((c) => c.name), ...connected])]
  // Off by the running server's word when there is one, else by the config file's.
  const isOff = (name: string) =>
    desk?.screens.find((s) => s.name === name)?.disabled ?? trusted?.find((c) => c.name === name)?.disabled ?? false
  const monitors = (name: string) => desk?.screens.find((s) => s.name === name)?.monitors ?? []
  const fingerprint = (name: string) => trusted?.find((c) => c.name === name)?.fingerprint
  // Connected ones first, then by name.
  names.sort((a, b) => Number(connected.has(b)) - Number(connected.has(a)) || a.localeCompare(b))

  return (
    <Panel title="Daftar client" onClose={onClose}>
      {desk && desk.pending.length > 0 && (
        <section className="space-y-1">
          <h2 className="text-sm font-semibold">Menunggu persetujuan</h2>
          <PendingApprovals pending={desk.pending} onTrust={onTrust} onDismiss={onDismiss} />
          {answering && (
            <p className="text-sm" style={{ color: 'var(--bad)' }}>
              {answering}
            </p>
          )}
        </section>
      )}
      {trusted && names.length === 0 && (
        <p className="text-sm text-muted-foreground">Belum ada client yang dipercaya.</p>
      )}
      <div className="divide-y">
        {names.map((name) => {
          const fp = fingerprint(name)
          const sizes = monitors(name).map((m) => `${m.w}×${m.h}`).join(', ')
          return (
            <div key={name} className="space-y-1 py-3 first:pt-0">
              <div className="flex items-center gap-2 font-semibold">
                {name}
                {fp && (
                  <Badge variant="outline" title={fp} style={{ color: 'var(--ok)' }}>
                    terpercaya
                  </Badge>
                )}
                <span className="ml-auto">
                  <ScreenSwitch
                    name={name}
                    connected={connected.has(name)}
                    disabled={isOff(name)}
                    onToggle={(d) => void setOff(name, d)}
                  />
                </span>
                {fp && <RevokeButton onConfirm={() => void revoke(name)} />}
              </div>
              <p className="text-xs text-muted-foreground">
                {isOff(name)
                  ? 'Nonaktif'
                  : connected.has(name)
                    ? `Terhubung · ${monitors(name).length} monitor: ${sizes}`
                    : 'Tidak terhubung'}
              </p>
            </div>
          )
        })}
      </div>
      {problem && (
        <p className="text-sm" style={{ color: 'var(--bad)' }}>
          {problem}
        </p>
      )}
    </Panel>
  )
}
