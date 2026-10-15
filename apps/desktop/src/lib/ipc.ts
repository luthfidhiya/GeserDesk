// The only file that talks to the Rust shell. Everything else uses these.

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type {
  DaemonInfo,
  DeskSnapshot,
  LogLine,
  Mode,
  Point,
  Settings,
  Size,
  Status,
  TransferProgress,
  Tuning,
  TrustedClient,
  TrustPrompt,
} from '@/types'

export const ipc = {
  loadSettings: () => invoke<Settings>('load_settings'),
  saveSettings: (settings: Settings) => invoke<void>('save_settings', { settings }),
  sessionStatus: () => invoke<Status>('session_status'),
  /** `null` while no daemon is running. */
  daemonInfo: () => invoke<DaemonInfo | null>('daemon_info'),
  /** Saves `settings`, then starts the client or server they select. */
  start: (settings: Settings) => invoke<void>('start_session', { settings }),
  /** Stop the session; the daemon that ran it leaves with it. */
  stop: () => invoke<void>('stop_session'),
  /** Stop the session and end the daemon, whoever started it. */
  shutdownDaemon: () => invoke<void>('shutdown_daemon'),
  ownFingerprint: (mode: Mode) => invoke<string | null>('own_fingerprint', { mode }),
  createDefaultConfig: (path: string) => invoke<void>('create_default_config', { path }),
  defaultConfigPath: () => invoke<string>('default_config_path'),
  readTuning: (path: string) => invoke<Tuning>('read_tuning', { path }),
  /** Takes effect at the next Start. */
  writeTuning: (path: string, tuning: Tuning) => invoke<void>('write_tuning', { path, tuning }),
  configExists: (path: string) => invoke<boolean>('config_exists', { path }),

  /** `null` while no server is running. */
  deskState: () => invoke<DeskSnapshot | null>('desk_state'),
  /** Resolves to whether the layout was written to the config file. */
  saveLayout: (positions: Record<string, Point>, sizes: Record<string, Size | null>) =>
    invoke<boolean>('save_layout', { positions, sizes }),
  /** The clients the server's config trusts, read from the file (no server needed). */
  trustedClients: () => invoke<TrustedClient[]>('trusted_clients'),
  approveClient: (name: string, fingerprint: string) =>
    invoke<void>('approve_client', { name, fingerprint }),
  dismissClient: (name: string, fingerprint: string) =>
    invoke<void>('dismiss_client', { name, fingerprint }),
  revokeClient: (name: string) => invoke<void>('revoke_client', { name }),

  currentPrompt: () => invoke<TrustPrompt | null>('current_server_prompt'),
  answerPrompt: (trust: boolean) => invoke<void>('answer_server_prompt', { trust }),

  logSnapshot: () => invoke<LogLine[]>('log_snapshot'),
  clearLog: () => invoke<void>('clear_log'),

  /** The "Tentang GeserDesk" box. */
  showAbout: () => invoke<void>('show_about'),
  /** Open (or raise) a tool window: `settings`, `clients`, `approvals`, `terminal` or `log`. */
  openTool: (tool: 'settings' | 'clients' | 'approvals' | 'terminal' | 'log') => invoke<void>('open_tool', { tool }),
  /** Run `geserdesk <line>` (the CLI beside the app). Output and the end arrive as events with the id it returns. */
  terminalRun: (line: string) => invoke<number>('terminal_run', { line }),
  terminalKill: () => invoke<void>('terminal_kill'),

  /** Stop the files pull in progress; what it staged is removed. */
  cancelTransfer: () => invoke<void>('cancel_transfer'),
  /** The latest pull report, for a progress window that opened after it was sent. */
  latestTransfer: () => invoke<TransferProgress | null>('latest_transfer'),
}

/** Subscribe to a shell event; returns the function that unsubscribes. */
export function on<T>(event: string, handler: (payload: T) => void): () => void {
  const un = listen<T>(event, (e) => handler(e.payload))
  return () => void un.then((f) => f())
}

export const EVENTS = {
  status: 'session://status',
  log: 'log://line',
  prompt: 'trust://prompt',
  daemon: 'daemon://info',
  navigate: 'ui://navigate',
  settings: 'settings://changed',
  terminalOut: 'terminal://out',
  terminalExit: 'terminal://exit',
  transfer: 'transfer://progress',
} as const

/** Error text from a rejected command (the shell rejects with a string). */
export function errorText(e: unknown): string {
  return typeof e === 'string' ? e : e instanceof Error ? e.message : String(e)
}
