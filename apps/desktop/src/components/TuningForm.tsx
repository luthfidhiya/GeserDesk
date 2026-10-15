import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { errorText, ipc } from '@/lib/ipc'
import type { Tuning } from '@/types'

/** What the server uses when the file says nothing (kept in step with Tuning::default in Rust). */
export const DEFAULT_TUNING: Tuning = {
  port: 24810,
  switch_delay_ms: 0,
  switch_double_tap_ms: 0,
  switch_corner_size: 0,
  jump_zone_size: 1,
  scroll_lock_locks: true,
  clipboard_sharing: true,
  clipboard_files: true,
}

type Msg = { text: string; kind: 'ok' | 'err' } | null

interface Props {
  /** The server config being edited. */
  path: string
  /** A session is running: the settings are read at Start, so they cannot change now. */
  disabled: boolean
}

const NUMBERS: { key: 'switch_delay_ms' | 'switch_double_tap_ms' | 'switch_corner_size' | 'jump_zone_size'; label: string; hint: string; min: number }[] = [
  { key: 'switch_delay_ms', label: 'Jeda pindah layar (ms)', hint: 'Kursor harus menempel di tepi selama ini. 0 = langsung.', min: 0 },
  { key: 'switch_double_tap_ms', label: 'Sentuh tepi dua kali (ms)', hint: 'Harus menyentuh tepi dua kali dalam waktu ini. 0 = mati.', min: 0 },
  { key: 'switch_corner_size', label: 'Hanya di sudut (px)', hint: 'Pindah hanya dekat sudut layar. 0 = di seluruh tepi.', min: 0 },
  { key: 'jump_zone_size', label: 'Lebar zona tepi (px)', hint: 'Seberapa dekat ke tepi yang dihitung menyentuh. Minimal 1.', min: 1 },
]

/** The port, how the cursor switches screens, and what is shared: edited here, read at the next Start. */
export function TuningForm({ path, disabled }: Props) {
  const [saved, setSaved] = useState<Tuning | null>(null)
  const [edit, setEdit] = useState<Tuning | null>(null)
  const [msg, setMsg] = useState<Msg>(null)

  useEffect(() => {
    let current = true
    setMsg(null)
    ipc
      .readTuning(path.trim())
      .then((t) => {
        if (!current) return
        setSaved(t)
        setEdit(t)
      })
      .catch((e) => current && setMsg({ text: errorText(e), kind: 'err' }))
    return () => {
      current = false
    }
  }, [path, disabled])

  if (!edit || !saved) return msg && <p className="text-sm" style={{ color: 'var(--bad)' }}>{msg.text}</p>

  const dirty = JSON.stringify(edit) !== JSON.stringify(saved)
  const change = (patch: Partial<Tuning>) => {
    setEdit({ ...edit, ...patch })
    setMsg(null)
  }
  const num = (v: string, fallback: number) => {
    const n = parseInt(v, 10)
    return Number.isFinite(n) ? n : fallback
  }

  const save = async () => {
    try {
      await ipc.writeTuning(path.trim(), edit)
      setSaved(edit)
      setMsg({ text: 'Tersimpan. Berlaku saat server dijalankan berikutnya.', kind: 'ok' })
    } catch (e) {
      setMsg({ text: errorText(e), kind: 'err' })
    }
  }

  return (
    <div className="space-y-3">
      <div className="text-sm">Port dan pindah layar</div>
      <div className="grid grid-cols-[14rem_8rem] items-center gap-x-3 gap-y-2">
        <Label htmlFor="tune-port">Port server</Label>
        <Input
          id="tune-port" type="number" min={1} max={65535} disabled={disabled} value={edit.port}
          onChange={(e) => change({ port: num(e.target.value, edit.port) })}
        />
        {NUMBERS.map(({ key, label, hint, min }) => (
          <div key={key} className="contents">
            <Label htmlFor={`tune-${key}`} title={hint}>{label}</Label>
            <Input
              id={`tune-${key}`} type="number" min={min} step={10} disabled={disabled} title={hint} value={edit[key]}
              onChange={(e) => change({ [key]: Math.max(min, num(e.target.value, edit[key])) })}
            />
          </div>
        ))}
      </div>
      {(
        [
          ['scroll_lock_locks', 'Scroll Lock mengunci kursor di layar ini'],
          ['clipboard_sharing', 'Bagikan clipboard (teks dan gambar)'],
          ['clipboard_files', 'Bagikan file yang disalin'],
        ] as const
      ).map(([key, label]) => (
        <div key={key} className="flex items-center gap-2">
          <Checkbox id={`tune-${key}`} disabled={disabled} checked={edit[key]} onCheckedChange={(v) => change({ [key]: v === true })} />
          <Label htmlFor={`tune-${key}`}>{label}</Label>
        </div>
      ))}
      <div className="flex flex-wrap items-center gap-2">
        <Button disabled={disabled || !dirty} onClick={() => void save()}>
          Simpan pengaturan
        </Button>
        <Button variant="outline" disabled={disabled || JSON.stringify(edit) === JSON.stringify(DEFAULT_TUNING)} onClick={() => change(DEFAULT_TUNING)}>
          Kembalikan bawaan
        </Button>
        {disabled && <span className="text-xs text-muted-foreground">Hentikan server dulu untuk mengubah.</span>}
        {msg && (
          <span className="text-sm" style={{ color: msg.kind === 'ok' ? 'var(--ok)' : 'var(--bad)' }}>
            {msg.text}
          </span>
        )}
      </div>
    </div>
  )
}
