//! Relay's Tauri shell: the main window, the hover window, tray, global hotkey, single instance, autostart and the
//! local DB. The UI in both windows is owned by the frontend; this file only routes events to it.

mod analyst;
mod clickup_sync;
mod commands;
mod github;
mod hover;
mod imports;
mod mcp;
mod secrets;
mod slack;

use hover::HoverState;
use relay_core::model::NewProject;
use relay_core::store::Store;
use relay_core::v2::V2Store;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent, Wry};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// Passed by the login entry so Relay starts hidden in the tray.
const AUTOSTART_ARG: &str = "--autostart";
/// Written once the first-run default has been applied, so turning it off later sticks.
const AUTOSTART_MARKER: &str = "autostart-initialized";
const MAIN: &str = "main";
const TRAY: &str = "main";
/// Summons the main window from anywhere. A taken hotkey must never stop Relay starting: the tray still works.
const HOTKEY: &str = "ctrl+alt+r";
/// Checks the Slack DMs now, without waiting for the socket. The tray has the same action.
const SYNC_HOTKEY: &str = "ctrl+alt+s";

/// Sent to the main window with a route (`/review`, `/settings`, `/today?task=<id>&focus=changes`) to navigate to.
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
    let sync = MenuItem::with_id(app, "sync", "Sync Slack now", true, None::<&str>)?;
    let review = MenuItem::with_id(app, "review", "Review queue", true, None::<&str>)?;
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
            &sync,
            &review,
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
            "sync" => slack::sync_in_background(app.clone()),
            "review" => open_route(app, "/review"),
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

/// Registers the summon and Slack-check hotkeys. A taken key never stops Relay starting: the
/// tray still works, and the tooltip names the summon key when that one failed.
fn register_hotkeys(app: &AppHandle) {
    let summon = app
        .global_shortcut()
        .on_shortcut(HOTKEY, |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                toggle_main(app);
            }
        });
    if let Err(err) = summon {
        eprintln!("relay: could not register {HOTKEY}: {err}");
        if let Some(tray) = app.tray_by_id(TRAY) {
            let _ = tray.set_tooltip(Some("Relay (Ctrl+Alt+R is taken; use the tray)"));
        }
    }

    if let Err(err) = app
        .global_shortcut()
        .on_shortcut(SYNC_HOTKEY, |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                slack::sync_in_background(app.clone());
            }
        })
    {
        eprintln!("relay: could not register {SYNC_HOTKEY}: {err}");
    }
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
        .manage(mcp::McpState::default())
        .invoke_handler(tauri::generate_handler![
            show_main_window,
            open_main,
            set_paused,
            set_dnd,
            check_for_updates,
            commands::relay_snapshot,
            commands::v2_notebook_snapshot,
            commands::v2_composer_save,
            commands::v2_task_assign_project,
            commands::ack_change,
            commands::decide_proposal,
            commands::notes_list,
            commands::note_create,
            commands::note_update,
            commands::note_set_pinned,
            commands::note_delete,
            commands::note_to_task,
            commands::open_t3,
            mcp::mcp_status,
            mcp::mcp_token,
            slack::slack_status,
            slack::slack_sync_now,
            slack::slack_set_transport,
            slack::slack_connect,
            slack::slack_disconnect,
            slack::slack_peers,
            slack::slack_set_peer_excluded,
            slack::slack_clear_capture,
            slack::episodes_list,
            slack::episode_get,
            slack::episode_analyze,
            slack::slack_reanalyze,
            slack::slack_reanalyze_recent,
            slack::jobs_list,
            slack::jev_status,
            slack::jev_set_key,
            slack::jev_clear_key,
            slack::slack_simulate_message,
            slack::job_retry,
            slack::job_resolve_review,
            github::github_status,
            github::github_set_token,
            github::github_clear_token,
            github::sync_prs_now,
            github::project_git_state,
            github::project_set_repo,
            imports::clickup_status,
            imports::clickup_set_token,
            imports::clickup_clear_token,
            imports::clickup_import,
            clickup_sync::clickup_workspaces,
            clickup_sync::clickup_spaces,
            clickup_sync::clickup_scope,
            clickup_sync::clickup_set_scope,
            clickup_sync::clickup_sync_now,
            imports::github_import_issues,
            imports::repos_scan,
            imports::repos_import,
            hover::hover_place,
            hover::hover_drag,
            hover::hover_pointer,
            hover::hover_focus,
            hover::hover_ready,
            hover::hover_set_pinned,
            hover::hover_state,
        ])
        .on_window_event(|window, event| {
            if window.label() != MAIN {
                return;
            }
            match event {
                // The tray is the app's home; closing the main window hides it.
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                // Hiding the main window also blurs it, so this covers hide-to-tray as well.
                WindowEvent::Focused(focused) => {
                    window
                        .state::<HoverState>()
                        .set_main_focused(window.app_handle(), *focused);
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
            let store = Store::open(dir.join("relay.db"))?;
            // The first project is the canonical workflow every other project's status
            // names map onto (view.rs). Seed it so a fresh install has a real Today list.
            if store.list_projects()?.is_empty() {
                store.create_project_with_default_workflow(NewProject::new(
                    "Inbox", "inbox", None,
                ))?;
            }
            let store = Arc::new(store);
            app.manage(store.clone());
            // v2 owns its own file and version. It has no credential dependency and
            // never reopens or migrates the legacy database.
            let v2_store = Arc::new(V2Store::open(dir.join("relay-v2.db"))?);
            app.manage(v2_store);
            mcp::start(app.handle().clone(), store.clone(), &dir);
            slack::start(app.handle().clone(), store.clone());
            github::start(app.handle().clone());
            clickup_sync::start(app.handle().clone());

            build_tray(app.handle())?;
            register_hotkeys(app.handle());
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
