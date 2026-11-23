//! Configurable keyboard shortcuts. The config is a small JSON file in app data, so a
//! keybind change survives restarts without a database migration. The summon chord is
//! re-registered at runtime; an invalid or taken chord is reported instead of silently
//! disabling the hotkey.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::hover;

pub const EVENT: &str = "relay://keybinds-change";
pub const DEFAULT_SUMMON: &str = "ctrl+alt+r";
const FILE: &str = "keybinds.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Keybinds {
    pub summon: String,
}

impl Default for Keybinds {
    fn default() -> Self {
        Self { summon: DEFAULT_SUMMON.into() }
    }
}

/// The chord currently owned by Relay. Kept so a failed edit can leave the working one in place.
static ACTIVE: Mutex<Option<String>> = Mutex::new(None);

fn validate(chord: &str) -> Result<String, String> {
    let chord = chord.trim().to_lowercase();
    if chord.is_empty() {
        return Err("The summon shortcut cannot be empty.".into());
    }
    chord
        .parse::<Shortcut>()
        .map_err(|_| format!("“{chord}” is not a valid shortcut. Try ctrl+alt+r."))?;
    Ok(chord)
}

fn config_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join(FILE))
        .map_err(|error| format!("Could not resolve the config directory: {error}"))
}

/// Reads the config; a missing or invalid file falls back to the defaults.
pub fn load(app: &AppHandle) -> Keybinds {
    let Ok(path) = config_path(app) else {
        return Keybinds::default();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Keybinds::default();
    };
    match serde_json::from_str::<Keybinds>(&text) {
        Ok(binds) => Keybinds {
            summon: validate(&binds.summon).unwrap_or_else(|_| DEFAULT_SUMMON.into()),
        },
        Err(_) => Keybinds::default(),
    }
}

/// Registers the summon chord and replaces the previous one. A taken chord never stops
/// Relay starting: the tray still works.
pub fn register(app: &AppHandle, summon: &str) -> bool {
    let chord = summon.to_string();
    let previous = ACTIVE.lock().expect("active shortcut").clone();
    if previous.as_deref() == Some(chord.as_str()) {
        return true;
    }
    let shortcut = match chord.parse::<Shortcut>() {
        Ok(shortcut) => shortcut,
        Err(_) => return false,
    };
    // Register the new chord before dropping the old one. A chord taken by another app
    // leaves the working shortcut in place.
    let registered = app
        .global_shortcut()
        .on_shortcut(shortcut, move |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                // global-hotkey dispatches `Pressed` synchronously from WM_HOTKEY on the main
                // thread, so the Dock can capture the foreground window and take focus here.
                hover::summon(app);
            }
        });
    if let Err(error) = registered {
        eprintln!("relay: could not register {chord}: {error}");
        let tooltip = format!("Relay ({chord} is taken; use the tray)");
        if let Some(tray) = app.tray_by_id("main") {
            let _ = tray.set_tooltip(Some(&tooltip));
        }
        return false;
    }
    if let Some(old) = previous {
        if let Ok(shortcut) = old.parse::<Shortcut>() {
            let _ = app.global_shortcut().unregister(shortcut);
        }
    }
    *ACTIVE.lock().expect("active shortcut") = Some(chord);
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some("Relay"));
    }
    true
}

/// Loads the config and registers the chord. Called once during setup.
pub fn start(app: &AppHandle) {
    let binds = load(app);
    register(app, &binds.summon);
}

#[tauri::command]
pub fn v2_keybinds_get(app: AppHandle) -> Keybinds {
    load(&app)
}

#[tauri::command]
pub fn v2_keybinds_set(app: AppHandle, binds: Keybinds) -> Result<Keybinds, String> {
    let binds = Keybinds { summon: validate(&binds.summon)? };
    if !register(&app, &binds.summon) {
        return Err(format!("“{}” is taken by another app. The old shortcut stays active.", binds.summon));
    }
    let path = config_path(&app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create the config directory: {error}"))?;
    }
    let json = serde_json::to_string_pretty(&binds)
        .map_err(|error| format!("Could not write the keybind config: {error}"))?;
    std::fs::write(&path, json)
        .map_err(|error| format!("Could not write the keybind config: {error}"))?;
    let _ = app.emit(EVENT, &binds);
    Ok(binds)
}
