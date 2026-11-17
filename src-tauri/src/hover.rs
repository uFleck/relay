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

/// The edge tab's size in logical pixels (design/README.md, "Hover companion").
const TAB_W: f64 = 52.0;
const TAB_H: f64 = 64.0;

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
}
