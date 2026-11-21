//! The hover window: one always-on-top, transparent tool window that is hidden until the summon hotkey.
//! It owns one Dock session at a time. Rust captures the previous foreground window at the keypress,
//! places the Dock while hidden, activates it with a checked fallback ladder, restores focus on an
//! explicit dismissal and leaves the user's chosen focus alone on blur. The page measures its CSS
//! height; Rust keeps the anchor, the monitor work area and the DPI conversion.

use serde::Serialize;
use std::sync::{
    Mutex, MutexGuard,
    atomic::{AtomicU64, Ordering},
};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const LABEL: &str = "hover";
/// Sent to every window with a [`Snapshot`] whenever one of its fields changes.
pub const STATE_EVENT: &str = "relay://state";

/// The edge tab's size in logical pixels before the page measures it (design/redesign/overlay-b-capsule.html).
const TAB_W: f64 = 34.0;
const TAB_H: f64 = 112.0;

/* ── Dock domain: pure, platform-independent ───────────────────────────────────────────────────
Rust owns the bottom-centre anchor, the monitor work area and the DPI conversion. The page only
sends a measured CSS height. Keeping the decisions here makes them testable without Win32 and
keeps reentrant Win32 calls out of the state lock. */

/// Preferred Dock width and bottom margin in CSS pixels (prototype, SPEC-v2 "Dock + Lens").
pub const DOCK_WIDTH_CSS: f64 = 760.0;
pub const DOCK_MARGIN_CSS: f64 = 16.0;

/// A rectangle in physical pixels. Coordinates are signed so monitors left of or above the primary
/// one keep their real position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// A monitor's physical work area, excluding taskbars and other docked bars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Ceil with a tolerance: `16.0 * 1.25` lands a hair above 20 in binary floating point and must not
/// become 21 physical pixels.
fn ceil_css(css: f64, scale: f64) -> i32 {
    let scaled = css * scale;
    let up = scaled.ceil();
    if scaled > 0.0 && up - scaled > 1.0 - 1e-9 {
        up as i32 - 1
    } else {
        up as i32
    }
}

/// The one rectangle used for every Dock size. Grow and shrink both go through this so the bottom
/// edge stays at `work.bottom - margin` and the width is centred on signed coordinates. Width and
/// height are capped to the work area; the page scrolls inside its CSS viewport past the cap.
pub fn dock_rect(area: WorkArea, scale: f64, css_width: f64, css_height: f64) -> PhysRect {
    let margin = ceil_css(DOCK_MARGIN_CSS, scale).min(area.height.max(0));
    let width = ceil_css(css_width, scale).min(area.width.max(0));
    let height = ceil_css(css_height, scale).min((area.height - margin).max(0));
    PhysRect {
        x: area.x + (area.width - width) / 2,
        y: area.y + area.height - margin - height,
        width,
        height,
    }
}

/// Identity captured at the keypress. An HWND can be reused, so the thread and process ids decide
/// whether the saved window is still the one the user was in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviousWindow {
    pub hwnd: isize,
    pub pid: u32,
    pub tid: u32,
}

/// Why the session ended. Blur is the only path where the user has already chosen the next focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dismissal {
    Escape,
    Saved,
    Toggle,
    Blur,
}

/// The saved window is restored only for an explicit dismissal, only while the Dock still owns
/// foreground, and only if the saved window is still valid with matching identity. Blur and stale
/// or invalid targets are discarded without a second focus grab.
pub fn restore_target(
    previous: Option<PreviousWindow>,
    how: Dismissal,
    dock_owned_foreground: bool,
    previous_still_valid: bool,
) -> Option<PreviousWindow> {
    if how == Dismissal::Blur {
        return None;
    }
    let target = previous?;
    (dock_owned_foreground && previous_still_valid).then_some(target)
}

/// Queued resize and dismissal work must carry the session id it was scheduled for.
pub fn session_current(active: Option<u64>, incoming: u64) -> bool {
    active == Some(incoming)
}

/// A delayed activation attempt may only act while the foreground is still a window this session
/// knows about: the Dock itself, or the window saved at the keypress. A click to any other app
/// terminates the attempt instead of stealing focus back.
pub fn deferred_attempt_allowed(
    foreground: Option<PreviousWindow>,
    dock: isize,
    previous: Option<PreviousWindow>,
) -> bool {
    match foreground {
        Some(front) => front.hwnd == dock || previous.is_some_and(|saved| saved == front),
        None => false,
    }
}

/// `SHQueryUserNotificationState` is polled on every summon. Only exclusive Direct3D full screen
/// (3) suppresses the Dock; `QUNS_BUSY` (2) and every other state still activate. A failed query is
/// reported as -1 and also activates.
pub fn suppress_for_notification_state(state: i32) -> bool {
    state == 3
}

struct Flags {
    pinned: bool,
    dnd: bool,
    paused: bool,
    /// Mirrored from the Dock session so both windows can read the same state.
    ready: bool,
    visible: bool,
}

impl Flags {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            pinned: self.pinned,
            dnd: self.dnd,
            paused: self.paused,
            ready: self.ready,
            visible: self.visible,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub pinned: bool,
    pub dnd: bool,
    pub paused: bool,
    pub ready: bool,
    pub visible: bool,
}

/// The Dock-only focus/session signal. The page focuses the Composer only when native activation
/// was verified, and shows a click hint when the fallback ladder ran out.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockFocus {
    pub session_id: u64,
    pub visible: bool,
    pub focused: bool,
    pub hint: Option<&'static str>,
}

/// The session whose monitor geometry changed; the page remeasures its CSS height for the new DPI.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DockDpi {
    pub session_id: u64,
}

pub struct HoverState {
    flags: Mutex<Flags>,
}

impl Default for HoverState {
    fn default() -> Self {
        Self {
            flags: Mutex::new(Flags {
                pinned: false,
                dnd: false,
                paused: false,
                ready: false,
                visible: false,
            }),
        }
    }
}

impl HoverState {
    /// A panic elsewhere must not take the hover down with it, so a poisoned lock is still used.
    fn lock(&self) -> MutexGuard<'_, Flags> {
        self.flags.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn snapshot(&self) -> Snapshot {
        self.lock().snapshot()
    }

    fn update(&self, app: &AppHandle, change: impl FnOnce(&mut Flags)) -> Snapshot {
        let after = {
            let mut f = self.lock();
            change(&mut f);
            f.snapshot()
        };
        let _ = app.emit(STATE_EVENT, after);
        after
    }

    pub fn set_pinned(&self, app: &AppHandle, pinned: bool) -> Snapshot {
        self.update(app, |f| f.pinned = pinned)
    }

    pub fn set_dnd(&self, app: &AppHandle, dnd: bool) -> Snapshot {
        self.update(app, |f| f.dnd = dnd)
    }

    pub fn set_paused(&self, app: &AppHandle, paused: bool) -> Snapshot {
        self.update(app, |f| f.paused = paused)
    }

    /// Mirrors readiness and session visibility for the pages that read `relay://state`.
    pub fn set_dock_status(&self, app: &AppHandle, ready: bool, visible: bool) -> Snapshot {
        self.update(app, |f| {
            f.ready = ready;
            f.visible = visible;
        })
    }
}

/// One Dock session. The geometry and the saved focus target are copied out of the lock before any
/// Win32 call, so a reentrant focus or size event can never observe a half-updated session.
#[derive(Clone, Copy)]
struct Session {
    id: u64,
    dock: isize,
    previous: Option<PreviousWindow>,
    area: WorkArea,
    scale: f64,
    rect: PhysRect,
    css_height: f64,
    activated: bool,
}

#[derive(Default)]
struct DockInner {
    ready: bool,
    compact_height: f64,
    session: Option<Session>,
}

pub struct DockRuntime {
    inner: Mutex<DockInner>,
    next_id: AtomicU64,
}

impl Default for DockRuntime {
    fn default() -> Self {
        Self {
            inner: Mutex::new(DockInner {
                ready: false,
                compact_height: 0.0,
                session: None,
            }),
            next_id: AtomicU64::new(1),
        }
    }
}

impl DockRuntime {
    fn lock(&self) -> MutexGuard<'_, DockInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn new_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// The page calls this once after registering its listeners and measuring the compact height.
    fn ready(&self, height: f64) {
        let mut inner = self.lock();
        inner.ready = true;
        inner.compact_height = height.max(1.0);
    }

    fn is_ready(&self) -> bool {
        self.lock().ready
    }

    fn session_id(&self) -> Option<u64> {
        self.lock().session.map(|s| s.id)
    }

    fn session(&self) -> Option<Session> {
        self.lock().session
    }

    fn start(&self, session: Session) {
        self.lock().session = Some(session);
    }

    /// Returns the session only when `id` is still the live one; otherwise queued work is dropped.
    fn take(&self, id: u64) -> Option<Session> {
        let mut inner = self.lock();
        if inner.session.map(|s| s.id) == Some(id) {
            inner.session.take()
        } else {
            None
        }
    }

    fn rect_for(&self, id: u64, css_height: f64) -> Option<(Session, PhysRect)> {
        let mut inner = self.lock();
        let session = inner.session.as_mut()?;
        if session.id != id {
            return None;
        }
        session.css_height = css_height;
        session.rect = dock_rect(session.area, session.scale, DOCK_WIDTH_CSS, css_height);
        Some((*session, session.rect))
    }

    fn mark_activated(&self, id: u64) {
        let mut inner = self.lock();
        if let Some(session) = inner.session.as_mut() {
            if session.id == id {
                session.activated = true;
            }
        }
    }

    /// Re-read work area and scale after a DPI change and recompute the anchored rectangle from
    /// the last measured CSS height, before the page sends a new measurement.
    fn refresh_geometry(&self, id: u64, area: WorkArea, scale: f64) -> Option<(Session, PhysRect)> {
        let mut inner = self.lock();
        let session = inner.session.as_mut()?;
        if session.id != id {
            return None;
        }
        session.area = area;
        session.scale = scale;
        session.rect = dock_rect(area, scale, DOCK_WIDTH_CSS, session.css_height);
        Some((*session, session.rect))
    }
}

/// Builds the Dock hidden at startup. It stays hidden until the hotkey summons it; the page reports
/// its measured height with `dock_ready` before any summon is allowed.
pub fn build(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window(LABEL) {
        return Ok(window);
    }
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("hover.html".into()))
        .title("Relay hover")
        .inner_size(TAB_W, TAB_H);
    if let Ok(Some(monitor)) = app.primary_monitor() {
        // The work area is in physical pixels; the builder takes logical ones.
        let area = monitor.work_area();
        let scale = monitor.scale_factor();
        let right = (area.position.x as f64 + area.size.width as f64) / scale;
        let bottom = (area.position.y as f64 + area.size.height as f64) / scale;
        builder = builder.position(right - TAB_W, bottom - TAB_H);
    }
    let window = builder
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()?;
    set_tool_style(&window);
    Ok(window)
}

/// A tool window is left out of Alt+Tab, which `skip_taskbar` alone does not do. `WS_EX_NOACTIVATE` is set here too
/// so a click on a button in the hover works without moving focus to it.
#[cfg(windows)]
fn set_tool_style(window: &WebviewWindow) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW,
    };
    if let Ok(hwnd) = window.hwnd() {
        unsafe {
            let style = GetWindowLongPtrW(hwnd.0, GWL_EXSTYLE);
            let style = (style | WS_EX_TOOLWINDOW as isize | WS_EX_NOACTIVATE as isize)
                & !(WS_EX_APPWINDOW as isize);
            SetWindowLongPtrW(hwnd.0, GWL_EXSTYLE, style);
        }
    }
}

#[cfg(not(windows))]
fn set_tool_style(_window: &WebviewWindow) {}

/// Sent to the hover window only, so the page focuses the Composer or shows the click-to-focus hint.
pub const FOCUS_EVENT: &str = "relay://dock-focus";
/// Sent after a native scale-factor change so the page remeasures against the new DPI.
pub const DPI_EVENT: &str = "relay://dock-dpi";

#[tauri::command]
pub fn hover_set_pinned(
    app: AppHandle,
    state: tauri::State<'_, HoverState>,
    pinned: bool,
) -> Snapshot {
    state.set_pinned(&app, pinned)
}

#[tauri::command]
pub fn hover_state(state: tauri::State<'_, HoverState>) -> Snapshot {
    state.snapshot()
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DismissReason {
    Escape,
    Saved,
}

/// Debug-only diagnostics. Nothing here carries a Note body, a window title or any credential.
fn debug(message: impl std::fmt::Display) {
    if cfg!(debug_assertions) {
        eprintln!("[relay:dock] {message}");
    }
}

fn emit_focus(app: &AppHandle, id: u64, focused: bool, hint: Option<&'static str>) {
    let _ = app.emit_to(
        LABEL,
        FOCUS_EVENT,
        DockFocus {
            session_id: id,
            visible: true,
            focused,
            hint,
        },
    );
}

fn emit_end(app: &AppHandle, id: u64) {
    let _ = app.emit_to(
        LABEL,
        FOCUS_EVENT,
        DockFocus {
            session_id: id,
            visible: false,
            focused: false,
            hint: None,
        },
    );
}

/// Debug-only state report for one summon: shell style, work area, scale and applied rectangle.
fn report_geometry(style: isize, area: WorkArea, scale: f64, rect: PhysRect) {
    debug(format!(
        "exstyle={style:#x} work=({},{}) {}x{} scale={scale:.2} rect=({},{}) {}x{}",
        area.x, area.y, area.width, area.height, rect.x, rect.y, rect.width, rect.height
    ));
}

/// Moves and sizes in one synchronous call. `NOACTIVATE` keeps focus where it is and `NOZORDER`
/// keeps the topmost band the tool window already has.
#[cfg(windows)]
fn place(window: &WebviewWindow, rect: PhysRect) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SetWindowPos,
    };
    if let Ok(hwnd) = window.hwnd() {
        unsafe {
            SetWindowPos(
                hwnd.0,
                std::ptr::null_mut(),
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
            )
        };
    }
}

#[cfg(not(windows))]
fn place(window: &WebviewWindow, rect: PhysRect) {
    let _ = window.set_position(tauri::PhysicalPosition::new(rect.x, rect.y));
    let _ = window.set_size(tauri::PhysicalSize::new(
        rect.width as u32,
        rect.height as u32,
    ));
}

/// Toggles only `WS_EX_NOACTIVATE`; the tool-window and topmost bits from `build` stay put. Re-applying
/// the whole style through the window layer would restore a stale visibility, so this stays raw.
#[cfg(windows)]
fn set_noactivate(window: &WebviewWindow, on: bool) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOACTIVATE,
    };
    let Ok(hwnd) = window.hwnd() else { return 0 };
    unsafe {
        let style = GetWindowLongPtrW(hwnd.0, GWL_EXSTYLE);
        let next = if on {
            style | WS_EX_NOACTIVATE as isize
        } else {
            style & !(WS_EX_NOACTIVATE as isize)
        };
        SetWindowLongPtrW(hwnd.0, GWL_EXSTYLE, next);
        next
    }
}

#[cfg(not(windows))]
fn set_noactivate(window: &WebviewWindow, on: bool) -> isize {
    let _ = window.set_focusable(!on);
    0
}

#[cfg(windows)]
fn show_window(window: &WebviewWindow, show: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow};
    if let Ok(hwnd) = window.hwnd() {
        unsafe { ShowWindow(hwnd.0, if show { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
    }
}

#[cfg(not(windows))]
fn show_window(window: &WebviewWindow, show: bool) {
    let _ = if show { window.show() } else { window.hide() };
}

#[cfg(windows)]
fn window_handle(window: &WebviewWindow) -> isize {
    window.hwnd().map(|h| h.0 as isize).unwrap_or(0)
}

#[cfg(not(windows))]
fn window_handle(_window: &WebviewWindow) -> isize {
    0
}

#[cfg(windows)]
fn identity_of(hwnd: isize) -> Option<(u32, u32)> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, IsWindow};
    let handle: HWND = hwnd as _;
    if hwnd == 0 || unsafe { IsWindow(handle) } == 0 {
        return None;
    }
    let mut pid = 0u32;
    let tid = unsafe { GetWindowThreadProcessId(handle, &mut pid) };
    Some((pid, tid))
}

#[cfg(not(windows))]
fn identity_of(_hwnd: isize) -> Option<(u32, u32)> {
    None
}

#[cfg(windows)]
fn previous_valid(previous: PreviousWindow) -> bool {
    identity_of(previous.hwnd) == Some((previous.pid, previous.tid))
}

#[cfg(not(windows))]
fn previous_valid(_previous: PreviousWindow) -> bool {
    false
}

/// Captured at the keypress, before the Dock is shown. Identity makes a reused HWND detectable.
#[cfg(windows)]
fn foreground_identity() -> Option<PreviousWindow> {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let hwnd = unsafe { GetForegroundWindow() } as isize;
    let (pid, tid) = identity_of(hwnd)?;
    Some(PreviousWindow { hwnd, pid, tid })
}

#[cfg(not(windows))]
fn foreground_identity() -> Option<PreviousWindow> {
    None
}

#[cfg(windows)]
fn foreground_is(hwnd: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    unsafe { GetForegroundWindow() as isize == hwnd }
}

#[cfg(not(windows))]
fn foreground_is(_hwnd: isize) -> bool {
    false
}

#[cfg(windows)]
fn notification_state() -> i32 {
    use windows_sys::Win32::UI::Shell::{QUNS_ACCEPTS_NOTIFICATIONS, SHQueryUserNotificationState};
    let mut state = QUNS_ACCEPTS_NOTIFICATIONS;
    let result = unsafe { SHQueryUserNotificationState(&mut state) };
    if result < 0 { -1 } else { state }
}

#[cfg(not(windows))]
fn notification_state() -> i32 {
    5
}

#[cfg(windows)]
fn modifiers_held() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU};
    let down = |key: u16| unsafe { GetAsyncKeyState(key as i32) as u16 & 0x8000 } != 0;
    down(VK_CONTROL) || down(VK_MENU)
}

#[cfg(not(windows))]
fn modifiers_held() -> bool {
    false
}

/// A synthetic Alt down/up is what `tao` uses to earn the foreground right after a refused call.
#[cfg(windows)]
fn alt_tap() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MENU,
    };
    let key = |flags: u32| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_MENU,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    unsafe { SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) == 2 }
}

#[cfg(not(windows))]
fn alt_tap() -> bool {
    false
}

#[cfg(windows)]
fn flash(window: &WebviewWindow) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FLASHW_ALL, FLASHW_TIMERNOFG, FLASHWINFO, FlashWindowEx,
    };
    let Ok(hwnd) = window.hwnd() else { return };
    let info = FLASHWINFO {
        cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
        hwnd: hwnd.0,
        dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
        uCount: 3,
        dwTimeout: 0,
    };
    unsafe { FlashWindowEx(&info) };
}

#[cfg(not(windows))]
fn flash(_window: &WebviewWindow) {}

/// One temporary thread-input attachment, skipped for a null, reused or same-thread target and
/// always detached after the attempt.
#[cfg(windows)]
fn attach_retry(dock: isize, previous: Option<PreviousWindow>) -> bool {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
    let Some(target) = previous else { return false };
    if !previous_valid(target) {
        return false;
    }
    let front = foreground_identity();
    if front.map(|f| f.tid) != Some(target.tid) {
        return false;
    }
    let ours = unsafe { GetCurrentThreadId() };
    if target.tid == 0 || target.tid == ours {
        return false;
    }
    debug(format!(
        "activation step 3: AttachThreadInput tid={}",
        target.tid
    ));
    if unsafe { AttachThreadInput(ours, target.tid, 1) } == 0 {
        return false;
    }
    let ok = unsafe { SetForegroundWindow(dock as windows_sys::Win32::Foundation::HWND) } != 0;
    unsafe { AttachThreadInput(ours, target.tid, 0) };
    ok && foreground_is(dock)
}

#[cfg(not(windows))]
fn attach_retry(_dock: isize, _previous: Option<PreviousWindow>) -> bool {
    false
}

enum Activation {
    Verified,
    /// The chord's modifiers were still held; a tap is queued off the hotkey stack.
    Pending,
    Refused,
}

#[cfg(windows)]
fn activate(app: &AppHandle, previous: Option<PreviousWindow>) -> Activation {
    use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
    let Some(window) = app.get_webview_window(LABEL) else {
        return Activation::Refused;
    };
    let Ok(hwnd) = window.hwnd() else {
        return Activation::Refused;
    };
    let dock = hwnd.0 as isize;
    debug("activation step 1: SetForegroundWindow");
    if unsafe { SetForegroundWindow(hwnd.0) } != 0 && foreground_is(dock) {
        return Activation::Verified;
    }
    debug("activation step 1 refused");
    if modifiers_held() {
        // SendInput while the chord is still held would be swallowed by the running handler.
        debug("modifiers held: deferring Alt tap");
        return Activation::Pending;
    }
    if alt_tap() {
        debug("activation step 2: Alt tap");
        if unsafe { SetForegroundWindow(hwnd.0) } != 0 && foreground_is(dock) {
            return Activation::Verified;
        }
    }
    if attach_retry(dock, previous) {
        return Activation::Verified;
    }
    debug("activation refused: flash and click-to-focus hint");
    Activation::Refused
}

#[cfg(not(windows))]
fn activate(app: &AppHandle, _previous: Option<PreviousWindow>) -> Activation {
    let _ = app.get_webview_window(LABEL).map(|w| w.set_focus());
    Activation::Verified
}

/// Rechecks the session and foreground after the hotkey stack has unwound. The tap is skipped if the
/// shortcut modifiers are still physically held, and nothing acts on a session that has since ended.
#[cfg(windows)]
fn defer_alt_tap(app: AppHandle, id: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let dock = handle.state::<DockRuntime>();
            if !session_current(dock.session_id(), id) {
                debug(format!("deferred tap: stale session {id}"));
                return;
            }
            let Some(window) = handle.get_webview_window(LABEL) else {
                return;
            };
            let Ok(hwnd) = window.hwnd() else {
                return;
            };
            let dock_hwnd = hwnd.0 as isize;
            if foreground_is(dock_hwnd) {
                return;
            }
            let previous = dock
                .session()
                .filter(|s| s.id == id)
                .and_then(|s| s.previous);
            // The user may have clicked another app while the tap was deferred. That choice wins:
            // end the session without restoring focus instead of pulling it back.
            if !deferred_attempt_allowed(foreground_identity(), dock_hwnd, previous) {
                debug(format!(
                    "deferred tap: foreground changed, ending session {id}"
                ));
                drop(dock);
                dismiss_native(&handle, id, Dismissal::Blur);
                return;
            }
            let held = modifiers_held();
            if held {
                debug("deferred tap: modifiers still held, skipping Alt");
            }
            let verified = if !held && alt_tap() {
                use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
                debug(format!("deferred tap: retry session {id}"));
                (unsafe { SetForegroundWindow(hwnd.0) } != 0 && foreground_is(dock_hwnd))
                    || attach_retry(dock_hwnd, previous)
            } else {
                // SendInput is blocked by held modifiers and by UIPI; a thread attachment is not.
                attach_retry(dock_hwnd, previous)
            };
            if verified {
                dock.mark_activated(id);
                emit_focus(&handle, id, true, None);
            } else {
                // Never leave a deferred session silently unfocused: flash it and offer the hint.
                flash(&window);
                emit_focus(&handle, id, false, Some("click-to-focus"));
            }
        });
    });
}

#[cfg(not(windows))]
fn defer_alt_tap(_app: AppHandle, _id: u64) {}

/// Monitor of the saved foreground window's rect centre, then the cursor, then primary. `work_area`
/// is physical pixels and excludes the taskbar on every edge.
fn target_monitor(app: &AppHandle, previous: Option<PreviousWindow>) -> Option<tauri::Monitor> {
    #[cfg(windows)]
    if let Some(target) = previous {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
        if previous_valid(target) {
            let handle: windows_sys::Win32::Foundation::HWND = target.hwnd as _;
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if unsafe { GetWindowRect(handle, &mut rect) } != 0 {
                let x = (rect.left as f64 + rect.right as f64) / 2.0;
                let y = (rect.top as f64 + rect.bottom as f64) / 2.0;
                if let Ok(Some(monitor)) = app.monitor_from_point(x, y) {
                    return Some(monitor);
                }
            }
        }
    }
    if let Ok(at) = app.cursor_position() {
        if let Ok(Some(monitor)) = app.monitor_from_point(at.x, at.y) {
            return Some(monitor);
        }
    }
    app.primary_monitor().ok().flatten()
}

fn monitor_geometry(app: &AppHandle, previous: Option<PreviousWindow>) -> (WorkArea, f64) {
    match target_monitor(app, previous) {
        Some(monitor) => {
            let area = monitor.work_area();
            (
                WorkArea {
                    x: area.position.x,
                    y: area.position.y,
                    width: area.size.width as i32,
                    height: area.size.height as i32,
                },
                monitor.scale_factor(),
            )
        }
        None => (
            WorkArea {
                x: 0,
                y: 0,
                width: 1920,
                height: 1032,
            },
            1.0,
        ),
    }
}

/// The hotkey's `Pressed` event runs on the main thread, so every Win32 call here is safe.
pub fn summon(app: &AppHandle) {
    let state = notification_state();
    if suppress_for_notification_state(state) {
        debug(format!("summon suppressed: notification state {state}"));
        return;
    }
    let runtime = app.state::<DockRuntime>();
    if !runtime.is_ready() {
        debug("summon suppressed: page not ready");
        return;
    }
    if let Some(id) = runtime.session_id() {
        // A second press while the Dock is open dismisses it.
        debug(format!("toggle dismiss session {id}"));
        dismiss_native(app, id, Dismissal::Toggle);
        return;
    }
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    let dock = window_handle(&window);
    let previous = foreground_identity().filter(|p| p.hwnd != dock);
    let (area, scale) = monitor_geometry(app, previous);
    let height = runtime.lock().compact_height;
    let rect = dock_rect(area, scale, DOCK_WIDTH_CSS, height);
    let id = runtime.new_id();

    // Place while hidden, then clear NOACTIVATE, show without activating and try to take focus.
    place(&window, rect);
    let style = set_noactivate(&window, false);
    report_geometry(style, area, scale, rect);
    debug(format!(
        "summon session {id} previous={:?}",
        previous.map(|p| p.hwnd)
    ));
    show_window(&window, true);
    runtime.start(Session {
        id,
        dock,
        previous,
        area,
        scale,
        rect,
        css_height: height,
        activated: false,
    });
    let _ = app.state::<HoverState>().set_dock_status(app, true, true);

    match activate(app, previous) {
        Activation::Verified => {
            debug(format!("activation verified session {id}"));
            runtime.mark_activated(id);
            emit_focus(app, id, true, None);
        }
        Activation::Pending => {
            debug(format!("activation pending session {id}"));
            emit_focus(app, id, false, None);
            defer_alt_tap(app.clone(), id);
        }
        Activation::Refused => {
            flash(&window);
            emit_focus(app, id, false, Some("click-to-focus"));
        }
    }
}

/// The page calls this once it has registered its listeners and measured the compact height.
#[tauri::command]
pub fn dock_ready(
    app: AppHandle,
    height: u32,
    runtime: tauri::State<'_, DockRuntime>,
    state: tauri::State<'_, HoverState>,
) -> bool {
    runtime.ready(height as f64);
    let _ = state.set_dock_status(&app, true, runtime.session_id().is_some());
    debug(format!("dock ready height={height}"));
    true
}

/// Applies a new CSS height to the live session. The command acknowledges only after the single
/// `SetWindowPos` has run; a stale session id is ignored so a late observation cannot resize a
/// Dock that has since been dismissed and reopened.
#[tauri::command]
pub fn dock_resize(
    app: AppHandle,
    height: u32,
    session_id: u64,
    runtime: tauri::State<'_, DockRuntime>,
) -> bool {
    let Some((_session, rect)) = runtime.rect_for(session_id, height as f64) else {
        debug(format!("resize ignored: stale session {session_id}"));
        return false;
    };
    let Some(window) = app.get_webview_window(LABEL) else {
        return false;
    };
    place(&window, rect);
    debug(format!(
        "resize session {session_id} height={height} applied={rect:?}"
    ));
    true
}

/// Explicit dismissal from an unconsumed Esc or a successful local save.
#[tauri::command]
pub fn dock_dismiss(app: AppHandle, session_id: u64, reason: DismissReason) -> bool {
    let how = match reason {
        DismissReason::Escape => Dismissal::Escape,
        DismissReason::Saved => Dismissal::Saved,
    };
    dismiss_native(&app, session_id, how)
}

/// Hides and clears the session. The foreground is sampled before hiding, and the saved window is
/// restored only when it is still valid and the Dock still owned focus at that moment.
pub fn dismiss_native(app: &AppHandle, id: u64, how: Dismissal) -> bool {
    let Some(session) = app.state::<DockRuntime>().take(id) else {
        debug(format!("dismiss ignored: stale session {id}"));
        return false;
    };
    let dock_owned = foreground_is(session.dock);
    let valid = session.previous.map(previous_valid).unwrap_or(false);
    let restore = restore_target(session.previous, how, dock_owned, valid);
    debug(format!(
        "dismiss session {id} reason={how:?} dock_owned={dock_owned} previous_valid={valid} restore={}",
        restore.is_some()
    ));

    if let Some(window) = app.get_webview_window(LABEL) {
        set_noactivate(&window, true);
        show_window(&window, false);
        if let Some(target) = restore {
            #[cfg(windows)]
            {
                use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
                let accepted = unsafe {
                    SetForegroundWindow(target.hwnd as windows_sys::Win32::Foundation::HWND)
                } != 0;
                // Record a refusal; never make a second focus grab after the user's choice.
                debug(format!(
                    "restore hwnd={} accepted={accepted} verified={}",
                    target.hwnd,
                    foreground_is(target.hwnd)
                ));
            }
            #[cfg(not(windows))]
            let _ = target;
        }
    }
    let _ = app.state::<HoverState>().set_dock_status(app, true, false);
    emit_end(app, id);
    true
}

/// Native Dock blur hides the Dock and discards the saved target without restoring focus.
pub fn handle_blur(app: &AppHandle) {
    let runtime = app.state::<DockRuntime>();
    let Some(session) = runtime.session() else {
        return;
    };
    if session.activated {
        dismiss_native(app, session.id, Dismissal::Blur);
    }
}

/// A native scale-factor change invalidates the work area and scale captured at summon. Re-anchor
/// from the saved foreground window before applying the new rectangle, then ask the page to
/// remeasure. The whole operation is one synchronous `SetWindowPos` with no activation.
pub fn handle_scale_change(app: &AppHandle) {
    let Some(session) = app.state::<DockRuntime>().session() else {
        return;
    };
    let (area, scale) = monitor_geometry(app, session.previous);
    let Some((updated, rect)) = app
        .state::<DockRuntime>()
        .refresh_geometry(session.id, area, scale)
    else {
        return;
    };
    if let Some(window) = app.get_webview_window(LABEL) {
        place(&window, rect);
    }
    report_geometry(0, area, scale, rect);
    debug(format!(
        "dpi change session {} height={} rect={rect:?}",
        updated.id, updated.css_height
    ));
    let _ = app.emit_to(
        LABEL,
        DPI_EVENT,
        DockDpi {
            session_id: session.id,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags() -> Flags {
        Flags {
            pinned: false,
            dnd: false,
            paused: false,
            ready: true,
            visible: false,
        }
    }

    #[test]
    fn flags_keep_the_provider_toggles_and_mirror_session_status() {
        let snapshot = Flags {
            pinned: true,
            dnd: true,
            paused: true,
            ..flags()
        }
        .snapshot();
        assert_eq!(
            snapshot,
            Snapshot {
                pinned: true,
                dnd: true,
                paused: true,
                ready: true,
                visible: false
            }
        );
    }

    #[test]
    fn physical_rect_at_one_hundred_one_twenty_five_and_one_fifty_percent() {
        // The same logical 1920×1032 work area at three scales.
        let rect = dock_rect(
            WorkArea {
                x: 0,
                y: 0,
                width: 1920,
                height: 1032,
            },
            1.0,
            DOCK_WIDTH_CSS,
            420.0,
        );
        assert_eq!(
            rect,
            PhysRect {
                x: 580,
                y: 596,
                width: 760,
                height: 420
            }
        );

        let rect = dock_rect(
            WorkArea {
                x: 0,
                y: 0,
                width: 2400,
                height: 1290,
            },
            1.25,
            DOCK_WIDTH_CSS,
            420.0,
        );
        assert_eq!(
            rect,
            PhysRect {
                x: 725,
                y: 745,
                width: 950,
                height: 525
            }
        );

        let rect = dock_rect(
            WorkArea {
                x: 0,
                y: 0,
                width: 2880,
                height: 1548,
            },
            1.5,
            DOCK_WIDTH_CSS,
            420.0,
        );
        assert_eq!(
            rect,
            PhysRect {
                x: 870,
                y: 894,
                width: 1140,
                height: 630
            }
        );
    }

    #[test]
    fn physical_rect_handles_a_negative_monitor_origin() {
        // A 150% monitor placed left of and above the primary one.
        let rect = dock_rect(
            WorkArea {
                x: -2880,
                y: -500,
                width: 2880,
                height: 1548,
            },
            1.5,
            DOCK_WIDTH_CSS,
            420.0,
        );
        assert_eq!(
            rect,
            PhysRect {
                x: -2010,
                y: 394,
                width: 1140,
                height: 630
            }
        );
    }

    #[test]
    fn physical_rect_caps_height_and_width_to_the_work_area() {
        let tall = dock_rect(
            WorkArea {
                x: 0,
                y: 0,
                width: 1920,
                height: 300,
            },
            1.0,
            DOCK_WIDTH_CSS,
            2000.0,
        );
        assert_eq!(
            tall,
            PhysRect {
                x: 580,
                y: 0,
                width: 760,
                height: 284
            }
        );

        let wide = dock_rect(
            WorkArea {
                x: -200,
                y: 48,
                width: 500,
                height: 1032,
            },
            1.0,
            DOCK_WIDTH_CSS,
            420.0,
        );
        assert_eq!(
            wide,
            PhysRect {
                x: -200,
                y: 644,
                width: 500,
                height: 420
            }
        );
    }

    #[test]
    fn css_to_physical_rounds_up_without_a_phantom_pixel() {
        assert_eq!(ceil_css(16.0, 1.0), 16);
        assert_eq!(ceil_css(16.0, 1.25), 20);
        assert_eq!(ceil_css(16.0, 1.5), 24);
        assert_eq!(ceil_css(101.0, 1.25), 127);
    }

    #[test]
    fn dismiss_restores_only_a_valid_target_that_the_dock_still_owns() {
        let previous = PreviousWindow {
            hwnd: 42,
            pid: 7,
            tid: 9,
        };
        assert_eq!(
            restore_target(Some(previous), Dismissal::Escape, true, true),
            Some(previous)
        );
        assert_eq!(
            restore_target(Some(previous), Dismissal::Saved, true, true),
            Some(previous)
        );
        assert_eq!(
            restore_target(Some(previous), Dismissal::Toggle, true, true),
            Some(previous)
        );
        // The user clicked elsewhere, the window was reused, or there was no session: no restore.
        assert_eq!(
            restore_target(Some(previous), Dismissal::Escape, false, true),
            None
        );
        assert_eq!(
            restore_target(Some(previous), Dismissal::Saved, true, false),
            None
        );
        assert_eq!(restore_target(None, Dismissal::Escape, true, true), None);
    }

    #[test]
    fn blur_discards_the_saved_target_even_when_it_is_valid() {
        let previous = PreviousWindow {
            hwnd: 42,
            pid: 7,
            tid: 9,
        };
        assert_eq!(
            restore_target(Some(previous), Dismissal::Blur, true, true),
            None
        );
    }

    #[test]
    fn a_stale_session_cannot_resize_or_dismiss() {
        assert!(session_current(Some(2), 2));
        assert!(!session_current(Some(2), 1));
        assert!(!session_current(None, 1));
    }

    #[test]
    fn a_delayed_attempt_terminates_when_the_user_clicked_elsewhere() {
        let previous = PreviousWindow {
            hwnd: 42,
            pid: 7,
            tid: 9,
        };
        let dock = 99;
        assert!(deferred_attempt_allowed(
            Some(previous),
            dock,
            Some(previous)
        ));
        assert!(deferred_attempt_allowed(
            Some(PreviousWindow {
                hwnd: dock,
                pid: 1,
                tid: 2
            }),
            dock,
            Some(previous)
        ));
        let other = PreviousWindow {
            hwnd: 55,
            pid: 3,
            tid: 4,
        };
        assert!(!deferred_attempt_allowed(Some(other), dock, Some(previous)));
        assert!(!deferred_attempt_allowed(None, dock, Some(previous)));
        assert!(!deferred_attempt_allowed(Some(previous), dock, None));
    }

    #[test]
    fn a_scale_change_keeps_the_bottom_edge_anchored() {
        let old = dock_rect(
            WorkArea {
                x: 0,
                y: 0,
                width: 2400,
                height: 1290,
            },
            1.25,
            DOCK_WIDTH_CSS,
            400.0,
        );
        let new = dock_rect(
            WorkArea {
                x: 0,
                y: 0,
                width: 2880,
                height: 1548,
            },
            1.5,
            DOCK_WIDTH_CSS,
            400.0,
        );
        // The work-area bottom minus the physical margin is the same in both scales: 1270 and 1524.
        assert_eq!(old.y + old.height, 1270);
        assert_eq!(new.y + new.height, 1524);
        assert_eq!(new.height, 600);
    }

    #[test]
    fn only_exclusive_direct3d_full_screen_suppresses_summon() {
        assert!(suppress_for_notification_state(3));
        assert!(!suppress_for_notification_state(2)); // QUNS_BUSY
        assert!(!suppress_for_notification_state(5)); // QUNS_ACCEPTS_NOTIFICATIONS
        assert!(!suppress_for_notification_state(-1)); // query failed
    }
}
