import { useCallback, useEffect, useMemo, useState } from 'react'
import { EVENTS, ipc, on } from '@/lib/ipc'
import type { LogLine } from '@/types'

const MAX_LINES = 3000

/**
 * Recent log lines (history first, then live), filtered by detail level.
 * `daemonPid` is the daemon being read: when a different one attaches the
 * lines are replaced by its own, and when it goes away they stay readable.
 */
export function useLog(showDebug: boolean, daemonPid: number | undefined) {
  const [lines, setLines] = useState<LogLine[]>([])

  useEffect(() => {
    if (daemonPid === undefined) return
    let live: LogLine[] = []
    let loaded = false
    const off = on<LogLine>(EVENTS.log, (line) => {
      if (loaded) setLines((prev) => [...prev.slice(-(MAX_LINES - 1)), line])
      else live.push(line) // arrived before the history did
    })
    void ipc.logSnapshot().then((history) => {
      loaded = true
      setLines([...history, ...live].slice(-MAX_LINES))
      live = []
    })
    return off
  }, [daemonPid])

  const shown = useMemo(
    () => (showDebug ? lines : lines.filter((l) => l.level === 'ERROR' || l.level === 'WARN' || l.level === 'INFO')),
    [lines, showDebug],
  )

  const clear = useCallback(async () => {
    await ipc.clearLog()
    setLines([])
  }, [])

  return { lines: shown, clear }
}
