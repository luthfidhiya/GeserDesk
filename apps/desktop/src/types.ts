// Mirrors of the Rust types the shell sends over IPC
// (crates/geserdesk-app and crates/geserdesk-server/src/admin.rs).

export interface Rect {
  x: number
  y: number
  w: number
  h: number
}

export interface Point {
  x: number
  y: number
}

export type Mode = 'client' | 'server'
export type Theme = 'system' | 'light' | 'dark'

export interface Settings {
  mode: Mode
  client: { server: string; name: string; log_only: boolean }
  server: { config: string }
  close_to_tray: boolean
  theme: Theme
}

export type Status =
  | { kind: 'stopped' }
  | { kind: 'connecting' }
  | { kind: 'connected' }
  | { kind: 'awaiting_approval'; detail: string }
  | { kind: 'retrying'; detail: string }
  | { kind: 'fatal'; detail: string }
  | { kind: 'server_running' }
  | { kind: 'error'; detail: string }

/** The daemon the app is attached to (the process that owns the session). */
export interface DaemonInfo {
  protocol: number
  app_version: string
  pid: number
  /** The kind of session running now, if any. */
  mode: Mode | null
  /** Who started it: `cli` or `app`. */
  started_by: string
  /** False when it speaks another protocol: it can only be read and ended. */
  compatible: boolean
}

export interface TrustPrompt {
  server: string
  fingerprint: string
}

export interface LogLine {
  time: string
  level: 'ERROR' | 'WARN' | 'INFO' | 'DEBUG' | 'TRACE'
  text: string
}

export type TransferState =
  | { kind: 'running' }
  | { kind: 'done' }
  | { kind: 'failed'; detail: string }

/** One report of a files pull in progress, or just finished (crates/geserdesk-app/src/transfer.rs). */
export interface TransferProgress {
  seq: number
  /** The other machine's screen name, or "server" when pulling from it. */
  peer: string
  file: string
  file_index: number
  file_count: number
  bytes_done: number
  bytes_total: number
  state: TransferState
}

export interface Screen {
  name: string
  local: boolean
  connected: boolean
  placeholder: boolean
  origin: Point
  monitors: Rect[]
  trusted_fingerprint: string | null
}

export interface Pending {
  name: string
  fingerprint: string
  addr: string
  replaces: string | null
  age_s: number
}

export interface DeskSnapshot {
  local: string
  screens: Screen[]
  config_path: string | null
  tls: boolean
  server_fingerprint: string | null
  pending: Pending[]
}

/** Positions being edited, by screen name. */
export type Draft = Record<string, Point>

export type Tab = 'control' | 'layout'
