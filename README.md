<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# GeserDesk

A software KVM: use one keyboard and mouse to control several computers over the
local network. A from-scratch Rust reimplementation inspired by
[Input Leap](https://github.com/input-leap/input-leap) (a fork of
Barrier/Synergy), with a fresh, clean wire protocol.

**Targets:** Linux (X11) and Windows 11. macOS and Wayland are deferred.

**Status:** early. On Linux/X11 and on Windows the server captures the keyboard and
mouse and moves them to a client by pushing the cursor against a screen edge (or
`Ctrl+Alt+Home`). A Windows server was driven end to end against a client on the same
machine only (see M7 below); it has not yet served a second machine.

| Milestone | Status |
|---|---|
| M1 — protocol, codec, framing | ✅ done — 30 unit tests + 6 integration tests |
| M2 — TCP + tokio, handshake, keep-alive | ✅ done — verified over loopback and end-to-end |
| M3 — input injection via `enigo` (`--features inject`) | ✅ verified on Linux/X11 and on a Windows 11 client (mouse over two monitors, all buttons, vertical wheel, keys); horizontal wheel not yet observed |
| M4 — X11 capture + hotkey switching | ✅ done |
| M5 — edge crossing | ✅ done |
| M6 — full keyboard (HID, modifiers, auto-repeat) | ✅ done on Linux; typing into a Windows client checked with a US layout (capitals, shifted symbols, the keypad, Menu and Scroll Lock needed fixes there); every key of the 104-key English layout registered on a keyboard-test page; other layouts not tried |
| Multi-monitor detection + layout editor | ✅ done, detection confirmed on Windows (2 monitors, one above the other); a client re-reports its monitors within one 5-second poll when one is unplugged and plugged back (seen on Windows with a real cable); a server does the same for its own monitors (also seen with a real cable: its screen went from 2 monitors to 1 and back, and the router follows); on X11 the capture backend's own idea of the desktop size is still read once, which was not tried |
| M8 — TLS 1.3, certificate pinning, trust-on-first-use | ✅ done, confirmed Linux server <-> Windows client |
| Desktop app (Tauri + React) with tray | ✅ built; on Windows it starts, spawns its daemon detached, attaches over the named pipe and leaves the daemon running when the app dies; the window itself (layout editor, tray, dialogs) is still to be driven on Windows |
| M9 — clipboard sync (text, HTML, pictures) | ✅ built; the real Windows clipboard checked in both directions through a real client (Unicode, CRLF, 900 KB, no echo, `--no-clipboard`); the real X11 clipboard checked on this desktop (text, HTML, a picture, a file list, a change signal) and the real Windows clipboard too (the same five, including GetClipboardSequenceNumber) |
| M10 — clipboard files (names/sizes announced at once, bytes pulled on arrival) | ✅ built and unit/component tested (manifest collection, path sanitising, the pull itself, routing a request to whichever machine owns the files, pulling for the server's own screen); Nautilus accepts and offers a file list from `arboard` with no extra code; a small always-on-top window shows a pull's progress and can cancel it (seen running on Windows with made-up reports, since a real pull needs a second machine); the Windows build compiles and passes its tests and the Windows clipboard holds a file list, but it was not yet tried with a real Windows client pulling files, nor pasted into a real Explorer/Nautilus from across the network, nor the cancel button against a real pull |
| M7 — Windows capture (Windows as the server) | ✅ built: low-level keyboard and mouse hooks, a message pump on a dedicated thread, a transparent window that hides the cursor while grabbed. Tested with real hooks on Windows 11 (`SendInput` as the user: pointer, motion deltas, buttons, wheel, keys, auto-repeat, the hotkey, release) and end to end over loopback with a `--dry-run` client (edge push, hand-over, motion, a key, the wheel, `Ctrl+Alt+Home` back), both from the CLI and with the desktop app starting the server and the daemon. Smooth-scroll steps (quarter and half notches, either way) and the characters keys type on German, French and US-International layouts (AltGr, dead keys) were checked too. Not yet served a second machine, or run against an elevated window (Windows does not deliver those) |

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

`geserdesk status` and `geserdesk stop` talk to a running server (see *Daemon and
controllers*).

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

On Linux and on Windows the server captures this machine's keyboard and mouse. Push the cursor
against the edge linked to a connected client (see `[links]`), or press
**Ctrl+Alt+Home**, to send input to it; push back (or press the hotkey again) to
return. `--no-capture` runs the network side only; `--demo` makes the server
drive a connected client's cursor in a square without capturing anything.

> Don't run the capturing server and an *injecting* client on the same X
> display (or the same Windows desktop) — the injected input would be captured again.
> Use `--dry-run` there.

> On Windows the grab is Windows' own low-level hooks, so two limits are Windows':
> a server that is not elevated receives nothing meant for an elevated window, and
> the secure desktop (UAC prompt, lock screen, `Ctrl+Alt+Del`) cannot be seen or taken
> over. Run the server elevated if you need the first.

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

One window for everything: pick **Client** or **Server** at the top, fill in the
settings on *Konfigurasi*, **Start / Stop**, and watch the status line and the live
**log** (tick *Rinci* for debug lines; *Salin* copies the log).

The window is a **controller**: the client or server runs in a separate **daemon**
that the app starts (this same program with `--daemon`, no window) and attaches to.
Closing the window therefore stops nothing, and the app also attaches to a server
that was started from the CLI (`geserdesk server ...`) or before the window was
opened. See *Daemon and controllers* below.

* **Client:** enter the server address and this computer's screen name, press
  Start. The first time, a dialog shows the server's fingerprint with
  *Percayai & Sambungkan*; after that the server is remembered. The window also
  shows this computer's own fingerprint, which the server lists when it asks you to
  approve the computer.
* **Server:** pick the config file (the app can create a starting one), press
  Start. The *Pengaturan monitor* tab then arranges the screens and approves or revokes
  computers (a badge shows how many are waiting). A computer the config does not list
  yet just shows up there when it connects (with TLS); approving it adds it to the
  config and the desk, to the right of the others, with no restart and no editing of
  the file. Stop ends the session, and the
  daemon with it, giving the keyboard and mouse back.
* **Tray:** the icon shows whether GeserDesk is running (grey idle, green up, red
  failed) and its menu starts/stops, opens the window or the monitor tab, and quits.
  Closing the window hides it to the tray (switch this off with *Tutup jendela =
  sembunyikan ke tray*). *Keluar* closes only the app and leaves the session running;
  *Hentikan dan keluar* ends the session too. Without a tray (GNOME needs the
  *AppIndicator* extension) closing the window quits the app, still not the session.
* Settings are remembered in `gui.toml` next to the identity files.
* `--start` begins as soon as the app opens: `pnpm -C apps/desktop tauri dev -- -- --start`.
* `GESERDESK_NO_CAPTURE=1` keeps a server started from the app from capturing this
  machine's keyboard and mouse (for trying the app out).

## Daemon and controllers

```
 app (window, tray) ─┐
 geserdesk status    ├── local socket + token ──►  daemon: owns the session
 geserdesk stop      ┘     (JSON, one line each)     (network, TLS, capture, injection)
```

* `geserdesk server --config ...` and `geserdesk client ...` **are** daemons: the same
  server and client as always, plus a control channel. There is no way to run either
  without it, so a session started from a terminal can always be attached to by the
  app and seen by `geserdesk status`. If the channel cannot be opened the command
  stops and says why (`GESERDESK_RUNTIME_DIR` moves it).
* The app starts a daemon itself when you press Start and none is running. It lives
  as long as its session: Stop (or a failure) ends it. A daemon nobody gives work to
  leaves after 30 s.
* Only one daemon per user. Press Start in the app while `geserdesk server` is
  running and the app attaches to it instead of fighting for port 24810.
* `geserdesk status` shows the daemon, its session and the screens (exit code 1 when
  there is none); `geserdesk stop` ends it.
* Ctrl-C and SIGTERM stop the session first, so the keyboard and mouse are given back.
* The channel is a Unix socket in `$XDG_RUNTIME_DIR/geserdesk/` (0600, directory 0700)
  or a named pipe on Windows, plus a random token in `control.token` that only you can
  read; a controller must present it. `GESERDESK_RUNTIME_DIR` moves the directory
  (isolated runs, tests). A socket path longer than about 100 bytes is refused.
* A controller of another protocol version may only read `Info` and send `Shutdown`,
  so an old daemon can always be ended after an upgrade; the app offers a button.
* A client started from the terminal still asks `Trust this server? [y/N]` there (or
  takes `--server-fingerprint`); the app attached to it is not asked as well, so a
  question never has two answerers.
* The app's daemon writes its output to `daemon.log` next to the identity files.

## Clipboard

Plain text, HTML and pictures are kept equal on the server and every client:
whoever copies last wins, within about half a second, with no screen switch
needed. The HTML is what carries a copied web page's style into Word or a mail
client; programs that only take text get the text. A screenshot (Win+Shift+S,
Shift+PrtSc) or a copied image travels as PNG, as long as nothing on the
clipboard is text (Office puts a picture of copied text next to it, which is not
sent). The X11 middle-click selection is not shared. A client connecting later
is not sent what was copied before it came; its own clipboard is left alone
until the next copy.

* Off for everyone: `clipboard_sharing = false` under `[options]` in the server
  config. Off for one machine: `geserdesk client --no-clipboard`.
* `clipboard_max_bytes` under `[options]` caps one copy (0 = the built-in ceiling,
  16 MiB). Over it the HTML is dropped first, then the picture; a copy whose text
  alone is too large stays where it was made.
* The server shares its desktop's clipboard only while it captures that desktop's
  input (so not with `--no-capture`), and a client only when it injects real input
  (so not with `--dry-run`): trial runs never rewrite your clipboard.
* A copy is sent in clear text to every connected machine, so a password copied
  from a password manager reaches them all. Turn sharing off if that is not what
  you want.

### Files

A right-click **Copy** of a file or folder (not drag-and-drop) follows the clipboard
too: the names and sizes go to every machine at once, but the bytes are only
pulled to whichever one the cursor actually arrives on, into a temporary folder,
and only then does **Paste** there find anything. Pasting on the machine that
copied them needs nothing pulled; it already has them. Nothing is pulled to a
machine the cursor never visits. The desktop app shows a small always-on-top
window while a pull is under way, with a **Batal** (cancel) button: cancelling
stops the pull, removes what it had staged and leaves the clipboard as it was.
The window's own ✕ only hides it; the pull keeps going. A chunk that does not
arrive within 30 seconds ends the pull as failed, and the window hides itself a
few seconds after a pull ends (and if it never hears of one).

* Off for everyone: `clipboard_files = false` under `[options]`. Off for one
  machine (it can still share text/HTML/pictures): `geserdesk client
  --no-clipboard-files`.
* `clipboard_files_max_bytes` under `[options]` caps one copy's total size
  (0 = the built-in ceiling, 256 MiB); a copy over it is never offered.
* Every path is checked before anything is written: nothing may escape the
  destination folder, collide with another entry once case is ignored, or use a
  name Windows reserves (`CON`, `NUL`, `COM1`, ...), whichever OS copied it or is
  pasting it.
* Nothing is ever read from, or written to, a symlink.

## Special keys (volume, brightness, media...)

The volume, mute, media, brightness and microphone keys, and the launcher and browser keys of
a laptop's top row, travel as named actions: `mute`, `volume_up`, `volume_down`,
`mic_mute`, `play_pause`, `next_track`, `previous_track`, `stop`, `brightness_up`,
`brightness_down`, `mail`, `calculator`, `explorer`, `browser`, `browser_search`,
`browser_home`, `browser_back`, `browser_forward`, `browser_stop`, `browser_refresh`
and `browser_favorites`. What each machine does with them is its own, kept in `keys.toml`
in the user's config folder (next to `gui.toml`: `%APPDATA%\geserdesk\config` on Windows,
`~/.config/geserdesk` on Linux). The file belongs to the machine and is read whichever
part it plays, so a computer that was a client and becomes the server keeps its setup, and
no two machines have to agree on anything but the names above. It only needs what differs
from the built-in setup; a line that cannot be understood is skipped with a warning in the
log.

```toml
# What this machine's keyboard means (read when it is the server). For the keys a laptop
# does not show to programs (on a Lenovo, brightness and microphone mute usually are not
# shown), use a combination of ordinary keys.
[capture]
"ctrl+alt+f5" = "brightness_down"
"ctrl+alt+f6" = "brightness_up"
"ctrl+alt+f4" = "mic_mute"

# How this machine does an action it is sent (read when it is a client): `ignore`, a
# keysym on Linux (`keysym:0x1008ff02`), a virtual key on Windows (`vk:0xB7`), or
# `action:name` to do another action.
[perform]
brightness_up = "ignore"
mic_mute = "keysym:0x1008ffb2"
```

A combination fires only while the cursor is on a client, and the client gets the action
alone: the combination's modifiers are let go on it first. Modifiers are `ctrl`, `alt`,
`altgr`, `shift` and `win`; keys are letters, digits, `f1` to `f24`, the arrows and
the navigation keys, or `hid:0x..`. A client can press brightness and microphone mute only
on Linux: Windows has no key for them.

## Security

Every machine has a self-signed certificate (created on first run, kept in
`~/.config/geserdesk/` or `%APPDATA%\geserdesk\config`). On Linux the key file is
mode 0600. On Windows GeserDesk sets no permissions of its own: the file inherits
those of your profile folder (you, `SYSTEM` and the Administrators). Nobody is trusted until a person says so, and trust is a certificate
**fingerprint**, which you compare by eye:

* **Client -> server.** On first contact the client prints the server's
  fingerprint and asks `Trust this server? [y/N]`; compare it with the line
  `server certificate ... SHA256:...` in the server log (or the *Pengaturan monitor* tab).
  After that the fingerprint is pinned in `known_servers.toml`, and a different
  certificate is refused outright (never retried). For unattended setups pass
  `--server-fingerprint SHA256:...`.
* **Server -> client.** A new client is refused and shown in the app's *Permintaan
  sambung* panel with its fingerprint (the client prints the same one at startup); the
  requests also wait under *Menunggu persetujuan* in *Alat > Daftar client*. Check they match and click *Percayai*; the fingerprint
  is saved as `fingerprint = "..."` on that `[[screens]]` entry, and the client
  gets in on its next automatic retry. A name the config does not list yet is treated
  the same way, marked as a new computer: *Percayai dan tambahkan* writes a new
  [[screens]] entry with the fingerprint and puts it on the desk. (Only with TLS: with
  --no-tls there is nothing to approve it by, so a name must already be listed. A name
  that is not plain text -- empty, over 64 characters, with control characters -- is
  refused outright and never listed.) A trusted name cannot be claimed by another
  certificate, and a changed certificate is flagged as a replacement.
* **Taking a client away.** *Cabut kepercayaan* (in *Daftar client*, which reads the config
  and so works with no server running) removes the fingerprint and ends that client's
  session at once; it may knock again and then waits for approval like a stranger. The dot
  on a screen's card in *Pengaturan monitor* turns that screen off instead, keeping its
  trust and its place on the desk: it is written as `disabled = true` on the `[[screens]]`
  entry. The client stays connected, but the cursor cannot go to it and it neither gets nor
  sends clipboard contents, until the dot is clicked again.
* Not covered: malware on either machine, and the first contact if you do not
  actually compare the fingerprints.

## Layout

```
crates/geserdesk-proto     message types, postcard codec, length-prefixed framing
crates/geserdesk-tls       TLS identities, certificate pinning, fingerprints
crates/geserdesk-platform  input capture: X11 and Windows backends, evdev and scan code->HID key tables
crates/geserdesk-server    TOML config, layout + switch policy, client hub, router, listener,
                           admin (what a front end reads and changes while it runs)
crates/geserdesk-client    connection + handshake, the InputSink trait + enigo backend
crates/geserdesk-app       application layer, no GUI toolkit: run client/server, status,
                           settings, log, the "trust this server?" question, and the daemon
                           with its control channel (control/, daemon.rs)
apps/cli                   the `geserdesk` binary (server / client / status / stop)
apps/desktop               the app: React + Tailwind + shadcn/ui page (src/) over a thin
                           Tauri shell (src-tauri/: commands, events, tray)
```

Dependencies only point down the list: the core crates know nothing about windows,
`geserdesk-app` knows nothing about Tauri, and the Tauri shell only carries calls
and events between the page and the daemon (through `geserdesk-app`'s `Remote`).

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

Algorithms and design ideas were studied from Input Leap (which is
GPL-2.0-**only**) and then **reimplemented**, not copied: GPL-2.0-only code is
not compatible with GPL-3.0, so no code is transliterated from it. See
[`docs/PLAN.md`](docs/PLAN.md) for the details of that boundary.
