# Dock: taking focus and growing upward on Windows

Research for wayfinder ticket #18. Plan only; no app code was changed. Sources are Microsoft Learn Win32 pages (read from the `MicrosoftDocs/sdk-api` and `MicrosoftDocs/win32` repos), the `tao`, `wry`, `tauri`, `tauri-plugin-global-shortcut` and `global-hotkey` sources on their default branches, and the local `src-tauri/src/hover.rs` and `lib.rs`. Anything not stated by a source is marked **inferred** or **verify on hardware**.

## Decision summary

1. Summon on `ShortcutState::Pressed`. The handler runs on Relay's main thread. In that handler: read `GetForegroundWindow` (the window to return to), place the Dock while hidden, clear `WS_EX_NOACTIVATE`, `ShowWindow`, `SetForegroundWindow`, then check `GetForegroundWindow() == dock`. The docs do not promise a hotkey handler may take the foreground, so the call needs a checked fallback ladder (below).
2. Dismiss with Esc or submit: restore the saved window. Dismiss by blur: hide only, never restore. The existing `hover.rs` logic already has this shape; it needs the `PREVIOUS` capture moved earlier and the `SetForegroundWindow` result checked.
3. Sizing: the webview measures its content (CSS px) and sends only a height; Rust owns the bottom-centre anchor, the monitor, its scale factor and the work area, and issues one `SetWindowPos` in physical pixels with `SWP_NOACTIVATE | SWP_NOZORDER`. Grow the window first and animate after; animate first and shrink after. Pick the monitor of the previously active window, falling back to the cursor's.

## 1. Does the hotkey give Relay the foreground right?

What the docs say, from `SetForegroundWindow` Remarks: the call is allowed only if the caller is a desktop app, no menus are active, the foreground process has not called `LockSetForegroundWindow`, **and** at least one of these holds: the foreground lock time-out has expired; the caller is the foreground process; the caller was started by the foreground process; there is no foreground window; **the calling process received the last input event**; or either process is being debugged. The page adds: "It is possible for a process to be denied the right to set the foreground window even if it meets these conditions", and that a refused call flashes the taskbar button instead. (learn.microsoft.com/windows/win32/api/winuser/nf-winuser-setforegroundwindow)

What the docs do not say: `RegisterHotKey` makes no statement that a `WM_HOTKEY` recipient counts as having "received the last input event". Its Remarks only say the system posts `WM_HOTKEY` to the registering thread's queue. (nf-winuser-registerhotkey) So the rule that would let the hotkey path work is the "received the last input event" clause, and that is **inferred, not documented**. Treat success as likely but verify it on hardware, including after the user has been typing in another app for a long time.

How Relay receives the hotkey (`tauri-plugin-global-shortcut` 2.4.x, `global-hotkey` 0.8):

- `global-hotkey` creates a hidden `WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW` message window and calls `RegisterHotKey` with `MOD_NOREPEAT` on it. The plugin creates the manager in `setup` and runs register/unregister through `run_on_main_thread`, so the window and its `WM_HOTKEY` dispatch live on Relay's main thread.
- The `WM_HOTKEY` window procedure calls the event handler synchronously with `Pressed`, then spawns a thread that polls `GetAsyncKeyState` every 50 ms and sends `Released` from **that other thread**. So a `Pressed` handler runs on the main thread (Win32 window calls are safe there), and a `Released` handler does not.
- Tauri `send_user_message` runs inline when called from the main thread, so `monitor_from_point`, `set_*` and similar Tauri window calls made inside the `Pressed` handler do not deadlock on their reply channel. Calls from other threads are marshalled to the event loop.

What fails and the fallback ladder (apply in order, stop at the first that leaves `GetForegroundWindow() == dock`):

| Step | Action | Source for when it fails |
|---|---|---|
| 1 | `SetForegroundWindow(dock)` after the style flip | Denied if none of the allow conditions hold; taskbar flashes instead. |
| 2 | Synthetic Alt tap via `SendInput`, then `SetForegroundWindow` again. This is what `tao`'s own `force_window_active` does after a failed first call. | `SendInput` is subject to UIPI: it cannot inject into a window at a higher integrity level, and neither the return value nor `GetLastError` reports the block. So this step is expected to do nothing when the foreground app is elevated. |
| 3 | `AttachThreadInput` from Relay's thread to the foreground thread, `SetForegroundWindow`, then detach. | Fails if either thread has no message queue, or a journal record hook is installed. Key state is reset by the call. Behaviour against elevated windows is not documented: **verify on hardware**. |
| 4 | Give up focusing: leave the Dock shown without activation, flash it (`FlashWindowEx`), and tell the user to click it, or open the main window instead. | No further mechanism is documented. |

`AllowSetForegroundWindow` does not help here: it grants the right to another process, and the Dock is in the same process. It would only matter if a helper process were launched.

Full-screen and elevated apps:

- **Elevated foreground app.** UIPI (cited above for `SendInput`) is the documented reason step 2 cannot work. Whether a non-elevated process may take the foreground from, or hand it back to, an elevated window is not stated in the `SetForegroundWindow` page, which lists no integrity-level condition. **Verify on hardware**; the saved-window restore must tolerate a refusal.
- **Full-screen apps.** `SHQueryUserNotificationState` returns `QUNS_RUNNING_D3D_FULL_SCREEN` for exclusive-mode Direct3D full screen, `QUNS_BUSY` for other full-screen apps and `QUNS_PRESENTATION_MODE` for presentations, and the docs say no notification is sent when a full-screen app starts or stops, so it has to be polled at summon time. (shellapi docs) That a top-most window cannot draw over exclusive D3D full screen is common knowledge, not stated by these sources: **inferred**. Recommendation: query it in the hotkey handler; on `QUNS_RUNNING_D3D_FULL_SCREEN` do not show the Dock (nothing to focus visibly), and optionally queue a toast. Borderless full-screen games and video players are the case to test.

Getting keyboard focus into the page: the Dock's HWND being foreground is not enough on its own, because input must reach the WebView2 content. `wry` handles `WM_SETFOCUS` on the parent window by calling `ICoreWebView2Controller::MoveFocus(PROGRAMMATIC)`, so once the Dock is activated the webview takes focus, and then the page's `input.focus()` works. Tauri's `webview.set_focus()` maps to the same `MoveFocus`.

Note on `set_focusable`: `tao` maps it to `WS_EX_NOACTIVATE` through its window-flag machinery, which re-applies stored visibility on every flag change. `hover.rs` already documents why it flips the style directly with `SetWindowLongPtrW`. Keep that.

Gaps in the current `hover.rs::set_focus`, for the implementation ticket: the `SetForegroundWindow` return value is ignored, there is no fallback, and `PREVIOUS` is captured when the page asks for focus over IPC rather than at the keypress.

## 2. Returning focus on dismiss

- Capture `GetForegroundWindow()` in the hotkey handler, before showing or activating the Dock. Skip the capture if it is already the Dock (toggle press) or null; the existing guard `front != ours && front != 0` does this. Re-validate with `IsWindow` before restoring (already done).
- Esc and submit: clear focus in the page, then call the existing `set_focus(false)` path: re-set `WS_EX_NOACTIVATE`, hide, and `SetForegroundWindow(previous)` only when the Dock is still the foreground window. The "still ours" check is correct: the docs let the foreground process call `SetForegroundWindow`, so the restore is allowed, while a user who clicked elsewhere keeps their choice.
- Blur (`WM_ACTIVATE` with `WA_INACTIVE`, or the webview `blur` event): hide the Dock and **do not** restore, since the user has already chosen a new foreground window. `WM_ACTIVATE` carries the window being activated in `lParam` when deactivating, which tells the handler who took focus. (wm-activate)
- `WS_EX_NOACTIVATE` documentation says the system will not bring such a window forward on minimize/close of another, and that the window "should not be activated through programmatic access"; to activate it, call `SetActiveWindow` or `SetForegroundWindow`. So re-enabling the style after dismiss keeps the passive hover mode intact. (extended-window-styles)
- Restoring puts the previous window back as foreground; which control inside it gets keyboard focus is Windows' normal per-window focus restore: **inferred**, not stated on the `SetForegroundWindow` page.

## 3. Growing upward with a fixed bottom edge

Who measures: the webview, because only it knows the laid-out content height. Who positions: Rust. The page should send one number, the desired content height in CSS pixels (from a `ResizeObserver`, rounded up), through a `dock_resize(height)` command. This differs from the hover's `hover_place`, where the page computes x and y; for the Dock, Rust keeps the anchor so the page never needs monitor or DPI knowledge.

Rust state held per summon: monitor work area (physical), monitor scale factor, the bottom-centre anchor, a fixed Dock width, bottom margin in logical px.

Per resize, in Rust, on the main thread:

```
physical_h = ceil(css_h * scale).min(work.height - margin)
x = work.x + (work.width - physical_w) / 2
y = work.y + work.height - margin_phys - physical_h
SetWindowPos(hwnd, null, x, y, physical_w, physical_h, SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER)
```

This is the existing `place` function in `hover.rs`, which already moves and sizes in one call so there is no frame with the content shifted. Reuse it.

Details:

- **Flicker.** `wry` subclasses the parent and on `WM_SIZE` sets the WebView2 controller bounds synchronously, so a synchronous `SetWindowPos` (no `SWP_ASYNCWINDOWPOS`) resizes the webview in the same step. `tao`'s own `set_outer_position` uses `SWP_ASYNCWINDOWPOS`, another reason to keep the raw call. Sequence it as: window grows first, then the page animates content into the new space; for shrinking, animate content out first, then shrink the window. With `transparent(true)` and `shadow(false)` (already set) the only visible artefact of a late repaint is empty transparent area. A cheaper alternative if flicker still shows is to keep the window at its maximum height and bottom-align the content in CSS, but then the transparent area above the Dock intercepts clicks, and `SetWindowRgn` would be needed to clip it (not researched); prefer three coarse window sizes (compact, picker, picker plus Lens).
- **Per-monitor DPI.** Convert CSS px to physical px with the **target monitor's** `scale_factor`, as `build()` already does for the work area. Place the window while hidden, before `ShowWindow`, so a cross-monitor DPI change (`WM_DPICHANGED`) cannot cause a visible jump. After a scale change event, re-send the height from the page. Rounding up avoids a clipped 1 px row at 125% and 150%. Whether a single `SetWindowPos` that moves and resizes across a DPI boundary lands exactly is not documented: **verify on a mixed-DPI setup**.
- **Which monitor.** For a keyboard-summoned composer, use the monitor of the window that was foreground at the keypress (the same window saved for focus return): take its `GetWindowRect` centre and call Tauri `monitor_from_point`, which avoids adding `windows-sys` `Win32_Graphics_Gdi`. Fall back to the cursor (`cursor_position`, physical, top-left of the main monitor is the origin and coordinates may be negative on multi-monitor desktops), then to `primary_monitor`. Tauri exposes `cursor_position`, `monitor_from_point`, `current_monitor` and `work_area()` on its window/monitor types.
- **Taskbar and work-area edges.** `Monitor::work_area()` (already used by `hover.rs::build`) returns the area excluding the taskbar and other docked bars, in physical pixels, so a bottom taskbar, or one on the left, right or top, is handled without special cases. Read it at summon time, not startup, because the taskbar can be moved or auto-hidden and a work-area change after a display change makes a cached value stale. Auto-hide behaviour of the reported work area: **verify on hardware**. Cap the height at the work-area height minus the margin so the Lens cannot push the top edge off screen; the page should scroll inside past the cap.
- **Z-order.** `SWP_NOZORDER` keeps the topmost band the window already has (`always_on_top(true)` at build), as the existing comment says.

## Not covered

- No run on real hardware: every "verify on hardware" item above is open (hotkey foreground right after long idle in another app, elevated apps, borderless and exclusive full screen, mixed-DPI monitors, auto-hide taskbar).
- `SetWindowRgn` click-through shaping for an oversized window.
- Virtual desktops, UWP foreground apps, and the Secure Desktop (UAC prompt, lock screen), where the hotkey is not delivered.
- IME and multi-seat input.

## Source index

- SetForegroundWindow, AllowSetForegroundWindow, RegisterHotKey, AttachThreadInput, SetWindowPos, SendInput (UIPI), SystemParametersInfo (`SPI_GETFOREGROUNDLOCKTIMEOUT`), SHQueryUserNotificationState, Extended Window Styles (`WS_EX_NOACTIVATE`), WM_ACTIVATE: Microsoft Learn Win32 docs.
- `tauri-apps/global-hotkey` `src/platform_impl/windows/mod.rs` and `src/lib.rs`; `tauri-apps/tauri-plugin-global-shortcut` `src/lib.rs`.
- `tauri-apps/tao` `src/platform_impl/windows/{window.rs,window_state.rs}` (`force_window_active`, `set_focusable`, `set_outer_position`); `tauri-apps/tauri` `crates/tauri/src/window/mod.rs` and `crates/tauri-runtime-wry/src/lib.rs` (`send_user_message`, `monitor_from_point`); `tauri-apps/wry` `src/webview2/mod.rs` (`WM_SETFOCUS`, `WM_SIZE`, `set_bounds`).
- Local: `src-tauri/src/hover.rs` (`set_tool_style`, `set_visible`, `place`, `set_focus`), `src-tauri/src/lib.rs` (`register_hotkeys`).
