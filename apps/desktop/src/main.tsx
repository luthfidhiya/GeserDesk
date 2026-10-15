import { StrictMode, type ComponentType } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import TransferPopup from './TransferPopup'
import './index.css'

// Until the saved theme loads, follow the system so the first paint is not the wrong colour
// (useTheme takes over from there).
document.documentElement.classList.toggle('dark', window.matchMedia('(prefers-color-scheme: dark)').matches)

// The same bundle serves the small transfer-progress window `transfer_window.rs`
// opens and the tool windows `windows.rs` opens (no router: one query parameter
// each is simpler than a routing library for a few extra pages).
const params = new URLSearchParams(window.location.search)
const tools: Record<string, { Page: ComponentType; title: string }> = {}
const tool = params.get('window')
const page = tool && tools[tool]
const Root = page
  ? page.Page
  : params.get('popup') === 'transfer'
    ? TransferPopup
    : App

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Root />
  </StrictMode>,
)
