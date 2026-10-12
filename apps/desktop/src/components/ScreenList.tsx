import { useRef, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { neighbours } from '@/lib/geometry'
import { screenColor } from '@/components/ScreenStage'
import type { Draft, Screen } from '@/types'

interface Props {
  screens: Screen[]
  draft: Draft
  selected: string | null
  onSelect: (name: string) => void
  onNumber: (name: string, axis: 'x' | 'y', value: number) => void
  onRevoke: (name: string) => void
  onEditing: (editing: boolean) => void
}

export function ScreenList({ screens, draft, selected, onSelect, onNumber, onRevoke, onEditing }: Props) {
  return (
    <div className="space-y-2">
      {screens.map((s, i) => {
        const near = neighbours(screens, draft, s.name)
        const sizes = s.monitors.map((m) => `${m.w}×${m.h}`).join(', ')
        return (
          <div
            key={s.name}
            onClick={() => onSelect(s.name)}
            className={`cursor-pointer rounded-lg border p-3 ${s.name === selected ? 'border-primary ring-1 ring-primary' : ''}`}
          >
            <div className="mb-1 flex items-center gap-2 font-semibold">
              <span className="size-3 rounded-sm" style={{ background: screenColor(i) }} />
              {s.name}
              {s.local && <Badge variant="secondary">server</Badge>}
              {s.trusted_fingerprint && (
                <Badge variant="outline" title={s.trusted_fingerprint} style={{ color: 'var(--ok)' }}>
                  terpercaya
                </Badge>
              )}
              <span
                className="ml-auto size-2.5 rounded-full"
                title={s.connected ? 'terhubung' : 'tidak terhubung'}
                style={{ background: s.connected ? 'var(--ok)' : 'var(--muted-foreground)' }}
              />
            </div>
            <p className="mb-1 text-xs text-muted-foreground">
              {s.connected ? 'Terhubung' : 'Tidak terhubung'} · {s.monitors.length} monitor: {sizes}
              {s.placeholder && ' (perkiraan, belum pernah terhubung)'}
            </p>
            {near.length > 0 ? (
              <p className="mb-1 text-xs text-muted-foreground">Bersebelahan: {near.join(', ')}</p>
            ) : (
              <p className="mb-1 text-xs" style={{ color: 'var(--warn)' }}>
                Tidak menyentuh layar lain: kursor tidak bisa pindah ke sini.
              </p>
            )}
            {s.trusted_fingerprint && <RevokeButton onConfirm={() => onRevoke(s.name)} />}
            <div className="mt-2 grid grid-cols-2 gap-2" onClick={(e) => e.stopPropagation()}>
              {(['x', 'y'] as const).map((axis) => (
                <div key={axis}>
                  <Label className="text-xs text-muted-foreground">{axis.toUpperCase()}</Label>
                  <Input
                    type="number"
                    step={10}
                    aria-label={`${s.name} ${axis.toUpperCase()}`}
                    value={draft[s.name]?.[axis] ?? 0}
                    onFocus={() => onEditing(true)}
                    onBlur={() => onEditing(false)}
                    onChange={(e) => {
                      const v = parseInt(e.target.value, 10)
                      if (Number.isFinite(v)) onNumber(s.name, axis, v)
                    }}
                  />
                </div>
              ))}
            </div>
          </div>
        )
      })}
    </div>
  )
}

/** Two clicks to revoke: the first arms it for a few seconds. */
export function RevokeButton({ onConfirm }: { onConfirm: () => void }) {
  const [armed, setArmed] = useState(false)
  const timer = useRef<number | undefined>(undefined)
  return (
    <Button
      variant="link"
      size="sm"
      className="h-auto p-0 text-xs text-muted-foreground"
      onClick={(e) => {
        e.stopPropagation()
        if (!armed) {
          setArmed(true)
          timer.current = window.setTimeout(() => setArmed(false), 4000)
          return
        }
        window.clearTimeout(timer.current)
        setArmed(false)
        onConfirm()
      }}
    >
      {armed ? 'Yakin? Klik lagi untuk mencabut' : 'Cabut kepercayaan'}
    </Button>
  )
}
