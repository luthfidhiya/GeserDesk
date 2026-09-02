<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# GeserDesk Wire Protocol

Version 1.0. This is a new protocol; it does **not** interoperate with Input
Leap / Barrier / Synergy.

Source of truth: `crates/geserdesk-proto`. This document describes the shape;
the crate defines the exact types.

## Transport

- TCP. Default port **24810**.
- `TCP_NODELAY` is set on both ends (input latency matters more than packing).
- TLS is **not yet implemented** (milestone M8). Until then the connection is
  cleartext — use only on a trusted LAN.

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
| `ScreenInfo` | `{ name: String, bounds: Rect, cursor: Point }` |
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
| `ClipboardGrab { id, seq }` | A local app took ownership of clipboard `id`. |
| `ClipboardData { id, seq, chunk }` | A chunk of clipboard contents the server asked for. |
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
| `ClipboardGrab { id, seq }` | The server (or another screen) grabbed a clipboard. |
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

`seq` in `Enter` increases each time the server hands control to a screen. The
client stores the latest `seq` and echoes it in `ClipboardGrab` / `ClipboardData`
so the server can order clipboard ownership across screens (the same role as
Input Leap's `kMsgCEnter` sequence number).
