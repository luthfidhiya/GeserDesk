// SPDX-License-Identifier: GPL-3.0-or-later
//! The layout editor: a small web page served by the server for arranging
//! machines and monitors on the shared virtual desk.
//!
//! * `GET /`            -- the page (one self-contained HTML file).
//! * `GET /api/state`   -- every configured screen with its real monitors,
//!   connection status and current position.
//! * `POST /api/layout` -- new positions; applied to the running router at once
//!   and written back into the config file (comments preserved).
//!
//! The page can rewrite the config, so it listens on loopback by default and
//! the server refuses requests whose `Host` header is not one it expects
//! (defence against DNS rebinding). `POST` requires a JSON content type, which
//! a cross-site form cannot send without a CORS preflight we never answer.

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use geserdesk_proto::{Point, Rect};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::desk::{effective_origins, DEFAULT_SIZE};
use crate::hub::Hub;
use crate::layout::Layout;

const PAGE: &str = include_str!("ui.html");

/// Largest coordinate the editor accepts, to keep arithmetic far from overflow.
const MAX_COORD: i32 = 1_000_000;

/// Everything the editor needs to read and change.
pub struct UiState {
    pub hub: Arc<Hub>,
    pub config_path: Option<PathBuf>,
    pub local: String,
    pub local_monitors: Vec<Rect>,
    pub layout: Layout,
    pub positions: Mutex<BTreeMap<String, Point>>,
    /// Last monitors seen for each screen, so a disconnected machine keeps its
    /// real shape on the page.
    known: Mutex<HashMap<String, Vec<Rect>>>,
}

impl UiState {
    pub fn new(
        hub: Arc<Hub>,
        config_path: Option<PathBuf>,
        local: String,
        local_monitors: Vec<Rect>,
        layout: Layout,
        positions: BTreeMap<String, Point>,
    ) -> Arc<Self> {
        Arc::new(Self {
            hub,
            config_path,
            local,
            local_monitors,
            layout,
            positions: Mutex::new(positions),
            known: Mutex::new(HashMap::new()),
        })
    }

    /// Monitors for `name` and whether they are real (as opposed to a
    /// placeholder for a machine that has never connected).
    fn monitors_for(&self, name: &str, live: &HashMap<String, Vec<Rect>>) -> (Vec<Rect>, bool) {
        if name == self.local {
            return (self.local_monitors.clone(), true);
        }
        if let Some(m) = live.get(name) {
            self.known
                .lock()
                .unwrap()
                .insert(name.to_string(), m.clone());
            return (m.clone(), true);
        }
        if let Some(m) = self.known.lock().unwrap().get(name) {
            return (m.clone(), true);
        }
        (vec![Rect::new(0, 0, DEFAULT_SIZE.0, DEFAULT_SIZE.1)], false)
    }

    /// Current monitors, origins and status of every screen.
    fn snapshot(&self) -> Vec<ScreenView> {
        let live = self.hub.snapshot();
        let names = self.layout.screens();
        let mons: HashMap<String, (Vec<Rect>, bool)> = names
            .iter()
            .map(|n| (n.clone(), self.monitors_for(n, &live)))
            .collect();
        let size_of = |n: &str| {
            mons.get(n)
                .and_then(|(m, _)| Rect::union_all(m))
                .map_or(DEFAULT_SIZE, |r| (r.w, r.h))
        };
        let origins = effective_origins(
            names,
            &self.local,
            &self.layout,
            &self.positions.lock().unwrap(),
            &size_of,
        );
        names
            .iter()
            .map(|n| {
                let (monitors, known) = mons[n].clone();
                ScreenView {
                    name: n.clone(),
                    local: *n == self.local,
                    connected: *n == self.local || live.contains_key(n),
                    placeholder: !known,
                    origin: origins[n],
                    monitors,
                }
            })
            .collect()
    }
}

struct ScreenView {
    name: String,
    local: bool,
    connected: bool,
    placeholder: bool,
    origin: Point,
    monitors: Vec<Rect>,
}

fn rect_json(r: &Rect) -> Value {
    json!({ "x": r.x, "y": r.y, "w": r.w, "h": r.h })
}

fn state_json(state: &UiState) -> Value {
    let screens: Vec<Value> = state
        .snapshot()
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "local": s.local,
                "connected": s.connected,
                "placeholder": s.placeholder,
                "origin": { "x": s.origin.x, "y": s.origin.y },
                "monitors": s.monitors.iter().map(rect_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "local": state.local,
        "screens": screens,
        "config_path": state.config_path.as_ref().map(|p| p.display().to_string()),
    })
}

#[derive(Deserialize)]
struct LayoutRequest {
    positions: BTreeMap<String, PointDto>,
}

#[derive(Deserialize)]
struct PointDto {
    x: i32,
    y: i32,
}

/// Why a submitted layout was refused.
#[derive(Debug, PartialEq, Eq)]
enum LayoutError {
    UnknownScreen(String),
    OutOfRange(String),
    Overlap(String, String),
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownScreen(n) => write!(f, "layar tidak dikenal: {n}"),
            Self::OutOfRange(n) => write!(f, "posisi {n} di luar batas"),
            Self::Overlap(a, b) => write!(f, "layar {a} dan {b} saling tumpang tindih"),
        }
    }
}

/// Validate `requested` against the current state and return the full set of
/// positions that should replace the stored ones.
fn validate(
    state: &UiState,
    requested: &BTreeMap<String, PointDto>,
) -> Result<BTreeMap<String, Point>, LayoutError> {
    let views = state.snapshot();
    let mut next: BTreeMap<String, Point> =
        views.iter().map(|v| (v.name.clone(), v.origin)).collect();
    for (name, p) in requested {
        if !next.contains_key(name) {
            return Err(LayoutError::UnknownScreen(name.clone()));
        }
        if p.x.abs() > MAX_COORD || p.y.abs() > MAX_COORD {
            return Err(LayoutError::OutOfRange(name.clone()));
        }
        next.insert(name.clone(), Point::new(p.x, p.y));
    }

    // No two machines may occupy the same desk pixels: the cursor would have
    // two places to go.
    let desk: Vec<(&str, Vec<Rect>)> = views
        .iter()
        .map(|v| {
            let o = next[&v.name];
            let moved = v
                .monitors
                .iter()
                .map(|m| Rect::new(m.x + o.x, m.y + o.y, m.w, m.h))
                .collect();
            (v.name.as_str(), moved)
        })
        .collect();
    for (i, (a, ma)) in desk.iter().enumerate() {
        for (b, mb) in &desk[i + 1..] {
            if ma.iter().any(|x| mb.iter().any(|y| intersects(x, y))) {
                return Err(LayoutError::Overlap((*a).to_string(), (*b).to_string()));
            }
        }
    }
    Ok(next)
}

fn intersects(a: &Rect, b: &Rect) -> bool {
    a.left() < b.right() && b.left() < a.right() && a.top() < b.bottom() && b.top() < a.bottom()
}

/// Write `x`/`y` into each matching `[[screens]]` table of the config file,
/// keeping everything else (comments, ordering) as it was.
pub fn write_positions(path: &Path, positions: &BTreeMap<String, Point>) -> anyhow::Result<()> {
    use toml_edit::{value, DocumentMut};

    let text = std::fs::read_to_string(path)?;
    let mut doc: DocumentMut = text.parse()?;
    let screens = doc
        .get_mut("screens")
        .and_then(|i| i.as_array_of_tables_mut())
        .ok_or_else(|| anyhow::anyhow!("config has no [[screens]]"))?;
    for table in screens.iter_mut() {
        let Some(name) = table
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        else {
            continue;
        };
        if let Some(p) = positions.get(&name) {
            table["x"] = value(i64::from(p.x));
            table["y"] = value(i64::from(p.y));
        }
    }

    // Replace atomically so a crash never leaves a half-written config.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, doc.to_string())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ---- HTTP ---------------------------------------------------------------------

async fn index() -> Html<&'static str> {
    Html(PAGE)
}

async fn get_state(State(state): State<Arc<UiState>>) -> Json<Value> {
    Json(state_json(&state))
}

async fn post_layout(
    State(state): State<Arc<UiState>>,
    Json(req): Json<LayoutRequest>,
) -> Response {
    let next = match validate(&state, &req.positions) {
        Ok(n) => n,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": e.to_string() })),
            )
                .into_response()
        }
    };

    let mut saved = false;
    if let Some(path) = &state.config_path {
        match write_positions(path, &next) {
            Ok(()) => saved = true,
            Err(e) => {
                warn!(error = %e, "could not save the layout to the config file");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "ok": false, "error": format!("gagal menyimpan config: {e}") })),
                )
                    .into_response();
            }
        }
    }
    *state.positions.lock().unwrap() = next.clone();
    state.hub.apply_positions(next);
    info!(saved, "layout updated from the editor");
    Json(json!({ "ok": true, "saved": saved })).into_response()
}

/// Reject requests whose Host header is not one we expect.
async fn check_host(State(allowed): State<Arc<Vec<String>>>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(host_without_port)
        .unwrap_or_default();
    if allowed.contains(&host) {
        next.run(req).await
    } else {
        (StatusCode::FORBIDDEN, "unexpected Host header").into_response()
    }
}

fn host_without_port(h: &str) -> String {
    if let Some(rest) = h.strip_prefix('[') {
        // [::1]:24811
        return format!("[{}", rest.split(']').next().unwrap_or("")) + "]";
    }
    h.rsplit_once(':').map_or(h, |(host, _)| host).to_string()
}

fn router(state: Arc<UiState>, addr: SocketAddr) -> Router {
    let mut allowed = vec![
        "127.0.0.1".to_string(),
        "localhost".to_string(),
        "[::1]".to_string(),
    ];
    if !addr.ip().is_loopback() && !addr.ip().is_unspecified() {
        allowed.push(addr.ip().to_string());
    }
    Router::new()
        .route("/", get(index))
        .route("/api/state", get(get_state))
        .route("/api/layout", post(post_layout))
        .layer(middleware::from_fn_with_state(
            Arc::new(allowed),
            check_host,
        ))
        .with_state(state)
}

/// Serve the editor on `addr` until the process ends.
pub async fn serve(addr: SocketAddr, state: Arc<UiState>) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    serve_on(listener, state).await
}

/// Like [`serve`] with an already bound listener (tests bind port 0).
pub async fn serve_on(
    listener: tokio::net::TcpListener,
    state: Arc<UiState>,
) -> anyhow::Result<()> {
    let addr = listener.local_addr()?;
    if !addr.ip().is_loopback() {
        warn!(%addr, "the layout editor is reachable from other machines and can rewrite the config; bind it to 127.0.0.1 unless you trust the network");
    }
    info!(url = %format!("http://{addr}/"), "layout editor ready");
    axum::serve(listener, router(state, addr)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn state_with(config: Option<PathBuf>) -> Arc<UiState> {
        let layout = Layout::new(vec!["linux-pc".into(), "win".into()]);
        UiState::new(
            Hub::new(),
            config,
            "linux-pc".into(),
            vec![Rect::new(0, 0, 1920, 1080)],
            layout,
            BTreeMap::new(),
        )
    }

    fn req(positions: &[(&str, i32, i32)]) -> BTreeMap<String, PointDto> {
        positions
            .iter()
            .map(|(n, x, y)| (n.to_string(), PointDto { x: *x, y: *y }))
            .collect()
    }

    fn temp_file(name: &str, content: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("geserdesk-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn unconnected_screens_get_a_placeholder_and_are_marked() {
        let st = state_with(None);
        let json = state_json(&st);
        let screens = json["screens"].as_array().unwrap();
        assert_eq!(screens[0]["name"], "linux-pc");
        assert_eq!(screens[0]["connected"], true);
        assert_eq!(screens[1]["name"], "win");
        assert_eq!(screens[1]["connected"], false);
        assert_eq!(screens[1]["placeholder"], true);
        // Unpositioned screens line up to the right.
        assert_eq!(screens[1]["origin"]["x"], 1920);
    }

    #[test]
    fn a_connected_client_shows_its_real_monitors() {
        let st = state_with(None);
        let _reg = st
            .hub
            .register(
                "win",
                vec![Rect::new(0, 0, 1280, 1024), Rect::new(1280, 0, 1280, 1024)],
            )
            .unwrap();
        let json = state_json(&st);
        let win = &json["screens"][1];
        assert_eq!(win["connected"], true);
        assert_eq!(win["placeholder"], false);
        assert_eq!(win["monitors"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn disconnected_screens_keep_their_last_seen_shape() {
        let st = state_with(None);
        drop(
            st.hub
                .register("win", vec![Rect::new(0, 0, 800, 600)])
                .unwrap(),
        );
        state_json(&st); // observed while connected
        let reg = st
            .hub
            .register("win", vec![Rect::new(0, 0, 800, 600)])
            .unwrap();
        state_json(&st);
        drop(reg);
        let json = state_json(&st);
        assert_eq!(json["screens"][1]["connected"], false);
        assert_eq!(json["screens"][1]["monitors"][0]["w"], 800);
    }

    #[test]
    fn validation_accepts_touching_and_rejects_overlap() {
        let st = state_with(None);
        let ok = validate(&st, &req(&[("win", 1920, 200)])).unwrap();
        assert_eq!(ok["win"], Point::new(1920, 200));
        assert_eq!(
            validate(&st, &req(&[("win", 1000, 0)])).unwrap_err(),
            LayoutError::Overlap("linux-pc".into(), "win".into())
        );
    }

    #[test]
    fn validation_rejects_unknown_screens_and_huge_values() {
        let st = state_with(None);
        assert_eq!(
            validate(&st, &req(&[("ghost", 0, 0)])).unwrap_err(),
            LayoutError::UnknownScreen("ghost".into())
        );
        assert_eq!(
            validate(&st, &req(&[("win", 2_000_000, 0)])).unwrap_err(),
            LayoutError::OutOfRange("win".into())
        );
    }

    #[test]
    fn writing_positions_keeps_comments_and_other_settings() {
        let path = temp_file(
            "write",
            "# my setup\n[server]\nlisten = \"0.0.0.0:24810\"  # keep me\n\n[[screens]]\nname = \"linux-pc\"\n\n[[screens]]\nname = \"win\"\n",
        );
        let mut p = BTreeMap::new();
        p.insert("linux-pc".to_string(), Point::new(0, 0));
        p.insert("win".to_string(), Point::new(1920, -150));
        write_positions(&path, &p).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my setup"));
        assert!(text.contains("# keep me"));
        let cfg = crate::config::Config::parse(&text).unwrap();
        assert_eq!(cfg.positions["win"], Point::new(1920, -150));
        assert_eq!(cfg.listen.port(), 24810);
    }

    async fn http(addr: SocketAddr, request: String) -> String {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(request.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn http_api_serves_state_saves_layout_and_guards_the_host_header() {
        let path = temp_file(
            "http",
            "[[screens]]\nname = \"linux-pc\"\n\n[[screens]]\nname = \"win\"\n",
        );
        let st = state_with(Some(path.clone()));
        let mut events = st.hub.take_events().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve_on(listener, Arc::clone(&st)));
        let host = format!("127.0.0.1:{}", addr.port());

        // The page and the state.
        let page = http(
            addr,
            format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(page.contains("<title>"));
        let state = http(
            addr,
            format!("GET /api/state HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(state.contains("\"linux-pc\""), "{state}");

        // A foreign Host header is refused (DNS rebinding).
        let evil = http(
            addr,
            "GET /api/state HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n".into(),
        )
        .await;
        assert!(evil.starts_with("HTTP/1.1 403"), "{evil}");

        // Saving without a JSON content type is refused.
        let body = r#"{"positions":{"win":{"x":1920,"y":100}}}"#;
        let wrong = http(addr, format!(
            "POST /api/layout HTTP/1.1\r\nHost: {host}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())).await;
        assert!(wrong.starts_with("HTTP/1.1 415"), "{wrong}");

        // A proper save is applied live and written to the file.
        let ok = http(addr, format!(
            "POST /api/layout HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())).await;
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.contains("\"saved\":true"), "{ok}");
        let cfg = crate::config::Config::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.positions["win"], Point::new(1920, 100));
        assert!(matches!(
            events.recv().await,
            Some(crate::hub::HubEvent::Positions(p)) if p["win"] == Point::new(1920, 100)
        ));

        // An overlapping layout is refused and leaves the file alone.
        let bad = r#"{"positions":{"win":{"x":10,"y":10}}}"#;
        let refused = http(addr, format!(
            "POST /api/layout HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{bad}", bad.len())).await;
        assert!(refused.starts_with("HTTP/1.1 400"), "{refused}");
        let cfg = crate::config::Config::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.positions["win"], Point::new(1920, 100));
    }

    #[test]
    fn host_header_parsing() {
        assert_eq!(host_without_port("127.0.0.1:24811"), "127.0.0.1");
        assert_eq!(host_without_port("localhost"), "localhost");
        assert_eq!(host_without_port("[::1]:24811"), "[::1]");
    }
}
