import { useEffect, useState } from 'react'
import appIcon from '@/assets/app-icon.png'
import { ThemeToggle } from '@/components/ThemeToggle'
import { ApprovalsPanel } from '@/components/ApprovalsPanel'
import { ClientsPanel } from '@/components/ClientsPanel'
import { TrustServerDialog } from '@/components/TrustServerDialog'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { TooltipProvider } from '@/components/ui/tooltip'
import { useDesk } from '@/hooks/useDesk'
import { useSession } from '@/hooks/useSession'
import { useSettings } from '@/hooks/useSettings'
import { useTheme } from '@/hooks/useTheme'
import { errorText, EVENTS, ipc, on } from '@/lib/ipc'
import { useT } from '@/lib/i18n'
import { onOpenClients } from '@/lib/panels'
import type { Mode, Tab } from '@/types'
import { ControlView } from '@/views/ControlView'
import { LayoutView } from '@/views/LayoutView'

export default function App() {
  const { settings, update } = useSettings()
  const session = useSession()
  const t = useT()
  const [tab, setTab] = useState<Tab>('control')
  const [busy, setBusy] = useState(false)
  // A running session decides the mode: the daemon may have been started from the CLI.
  const mode = session.running && session.daemon?.mode ? session.daemon.mode : settings?.mode
  const isServer = mode === 'server'
  const { desk, refresh } = useDesk(session.running && isServer, busy)
  useTheme(settings?.theme)

  // A computer asking to connect: the daemon says so, and the requests come up over this window.
  const [approvalsOpen, setApprovalsOpen] = useState(false)
  const [approvalsProblem, setApprovalsProblem] = useState<string | null>(null)
  useEffect(() => on<null>(EVENTS.approvals, () => void refresh().then(() => setApprovalsOpen(true))), [refresh])
  // The clients panel: from the Alat menu or the button in the monitor tab. A request that comes
  // in takes its place.
  const [clientsOpen, setClientsOpen] = useState(false)
  useEffect(() => onOpenClients(() => setClientsOpen(true)), [])
  const pending = desk?.pending ?? []
  const showApprovals = approvalsOpen && pending.length > 0
  const showClients = clientsOpen && !showApprovals
  const answer = async (action: () => Promise<void>) => {
    try {
      await action()
      setApprovalsProblem(null)
      await refresh()
    } catch (e) {
      setApprovalsProblem(`${t('Gagal', 'Failed')}: ${errorText(e)}`)
    }
  }

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
              <TabsTrigger value="control">{t('Konfigurasi', 'Configuration')}</TabsTrigger>
              <TabsTrigger value="layout">
                {t('Monitor', 'Monitors')}
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
      {showClients && (
        <ClientsPanel
          desk={desk}
          refresh={refresh}
          onTrust={(p) => void answer(() => ipc.approveClient(p.name, p.fingerprint))}
          onDismiss={(p) => void answer(() => ipc.dismissClient(p.name, p.fingerprint))}
          problem={approvalsProblem}
          onClose={() => setClientsOpen(false)}
        />
      )}
      {showApprovals && (
        <ApprovalsPanel
          pending={pending}
          problem={approvalsProblem}
          onTrust={(p) => void answer(() => ipc.approveClient(p.name, p.fingerprint))}
          onDismiss={(p) => void answer(() => ipc.dismissClient(p.name, p.fingerprint))}
          onClose={() => setApprovalsOpen(false)}
        />
      )}
      <TrustServerDialog prompt={session.prompt} onAnswer={(t) => void session.answer(t)} />
    </TooltipProvider>
  )
}
