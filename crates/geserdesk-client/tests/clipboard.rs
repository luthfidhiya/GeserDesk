// SPDX-License-Identifier: GPL-3.0-or-later
//! Clipboard sharing between a real server and real clients over loopback, with
//! in-memory clipboards. Nothing here touches the desktop's clipboard.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use geserdesk_client::{connect, ClientConfig, NullSink, DEFAULT_HANDSHAKE_TIMEOUT};
use geserdesk_clipboard::{
    Clipboard, ClipboardContent, ClipboardError, ClipboardSync, MemoryClipboard,
};
use geserdesk_proto::{
    ClientMsg, MessageStream, Point, Rect, ScreenInfo, ServerMsg, PROTOCOL_VERSION,
};
use geserdesk_server::{run_server_shared, Config};
use tokio::sync::watch;
use tokio::time::{sleep, timeout};

/// A memory clipboard that counts how often something *other than the user*
/// wrote to it.
#[derive(Clone)]
struct Counting {
    mem: MemoryClipboard,
    writes: Arc<AtomicUsize>,
}

impl Counting {
    fn new() -> Self {
        Self {
            mem: MemoryClipboard::new(),
            writes: Arc::new(AtomicUsize::new(0)),
        }
    }
    fn writes(&self) -> usize {
        self.writes.load(Ordering::SeqCst)
    }
}

impl Clipboard for Counting {
    fn get(&mut self) -> Result<Option<ClipboardContent>, ClipboardError> {
        self.mem.clone().get()
    }
    fn set(&mut self, content: &ClipboardContent) -> Result<(), ClipboardError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.mem.clone().set(content)
    }
}

struct Rig {
    port: u16,
    server: Counting,
    a: Counting,
    b: Counting,
    stop: watch::Sender<bool>,
}

/// A free port that no other test in this process has been given. The port is
/// only free until the probe is dropped and the server binds it a moment later,
/// and these tests run in parallel: if two were ever handed the same port, each
/// one's clients would connect to the other's server (and the server that lost
/// the bind fails unseen). Seen once as exactly that: a server that does not share
/// the clipboard leaking text to a client, while a test of one that does found
/// its text never arriving. The collision itself was not reproduced.
fn free_port() -> u16 {
    static GIVEN: std::sync::Mutex<Vec<u16>> = std::sync::Mutex::new(Vec::new());
    loop {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        let mut given = GIVEN.lock().unwrap();
        if !given.contains(&port) {
            given.push(port);
            return port;
        }
    }
}
async fn rig(extra_options: &str) -> Rig {
    rig_with(extra_options, true).await
}

/// `connect_a` off leaves screen "a" free for a hand-made peer.
async fn rig_with(extra_options: &str, connect_a: bool) -> Rig {
    let port = free_port();
    let config = Config::parse(&format!(
        "[server]\nlisten = \"127.0.0.1:{port}\"\nscreen = \"desk\"\ntls = false\n\n\
         [[screens]]\nname = \"desk\"\n\n[[screens]]\nname = \"a\"\n\n[[screens]]\nname = \"b\"\n\n\
         [options]\n{extra_options}\n"
    ))
    .unwrap();
    let server = Counting::new();
    let (stop, shutdown) = watch::channel(false);
    tokio::spawn(run_server_shared(
        config,
        false,
        shutdown,
        None,
        Some(Box::new(server.clone())),
        None,
    ));
    sleep(Duration::from_millis(200)).await;

    let client = |name: &str| {
        let clip = Counting::new();
        let cfg = ClientConfig {
            server: format!("127.0.0.1:{port}"),
            name: name.into(),
            monitors: vec![Rect::new(0, 0, 1920, 1080)],
            detect: None,
            tls: None,
            handshake_timeout: DEFAULT_HANDSHAKE_TIMEOUT,
            clipboard: Some(ClipboardSync::new(clip.clone())),
            transfer_hooks: None,
        };
        tokio::spawn(async move { connect(&cfg, NullSink).await });
        clip
    };
    let a = if connect_a {
        client("a")
    } else {
        Counting::new()
    };
    let b = client("b");
    // Connected, and each side has taken note of what was already there.
    sleep(Duration::from_millis(900)).await;
    Rig {
        port,
        server,
        a,
        b,
        stop,
    }
}

async fn arrives(clip: &Counting, text: &str) {
    timeout(Duration::from_secs(5), async {
        while clip.mem.contents().as_deref() != Some(text) {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "{:?} never arrived; clipboard has {:?}",
            &text[..text.len().min(20)],
            clip.mem.contents().map(|c| c.len())
        )
    });
}

async fn picture_arrives(clip: &Counting, png: &[u8]) {
    timeout(Duration::from_secs(5), async {
        while clip.mem.image().as_deref() != Some(png) {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the picture never arrived"));
}

/// A screenshot-sized picture: noisy enough that PNG cannot shrink it to nothing.
fn screenshot() -> Vec<u8> {
    let (w, h) = (640u32, 400u32);
    let mut bytes = Vec::with_capacity((w * h * 4) as usize);
    let mut x: u32 = 12345;
    for _ in 0..w * h {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        bytes.extend_from_slice(&[(x >> 16) as u8, (x >> 8) as u8, x as u8, 255]);
    }
    geserdesk_clipboard::picture::to_png(&geserdesk_clipboard::picture::Rgba {
        width: w,
        height: h,
        bytes,
    })
    .unwrap()
}

#[tokio::test]
async fn a_screenshot_reaches_everyone_and_text_after_it_replaces_it() {
    let rig = rig("").await;
    let png = screenshot();
    assert!(png.len() > 32 * 1024, "spans several chunks");

    rig.a.mem.copy_image(png.clone());
    picture_arrives(&rig.server, &png).await;
    picture_arrives(&rig.b, &png).await;

    rig.b.mem.copy("teks sesudah gambar");
    arrives(&rig.a, "teks sesudah gambar").await;
    assert_eq!(rig.a.mem.image(), None, "the text replaced the picture");
    let _ = rig.stop.send(true);
}

#[tokio::test]
async fn a_copy_anywhere_reaches_everyone_once() {
    let rig = rig("").await;

    rig.a.mem.copy("from a");
    arrives(&rig.server, "from a").await;
    arrives(&rig.b, "from a").await;

    rig.server.mem.copy("from the server");
    arrives(&rig.a, "from the server").await;
    arrives(&rig.b, "from the server").await;

    // Longer than one chunk, with characters that span several bytes.
    let long = "dua baris\n€é ".repeat(20_000);
    rig.b.mem.copy(&long);
    arrives(&rig.server, &long).await;
    arrives(&rig.a, &long).await;

    // Let any echo happen. Each machine was written to once per copy made
    // elsewhere, and never for a copy it made itself.
    sleep(Duration::from_millis(1500)).await;
    assert_eq!(rig.a.writes(), 2, "the server's copy and b's");
    assert_eq!(rig.b.writes(), 2, "a's copy and the server's");
    assert_eq!(rig.server.writes(), 2, "a's copy and b's");
    let _ = rig.stop.send(true);
}

/// The point of carrying HTML: a page copied on one machine pastes with its style
/// on another, in both directions and through the server.
#[tokio::test]
async fn a_copied_pages_style_travels_with_its_text() {
    let rig = rig("").await;

    let page = "<h1 style=\"color:red\">Judul</h1><p>isi <b>tebal</b></p>";
    rig.server.mem.copy_html("Judul\nisi tebal", page);
    arrives(&rig.a, "Judul\nisi tebal").await;
    arrives(&rig.b, "Judul\nisi tebal").await;
    assert_eq!(rig.a.mem.html().as_deref(), Some(page));
    assert_eq!(rig.b.mem.html().as_deref(), Some(page));

    let other = "<ul><li>satu</li><li>dua</li></ul>";
    rig.b.mem.copy_html("satu\ndua", other);
    arrives(&rig.server, "satu\ndua").await;
    arrives(&rig.a, "satu\ndua").await;
    assert_eq!(rig.server.mem.html().as_deref(), Some(other));
    assert_eq!(rig.a.mem.html().as_deref(), Some(other));

    // Plain text after a styled copy must not keep the old style.
    rig.a.mem.copy("polos");
    arrives(&rig.server, "polos").await;
    arrives(&rig.b, "polos").await;
    assert_eq!(rig.server.mem.html(), None);
    assert_eq!(rig.b.mem.html(), None);

    sleep(Duration::from_millis(1500)).await;
    assert_eq!(rig.server.writes(), 2, "b's page and a's plain text");
    let _ = rig.stop.send(true);
}

#[tokio::test]
async fn a_server_that_does_not_share_keeps_every_clipboard_to_itself() {
    let rig = rig("clipboard_sharing = false").await;

    rig.a.mem.copy("private");
    rig.server.mem.copy("also private");
    sleep(Duration::from_millis(1500)).await;
    assert_eq!(rig.server.mem.contents().as_deref(), Some("also private"));
    assert_eq!(rig.b.mem.contents(), None);
    assert_eq!(rig.a.mem.contents().as_deref(), Some("private"));
    assert_eq!(rig.a.writes() + rig.b.writes() + rig.server.writes(), 0);
    let _ = rig.stop.send(true);
}

#[tokio::test]
async fn a_text_over_the_limit_stays_where_it_was_copied() {
    let rig = rig("clipboard_max_bytes = 100").await;

    rig.a.mem.copy(&"x".repeat(101));
    sleep(Duration::from_millis(1500)).await;
    assert_eq!(rig.b.mem.contents(), None);
    assert_eq!(rig.server.mem.contents(), None);

    rig.a.mem.copy(&"y".repeat(100));
    arrives(&rig.b, &"y".repeat(100)).await;
    let _ = rig.stop.send(true);
}

/// A client built before clipboard sharing: it answers keep-alives and ignores
/// whatever else the server says. It must stay connected, and the machines that
/// do share must not notice it.
#[tokio::test]
async fn a_client_that_predates_clipboard_sharing_is_unharmed() {
    let rig = rig_with("", false).await;
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", rig.port))
        .await
        .unwrap();
    let mut old = MessageStream::new(stream);
    let screen = ScreenInfo::from_monitors(
        "a".to_string(),
        vec![Rect::new(0, 0, 1920, 1080)],
        Point::new(960, 540),
    );
    old.send(&ClientMsg::Hello {
        version: PROTOCOL_VERSION,
        screen,
    })
    .await
    .unwrap();
    assert!(matches!(
        old.recv::<ServerMsg>().await.unwrap(),
        ServerMsg::Welcome { .. }
    ));
    assert!(matches!(
        old.recv::<ServerMsg>().await.unwrap(),
        ServerMsg::SetOptions(_)
    ));

    let old_peer = tokio::spawn(async move {
        let (mut clipboard_msgs, mut keepalives) = (0, 0);
        // Long enough for the server to probe it more than once.
        let _ = timeout(Duration::from_secs(7), async {
            while let Ok(msg) = old.recv::<ServerMsg>().await {
                match msg {
                    ServerMsg::KeepAlive => {
                        keepalives += 1;
                        old.send(&ClientMsg::KeepAlive).await.unwrap();
                    }
                    ServerMsg::ClipboardGrab { .. } | ServerMsg::ClipboardData { .. } => {
                        clipboard_msgs += 1
                    }
                    ServerMsg::Close(reason) => panic!("the server closed it: {reason}"),
                    _ => {}
                }
            }
            panic!("the server hung up");
        })
        .await;
        (clipboard_msgs, keepalives)
    });

    rig.server.mem.copy("for everyone");
    arrives(&rig.b, "for everyone").await;
    rig.b.mem.copy("from b");
    arrives(&rig.server, "from b").await;

    let (clipboard_msgs, keepalives) = old_peer.await.unwrap();
    assert!(clipboard_msgs >= 2, "the server did send it the copies");
    assert!(
        keepalives >= 2,
        "and it was still being probed, so still connected"
    );
    let _ = rig.stop.send(true);
}
