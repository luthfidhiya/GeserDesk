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
| Multi-monitor detection + layout editor | ✅ done, detection confirmed on Windows (2 monitors) |
| M8 — TLS 1.3, certificate pinning, trust-on-first-use | ✅ done, confirmed Linux server <-> Windows client |
| Desktop app (Tauri + React) with tray | ✅ built; start/stop, layout editor and tray still to be driven end to end on a real desktop |
| M7, M9+ — Windows capture, clipboard, files | ⬜ not started |

The full plan is in [`docs/PLAN.md`](docs/PLAN.md); the wire protocol is described
in [`docs/PROTOCOL.md`](docs/PROTOCOL.md).

## Build

Requires Rust ≥ 1.85 (the desktop app; the other crates need ≥ 1.75), Node 22 and
pnpm for the desktop app. On Linux the `inject` feature (M3) needs `libxdo-dev`
and the desktop app needs the Tauri system packages:

```bash
sudo apt install libxdo-dev libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev \
    libayatana-appindicator3-dev patchelf
```

```bash
pnpm -C apps/desktop install && pnpm -C apps/desktop build   # the page the Tauri crate embeds
cargo build --workspace          # everything except real injection
cargo test  --workspace          # no devices or GUI needed
cargo clippy --workspace --all-targets
cargo fmt --all --check
pnpm -C apps/desktop test        # the screen-arrangement geometry

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

A client reconnects by itself when the link drops or the server restarts (retrying
after 1 s, 2 s, 4 s ... up to 10 s); `--no-reconnect` exits instead. Only a
protocol-version mismatch stops it.

A client reports its monitors automatically; `--monitors 1920x1080+0+0,1280x1024+1920+0`
overrides detection (VMs, testing).

`--dry-run` logs input events instead of injecting them (useful without a
graphical session).

**Arranging screens:** run the desktop app (see *The app* below), start the server,
and open the *Pengaturan monitor* tab. Every machine appears with its real monitors
(clients report them when they connect); drag them where they sit on your desk,
offsets included. Edges snap together, and *Simpan & terapkan* applies the layout
immediately and writes `x`/`y` into the config file. The CLI has no layout editor:
edit `x`/`y` in the config by hand there. Old `[links]` configs still work, and a
leftover `[server] ui = ...` key is ignored with a warning.

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

> The connection is **TLS 1.3 with mutual authentication** (see *Security* below).
> `--no-tls` on both sides turns it off for debugging; never do that on a real network.

## The app (`geserdesk-desktop`)

```bash
pnpm -C apps/desktop tauri dev          # run it
pnpm -C apps/desktop tauri build        # installable package
```

One window for everything: pick **Client** or **Server** on the *Kontrol* tab, fill
in the settings, **Start / Stop**, and watch the status line and the live **log**
(tick *Rinci* for debug lines; *Salin* copies the log).

* **Client:** enter the server address and this computer's screen name, press
  Start. The first time, a dialog shows the server's fingerprint with
  *Percayai & Sambungkan*; after that the server is remembered. The window also
  shows this computer's own fingerprint, which the server lists when it asks you to
  approve the computer.
* **Server:** pick the config file (the app can create a starting one), press
  Start. The *Pengaturan monitor* tab then arranges the screens and approves or revokes
  computers (a badge shows how many are waiting). Stop (or quitting) closes every
  session and gives the keyboard and mouse back.
* **Tray:** the icon shows whether GeserDesk is running (grey idle, green up, red
  failed) and its menu starts/stops, opens the window or the layout tab, and quits.
  Closing the window hides it to the tray; *Keluar* in the tray menu really quits
  (switch this off with *Tutup jendela = sembunyikan ke tray*). Without a tray
  (GNOME needs the *AppIndicator* extension) closing the window quits instead.
* Settings are remembered in `gui.toml` next to the identity files.
* `--start` begins as soon as the app opens: `pnpm -C apps/desktop tauri dev -- -- --start`.

## Security

Every machine has a self-signed certificate (created on first run, kept in
`~/.config/geserdesk/` or `%APPDATA%\geserdesk\config`, key file readable only by
you). Nobody is trusted until a person says so, and trust is a certificate
**fingerprint**, which you compare by eye:

* **Client -> server.** On first contact the client prints the server's
  fingerprint and asks `Trust this server? [y/N]`; compare it with the line
  `server certificate ... SHA256:...` in the server log (or the *Pengaturan monitor* tab).
  After that the fingerprint is pinned in `known_servers.toml`, and a different
  certificate is refused outright (never retried). For unattended setups pass
  `--server-fingerprint SHA256:...`.
* **Server -> client.** A new client is refused and listed under *Menunggu
  persetujuan* in the app's *Pengaturan monitor* tab with its fingerprint (the client prints the
  same one at startup). Check they match and click *Percayai*; the fingerprint
  is saved as `fingerprint = "..."` on that `[[screens]]` entry, and the client
  gets in on its next automatic retry. A trusted name cannot be claimed by another
  certificate, and a changed certificate is flagged as a replacement.
* Not covered: malware on either machine, and the first contact if you do not
  actually compare the fingerprints.

## Layout

```
crates/geserdesk-proto     message types, postcard codec, length-prefixed framing
crates/geserdesk-tls       TLS identities, certificate pinning, fingerprints
crates/geserdesk-platform  input capture: X11 backend, evdev->HID key table
crates/geserdesk-server    TOML config, layout + switch policy, client hub, router, listener,
                           admin (what a front end reads and changes while it runs)
crates/geserdesk-client    connection + handshake, the InputSink trait + enigo backend
crates/geserdesk-app       application layer, no GUI toolkit: run client/server, status,
                           settings, log, the "trust this server?" question
apps/cli                   the `geserdesk` binary (server / client subcommands)
apps/desktop               the app: React + Tailwind + shadcn/ui page (src/) over a thin
                           Tauri shell (src-tauri/: commands, events, tray)
```

Dependencies only point down the list: the core crates know nothing about windows,
`geserdesk-app` knows nothing about Tauri, and the Tauri shell only carries calls
and events between the page and `geserdesk-app`.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

Algorithms and design ideas were studied from Input Leap (which is
GPL-2.0-**only**) and then **reimplemented**, not copied: GPL-2.0-only code is
not compatible with GPL-3.0, so no code is transliterated from it. See
[`docs/PLAN.md`](docs/PLAN.md) for the details of that boundary.
