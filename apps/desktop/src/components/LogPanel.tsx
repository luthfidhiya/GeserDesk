import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Label } from '@/components/ui/label'
import { useLog } from '@/hooks/useLog'
import type { LogLine } from '@/types'

const LEVEL_COLOR: Record<LogLine['level'], string> = {
  ERROR: 'var(--bad)',
  WARN: 'var(--warn)',
  INFO: 'var(--foreground)',
  DEBUG: 'var(--muted-foreground)',
  TRACE: 'var(--muted-foreground)',
}

export function LogPanel() {
  const [debug, setDebug] = useState(false)
  const { lines, clear } = useLog(debug)
  const box = useRef<HTMLDivElement>(null)
  const stuck = useRef(true)

  // Follow the newest line unless the person scrolled up to read.
  useEffect(() => {
    const el = box.current
    if (el && stuck.current) el.scrollTop = el.scrollHeight
  }, [lines])

  const copy = () =>
    void writeText(lines.map((l) => `${l.time} ${l.level.padEnd(5)} ${l.text}`).join('\n'))

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <div className="flex items-center gap-3">
        <span className="text-sm font-semibold">Log</span>
        <div className="flex items-center gap-2">
          <Checkbox id="debug" checked={debug} onCheckedChange={(c) => setDebug(c === true)} />
          <Label htmlFor="debug">Rinci (debug)</Label>
        </div>
        <Button variant="outline" size="sm" onClick={copy}>
          Salin
        </Button>
        <Button variant="outline" size="sm" onClick={() => void clear()}>
          Bersihkan
        </Button>
      </div>
      <div
        ref={box}
        onScroll={(e) => {
          const el = e.currentTarget
          stuck.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24
        }}
        className="min-h-0 flex-1 overflow-y-auto rounded-md border bg-muted/40 p-2 font-mono text-xs"
      >
        {lines.map((l, i) => (
          <div key={i} className="flex gap-2 break-all">
            <span className="shrink-0 text-muted-foreground">{l.time}</span>
            <span style={{ color: LEVEL_COLOR[l.level] }}>{l.text}</span>
          </div>
        ))}
      </div>
    </div>
  )
}
