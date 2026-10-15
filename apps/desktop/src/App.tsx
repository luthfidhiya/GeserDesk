import { useEffect, useRef, useState } from 'react'
import appIcon from '@/assets/app-icon.png'
import { ThemeToggle } from '@/components/ThemeToggle'
import { TrustServerDialog } from '@/components/TrustServerDialog'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { TooltipProvider } from '@/components/ui/tooltip'
import { useDesk } from '@/hooks/useDesk'
import { useSession } from '@/hooks/useSession'
import { useSettings } from '@/hooks/useSettings'
import { useTheme } from '@/hooks/useTheme'
import { EVENTS, ipc, on } from '@/lib/ipc'
import type { Mode, Tab } from '@/types'
import { ControlView } from '@/views/ControlView'
import { LayoutView } from '@/views/LayoutView'

export default function App() {
  const { settings, update } = useSettings()
  const session = useSession()
  const [tab, setTab] = useState<Tab>('control')
  const [busy, setBusy] = useState(false)
  // A running session decides the mode: the daemon may have been started from the CLI.
  const mode = session.running && session.daemon?.mode ? session.daemon.mode : settings?.mode
  const isServer = mode === 'server'
  const { desk, refresh } = useDesk(session.running && isServer, busy)
  useTheme(settings?.theme)

  // The tray's "Pengaturan layar" asks the window to show the monitor tab (server only).
  useEffect(() => on<string>(EVENTS.navigate, (to) => to === 'layout' && setTab('layout')), [])

  // A computer asking to connect for the first time gets a window of its own. One window
  // per new request: a request left undecided does not open it again, but a computer that
  // asks again after it was answered does.
  const asked = useRef(new Set<string>())
  useEffect(() => {
    if (!desk) return
    const now = new Set(desk.pending.map((p) => p.name + '/' + p.fingerprint))
    const fresh = [...now].some((key) => !asked.current.has(key))
    asked.current = now
    if (fresh) void ipc.openTool('approvals')
  }, [desk])

  if (!settings) return null

  // A client has no monitor settings, so it only ever shows its configuration.
  const shownTab: Tab = isServer ? tab : 'control'
  const shown = mode && mode !== settings.mode ? { ...settings, mode } : settings

  return (
    <TooltipProvider>
      <div className="flex h-full flex-col gap-3 p-4">
        <header className="flex flex-wrap items-center gap-4">
          <div className="flex items-center gap-2">
            <img src={appIcon} alt="" className="h-7 w-7 rounded-md" />
            <h1 className="text-lg font-semibold">GeserDesk</h1>
          </div>
          <Tabs
            value={mode}
            onValueChange={(m) => {
              session.setProblem(null)
              setTab('control')
              update((s) => ({ ...s, mode: m as Mode }))
            }}
          >
            <TabsList>
              <TabsTrigger value="client" disabled={session.running}>
                Client
              </TabsTrigger>
              <TabsTrigger value="server" disabled={session.running}>
                Server
              </TabsTrigger>
            </TabsList>
          </Tabs>
          <div className="ml-auto">
            <ThemeToggle theme={settings.theme} onChange={(theme) => update((s) => ({ ...s, theme }))} />
          </div>
        </header>

        {isServer && (
          <Tabs value={shownTab} onValueChange={(t) => setTab(t as Tab)}>
            <TabsList>
              <TabsTrigger value="control">Konfigurasi</TabsTrigger>
              <TabsTrigger value="layout">
                Pengaturan monitor
                {desk && desk.pending.length > 0 && (
                  <span className="ml-1.5 rounded-full px-1.5 text-xs text-white" style={{ background: 'var(--warn)' }}>
                    {desk.pending.length}
                  </span>
                )}
              </TabsTrigger>
            </TabsList>
          </Tabs>
        )}

        <main className="min-h-0 flex-1">
          {shownTab === 'control' ? (
            <ControlView
              settings={shown}
              daemon={session.daemon}
              onChange={update}
              status={session.status}
              running={session.running}
              problem={session.problem}
              setProblem={session.setProblem}
              onStart={() => void session.start(settings)}
              onStop={() => void session.stop()}
              onShutdownDaemon={() => void session.shutdownDaemon()}
            />
          ) : (
            <LayoutView desk={desk} running={session.running} refresh={refresh} onBusy={setBusy} />
          )}
        </main>
      </div>
      <TrustServerDialog prompt={session.prompt} onAnswer={(t) => void session.answer(t)} />
    </TooltipProvider>
  )
}
