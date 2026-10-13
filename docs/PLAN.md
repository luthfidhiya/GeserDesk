<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# GeserDesk — Implementation Plan

## Context

The need: one keyboard and mouse driving several computers over a LAN. Input
Leap (C++, a fork of Barrier/Synergy) already does this, but it carries manual
memory management in a long-running background process, a heavyweight
cross-platform build (CMake + Qt + native deps), and Synergy-era protocol
baggage.

GeserDesk rebuilds the idea in Rust: compile-time memory safety, `cargo`
tooling, and a clean new protocol. Core features: keyboard/mouse sharing,
clipboard sync, file transfer.

### Settled parameters

| | |
|---|---|
| Name | **GeserDesk** — repo `geserdesk`, binary `geserdesk`, crates `geserdesk-{proto,server,client}` |
| Platforms | **Linux (X11) + Windows 11 only.** macOS cut (no Mac hardware for testing). Wayland deferred to a late milestone. |
| Protocol | New, no interop with Input Leap |
| License | **GPL-3.0-or-later** |
| Time budget | ~15+ h/week — full scope through GUI + installer is realistic |
| Testing | **Direct Linux ↔ Windows**, both physical machines, no VM |
| Rust experience | New to the language — this drives the milestone ordering below |

---

## License boundary (read before touching code)

The Input Leap source headers say *"under the terms of the GNU General Public
License found in the file LICENSE"* with **no "or any later version" clause**, and
that file is GPL **version 2**. So Input Leap is **GPL-2.0-only**, and
GPL-2.0-only code cannot be included in a GPL-3.0 project.

| Source | Read it? | Copy / transliterate? |
|---|---|---|
| Input Leap (C++) | yes — study the algorithms and mechanisms | **no** — understand the concept, then write your own Rust |
| `lan-mouse` (Rust) | yes | **check its license first.** If GPL-3.0, adapt with attribution; otherwise treat like Input Leap |

Algorithms and architecture ideas are not copyrightable; the expression is. This
is not a real obstacle — the protocol is new and the Rust is written from
scratch. Only literal copying is off-limits.

---

## Principles behind the milestone order

Because Rust is new, the usual order is inverted: **the parts that teach Rust
come first; the parts that need `unsafe` and OS APIs come last.**

1. **Start with pure Rust.** The protocol and codec touch no OS API — ideal for
   learning enums, traits, `Result`, and testing without platform noise.
2. **Injection before capture.** Injection can use the `enigo` crate (safe, no
   `unsafe`, works on Linux *and* Windows). Capture needs direct platform APIs.
   Injection first means something moves on screen far sooner.
3. **Hotkey before edge-crossing.** Switching screens with a hotkey is far
   simpler than edge detection + pointer grab + cursor warp. Prove the loop
   works, then add the signature feature.
4. **TLS deferred to M8.** `rustls` + trust-on-first-use is a lot of concepts at
   once. Plain TCP until then — **do not use on an untrusted network before M8**,
   every keystroke is sent in clear text.
5. **Prefer crates that hide `unsafe`:** `x11rb` (not raw X11 bindings),
   `windows` (not hand-rolled `winapi`), `enigo`, `arboard`.

---

## Findings from studying Input Leap

**1. Not all the crates in the original brief fit.**

| Crate | For | Verdict |
|---|---|---|
| `enigo` | input injection (client) | **Good, and the starting point.** Wraps XTest and SendInput — exactly what Input Leap uses. |
| `rdev` | input capture (server) | **Not enough.** No pointer grab/ungrab, no cursor warp, no relative mode. Capture needs direct platform APIs. |
| `arboard` | clipboard | **Fine for read/write**, but no change notification — needs polling. |
| `tokio` | networking | **Good**, with the concurrency caveat below. |

**2. The `i16` coordinate limit is a real, fixable shortcoming.** Input Leap
sends positions as signed 16-bit (`kMsgDMouseMove = "DMMV%2i%2i"` in
`protocol_types.cpp`), capping screens at ±32767 px. GeserDesk uses `i32`.

**3. The key model is worth imitating in concept.** Input Leap sends `KeyID`
(UTF-32, U+E000–U+EFFF for control keys) **and** `KeyButton` (physical
scancode) — see `key_types.h`. The reason matters: on release the produced
`KeyID` can differ from press (dead keys, mismatched layouts), so the receiver
needs a stable physical id to know which key to release. GeserDesk uses the
**HID usage code** as that physical id — it has off-the-shelf mapping tables to
evdev / Windows VK / macOS everywhere, unlike Input Leap's private scheme.

**4. Prior art to study: [`lan-mouse`](https://github.com/feschber/lan-mouse).**
A Rust software KVM that has already solved the hardest platform parts
(including libei/Wayland). Not to copy — a reference for when a backend gets
stuck. **Verify its license before borrowing any code.**

**Scale note:** Input Leap's platform code alone is ~28,800 lines; the whole
tree ~75,000. The milestones below keep each step to something that runs.

---

## Workspace layout

Start small — don't create every crate on day one; split when it hurts.

```
geserdesk/
├── Cargo.toml                    # [workspace]
├── LICENSE                       # GPL-3.0
├── crates/
│   ├── geserdesk-proto/          # M1: message types, codec, framing
│   ├── geserdesk-platform/       # M4: trait + x11/ and windows/ backends
│   ├── geserdesk-tls/            # M8: identities, fingerprints, pinned TLS 1.3
│   ├── geserdesk-server/         # M5: screen layout, switch policy; admin.rs for front ends
│   ├── geserdesk-client/         # M3: connection, receive events, inject
│   └── geserdesk-app/            # application layer for any front end (no GUI toolkit)
└── apps/
    ├── cli/                      # the `geserdesk` binary (server / client)
    └── desktop/                  # M11: Tauri shell (src-tauri/) + React page (src/), with tray
```

One CLI binary with subcommands (`geserdesk server`, `geserdesk client`), not
two separate binaries like Input Leap.

Core crates: `serde` + `postcard` (codec), `tokio` (async), `anyhow` +
`thiserror` (errors), `tracing` (logging), `clap` (CLI), `toml` (config).

---

## Protocol design

**Framing:** `[u32 length, little-endian][payload]`. Payload is encoded with
**`postcard`** (serde, varint, compact — important because mouse events stream
tens of times per second).

**Handshake:** the client sends `Hello` (version + screen name + geometry); the
server replies `Welcome` or `Incompatible`. Different major = reject.

```rust
enum ClientMsg {
    Hello { version: Version, screen: ScreenInfo },
    ScreenChanged(ScreenInfo),           // resent when resolution changes
    ClipboardGrab { id: ClipboardId, seq: u64 },
    ClipboardData { id: ClipboardId, seq: u64, chunk: Chunk },
    FileOffer { name: String, size: u64 },
    FileChunk(Chunk),
    KeepAlive,
}

enum ServerMsg {
    Welcome { version: Version, keepalive: Duration },
    Incompatible { server_version: Version },
    Rejected { reason: String },
    Enter { x: i32, y: i32, seq: u64, toggle_mods: ModMask },
    Leave,
    Key(KeyEvent),                        // Down | Up | Repeat
    Mouse(MouseEvent),                    // MoveAbs | MoveRel | Button | Wheel
    ClipboardGrab { .. }, ClipboardData { .. },
    FileOffer { .. }, FileChunk(..),
    SetOptions(Options),
    Close(CloseReason),
    KeepAlive,
}

struct KeyEvent {
    hid: u16,           // HID usage code — the canonical physical id
    ch: Option<char>,   // the char the server's layout produced — fallback
    mods: ModMask,
    action: KeyAction,
}
```

`Chunk` = `Start { total: u64 } | Data(Vec<u8>) | End`, mirroring Input Leap's
`kDataStart / kDataChunk / kDataEnd` marks (`FileChunk.cpp`), which is a proven
design. Chunk size is 32 KiB, like Input Leap's `StreamChunker`.

**Improvements over Input Leap 1.6:** `i32` coordinates, `u64` sequence numbers,
no 32 KiB clipboard cap (backpressure instead), file transfers carry name/size
metadata from the start.

**Security (M8):** TLS 1.3 via `rustls` (pure Rust, no OpenSSL — removes a
painful native dependency). Self-signed certs + trust-on-first-use with a
SHA-256 fingerprint shown to the user and stored in a trusted-fingerprints file,
mirroring `src/lib/net/FingerprintDatabase.cpp`.

See [`PROTOCOL.md`](PROTOCOL.md) for the on-the-wire details.

---

## Platform abstraction

Trait in `crates/geserdesk-platform/src/lib.rs` — created at M4, once there are
two real implementations to compare:

```rust
pub trait InputInject: Send {            // client side — EASY, use enigo
    fn key(&mut self, ev: &KeyEvent);
    fn mouse_button(&mut self, btn: ButtonId, down: bool);
    fn mouse_move_abs(&mut self, x: i32, y: i32);
    fn mouse_move_rel(&mut self, dx: i32, dy: i32);
    fn mouse_wheel(&mut self, dx: i32, dy: i32);
}

pub trait InputCapture: Send {           // server side — HARD, direct platform APIs
    fn set_active_edges(&mut self, edges: EdgeSet);
    fn grab(&mut self) -> Result<()>;
    fn ungrab(&mut self) -> Result<()>;
    fn warp_cursor(&mut self, x: i32, y: i32);
    fn hide_cursor(&mut self, hidden: bool);
    fn screen_bounds(&self) -> Rect;
}
```

| Platform | Inject | Capture |
|---|---|---|
| Linux X11 | XTest via `enigo` | XInput2 raw events + `XGrabPointer` (`platform/XWindowsScreen.cpp`) via `x11rb` |
| Windows | `SendInput` via `enigo` | `SetWindowsHookEx(WH_MOUSE_LL / WH_KEYBOARD_LL)` (`platform/MSWindowsHook.cpp`) via the `windows` crate |

**Windows notes:** low-level hooks do **not** need DLL injection (Input Leap's
`synwinhk` DLL is legacy). Anticipate: UIPI stops a non-elevated process from
sending input to elevated windows, and the secure desktop (UAC / lock screen) is
unreachable. Document these as limitations; don't fight them.

---

## Concurrency model (the easiest thing to get wrong)

Platform input APIs are **not async and are thread-bound**: a Windows low-level
hook needs a message pump on the thread that installed it; X11 has its own event
loop.

Input Leap solves this with `EventQueue` + per-platform `*EventQueueBuffer`. The
Rust equivalent:

> **Each platform backend runs on a dedicated OS thread with its own native
> event loop, and talks to the tokio world over `tokio::sync::mpsc`.**

Do not try to wrap a hook or event tap in an `async fn`.

```
[platform thread]  --mpsc-->  [tokio task: server logic]  --mpsc-->  [tokio task: per client]
   (native loop)                 (edge detect, switch)                    (framed I/O)
```

Injection on the client also goes through a channel to the platform thread, not
called directly from an async task.

---

## Server core logic

In `crates/geserdesk-server` — concepts taken from Input Leap's `Server.cpp`
(`mapToNeighbor`, `isSwitchOkay`, `getJumpZoneSize`, `isLockedToScreen`),
**rewritten, not transliterated**:

- **Screen layout graph** — each screen has left/right/up/down neighbours.
- **Edge detection** — a jump zone at each screen edge.
- **Switch policy** — switch delay, double-tap, corner size, modifier
  requirements.
- **Lock to screen** — Scroll Lock pins the cursor to the active screen.
- After a switch: hide and warp the local cursor to centre, grab input, forward
  as relative motion.

Write `neighbor` and the switch policy as **pure functions** over layout structs
— no I/O, no wall-clock time — so they unit-test without any devices.

---

## Configuration

TOML, not the bespoke `section: screens … end` grammar Input Leap uses. Location
via the `directories` crate.

```toml
[server]
listen = "0.0.0.0:24810"

[[screens]]
name = "linux-pc"
[[screens]]
name = "windows-pc"

[links]
linux-pc   = { right = "windows-pc" }
windows-pc = { left  = "linux-pc" }

[options]
switch_delay_ms      = 0
switch_double_tap_ms = 250
scroll_lock_locks    = true
clipboard_sharing    = true
```

Options are distilled from Input Leap's `option_types.h`, dropping the obsolete
ones (heartbeat, half-duplex, XTest-Xinerama).

---

## Milestones

All testing is **direct Linux ↔ Windows**, no VM.

### Phase A — pure Rust (learn the language, no OS APIs)

| # | Milestone | Done when | Rust concepts exercised |
|---|---|---|---|
| **M1** | workspace + `geserdesk-proto`: message types, postcard codec, framing | `cargo test` green; every message variant round-trips | enums, `derive`, `Result`, modules, unit tests |
| **M2** | plain TCP + tokio: server listens, client connects, exchange `Hello`/`Welcome`/`KeepAlive` | Linux and Windows handshake over the LAN | `async`/`await`, `tokio::spawn`, channels, error handling |

### Phase B — touch the OS via the easy path

| # | Milestone | Done when |
|---|---|---|
| **M3** | injection via `enigo` (Linux + Windows at once — `enigo` is cross-platform). Server sends synthetic mouse events | **the mouse on the Windows PC moves, driven from Linux.** No capture yet |

> M3 is the **feasibility gate**. If it works, the rest is predictable work.

### Phase C — capture, the first hard part

| # | Milestone | Done when |
|---|---|---|
| **M4** | X11 capture (`x11rb`), **hotkey mode**: press hotkey → input forwarded to Windows, press again → back to local | **a working KVM**, even if switching is via a key |
| **M5** | edge crossing: edge detection + `XGrabPointer` + cursor warp + relative motion | the signature feature — the cursor moves between screens by pushing to the edge |
| **M6** | full keyboard: HID mapping, modifiers, auto-repeat, correct key release | typing works on Windows from the Linux keyboard, including mismatched layouts |
| **M7** | Windows capture: low-level hook + message pump on a dedicated thread | Windows can be the server — the reverse direction works |

### Phase D — production

| # | Milestone |
|---|---|
| **M8** | TLS (`tokio-rustls`) + fingerprint TOFU, concept from `FingerprintDatabase.cpp`. **Safe on a real network from here on** |
| **M9** | clipboard sync (`arboard` + change polling) |
| **M10** | file transfer (32 KiB chunks on the M1 foundation) |
| **M11** | GUI + tray — drag-and-drop grid for the layout. Planned as `egui`; **built as Tauri 2 + React instead** (session 6): the layout editor and approvals were already a web UI, and one front end beats two. Costs a system WebView (WebKitGTK on Linux, WebView2 on Windows 11) |
| **M12** | autostart + packaging (`.deb`, InnoSetup installer). The daemon from session 7 is what autostart now has to launch at login (systemd user unit; on Windows a per-user startup entry, not a service, because capture needs the interactive session) |

### Later

| # | Milestone |
|---|---|
| **M13** | Wayland: portal `InputCapture` + libei/EIS (`reis` + `ashpd`), for Ubuntu's default session |

---

## Progress

### Session 1 — 2026-09-02 — M1 + M2 done and verified

| Area | Status | Verification |
|---|---|---|
| Rust toolchain (rustup, stable 1.98) + `gcc` + `libxdo-dev` | done | installed |
| Workspace + `LICENSE` + `.gitignore` + CI (`.github/workflows/ci.yml`, ubuntu + windows matrix) | done | present |
| **M1** — `geserdesk-proto`: `Version`/negotiation, `geometry` (Point/Rect/Edge/EdgeSet), `input` (KeyEvent HID + ModMask + MouseEvent), `clipboard` (Chunk + ChunkAssembler), `message` (ClientMsg/ServerMsg/Options), `codec` (LE u32 framing + postcard, large-frame guard) | done | **30 unit + 6 integration tests green, incl. a pseudo-fuzz that the decoder never panics** |
| **M2** — `geserdesk-server::net` (listener, accept loop, `server_handshake`, keep-alive 3 s / 3 misses) + `geserdesk-client::net` (connect, `client_handshake`, session loop) | done | **22 server + 5 client tests; end-to-end loopback smoke test: handshake + 3 keep-alive cycles + clean shutdown** |
| `geserdesk-server::config` (TOML) + `layout` (pure `neighbor` / `SwitchState::evaluate` — delay, double-tap, scroll-lock) | done | covered by the 22 server tests |
| CLI `geserdesk` (`clap` server/client subcommands, `--dry-run`, `-v/-vv`, `tracing`) | done | **run, logs verified** |
| **M3** — `InputSink` trait + `NullSink`/`RecordingSink` + `EnigoSink` (feature `inject`, enigo 0.3) | written | **`cargo build --features inject` succeeds; real injection not yet tested** (needs a graphical session) |
| `cargo clippy --all-targets` + `cargo fmt --check` | done | **clean** |

Total: **57 tests green**, clippy and fmt clean, ~3,150 lines of Rust.

**Next session, on the desktop:**
```bash
cd geserdesk
cargo test --workspace                 # still 57 green
cargo run -p geserdesk-cli -- server --config dev-config.toml -v
cargo run -p geserdesk-cli --features inject -- client --name windows-pc --server 127.0.0.1:24810 -v
# then real M3: does EnigoSink actually move the mouse / type? Fix the enigo API
# if needed (consider bumping to enigo 0.6 to drop the libxdo C dependency).
```

### Session 2 — 2026-10-12 — M3 verified; M4 + M5 + M6 on Linux/X11

| Area | Status | Verification |
|---|---|---|
| **M3** — `EnigoSink` injection | done on Linux | server `--demo` drove the real cursor through a square on X11 |
| `geserdesk-platform` crate: `CaptureEvent`/`CaptureCmd`, `keymap` (evdev -> HID), X11 backend (`x11rb`, dedicated thread) | done | 12 unit tests; exercised against a live X server |
| **M4** — grab/ungrab, blank cursor, delta-by-warp, hotkey **Ctrl+Alt+Home** (polled via `QueryKeymap`) | done | hotkey toggled there and back; grabbed keys reached the client |
| **M5** — edge detection, proportional entry point, return through the link, multi-hop between remotes, disconnect handling | done | `router::Core` pure state machine (19 tests incl. a fake-capture -> TCP pipeline); live X11 edge crossing both ways |
| **M6** — keyboard: HID + `ch` + modifiers, auto-repeat (XKB detectable repeat), release by remembered key | done on Linux | `a`, `Shift+a`, Enter, arrows, mouse buttons, wheel all arrived correctly |
| `Hub` (per-client queues), `local_screen` config, real client display size via `enigo` | done | tests |

Bugs found and fixed on the way:

- `MessageStream::recv` used `read_exact`, which is **not cancel-safe**; losing a `select!` race could drop half a frame and desync the stream. Rewritten around a persistent buffer.
- Ctrl-C did nothing: the accept loop never watched the shutdown signal.
- After a grab, deltas were measured from the screen centre instead of the pointer's real position, giving a huge first jump.
- **Ctrl+Alt+F1-F12 can't be the hotkey**: Xorg turns them into VT switches and never delivers them. Passive `XGrabKey` also got nothing under XTest here, so the hotkey is polled instead.

Not done / known limits:

- Windows is still client-only (capture is M7). Windows injection untested.
- On Windows `enigo` types printable keys as Unicode, so Ctrl+letter shortcuts may not behave; a VK/scancode path is needed.
- The hotkey's own modifiers are released on the remote but the local app that had focus never sees their release (can leave a modifier "stuck" until the next press).
- Mouse is always sent as absolute (`MoveAbs`) positions; `relative_mouse_moves` (for games) is not honoured yet.
- Do **not** run server (capture) and client (inject) on the same X display: the client's injected input is captured again. Use `--dry-run` for loopback.

### Session 3 — 2026-10-12 — monitor detection and the layout editor

- **Protocol 2.0:** `ScreenInfo.monitors`; clients detect monitors (X11 RandR; Windows `EnumDisplayMonitors`, **compiles but never run on Windows**) and re-report on change.
- **Virtual desk** (`desk.rs`): each machine has an origin; monitors that touch are neighbours, so offsets work. `[links]` still place unpositioned screens. The router (`router.rs`) was rewritten on this model: entry points follow the real geometry, multi-monitor machines work, dead stretches of edge don't switch.
- **Layout editor** (`ui.rs` + `ui.html`): axum on `127.0.0.1:24811`; drag/snap/arrow-nudge, overlap check, saves live and into the config with `toml_edit` (comments kept). Guarded by a Host-header check and JSON-only POST.
- Verified in a real browser: drag, snap, save, config rewritten, server picked it up. 123 tests green.
- Windows client needs the DPI-aware call (`init_process`) so reported sizes match real pixels.

**Needs a rebuild on both machines** (protocol 2.0 is not compatible with 1.x).

### Session 4 — 2026-10-12 — M8: TLS, pinning, trust on first use

- **`geserdesk-tls`**: self-signed identities (`rcgen`), SHA-256 fingerprints, TLS 1.3 only via `rustls` + `ring` (no OpenSSL, no CMake/NASM), custom verifiers that check the handshake signature in full but judge trust by fingerprint.
- **Both directions authenticated.** Client pins the server (first contact asks; a changed certificate is fatal and never retried). Server requires a client certificate and maps fingerprint -> screen name after `Hello`; unknown or changed certificates are refused and listed for approval.
- **Approval in the layout editor** (decided with the user): pending list with fingerprints, *Percayai* / *Abaikan*, per-screen *Cabut kepercayaan*; saved as `fingerprint = "..."` in the config. Only a pending request with the exact fingerprint shown can be approved.
- Found on the way: the handshake had **no timeout**, so a plaintext/TLS mismatch (or a stalled peer) left both sides waiting forever. Added 10 s limits on both ends.
- Verified with the real binaries: new client -> pending -> approved in the browser -> connects on retry; wrong server fingerprint exits with an error; plaintext client refused; terminal prompt `n`/`y`/remembered. 162 tests.
- **Confirmed on real hardware afterwards:** the Windows build (MSVC + `ring`), the TLS pairing (server fingerprint confirmed, client approved in the layout editor), a live session with cursor crossings, and Windows monitor detection (two monitors reported). **Still unchecked:** key-file permissions on Windows.
- Both machines need rebuilding; a TLS client cannot talk to a `--no-tls` server or vice versa.

### Session 5 — 2026-10-12 — the `geserdesk-gui` window

- **Why:** the user wanted less CLI: a window like Barrier's (mode, settings, Start/Stop, status, live log). Layout and approvals stay on the server's web page; the client has no web page.
- **`apps/gui`** (eframe/egui): Client or Server mode, remembered settings (`gui.toml`), coloured status, scrolling log fed by a `tracing` layer, *Buka pengaturan layar*, and a dialog instead of the terminal prompt for trusting a new server. `--start` begins immediately.
- **Core changes it needed:** `run_server(config, capture, shutdown)` stops cleanly (closes every session via a `JoinSet`, drops the capture handle so the grab is released, stops the editor); `connect_with_status` reports a `ClientStatus`; the known-servers store moved to `geserdesk_client::KnownServers` so the CLI and the window share it.
- **Bug found by driving the real window:** the prompt takes a human far longer than the server's 10 s Hello timeout, so the connection that showed the prompt was dead by the time the user clicked (log: `Broken pipe`, then a retry). After approval the client now opens a fresh connection pinned to exactly the approved fingerprint (a different certificate there is a fatal mismatch, not a second prompt).
- Verified on this Linux desktop by typing into and clicking the window with XTest: type the address, Start, fingerprint dialog, approve on the server, connected; server mode Start/Stop/Start; closing the window frees the ports. 174 tests.
- **Not done:** the tray icon (Windows-first; GTK on Linux), autostart, and any check of the Windows build of the window (`ring` needs a C compiler to cross-check from Linux).

---

## Odds of getting there (≥15 h/week, Linux + Windows only)

| Point | Odds | Cumulative time estimate |
|---|---|---|
| **M2** — protocol + networking working | ~97% | 2–3 weeks |
| **M3** — Windows mouse moves from Linux | ~88% | 1–1.5 months |
| **M5** — edge crossing Linux → Windows | ~72% | 2.5–4 months |
| **M7** — full two-way, mouse + keyboard | ~62% | 4–6 months |
| **M12** — the whole thing, GUI + installer | ~40% | 9–14 months |

Cutting macOS/Wayland and a real time budget lift these meaningfully. The
biggest risk is still **not technical** — it's losing momentum after the fun part
(M5–M6).

---

## Verification strategy

**Unit tests** (`cargo test`) — run from M1, no devices needed:
- `proto` codec: round-trip every variant, plus a fuzz that never panics
- framing: truncated and oversized frames are rejected, not panicked
- `server`: `neighbor` and switch policy as pure functions over mock layouts

**Integration tests** — the key to testing without physical devices:
- mock backends `MockCapture` (replays recorded events) and `MockInject`
  (records what it receives)
- server + client in one process over `tokio::io::duplex()`
- assert the client injects exactly the sequence the server captured
- scenarios: enter/leave, clipboard round-trip, client drop mid-transfer

**Manual, per milestone:**
```bash
# Linux PC — server
cargo run -p geserdesk-cli -- server --config ./dev-config.toml -v
# Windows PC — client
cargo run -p geserdesk-cli --features inject -- client --name windows-pc --server <IP>:24810 -v
```
M1–M2 test over loopback on one machine. M3+ needs both PCs on the same LAN.

**CI:** GitHub Actions matrix `ubuntu-latest` + `windows-latest` running
`cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, and a build
with the `inject` feature.

---

## Main risks

| Risk | Mitigation |
|---|---|
| New to Rust + systems programming at once | Phase A is all pure Rust; `unsafe` first appears at M7. Prefer crates that wrap raw APIs |
| Copying Input Leap (GPL-2.0-only) into a GPL-3.0 project | Read for concepts, **rewrite**. Verify `lan-mouse`'s license before adapting its code |
| Cross-layout keyboard mapping — Input Leap's biggest source of bugs | Send HID scancode *and* character; client tries the scancode first, falls back to synthesising the character. Test explicitly with different server/client layouts |
| Grab/warp timing on X11 is subtle and hard to debug | M4 (hotkey) separates "capture works" from "edge crossing works" so bugs can be isolated |
| Stalling after the prototype runs | M3 is deliberately early as a morale marker; every milestone produces something visible |
| Using it on a real network before TLS exists | Flagged explicitly: trusted networks only until M8 |

### Session 6 — 2026-10-12 — one front end: Tauri + React, and a layered code base

- **Why:** two half-finished front ends (the egui window and the axum web page) split the settings across two places, and window code had leaked into the core (`runner.rs` woke egui; `ui.rs` mixed state, validation, config writing and HTTP). The user asked for a single GUI on Tauri + React and a tidier core/GUI split.
- **Decisions (with the user):** the web page is **deleted outright** (no HTTP left in the core, so `axum`/`serde_json`/`webbrowser` are gone from the workspace); UI is Tailwind + shadcn/ui; system packages installed from the session.
- **Layers:** `proto -> platform/tls -> server/client -> geserdesk-app -> geserdesk-desktop -> React`.
  - `geserdesk-server::admin::DeskState` is the old `ui.rs` minus HTTP: `snapshot()`, `apply_layout()`, `approve/dismiss/revoke()`, typed `AdminError`, same validation, same comment-preserving `toml_edit` writes, same undo when a config write fails. `run_server_with(.., ready)` hands the `DeskState` to the caller once the listener is up; `run_server` is unchanged for the CLI.
  - `geserdesk-app` (new): `Session` (start/stop, `Status` over a `watch` channel, same 3 s grace before forcing a server down), `settings` (`gui.toml`, plus `close_to_tray`), `log` (ring buffer + `broadcast` stream), `trust` (`PromptTrust`, the 5-minute first-contact question).
  - `apps/desktop/src-tauri`: commands, event forwarding, tray. Quit always stops the session first so the keyboard and mouse are released.
  - `apps/desktop/src`: React; `lib/geometry.ts` is a pure port of the old page's snap/overlap/neighbour/auto-arrange code with vitest tests.
- **Config:** `Config.ui` and `--no-ui` are gone; a leftover `[server] ui = ...` is ignored with a warning, so old configs still load.
- **Tray:** icon + tooltip follow the status; menu = open, layout tab, Start/Stop, quit; left click toggles the window; closing the window hides it (setting `close_to_tray`); if the tray cannot be created the window closes normally.
- **Verified:** `cargo fmt`, `clippy -D warnings`, `cargo test --workspace` (all green; server 88, app 15), `vitest` (10), and the page builds.
- **Driven end to end on this Ubuntu 24.04 / X11 desktop** (`tauri dev`, isolated `XDG_CONFIG_HOME`, loopback ports, input via libxdo, screenshots read back): server Start; a dry-run CLI client appearing under *Menunggu persetujuan* with the same fingerprint it printed; *Percayai* writing `fingerprint = ...` and the client getting in on its next retry; dragging with snap and *Simpan & terapkan* writing `x`/`y` with the config's comments intact; two-click *Cabut kepercayaan* removing the fingerprint; Stop releasing the port. Client mode: the first-contact dialog showed the server's fingerprint (matching the server log), *Percayai & Sambungkan* pinned it in `known_servers.toml`, the status became *Menunggu persetujuan*. Tray: icon visible, menu (Buka jendela / Pengaturan layar / Start-Stop / Keluar) works, its Stop and Start drive the session and the label follows the status, closing the window hides it with the session still running, *Pengaturan layar* re-shows it on the layout tab, *Keluar* stops the session and exits.
- **Still not verified:** the Windows build of the app and the new CI steps (pnpm/Node install, Tauri apt packages); a real cursor crossing driven from the app (the test server never had a capturing client at an edge); the exit code of `tauri dev` after *Keluar* (pnpm printed `ELIFECYCLE` although the app logged a clean quit).
- **Known gaps:** no toast when the window hides to the tray (a log line says so); on GNOME the window comes back at a different position after hide/show; the layout is polled every 2 s rather than pushed; autostart is still M12.
- **UI rework (same session, after review):** mode tabs *Client | Server* now sit at the top; a server gets *Konfigurasi* and *Pengaturan monitor* beneath them, a client only its configuration. The monitor tab lists only machines that are connected right now (no stand-in boxes); trusted-but-offline machines get a small list so they can still be revoked. `DeskState::validate` now checks overlap only among connected machines. New `theme` setting (`system` / `light` / `dark`, in `gui.toml`) with a single sun/moon icon toggle (the stored value stays `system` until the first click). Checked by screenshot only (client + light, server + dark); the monitor tab itself was not driven again because the user's own server was running.

### Session 7 — 2026-10-12 — daemon and controllers (the lan-mouse model)

- **Why:** the app hosted its own server, so it clashed with a CLI server (port 24810, two input grabs) and closing the window stopped it. The agreed direction: one long-lived daemon owns network/TLS/capture/injection, and the app, the CLI and the tray are controllers that attach.
- **Decisions (with the user):** the app **always** starts the daemon as a separate process (no server inside the window any more); the daemon is **this same program run with `--daemon`**, so no sidecar binary to ship; a daemon lives as long as its session; closing the window stops nothing.
- **Built:** `geserdesk-app::control` (JSON lines over a Unix socket / named pipe, plus a token file; `wire`, `transport`, `service`, `remote`), `daemon.rs` (runs the service, exits when the session ends / on shutdown / idle / Ctrl-C / SIGTERM, stopping the session first), `Session::start_server_config`, CLI `server` through the daemon plus `status` / `stop`, and the Tauri shell as a controller (`link.rs` attaches, also to a daemon started later; the tray gained *Hentikan dan keluar*). The page follows the daemon's mode and shows a version warning.
- **Why JSON, not the wire protocol's postcard:** the status types are tagged enums that postcard cannot read back, and JSON can be watched with `socat`. The channel is local and low volume.
- **Found while testing:** a `sockaddr_un` path holds about 100 bytes, so a long directory (isolated test dirs) made the OS error opaque; it is now explained. A request made after the daemon was gone waited the full 20 s timeout; it now fails at once. A daemon refusing a controller of another protocol would have left an old daemon unstoppable after an upgrade, hence the limited mode (Info and Shutdown only).
- **Verified:** `cargo fmt`, `clippy -D warnings`, `cargo test --workspace` (app: 23 unit + 12 integration tests running a real daemon over a real socket without a display), vitest. By hand with an isolated runtime directory: `geserdesk server --no-capture`, `status`, a refused second daemon, `stop` (socket and token removed, port freed), SIGTERM, and the GUI binary as a headless `--daemon` including its idle exit.
- **Driven with the real window afterwards** (isolated runtime directory and `XDG_CONFIG_HOME`, `GESERDESK_NO_CAPTURE=1`, loopback port, the user's own server stopped first, screenshots read back): the window opened with no daemon shows *Berhenti*; with a CLI daemon already running it attaches by itself and shows the daemon's mode (saved mode was client, shown as server), "dimulai oleh CLI", and the daemon's log history; a dry-run client appears under *Menunggu persetujuan*, *Percayai* writes the fingerprint through the CLI daemon and `geserdesk status` agrees; the monitor tab lists only connected machines; closing the window and *Keluar (sesi tetap jalan)* leave the CLI daemon and its client running. With no daemon, *Start* spawns the daemon (own process group, output in `daemon.log`, "dimulai oleh aplikasi"); quitting the window leaves it running; a new window attaches to the same PID with the full log; *Stop* ends the session and the daemon (port freed, runtime directory emptied); *Hentikan dan keluar* ends both.
- **Not verified:** the incompatible-protocol warning in the window (needs a daemon of another version; the daemon side is covered by an integration test), the detached spawn on Windows, the named pipe and its default access rules on Windows (the token is the protection there), and an upgrade across two real versions.
- **Known gaps:** the attach poll is every 1.5 s; no autostart yet (M12); a window attached to a running client shows the window's saved settings in its (disabled) fields, not the running client's address and name.
- **One path, the daemon (same day):** the user's aim is one app that runs from a terminal or from the window, with both seeing the same session. The Docker-style alternative (a daemon that stays up, a CLI of pure controllers) was discussed and set aside as more than needed; it fits better once M12 autostart is done. What changed: `geserdesk client` runs as the daemon too, `--no-control` is gone (a session without a channel would be invisible to the window), `StartSpec::Client` carries `ClientOptions` (`reconnect`, `tls`, `identity_dir`, forced `monitors` as well), `PROTOCOL` is 2, and `Session::start_client_options` takes who decides about trust (none = ask the attached controller; the CLI passes its terminal prompt). A bug found on the way: a session that ended before the daemon's first look was never noticed, so the daemon stayed up; `Session` now counts the sessions it started.
- **Verified:** unit tests for options -> connection settings, integration tests for a client through the channel against a real in-process server (connected with forced monitors, retrying while the server is away, `reconnect=false` ending with the connection, an unreachable server failing the daemon), and by hand with the real binaries in separate runtime directories: a client to nothing exits 1, a server daemon and a client daemon each show in their own `status`, `stop` on each cleans up, a one-shot client exits when the server is stopped. With the window (screenshots only): it attaches to a client started from the CLI and shows mode client, "Tersambung" and the client's log; against the user's own server, still the previous build (protocol 1), it shows the "protokol 1" warning and the *Hentikan daemon lama* button.
- **Not verified:** the terminal trust prompt on a real TTY (unchanged code, driven only with `--server-fingerprint`), Windows (named pipe, detached spawn), and an app attached to a client for Stop from the window.
