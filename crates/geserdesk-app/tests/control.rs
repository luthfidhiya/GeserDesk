// SPDX-License-Identifier: GPL-3.0-or-later
//! The daemon and its controllers over a real local socket. No display needed:
//! servers run with capture off and TLS off, on port 0.

use std::path::PathBuf;
use std::time::Duration;

use geserdesk_app::control::wire::{encode, read_line, Command, Request, ToController};
use geserdesk_app::control::{ClientOptions, ConnectError, Endpoint, Event, StartSpec, PROTOCOL};
use geserdesk_app::daemon::{run, DaemonOptions};
use geserdesk_app::{LogBuffer, Mode, Remote, Status};
use geserdesk_proto::{Point, Rect};
use geserdesk_server::{run_server_with, Config};
use interprocess::local_socket::traits::tokio::Stream as _;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

type Daemon = JoinHandle<anyhow::Result<()>>;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("gd-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A server config with TLS off (so no identity is written anywhere) on a free port.
fn server_config(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("server.toml");
    std::fs::write(
        &path,
        "[server]\nlisten = \"127.0.0.1:0\"\nscreen = \"linux-pc\"\ntls = false\n\n\
         [[screens]]\nname = \"linux-pc\"\n\n[[screens]]\nname = \"win\"\n",
    )
    .unwrap();
    path
}

async fn start_daemon(dir: &std::path::Path, exit_on_stop: bool) -> (Endpoint, Daemon) {
    start_daemon_with(dir, exit_on_stop, LogBuffer::new()).await
}

async fn start_daemon_with(
    dir: &std::path::Path,
    exit_on_stop: bool,
    log: LogBuffer,
) -> (Endpoint, Daemon) {
    let endpoint = Endpoint::in_dir(dir.join("run"));
    let opts = DaemonOptions {
        endpoint: endpoint.clone(),
        started_by: "test".into(),
        exit_on_stop,
        idle_timeout: None,
        initial: None,
    };
    let task = tokio::spawn(run(opts, log));
    for _ in 0..100 {
        if endpoint.connect().await.is_ok() {
            return (endpoint, task);
        }
        sleep(Duration::from_millis(30)).await;
    }
    panic!("the daemon never started listening");
}

async fn until<T, F: std::future::Future<Output = Option<T>>>(mut probe: impl FnMut() -> F) -> T {
    for _ in 0..200 {
        if let Some(v) = probe().await {
            return v;
        }
        sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting");
}

/// Send one raw line and read the answer.
async fn raw(endpoint: &Endpoint, cmd: Command) -> ToController {
    let stream = endpoint.connect().await.unwrap();
    let (recv, mut send) = stream.split();
    let mut recv = BufReader::new(recv);
    send.write_all(encode(&Request { id: 1, cmd }).as_bytes())
        .await
        .unwrap();
    let line = timeout(Duration::from_secs(5), read_line(&mut recv))
        .await
        .unwrap()
        .unwrap()
        .expect("the daemon answered");
    serde_json::from_str(&line).unwrap()
}

fn refusal(message: ToController) -> String {
    match message {
        ToController::Response {
            result: Err(why), ..
        } => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_controller_attaches_and_sees_the_daemon() {
    let dir = scratch("attach");
    let (endpoint, _daemon) = start_daemon(&dir, false).await;
    let remote = Remote::connect(&endpoint).await.unwrap();
    assert_eq!(remote.info().protocol, PROTOCOL);
    assert_eq!(remote.info().started_by, "test");
    assert_eq!(remote.info().mode, None);
    assert_eq!(remote.status().await.unwrap(), Status::Stopped);
    assert_eq!(remote.desk().await.unwrap().map(|d| d.local), None);
}

#[tokio::test]
async fn a_controller_can_cancel_transfers_whether_or_not_one_is_running() {
    let dir = scratch("canceltransfer");
    let (endpoint, _daemon) = start_daemon(&dir, false).await;
    let remote = Remote::connect(&endpoint).await.unwrap();
    // Nothing is being pulled: the command is still accepted, and changes nothing.
    remote.cancel_transfer().await.unwrap();
    assert_eq!(remote.status().await.unwrap(), Status::Stopped);
}

#[tokio::test]
async fn a_wrong_token_or_a_missing_hello_is_refused() {
    let dir = scratch("refuse");
    let (endpoint, _daemon) = start_daemon(&dir, false).await;

    let bad_token = Command::Hello {
        protocol: PROTOCOL,
        token: "nope".into(),
    };
    assert!(refusal(raw(&endpoint, bad_token).await).contains("token"));

    assert!(refusal(raw(&endpoint, Command::Stop).await).contains("Hello"));

    // None of that stopped the daemon from serving a proper controller.
    assert!(Remote::connect(&endpoint).await.is_ok());
}

#[tokio::test]
async fn a_controller_of_another_protocol_can_only_read_info_and_shut_down() {
    let dir = scratch("version");
    let (endpoint, daemon) = start_daemon(&dir, false).await;
    let token = endpoint.read_token().unwrap();

    // Over a raw connection: Hello is accepted, then everything but Info/Shutdown is not.
    let stream = endpoint.connect().await.unwrap();
    let (recv, mut send) = stream.split();
    let mut recv = BufReader::new(recv);
    let say = |id, cmd| encode(&Request { id, cmd });
    send.write_all(
        say(
            1,
            Command::Hello {
                protocol: PROTOCOL + 1,
                token,
            },
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let hello = read_line(&mut recv).await.unwrap().unwrap();
    assert!(
        hello.contains(&format!("\"protocol\":{PROTOCOL}")),
        "{hello}"
    );
    send.write_all(say(2, Command::Status).as_bytes())
        .await
        .unwrap();
    let refused = read_line(&mut recv).await.unwrap().unwrap();
    assert!(refused.contains("versi protokol beda"), "{refused}");
    send.write_all(say(3, Command::Info).as_bytes())
        .await
        .unwrap();
    let info = read_line(&mut recv).await.unwrap().unwrap();
    assert!(info.contains("\"pid\""), "{info}");
    send.write_all(say(4, Command::Shutdown).as_bytes())
        .await
        .unwrap();
    let _ = read_line(&mut recv).await.unwrap();

    timeout(Duration::from_secs(10), daemon)
        .await
        .expect("the old daemon was ended")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn nothing_listening_is_reported_as_no_daemon() {
    let dir = scratch("none");
    let endpoint = Endpoint::in_dir(dir.join("run"));
    assert!(matches!(
        Remote::connect(&endpoint).await,
        Err(ConnectError::NotRunning(_))
    ));
}

#[tokio::test]
async fn a_second_daemon_is_refused() {
    let dir = scratch("second");
    let (endpoint, _daemon) = start_daemon(&dir, false).await;
    let err = endpoint
        .bind()
        .await
        .err()
        .expect("a live daemon is not replaced");
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
}

#[cfg(unix)]
#[tokio::test]
async fn a_socket_left_by_a_dead_daemon_is_replaced() {
    let dir = scratch("stale");
    let run_dir = dir.join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    // A listener that is dropped without cleanup leaves its file behind.
    drop(std::os::unix::net::UnixListener::bind(run_dir.join("control.sock")).unwrap());
    assert!(run_dir.join("control.sock").exists());

    let endpoint = Endpoint::in_dir(&run_dir);
    let bound = endpoint.bind().await.expect("the stale socket is cleared");
    assert_eq!(bound.token.len(), 64);
    assert_eq!(endpoint.read_token().unwrap(), bound.token);
}

#[cfg(unix)]
#[tokio::test]
async fn the_socket_and_token_are_private_to_the_user() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("perms");
    let (endpoint, _daemon) = start_daemon(&dir, false).await;
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(endpoint.dir()), 0o700);
    assert_eq!(mode(&endpoint.token_path()), 0o600);
    assert_eq!(mode(&endpoint.dir().join("control.sock")), 0o600);
}

#[tokio::test]
async fn a_server_runs_through_the_channel_and_stopping_ends_the_daemon() {
    let dir = scratch("server");
    let config = server_config(&dir);
    let (endpoint, daemon) = start_daemon(&dir, true).await;
    let remote = Remote::connect(&endpoint).await.unwrap();

    remote
        .start(StartSpec::Server {
            config: config.display().to_string(),
            capture: false,
        })
        .await
        .unwrap();
    assert_eq!(remote.status().await.unwrap(), Status::ServerRunning);
    assert_eq!(
        remote.refresh_info().await.unwrap().mode,
        Some(Mode::Server)
    );

    // The desk appears once the listener is up.
    let desk = until(|| async { remote.desk().await.unwrap() }).await;
    assert_eq!(desk.local, "linux-pc");
    assert_eq!(desk.screens.len(), 2);
    assert!(!desk.tls);

    // Starting again is refused while it runs.
    let again = remote
        .start(StartSpec::Server {
            config: config.display().to_string(),
            capture: false,
        })
        .await;
    assert!(again.unwrap_err().contains("Sudah berjalan"));

    // A saved layout reaches the config file.
    let mut positions = std::collections::BTreeMap::new();
    positions.insert("win".to_string(), Point::new(1920, 0));
    assert!(remote.save_layout(positions).await.unwrap());
    assert!(std::fs::read_to_string(&config)
        .unwrap()
        .contains("x = 1920"));

    // Trust commands reach the server and report its refusals as text.
    assert!(remote.approve("win", "nope").await.is_err());

    remote.stop().await.unwrap();
    let ended = timeout(Duration::from_secs(10), daemon)
        .await
        .expect("the daemon ended");
    ended.unwrap().unwrap();
    assert!(
        endpoint.read_token().is_err(),
        "the token file is removed with the daemon"
    );
    assert!(Remote::connect(&endpoint).await.is_err());
}

#[tokio::test]
async fn every_subscriber_hears_the_status_changes() {
    let dir = scratch("events");
    let config = server_config(&dir);
    let (endpoint, _daemon) = start_daemon(&dir, false).await;
    let a = Remote::connect(&endpoint).await.unwrap();
    let b = Remote::connect(&endpoint).await.unwrap();
    let (mut ea, mut eb) = (a.events(), b.events());
    a.subscribe().await.unwrap();
    b.subscribe().await.unwrap();

    a.start(StartSpec::Server {
        config: config.display().to_string(),
        capture: false,
    })
    .await
    .unwrap();

    for events in [&mut ea, &mut eb] {
        let status = timeout(Duration::from_secs(5), async {
            loop {
                if let Event::Status(s) = events.recv().await.unwrap() {
                    return s;
                }
            }
        })
        .await
        .expect("a status event arrived");
        assert_eq!(status, Status::ServerRunning);
    }
    a.stop().await.unwrap();
}

#[tokio::test]
async fn log_lines_flow_to_subscribers_and_history_is_there_for_a_late_controller() {
    let dir = scratch("logs");
    let log = LogBuffer::new();
    let (endpoint, _daemon) = start_daemon_with(&dir, false, log.clone()).await;
    log.push(tracing::Level::INFO, "before anyone attached".into());

    let first = Remote::connect(&endpoint).await.unwrap();
    let mut events = first.events();
    first.subscribe().await.unwrap();
    log.push(tracing::Level::WARN, "after subscribing".into());

    let line = timeout(Duration::from_secs(5), async {
        loop {
            if let Event::Log(line) = events.recv().await.unwrap() {
                return line;
            }
        }
    })
    .await
    .expect("a log event arrived");
    assert_eq!(line.text, "after subscribing");
    assert_eq!(line.level, tracing::Level::WARN);

    // A controller that attaches later reads the history, oldest first.
    let late = Remote::connect(&endpoint).await.unwrap();
    let texts: Vec<_> = late
        .log_snapshot()
        .await
        .unwrap()
        .into_iter()
        .map(|l| l.text)
        .collect();
    assert_eq!(texts, ["before anyone attached", "after subscribing"]);

    late.clear_log().await.unwrap();
    assert!(first.log_snapshot().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_controller_that_reattaches_finds_the_session_still_running() {
    let dir = scratch("reattach");
    let config = server_config(&dir);
    let (endpoint, _daemon) = start_daemon(&dir, false).await;
    {
        let first = Remote::connect(&endpoint).await.unwrap();
        first
            .start(StartSpec::Server {
                config: config.display().to_string(),
                capture: false,
            })
            .await
            .unwrap();
        until(|| async { first.desk().await.unwrap() }).await;
    } // the first controller goes away; the session must not

    let second = Remote::connect(&endpoint).await.unwrap();
    assert_eq!(second.info().mode, Some(Mode::Server));
    assert_eq!(second.status().await.unwrap(), Status::ServerRunning);
    assert!(second.desk().await.unwrap().is_some());
    second.stop().await.unwrap(); // exit_on_stop is off: the daemon stays for another session
    assert_eq!(second.status().await.unwrap(), Status::Stopped);
}

#[tokio::test]
async fn shutdown_stops_the_session_and_ends_the_daemon() {
    let dir = scratch("shutdown");
    let config = server_config(&dir);
    let (endpoint, daemon) = start_daemon(&dir, false).await;
    let remote = Remote::connect(&endpoint).await.unwrap();
    let mut events = remote.events();
    remote
        .start(StartSpec::Server {
            config: config.display().to_string(),
            capture: false,
        })
        .await
        .unwrap();

    remote.shutdown().await.unwrap();
    timeout(Duration::from_secs(10), daemon)
        .await
        .expect("the daemon ended")
        .unwrap()
        .unwrap();
    timeout(Duration::from_secs(5), remote.closed())
        .await
        .expect("the controller noticed");
    assert!(remote.is_closed());
    // Anyone still waiting hears that the daemon is gone.
    let mut saw_closing = false;
    while let Ok(ev) = events.try_recv() {
        saw_closing |= matches!(ev, Event::Closing);
    }
    assert!(saw_closing);
    assert!(remote.status().await.is_err());
}

/// A plain-TCP server in this process for a client session to connect to.
/// Returns its address, a way to stop it, and its desk.
async fn local_server() -> (
    String,
    tokio::sync::watch::Sender<bool>,
    std::sync::Arc<geserdesk_server::DeskState>,
) {
    let port = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };
    let config = Config::parse(&format!(
        "[server]\nlisten = \"127.0.0.1:{port}\"\nscreen = \"linux-pc\"\ntls = false\n\n\
         [[screens]]\nname = \"linux-pc\"\n\n[[screens]]\nname = \"win\"\n"
    ))
    .unwrap();
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(run_server_with(config, false, shutdown, Some(ready_tx)));
    let desk = timeout(Duration::from_secs(5), ready_rx)
        .await
        .expect("the server came up")
        .unwrap();
    // The listener is bound just after the desk is handed over.
    sleep(Duration::from_millis(100)).await;
    (format!("127.0.0.1:{port}"), stop, desk)
}

fn plain_client(server: &str, reconnect: bool) -> StartSpec {
    StartSpec::Client(ClientOptions {
        server: server.into(),
        name: "win".into(),
        log_only: true,
        reconnect,
        tls: false,
        monitors: Some(vec![Rect::new(0, 0, 1280, 1024)]),
        ..ClientOptions::default()
    })
}

#[tokio::test]
async fn a_client_runs_through_the_channel_and_reports_its_forced_monitors() {
    let dir = scratch("client");
    let (addr, stop, desk) = local_server().await;
    let (endpoint, _daemon) = start_daemon(&dir, false).await;
    let remote = Remote::connect(&endpoint).await.unwrap();

    remote.start(plain_client(&addr, true)).await.unwrap();
    assert_eq!(
        remote.refresh_info().await.unwrap().mode,
        Some(Mode::Client)
    );
    until(|| async { (remote.status().await.unwrap() == Status::Connected).then_some(()) }).await;

    // The server sees the machine with the monitors the options forced, not detected ones.
    let win = desk
        .snapshot()
        .screens
        .into_iter()
        .find(|s| s.name == "win")
        .unwrap();
    assert!(win.connected);
    assert_eq!(win.monitors, vec![Rect::new(0, 0, 1280, 1024)]);

    // A client keeps trying while the server is away, and a second start is refused.
    assert!(remote.start(plain_client(&addr, true)).await.is_err());
    let _ = stop.send(true);
    until(|| async { matches!(remote.status().await.unwrap(), Status::Retrying(_)).then_some(()) })
        .await;
    remote.stop().await.unwrap();
    assert_eq!(remote.status().await.unwrap(), Status::Stopped);
}

#[tokio::test]
async fn a_client_without_reconnect_ends_with_its_connection() {
    let dir = scratch("oneshot");
    let (addr, stop, _desk) = local_server().await;
    let (endpoint, daemon) = start_daemon(&dir, true).await;
    let remote = Remote::connect(&endpoint).await.unwrap();
    remote.start(plain_client(&addr, false)).await.unwrap();
    until(|| async { (remote.status().await.unwrap() == Status::Connected).then_some(()) }).await;

    let _ = stop.send(true);
    // The daemon was told to leave with its session, and it does.
    let result = timeout(Duration::from_secs(10), daemon)
        .await
        .expect("the daemon ended after the connection did")
        .unwrap();
    // The server said goodbye, so this is a clean end (exit code 0), as `--no-reconnect` always was.
    assert!(result.is_ok(), "{result:?}");
}

#[tokio::test]
async fn a_client_that_cannot_reach_the_server_without_reconnect_fails_the_daemon() {
    let dir = scratch("refused");
    // Port 0 is never connectable and every OS rejects it at once. A freed
    // port would not do: Windows takes ~2 s to refuse it, and a parallel test
    // may take the port over.
    let (endpoint, daemon) = start_daemon(&dir, true).await;
    let remote = Remote::connect(&endpoint).await.unwrap();
    remote
        .start(plain_client("127.0.0.1:0", false))
        .await
        .unwrap();
    let result = timeout(Duration::from_secs(10), daemon)
        .await
        .expect("the daemon ended")
        .unwrap();
    assert!(
        result.is_err(),
        "an unreachable server is reported as a failure"
    );
}
