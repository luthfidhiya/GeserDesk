import { useCallback, useEffect, useState } from 'react'
import { EVENTS, errorText, ipc, on } from '@/lib/ipc'
import type { DaemonInfo, Settings, Status, TrustPrompt } from '@/types'

const RUNNING: Status['kind'][] = [
  'connecting',
  'connected',
  'awaiting_approval',
  'retrying',
  'server_running',
]

export const isRunning = (s: Status) => RUNNING.includes(s.kind)

/** Status of the running client or server, the trust question, and Start/Stop. */
export function useSession() {
  const [status, setStatus] = useState<Status>({ kind: 'stopped' })
  const [prompt, setPrompt] = useState<TrustPrompt | null>(null)
  const [problem, setProblem] = useState<string | null>(null)
  const [daemon, setDaemon] = useState<DaemonInfo | null>(null)

  useEffect(() => {
    void ipc.sessionStatus().then(setStatus)
    void ipc.currentPrompt().then(setPrompt)
    void ipc.daemonInfo().then(setDaemon)
    const offs = [
      on<Status>(EVENTS.status, setStatus),
      on<TrustPrompt | null>(EVENTS.prompt, setPrompt),
      on<DaemonInfo | null>(EVENTS.daemon, setDaemon),
    ]
    return () => offs.forEach((off) => off())
  }, [])

  const start = useCallback(async (settings: Settings) => {
    setProblem(null)
    try {
      await ipc.start(settings)
    } catch (e) {
      setProblem(errorText(e))
    }
  }, [])

  const stop = useCallback(async () => {
    setProblem(null)
    await ipc.stop()
  }, [])

  const shutdownDaemon = useCallback(async () => {
    setProblem(null)
    try {
      await ipc.shutdownDaemon()
    } catch (e) {
      setProblem(errorText(e))
    }
  }, [])

  const answer = useCallback(async (trust: boolean) => {
    setPrompt(null)
    await ipc.answerPrompt(trust)
  }, [])

  return { status, running: isRunning(status), daemon, prompt, problem, setProblem, start, stop, shutdownDaemon, answer }
}
