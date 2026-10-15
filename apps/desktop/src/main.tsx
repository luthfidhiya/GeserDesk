import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import TransferPopup from './TransferPopup'
import AdvancedWindow from './windows/AdvancedWindow'
import { ipc } from './lib/ipc'
import { openClients } from './lib/panels'
import Frame, { type Menu } from './windows/Frame'
import LogWindow from './windows/LogWindow'
import TerminalWindow from './windows/TerminalWindow'
import './index.css'

// Until the saved theme loads, follow the system so the first paint is not the wrong colour
// (useTheme takes over from there).
document.documentElement.classList.toggle('dark', window.matchMedia('(prefers-color-scheme: dark)').matches)

// The same bundle serves the small transfer-progress window `transfer_window.rs`
// opens and the tool windows `windows.rs` opens (no router: one query parameter
// each is simpler than a routing library for a few extra pages).
const params = new URLSearchParams(window.location.search)
const tools = {
  settings: { Page: AdvancedWindow, title: 'Pengaturan lanjutan' },
  terminal: { Page: TerminalWindow, title: 'Terminal' },
  log: { Page: LogWindow, title: 'Log' },
} as const
const mainMenus: Menu[] = [
  {
    label: 'Alat',
    items: [
      { label: 'Pengaturan lanjutan', run: () => void ipc.openTool('settings') },
      { label: 'Daftar client', run: openClients },
      { label: 'Terminal', run: () => void ipc.openTool('terminal') },
      { label: 'Log', run: () => void ipc.openTool('log') },
    ],
  },
  { label: 'Bantuan', items: [{ label: 'Tentang GeserDesk', run: () => void ipc.showAbout() }] },
]
const tool = params.get('window') as keyof typeof tools | null
const page = tool && tools[tool]
const Root = page
  ? () => (
      <Frame title={page.title}>
        <page.Page />
      </Frame>
    )
  : params.get('popup') === 'transfer'
    ? TransferPopup
    : () => (
        <Frame menus={mainMenus}>
          <App />
        </Frame>
      )

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Root />
  </StrictMode>,
)
