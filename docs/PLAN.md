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
│   ├── geserdesk-server/         # M5: screen layout, switch policy
│   └── geserdesk-client/         # M3: connection, receive events, inject
└── apps/
    ├── cli/                      # the `geserdesk` binary (server / client)
    └── gui/                      # M11: `geserdesk-gui` (eframe/egui + tray)
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
| **M11** | `egui` GUI + tray (`tray-icon`) — drag-and-drop grid for the layout, **no system Qt/GTK dependency** |
| **M12** | service/autostart (`windows-service`, systemd user unit) + packaging (`.deb`, InnoSetup installer) |

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
