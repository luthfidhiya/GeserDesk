import { getCurrentWindow } from '@tauri-apps/api/window'
import { useEffect, useState } from 'react'
import { PendingApprovals } from '@/components/PendingApprovals'
import { useDesk } from '@/hooks/useDesk'
import { useSession } from '@/hooks/useSession'
import { useSettings } from '@/hooks/useSettings'
import { useTheme } from '@/hooks/useTheme'
import { errorText, ipc } from '@/lib/ipc'
import type { Pending } from '@/types'

/**
 * Computers asking to connect, to be trusted or turned away. The main window opens
 * it when a new request arrives; it closes itself once none is left.
 */
export default function ApprovalsWindow() {
  const { settings } = useSettings()
  const session = useSession()
  const [problem, setProblem] = useState<string | null>(null)
  useTheme(settings?.theme)
  const { desk, refresh } = useDesk(session.running, false)
  const pending = desk?.pending

  // Nothing left to decide (answered here, or the request timed out or was answered elsewhere).
  useEffect(() => {
    if (pending && pending.length === 0) void getCurrentWindow().close()
  }, [pending])

  const run = async (action: () => Promise<void>) => {
    try {
      await action()
      setProblem(null)
      await refresh()
    } catch (e) {
      setProblem(`Gagal: ${errorText(e)}`)
    }
  }

  return (
    <div className="flex h-full flex-col gap-3 overflow-y-auto p-4">
      <PendingApprovals
        pending={pending ?? []}
        onTrust={(p: Pending) => void run(() => ipc.approveClient(p.name, p.fingerprint))}
        onDismiss={(p: Pending) => void run(() => ipc.dismissClient(p.name, p.fingerprint))}
      />
      {problem && (
        <p className="text-sm" style={{ color: 'var(--bad)' }}>
          {problem}
        </p>
      )}
    </div>
  )
}
