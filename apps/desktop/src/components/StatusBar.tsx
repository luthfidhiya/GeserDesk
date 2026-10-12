import { describe, TONE_COLOR } from '@/lib/status'
import type { Status } from '@/types'

export function StatusBar({ status }: { status: Status }) {
  const { text, tone } = describe(status)
  return (
    <div className="flex items-center gap-2 text-sm" style={{ color: TONE_COLOR[tone] }}>
      <span aria-hidden>●</span>
      <span>{text}</span>
    </div>
  )
}
