//! goodvoice client core.
//!
//! The Tauri shell is a thin host: every concern lives in its own module and
//! talks to the others only through the public API declared here.

pub mod audio;
pub mod capture;
pub mod rtc;
pub mod tray;

use serde::Serialize;

/// Identity of the running client, surfaced to the UI on boot.
#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

impl ClientInfo {
    #[must_use]
    pub fn current() -> Self {
        Self {
            name: env!("CARGO_PKG_NAME").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

#[tauri::command]
fn client_info() -> ClientInfo {
    ClientInfo::current()
}

/// Builds and runs the Tauri application.
///
/// # Panics
///
/// Panics if the webview host cannot be created — there is no useful degraded
/// mode for a windowless GUI client.
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![client_info])
        .run(tauri::generate_context!())
        .expect("error while running goodvoice");
}

#[cfg(test)]
mod tests {
    use super::ClientInfo;

    #[test]
    fn client_info_reports_crate_metadata() {
        let info = ClientInfo::current();
        assert_eq!(info.name, "goodvoice-client");
        assert!(!info.version.is_empty());
    }
}
