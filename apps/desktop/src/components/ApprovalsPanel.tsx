import { Panel } from '@/components/Panel'
import { PendingApprovals } from '@/components/PendingApprovals'
import type { Pending } from '@/types'

interface Props {
  pending: Pending[]
  problem: string | null
  onTrust: (p: Pending) => void
  onDismiss: (p: Pending) => void
  onClose: () => void
}

/** Computers asking to connect, to be trusted or turned away. */
export function ApprovalsPanel({ pending, problem, onTrust, onDismiss, onClose }: Props) {
  return (
    <Panel title="Permintaan sambung" onClose={onClose}>
      <PendingApprovals pending={pending} onTrust={onTrust} onDismiss={onDismiss} />
      {problem && (
        <p className="text-sm" style={{ color: 'var(--bad)' }}>
          {problem}
        </p>
      )}
    </Panel>
  )
}
