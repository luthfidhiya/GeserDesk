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
| **M7** | Windows capture: low-level hook + message pump on a dedicated thread | Windows can be the server — the reverse direction works. **Built, session 15; driven end to end on one machine, not yet with a second one** |

### Phase D — production

| # | Milestone |
|---|---|
| **M8** | TLS (`tokio-rustls`) + fingerprint TOFU, concept from `FingerprintDatabase.cpp`. **Safe on a real network from here on** |
| **M9** | clipboard sync (`arboard` + change polling) — **text session 8, HTML session 10, pictures session 11** |
| **M10** | file transfer — **done differently than planned: no chunked stream on the M1 foundation; a copy announces its manifest at once and the bytes are pulled by whoever needs them, when they need them (session 12)** |
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

- Windows is still client-only (capture is M7). Windows injection untested. *(Session 9: tested; see below.)*
- On Windows `enigo` types printable keys as Unicode, so Ctrl+letter shortcuts may not behave; a VK/scancode path is needed. *(Session 9: Ctrl+letter arrives as a real `VK_A` with Control held; the actual defect was elsewhere, in capitals and shifted symbols, now fixed.)*
- The hotkey's own modifiers are released on the remote but the local app that had focus never sees their release (can leave a modifier "stuck" until the next press).
- Mouse is always sent as absolute (`MoveAbs`) positions; `relative_mouse_moves` (for games) is not honoured yet.
- Do **not** run server (capture) and client (inject) on the same X display: the client's injected input is captured again. Use `--dry-run` for loopback.

### Session 3 — 2026-10-12 — monitor detection and the layout editor

- **Protocol 2.0:** `ScreenInfo.monitors`; clients detect monitors (X11 RandR; Windows `EnumDisplayMonitors`, **compiles but never run on Windows**) and re-report on change. *(Session 4 ran it on Windows; session 9 compared it with `EnumDisplayMonitors` and `GetSystemMetrics`: same rectangles. Re-reporting after a hot-plug while running is still not exercised.)*
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
- **Confirmed on real hardware afterwards:** the Windows build (MSVC + `ring`), the TLS pairing (server fingerprint confirmed, client approved in the layout editor), a live session with cursor crossings, and Windows monitor detection (two monitors reported). **Still unchecked:** key-file permissions on Windows. *(Session 9: checked. GeserDesk sets no ACL on Windows (`identity.rs` only sets mode 0600 under `cfg(unix)`); the file inherits the profile folder's ACL: the user, `SYSTEM`, Administrators. README now says so.)*
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
- **Still not verified:** *(Windows build of the app: built and launched in session 9)* the new CI steps (pnpm/Node install, Tauri apt packages); a real cursor crossing driven from the app (the test server never had a capturing client at an edge); the exit code of `tauri dev` after *Keluar* (pnpm printed `ELIFECYCLE` although the app logged a clean quit).
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
- **Not verified:** the incompatible-protocol warning in the window (needs a daemon of another version; the daemon side is covered by an integration test), the detached spawn on Windows, the named pipe and its default access rules on Windows (the token is the protection there), and an upgrade across two real versions. *(Session 9: the detached spawn and the named pipe work on Windows; the pipe's access rules and an upgrade across two versions remain unchecked.)*
- **Known gaps:** the attach poll is every 1.5 s; no autostart yet (M12); a window attached to a running client shows the window's saved settings in its (disabled) fields, not the running client's address and name.
- **One path, the daemon (same day):** the user's aim is one app that runs from a terminal or from the window, with both seeing the same session. The Docker-style alternative (a daemon that stays up, a CLI of pure controllers) was discussed and set aside as more than needed; it fits better once M12 autostart is done. What changed: `geserdesk client` runs as the daemon too, `--no-control` is gone (a session without a channel would be invisible to the window), `StartSpec::Client` carries `ClientOptions` (`reconnect`, `tls`, `identity_dir`, forced `monitors` as well), `PROTOCOL` is 2, and `Session::start_client_options` takes who decides about trust (none = ask the attached controller; the CLI passes its terminal prompt). A bug found on the way: a session that ended before the daemon's first look was never noticed, so the daemon stayed up; `Session` now counts the sessions it started.
- **Verified:** unit tests for options -> connection settings, integration tests for a client through the channel against a real in-process server (connected with forced monitors, retrying while the server is away, `reconnect=false` ending with the connection, an unreachable server failing the daemon), and by hand with the real binaries in separate runtime directories: a client to nothing exits 1, a server daemon and a client daemon each show in their own `status`, `stop` on each cleans up, a one-shot client exits when the server is stopped. With the window (screenshots only): it attaches to a client started from the CLI and shows mode client, "Tersambung" and the client's log; against the user's own server, still the previous build (protocol 1), it shows the "protokol 1" warning and the *Hentikan daemon lama* button.
- **Then driven further** (isolated runtime directories; the user's own server had no client connected, so no input could be forwarded): the terminal trust prompt on a real pseudo-terminal (answer `y` pins the server in `known_servers.toml`, a pinned server is not asked again, `n` exits 1 and saves nothing, and the server then sees the client); a second server in the same runtime directory is refused; `--no-tls` on both sides with `--monitors` (the server sees the two forced monitors, the identity directory stays untouched); SIGTERM on a client daemon releases the channel and the server sees it leave; an over-long runtime directory is explained; the window started with `--start` in client mode spawns its own daemon, connects over TLS to a pinned server, *Stop* ends the session and the daemon; a client started from the CLI is picked up by the already open window and *Stop* in the window ends that CLI process cleanly. Found: the window kept the previous daemon's log lines when another daemon attached; the log now follows the daemon (and stays readable after it ends).
- **Not verified:** Windows (named pipe, detached spawn; cannot be cross-checked here, `ring` needs a mingw C compiler), a daemon of a newer protocol than the window, and real input injection (not tried on this desktop on purpose). *(Session 9 covered Windows and real injection; a daemon of a newer protocol is still untried.)*

### Session 8 — 2026-10-13 — M9: clipboard sync (text)

- **Model:** push on change, last copy wins, no screen switch needed. A machine polls its clipboard every 500 ms (neither X11 via `arboard` nor Windows gives a cheap change signal); a text that differs from what it last saw or wrote is pushed as `ClipboardGrab` + `Start/Data/End`. Text a peer sent is written locally and remembered, so the next poll does not echo it. The server is the hub: a client's text goes onto the server's clipboard and on to every other client. Input Leap instead hands ownership over on screen entry/leave; with two or three machines on one desk a plain "equal everywhere" is simpler and what people expect. The cost is privacy (a copied password reaches every machine at once), hence `clipboard_sharing`, `--no-clipboard` and the README note.
- **Built:** `geserdesk-clipboard` (backend trait, `SystemClipboard` over `arboard` text only, `MemoryClipboard`, `ClipboardSync` change detection, `Inbound` reassembly), `ChunkAssembler::with_limit` (the declared `total` came from the peer and was passed straight to `reserve`; now at most 1 MiB is reserved up front and a limit is enforced as chunks arrive), server `clipboard` relay task plus `Hub::take_clipboard` / `broadcast_except`, `run_server_shared` (clipboard chosen by the caller; `run_server_with` picks the system one when capturing), client `ClientConfig.clipboard` and the options from the handshake's `SetOptions` (they used to be dropped), live `SetOptions`, `ClientOptions.clipboard`, `geserdesk client --no-clipboard`.
- **Decisions:** `seq` on clipboard messages is the sender's transfer number, not the `Enter` seq PROTOCOL.md used to promise (nothing orders ownership here); the wire format is unchanged, so no version bump and an older peer simply ignores the messages. `clipboard_max_bytes = 0` now means 16 MiB, not unlimited. Dry-run clients and `--no-capture` servers never open the desktop clipboard.
- **Verified:** unit tests (assembler limits, `Inbound`, change detection and echo suppression, hub broadcast, the relay) and three loopback tests with a real server and two real clients on in-memory clipboards: a copy on a client, on the server, and a 320 KB multi-byte text each reach everyone, every machine is written exactly once per foreign copy (no echo); `clipboard_sharing = false` keeps all three apart; a text over `clipboard_max_bytes` stays local and a smaller one still goes through. `fmt`, `clippy -D warnings`, `cargo test --workspace`.
- **Not verified:** `SystemClipboard` against a real X11 or Windows clipboard (it would overwrite the user's own clipboard; the code path is `arboard`'s `get_text`/`set_text`), clipboard managers and apps that take a moment to hand over selection data, and Windows at all. *(Session 9: the Windows clipboard is verified; X11 and clipboard managers are not.)*
- **Known gaps:** text only (no images, files or rich text; the X11 primary selection is skipped on purpose), no GUI switch for it yet (`ClientOptions.clipboard` exists on the wire), Wayland needs M13, and a client that was offline does not catch up on copies it missed.

### Session 9 — 2026-10-13 — the Windows 11 client, driven for real

The client was built (MSVC Build Tools, Rust 1.99, Node 24 with pnpm through corepack) and exercised on a Windows 11 laptop with two monitors (one above the other, desktop `0,-1080` to `1919,1079`). Input went into a test window of ours only, never into other windows (the harness aborted unless the test window held the foreground).

**Bugs found and fixed (all in `geserdesk-client`):**

- **Absolute moves reached only the primary monitor, and the last pixel column was unreachable.** `enigo` maps `MoveAbs` onto the primary display, so the monitor above it could not be reached, and the right-most column was silently dropped by Windows (an absolute value of 65502 or more is ignored, not clamped), so the cursor stopped short of that edge when moving fast. The client now sends `MOUSEEVENTF_VIRTUALDESK` moves with Microsoft's `ceil(px * 65536 / size)` mapping and finishes onto the right edge with a small relative move (`inject.rs`, module `win`). A 32-case simulation (all four edges, three corners, 3 to 300 steps, no pauses) lands exactly.
- **Capitals and shifted symbols typed nothing.** `enigo` 0.3.0 keeps the shift-state byte of `VkKeyScanExW` in the key code (`'A'` becomes `0x0141`), an invalid key. `os_key` now looks the character up itself and gives enigo the plain virtual key; the server's own Shift event supplies the shift. `A ! @ ? Q B` verified.
- **Characters with no key on the layout (`ñ`, CJK) were typed twice.** enigo sends such text as a whole press-and-release on every call, so key-down and key-up each typed it. They are now typed once, on press (`text_only`).
- **The Menu / Application key (between Right Alt and Right Ctrl) did nothing.** The Linux server already sent it (evdev 127 to HID `0x65`), but `named_key` in the client had no entry, so it was dropped with "no way to type this key". On Windows it now maps to `Key::Apps` (`VK_APPS`); checked as `KeyDown Apps` / `KeyUp Apps` in the test window. enigo has no such key for X11, so a Linux client still ignores it.
- **Keypad digits were typed as the digit row.** The server sends keypad keys as "printable" with the character they make, and the client typed that character (`VK_1`). The client now maps the keypad HIDs (`0x54..=0x57`, `0x59..=0x63`) to `VK_NUMPAD0..9`, `VK_ADD`, `VK_SUBTRACT`, `VK_MULTIPLY`, `VK_DIVIDE`, `VK_DECIMAL` on Windows. Checked as `NumPad1`, `NumPad2`, `NumPad3`, `NumPad0`, `Decimal`, `Add`, `Subtract`, `Multiply`, `Divide` and Enter, with NumLock off on that machine (the text still arrived). Not checked: how a keypad key behaves when the server's and the client's NumLock differ; `Enter { toggle_mods }` is not acted on yet.
- **Scroll Lock did nothing, in two ways.** The design (above, *Lock to screen*, and the `scroll_lock_locks` option) is implemented in `SwitchState` (`toggle_lock`, `evaluate`, tested), but nothing calls `toggle_lock`: `router.rs` forwards the key like any other and the client had no mapping for HID `0x47`. **Decision (with the user): Scroll Lock is an ordinary key.** On Windows the client now maps it to `VK_SCROLL`; one tap toggled Windows' own Scroll Lock state and a second tap restored it (read back with `GetKeyState`). The lock-to-screen design stays unbuilt: `scroll_lock_locks` and `SwitchState::toggle_lock` are inert, and `evaluate` is only reached from `on_pointer` (cursor on the server's own screen), so wiring it later would not hold the cursor on a client unless `on_motion` is changed too. A Linux client still ignores the key (no enigo mapping for X11).
- **A server that stops could make a `--no-reconnect` client fail.** The server aborts its sessions without a `Close`, and Windows ends a socket with an RST when data of ours is still in flight, which dropped the goodbye and surfaced as os error 10054. A reset, abort or broken pipe during a session now ends it like a close (`session_ended`). The same fix covers the new clipboard send.
- **`cargo clippy -D warnings` failed on Windows** (`Endpoint::socket_path` is only used by the Unix socket code, so it was dead code there). CI runs clippy on a Windows runner too, so that job would have been red; the method is now `#[cfg(unix)]`.
- **Two tests assumed a fast refusal.** Windows takes about 2 s to refuse a connection to a closed port, and a parallel test can take a freed port. Both now connect to `127.0.0.1:0`, which every OS rejects at once. `control.rs` ran 40 times without a failure (it failed about one run in six before).

**Verified on Windows, with real binaries (isolated runtime and identity directories where the user's own files could be touched):**

- Mouse: left, right, middle, Back and Forward (raw `WM_XBUTTONDOWN` 1 and 2 plus `WM_APPCOMMAND`), vertical wheel (+120 and -240) and relative moves; `leave()` released a Shift that was still held.
- Keyboard (US layout): letters, digits, space, Enter, Backspace, arrows, Home, End, Delete, PageDown, F5, Escape, `- = ; ' , . /`, Shift+letter, shifted symbols, Ctrl+A arriving as `VK_A` with Control, auto-repeat, and non-ASCII text.
- Network: TLS 1.3 pairing; a wrong `--server-fingerprint` is refused as a possible impostor and not retried; a right one waits for approval; approval by `fingerprint = ...` in the config lets it in; `--monitors` is honoured (the server sees the forced rectangles); a plaintext client against a TLS server is refused with a clear message on both sides; reconnect after the server is stopped and started again (back in 4 s).
- Daemon on Windows: `status` and `stop` over the named pipe (a server and a client daemon each in their own runtime directory), `stop` empties the runtime directory and frees the port, `status` then exits 1, and a second daemon in the same directory is refused.
- The desktop app: `geserdesk-desktop --start` in server mode spawns a detached daemon ("started by app"), `status` sees it, killing the app leaves the daemon running, `stop` ends it.
- Clipboard (the real Windows clipboard, through a real client, a server and a peer with in-memory clipboards; the user's own clipboard text saved and restored): Windows to server and peer in about 0.6 s with no echo; peer to Windows and server to Windows, text identical (accents, CJK, emoji, CRLF, tab); 300,000 characters (900 KB) in about 0.7 s; an empty clipboard and a file list on the clipboard are ignored without error; `--no-clipboard` stops both directions.
- Monitor detection equals `EnumDisplayMonitors` and the virtual-screen metrics. The process is DPI aware: the registry's `AppliedDPI` is 106 (about 110% scaling) and the client still reports the 1920x1080 real pixels the video controller reports.

**Found and not changed:**

- **`--no-capture` servers relay no clipboard at all.** The server only gets a clipboard backend when it captures (`net.rs`, `capture && clipboard_sharing`), and the relay task only exists with a backend, so two clients behind a `--no-capture` server do not share text. It follows from "never open the desktop clipboard" but may surprise anyone testing with `--no-capture`.
- **With TLS, a stopping server makes clients log a warning** (`peer closed connection without sending TLS close_notify`) because `abort_all()` drops the sessions without a `Close` or a TLS close. Harmless with reconnect; the proper fix is a graceful close on the server.
- The Windows key file gets no ACL of its own (see README *Security*).

**Not verified:**

- A real Linux server driving this client through cursor crossings, typing and the clipboard with an X11 clipboard (the connection itself was up; it needs a person at the Linux machine).
- Horizontal wheel: `enigo` sends `MOUSEEVENTF_HWHEEL` correctly, but no test window here received horizontal wheel messages (not even a plain Win32 call reached it), so it is untested rather than shown broken.
- Other keyboard layouts (cross-layout typing was only checked with US) and Tab in the test window (the form swallows it). The keys left out of the injection tests because they would act on the desktop (Alt, Windows, Caps/Num Lock, Print Screen, Pause, Insert, Page Up, F1 to F4, F6 to F12) were then pressed by the user on the Linux keyboard in front of a keyboard-test web page on this Windows machine: every key of the 104-key English layout lit up, the keypad block as keypad keys. That shows the events arrived, not what they typed, and the page may not tell left from right (Right Alt and Right Windows are sent as the left ones anyway).
- A monitor plugged or unplugged while the client runs (it would need switching the display topology, which was left alone).
- The window of the app on Windows (layout editor, tray, the trust dialog), the pipe's access rules, an upgrade across two versions.
- The unit tests for `os_key` depend on the focused window's layout, so they only assert what holds on every Latin layout.

### Session 10 — 2026-10-13 — clipboard style: HTML next to the text (protocol 3.0)

- **Why:** copying from a web page on Linux and pasting into Word on Windows lost the style, because M9 carried plain text only (a browser puts both `text/plain` and `text/html` on the clipboard; Windows to Windows keeps both). The user asked for this and said rebuilding both sides is fine, so the clean design was chosen over a compatible patch.
- **Design:** a transfer's bytes are now a postcard-encoded `ClipboardContent`, a list of `(mime, bytes)`; `text/plain` and `text/html` are shared, any other MIME type is carried and ignored, so an image or RTF can be added later without touching the wire. The message shapes are unchanged, but a 2.x peer would read the new bytes as text, so `PROTOCOL_VERSION` is **3.0** and a 2.x peer gets `Incompatible`. `Clipboard` is now `get() -> Option<ClipboardContent>` / `set(&ClipboardContent)`; `SystemClipboard` reads text and HTML (`arboard`'s `get().html()`, any error there meaning "no style") and writes both with `set().html(html, text)`.
- **Details that matter:** change detection hashes the whole content, so the same text copied again with another style is a new copy; after writing, the clipboard is read back and *that* is remembered, because a platform may return the HTML slightly changed (Windows wraps it as CF_HTML) and a mismatch would be mistaken for a new copy and ping-pong between machines. A copy over `clipboard_max_bytes` falls back to its plain text; the receiver allows a few KiB of encoding on top of the limit and checks the data length after decoding.
- **Verified:** unit tests (content encode/decode and unknown MIME types, the size fallback, `Inbound` with styled content, change detection with and without style); loopback tests with a real server and two clients where a styled page copied on the server reaches both clients with its HTML, one copied on a client reaches the server and the other client, and a plain copy afterwards leaves no old style behind; no echo (write counts). **On this X11 desktop with its real clipboard** (`cargo test -p geserdesk-clipboard -- --ignored`, which saves and restores the clipboard): HTML and text written, read back (HTML contains the markup), and a following plain-text write leaves no HTML.
- **Not verified:** HTML from a real browser through to Word on Windows (the Windows side needs the new build); how Firefox/Chrome offer HTML to `arboard` on X11 beyond the round trip above; large pages on slow links.
- **Known gaps:** images, RTF, files and the primary selection are still not shared; HTML is sent as the source application wrote it (links and hidden attributes included).

### Session 11 — 2026-10-13 — clipboard: a change signal and pictures (protocol 3.1)

- **Plan for the rest of the clipboard** (agreed with the user): screenshots and copied files follow the clipboard, not drag-and-drop. Pictures go at once like text. Files: `arboard` only (it cannot hand out files that do not exist yet), so a copy only announces names and sizes, and the bytes are pulled to one machine when the cursor enters it, with a small progress window; 256 MiB per copy. Nautilus may need `x-special/gnome-copied-files`, which `arboard` does not write; a spike decides whether a small X11 owner is added. Truly lazy paste (FUSE / OLE virtual files) was discussed and set aside.
- **Change signal:** `Clipboard::change_count`. Windows has `GetClipboardSequenceNumber`; on X11 one thread per process asks XFixes for `SelectionNotify` on `CLIPBOARD` and counts. While the count is unchanged the clipboard is not read, so a screenshot sitting on it is not decoded twice a second. Without a signal the old read-every-500-ms applies. The X11 thread lives as long as the process (one per process, not per session).
- **Pictures:** `arboard` with `image-data`, crate `image` (PNG only). RGBA from the OS becomes PNG with fast compression; a received PNG has its dimensions checked from the header (16384 per side, 64 megapixels) before it is decoded with `image::Limits`. A picture is only taken from a clipboard without text, because Office puts a bitmap of copied text next to it; when setting, text wins over a picture. `fit()` drops HTML first, then the picture. Protocol 3.1 (a 3.0 peer skips the item).
- **Verified:** unit tests (PNG round trip with alpha, a wrong byte count, not-a-PNG, a forged header with a valid CRC claiming 60000x60000, `fit` with pictures, the change signal reading only after a change); a loopback test where a 640x400 noisy screenshot (several chunks) copied on a client reaches the server and the other client byte for byte, and a later text copy replaces it. **On this X11 desktop's real clipboard** (`cargo test -p geserdesk-clipboard -- --ignored --test-threads=1`, saving and restoring the clipboard): XFixes reports a copy and nothing else; a picture set and read back has the same pixels.
- **Not verified:** Win+Shift+S and Shift+PrtSc through to the other machine (needs the new build on both sides); `GetClipboardSequenceNumber` on Windows.

### Session 12 — 2026-10-13 — clipboard files: pulled when and where they are needed (protocol 4.0, M10)

- **Design, changed from the session 11 plan during a spike:** files only ever announce a manifest (names, sizes,
  folder flag) through the clipboard, same pipe as text/HTML/pictures; the spike (offering a real folder via
  `arboard::set().file_list()` from this process, Ctrl+V in Nautilus, and the reverse) showed Nautilus accepts and
  offers a plain `text/uri-list` with no extra X11 code, so the planned fallback owner was dropped. One real bug
  found there: `arboard`'s X11 reader only splits `text/uri-list` on `\n`, so a path Nautilus wrote (`\r\n` endings)
  keeps a trailing `\r`; `SystemClipboard::file_list()` trims it. Bytes are pulled afterwards, by index into the
  manifest, through new `FileRead`/`FileReply` messages (replacing `FileOffer`/`FileChunk`, never implemented) --
  `PROTOCOL` is **4.0**. A consumer pulls only when it actually needs the files: a client when the cursor arrives on
  its screen, the server for its own screen when the cursor returns there. **Not built this session, unlike the
  approved plan:** the progress window (a small Tauri popup showing percent/speed/Cancel). Transfers run silently;
  this is a real gap for anything but a quick, small copy and should be the next thing added.
- **New:** `geserdesk-clipboard`: `collect` (walks a copy's paths into a manifest + their absolute paths,
  depth-first, symlinks left out, capped at 10 000 entries / 32 levels deep / a configurable byte total),
  `FileOwner` (serves reads of what *this* machine last announced, checking the file has not changed size and never
  following a symlink), `stage::sanitize_manifest` (resolves a peer's relative paths under a destination, refusing
  anything that could escape it, collide once case is ignored, or use a name Windows reserves), `pull` (drives a
  caller-supplied chunk reader against a manifest: creates folders in order, writes files sequentially, removes
  everything on any failure), `Clipboard::file_list`/`set_file_list` (while files are on the clipboard, `get()` now
  returns `None`: GTK and Windows both also put the path as plain text, which must not be shared as text).
  `geserdesk-server`: `FileRouter` (who owns files now, at what `seq`, with their manifest; routes a `FileRead` to
  the owner -- rewriting `req` so two askers cannot collide -- or answers directly if the server itself owns it;
  `spawn_local_pull` pulls for the server's own screen when `router::run_shared` sees the cursor return home while a
  client still owns files). `geserdesk-client`: `FileChannel` (a pull cannot write to the session's one
  `MessageStream` from its own task, so it queues what to send and is handed back the matching reply) and
  `ClientClipboard` learned to push its own files (gated so it never pushes a second time before the server's echo
  confirms the first `seq` -- the only way a client learns what `seq` its own push became), remember a remote
  announcement, and pull it on `ServerMsg::Enter`.
- **Decisions:** `clipboard_sharing` and `clipboard_files` are independent switches now (a server can share text
  without files, or the reverse); the inbound grab/data stream is reassembled whenever either is on, and only the
  decoded content's own kind decides whether it is relayed. A files announcement is echoed to **everyone including
  the pusher** (unlike text/HTML/pictures, which are never echoed back) -- the only way the pusher learns the `seq`
  the server assigned, since the server always reassigns its own `seq` when relaying. `clipboard_files_max_bytes`
  defaults to 256 MiB; a copy over it, or one whose `collect()` fails (too many entries, too deep), is simply never
  announced. `--no-clipboard-files` / `ClientOptions.clipboard_files` narrows what the server's own option allows,
  via a small `NoFiles` wrapper that keeps the `Clipboard` trait's default (never report, never accept a file list)
  while text/HTML/pictures still pass through.
- **Verified:** unit and component tests across all three crates (manifest collection with real folders including a
  symlink and the real `MAX_ENTRIES` limit; path sanitising against `../x`, `/etc/passwd`, `C:\x`, `con.txt`, a
  trailing dot/space, an overlong component, a case-insensitive duplicate; `pull` against an in-memory reader,
  including a byte-limit refusal, an unsafe manifest refusal, a short read, and cleanup on any failure -- a real bug
  found here: a `tokio::fs::File` write was not reliably visible to a plain `std::fs::read` without an explicit
  `flush`, which made one test flaky about half the time; `FileRouter` routing a request to a client owner and back
  with `req` rewritten, refusing a reply from the wrong connection, failing what was pending when the owner
  disconnects, and pulling for the server's own screen; both client-side directions driven through `session_loop`
  over a real duplex stream -- pulling a remote announcement into a real staging folder once a faked `Enter`
  arrives, and serving a `FileRead` for files pushed and echoed back with a seq). **On this X11 desktop's real
  clipboard:** the Nautilus spike (folder + subfolder pasted and read back whole) and `SystemClipboard::file_list`/
  `set_file_list` round-tripping, `get()` returning `None` while files are held. `fmt`, `clippy -D warnings`,
  `cargo test --workspace` throughout.
- **Not verified:** a real Windows client pushing or pulling files at all (needs the new build there); pasting a
  pulled folder into a real Explorer or Nautilus across the network (only the staging and clipboard-set steps were
  exercised, with a fake or in-memory peer standing in for the network); the progress window (not built); a true
  three-machine run (server + two real TCP clients) exercising a cursor-driven pull, since that needs real input
  capture, which was not exercised here on purpose.
- **Known gaps:** no progress indication during a pull -- a large copy looks like nothing is happening until it
  finishes or fails; a `--no-capture` server relays no files at all, same as it already did for text (no screen to
  return to, so nothing ever triggers a local pull, and nobody's `Enter` ever arrives either); pulling is strictly
  sequential (one chunk in flight at a time), not pipelined, so a slow link pulls more slowly than it could; no
  resuming a pull that failed partway, no images/RTF, and the primary selection is still not shared.

### Session 13 — 2026-10-13 — the progress window the session 12 plan deferred

- **What:** a small, frameless, always-on-top window now shows a files pull while it happens, closing the "no
  progress indication" gap session 12 left open. `geserdesk_clipboard::pull::pull_with_progress` already reported
  each chunk; it now also reports `Done`/`Failed` as a `PullEvent`, and every `PullReport` names which machine the
  files are coming from (`"server"` from a client's own view; the owning client's screen name from the server's,
  threaded through from `router::run_shared` where it already knew it). `spawn_pull` (client) and `spawn_local_pull`
  (server, now taking a `LocalPullJob` instead of eight positional arguments -- clippy's own limit) both accept an
  optional sink and drive it through a whole pull's lifecycle, not just its chunks.
- **New in `geserdesk-app`:** `Session.transfers: TransferHub`, a `broadcast` stream of `TransferProgress`
  (seq, peer, file, counts, byte totals, running/done/failed), fed whenever a client or a server session starts.
  `Event::Transfer` carries it to controllers; `PROTOCOL` moved to **3** (an older app would fail to decode a new
  tagged-enum variant it has never seen, so this needed the same version bump a wire-shape change always gets, not
  just an additive field).
- **New in the desktop app:** `transfer_window.rs` opens a second Tauri window (same bundle, `?popup=transfer`,
  no router added) near the screen's corner the first time a `Running` report arrives, and raises it again for each
  later pull; `TransferPopup.tsx` renders whatever report it is currently sent and hides itself a couple seconds
  after `Done`, longer after `Failed` (so the message stays readable) -- the Rust side never has to decide when to
  hide it, only when to show it again.
- **Verified:** new unit tests for `PullEvent`/`PullReport` end-to-end through `spawn_pull`/`spawn_local_pull`, and
  for `TransferHub` (a sink's own `seq` tracked independently of another sink's, a `Failed` with no `Progress` first
  still a valid report). `fmt`, `clippy -D warnings`, `cargo test --workspace`, the frontend's existing vitest suite,
  `tsc --noEmit`, `pnpm build`, all green -- all without a display, so none of this was watched actually opening,
  moving, or disappearing on screen.
- **Known gaps:** no Cancel button -- aborting a pull already in flight is not wired anywhere in this chain, so the
  window is informational only; its own close button (✕) only hides the window, the pull keeps running underneath.
  The popup's position, sizing against a real multi-monitor and multi-DPI setup, and its appearance in both themes
  are all unverified (`tauri dev` was not run). A transfer that never reports `Done`/`Failed` (a bug, or a pull that
  hangs rather than erroring) would leave the window open indefinitely with no timeout of its own.

### Session 14 — 2026-10-14 — sessions 10–13 checked on the Windows 11 machine

- **What:** after pulling sessions 10–13, everything that can run on this Windows 11 desktop alone was run. No Linux machine and no second machine were involved.
- **Fixed:** `geserdesk-clipboard` did not compile its tests on Windows (two tests in `files.rs` called `std::os::unix::fs::symlink`). A `make_symlink` helper now picks `symlink` or `symlink_file`; on Windows, creating a link needs Developer Mode or an elevated process, so the two symlink tests return early when the link cannot be made instead of failing. They therefore only exercise the symlink refusal on Linux, or on Windows when that right is present.
- **Verified here:** `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` (all green), and the frontend (`tsc --noEmit`, vitest 10/10, `pnpm build`). The four `--ignored` tests of `geserdesk-clipboard`, which use the **real Windows clipboard** (it is saved and restored): text with HTML, a picture, a file list (and `get()` returning `None` while files are held), and `GetClipboardSequenceNumber` (a copy changes the count and nothing else does). This closes the "not verified" items of sessions 10 and 11 for the clipboard API itself, though not for a browser, Word or Win+Shift+S.
- **The desktop app, run for real:** the debug build of `geserdesk-desktop` (with vite on port 1420) started, its window answered, and a screenshot showed the main window drawn correctly (Client/Server tabs, the form, this machine's fingerprint, the log pane). Start was not pressed, so no daemon was spawned and the popup was never raised; there is no way to trigger a pull from here without a second machine. The app and vite were stopped afterwards.
- **Not verified, needs more than this machine:** a Windows client pushing or pulling files against a real server; pasting a pulled folder into a real Explorer; the progress window actually shown on screen (the main window was seen, the popup was not); a screenshot through to another machine; HTML from a real browser into Word. Windows as the server (M7) is still not started.
### Session 15 — 2026-10-14 — M7: Windows as the server

- **What:** `geserdesk-platform` has a Windows capture backend (`windows.rs`) behind the same `start(events) -> CaptureHandle` and `CaptureCmd::{Grab, Ungrab}` the X11 one uses, so the server, router, config and desktop app needed no change. `windows-sys` only (no new dependency); `unsafe` is confined to that module, each block with its reason.
- **How it works:** one thread installs `WH_MOUSE_LL` and `WH_KEYBOARD_LL`, pumps messages (`PeekMessage` plus `MsgWaitForMultipleObjectsEx` with a 2 ms timeout, which is what services the hooks) and answers the server's commands. *Local* mode polls `GetCursorPos` like the X11 backend and reports `Pointer`; the hooks only watch for the hotkey. *Grabbed* mode swallows every mouse and keyboard event and reports it. The hook procedures only push onto the channel, because Windows skips a hook that takes a few hundred milliseconds.
- **Differences from X11, on purpose:** no warp-to-centre dance. A swallowed move never reaches the cursor, so with the cursor parked in the middle of the monitor it was on, `pt - centre` of each move event is the motion since the last one (the test checks the deltas stay one step each and do not pile up). A near-transparent topmost window (`WS_EX_LAYERED`, alpha 1, `WM_SETCURSOR` answers "no cursor", never activated) covers the desktop to hide the cursor, because a fully transparent layered window would be click-through. A button or key whose press came *before* the grab (a drag carried to the edge, the Ctrl and Alt of the hotkey) has its release passed to the desktop instead of swallowed, so nothing stays pressed locally; the made-up Left Ctrl Windows sends for AltGr (scan code 0x21D) is dropped.
- **Keys:** `keymap::win_key_to_hid(vk, scan, extended)` maps by scan code, which names the physical key under any layout (extended `E0` tells Home from keypad 7, Enter from KP Enter), with the virtual key deciding only Num Lock, Pause, Print Screen and F13-F20, where Windows reports the scan code inconsistently. A test checks every HID usage the evdev table can send has a Windows key that produces it. `ch` comes from `ToUnicodeEx` with the focused window's layout and flag 4 (does not disturb a dead key), Ctrl and Alt left out, a held Right Alt standing for AltGr with a retry without it.
- **A bug found by the real-hook test, worth remembering:** a swallowed key never updates Windows' own key state, so `GetAsyncKeyState(VK_CONTROL)` is false for the whole grab and the first version never saw `Ctrl+Alt+Home` (every other part of the test already passed). Held modifiers and the Caps/Num Lock toggles are now tracked from the hook's own events, and re-read from Windows between grabs (and at the start of each) to correct any release the hook never saw.
- **Verified here (Windows 11, this desktop):** `cargo test -p geserdesk-platform -- --ignored --test-threads=1` takes the real keyboard and mouse for about a second and drives them with `SendInput`: the local pointer is reported; grabbed motion arrives as bounded deltas and the cursor stays parked; Back button down and up, vertical and horizontal wheel, A with Down then Repeat(1) then Up and a letter as `ch`, Shift, End kept apart from keypad 1; the hotkey is reported once however long Home is held and its Home is not forwarded; `Ungrab` puts the cursor at the requested point and afterwards nothing is captured; a second capture can start after the first is dropped. **End to end**, the CLI built as is: a server with real capture (2 monitors, one above the other) and a `--dry-run` client over loopback, no TLS, each with its own `GESERDESK_RUNTIME_DIR` (a machine has one daemon per directory, so a server and a client on one machine need two): pushing the cursor to the right edge logged `switching to remote screen` and `grabbed`, the client logged `enter` and eight motion events, a key (`hid 4`, `ch 'a'`), the wheel, then Ctrl+Alt+Home logged `switching back` and `released`. `fmt`, `clippy -D warnings` and the workspace tests were run.
- **Not verified:** a second machine (a Windows server serving a Linux or Windows client over a real network, with TLS); the clipboard and files with a Windows server (clipboard sync is off in the end-to-end run, and a server that captures with an injecting client on the same desktop would capture its own input); typing from a non-US layout, dead keys, AltGr; an elevated foreground window or the secure desktop (Windows delivers neither to an unelevated hook; this was meant to be a documented limit, not fought); a laptop touchpad's smooth scrolling (deltas under 120); the desktop app driving a Windows server (it needs no change but was not run); a hook that Windows removed after a timeout.
- **Seen, not caused by this:** after `Ctrl+Alt+Home` the cursor comes back at the centre of the union of the monitors, which for two monitors stacked vertically is the seam between them (`router::local_center`; the X11 server does the same). Two flaky tests turned up in `cargo test --workspace` runs and were fixed (15 runs in a row green afterwards, from 6 of 8 failing):
  - `geserdesk-client`'s `a_pull_reports_its_progress_tagged_with_seq` (session 13) timed out waiting for the pull's first read. A test in `net.rs` clears `staging_dir(99)` whole, and this one's folder was `staging_dir(99)/<pid>`, inside it; run in parallel in one process (only the workspace build compiles those net tests in), it had its folder deleted mid-pull. It now has a seq of its own.
  - `geserdesk-app`'s `the_starting_config_is_valid_and_never_overwrites` failed now and then with `NoScreens`: it wrote `# mine` into `%TEMP%\geserdesk-app-<pid>` and never removed it, so a later run that got the same process id (Windows reuses them quickly) found that file and the default config, which never overwrites, was not written. It, and the same pattern in `geserdesk-app`'s trust test, now clear the folder first.

### Session 16 — 2026-10-14 — cancelling a pull, and a progress window that could hang or stay blank

- **Asked for:** a Cancel for a pull in flight, and a timeout so the progress window cannot stay open for ever on a pull that hangs (both listed as gaps in sessions 12-13).
- **Cancel:** `geserdesk_clipboard::pull` has `CancelSwitch`, a generation counter: `cancel()` ends every pull whose `watch()` was taken before it, and none started after (a Cancel button means "what is being pulled now", and cannot name a pull). The watch is taken when the pull is *started*, not when its task first runs, so a Cancel in between counts. `pull_with_options` (the old `pull_with_progress` is now a wrapper) selects on it with `biased` ahead of each chunk, so a cancel does not wait for a chunk that is still on its way; the staged folder is removed and the result is `PullError::Cancelled`, reported as the new `PullEvent::Cancelled` (not as a failure). `PullHooks` bundles the progress sink with the switch, so `spawn_pull`, `spawn_local_pull`, `ClientConfig` and `run_server_with_progress` carry one value instead of growing another argument (`transfer_progress` became `transfer_hooks`). `TransferHub` owns the switch (`cancel_all`, `pull_hooks`); the control channel has `Command::CancelTransfer`, and because that, and the new `TransferState::Cancelled`, are shapes an older app cannot read, `control::PROTOCOL` is **4**. The desktop app has `cancel_transfer` and the popup a **Batal** button. A cancelled read no longer leaves its entry in the client's pending table (`Forget`, dropped with the waiting call).
- **Timeout:** each chunk now has `READ_TIMEOUT` (30 s) in `pull_with_options`; one that does not arrive ends the pull as `Failed("the owner did not answer within 30 s")` with the folder removed, which is the root cause of a pull "hanging" (nothing ever answered the read). The popup also has a backstop for a report that never comes (the daemon gone): a running pull silent for 45 s (`STALL_AFTER_MS`, longer than the 30 s so the real answer wins) is shown as "Tidak ada kabar dari ..." and the window hides after 7 s; a later report raises it again. A window that never heard anything hides after 10 s.
- **Two bugs from session 13 found only by running the popup** (it had been built and tested without a display): (1) the capability file listed only the `main` window, so the `transfer` window had no permission to listen to events, hide itself, or call a command: it was a **blank black rectangle**. A control run with `capabilities/transfer.json` removed shows exactly that and the command never reached; with it, the progress shows. (2) The window is created when the first running report is sent, and its page is not loaded yet, so the first reports are lost; for a small copy that finished before the page loaded, the window would have been blank with no timer at all. `AppState` now keeps the latest report, the page asks for it on load (`latest_transfer`), and an event that arrives first wins over it.
- **Verified:** unit tests for the switch and the timeout in `geserdesk-clipboard` (a cancel mid-pull with a chunk that never comes, a cancel taken before the task ran, a pull started after a cancel not affected, a stalled chunk timing out with cleanup), for `spawn_pull` and `spawn_local_pull` (a cancel is reported once, the folder is gone, the clipboard untouched, no pending read left), for the hub (`Cancelled` is not a failure, `cancel_all` reaches pulls already running and not later ones, the JSON spelling the page reads) and for the command over the real control channel; vitest for the popup's timers and headings (21 tests); `tsc`, `pnpm build`, `fmt`, `clippy -D warnings`, and 6 `cargo test --workspace` runs in a row. **The real app on this Windows 11 desktop**, with a temporary harness (not kept) that emitted made-up reports through the same path `link.rs` uses: the popup shows `laporan.pdf (1/2)` with its bar growing, a click on **Batal** reached the command in 3 of 3 runs and the window showed "Dibatalkan"; a copy that finished before the page loaded showed "Selesai" (it used to be blank) and the window then went away on its own.
- **Not verified:** a real pull (needs a second machine) cancelled from the button, or timing out on a real stalled peer; the 45 s backstop (the unit of time was not waited out); the owner of the files is not told and keeps serving reads until its own end of the session. **Seen, not fixed:** the popup is placed from the monitor's size, not its work area, so on this screen (110 % scale) its lower part overlaps the taskbar.
### Session 17 — 2026-10-14 — where the cursor comes back to, where the popup sits, a third flaky test

- **Where input comes back (`router::local_center`):** it used the middle of the union of the local monitors, which for two monitors stacked vertically is the seam between them: after `Ctrl+Alt+Home` the cursor landed at `(960, 0)`, the top row of the lower monitor (the X11 server did the same). The router now remembers the local monitor the pointer was last on while the server had the input (`last_local_monitor`) and returns to the middle of that one; with no pointer seen yet, to the middle of the primary monitor (the one holding the origin), never the middle of the whole desktop. A client going away uses the same point. Tests: two stacked monitors with the pointer on the upper one, the lower one, moving from one to the other, no pointer seen, and a client disconnecting. **Checked on the real Windows backend**: the same loopback run as session 15 (server with real capture, two monitors one above the other, `--dry-run` client) now ends with the cursor at `(960, 540)`, where it was `(960, 0)`.
- **Where the popup sits (`transfer_window.rs`):** it was placed from the monitor's size, so its lower part overlapped the taskbar. It now uses the monitor's work area (`corner`, a pure function with three tests: the corner of the work area, scaled, and an area that does not start at the origin). Seen on screen with the same made-up-report harness as session 16 (not kept): the window now sits above the taskbar.
- **A third flaky test, found by the workspace runs of this session:** one run in about twenty failed two tests of `geserdesk-client`'s `tests/clipboard.rs` at once, and they were exactly a pair: the server that does *not* share the clipboard saw `"private"` arrive at a client (`left: Some("private")`), while the styled-page test, whose server *does* share, never got its text. That is the signature of two parallel tests being handed the same port (each test's clients connect to the other's server, and the one that lost the bind fails unseen, since `run_server_shared` runs in a spawned task). The rig and `control.rs` chose a port by binding port 0 and dropping the probe, and the server binds it a moment later. Both now take their port from `free_port()`, which never gives the same one twice in a process. **The cause is inferred from that signature; the collision itself was not reproduced** (rebinding port 0 after dropping it gave the same port 0 times in 2000 on this machine) and the flake is too rare to show the fix working: 10 workspace runs in a row passed afterwards, which proves little at that rate.
- **Verified:** the router tests above (35 in `router`), the three placement tests, `fmt`, `clippy -D warnings`, and 10 `cargo test --workspace` runs in a row.
### Session 18 — 2026-10-14 — what could be tested on this one machine and had not been

Everything on the "can be tried here, not yet tried" list of the previous sessions, one item at a time. Nothing needed a code fix; the only code change is that `windows::translate` (what a key types in a layout under given modifiers) was pulled out of `HookState::char_for` so a test can hand it any layout.

- **Smooth scrolling** (`smooth_scrolling_passes_through_in_small_and_negative_steps`, real hooks, `SendInput`): wheel steps of +30, -60 and -240 and a horizontal -30 arrive as `(0,30) (0,-60) (0,-240) (-30,0)`, unchanged. The client's rounding of a small one up to a notch was already tested (`small_wheel_deltas_still_scroll`).
- **Non-US layouts, AltGr, dead keys** (`a_key_types_what_its_layout_says_including_altgr_and_dead_keys`, 22 checks): `LoadKeyboardLayoutW` loads US, German, French and US-International into the test process, `MapVirtualKeyExW` gives the virtual key each layout assigns a scan code (what the hook would be handed), and the real `ToUnicodeEx` is asked. German: Z and Y swapped, `AltGr+Q` is `@`, `AltGr+E` is `€`, `AltGr+7` is `{`, `ü`, and the acute accent is a dead key (no character). French: A and Q swapped, the digit row needs Shift, `AltGr+0` is `@`, the circumflex is dead. US-International: `AltGr+A` is `á`, the apostrophe is dead. US: no AltGr level, so `AltGr+Q` falls back to `q`. This checks the translation, not typing it end to end into another machine with a non-US layout.
- **The desktop app driving a Windows server**: the app started with `--start` and saved settings for a server (settings file backed up and restored afterwards; the daemon's channel in a temporary folder; `daemon.log` in the config folder grew, as it does whenever the app starts a daemon). The window showed "Server berjalan", "Terhubung ke daemon (PID …, dimulai oleh aplikasi)" and the log streaming live: capture started with 2 monitors, a `--dry-run` client connected, `switching to remote screen`, `grabbed`, `switching back`. The client saw `enter`, six motion events, the key B (`ch 'b'`) and the hotkey's modifiers, then `leave`; the cursor came back to `(960, 540)`. `geserdesk stop` answered "Stopped." and the daemon was gone; no process or port was left.
- **The popup's 45-second backstop**: with a harness (not kept) that sends one running report and then nothing, the window showed "Menerima dari fake-peer" until the limit, then "Tidak ada kabar dari fake-peer" with an amber bar, and hid itself seven seconds later.
- **A hook that answers too slowly** (a throwaway experiment, not kept): a `WH_MOUSE_LL` hook that sleeps 2.5 s on its first call is **not removed** by Windows 11 here (`LowLevelHooksTimeout` is not set, so the default applies): it is not called during the stall and is called again afterwards as before. So a stalled capture thread would lose events for a moment but not stay deaf, and no watchdog that reinstalls hooks is needed for this.
- **Not done on purpose: a monitor plugged or unplugged while running.** Switching the display topology would rearrange this person's windows. What is covered is the logic, with made-up monitor lists (`monitor_watch_reports_only_real_changes`, `fresh_monitors_are_used_when_reconnecting`); the OS notification itself is still unobserved.