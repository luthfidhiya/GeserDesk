import { useCallback, useEffect, useRef, useState } from 'react'
import { ipc } from '@/lib/ipc'
import type { Settings } from '@/types'

/** The remembered settings, saved a moment after each edit. */
export function useSettings() {
  const [settings, setSettings] = useState<Settings | null>(null)
  const timer = useRef<number | undefined>(undefined)

  useEffect(() => {
    void ipc.loadSettings().then(setSettings)
  }, [])

  const update = useCallback((change: (s: Settings) => Settings) => {
    setSettings((prev) => {
      if (!prev) return prev
      const next = change(prev)
      window.clearTimeout(timer.current)
      timer.current = window.setTimeout(() => void ipc.saveSettings(next), 400)
      return next
    })
  }, [])

  return { settings, update }
}
