//! Third-party tokens in Windows Credential Manager, under the app's service name.
//! The frontend never sees a stored token; a command only reports whether one exists.
//!
//! v2 uses a separate service. A v1 credential fixture in `dev.relay.app` must never
//! make a fresh v2 profile look connected, and a v2 write must never touch v1 data.

const SERVICE: &str = "dev.relay.app";
const V2_SERVICE: &str = "dev.relay.app.v2";

pub fn read(key: &str) -> Option<String> {
    read_from(SERVICE, key)
}

pub fn write(key: &str, value: &str) -> Result<(), String> {
    write_to(SERVICE, key, value)
}

pub fn delete(key: &str) {
    delete_from(SERVICE, key)
}

/// The v2 keyring. Keys stay in Credential Manager; only a last-four mask ever
/// reaches a DTO, an event or the frontend.
pub mod v2 {
    use super::{V2_SERVICE, delete_from, read_from, write_to};

    pub fn read(key: &str) -> Option<String> {
        read_from(V2_SERVICE, key)
    }

    pub fn write(key: &str, value: &str) -> Result<(), String> {
        write_to(V2_SERVICE, key, value)
    }

    pub fn delete(key: &str) {
        delete_from(V2_SERVICE, key);
    }
}

fn read_from(service: &str, key: &str) -> Option<String> {
    keyring::Entry::new(service, key)
        .ok()?
        .get_password()
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn write_to(service: &str, key: &str, value: &str) -> Result<(), String> {
    keyring::Entry::new(service, key)
        .map_err(|error| error.to_string())?
        .set_password(value)
        .map_err(|error| error.to_string())
}

fn delete_from(service: &str, key: &str) {
    if let Ok(entry) = keyring::Entry::new(service, key) {
        let _ = entry.delete_credential();
    }
}
