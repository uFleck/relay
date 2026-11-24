//! Relay's Tauri shell: the main window, the hover window, tray, global hotkey, single instance, autostart and the
//! local DB. The UI in both windows is owned by the frontend; this file only routes events to it.

mod commands;
mod dispatch;
mod github;
mod grilling;
mod grilling_mcp;
mod hover;
mod keybinds;
mod lifecycle;
mod live;
mod note_import;
mod opencode;
mod process;
mod progress;
mod recovery;
mod secrets;
mod settings;
mod slack;
mod t3;
mod t3_cli;

use hover::{DockRuntime, HoverState};
use relay_core::v2::V2Store;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent, Wry};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

/// Passed by the login entry so Relay starts hidden in the tray.
const AUTOSTART_ARG: &str = "--autostart";
/// Written once the first-run default has been applied, so turning it off later sticks.
const AUTOSTART_MARKER: &str = "autostart-initialized";
const MAIN: &str = "main";
const TRAY: &str = "main";
/// Sent to the main window with a v2 route (`/notebook`, `/tasks?task=<id>`,
/// `/settings?provider=<key>`) to navigate to. Retired v1 routes land on Notebook.
const OPEN_EVENT: &str = "relay://open";
/// Sent to the main window to run a manual update check (`src/updater.ts`).
const UPDATE_CHECK_EVENT: &str = "update://check";

/// The tray's check items, kept so a change made from a page shows in the menu too.
struct TrayChecks {
    paused: CheckMenuItem<Wry>,
    dnd: CheckMenuItem<Wry>,
}

fn data_file(app: &AppHandle, name: &str) -> tauri::Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join(name))
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn hide_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN) {
        let _ = window.hide();
    }
}

fn toggle_main(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN) else {
        return;
    };
    if window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false) {
        let _ = window.hide();
    } else {
        show_main(app);
    }
}

fn open_route(app: &AppHandle, route: &str) {
    show_main(app);
    let _ = app.emit_to(MAIN, OPEN_EVENT, route);
}

fn request_update_check(app: &AppHandle) {
    let _ = app.emit_to(MAIN, UPDATE_CHECK_EVENT, ());
}

fn set_paused_flag(app: &AppHandle, paused: bool) -> hover::Snapshot {
    if let Some(checks) = app.try_state::<TrayChecks>() {
        let _ = checks.paused.set_checked(paused);
    }
    // Managed on the builder, so it is always there.
    app.state::<HoverState>().set_paused(app, paused)
}

fn set_dnd_flag(app: &AppHandle, dnd: bool) -> hover::Snapshot {
    if let Some(checks) = app.try_state::<TrayChecks>() {
        let _ = checks.dnd.set_checked(dnd);
    }
    app.state::<HoverState>().set_dnd(app, dnd)
}

#[tauri::command]
fn show_main_window(app: AppHandle) {
    show_main(&app);
}

/// Shows and focuses the main window on `route`. The hover uses it for every click that leads somewhere.
#[tauri::command]
fn open_main(app: AppHandle, route: String) {
    open_route(&app, &route);
}

#[tauri::command]
fn set_paused(app: AppHandle, paused: bool) -> hover::Snapshot {
    set_paused_flag(&app, paused)
}

#[tauri::command]
fn set_dnd(app: AppHandle, dnd: bool) -> hover::Snapshot {
    set_dnd_flag(&app, dnd)
}

/// For the hover's update row: the main window owns the updater, so the check runs there.
#[tauri::command]
fn check_for_updates(app: AppHandle) {
    request_update_check(&app);
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "Show / Hide", true, None::<&str>)?;
    let paused = CheckMenuItem::with_id(app, "pause", "Pause capture", true, false, None::<&str>)?;
    let dnd = CheckMenuItem::with_id(app, "dnd", "Do not disturb", true, false, None::<&str>)?;
    let notebook = MenuItem::with_id(app, "notebook", "Notebook", true, None::<&str>)?;
    let tasks = MenuItem::with_id(app, "tasks", "Tasks", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let updates = MenuItem::with_id(app, "updates", "Check for updates", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &toggle,
            &PredefinedMenuItem::separator(app)?,
            &paused,
            &dnd,
            &PredefinedMenuItem::separator(app)?,
            &notebook,
            &tasks,
            &settings,
            &updates,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    app.manage(TrayChecks { paused, dnd });

    let mut tray = TrayIconBuilder::with_id(TRAY)
        .tooltip("Relay")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => toggle_main(app),
            // The menu flips a check item's mark on its own; the stored flag is the truth, so it is set from that.
            "pause" => {
                let paused = app.state::<HoverState>().snapshot().paused;
                set_paused_flag(app, !paused);
            }
            "dnd" => {
                let dnd = app.state::<HoverState>().snapshot().dnd;
                set_dnd_flag(app, !dnd);
            }
            "notebook" => open_route(app, "/notebook"),
            "tasks" => open_route(app, "/tasks"),
            "settings" => open_route(app, "/settings"),
            "updates" => {
                show_main(app);
                request_update_check(app);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Turns on start-on-login the first time a release build runs; the tray/OS owns it afterwards.
fn enable_autostart_once(app: &AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    let Ok(marker) = data_file(app, AUTOSTART_MARKER) else {
        return;
    };
    if marker.exists() {
        return;
    }
    if app.autolaunch().enable().is_ok() {
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&marker, "");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ))
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(HoverState::default())
        .manage(opencode::BriefingState::default())
        .manage(grilling::GrillingState::default())
        .manage(dispatch::DispatchGate::default())
        // progress::init and its commands all take Arc<ProgressState>; manage that type
        // so a fresh process can read it during setup.
        .manage(Arc::new(progress::ProgressState::default()))
        .manage(DockRuntime::default())
        .invoke_handler(tauri::generate_handler![
            show_main_window,
            open_main,
            set_paused,
            set_dnd,
            check_for_updates,
            commands::app_version,
            commands::v2_notebook_snapshot,
            commands::v2_workspace_snapshot,
            commands::v2_workspace_search,
            commands::v2_composer_save,
            commands::v2_reference_picker,
            commands::v2_reference_parse,
            live::v2_reference_search,
            live::v2_reference_exact,
            live::v2_reference_pick,
            slack::directory::v2_slack_refresh,
            slack::directory::v2_slack_search,
            slack::window::v2_slack_window_start,
            commands::v2_task_assign_project,
            opencode::v2_briefing_view,
            opencode::v2_briefing_generate,
            opencode::v2_briefing_approve,
            opencode::v2_briefing_rewrite,
            opencode::v2_briefing_continue_link_only,
            opencode::v2_briefing_retry,
            grilling::v2_grilling_view,
            grilling::v2_grilling_start,
            grilling::v2_grilling_answer,
            grilling::v2_grilling_retry,
            grilling::v2_grilling_cancel,
            grilling::v2_grilling_go,
            grilling::v2_grilling_continue_link_only,
            dispatch::v2_dispatch_view,
            dispatch::v2_dispatch_retry,
            dispatch::v2_dispatch_check,
            dispatch::v2_dispatch_link,
            dispatch::v2_dispatch_cancel,
            progress::v2_t3_progress,
            progress::v2_t3_refresh,
            progress::v2_t3_send,
            progress::v2_t3_check_again,
            progress::v2_t3_resend,
            progress::v2_t3_close,
            keybinds::v2_keybinds_get,
            keybinds::v2_keybinds_set,
            settings::v2_provider_status,
            settings::v2_provider_refresh,
            settings::v2_github_set_token,
            settings::v2_github_clear_token,
            settings::v2_clickup_set_token,
            settings::v2_clickup_clear_token,
            settings::v2_clickup_workspaces,
            settings::v2_clickup_spaces,
            settings::v2_settings_update,
            settings::v2_opencode_set_model,
            settings::v2_opencode_set_key,
            settings::v2_opencode_clear_key,
            settings::v2_repos_scan,
            settings::v2_repos_register,
            note_import::v2_import_now,
            settings::v2_t3_set_target,
            settings::v2_t3_check_target,
            commands::open_t3,
            hover::dock_ready,
            hover::dock_resize,
            hover::dock_dismiss,
            hover::hover_set_pinned,
            hover::hover_state,
        ])
        .on_window_event(|window, event| {
            if window.label() == hover::LABEL {
                match event {
                    // A native blur means the user clicked elsewhere: hide and keep their focus.
                    WindowEvent::Focused(false) => hover::handle_blur(window.app_handle()),
                    // Re-read the work area and scale before the page applies a new CSS height.
                    WindowEvent::ScaleFactorChanged { .. } => {
                        hover::handle_scale_change(window.app_handle());
                    }
                    _ => {}
                }
                return;
            }
            if window.label() != MAIN {
                return;
            }
            match event {
                // The tray is the app's home; closing the main window hides it.
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                _ => {}
            }
        })
        .setup(|app| {
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            // v2 owns its own file and version. No legacy database is opened, seeded or
            // migrated, and no v1 credential is read.
            let v2_store = Arc::new(V2Store::open(dir.join("relay-v2.db"))?);
            app.manage(v2_store);
            // Offline status only: a provider probe never runs during startup.
            settings::start(app.handle());
            // Background Reference recovery reads due rows first and releases SQLite before
            // any network await.
            recovery::start(app.handle().clone());
            // #34: an OpenCode attempt abandoned by a previous run becomes a manual Retry.
            opencode::init(app.handle());
            // #35: an interrupted Grilling turn is retryable without losing its transcript.
            grilling::init(app.handle());
            // #36: reconcile attempts left uncertain by a restart, then start ready Tasks.
            dispatch::init(app.handle());
            // #37: observe bound threads from the store; never replay a dispatch or a pending send.
            progress::init(app.handle());
            // One v2 service owns startup, scheduled and manual Import for both providers.
            note_import::start(app.handle().clone());
            // #33: a first setup or a directory older than seven days gets its metadata scan on a
            // dedicated guard; thread reads, window reads and searches stay independent.
            slack::directory::scan_if_due(app.handle());

            build_tray(app.handle())?;
            keybinds::start(app.handle());
            enable_autostart_once(app.handle());

            // A hover that fails to build must not stop Relay; the main window and tray still work.
            if let Err(err) = hover::build(app.handle()) {
                eprintln!("relay: could not build the hover window: {err}");
            }

            if std::env::args().any(|arg| arg == AUTOSTART_ARG) {
                hide_main(app.handle());
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Relay");

    app.run(|_app, event| {
        // Hiding the main window must not end the process. Only the tray's Quit (app.exit) does.
        if let RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            api.prevent_exit();
        }
    });
}
