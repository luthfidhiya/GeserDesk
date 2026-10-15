import { Select } from '@base-ui/react/select'
import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useSession } from '@/hooks/useSession'
import { useSettings } from '@/hooks/useSettings'
import { useTheme } from '@/hooks/useTheme'
import { errorText, ipc } from '@/lib/ipc'
import type { KeyAction, KeyEntries } from '@/types'

/** Indonesian labels for the actions; the list itself comes from Rust (`key_actions`). */
const LABELS: [string, string][] = [
  ['mute', 'Bisukan suara'],
  ['volume_up', 'Volume naik'],
  ['volume_down', 'Volume turun'],
  ['mic_mute', 'Bisukan mikrofon'],
  ['play_pause', 'Putar / jeda'],
  ['next_track', 'Lagu berikutnya'],
  ['previous_track', 'Lagu sebelumnya'],
  ['stop', 'Berhenti'],
  ['brightness_up', 'Kecerahan naik'],
  ['brightness_down', 'Kecerahan turun'],
  ['mail', 'Email'],
  ['calculator', 'Kalkulator'],
  ['explorer', 'Penjelajah berkas'],
  ['browser', 'Peramban'],
  ['browser_search', 'Peramban: cari'],
  ['browser_home', 'Peramban: beranda'],
  ['browser_back', 'Peramban: mundur'],
  ['browser_forward', 'Peramban: maju'],
  ['browser_stop', 'Peramban: berhenti'],
  ['browser_refresh', 'Peramban: muat ulang'],
  ['browser_favorites', 'Peramban: favorit'],
]
const LABEL: Record<string, string> = Object.fromEntries(LABELS)

/** Windows presses a virtual key and Linux an X11 keysym: a bare number means this machine's own. */
const ON_WINDOWS = /Windows/i.test(navigator.userAgent)
const OWN = ON_WINDOWS ? 'vk' : 'keysym'

/** An action with its own way on this machine; an action with no entry does the built-in. */
interface Way {
  action: string
  text: string
}

/** The file's way as the text the page shows. */
const showWay = (way: string): string => {
  const t = way.trim()
  const i = t.indexOf(':')
  if (i < 0) return t
  const kind = t.slice(0, i).trim().toLowerCase()
  const rest = t.slice(i + 1).trim()
  return kind === 'action' || kind === OWN ? rest : t
}

/** The text as the way `keys.toml` writes: `ignore`, an action name, a number, or a full `kind:value`. */
const wayText = (text: string): string => {
  const t = text.trim()
  if (t.toLowerCase() === 'ignore' || t.includes(':')) return t
  if (LABEL[t]) return `action:${t}`
  return `${OWN}:${t}`
}

const NAMED: Record<string, string> = {
  Enter: 'enter', Escape: 'esc', Backspace: 'backspace', Tab: 'tab', Space: 'space',
  PrintScreen: 'printscreen', ScrollLock: 'scrolllock', Pause: 'pause', Insert: 'insert',
  Home: 'home', PageUp: 'pageup', Delete: 'delete', End: 'end', PageDown: 'pagedown',
  ArrowRight: 'right', ArrowLeft: 'left', ArrowDown: 'down', ArrowUp: 'up',
}

/** The key of a browser event as `keys.toml` names it, or null for a modifier or a key it has no name for. */
function keyName(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code)
  if (letter) return letter[1].toLowerCase()
  const digit = /^Digit([0-9])$/.exec(code)
  if (digit) return digit[1]
  const f = /^F([0-9]{1,2})$/.exec(code)
  if (f && +f[1] >= 1 && +f[1] <= 24) return `f${f[1]}`
  return NAMED[code] ?? null
}

type Status = 'unset' | 'default' | 'custom' | 'ignored'
const STATUS: Record<Status, { text: string; color: string }> = {
  unset: { text: 'Belum diatur', color: 'var(--bad)' },
  default: { text: 'Bawaan', color: 'var(--muted-foreground)' },
  custom: { text: 'Custom', color: 'var(--foreground)' },
  ignored: { text: 'Diabaikan', color: 'var(--muted-foreground)' },
}

/** A slim pill that opens a small list of its own, in the app's colours. */
function StatusSelect({
  value, color, options, onChange,
}: {
  value: string
  color: string
  options: [string, string][]
  onChange: (value: string) => void
}) {
  return (
    <Select.Root value={value} onValueChange={(v) => v !== null && onChange(v as string)}>
      <Select.Trigger
        className="inline-flex h-6 w-32 shrink-0 cursor-pointer items-center justify-between gap-1 rounded-full bg-muted/60 px-3 text-xs font-medium outline-none transition-colors hover:bg-muted focus-visible:ring-1 focus-visible:ring-ring data-popup-open:bg-muted"
        style={{ color }}
      >
        <Select.Value>{(v: string) => options.find(([id]) => id === v)?.[1] ?? v}</Select.Value>
        <Select.Icon className="text-[9px] opacity-60">▾</Select.Icon>
      </Select.Trigger>
      <Select.Portal>
        <Select.Positioner sideOffset={4} alignItemWithTrigger={false} className="z-50">
          <Select.Popup className="min-w-32 rounded-lg bg-popover p-1 text-popover-foreground shadow-md ring-1 ring-foreground/10 outline-none">
            <Select.List>
              {options.map(([id, label]) => (
                <Select.Item
                  key={id} value={id}
                  className="flex cursor-pointer items-center justify-between gap-3 rounded-md px-2 py-1 text-xs outline-none data-highlighted:bg-muted"
                >
                  <Select.ItemText>{label}</Select.ItemText>
                  <Select.ItemIndicator className="text-[10px]">✓</Select.ItemIndicator>
                </Select.Item>
              ))}
            </Select.List>
          </Select.Popup>
        </Select.Positioner>
      </Select.Portal>
    </Select.Root>
  )
}

/** The special keys of this machine (`keys.toml`): every action, and whether it is set up. */
export default function KeysWindow() {
  const { settings } = useSettings()
  const session = useSession()
  useTheme(settings?.theme)
  const [rules, setRules] = useState<{ trigger: string; action: string }[] | null>(null)
  const [actions, setActions] = useState<KeyAction[]>([])
  /** What is wrong in the file as saved, until the next save. */
  const [problems, setProblems] = useState<string[]>([])
  const [ways, setWays] = useState<Way[]>([])
  const [saved, setSaved] = useState('')
  /** The action whose combination (rule) or key (way) is being listened for. */
  const [listenRule, setListenRule] = useState<string | null>(null)
  /** The combination as pressed so far while listening (confirmed with Enter). */
  const [preview, setPreview] = useState('')
  const [msg, setMsg] = useState<{ text: string; kind: 'ok' | 'err' } | null>(null)
  /** The action whose Custom editor is open, under its row. */
  const [editing, setEditing] = useState<string | null>(null)
  const [mode, setMode] = useState<'default' | 'ignore' | 'custom'>('default')
  /** Under Custom: a key combination on this keyboard, or the code of the key to press. */
  const [sub, setSub] = useState<'combo' | 'code'>('combo')

  const entries = (): KeyEntries => ({
    rules: rules ?? [],
    ways: ways
      .filter((w) => w.text.trim() !== '')
      .map((w) => ({ action: w.action, way: wayText(w.text) })),
  })

  useEffect(() => {
    ipc.keyActions().then(setActions).catch((e) => setMsg({ text: errorText(e), kind: 'err' }))
    ipc
      .readKeys()
      .then((e) => {
        setRules(e.rules)
        setProblems(e.problems ?? [])
        setWays(e.ways.map((x) => ({ action: x.action, text: showWay(x.way) })))
        setSaved(JSON.stringify({ rules: e.rules, ways: e.ways }))
      })
      .catch((e) => setMsg({ text: errorText(e), kind: 'err' }))
  }, [])

  // While listening, the keys pressed are shown as they are held and are not acted on:
  // the buttons confirm or cancel, so every key, Enter and Esc too, can be part of a combination.
  useEffect(() => {
    setPreview('')
    if (!listenRule) return
    let combo: string | null = null
    const shown = (ev: KeyboardEvent, key: string | null) =>
      [ev.ctrlKey && 'ctrl', ev.altKey && 'alt', ev.shiftKey && 'shift', ev.metaKey && 'win', key].filter(Boolean).join('+')
    const down = (ev: KeyboardEvent) => {
      ev.preventDefault()
      ev.stopPropagation()
      const key = keyName(ev.code)
      if (key) combo = shown(ev, key)
      setPreview(key ? combo! : shown(ev, null) + (shown(ev, null) ? '+' : ''))
    }
    const up = (ev: KeyboardEvent) => ev.preventDefault()
    window.addEventListener('keydown', down, true)
    window.addEventListener('keyup', up, true)
    return () => {
      window.removeEventListener('keydown', down, true)
      window.removeEventListener('keyup', up, true)
    }
  }, [listenRule])

  if (!rules) return msg && <p className="p-4 text-sm" style={{ color: 'var(--bad)' }}>{msg.text}</p>

  const current = entries()
  const dirty = JSON.stringify(current) !== saved
  const edited = () => setMsg(null)
  const statusOf = (id: string): Status => {
    if (ways.some((w) => w.action === id && w.text.trim().toLowerCase() === 'ignore')) return 'ignored'
    if (rules.some((r) => r.action === id) || ways.some((w) => w.action === id && w.text.trim() !== '')) return 'custom'
    return actions.find((a) => a.name === id)?.built_in === false ? 'unset' : 'default'
  }

  const save = async () => {
    try {
      await ipc.writeKeys(current)
      setSaved(JSON.stringify(current))
      setProblems([])
      setMsg({ text: 'Tersimpan. Berlaku saat Start berikutnya.', kind: 'ok' })
    } catch (e) {
      setMsg({ text: errorText(e), kind: 'err' })
    }
  }

  /** Open the Custom editor for an action, keeping what it already has. */
  const openCustom = (id: string) => {
    const t = ways.find((x) => x.action === id)?.text.trim() ?? ''
    const hasRule = rules.some((r) => r.action === id)
    const hasCode = t !== '' && t.toLowerCase() !== 'ignore'
    const nextSub = hasCode && !hasRule ? 'code' : 'combo'
    setSub(nextSub)
    setMode('custom')
    if (t.toLowerCase() === 'ignore') setWays(withoutWay(id))
    if (nextSub === 'code' && !hasCode) setWays([...withoutWay(id), { action: id, text: '' }])
    setListenRule(null)
    setEditing(id)
  }
  const pick = (id: string, value: string) => {
    if (value === 'custom') openCustom(id)
    else chooseMode(id, value as typeof mode)
  }
  const close = () => {
    setListenRule(null)
    setEditing(null)
  }
  const withoutWay = (id: string) => ways.filter((w) => w.action !== id)
  const chooseMode = (id: string, m: typeof mode) => {
    setMode(m)
    setListenRule(null)
    edited()
    if (m === 'default') {
      setWays(withoutWay(id))
      setRules(rules.filter((r) => r.action !== id))
    } else if (m === 'ignore') setWays([...withoutWay(id), { action: id, text: 'ignore' }])
    else setWays(sub === 'code' ? [...withoutWay(id), { action: id, text: '' }] : withoutWay(id))
  }
  const chooseSub = (id: string, c: typeof sub) => {
    setSub(c)
    setListenRule(null)
    edited()
    setWays(c === 'code' ? [...withoutWay(id), { action: id, text: '' }] : withoutWay(id))
  }
  const setCode = (id: string, text: string) => {
    setWays(ways.map((w) => (w.action === id ? { ...w, text } : w)))
    edited()
  }
  /** What is set for an action, in one line. */
  const summary = (id: string) => {
    const parts = rules.filter((r) => r.action === id).map((r) => r.trigger)
    const w = ways.find((x) => x.action === id)
    const t = w?.text.trim() ?? ''
    if (t.toLowerCase() === 'ignore') return parts.join(', ')
    if (LABEL[t]) parts.push(`jalankan "${LABEL[t]}"`)
    else if (t) parts.push(`tekan ${t}`)
    return parts.join(', ')
  }
  
  return (
    <div className="flex h-full flex-col gap-3 overflow-y-auto px-5 py-4">
      <p className="text-[11px] leading-relaxed text-muted-foreground">
        Yang berstatus Bawaan sudah jalan tanpa diisi. Pilih Custom untuk kombinasi atau kode tombol sendiri. Berlaku saat Start berikutnya.
      </p>
      {problems.length > 0 && (
        <div className="rounded-md px-3 py-2 text-xs" style={{ color: 'var(--bad)', background: 'color-mix(in oklch, var(--bad) 10%, transparent)' }}>
          <div className="font-medium">keys.toml punya baris yang dilewati saat Start:</div>
          <ul className="mt-1 list-disc pl-4 font-mono">
            {problems.map((p) => <li key={p}>{p}</li>)}
          </ul>
          <div className="mt-1">Perbaiki di bawah lalu Simpan.</div>
        </div>
      )}
      <div className="divide-y divide-border/40">
        {actions.map(({ name: id }) => {
          const label = LABEL[id] ?? id
          const status = statusOf(id)
          const value = status === 'custom' ? 'custom' : status === 'ignored' ? 'ignore' : 'default'
          return (
            <div key={id}>
              <div className="flex w-full items-center gap-3 px-1 py-1.5">
              <span className="w-44 shrink-0 text-[13px]">{label}</span>
              <StatusSelect
                value={value} color={STATUS[status].color} onChange={(v) => pick(id, v)}
                options={[
                  ['default', status === 'unset' ? 'Belum diatur' : 'Bawaan'],
                  ['custom', 'Custom'],
                  ['ignore', 'Diabaikan'],
                ]}
              />
              {status === 'custom' ? (
                <button className="min-w-0 flex-1 truncate text-left font-mono text-[11px] text-muted-foreground hover:text-foreground" onClick={() => openCustom(id)}>
                  {summary(id) || 'Atur…'}
                </button>
              ) : (
                <span className="flex-1" />
              )}
            </div>
              {editing === id && mode === 'custom' && (
                <div className="mx-1 mb-2 space-y-3 rounded-lg bg-muted/30 p-3">
                  <label className="flex items-center gap-2 text-sm">
                    <input type="radio" name="custom-kind" checked={sub === 'combo'} onChange={() => chooseSub(id, 'combo')} />
                    Kombinasi tombol (mis. ctrl+shift+r)
                  </label>
                  {sub === 'combo' && (
                    <div className="space-y-1 pl-6">
                      <p className="text-xs text-muted-foreground">
                        Berarti aksi ini saat komputer ini menjadi server.
                      </p>
                      <div className="flex flex-wrap items-center gap-1">
                        {rules.filter((r) => r.action === id).map((r) => (
                          <span key={r.trigger} className="inline-flex items-center gap-1 rounded bg-muted px-1.5 py-0.5 text-xs">
                            <code>{r.trigger}</code>
                            <button
                              className="text-muted-foreground hover:text-foreground" title="Hapus"
                              onClick={() => { setRules(rules.filter((x) => x.trigger !== r.trigger)); edited() }}
                            >
                              ×
                            </button>
                          </span>
                        ))}
                        {listenRule === id ? (
                          <>
                            <code className="rounded border border-primary px-2 py-0.5 text-xs">
                              {preview || 'tekan kombinasi…'}
                            </code>
                            <Button size="sm" disabled={!/[^+]$/.test(preview) || preview.endsWith('+')} onClick={() => {
                              const trigger = preview
                              setRules([...rules.filter((x) => x.trigger !== trigger), { trigger, action: id }])
                              edited()
                              setListenRule(null)
                            }}>
                              Simpan
                            </Button>
                            <Button variant="outline" size="sm" onClick={() => setListenRule(null)}>Batal</Button>
                          </>
                        ) : (
                          <Button variant="outline" size="sm" onClick={() => setListenRule(id)}>
                            Tekan kombinasi
                          </Button>
                        )}
                      </div>
                    </div>
                  )}
                  <label className="flex items-center gap-2 text-sm">
                    <input type="radio" name="custom-kind" checked={sub === 'code'} onChange={() => chooseSub(id, 'code')} />
                    Kode tombol ({ON_WINDOWS ? 'virtual key' : 'keysym'})
                  </label>
                  {sub === 'code' && (
                    <div className="space-y-1 pl-6">
                      <p className="text-xs text-muted-foreground">
                        Tombol yang ditekan komputer ini saat aksi diterima (saat menjadi client).
                      </p>
                      <Input
                        className="h-8" value={ways.find((w) => w.action === id)?.text ?? ''} placeholder={ON_WINDOWS ? '0xB7' : '0x1008ff12'}
                        onChange={(e) => setCode(id, e.target.value)}
                      />
                    </div>
                  )}
                  <div className="flex justify-end">
                    <Button size="sm" onClick={close}>Selesai</Button>
                  </div>
                </div>
              )}
            </div>
          )
        })}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Button disabled={!dirty} onClick={() => void save()}>Simpan</Button>
        {session.running && (
          <span className="text-xs text-muted-foreground">Sesi sedang berjalan; perubahan berlaku setelah Stop lalu Start.</span>
        )}
        {msg && (
          <span className="text-sm" style={{ color: msg.kind === 'ok' ? 'var(--ok)' : 'var(--bad)' }}>
            {msg.text}
          </span>
        )}
      </div>
    </div>
  )
}
