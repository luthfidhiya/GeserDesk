import { ShieldOff } from 'lucide-react'
import { useRef, useState } from 'react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { isStretched, neighbours, union } from '@/lib/geometry'
import { ScreenSwitch } from '@/components/ScreenSwitch'
import { screenColor } from '@/components/ScreenStage'
import type { Draft, Screen } from '@/types'
import { useT } from '@/lib/i18n'

interface Props {
  screens: Screen[]
  draft: Draft
  selected: string | null
  onSelect: (name: string) => void
  onNumber: (name: string, axis: 'x' | 'y', value: number) => void
  onSize: (name: string, axis: 'w' | 'h', value: number) => void
  onResetSize: (name: string) => void
  onRevoke: (name: string) => void
  onDisabled: (name: string, disabled: boolean) => void
  onEditing: (editing: boolean) => void
}

export function ScreenList({ screens, draft, selected, onSelect, onNumber, onSize, onResetSize, onRevoke, onDisabled, onEditing }: Props) {
  const t = useT()
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
                  {t('terpercaya', 'trusted')}
                </Badge>
              )}
              {s.local ? (
                <span className="ml-auto size-2.5 rounded-full" title={t('terhubung', 'connected')} style={{ background: 'var(--ok)' }} />
              ) : (
                <span className="ml-auto">
                  <ScreenSwitch name={s.name} connected={s.connected} disabled={s.disabled} onToggle={(d) => onDisabled(s.name, d)} />
                </span>
              )}
              {s.trusted_fingerprint && <RevokeButton onConfirm={() => onRevoke(s.name)} />}
            </div>
            <p className="mb-1 text-xs text-muted-foreground">
              {s.disabled ? t('Nonaktif', 'Off') : s.connected ? t('Terhubung', 'Connected') : t('Tidak terhubung', 'Not connected')}
              {s.disabled && s.connected && t(' (tetap tersambung)', ' (still connected)')} · {s.monitors.length}{' '}
              {t('monitor', 'monitor(s)')}: {sizes}
              {s.placeholder && t(' (perkiraan, belum pernah terhubung)', ' (estimated, never connected)')}
            </p>
            {s.disabled ? null : near.length > 0 ? (
              <p className="mb-1 text-xs text-muted-foreground">{t('Bersebelahan', 'Next to')}: {near.join(', ')}</p>
            ) : (
              <p className="mb-1 text-xs" style={{ color: 'var(--warn)' }}>
                {t('Tidak bersentuhan dengan layar lain: kursor tak bisa pindah ke sini.', 'Touches no other screen: the cursor cannot move here.')}
              </p>
            )}
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
            <div className="mt-2 grid grid-cols-2 gap-2" onClick={(e) => e.stopPropagation()}>
              {(['w', 'h'] as const).map((axis) => (
                <div key={axis}>
                  <Label className="text-xs text-muted-foreground">{axis === 'w' ? t('Lebar', 'Width') : t('Tinggi', 'Height')}</Label>
                  <Input
                    type="number"
                    step={10}
                    aria-label={`${s.name} ${axis === 'w' ? t('lebar', 'width') : t('tinggi', 'height')}`}
                    value={draft[s.name]?.[axis] ?? union(s.monitors)[axis]}
                    onFocus={() => onEditing(true)}
                    onBlur={() => onEditing(false)}
                    onChange={(e) => {
                      const v = parseInt(e.target.value, 10)
                      if (Number.isFinite(v)) onSize(s.name, axis, v)
                    }}
                  />
                </div>
              ))}
            </div>
            {isStretched(s, draft[s.name]) && (
              <div className="mt-1 flex items-center justify-between gap-2" onClick={(e) => e.stopPropagation()}>
                <span className="text-xs text-muted-foreground">
                  {t('Diregangkan dari', 'Stretched from')} {union(s.monitors).w}×{union(s.monitors).h}
                </span>
                <Button variant="outline" size="sm" onClick={() => onResetSize(s.name)}>
                  Reset
                </Button>
              </div>
            )}
          </div>
        )
      })}
    </div>
  )
}

/** An icon that revokes trust in two clicks: the first arms it (red, "Yakin?") for a few seconds. */
export function RevokeButton({ onConfirm }: { onConfirm: () => void }) {
  const t = useT()
  const [armed, setArmed] = useState(false)
  const timer = useRef<number | undefined>(undefined)
  return (
    <Button
      variant={armed ? 'destructive' : 'ghost'}
      size={armed ? 'sm' : 'icon-sm'}
      title={armed ? t('Klik lagi untuk mencabut', 'Click again to revoke') : t('Cabut kepercayaan', 'Revoke trust')}
      aria-label={t('Cabut kepercayaan', 'Revoke trust')}
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
      <ShieldOff />
      {armed && t('Yakin?', 'Sure?')}
    </Button>
  )
}
