// SPDX-License-Identifier: GPL-3.0-or-later
//! A cheap "has the clipboard changed?" signal from the OS, so the clipboard is
//! only read (and an image only decoded) when there is something new.
//!
//! * Windows numbers every change: `GetClipboardSequenceNumber`.
//! * X11 tells a client that asks (XFixes) whenever the `CLIPBOARD` selection gets a
//!   new owner, which is what every copy does. One thread per process counts those
//!   notifications; it lives as long as the process.

/// A number that changes whenever the clipboard does; `None` if this platform (or
/// this session) gives no such signal, in which case the clipboard is read on
/// every poll.
pub fn change_count() -> Option<u64> {
    imp::change_count()
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber;

    pub fn change_count() -> Option<u64> {
        // 0 means the caller has no access to the clipboard's window station.
        match unsafe { GetClipboardSequenceNumber() } {
            0 => None,
            n => Some(n as u64),
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, OnceLock};

    use x11rb::connection::Connection;
    use x11rb::protocol::xfixes::{self, ConnectionExt as _, SelectionEventMask};
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::protocol::Event;

    static COUNT: OnceLock<Option<Arc<AtomicU64>>> = OnceLock::new();

    pub fn change_count() -> Option<u64> {
        COUNT
            .get_or_init(|| match start() {
                Ok(count) => Some(count),
                Err(e) => {
                    tracing::debug!(error = %e, "no clipboard change signal; reading it on every poll");
                    None
                }
            })
            .as_ref()
            .map(|c| c.load(Ordering::SeqCst))
    }

    fn start() -> Result<Arc<AtomicU64>, Box<dyn std::error::Error>> {
        let (conn, screen) = x11rb::connect(None)?;
        conn.xfixes_query_version(5, 0)?.reply()?;
        let root = conn.setup().roots[screen].root;
        let clipboard = conn.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
        conn.xfixes_select_selection_input(
            root,
            clipboard,
            SelectionEventMask::SET_SELECTION_OWNER
                | SelectionEventMask::SELECTION_WINDOW_DESTROY
                | SelectionEventMask::SELECTION_CLIENT_CLOSE,
        )?;
        conn.flush()?;

        let count = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&count);
        std::thread::Builder::new()
            .name("clipboard-watch".into())
            .spawn(move || {
                while let Ok(event) = conn.wait_for_event() {
                    if let Event::XfixesSelectionNotify(xfixes::SelectionNotifyEvent { .. }) = event
                    {
                        counter.fetch_add(1, Ordering::SeqCst);
                    }
                }
                // The display went away; the count stays put, which reads as "no
                // change" from now on. Nothing on this display can be copied either.
            })?;
        Ok(count)
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    pub fn change_count() -> Option<u64> {
        None
    }
}
