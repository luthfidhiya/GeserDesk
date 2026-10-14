// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows capture backend: low-level keyboard and mouse hooks.
//!
//! The same two modes as the X11 backend, switched by [`CaptureCmd`]:
//!
//! * **Local** -- the pointer belongs to this machine. We poll its position
//!   (`GetCursorPos` every couple of milliseconds) and report it so the server
//!   can notice a push against a screen edge. The hooks stay out of the way
//!   except for the screen-switch hotkey, `Ctrl+Alt+Home`, which they swallow.
//! * **Grabbed** -- another screen is active. The hooks swallow every mouse and
//!   keyboard event so nothing reaches local windows, and report them instead.
//!   The cursor is parked in the middle of a monitor, and since a swallowed
//!   move never reaches the cursor, every move event's position minus that
//!   centre is exactly the motion since the last one: no warping back and
//!   forth, unlike X11. A transparent window over the desktop hides the cursor.
//!
//! No DLL injection is needed: `WH_KEYBOARD_LL` and `WH_MOUSE_LL` run in the
//! thread that installed them, which must pump messages, so the backend thread
//! does nothing but that and poll. Hook procedures must answer within a few
//! hundred milliseconds or Windows skips them, so they only push onto a channel.
//!
//! **Limits** (Windows', not ours): a process that is not elevated receives no
//! input meant for an elevated window (UIPI), and nothing on the secure desktop
//! (UAC prompt, lock screen, `Ctrl+Alt+Del`) can be seen or taken over.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use geserdesk_proto::{Button, KeyAction, KeyEvent, ModMask, Point, Rect};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, error, info, warn};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, GetKeyboardLayout, ToUnicodeEx, HKL,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
    GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, MsgWaitForMultipleObjectsEx,
    PeekMessageW, RegisterClassExW, SetCursor, SetCursorPos, SetLayeredWindowAttributes,
    SetWindowPos, SetWindowsHookExW, ShowWindow, TranslateMessage, UnhookWindowsHookEx, HHOOK,
    HWND_TOPMOST, KBDLLHOOKSTRUCT, LWA_ALPHA, MA_NOACTIVATE, MSG, MSLLHOOKSTRUCT,
    MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, WH_KEYBOARD_LL,
    WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEACTIVATE, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP,
    WM_SETCURSOR, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::keymap::{hid_is_printable, win_key_to_hid};
use crate::{CaptureCmd, CaptureError, CaptureEvent, CaptureHandle};

/// How long the backend waits for input when there was nothing to do.
const IDLE_WAIT_MS: u32 = 2;

const WM_KEYUP: u32 = 0x0101;
const LLKHF_EXTENDED: u32 = 0x01;
/// Windows reports AltGr as a Right Alt press preceded by a made-up Left Ctrl
/// with this scan code. It is not a key anyone pressed.
const FAKE_LCONTROL_SCAN: u32 = 0x21D;

const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;
const VK_CAPITAL: i32 = 0x14;
const VK_HOME: u32 = 0x24;
const VK_LWIN: i32 = 0x5B;
const VK_RWIN: i32 = 0x5C;
const VK_NUMLOCK: i32 = 0x90;
const VK_LCONTROL: i32 = 0xA2;
const VK_LSHIFT: i32 = 0xA0;
const VK_RSHIFT: i32 = 0xA1;
const VK_RCONTROL: i32 = 0xA3;
const VK_LMENU: i32 = 0xA4;
const VK_RMENU: i32 = 0xA5;

/// Ids for the buttons a grab has swallowed the press of.
const BTN_LEFT: u8 = 1;
const BTN_MIDDLE: u8 = 2;
const BTN_RIGHT: u8 = 3;
const BTN_BACK: u8 = 8;
const BTN_FORWARD: u8 = 9;

/// Connect to the desktop and start the capture thread.
pub fn start(events: UnboundedSender<CaptureEvent>) -> Result<CaptureHandle, CaptureError> {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<Rect, String>>();
    std::thread::Builder::new()
        .name("windows-capture".into())
        .spawn(move || {
            // The hooks belong to the thread that installs them, so everything
            // is built here, not in `start`.
            match Capture::new(events, cmd_rx) {
                Ok(mut capture) => {
                    let _ = ready_tx.send(Ok(capture.bounds));
                    if let Err(e) = capture.run() {
                        error!(error = %e, "Windows capture thread stopped");
                    }
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })
        .map_err(|e| CaptureError::Connect(e.to_string()))?;

    let bounds = ready_rx
        .recv()
        .map_err(|_| CaptureError::Connect("the capture thread died while starting".into()))?
        .map_err(CaptureError::Connect)?;
    info!(?bounds, "Windows capture started (hotkey: Ctrl+Alt+Home)");
    Ok(CaptureHandle::new(bounds, cmd_tx))
}

/// What the hook procedures need. They are plain functions Windows calls on
/// the capture thread, so this lives in a thread-local; all of it is touched
/// only from that thread.
struct HookState {
    events: UnboundedSender<CaptureEvent>,
    grabbed: Cell<bool>,
    /// Where the cursor is parked while grabbed.
    center: Cell<(i32, i32)>,
    /// Keys whose press a grab swallowed (HID usage -> repeat count). A release
    /// of any other key began before the grab and belongs to the local desktop.
    keys: RefCell<HashMap<u16, u16>>,
    /// The same for mouse buttons.
    buttons: RefCell<HashSet<u8>>,
    /// The Home key of the hotkey chord is down (so its repeats and its release
    /// are swallowed too, and it is reported once).
    hotkey_down: Cell<bool>,
    /// The modifier keys held, one bit per HID usage 0xE0..=0xE7.
    held: Cell<u16>,
    /// Caps Lock and Num Lock as the grab sees them: a swallowed lock key does
    /// not toggle the local one, so the toggles are followed here.
    caps: Cell<bool>,
    num: Cell<bool>,
}

thread_local! {
    static STATE: RefCell<Option<Rc<HookState>>> = const { RefCell::new(None) };
}

fn state() -> Option<Rc<HookState>> {
    STATE.with(|s| s.try_borrow().ok().and_then(|s| s.clone()))
}

impl HookState {
    fn emit(&self, ev: CaptureEvent) {
        let _ = self.events.send(ev);
    }

    /// Returns `true` to swallow the event.
    fn on_mouse(&self, msg: u32, info: &MSLLHOOKSTRUCT) -> bool {
        if !self.grabbed.get() {
            return false;
        }
        let high = (info.mouseData >> 16) as u16;
        match msg {
            WM_MOUSEMOVE => {
                let (cx, cy) = self.center.get();
                let (dx, dy) = (info.pt.x - cx, info.pt.y - cy);
                if dx != 0 || dy != 0 {
                    self.emit(CaptureEvent::Motion { dx, dy });
                }
                true
            }
            WM_LBUTTONDOWN => self.button(Button::Left, BTN_LEFT, true),
            WM_LBUTTONUP => self.button(Button::Left, BTN_LEFT, false),
            WM_RBUTTONDOWN => self.button(Button::Right, BTN_RIGHT, true),
            WM_RBUTTONUP => self.button(Button::Right, BTN_RIGHT, false),
            WM_MBUTTONDOWN => self.button(Button::Middle, BTN_MIDDLE, true),
            WM_MBUTTONUP => self.button(Button::Middle, BTN_MIDDLE, false),
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                let down = msg == WM_XBUTTONDOWN;
                match high {
                    1 => self.button(Button::Back, BTN_BACK, down),
                    2 => self.button(Button::Forward, BTN_FORWARD, down),
                    _ => true,
                }
            }
            WM_MOUSEWHEEL => {
                self.emit(CaptureEvent::Wheel {
                    dx: 0,
                    dy: i32::from(high as i16),
                });
                true
            }
            WM_MOUSEHWHEEL => {
                self.emit(CaptureEvent::Wheel {
                    dx: i32::from(high as i16),
                    dy: 0,
                });
                true
            }
            _ => true,
        }
    }

    /// A press is swallowed and reported; so is its release, unless the press
    /// happened before the grab (a drag that carried the cursor to the edge),
    /// in which case the release must reach the local desktop or its button
    /// would stay down.
    fn button(&self, button: Button, id: u8, down: bool) -> bool {
        let mut held = self.buttons.borrow_mut();
        if down {
            held.insert(id);
        } else if !held.remove(&id) {
            return false;
        }
        drop(held);
        self.emit(CaptureEvent::Button { button, down });
        true
    }

    /// Returns `true` to swallow the event.
    fn on_key(&self, info: &KBDLLHOOKSTRUCT, msg: u32) -> bool {
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let extended = info.flags & LLKHF_EXTENDED != 0;
        let hid = win_key_to_hid(info.vkCode, info.scanCode as u16, extended);

        // What is held is tracked here, from every event, because a swallowed
        // key never reaches Windows' own key state (`GetAsyncKeyState` would
        // say nothing is held during a grab). The made-up Ctrl of AltGr is not
        // a key anyone pressed.
        if info.scanCode != FAKE_LCONTROL_SCAN {
            if let Some(bit) = hid.and_then(modifier_bit) {
                let held = self.held.get();
                self.held.set(if down { held | bit } else { held & !bit });
            }
        }

        // The hotkey works in both modes, and its keys never reach anyone.
        if info.vkCode == VK_HOME && extended {
            if down && self.ctrl_and_alt_held() {
                if !self.hotkey_down.replace(true) {
                    self.emit(CaptureEvent::Hotkey);
                }
                return true;
            }
            if self.hotkey_down.get() {
                if !down {
                    self.hotkey_down.set(false);
                }
                return true;
            }
        }

        if !self.grabbed.get() {
            return false;
        }
        if info.scanCode == FAKE_LCONTROL_SCAN {
            return true;
        }
        let Some(hid) = hid else {
            debug!(
                vk = info.vkCode,
                scan = info.scanCode,
                "unmapped key ignored"
            );
            return true;
        };

        let action = if down {
            let mut keys = self.keys.borrow_mut();
            match keys.get_mut(&hid) {
                Some(n) => {
                    *n = n.saturating_add(1);
                    KeyAction::Repeat(*n)
                }
                None => {
                    keys.insert(hid, 0);
                    // A lock key changes the state the next letters are typed in.
                    match hid {
                        0x39 => self.caps.set(!self.caps.get()),
                        0x53 => self.num.set(!self.num.get()),
                        _ => {}
                    }
                    KeyAction::Down
                }
            }
        } else if self.keys.borrow_mut().remove(&hid).is_some() {
            KeyAction::Up
        } else {
            return false; // pressed before the grab: its release is the desktop's
        };

        let ch = if hid_is_printable(hid) {
            self.char_for(info.vkCode, info.scanCode)
        } else {
            None
        };
        self.emit(CaptureEvent::Key(KeyEvent {
            hid,
            ch,
            mods: self.mods(),
            action,
        }));
        true
    }

    fn is_held(&self, hid: u16) -> bool {
        modifier_bit(hid).is_some_and(|bit| self.held.get() & bit != 0)
    }

    fn ctrl_and_alt_held(&self) -> bool {
        (self.is_held(0xE0) || self.is_held(0xE4)) && (self.is_held(0xE2) || self.is_held(0xE6))
    }

    /// Take the held modifiers and lock states from Windows itself. Only right
    /// while nothing is swallowed, so it is how drift (a release the hook never
    /// saw) is corrected between grabs.
    fn resync(&self) {
        let mut held = 0;
        for (vk, hid) in [
            (VK_LCONTROL, 0xE0),
            (VK_LSHIFT, 0xE1),
            (VK_LMENU, 0xE2),
            (VK_LWIN, 0xE3),
            (VK_RCONTROL, 0xE4),
            (VK_RSHIFT, 0xE5),
            (VK_RMENU, 0xE6),
            (VK_RWIN, 0xE7),
        ] {
            if async_down(vk) {
                held |= modifier_bit(hid).unwrap_or(0);
            }
        }
        self.held.set(held);
        self.caps.set(toggled(VK_CAPITAL));
        self.num.set(toggled(VK_NUMLOCK));
    }

    /// The modifiers held, the way the X11 backend reports its state.
    fn mods(&self) -> ModMask {
        let alt_gr = self.is_held(0xE6);
        let mut m = ModMask::empty();
        m.set(ModMask::SHIFT, self.is_held(0xE1) || self.is_held(0xE5));
        m.set(ModMask::CAPS_LOCK, self.caps.get());
        m.set(ModMask::CONTROL, self.is_held(0xE4) || self.is_held(0xE0));
        m.set(ModMask::ALT, self.is_held(0xE2));
        m.set(ModMask::NUM_LOCK, self.num.get());
        m.set(ModMask::SUPER, self.is_held(0xE3) || self.is_held(0xE7));
        m.set(ModMask::ALT_GR, alt_gr);
        m
    }

    /// The character a key types under the modifiers held now, in the layout of
    /// the focused window; `None` for dead keys and for keys that type nothing.
    ///
    /// Ctrl and Alt are left out (the server sends them as keys of their own and
    /// a Ctrl+A would otherwise come out as a control character), except that a
    /// held Right Alt stands for AltGr, which layouts use for `@`, `{` and so on.
    fn char_for(&self, vk: u32, scan: u32) -> Option<char> {
        // SAFETY: both calls take no pointers; with no foreground window the
        // layout of thread 0 is the current thread's.
        let layout = unsafe {
            GetKeyboardLayout(GetWindowThreadProcessId(
                GetForegroundWindow(),
                std::ptr::null_mut(),
            ))
        };
        let state = Typing {
            shift: self.is_held(0xE1) || self.is_held(0xE5),
            caps: self.caps.get(),
            num: self.num.get(),
            alt_gr: self.is_held(0xE6),
        };
        translate(vk, scan, layout, state)
    }
}

/// The modifiers a character is typed under.
#[derive(Debug, Clone, Copy, Default)]
struct Typing {
    shift: bool,
    caps: bool,
    num: bool,
    /// Right Alt, which stands for AltGr.
    alt_gr: bool,
}

/// What key `vk` (scan code `scan`) types in `layout` under `typing`: `None` for a
/// dead key (it types nothing until the next letter, which the other machine's own
/// layout would have to combine) and for keys that type no character.
///
/// AltGr is how a layout reaches `@`, `{`, `€`; Windows models it as Ctrl+Alt, so
/// that is what is asked. If that types nothing (a layout with no AltGr level, a
/// US one) the key is asked again without it.
fn translate(vk: u32, scan: u32, layout: HKL, typing: Typing) -> Option<char> {
    let attempt = |alt_gr: bool| -> Option<char> {
        let mut keys = [0u8; 256];
        if typing.shift {
            keys[VK_SHIFT as usize] = 0x80;
        }
        if typing.caps {
            keys[VK_CAPITAL as usize] = 1;
        }
        if typing.num {
            keys[VK_NUMLOCK as usize] = 1;
        }
        if alt_gr {
            keys[VK_CONTROL as usize] = 0x80;
            keys[VK_MENU as usize] = 0x80;
        }
        let mut buf = [0u16; 8];
        // Bit 2 of the flags (Windows 10 1607+): do not change the keyboard
        // state, so asking never disturbs a dead key waiting for its letter.
        // SAFETY: `keys` is 256 bytes and `buf` has the 8 units promised.
        let n = unsafe { ToUnicodeEx(vk, scan, keys.as_ptr(), buf.as_mut_ptr(), 8, 0x4, layout) };
        if n < 1 {
            return None; // dead key (-1), or nothing
        }
        let c = char::decode_utf16(buf[..n as usize].iter().copied())
            .next()?
            .ok()?;
        (!c.is_control()).then_some(c)
    };
    attempt(typing.alt_gr).or_else(|| if typing.alt_gr { attempt(false) } else { None })
}

/// The bit a modifier's HID usage (0xE0..=0xE7) has in [`HookState::held`].
fn modifier_bit(hid: u16) -> Option<u16> {
    (0xE0..=0xE7).contains(&hid).then(|| 1 << (hid - 0xE0))
}

fn async_down(vk: i32) -> bool {
    // SAFETY: GetAsyncKeyState has no preconditions.
    unsafe { GetAsyncKeyState(vk) as u16 & 0x8000 != 0 }
}

fn toggled(vk: i32) -> bool {
    // SAFETY: GetKeyState has no preconditions.
    unsafe { GetKeyState(vk) & 1 != 0 }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == 0 {
        if let Some(st) = state() {
            // SAFETY: for WH_MOUSE_LL with code HC_ACTION (0), lparam points at
            // a MSLLHOOKSTRUCT that lives for the duration of this call.
            let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
            if st.on_mouse(wparam as u32, info) {
                return 1;
            }
        }
    }
    // SAFETY: passing the hook arguments on unchanged.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == 0 {
        if let Some(st) = state() {
            // SAFETY: for WH_KEYBOARD_LL with code HC_ACTION (0), lparam points
            // at a KBDLLHOOKSTRUCT that lives for the duration of this call.
            let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            let msg = wparam as u32;
            if matches!(msg, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP)
                && st.on_key(info, msg)
            {
                return 1;
            }
        }
    }
    // SAFETY: passing the hook arguments on unchanged.
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// The window that hides the cursor while grabbed: it covers the desktop, is
/// all but invisible, and answers `WM_SETCURSOR` with "no cursor".
unsafe extern "system" fn cursor_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_SETCURSOR => {
            // SAFETY: a null cursor hides it; no preconditions.
            unsafe { SetCursor(std::ptr::null_mut()) };
            1
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        // SAFETY: the arguments are the ones Windows gave this procedure.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn virtual_screen() -> Rect {
    // SAFETY: GetSystemMetrics has no preconditions.
    unsafe {
        Rect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

fn cursor_pos() -> Option<Point> {
    let mut p = POINT { x: 0, y: 0 };
    // SAFETY: `p` is a valid POINT.
    (unsafe { GetCursorPos(&mut p) } != 0).then(|| Point::new(p.x, p.y))
}

/// The middle of the monitor `p` is on, where the cursor is parked: the middle
/// of the whole desktop can lie between monitors, where no cursor can be.
fn monitor_center(p: Point) -> Point {
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    // SAFETY: `info` is a valid, correctly sized MONITORINFO.
    let ok = unsafe {
        let monitor = MonitorFromPoint(POINT { x: p.x, y: p.y }, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, &mut info)
    };
    if ok != 0 {
        let r = info.rcMonitor;
        Rect::new(r.left, r.top, r.right - r.left, r.bottom - r.top).center()
    } else {
        virtual_screen().center()
    }
}

struct Capture {
    bounds: Rect,
    events: UnboundedSender<CaptureEvent>,
    cmds: Receiver<CaptureCmd>,
    state: Rc<HookState>,
    mouse_hook: HHOOK,
    keyboard_hook: HHOOK,
    window: HWND,
    last_local: Option<Point>,
}

impl Capture {
    fn new(
        events: UnboundedSender<CaptureEvent>,
        cmds: Receiver<CaptureCmd>,
    ) -> Result<Self, String> {
        let bounds = virtual_screen();
        if bounds.w <= 0 || bounds.h <= 0 {
            return Err("the desktop reports no size".into());
        }
        let state = Rc::new(HookState {
            events: events.clone(),
            grabbed: Cell::new(false),
            center: Cell::new((bounds.center().x, bounds.center().y)),
            keys: RefCell::new(HashMap::new()),
            buttons: RefCell::new(HashSet::new()),
            hotkey_down: Cell::new(false),
            held: Cell::new(0),
            caps: Cell::new(false),
            num: Cell::new(false),
        });
        state.resync();
        STATE.with(|s| *s.borrow_mut() = Some(Rc::clone(&state)));

        // SAFETY: a null module name asks for this executable's handle.
        let hinstance = unsafe { GetModuleHandleW(std::ptr::null()) };
        // SAFETY: the procedures are `extern "system"` with the hook signature;
        // a null `hmod` and thread 0 make them global hooks that run on this
        // thread (low-level hooks need no DLL).
        let mouse_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hinstance, 0) };
        if mouse_hook.is_null() {
            return Err("cannot install the mouse hook".into());
        }
        // SAFETY: as above.
        let keyboard_hook =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hinstance, 0) };
        if keyboard_hook.is_null() {
            // SAFETY: the hook was just installed.
            unsafe { UnhookWindowsHookEx(mouse_hook) };
            return Err("cannot install the keyboard hook".into());
        }

        let window = create_cursor_window(hinstance);
        if window.is_null() {
            warn!("cannot create the cursor-hiding window; the cursor stays visible while grabbed");
        }

        Ok(Self {
            bounds,
            events,
            cmds,
            state,
            mouse_hook,
            keyboard_hook,
            window,
            last_local: None,
        })
    }

    fn run(&mut self) -> Result<(), String> {
        loop {
            loop {
                match self.cmds.try_recv() {
                    Ok(cmd) => self.on_cmd(cmd),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        // The server dropped its handle: give the devices back.
                        if self.state.grabbed.get() {
                            self.release(None);
                        }
                        return Ok(());
                    }
                }
            }
            if self.events.is_closed() {
                if self.state.grabbed.get() {
                    self.release(None);
                }
                return Ok(());
            }

            // Hooks are called from inside the message functions, so this is
            // what services them.
            let mut busy = false;
            // SAFETY: `msg` is a valid MSG for each call.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    busy = true;
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }

            if !self.state.grabbed.get() {
                self.state.resync();
                if let Some(pos) = cursor_pos() {
                    if self.last_local != Some(pos) {
                        self.last_local = Some(pos);
                        busy = true;
                        let _ = self.events.send(CaptureEvent::Pointer(pos));
                    }
                }
            }

            if !busy {
                // SAFETY: no handles are waited on; this only waits for input
                // or the timeout, and services hooks while it does.
                unsafe {
                    MsgWaitForMultipleObjectsEx(
                        0,
                        std::ptr::null(),
                        IDLE_WAIT_MS,
                        QS_ALLINPUT,
                        MWMO_INPUTAVAILABLE,
                    );
                }
            }
        }
    }

    fn on_cmd(&mut self, cmd: CaptureCmd) {
        match cmd {
            CaptureCmd::Grab => {
                if self.state.grabbed.get() {
                    return;
                }
                if let Err(e) = self.grab() {
                    warn!(error = %e, "grab failed");
                    let _ = self.events.send(CaptureEvent::GrabFailed);
                }
            }
            CaptureCmd::Ungrab { warp } => self.release(Some(warp)),
        }
    }

    fn grab(&mut self) -> Result<(), String> {
        self.state.resync();
        let here = cursor_pos().unwrap_or_else(|| self.bounds.center());
        let center = monitor_center(here);
        // SAFETY: SetCursorPos takes plain coordinates.
        if unsafe { SetCursorPos(center.x, center.y) } == 0 {
            return Err("cannot move the cursor (a secure desktop is showing?)".into());
        }
        self.state.center.set((center.x, center.y));
        self.state.keys.borrow_mut().clear();
        self.state.buttons.borrow_mut().clear();
        if !self.window.is_null() {
            let r = virtual_screen();
            // SAFETY: `window` is the window this thread created.
            unsafe {
                SetWindowPos(
                    self.window,
                    HWND_TOPMOST,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }
        self.state.grabbed.set(true);
        info!("grabbed pointer and keyboard");
        Ok(())
    }

    fn release(&mut self, warp_to: Option<Point>) {
        self.state.grabbed.set(false);
        self.state.keys.borrow_mut().clear();
        self.state.buttons.borrow_mut().clear();
        if !self.window.is_null() {
            // SAFETY: `window` is the window this thread created.
            unsafe { ShowWindow(self.window, SW_HIDE) };
        }
        if let Some(p) = warp_to {
            let p = self.bounds.clamp(p);
            // SAFETY: SetCursorPos takes plain coordinates.
            unsafe { SetCursorPos(p.x, p.y) };
            self.last_local = Some(p);
        }
        debug!("released pointer and keyboard");
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // SAFETY: these are the hooks and window this thread created.
        unsafe {
            UnhookWindowsHookEx(self.mouse_hook);
            UnhookWindowsHookEx(self.keyboard_hook);
            if !self.window.is_null() {
                DestroyWindow(self.window);
            }
        }
        STATE.with(|s| *s.borrow_mut() = None);
    }
}

fn create_cursor_window(hinstance: windows_sys::Win32::Foundation::HINSTANCE) -> HWND {
    let class = wide("GeserDeskCaptureCursor");
    let class_def = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: Some(cursor_window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
        hIconSm: std::ptr::null_mut(),
    };
    // SAFETY: `class_def` is complete and `class` outlives the calls. A second
    // registration (another capture in this process) fails harmlessly: the
    // class already exists.
    unsafe {
        RegisterClassExW(&class_def);
        let r = virtual_screen();
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            class.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            r.x,
            r.y,
            r.w,
            r.h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        );
        if !hwnd.is_null() {
            // Alpha 1 of 255: invisible, but still something the cursor is over
            // (a fully transparent layered window is click-through).
            SetLayeredWindowAttributes(hwnd, 0, 1, LWA_ALPHA);
        }
        hwnd
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use tokio::sync::mpsc::UnboundedReceiver;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
        KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_MOVE,
        MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    };

    fn send_mouse(dx: i32, dy: i32, data: u32, flags: u32) {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        // SAFETY: one valid INPUT of the size passed.
        unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
    }

    /// A key as the keyboard itself would send it: by scan code.
    fn send_key(scan: u16, extended: bool, up: bool) {
        let mut flags = KEYEVENTF_SCANCODE;
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if up {
            flags |= KEYEVENTF_KEYUP;
        }
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        // SAFETY: one valid INPUT of the size passed.
        unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
    }

    fn drain(rx: &mut UnboundedReceiver<CaptureEvent>, wait: Duration) -> Vec<CaptureEvent> {
        let end = Instant::now() + wait;
        let mut out = Vec::new();
        while Instant::now() < end {
            while let Ok(ev) = rx.try_recv() {
                out.push(ev);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        out
    }

    /// Takes over this desktop's real keyboard and mouse for about a second and
    /// drives them with `SendInput`, so run it on purpose, not by accident:
    /// `cargo test -p geserdesk-platform -- --ignored --test-threads=1`. What it
    /// sends is swallowed by the grab, so it changes nothing on the desktop.
    #[test]
    #[ignore = "grabs the real keyboard and mouse for a moment"]
    fn a_grab_reports_input_and_the_hotkey_ends_it() {
        crate::monitors::init_process();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = start(tx).expect("hooks install");
        let before = cursor_pos().unwrap();

        // Local mode: the pointer is reported, nothing is swallowed.
        let p = Point::new(before.x + 3, before.y + 2);
        unsafe { SetCursorPos(p.x, p.y) };
        let seen = drain(&mut rx, Duration::from_millis(200));
        assert!(
            seen.contains(&CaptureEvent::Pointer(p)),
            "the local pointer move was not reported: {seen:?}"
        );

        // Grabbed: motion arrives as deltas that do not pile up.
        assert!(handle.send(CaptureCmd::Grab));
        let _ = drain(&mut rx, Duration::from_millis(150));
        for _ in 0..12 {
            send_mouse(10, 0, 0, MOUSEEVENTF_MOVE);
            std::thread::sleep(Duration::from_millis(15));
        }
        send_mouse(0, -6, 0, MOUSEEVENTF_MOVE);
        std::thread::sleep(Duration::from_millis(15));
        let seen = drain(&mut rx, Duration::from_millis(200));
        let motions: Vec<(i32, i32)> = seen
            .iter()
            .filter_map(|e| match e {
                CaptureEvent::Motion { dx, dy } => Some((*dx, *dy)),
                _ => None,
            })
            .collect();
        assert!(motions.len() >= 10, "motion missing: {seen:?}");
        let max_dx = motions.iter().map(|m| m.0).max().unwrap();
        assert!(
            (1..=60).contains(&max_dx),
            "deltas should be one step each, not a running total: {motions:?}"
        );
        assert!(
            motions.iter().any(|m| m.1 < 0),
            "upward move lost: {motions:?}"
        );
        let parked = cursor_pos().unwrap();
        assert_eq!(parked, monitor_center(p), "the cursor must stay parked");

        // A button, the wheel, a key and its release, auto-repeat, a modifier.
        send_mouse(0, 0, 1, MOUSEEVENTF_XDOWN);
        send_mouse(0, 0, 1, MOUSEEVENTF_XUP);
        send_mouse(0, 0, 120, MOUSEEVENTF_WHEEL);
        send_mouse(0, 0, 120, MOUSEEVENTF_HWHEEL);
        send_key(0x1E, false, false); // A
        send_key(0x1E, false, false); // held: a repeat
        send_key(0x1E, false, true);
        send_key(0x2A, false, false); // Left Shift
        send_key(0x2A, false, true);
        send_key(0x4F, true, false); // End
        send_key(0x4F, true, true);
        let seen = drain(&mut rx, Duration::from_millis(250));
        let has = |want: &CaptureEvent| seen.contains(want);
        assert!(
            has(&CaptureEvent::Button {
                button: Button::Back,
                down: true
            }),
            "{seen:?}"
        );
        assert!(
            has(&CaptureEvent::Button {
                button: Button::Back,
                down: false
            }),
            "{seen:?}"
        );
        assert!(has(&CaptureEvent::Wheel { dx: 0, dy: 120 }), "{seen:?}");
        assert!(has(&CaptureEvent::Wheel { dx: 120, dy: 0 }), "{seen:?}");
        let keys: Vec<&KeyEvent> = seen
            .iter()
            .filter_map(|e| match e {
                CaptureEvent::Key(k) => Some(k),
                _ => None,
            })
            .collect();
        let a: Vec<_> = keys.iter().filter(|k| k.hid == 0x04).collect();
        assert_eq!(a.len(), 3, "A down, repeat, up: {keys:?}");
        assert_eq!(a[0].action, KeyAction::Down);
        assert_eq!(a[1].action, KeyAction::Repeat(1));
        assert_eq!(a[2].action, KeyAction::Up);
        // Which letter depends on the layout; that there is one does not.
        assert!(a[0].ch.is_some_and(char::is_alphabetic), "{:?}", a[0]);
        assert!(keys
            .iter()
            .any(|k| k.hid == 0xE1 && k.action == KeyAction::Down));
        let end: Vec<_> = keys.iter().filter(|k| k.hid == 0x4D).collect();
        assert_eq!(
            end.len(),
            2,
            "End is the navigation key, not keypad 1: {keys:?}"
        );
        assert_eq!(end[0].ch, None);

        // The hotkey is swallowed and reported once, however long it is held.
        send_key(0x1D, false, false); // Left Ctrl
        send_key(0x38, false, false); // Left Alt
        send_key(0x47, true, false); // Home
        send_key(0x47, true, false); // Home, repeating
        send_key(0x47, true, true);
        send_key(0x38, false, true);
        send_key(0x1D, false, true);
        let seen = drain(&mut rx, Duration::from_millis(250));
        let hotkeys = seen.iter().filter(|e| **e == CaptureEvent::Hotkey).count();
        assert_eq!(hotkeys, 1, "{seen:?}");
        assert!(
            !seen
                .iter()
                .any(|e| matches!(e, CaptureEvent::Key(k) if k.hid == 0x4A)),
            "Home of the hotkey must not be forwarded: {seen:?}"
        );

        // Ungrab puts the cursor where it was told and hands the devices back.
        assert!(handle.send(CaptureCmd::Ungrab { warp: before }));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(cursor_pos().unwrap(), before);
        let _ = drain(&mut rx, Duration::from_millis(100));
        send_key(0x4F, true, false);
        send_key(0x4F, true, true); // End reaches the desktop now: no event
        let seen = drain(&mut rx, Duration::from_millis(150));
        assert!(
            !seen.iter().any(|e| matches!(e, CaptureEvent::Key(_))),
            "after the release nothing is captured: {seen:?}"
        );
    }

    /// A touchpad scrolls in steps smaller than a notch, and either way. The server
    /// forwards them as they are; the client rounds a small one up to a notch.
    #[test]
    #[ignore = "grabs the real keyboard and mouse for a moment"]
    fn smooth_scrolling_passes_through_in_small_and_negative_steps() {
        crate::monitors::init_process();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = start(tx).expect("hooks install");
        assert!(handle.send(CaptureCmd::Grab));
        let _ = drain(&mut rx, Duration::from_millis(150));

        send_mouse(0, 0, 30, MOUSEEVENTF_WHEEL); // a quarter notch up
        send_mouse(0, 0, (-60i32) as u32, MOUSEEVENTF_WHEEL); // half a notch down
        send_mouse(0, 0, (-240i32) as u32, MOUSEEVENTF_WHEEL); // two notches down at once
        send_mouse(0, 0, (-30i32) as u32, MOUSEEVENTF_HWHEEL); // a little to the left
        let seen = drain(&mut rx, Duration::from_millis(250));
        let wheels: Vec<(i32, i32)> = seen
            .iter()
            .filter_map(|e| match e {
                CaptureEvent::Wheel { dx, dy } => Some((*dx, *dy)),
                _ => None,
            })
            .collect();
        assert_eq!(
            wheels,
            vec![(0, 30), (0, -60), (0, -240), (-30, 0)],
            "{seen:?}"
        );

        assert!(handle.send(CaptureCmd::Ungrab {
            warp: cursor_pos().unwrap()
        }));
        std::thread::sleep(Duration::from_millis(100));
    }

    /// What a key types depends on the layout, and on more than shift: AltGr
    /// reaches `@` and `€` on a German or French layout, and a dead key types
    /// nothing by itself. `ToUnicodeEx` takes the layout as an argument, so this
    /// needs no change to the keyboard the person is using.
    #[test]
    #[ignore = "loads keyboard layouts into this process"]
    fn a_key_types_what_its_layout_says_including_altgr_and_dead_keys() {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            LoadKeyboardLayoutW, MapVirtualKeyExW,
        };
        const NO_SHELL: u32 = 0x80; // KLF_NOTELLSHELL: do not announce it
        const SCAN_TO_VK: u32 = 3; // MAPVK_VSC_TO_VK_EX

        let plain = Typing::default();
        let shift = Typing {
            shift: true,
            ..plain
        };
        let alt_gr = Typing {
            alt_gr: true,
            ..plain
        };
        // (layout id, what it is, [(scan code, typing, expected)])
        #[allow(clippy::type_complexity)]
        let cases: Vec<(&str, &str, Vec<(u32, Typing, Option<char>)>)> = vec![
            (
                "00000409",
                "US",
                vec![
                    (0x1E, plain, Some('a')),
                    (0x1E, shift, Some('A')),
                    (0x02, shift, Some('!')),
                    (0x28, plain, Some('\'')),
                    // No AltGr level: asked again without it.
                    (0x10, alt_gr, Some('q')),
                ],
            ),
            (
                "00000407",
                "German",
                vec![
                    (0x2C, plain, Some('y')), // where Z is on a US keyboard
                    (0x15, plain, Some('z')),
                    (0x03, shift, Some('"')),
                    (0x10, alt_gr, Some('@')),
                    (0x12, alt_gr, Some('€')),
                    (0x08, alt_gr, Some('{')),
                    (0x1A, plain, Some('ü')),
                    (0x0D, plain, None), // the acute accent: a dead key
                ],
            ),
            (
                "0000040C",
                "French",
                vec![
                    (0x10, plain, Some('a')), // where Q is on a US keyboard
                    (0x1E, plain, Some('q')),
                    (0x02, plain, Some('&')), // the digit row needs shift
                    (0x02, shift, Some('1')),
                    (0x0B, alt_gr, Some('@')),
                    (0x1A, plain, None), // the circumflex: a dead key
                ],
            ),
            (
                "00020409",
                "US International",
                vec![
                    (0x1E, plain, Some('a')),
                    (0x1E, alt_gr, Some('á')),
                    (0x28, plain, None), // the apostrophe is a dead key here
                ],
            ),
        ];

        let mut checked = 0;
        for (id, name, keys) in cases {
            let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: a NUL-terminated layout id.
            let layout = unsafe { LoadKeyboardLayoutW(wide.as_ptr(), NO_SHELL) };
            if layout.is_null() {
                eprintln!("the {name} layout is not available here; skipped");
                continue;
            }
            for (scan, typing, want) in keys {
                // The virtual key the layout gives that scan code: what the hook
                // would be handed with this layout active.
                // SAFETY: plain arguments.
                let vk = unsafe { MapVirtualKeyExW(scan, SCAN_TO_VK, layout) };
                let got = translate(vk, scan, layout, typing);
                assert_eq!(got, want, "{name}, scan {scan:#x}, vk {vk:#x}, {typing:?}");
                checked += 1;
            }
        }
        assert!(checked > 0, "no layout could be loaded at all");
        eprintln!("{checked} key/layout checks");
    }

    #[test]
    #[ignore = "installs real low-level hooks for a moment"]
    fn dropping_the_handle_removes_the_hooks() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = start(tx).expect("hooks install");
        assert!(handle.bounds().w > 0);
        drop(handle);
        // The thread notices within a poll or two; a second capture must be
        // able to start (and install its own hooks) right after.
        std::thread::sleep(Duration::from_millis(100));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        assert!(start(tx).is_ok());
    }
}
