import { StrictMode, useEffect, type ReactNode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App'
import TransferPopup from './TransferPopup'
import AdvancedWindow from './windows/AdvancedWindow'
import { useSettings } from './hooks/useSettings'
import { LangProvider, pick, type Lang } from './lib/i18n'
import { ipc } from './lib/ipc'
import { openClients } from './lib/panels'
import Frame, { type Menu } from './windows/Frame'
import KeysWindow from './windows/KeysWindow'
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
  settings: { Page: AdvancedWindow, title: ['Pengaturan lanjutan', 'Advanced settings'] },
  keys: { Page: KeysWindow, title: ['Tombol khusus', 'Special keys'] },
  terminal: { Page: TerminalWindow, title: ['Terminal', 'Terminal'] },
  log: { Page: LogWindow, title: ['Log', 'Log'] },
} as const

/** The menus of the main window's title bar, in the window's language. */
function mainMenus(lang: Lang, setLang: (l: Lang) => void): Menu[] {
  const t = (id: string, en: string) => pick(lang, id, en)
  const mark = (l: Lang) => (lang === l ? '✓ ' : '\u2003')
  return [
    {
      label: t('Alat', 'Tools'),
      items: [
        { label: t('Pengaturan lanjutan', 'Advanced settings'), run: () => void ipc.openTool('settings') },
        { label: t('Tombol khusus', 'Special keys'), run: () => void ipc.openTool('keys') },
        { label: t('Daftar client', 'Clients'), run: openClients },
        { label: 'Terminal', run: () => void ipc.openTool('terminal') },
        { label: 'Log', run: () => void ipc.openTool('log') },
      ],
    },
    {
      label: t('Bahasa', 'Language'),
      items: [
        { label: `${mark('id')}Bahasa Indonesia`, run: () => setLang('id') },
        { label: `${mark('en')}English`, run: () => setLang('en') },
      ],
    },
    { label: t('Bantuan', 'Help'), items: [{ label: t('Tentang GeserDesk', 'About GeserDesk'), run: () => void ipc.showAbout() }] },
  ]
}

/** Every page speaks the saved language, and follows it when another window changes it. */
function Localized({ children }: { children: (lang: Lang, setLang: (l: Lang) => void) => ReactNode }) {
  const { settings, update } = useSettings()
  const lang: Lang = settings?.language ?? 'id'
  useEffect(() => {
    document.documentElement.lang = lang
  }, [lang])
  return <LangProvider value={lang}>{children(lang, (l) => update((s) => ({ ...s, language: l })))}</LangProvider>
}

const tool = params.get('window') as keyof typeof tools | null
const page = tool && tools[tool]
const Root = page
  ? () => (
      <Localized>
        {(lang) => (
          <Frame title={pick(lang, page.title[0], page.title[1])}>
            <page.Page />
          </Frame>
        )}
      </Localized>
    )
  : params.get('popup') === 'transfer'
    ? () => <Localized>{() => <TransferPopup />}</Localized>
    : () => (
        <Localized>
          {(lang, setLang) => (
            <Frame menus={mainMenus(lang, setLang)}>
              <App />
            </Frame>
          )}
        </Localized>
      )

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Root />
  </StrictMode>,
)
