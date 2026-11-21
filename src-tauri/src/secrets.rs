//! Third-party tokens in Windows Credential Manager, under the app's v2 service name.
//! The frontend never sees a stored token; a command only reports whether one exists and
//! a last-four mask. The retired v1 credential service and its tokens are gone, with no
//! migration.

const V2_SERVICE: &str = "dev.relay.app.v2";

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
