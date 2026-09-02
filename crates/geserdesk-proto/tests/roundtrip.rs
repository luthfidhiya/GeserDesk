// SPDX-License-Identifier: GPL-3.0-or-later
//! Every message variant must survive an encode/decode round-trip over a framed
//! stream, and a lightly-fuzzed byte stream must never panic the decoder.

use std::time::Duration;

use geserdesk_proto::clipboard::ClipboardId;
use geserdesk_proto::{
    Button, Chunk, ClientMsg, CloseReason, Edge, KeyAction, KeyEvent, MessageStream, ModMask,
    MouseEvent, Options, Point, Rect, ScreenInfo, ServerMsg, Version,
};

fn screen() -> ScreenInfo {
    ScreenInfo {
        name: "screen-with-a-fairly-long-name".into(),
        bounds: Rect::new(-1920, 0, 1920, 1080),
        cursor: Point::new(-5, 1079),
    }
}

fn all_client_msgs() -> Vec<ClientMsg> {
    vec![
        ClientMsg::Hello {
            version: Version::new(1, 0),
            screen: screen(),
        },
        ClientMsg::ScreenChanged(screen()),
        ClientMsg::ClipboardGrab {
            id: ClipboardId::Clipboard,
            seq: 42,
        },
        ClientMsg::ClipboardData {
            id: ClipboardId::Primary,
            seq: 7,
            chunk: Chunk::Data(vec![1, 2, 3, 4, 5]),
        },
        ClientMsg::FileOffer {
            name: "notes.txt".into(),
            size: 12_345,
        },
        ClientMsg::FileChunk(Chunk::Start { total: 999 }),
        ClientMsg::FileChunk(Chunk::End),
        ClientMsg::KeepAlive,
    ]
}

fn all_server_msgs() -> Vec<ServerMsg> {
    vec![
        ServerMsg::Welcome {
            version: Version::new(1, 0),
            keepalive: Duration::from_millis(2500),
        },
        ServerMsg::Incompatible {
            server_version: Version::new(1, 0),
        },
        ServerMsg::Rejected {
            reason: "screen name not in config".into(),
        },
        ServerMsg::Enter {
            x: -1,
            y: 2_000_000,
            seq: u64::MAX,
            toggle_mods: ModMask::CAPS_LOCK | ModMask::NUM_LOCK,
        },
        ServerMsg::Leave,
        ServerMsg::Key(KeyEvent {
            hid: 0x1D,
            ch: Some('Z'),
            mods: ModMask::SHIFT | ModMask::CONTROL,
            action: KeyAction::Repeat(3),
        }),
        ServerMsg::Key(KeyEvent {
            hid: 0xE1,
            ch: None,
            mods: ModMask::empty(),
            action: KeyAction::Up,
        }),
        ServerMsg::Mouse(MouseEvent::MoveAbs {
            x: i32::MIN,
            y: i32::MAX,
        }),
        ServerMsg::Mouse(MouseEvent::MoveRel { dx: -3, dy: 4 }),
        ServerMsg::Mouse(MouseEvent::Button {
            button: Button::Extra(7),
            down: true,
        }),
        ServerMsg::Mouse(MouseEvent::Wheel { dx: 0, dy: -240 }),
        ServerMsg::ClipboardGrab {
            id: ClipboardId::Clipboard,
            seq: 1,
        },
        ServerMsg::ClipboardData {
            id: ClipboardId::Clipboard,
            seq: 1,
            chunk: Chunk::Data(vec![0xAA; 1000]),
        },
        ServerMsg::FileOffer {
            name: "archive.zip".into(),
            size: u64::MAX,
        },
        ServerMsg::FileChunk(Chunk::Data(vec![9; 32 * 1024])),
        ServerMsg::SetOptions(Options::default()),
        ServerMsg::Close(CloseReason::ProtocolError),
        ServerMsg::KeepAlive,
    ]
}

#[tokio::test]
async fn client_messages_roundtrip() {
    let (a, b) = tokio::io::duplex(1 << 20);
    let mut tx = MessageStream::new(a);
    let mut rx = MessageStream::new(b);
    for msg in all_client_msgs() {
        tx.send(&msg).await.unwrap();
        let got: ClientMsg = rx.recv().await.unwrap();
        assert_eq!(got, msg);
    }
}

#[tokio::test]
async fn server_messages_roundtrip() {
    let (a, b) = tokio::io::duplex(1 << 20);
    let mut tx = MessageStream::new(a);
    let mut rx = MessageStream::new(b);
    for msg in all_server_msgs() {
        tx.send(&msg).await.unwrap();
        let got: ServerMsg = rx.recv().await.unwrap();
        assert_eq!(got, msg);
    }
}

#[tokio::test]
async fn interleaved_bidirectional_stream() {
    let (a, b) = tokio::io::duplex(1 << 20);
    let mut server = MessageStream::new(a);
    let mut client = MessageStream::new(b);

    let client_task = tokio::spawn(async move {
        for msg in all_client_msgs() {
            client.send(&msg).await.unwrap();
            let reply: ServerMsg = client.recv().await.unwrap();
            assert!(matches!(reply, ServerMsg::KeepAlive));
        }
    });

    for expected in all_client_msgs() {
        let got: ClientMsg = server.recv().await.unwrap();
        assert_eq!(got, expected);
        server.send(&ServerMsg::KeepAlive).await.unwrap();
    }
    client_task.await.unwrap();
}

/// Deterministic pseudo-fuzz: feed many different byte patterns as "frames" and
/// require the decoder to return an error, never panic.
#[tokio::test]
async fn decoder_never_panics_on_garbage() {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for _ in 0..2000 {
        let len = (next() % 64) as usize;
        let payload: Vec<u8> = (0..len).map(|_| (next() & 0xFF) as u8).collect();

        let (a, mut b) = tokio::io::duplex(4096);
        let frame_len = (payload.len() as u32).to_le_bytes();
        tokio::io::AsyncWriteExt::write_all(&mut b, &frame_len)
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut b, &payload)
            .await
            .unwrap();
        drop(b);

        let mut s = MessageStream::new(a);
        // Either it happens to decode to a valid message, or it errors. Both
        // fine; a panic would fail the test.
        let _: Result<ServerMsg, _> = s.recv().await;
    }
}

#[test]
fn edge_helpers_are_consistent() {
    for e in Edge::ALL {
        assert_eq!(e.opposite().opposite(), e);
    }
}
