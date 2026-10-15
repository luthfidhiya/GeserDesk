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

export interface Size {
  w: number
  h: number
}

/** The server config's port, switching and sharing settings (crates/geserdesk-server/src/config.rs). */
export interface Tuning {
  port: number
  switch_delay_ms: number
  switch_double_tap_ms: number
  switch_corner_size: number
  jump_zone_size: number
  scroll_lock_locks: boolean
  clipboard_sharing: boolean
  clipboard_files: boolean
}

export type Mode = 'client' | 'server'
export type Theme = 'system' | 'light' | 'dark'

export interface Settings {
  mode: Mode
  client: { server: string; name: string; log_only: boolean }
  server: { config: string }
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
  | { kind: 'cancelled' }
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
  /** How much of the desk it covers: its real extent, or what it was stretched to. */
  size: Size
  monitors: Rect[]
  trusted_fingerprint: string | null
  /** Turned off: nothing connects as it until it is turned on again. */
  disabled: boolean
}

/** A client the server trusts: the screen name it may use and its certificate. */
export interface TrustedClient {
  name: string
  fingerprint: string
}

export interface Pending {
  name: string
  fingerprint: string
  addr: string
  replaces: string | null
  age_s: number
  /** The config does not list this name: approving adds it as a new screen. */
  new_screen?: boolean
}

export interface DeskSnapshot {
  local: string
  screens: Screen[]
  config_path: string | null
  tls: boolean
  server_fingerprint: string | null
  pending: Pending[]
}

/** Where a machine sits on the desk and, when stretched, how big it is there. */
export interface Placement extends Point {
  /** Desk size; both absent means the machine's real size. */
  w?: number
  h?: number
}

/** Placements being edited, by screen name. */
export type Draft = Record<string, Placement>

export type Tab = 'control' | 'layout'
