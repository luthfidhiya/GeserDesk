// SPDX-License-Identifier: GPL-3.0-or-later
//! Enumerate this machine's monitors.
//!
//! The server needs the real arrangement of each machine's monitors to place
//! them against one another, so both ends call [`monitors`]: the client to fill
//! in its `Hello`, the server for its own screen. Coordinates are the OS's own
//! desktop coordinates (primary monitor at the origin, secondary monitors may
//! have negative positions).

use geserdesk_proto::Rect;

use crate::CaptureError;

/// Prepare the process for accurate screen measurements. Call once at startup,
/// before [`monitors`] or any input injection.
///
/// On Windows a process that is not DPI-aware is lied to about monitor sizes
/// (a 4K screen at 150 % scale reports 2560x1440), which would make the layout
/// editor and cursor placement disagree with the real pixels. The low-level mouse
/// hook always reports real pixels, so a server that is not aware reads the gap
/// between them as motion: at 125 % every move is off by a fifth of the screen
/// and the other computer's cursor runs into a corner. Per-monitor awareness
/// keeps them equal on every monitor, whatever its scale. No-op elsewhere.
pub fn init_process() {
    #[cfg(windows)]
    // SAFETY: both take plain values and have no preconditions; they fail
    // harmlessly when the awareness is already set.
    unsafe {
        use windows_sys::Win32::UI::HiDpi::{
            SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        };
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
            windows_sys::Win32::UI::WindowsAndMessaging::SetProcessDPIAware();
        }
    }
}

/// Every monitor of the desktop, sorted left-to-right then top-to-bottom.
/// Never returns an empty list on success.
pub fn monitors() -> Result<Vec<Rect>, CaptureError> {
    let mut list = platform_monitors()?;
    list.retain(|r| r.w > 0 && r.h > 0);
    if list.is_empty() {
        return Err(CaptureError::Connect("no monitors reported".into()));
    }
    list.sort_by_key(|r| (r.x, r.y));
    Ok(list)
}

/// The monitors RandR 1.5 reports on `root`, or `None` without it.
#[cfg(target_os = "linux")]
pub(crate) fn randr_monitors(
    conn: &impl x11rb::connection::Connection,
    root: x11rb::protocol::xproto::Window,
) -> Option<Vec<Rect>> {
    use x11rb::protocol::randr::ConnectionExt as _;

    conn.randr_get_monitors(root, true)
        .ok()
        .and_then(|c| c.reply().ok())
        .map(|r| {
            r.monitors
                .iter()
                .map(|m| {
                    Rect::new(
                        i32::from(m.x),
                        i32::from(m.y),
                        i32::from(m.width),
                        i32::from(m.height),
                    )
                })
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
}

#[cfg(target_os = "linux")]
fn platform_monitors() -> Result<Vec<Rect>, CaptureError> {
    use x11rb::connection::Connection;

    let (conn, screen_num) =
        x11rb::connect(None).map_err(|e| CaptureError::Connect(e.to_string()))?;
    let screen = &conn.setup().roots[screen_num];

    // Without RandR 1.5 the whole root window is the best we can say.
    Ok(randr_monitors(&conn, screen.root).unwrap_or_else(|| {
        vec![Rect::new(
            0,
            0,
            i32::from(screen.width_in_pixels),
            i32::from(screen.height_in_pixels),
        )]
    }))
}

#[cfg(windows)]
fn platform_monitors() -> Result<Vec<Rect>, CaptureError> {
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    };

    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _dc: HDC,
        _clip: *mut RECT,
        data: LPARAM,
    ) -> i32 {
        // SAFETY: `data` is the `&mut Vec<Rect>` passed to EnumDisplayMonitors
        // below, which outlives the call and is only touched on this thread.
        let out = unsafe { &mut *(data as *mut Vec<Rect>) };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = size_of::<MONITORINFO>() as u32;
        // SAFETY: `info` is a valid, correctly sized MONITORINFO.
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            let r = info.rcMonitor;
            out.push(Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top));
        }
        1 // continue enumerating
    }

    let mut out: Vec<Rect> = Vec::new();
    // SAFETY: the callback only dereferences `data` as the Vec above.
    let ok = unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(collect),
            &mut out as *mut Vec<Rect> as LPARAM,
        )
    };
    if ok == 0 {
        return Err(CaptureError::Connect("EnumDisplayMonitors failed".into()));
    }
    Ok(out)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn platform_monitors() -> Result<Vec<Rect>, CaptureError> {
    Err(CaptureError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a display; skipped (not failed) on headless CI.
    #[test]
    fn lists_at_least_one_monitor_when_a_display_exists() {
        match monitors() {
            Ok(list) => {
                assert!(!list.is_empty());
                assert!(list.iter().all(|r| r.w > 0 && r.h > 0));
            }
            Err(e) => eprintln!("no display available, skipping: {e}"),
        }
    }
}
