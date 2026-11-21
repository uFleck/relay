//! The hover: a small always-on-top window at the screen edge that shows Relay's counts, claims and "needs you"
//! badge without switching to the main window. It is built hidden at startup and copies Spawner's companion: it never
//! takes focus, stays out of Alt+Tab, and is shown and hidden with raw Win32 calls. When to show it is decided here;
//! the page (`hover.html`) sizes and places the window itself through `hover_place`.

use serde::Serialize;
use std::sync::{Mutex, MutexGuard};
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

/// `SHQueryUserNotificationState` is polled on every summon. Only exclusive Direct3D full screen
/// (3) suppresses the Dock; `QUNS_BUSY` (2) and every other state still activate. A failed query is
/// reported as -1 and also activates.
pub fn suppress_for_notification_state(state: i32) -> bool {
    state == 3
}

struct Flags {
    /// The user setting for the hover. There is no switch for it yet, so it is always on.
    enabled: bool,
    main_focused: bool,
    pinned: bool,
    /// Set once the page has loaded and placed itself, so the user never sees an empty or misplaced window.
    ready: bool,
    /// Do not disturb only stops pulses and toasts; the tab stays visible.
    dnd: bool,
    paused: bool,
    shown: bool,
}

impl Flags {
    fn want_shown(&self) -> bool {
        self.enabled && (!self.main_focused || self.pinned) && self.ready
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            pinned: self.pinned,
            dnd: self.dnd,
            paused: self.paused,
            ready: self.ready,
            visible: self.shown,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub pinned: bool,
    pub dnd: bool,
    pub paused: bool,
    pub ready: bool,
    pub visible: bool,
}

pub struct HoverState {
    flags: Mutex<Flags>,
}

impl Default for HoverState {
    fn default() -> Self {
        Self {
            flags: Mutex::new(Flags {
                enabled: true,
                main_focused: false,
                pinned: false,
                ready: false,
                dnd: false,
                paused: false,
                shown: false,
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

    pub fn set_main_focused(&self, app: &AppHandle, focused: bool) -> Snapshot {
        self.update(app, |f| f.main_focused = focused)
    }

    pub fn set_pinned(&self, app: &AppHandle, pinned: bool) -> Snapshot {
        self.update(app, |f| f.pinned = pinned)
    }

    pub fn set_ready(&self, app: &AppHandle) -> Snapshot {
        self.update(app, |f| f.ready = true)
    }

    pub fn set_dnd(&self, app: &AppHandle, dnd: bool) -> Snapshot {
        self.update(app, |f| f.dnd = dnd)
    }

    pub fn set_paused(&self, app: &AppHandle, paused: bool) -> Snapshot {
        self.update(app, |f| f.paused = paused)
    }

    /// Applies `change`, shows or hides the window when that flips whether it should be visible, and tells the pages
    /// when anything they can see changed.
    fn update(&self, app: &AppHandle, change: impl FnOnce(&mut Flags)) -> Snapshot {
        let (before, after, flip) = {
            let mut f = self.lock();
            let before = f.snapshot();
            change(&mut f);
            let want = f.want_shown();
            let flip = (want != f.shown).then_some(want);
            if flip.is_some() {
                f.shown = want;
            }
            (before, f.snapshot(), flip)
        };
        if let Some(show) = flip {
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || match handle.get_webview_window(LABEL) {
                Some(window) => set_visible(&window, show),
                // Not built, so nothing is shown: the next update tries again.
                None => {
                    if let Some(state) = handle.try_state::<HoverState>() {
                        state.lock().shown = false;
                    }
                }
            });
        }
        if after != before {
            let _ = app.emit(STATE_EVENT, after);
        }
        after
    }
}

/// Builds the hover hidden in the bottom right corner of the primary work area. The page moves it to its saved edge
/// once it has loaded and measured itself, then calls `hover_ready`.
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

/// Shown without activating it, so the window the user is typing in keeps focus. Tauri's `show` activates the
/// window, and its hidden flag would go stale after a raw show, so both directions go through Win32 here.
#[cfg(windows)]
fn set_visible(window: &WebviewWindow, show: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow};
    if let Ok(hwnd) = window.hwnd() {
        unsafe { ShowWindow(hwnd.0, if show { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
    }
}

#[cfg(not(windows))]
fn set_visible(window: &WebviewWindow, show: bool) {
    let _ = if show { window.show() } else { window.hide() };
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

/// The page calls this once it has loaded and placed itself; until then the hover stays hidden.
#[tauri::command]
pub fn hover_ready(app: AppHandle, state: tauri::State<'_, HoverState>) -> Snapshot {
    state.set_ready(&app)
}

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

/// Moves and sizes the hover in one step, in physical pixels. A separate move and resize would draw one frame with
/// the content shifted while the tab grows into the panel.
#[tauri::command]
pub fn hover_place(app: AppHandle, x: i32, y: i32, width: u32, height: u32) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    place(&window, x, y, width, height);
}

#[cfg(windows)]
fn place(window: &WebviewWindow, x: i32, y: i32, width: u32, height: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SetWindowPos,
    };
    if let Ok(hwnd) = window.hwnd() {
        // NOZORDER keeps the topmost band the window already has; NOACTIVATE keeps focus where it is.
        unsafe {
            SetWindowPos(
                hwnd.0,
                std::ptr::null_mut(),
                x,
                y,
                width as i32,
                height as i32,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER,
            )
        };
    }
}

#[cfg(not(windows))]
fn place(window: &WebviewWindow, x: i32, y: i32, width: u32, height: u32) {
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    let _ = window.set_size(tauri::PhysicalSize::new(width, height));
}

/// Lets the user drag the hover and returns once they let go, so the page knows when to snap it to an edge.
#[tauri::command]
pub async fn hover_drag(app: AppHandle) {
    let (done, dropped) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(LABEL) {
            drag(&window);
        }
        let _ = done.send(());
    });
    let _ = dropped.await;
}

/// Tauri's `start_dragging` posts the message and returns at once. Sent instead, the OS move loop runs inside the
/// call, which returns when the mouse button is released.
#[cfg(windows)]
fn drag(window: &WebviewWindow) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, HTCAPTION, SendMessageW, WM_NCLBUTTONDOWN,
    };
    if let Ok(hwnd) = window.hwnd() {
        let mut at = POINT { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut at);
            // The webview holds the mouse capture from the press; the move loop needs it released first.
            ReleaseCapture();
            // lParam packs the screen point as two 16-bit words, y high and x low, like MAKELPARAM.
            let point = ((at.y as u16 as u32) << 16 | at.x as u16 as u32) as isize;
            SendMessageW(hwnd.0, WM_NCLBUTTONDOWN, HTCAPTION as usize, point);
        }
    }
}

#[cfg(not(windows))]
fn drag(window: &WebviewWindow) {
    let _ = window.start_dragging();
}

/// Where the cursor is inside the hover, in physical pixels from its top left corner, or none when it is outside the
/// window or another window covers it. The page polls this for hover-to-expand: the webview misreports that the mouse
/// left once focus has moved.
#[tauri::command]
pub fn hover_pointer(app: AppHandle) -> Option<(i32, i32)> {
    let window = app.get_webview_window(LABEL)?;
    pointer(&window)
}

#[cfg(windows)]
fn pointer(window: &WebviewWindow) -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetCursorPos, GetWindowRect, WindowFromPoint,
    };
    let hwnd = window.hwnd().ok()?.0;
    unsafe {
        let mut at = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut at) == 0 {
            return None;
        }
        // The hit is the webview's child window, so compare its top-level ancestor; a window on top of the hover
        // makes this fail, which is what "covered" means.
        let hit = WindowFromPoint(at);
        if hit.is_null() || GetAncestor(hit, GA_ROOT) != hwnd {
            return None;
        }
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return None;
        }
        Some((at.x - rect.left, at.y - rect.top))
    }
}

#[cfg(not(windows))]
fn pointer(window: &WebviewWindow) -> Option<(i32, i32)> {
    let cursor = window.cursor_position().ok()?;
    let at = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    let (x, y) = (
        (cursor.x - at.x as f64) as i32,
        (cursor.y - at.y as f64) as i32,
    );
    (x >= 0 && y >= 0 && x < size.width as i32 && y < size.height as i32).then_some((x, y))
}

/// The window that had focus before the hover was made focusable, so it can get it back.
#[cfg(windows)]
static PREVIOUS: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// The hover never takes focus on its own. Only when the page needs the keyboard (typing in a field) does it ask for
/// focus, and it gives focus back to the window that had it when done.
#[tauri::command]
pub fn hover_focus(app: AppHandle, focus: bool) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    set_focus(&window, focus);
}

#[cfg(windows)]
fn set_focus(window: &WebviewWindow, focus: bool) {
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, IsWindow, SetForegroundWindow,
        SetWindowLongPtrW, WS_EX_NOACTIVATE,
    };
    let Ok(hwnd) = window.hwnd() else { return };
    let ours = hwnd.0 as isize;
    // The style is flipped here and not through `set_focusable`: that makes the window layer re-apply the visibility it
    // remembers, and this window is shown and hidden with raw Win32 calls it knows nothing about, so it would be hidden.
    let style = unsafe { GetWindowLongPtrW(hwnd.0, GWL_EXSTYLE) };
    if focus {
        let front = unsafe { GetForegroundWindow() } as isize;
        if front != ours && front != 0 {
            PREVIOUS.store(front, Ordering::Relaxed);
        }
        unsafe {
            SetWindowLongPtrW(hwnd.0, GWL_EXSTYLE, style & !(WS_EX_NOACTIVATE as isize));
            SetForegroundWindow(hwnd.0);
        }
    } else {
        unsafe { SetWindowLongPtrW(hwnd.0, GWL_EXSTYLE, style | WS_EX_NOACTIVATE as isize) };
        let previous = PREVIOUS.swap(0, Ordering::Relaxed);
        // Only hand focus back when it is still ours; the user may have clicked somewhere else already.
        if previous != 0
            && unsafe { GetForegroundWindow() } as isize == ours
            && unsafe { IsWindow(previous as _) } != 0
        {
            unsafe { SetForegroundWindow(previous as _) };
        }
    }
}

#[cfg(not(windows))]
fn set_focus(window: &WebviewWindow, focus: bool) {
    let _ = window.set_focusable(focus);
    if focus {
        let _ = window.set_focus();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags() -> Flags {
        Flags {
            enabled: true,
            main_focused: false,
            pinned: false,
            ready: true,
            dnd: false,
            paused: false,
            shown: false,
        }
    }

    #[test]
    fn hidden_until_the_page_is_ready() {
        assert!(
            !Flags {
                ready: false,
                ..flags()
            }
            .want_shown()
        );
        assert!(flags().want_shown());
    }

    #[test]
    fn main_focus_hides_it_unless_pinned() {
        assert!(
            !Flags {
                main_focused: true,
                ..flags()
            }
            .want_shown()
        );
        assert!(
            Flags {
                main_focused: true,
                pinned: true,
                ..flags()
            }
            .want_shown()
        );
    }

    #[test]
    fn do_not_disturb_and_pause_keep_the_tab() {
        assert!(
            Flags {
                dnd: true,
                paused: true,
                ..flags()
            }
            .want_shown()
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
    fn only_exclusive_direct3d_full_screen_suppresses_summon() {
        assert!(suppress_for_notification_state(3));
        assert!(!suppress_for_notification_state(2)); // QUNS_BUSY
        assert!(!suppress_for_notification_state(5)); // QUNS_ACCEPTS_NOTIFICATIONS
        assert!(!suppress_for_notification_state(-1)); // query failed
    }
}
