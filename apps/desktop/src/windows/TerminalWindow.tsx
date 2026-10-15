import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useSettings } from '@/hooks/useSettings'
import { useTheme } from '@/hooks/useTheme'
import { EVENTS, errorText, ipc, on } from '@/lib/ipc'
import { useLang, useT } from '@/lib/i18n'

type Kind = 'in' | 'out' | 'err' | 'sys'
interface Line {
  kind: Kind
  text: string
}
interface Out {
  id: number
  stream: 'out' | 'err'
  line: string
}
interface Exit {
  id: number
  code: number | null
}

const MAX_LINES = 5000
const COLOR: Record<Kind, string> = {
  in: 'var(--foreground)',
  out: 'var(--foreground)',
  err: 'var(--warn)',
  sys: 'var(--muted-foreground)',
}
// The CLI is told not to colour, but a line that has some anyway should not show escape codes.
// eslint-disable-next-line no-control-regex
const ANSI = /\u001b\[[0-9;]*[A-Za-z]/g

const HELP: Record<'id' | 'en', string[]> = {
  id: [
    'Ketik perintah CLI tanpa kata "geserdesk". Contoh:',
    '  status          apa yang dikerjakan daemon yang berjalan',
    '  stop            hentikan daemon dan sesinya',
    '  --help          semua perintah;  server --help / client --help  untuk opsinya',
    'server dan client memulai daemon sendiri dan baru selesai setelah dihentikan (tombol Hentikan,',
    'atau tutup jendela ini). Kalau sesi sudah berjalan dari aplikasi, keduanya akan menolak.',
    'help: tampilkan ini lagi.  clear: bersihkan layar.  Panah atas/bawah: riwayat.',
  ],
  en: [
    'Type a CLI command without the word "geserdesk". For example:',
    '  status          what the running daemon is doing',
    '  stop            stop the daemon and its session',
    '  --help          every command;  server --help / client --help  for their options',
    'server and client start a daemon of their own and only finish once stopped (the Stop button,',
    'or close this window). If a session already runs from the app, both refuse.',
    'help: show this again.  clear: clear the screen.  Up/down arrows: history.',
  ],
}
const help = (lang: 'id' | 'en'): Line[] => HELP[lang].map((text) => ({ kind: 'sys', text }))

/** Try `geserdesk` command-line commands here; output streams in as it is printed. */
export default function TerminalWindow() {
  const { settings } = useSettings()
  useTheme(settings?.theme)
  const t = useT()
  const lang = useLang()
  // The help shown on opening follows the language, which arrives a moment after the window.
  const [intro, setIntro] = useState(true)
  const [lines, setLines] = useState<Line[]>([])
  const [input, setInput] = useState('')
  const [running, setRunning] = useState(false)
  const history = useRef<string[]>([])
  const cursor = useRef(0)
  const box = useRef<HTMLDivElement>(null)
  const stuck = useRef(true)
  const field = useRef<HTMLInputElement>(null)

  const add = (more: Line[]) => setLines((prev) => [...prev, ...more].slice(-MAX_LINES))

  useEffect(() => {
    const offs = [
      on<Out>(EVENTS.terminalOut, (o) => add([{ kind: o.stream, text: o.line.replace(ANSI, '') }])),
      on<Exit>(EVENTS.terminalExit, (e) => {
        add([{ kind: 'sys', text: e.code === null ? t('[dihentikan]', '[stopped]') : t(`[selesai, kode ${e.code}]`, `[finished, code ${e.code}]`) }])
        setRunning(false)
      }),
    ]
    return () => offs.forEach((off) => off())
  }, [])

  // The field is disabled while a command runs; take the focus back when it ends.
  useEffect(() => {
    if (!running) field.current?.focus()
  }, [running])

  useEffect(() => {
    const el = box.current
    if (el && stuck.current) el.scrollTop = el.scrollHeight
  }, [lines])

  const run = async () => {
    const line = input.trim()
    if (!line || running) return
    setInput('')
    history.current.push(line)
    cursor.current = history.current.length
    const word = line.toLowerCase()
    if (word === 'clear' || word === 'cls') {
      setLines([])
      setIntro(false)
      return
    }
    if (word === 'help') {
      add([{ kind: 'in', text: `> ${line}` }, ...help(lang)])
      return
    }
    add([{ kind: 'in', text: `> geserdesk ${line}` }])
    setRunning(true)
    try {
      await ipc.terminalRun(line)
    } catch (e) {
      add([{ kind: 'err', text: errorText(e) }])
      setRunning(false)
    }
  }

  const recall = (step: number) => {
    const h = history.current
    if (h.length === 0) return
    cursor.current = Math.min(h.length, Math.max(0, cursor.current + step))
    setInput(h[cursor.current] ?? '')
  }

  return (
    <div className="flex h-full min-h-0 flex-col gap-2 p-3">
      <div
        ref={box}
        onScroll={(e) => {
          const el = e.currentTarget
          stuck.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24
        }}
        className="min-h-0 flex-1 overflow-y-auto rounded-md border bg-muted/40 p-2 font-mono text-xs"
      >
        {[...(intro ? help(lang) : []), ...lines].map((l, i) => (
          <div key={i} className="whitespace-pre-wrap break-all" style={{ color: COLOR[l.kind] }}>
            {l.text || ' '}
          </div>
        ))}
      </div>
      <div className="flex items-center gap-2">
        <span className="font-mono text-xs text-muted-foreground">geserdesk</span>
        <Input
          ref={field}
          autoFocus
          className="font-mono text-xs"
          aria-label={t('Perintah', 'Command')}
          placeholder={running ? t('berjalan…', 'running…') : 'status'}
          disabled={running}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              void run()
            } else if (e.key === 'ArrowUp') {
              e.preventDefault()
              recall(-1)
            } else if (e.key === 'ArrowDown') {
              e.preventDefault()
              recall(1)
            }
          }}
        />
        {running ? (
          <Button variant="destructive" onClick={() => void ipc.terminalKill()}>
            {t('Hentikan', 'Stop')}
          </Button>
        ) : (
          <Button onClick={() => void run()} disabled={!input.trim()}>
            {t('Jalankan', 'Run')}
          </Button>
        )}
        <Button variant="outline" onClick={() => { setLines([]); setIntro(false) }}>
          {t('Bersihkan', 'Clear')}
        </Button>
      </div>
    </div>
  )
}
