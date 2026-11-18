//! Third-party tokens in Windows Credential Manager, under the app's service name.
//! The frontend never sees a stored token; a command only reports whether one exists.

const SERVICE: &str = "dev.relay.app";

pub fn read(key: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, key)
        .ok()?
        .get_password()
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub fn write(key: &str, value: &str) -> Result<(), String> {
    keyring::Entry::new(SERVICE, key)
        .map_err(|error| error.to_string())?
        .set_password(value)
        .map_err(|error| error.to_string())
}

pub fn delete(key: &str) {
    if let Ok(entry) = keyring::Entry::new(SERVICE, key) {
        let _ = entry.delete_credential();
    }
}
