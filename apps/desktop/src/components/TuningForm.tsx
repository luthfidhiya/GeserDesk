import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { errorText, ipc } from '@/lib/ipc'
import type { Tuning } from '@/types'
import { useT } from '@/lib/i18n'

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

type Two = [string, string]
const NUMBERS: { key: 'switch_delay_ms' | 'switch_double_tap_ms' | 'switch_corner_size' | 'jump_zone_size'; label: Two; hint: Two; min: number }[] = [
  { key: 'switch_delay_ms', label: ['Jeda pindah layar (ms)', 'Switch delay (ms)'], hint: ['Kursor harus menempel di tepi selama ini. 0 = langsung.', 'The cursor has to stay at the edge this long. 0 = at once.'], min: 0 },
  { key: 'switch_double_tap_ms', label: ['Sentuh tepi dua kali (ms)', 'Double tap the edge (ms)'], hint: ['Harus menyentuh tepi dua kali dalam waktu ini. 0 = mati.', 'The edge has to be touched twice within this time. 0 = off.'], min: 0 },
  { key: 'switch_corner_size', label: ['Hanya di sudut (px)', 'Corners only (px)'], hint: ['Pindah hanya dekat sudut layar. 0 = di seluruh tepi.', 'Switch only near a corner. 0 = along the whole edge.'], min: 0 },
  { key: 'jump_zone_size', label: ['Lebar zona tepi (px)', 'Edge zone width (px)'], hint: ['Seberapa dekat ke tepi yang dihitung menyentuh. Minimal 1.', 'How close to the edge counts as touching it. At least 1.'], min: 1 },
]

/** The port, how the cursor switches screens, and what is shared: edited here, read at the next Start. */
export function TuningForm({ path, disabled }: Props) {
  const t = useT()
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
      setMsg({ text: t('Tersimpan. Berlaku saat server dijalankan berikutnya.', 'Saved. Takes effect the next time the server starts.'), kind: 'ok' })
    } catch (e) {
      setMsg({ text: errorText(e), kind: 'err' })
    }
  }

  return (
    <div className="space-y-3">
      <div className="text-sm">{t('Port dan pindah layar', 'Port and switching')}</div>
      <div className="grid grid-cols-[14rem_8rem] items-center gap-x-3 gap-y-2">
        <Label htmlFor="tune-port">{t('Port server', 'Server port')}</Label>
        <Input
          id="tune-port" type="number" min={1} max={65535} disabled={disabled} value={edit.port}
          onChange={(e) => change({ port: num(e.target.value, edit.port) })}
        />
        {NUMBERS.map((n) => ({ ...n, label: t(...n.label), hint: t(...n.hint) })).map(({ key, label, hint, min }) => (
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
          ['scroll_lock_locks', t('Scroll Lock mengunci kursor di layar ini', 'Scroll Lock keeps the cursor on this screen')],
          ['clipboard_sharing', t('Bagikan clipboard (teks dan gambar)', 'Share the clipboard (text and pictures)')],
          ['clipboard_files', t('Bagikan file yang disalin', 'Share copied files')],
        ] as const
      ).map(([key, label]) => (
        <div key={key} className="flex items-center gap-2">
          <Checkbox id={`tune-${key}`} disabled={disabled} checked={edit[key]} onCheckedChange={(v) => change({ [key]: v === true })} />
          <Label htmlFor={`tune-${key}`}>{label}</Label>
        </div>
      ))}
      <div className="flex flex-wrap items-center gap-2">
        <Button disabled={disabled || !dirty} onClick={() => void save()}>
          {t('Simpan pengaturan', 'Save settings')}
        </Button>
        <Button variant="outline" disabled={disabled || JSON.stringify(edit) === JSON.stringify(DEFAULT_TUNING)} onClick={() => change(DEFAULT_TUNING)}>
          {t('Kembalikan bawaan', 'Restore defaults')}
        </Button>
        {disabled && <span className="text-xs text-muted-foreground">{t('Hentikan server dulu untuk mengubah.', 'Stop the server first to change these.')}</span>}
        {msg && (
          <span className="text-sm" style={{ color: msg.kind === 'ok' ? 'var(--ok)' : 'var(--bad)' }}>
            {msg.text}
          </span>
        )}
      </div>
    </div>
  )
}
