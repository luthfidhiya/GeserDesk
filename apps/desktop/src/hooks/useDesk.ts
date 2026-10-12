import { useCallback, useEffect, useRef, useState } from 'react'
import { ipc } from '@/lib/ipc'
import type { DeskSnapshot } from '@/types'

const POLL_MS = 2000

/**
 * The server's desk, refreshed every couple of seconds while `active`.
 * `paused` holds refreshes back (while dragging, or typing a number) so the
 * screen does not change under the person's hands.
 */
export function useDesk(active: boolean, paused: boolean) {
  const [desk, setDesk] = useState<DeskSnapshot | null>(null)
  const pausedRef = useRef(paused)
  pausedRef.current = paused

  const refresh = useCallback(async () => {
    try {
      setDesk(await ipc.deskState())
    } catch {
      // The server may be stopping; the next poll tells.
    }
  }, [])

  useEffect(() => {
    if (!active) {
      setDesk(null)
      return
    }
    void refresh()
    const id = window.setInterval(() => {
      if (!pausedRef.current) void refresh()
    }, POLL_MS)
    return () => window.clearInterval(id)
  }, [active, refresh])

  return { desk, refresh }
}
