import { useCallback, useEffect, useMemo, useState } from 'react'
import { Button } from '@/components/ui/button'
import { ScreenList } from '@/components/ScreenList'
import { ScreenStage } from '@/components/ScreenStage'
import { autoArrange, deskSize, MIN_DESK_SIZE, overlapping, union, unstretched } from '@/lib/geometry'
import { errorText, ipc } from '@/lib/ipc'
import type { DeskSnapshot, Draft, Placement, Point, Size } from '@/types'

type Msg = { text: string; kind: 'ok' | 'err' | 'info' } | null

const MSG_COLOR = { ok: 'var(--ok)', err: 'var(--bad)', info: 'var(--muted-foreground)' }

interface Props {
  desk: DeskSnapshot
  refresh: () => Promise<void>
  /** Hold back automatic refreshes while the person is dragging or typing. */
  onBusy: (busy: boolean) => void
}

/** Arrange the machines on the desk. Who may connect is in the clients window. */
export function LayoutEditor({ desk, refresh, onBusy }: Props) {
  const [draft, setDraft] = useState<Draft>({})
  const [dirty, setDirty] = useState(false)
  const [selected, setSelected] = useState<string | null>(null)
  const [msg, setMsg] = useState<Msg>(null)
  const [dragging, setDragging] = useState(false)
  const [typing, setTyping] = useState(false)

  // Only machines that are running now: no stand-ins for ones that never connected.
  const screens = useMemo(() => desk.screens.filter((s) => s.connected), [desk])

  useEffect(() => onBusy(dragging || typing), [dragging, typing, onBusy])

  // Follow the server's positions, but never overwrite what is being edited.
  useEffect(() => {
    setDraft((prev) => {
      const next: Draft = {}
      for (const s of screens)
        next[s.name] = dirty && prev[s.name] ? prev[s.name] : { ...s.origin, w: s.size.w, h: s.size.h }
      return next
    })
    setSelected((cur) => (cur && screens.some((s) => s.name === cur) ? cur : desk.local))
  }, [desk, screens, dirty])

  const complete = screens.every((s) => draft[s.name])
  const bad = useMemo(() => (complete ? overlapping(screens, draft) : new Set<string>()), [screens, draft, complete])

  const move = useCallback((name: string, to: { x: number; y: number }) => {
    setDraft((d) => ({ ...d, [name]: { ...d[name], x: Math.round(to.x), y: Math.round(to.y) } }))
    setDirty(true)
  }, [])

  const resize = useCallback((name: string, to: Placement) => {
    setDraft((d) => ({ ...d, [name]: to }))
    setDirty(true)
  }, [])

  const setSize = (name: string, axis: 'w' | 'h', v: number) => {
    const s = screens.find((x) => x.name === name)
    const n = s && union(s.monitors)
    if (!s || !n || v < MIN_DESK_SIZE) return
    setDraft((d) => ({ ...d, [name]: { ...d[name], w: d[name].w ?? n.w, h: d[name].h ?? n.h, [axis]: v } }))
    setDirty(true)
  }

  const resetSize = (name: string) => {
    setDraft((d) => ({ ...d, [name]: unstretched(d[name]) }))
    setDirty(true)
    setMsg({ text: `Ukuran "${name}" dikembalikan. Klik Simpan untuk menerapkan.`, kind: 'info' })
  }

  // Arrow keys nudge the selected machine; Shift = 1 px.
  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if (!selected || (ev.target as HTMLElement)?.tagName === 'INPUT') return
      const d = ev.shiftKey ? 1 : 10
      const delta = { ArrowLeft: [-d, 0], ArrowRight: [d, 0], ArrowUp: [0, -d], ArrowDown: [0, d] }[ev.key]
      if (!delta) return
      ev.preventDefault()
      setDraft((cur) => (cur[selected] ? { ...cur, [selected]: { ...cur[selected], x: cur[selected].x + delta[0], y: cur[selected].y + delta[1] } } : cur))
      setDirty(true)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [selected])

  const run = async (action: () => Promise<void>, ok?: string) => {
    try {
      await action()
      await refresh()
      if (ok) setMsg({ text: ok, kind: 'ok' })
    } catch (e) {
      setMsg({ text: `Gagal: ${errorText(e)}`, kind: 'err' })
    }
  }

  const save = async () => {
    setMsg({ text: 'Menyimpan…', kind: 'info' })
    try {
      // Every machine's size is sent: a real size is `null`, which also undoes a stretch.
      const positions: Record<string, Point> = {}
      const sizes: Record<string, Size | null> = {}
      for (const s of screens) {
        positions[s.name] = { x: draft[s.name].x, y: draft[s.name].y }
        sizes[s.name] = deskSize(s, draft[s.name])
      }
      const saved = await ipc.saveLayout(positions, sizes)
      await refresh() // fetch the applied positions before the draft lets go
      setDirty(false)
      setMsg({
        text: saved
          ? 'Tersimpan ke config dan langsung berlaku.'
          : 'Berlaku sekarang (config tidak punya berkas, tidak disimpan).',
        kind: 'ok',
      })
    } catch (e) {
      setMsg({ text: errorText(e), kind: 'err' })
    }
  }

  const revert = async () => {
    setDirty(false)
    await refresh()
    setMsg({ text: 'Perubahan dibatalkan.', kind: 'info' })
  }

  const arrange = () => {
    setDraft(autoArrange(screens))
    setDirty(true)
    setMsg({ text: 'Disusun berjajar. Klik Simpan untuk menerapkan.', kind: 'info' })
  }

  const revoke = (name: string) =>
    run(
      () => ipc.revokeClient(name),
      `Kepercayaan untuk "${name}" dicabut. Sesi yang sedang berjalan tidak diputus.`,
    )

  if (!complete) return null

  return (
    <div className="flex h-full min-h-0 flex-col">
      {!desk.tls && (
        <div className="mb-3 rounded-lg border p-3 text-sm" style={{ borderColor: 'var(--bad)', color: 'var(--bad)' }}>
          TLS mati di server ini: semua ketikan dikirim tanpa enkripsi dan komputer mana pun bisa
          menyambung. Hanya untuk debugging.
        </div>
      )}
      <p className="mb-2 text-xs text-muted-foreground">
        {desk.config_path ? `Disimpan ke ${desk.config_path}` : 'Config tidak punya berkas: perubahan tidak disimpan'}
      </p>
      <div className="grid min-h-0 flex-1 grid-cols-[1fr_20rem] gap-4">
        <div className="flex min-h-0 flex-col rounded-lg border">
          <div className="min-h-0 flex-1">
            <ScreenStage
              screens={screens}
              draft={draft}
              selected={selected}
              bad={bad}
              onSelect={setSelected}
              onMove={move}
              onResize={resize}
              onDragging={setDragging}
            />
          </div>
          <p className="p-2 text-xs text-muted-foreground">
            Seret kotak untuk menggeser komputer; tarik kotak-kotak kecil di tepinya (pada komputer yang
            dipilih) untuk meregangkannya, misalnya setinggi dua monitor server. Tepi menempel otomatis ke
            layar lain. Tahan <kbd>Alt</kbd> untuk tanpa tempel; panah menggeser 10 px (<kbd>Shift</kbd> = 1 px).
          </p>
        </div>
        <div className="min-h-0 space-y-3 overflow-y-auto pr-1">
          <div className="flex items-center justify-between gap-2 rounded-lg border p-3">
            <span className="text-sm">Komputer yang terhubung dan dipercaya</span>
            <Button variant="outline" size="sm" onClick={() => void ipc.openTool('clients')}>
              Daftar client
            </Button>
          </div>
          <ScreenList
            screens={screens}
            draft={draft}
            selected={selected}
            onSelect={setSelected}
            onNumber={(name, axis, v) => move(name, { ...draft[name], [axis]: v })}
            onSize={setSize}
            onResetSize={resetSize}
            onRevoke={(n) => void revoke(n)}
            onEditing={setTyping}
          />
          {desk.tls && desk.server_fingerprint && (
            <div className="rounded-lg border p-3">
              <div className="font-semibold">Sidik jari server ini</div>
              <p className="mb-1 text-xs text-muted-foreground">
                Client menampilkan angka ini saat pertama menyambung. Pastikan sama.
              </p>
              <code className="break-all font-mono text-xs">{desk.server_fingerprint}</code>
            </div>
          )}
          <div className="rounded-lg border p-3">
            <div className="flex flex-wrap gap-2">
              <Button disabled={bad.size > 0 || !dirty} onClick={() => void save()}>
                Simpan &amp; terapkan
              </Button>
              <Button variant="outline" disabled={!dirty} onClick={() => void revert()}>
                Batalkan
              </Button>
              <Button variant="outline" onClick={arrange}>
                Susun otomatis
              </Button>
            </div>
            {bad.size > 0 ? (
              <p className="mt-2 text-sm" style={{ color: 'var(--bad)' }}>
                Ada layar yang saling menimpa (merah). Geser agar tidak bertumpuk.
              </p>
            ) : (
              msg && (
                <p className="mt-2 text-sm" style={{ color: MSG_COLOR[msg.kind] }}>
                  {msg.text}
                </p>
              )
            )}
          </div>
        </div>
      </div>
    </div>
  )
}
