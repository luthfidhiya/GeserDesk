<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# GeserDesk

A software KVM: use one keyboard and mouse to control several computers over the
local network. A from-scratch Rust reimplementation inspired by
[Input Leap](https://github.com/input-leap/input-leap) (a fork of
Barrier/Synergy), with a fresh, clean wire protocol.

**Targets:** Linux (X11) and Windows 11. macOS and Wayland are deferred.

**Status:** early. Milestones M1 and M2 are complete and tested; M3 (input
injection) is written but not yet exercised on real hardware.

| Milestone | Status |
|---|---|
| M1 — protocol, codec, framing | ✅ done — 30 unit tests + 6 integration tests |
| M2 — TCP + tokio, handshake, keep-alive | ✅ done — verified over loopback and end-to-end |
| M3 — input injection via `enigo` | 🚧 code exists (`--features inject`), not yet tested |
| M4+ — X11 capture, edge crossing, keyboard, … | ⬜ not started |

The full plan is in [`docs/PLAN.md`](docs/PLAN.md); the wire protocol is described
in [`docs/PROTOCOL.md`](docs/PROTOCOL.md).

## Build

Requires Rust ≥ 1.75. On Linux the `inject` feature (M3) needs `libxdo-dev`.

```bash
cargo build --workspace          # everything except real injection
cargo test  --workspace          # 57 tests, no devices or GUI needed
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

`--dry-run` logs input events instead of injecting them (useful without a
graphical session).

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
crates/geserdesk-server    TOML config, screen layout + switch policy (pure functions), listener
crates/geserdesk-client    connection + handshake, the InputSink trait + enigo backend
apps/cli                   the `geserdesk` binary (server / client subcommands)
```

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

Algorithms and design ideas were studied from Input Leap (which is
GPL-2.0-**only**) and then **reimplemented**, not copied: GPL-2.0-only code is
not compatible with GPL-3.0, so no code is transliterated from it. See
[`docs/PLAN.md`](docs/PLAN.md) for the details of that boundary.
