<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# GeserDesk

A software KVM: use one keyboard and mouse to control several computers over the
local network. A from-scratch Rust reimplementation inspired by
[Input Leap](https://github.com/input-leap/input-leap) (a fork of
Barrier/Synergy), with a fresh, clean wire protocol.

**Targets:** Linux (X11) and Windows 11. macOS and Wayland are deferred.

**Status:** early. On Linux/X11 the server captures the keyboard and mouse and
moves them to a client by pushing the cursor against a screen edge (or
`Ctrl+Alt+Home`). Windows is client-only so far.

| Milestone | Status |
|---|---|
| M1 — protocol, codec, framing | ✅ done — 30 unit tests + 6 integration tests |
| M2 — TCP + tokio, handshake, keep-alive | ✅ done — verified over loopback and end-to-end |
| M3 — input injection via `enigo` (`--features inject`) | ✅ verified on Linux/X11 |
| M4 — X11 capture + hotkey switching | ✅ done |
| M5 — edge crossing | ✅ done |
| M6 — full keyboard (HID, modifiers, auto-repeat) | ✅ done on Linux |
| Multi-monitor detection + browser layout editor | ✅ done (Windows detection not yet run on real hardware) |
| M7+ — Windows capture, TLS, clipboard, files, native GUI | ⬜ not started |

The full plan is in [`docs/PLAN.md`](docs/PLAN.md); the wire protocol is described
in [`docs/PROTOCOL.md`](docs/PROTOCOL.md).

## Build

Requires Rust ≥ 1.75. On Linux the `inject` feature (M3) needs `libxdo-dev`.

```bash
cargo build --workspace          # everything except real injection
cargo test  --workspace          # no devices or GUI needed
cargo clippy --workspace --all-targets
cargo fmt --all --check

# CLI with real input injection (M3):
cargo build -p geserdesk-cli --features inject
```

## Try it (loopback, one machine)

```bash
# Terminal 1
cargo run -p geserdesk-cli -- server --config dev-config.toml -v

# Terminal 2
cargo run -p geserdesk-cli -- client --name windows-pc --server 127.0.0.1:24810 --dry-run -v
```

A client reports its monitors automatically; `--monitors 1920x1080+0+0,1280x1024+1920+0`
overrides detection (VMs, testing).

`--dry-run` logs input events instead of injecting them (useful without a
graphical session).

**Arranging screens:** with the server running, open <http://127.0.0.1:24811/> in a
browser. Every machine appears with its real monitors (clients report them when
they connect); drag them where they sit on your desk, offsets included. Edges
snap together, and *Save* applies the layout immediately and writes `x`/`y` into
the config file. The page only listens on loopback (`[server] ui = ""` or
`--no-ui` turns it off). Old `[links]` configs still work.

On Linux the server captures this machine's keyboard and mouse. Push the cursor
against the edge linked to a connected client (see `[links]`), or press
**Ctrl+Alt+Home**, to send input to it; push back (or press the hotkey again) to
return. `--no-capture` runs the network side only; `--demo` makes the server
drive a connected client's cursor in a square without capturing anything.

> Don't run the capturing server and an *injecting* client on the same X
> display — the injected input would be captured again. Use `--dry-run` there.

## Try it (Linux ↔ Windows, two machines)

1. Edit `dev-config.toml`: screen names and the `[links]` arrangement.
2. On the Linux box (server):
   `cargo run -p geserdesk-cli -- server --config dev-config.toml -v`
3. On the Windows box (client):
   `cargo run -p geserdesk-cli --features inject -- client --name windows-pc --server <LINUX-IP>:24810 -v`

> ⚠️ The transport is still **plain, unencrypted TCP**. TLS arrives in M8. Until
> then, only use this on a network you trust.

## Layout

```
crates/geserdesk-proto     message types, postcard codec, length-prefixed framing
crates/geserdesk-platform  input capture: X11 backend, evdev->HID key table
crates/geserdesk-server    TOML config, layout + switch policy, client hub, router, listener
crates/geserdesk-client    connection + handshake, the InputSink trait + enigo backend
apps/cli                   the `geserdesk` binary (server / client subcommands)
```

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

Algorithms and design ideas were studied from Input Leap (which is
GPL-2.0-**only**) and then **reimplemented**, not copied: GPL-2.0-only code is
not compatible with GPL-3.0, so no code is transliterated from it. See
[`docs/PLAN.md`](docs/PLAN.md) for the details of that boundary.
