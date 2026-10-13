<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# GeserDesk Wire Protocol

Version 3.1. This is a new protocol; it does **not** interoperate with Input
Leap / Barrier / Synergy.

Source of truth: `crates/geserdesk-proto`. This document describes the shape;
the crate defines the exact types.

## Transport

- TCP. Default port **24810**.
- `TCP_NODELAY` is set on both ends (input latency matters more than packing).
- **TLS 1.3, mutually authenticated** (milestone M8). Both sides present a
  self-signed ECDSA P-256 certificate; there are no certificate authorities.
  - The client pins the server's certificate **fingerprint** (SHA-256 of the
    DER certificate, written `SHA256:ab:cd:...`). With no pin yet it accepts the
    certificate at the TLS layer and must be confirmed by a person *before any
    application data is sent*.
  - The server requires a client certificate, proves nothing about *who* it is
    at the TLS layer, and decides after `Hello`: the fingerprint must be the one
    approved for the claimed screen name, otherwise it answers `Rejected` (an
    unknown certificate is recorded as pending approval).
  - Handshakes must complete within 10 s (TLS, then `Hello`).
  - `--no-tls` on both ends restores the cleartext framing below, for debugging.

## Framing

Every message is one length-prefixed frame:

```
+------------------+--------------------------------+
| u32 length (LE)  | payload (`length` bytes)       |
+------------------+--------------------------------+
```

- `length` is the payload size in bytes, little-endian.
- The payload is the message value serialized with
  [`postcard`](https://docs.rs/postcard) (a compact, varint-based serde format).
- A receiver rejects any frame whose declared `length` exceeds a configurable
  cap (default 4 MiB) **before allocating**, then drops the connection. This
  mirrors Input Leap's "messages of very large size" guard.
- A clean EOF exactly on a frame boundary is a normal close. An EOF partway
  through a frame is a protocol error.

## Types

| Type | Encoding |
|---|---|
| `Version` | `{ major: u16, minor: u16 }` |
| `Point` | `{ x: i32, y: i32 }` (absolute screen coordinates) |
| `Rect` | `{ x: i32, y: i32, w: i32, h: i32 }`, half-open |
| `ScreenInfo` | `{ name: String, bounds: Rect, monitors: Vec<Rect>, cursor: Point }` — `bounds` is the union of `monitors`, all in the machine's own desktop coordinates (primary at the origin; secondary monitors may be negative). A client re-sends it via `ScreenChanged` when its monitors change. |
| `ModMask` | `u32` bitmask: shift 0x1, control 0x2, alt 0x4, meta 0x8, super 0x10, altgr 0x20, capslock 0x1000, numlock 0x2000 |
| `KeyAction` | `Down` \| `Up` \| `Repeat(u16)` |
| `KeyEvent` | `{ hid: u16, ch: Option<char>, mods: ModMask, action: KeyAction }` |
| `Button` | `Left` \| `Middle` \| `Right` \| `Back` \| `Forward` \| `Extra(u8)` |
| `MouseEvent` | `MoveAbs { x, y }` \| `MoveRel { dx, dy }` \| `Button { button, down }` \| `Wheel { dx, dy }` (deltas in 120-per-notch units) |
| `ClipboardId` | `Clipboard` \| `Primary` (X11 middle-click selection) |
| `Chunk` | `Start { total: u64 }` \| `Data(Vec<u8>)` \| `End` — a well-formed stream is one `Start`, then any number of `Data`, then one `End`; 32 KiB per `Data` |
| `Options` | `{ clipboard_sharing: bool, clipboard_max_bytes: u64, scroll_lock_locks: bool, screensaver_sync: bool, relative_mouse_moves: bool }` |
| `CloseReason` | `ServerShutdown` \| `Timeout` \| `ProtocolError` \| `Reconfigured` |

### `hid` — physical key identity

`KeyEvent.hid` is the USB HID usage code (usage page 0x07). It is stable across
press and release even when the produced character is not (dead keys, mismatched
layouts). The client maps `hid` to a local key; if it cannot reach the same
symbol that way, it falls back to synthesising `ch`.

## Messages

### Client → server (`ClientMsg`)

| Variant | Meaning |
|---|---|
| `Hello { version, screen }` | First frame. Advertises protocol version and this machine's screen. |
| `ScreenChanged(ScreenInfo)` | Resolution / monitor arrangement changed. |
| `ClipboardGrab { id, seq }` | A local app changed clipboard `id`; its contents follow as `ClipboardData` with the same `seq` (see *Clipboard*). |
| `ClipboardData { id, seq, chunk }` | A chunk of the clipboard contents announced by the `ClipboardGrab` with this `seq`. |
| `FileOffer { name, size }` | Offer to send a file (drag-and-drop from the client). |
| `FileChunk(Chunk)` | A chunk of an in-progress file transfer. |
| `KeepAlive` | Reply to `ServerMsg::KeepAlive`. |

### Server → client (`ServerMsg`)

| Variant | Meaning |
|---|---|
| `Welcome { version, keepalive }` | Handshake accepted. `version` is the negotiated one; `keepalive` is the probe interval. |
| `Incompatible { server_version }` | Rejected: protocol majors differ. |
| `Rejected { reason }` | Rejected: screen name not in config, or already connected. |
| `Enter { x, y, seq, toggle_mods }` | The cursor entered this screen at `(x, y)` in the client's own coordinates. `seq` orders cross-screen messages; the client echoes it on clipboard messages. `toggle_mods` = lock keys that should be active on entry. |
| `Leave` | The cursor left. The client releases all held keys/buttons and sends clipboard data for any selection it grabbed. |
| `Key(KeyEvent)` | Synthesize a key event. |
| `Mouse(MouseEvent)` | Synthesize a mouse event. |
| `ClipboardGrab { id, seq }` | The server (or, relayed, another screen) changed clipboard `id`; its contents follow as `ClipboardData` with the same `seq`. |
| `ClipboardData { id, seq, chunk }` | A chunk of clipboard contents to apply locally. |
| `FileOffer { name, size }` / `FileChunk(Chunk)` | Incoming file transfer. |
| `SetOptions(Options)` | Push a new set of runtime options. |
| `Close(CloseReason)` | The server is closing the connection. |
| `KeepAlive` | Liveness probe; the client must reply with `ClientMsg::KeepAlive`. |

## Handshake

```
client                          server
  |  Hello { version, screen }    |
  | ----------------------------> |
  |                               |  negotiate version (same major, lower minor)
  |                               |  check screen.name is in config
  |                               |  check screen.name not already connected
  |     Welcome { version,        |
  |               keepalive }     |
  | <---------------------------- |
  |     SetOptions(Options)       |
  | <---------------------------- |
  |                               |
  |  ... session ...              |
```

Rejections (`Incompatible`, `Rejected`) are sent instead of `Welcome`, after
which the server closes the connection.

## Keep-alive

- The server sends `KeepAlive` every `keepalive` interval (default 3 s).
- The client replies with `KeepAlive` on receipt.
- After `KEEPALIVE_MISSES` (default 3) unanswered probes, the server sends
  `Close(Timeout)` and drops the client.
- If the client hears nothing at all for `3 × keepalive` (min 5 s), it assumes
  the link is dead and disconnects.

## Sequence numbers

`seq` in `Enter` increases each time the server hands control to a screen.
Clipboard messages do **not** echo it (see below); their `seq` is a transfer number.

## Clipboard

Sharing is push-on-change, not ownership handover, so no request message exists:

1. Whoever sees its clipboard change (a local application wrote it, not a peer)
   sends `ClipboardGrab { Clipboard, seq }`, then `ClipboardData { Clipboard, seq, chunk }`
   for `Start { total }`, `Data`*, `End` (`Chunk::stream`), with nothing else from that
   sender in between.
2. `seq` is chosen by the sender and increases per push. It ties chunks to their
   grab; a new `ClipboardGrab` discards a transfer still in progress, and chunks
   whose `seq` is not the current one are ignored.
3. The server writes a client's text onto its own clipboard and relays it, as its own
   transfer, to every other client (never back to the sender).
4. The bytes of that stream are a postcard-encoded `ClipboardContent`: a list of
   `(mime, data)` items. `text/plain`, `text/html` (UTF-8) and `image/png` are shared today; a
   receiver skips any other MIME type, so formats can be added without a new
   version. Plain text is what every program pastes; the HTML is what carries a
   copied web page's style into Word or a mail client. A receiver writes the
   formats it has and reads back only those.
5. A receiver refuses more than `clipboard_max_bytes` of data (0 means 16 MiB) as
   soon as `Start.total` or the bytes received exceed it (plus a few KiB of
   encoding), and drops a transfer that breaks the grammar or does not decode. A
   sender whose copy exceeds the limit sends the plain text alone when that fits,
   and nothing otherwise. The connection stays up either way.
6. `Options.clipboard_sharing = false` turns it off: the server ignores clipboard
   messages and sends none, and a client stops reading and pushing. A client takes
   the options from the handshake's `SetOptions` and from later ones.
7. Only `ClipboardId::Clipboard` is shared; `Primary` is ignored.

A peer that predates clipboard sharing logs and ignores these messages.

## Changes

- **3.1** — clipboard contents may carry `image/png` (a screenshot, a copied picture).
  A picture is only taken from a clipboard that holds no text, and a receiver
  checks its dimensions from the PNG header (at most 16384 per side and 64
  megapixels) before decoding. A 3.0 peer skips the item.

- **3.0** — a clipboard transfer carries a `ClipboardContent` (several formats)
  instead of bare UTF-8 text. The message shapes did not change, but a 2.x peer
  would read the new bytes as text, so this is a major bump: a 2.x peer is
  answered with `Incompatible`.

- **2.0** — `ScreenInfo` gained `monitors`. The wire format is positional
  (postcard), so this is a major bump: a 1.x peer is answered with
  `Incompatible` instead of misreading the handshake.
