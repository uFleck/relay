//! Relay's Tauri shell. The hover window and the real UI are owned by other agents;
//! this scaffold has the main window, tray, single instance, autostart and the local DB.

use relay_core::store::Store;
use std::path::PathBuf;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

/// Passed by the login entry so Relay starts hidden in the tray.
const AUTOSTART_ARG: &str = "--autostart";
/// Written once the first-run default has been applied, so turning it off later sticks.
const AUTOSTART_MARKER: &str = "autostart-initialized";
/// The main window is the only window in the scaffold.
const MAIN: &str = "main";

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

/// The future hover window summons the main window through this command.
#[tauri::command]
fn show_main_window(app: AppHandle) {
    show_main(&app);
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &quit])?;

    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Relay")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main(app),
            "hide" => hide_main(app),
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
        .invoke_handler(tauri::generate_handler![show_main_window])
        .on_window_event(|window, event| {
            if window.label() == MAIN
                && let WindowEvent::CloseRequested { api, .. } = event
            {
                // The tray is the app's home; closing the main window hides it.
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            app.manage(Store::open(dir.join("relay.db"))?);

            build_tray(app.handle())?;
            enable_autostart_once(app.handle());

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
