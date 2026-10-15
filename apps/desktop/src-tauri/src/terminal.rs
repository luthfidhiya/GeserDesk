// SPDX-License-Identifier: GPL-3.0-or-later
//! The terminal window's side: runs one `geserdesk` command-line program at a time
//! and streams what it prints to the window.
//!
//! Only the CLI that sits next to this program is ever run, and only with the
//! arguments typed (split into words here, never handed to a shell), so the window
//! cannot be used to start anything else.

use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tracing::{info, warn};

use crate::events::{TERMINAL_EXIT, TERMINAL_OUT};

#[derive(Default)]
pub struct TerminalState {
    current: Mutex<Option<Arc<Mutex<Child>>>>,
    next_id: AtomicU64,
}

#[derive(Clone, Serialize)]
struct Out {
    id: u64,
    /// `out` or `err`.
    stream: &'static str,
    line: String,
}

#[derive(Clone, Serialize)]
struct Exit {
    id: u64,
    /// `None` when the program was ended by a signal or by Hentikan.
    code: Option<i32>,
}

/// The CLI program: `geserdesk` beside this one.
fn cli_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("tidak tahu lokasi aplikasi: {e}"))?;
    let name = if cfg!(windows) {
        "geserdesk.exe"
    } else {
        "geserdesk"
    };
    let path = exe.with_file_name(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "{} tidak ditemukan di samping aplikasi ({}).",
            name,
            path.display()
        ))
    }
}

/// Split a typed line into arguments: spaces separate them, quotes (single or
/// double) keep spaces inside one, and a backslash is kept as it is (Windows paths).
/// An unclosed quote is an error rather than a guess.
pub fn split_args(line: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                has = true;
            }
            (None, c) if c.is_whitespace() => {
                if has {
                    args.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, c) => {
                cur.push(c);
                has = true;
            }
        }
    }
    if quote.is_some() {
        return Err("tanda kutip belum ditutup".into());
    }
    if has {
        args.push(cur);
    }
    Ok(args)
}

/// Forward each line of `from` to the window.
fn pump(app: AppHandle, id: u64, stream: &'static str, from: impl Read + Send + 'static) {
    std::thread::spawn(move || {
        for raw in BufReader::new(from).split(b'\n') {
            let Ok(raw) = raw else { break };
            let line = String::from_utf8_lossy(&raw)
                .trim_end_matches('\r')
                .to_string();
            let _ = app.emit(TERMINAL_OUT, Out { id, stream, line });
        }
    });
}

/// Run `geserdesk <line>`. Resolves to the run's id; its output and its end arrive
/// as events carrying that id.
#[tauri::command]
pub fn terminal_run(
    app: AppHandle,
    state: State<'_, TerminalState>,
    line: String,
) -> Result<u64, String> {
    let mut args = split_args(&line)?;
    // Typing the program's own name first is natural; it is not an argument.
    if args.first().is_some_and(|a| {
        a.eq_ignore_ascii_case("geserdesk") || a.eq_ignore_ascii_case("geserdesk.exe")
    }) {
        args.remove(0);
    }
    let mut running = state.current.lock().unwrap();
    if running.is_some() {
        return Err("Masih ada perintah yang berjalan. Hentikan dulu.".into());
    }
    let path = cli_path()?;
    let mut cmd = Command::new(&path);
    cmd.args(&args)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flashing up
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let id = state.next_id.fetch_add(1, Ordering::SeqCst) + 1;
    info!(id, ?args, "terminal command started");
    if let Some(out) = child.stdout.take() {
        pump(app.clone(), id, "out", out);
    }
    if let Some(err) = child.stderr.take() {
        pump(app.clone(), id, "err", err);
    }
    let child = Arc::new(Mutex::new(child));
    *running = Some(Arc::clone(&child));
    drop(running);

    // Wait for it to end, without holding the child while waiting so Hentikan can
    // still reach it.
    let app = app.clone();
    std::thread::spawn(move || {
        let code = loop {
            match child.lock().unwrap().try_wait() {
                Ok(Some(status)) => break status.code(),
                Ok(None) => {}
                Err(e) => {
                    warn!(error = %e, "waiting for the terminal command");
                    break None;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        // A short pause so the last lines of output reach the window before the end does.
        std::thread::sleep(Duration::from_millis(150));
        let state = app.state::<TerminalState>();
        let mut current = state.current.lock().unwrap();
        if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, &child)) {
            *current = None;
        }
        drop(current);
        let _ = app.emit(TERMINAL_EXIT, Exit { id, code });
    });
    Ok(id)
}

/// End the command that is running, if any.
#[tauri::command]
pub fn terminal_kill(state: State<'_, TerminalState>) {
    kill(&state);
}

pub fn kill(state: &TerminalState) {
    if let Some(child) = state.current.lock().unwrap().as_ref() {
        let _ = child.lock().unwrap().kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(line: &str) -> Vec<String> {
        split_args(line).unwrap()
    }

    #[test]
    fn splits_on_spaces_and_drops_the_extra_ones() {
        assert_eq!(s("  status  "), ["status"]);
        assert_eq!(
            s("client --name a   --server h:1"),
            ["client", "--name", "a", "--server", "h:1"]
        );
        assert!(s("   ").is_empty());
    }

    #[test]
    fn quotes_keep_spaces_together_and_an_empty_pair_is_an_argument() {
        assert_eq!(
            s(r#"server --config "C:\My Dir\s.toml""#),
            ["server", "--config", r"C:\My Dir\s.toml"]
        );
        assert_eq!(s("a 'b c' d"), ["a", "b c", "d"]);
        assert_eq!(s(r#"a "" b"#), ["a", "", "b"]);
        assert_eq!(s(r#"x"y z"w"#), ["xy zw"]);
    }

    #[test]
    fn a_backslash_is_kept_for_windows_paths() {
        assert_eq!(s(r"--config C:\a\b.toml"), ["--config", r"C:\a\b.toml"]);
    }

    #[test]
    fn an_unclosed_quote_is_refused() {
        assert!(split_args(r#"a "b c"#).is_err());
    }
}
