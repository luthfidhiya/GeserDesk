import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import TransferPopup from './TransferPopup'
import './index.css'

// Until the saved theme loads, follow the system so the first paint is not the wrong colour
// (useTheme takes over from there).
document.documentElement.classList.toggle('dark', window.matchMedia('(prefers-color-scheme: dark)').matches)

// The same bundle serves the small transfer-progress window `transfer_window.rs`
// opens (no router: one extra query param is simpler for one extra page).
const popup = new URLSearchParams(window.location.search).get('popup')
const Root = popup === 'transfer' ? TransferPopup : App

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Root />
  </StrictMode>,
)
