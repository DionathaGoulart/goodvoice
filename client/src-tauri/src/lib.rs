//! goodvoice client core.
//!
//! The Tauri shell is a thin host: every concern lives in its own module and
//! talks to the others only through the public API declared here. The commands
//! below are the whole surface the UI sees — join, leave, mute, deafen — and
//! each is a one-line forward into [`rtc::session`].

pub mod audio;
pub mod capture;
pub mod rtc;
pub mod tray;

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Emitter as _, Manager as _, State};
use tokio::sync::{watch, Mutex};

use audio::{device::AudioSink, hardware, vad::TransmitMode};
use rtc::{
    reconnect::CallState,
    session::{Call, CallOptions},
    signaling::Participant,
};

/// The deploy a fresh install talks to. Overridable at build time so a
/// self-hoster can ship a client pointed at their own Worker without touching
/// the source (docs/self-hosting.md, task 6.1).
pub const DEFAULT_SERVER: &str = match option_env!("GOODVOICE_SERVER") {
    Some(url) => url,
    None => "https://goodvoice.goodvoice-server.workers.dev",
};

/// The event the UI listens on for room changes.
const ROSTER_EVENT: &str = "goodvoice://roster";

/// The event the UI listens on for the call's own health: live, reconnecting,
/// or over. A dropped call must never look like a quiet one (prd.md §5 flow E).
const STATE_EVENT: &str = "goodvoice://state";

/// The event the UI listens on for who is talking, as participant ids.
///
/// Separate from the roster because the two move at completely different
/// rates: a roster changes when somebody joins or leaves, this changes with
/// every sentence. Sending them together would mean re-rendering the whole
/// roster ten times a second.
const SPEAKING_EVENT: &str = "goodvoice://speaking";

/// Identity of the running client, surfaced to the UI on boot.
#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
    /// Where this build joins rooms by default.
    pub server: String,
}

impl ClientInfo {
    #[must_use]
    pub fn current() -> Self {
        Self {
            name: env!("CARGO_PKG_NAME").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            server: DEFAULT_SERVER.to_owned(),
        }
    }
}

/// What the UI is told after a successful join.
#[derive(Debug, Clone, Serialize)]
pub struct CallStatus {
    /// This client's participant id, so the UI can pick itself out of the
    /// roster.
    pub self_id: String,
    pub room: String,
    pub participants: Vec<Participant>,
}

/// What the UI is told when the call's health changes.
///
/// The participant id rides along because a reconnect takes a new seat in the
/// room, and the UI needs the new one to keep picking itself out of the roster.
#[derive(Debug, Clone, Serialize)]
pub struct CallHealth {
    #[serde(flatten)]
    pub state: CallState,
    pub self_id: String,
}

/// The one call this process can be in.
///
/// A `Mutex` rather than a lock-free cell because join and leave must not
/// interleave: two joins racing would leave an orphaned `Call` publishing a
/// microphone nobody can mute. The `Call` is held directly rather than behind
/// an `Arc` so leaving can consume it — the tasks that push at the webview get
/// watch receivers, never a handle on the call itself.
#[derive(Default)]
struct CurrentCall {
    call: Mutex<Option<Call>>,
}

/// Joins a room, opening the microphone and speakers on the way.
///
/// # Errors
///
/// Returns the reason as a string for the UI to show: no audio device, the
/// room refusing the join, or the transport failing to connect.
#[tauri::command]
async fn join_room(
    app: AppHandle,
    state: State<'_, CurrentCall>,
    server: String,
    room: String,
    name: String,
    mode: TransmitMode,
) -> Result<CallStatus, String> {
    let mut current = state.call.lock().await;
    if current.is_some() {
        return Err("already in a call".to_owned());
    }

    let (microphone, speakers) = hardware::open().map_err(|error| error.to_string())?;

    let call = Call::join(
        CallOptions {
            base: server,
            room: room.clone(),
            name,
            mode,
        },
        Box::new(microphone),
        Arc::new(speakers) as Arc<dyn AudioSink>,
    )
    .await
    .map_err(|error| error.to_string())?;

    let status = CallStatus {
        self_id: call.self_id(),
        room,
        participants: call.roster().borrow().clone(),
    };

    // Watch receivers, not the call: these outlive `leave_room` taking the
    // call apart, and they end on their own when its state is dropped.
    tauri::async_runtime::spawn(push_roster(app.clone(), call.roster()));
    tauri::async_runtime::spawn(push_speaking(app.clone(), call.speaking()));
    tauri::async_runtime::spawn(push_state(app, call.state(), call.self_id_watch()));
    *current = Some(call);

    Ok(status)
}

/// Leaves the room and closes the devices.
///
/// # Errors
///
/// Never fails in practice; the `Result` exists so the UI can await it the
/// same way it awaits the others.
#[tauri::command]
async fn leave_room(state: State<'_, CurrentCall>) -> Result<(), String> {
    end_call_in(&state).await;
    Ok(())
}

/// Ends whatever call is in progress, if any.
///
/// The UI leaves through [`leave_room`]; the tray's Quit comes here directly,
/// because the window closing stopped meaning "goodbye" the moment task 4.1
/// turned it into a hide.
pub async fn end_call(app: &AppHandle) {
    end_call_in(&app.state::<CurrentCall>()).await;
}

async fn end_call_in(current: &CurrentCall) {
    let taken = current.call.lock().await.take();
    if let Some(call) = taken {
        call.leave().await;
    }
}

/// # Errors
///
/// Returns an error when there is no call to mute.
#[tauri::command]
async fn set_muted(state: State<'_, CurrentCall>, muted: bool) -> Result<(), String> {
    let call = state.call.lock().await;
    let call = call.as_ref().ok_or_else(|| "not in a call".to_owned())?;
    call.set_muted(muted).await;
    Ok(())
}

/// # Errors
///
/// Returns an error when there is no call to deafen.
#[tauri::command]
async fn set_deafened(state: State<'_, CurrentCall>, deafened: bool) -> Result<(), String> {
    let call = state.call.lock().await;
    let call = call.as_ref().ok_or_else(|| "not in a call".to_owned())?;
    call.set_deafened(deafened).await;
    Ok(())
}

/// Chooses how transmission is gated: always on, a held key, or a detected
/// voice.
///
/// # Errors
///
/// Returns an error when there is no call. The setting itself lives in the UI,
/// which restores it and hands it to [`join_room`]; this is only how a change
/// made mid-call reaches the microphone.
#[tauri::command]
async fn set_transmit_mode(
    state: State<'_, CurrentCall>,
    mode: TransmitMode,
) -> Result<(), String> {
    let call = state.call.lock().await;
    let call = call.as_ref().ok_or_else(|| "not in a call".to_owned())?;
    call.set_transmit_mode(mode);
    Ok(())
}

/// Reports the push-to-talk key going down or coming up.
///
/// # Errors
///
/// Returns an error when there is no call to talk into.
#[tauri::command]
async fn set_talk_key(state: State<'_, CurrentCall>, down: bool) -> Result<(), String> {
    let call = state.call.lock().await;
    let call = call.as_ref().ok_or_else(|| "not in a call".to_owned())?;
    call.set_talk_key(down);
    Ok(())
}

#[tauri::command]
fn client_info() -> ClientInfo {
    ClientInfo::current()
}

/// Forwards every roster change to the webview until the call ends.
async fn push_roster(app: AppHandle, mut roster: watch::Receiver<Vec<Participant>>) {
    loop {
        let participants = roster.borrow_and_update().clone();
        let _ = app.emit(ROSTER_EVENT, participants);

        if roster.changed().await.is_err() {
            return;
        }
    }
}

/// Forwards the set of people talking to the webview until the call ends.
///
/// The call only sends when the set actually changes, so a room full of
/// listeners costs this loop one wakeup and no events at all.
async fn push_speaking(app: AppHandle, mut speaking: watch::Receiver<Vec<String>>) {
    loop {
        let talking = speaking.borrow_and_update().clone();
        let _ = app.emit(SPEAKING_EVENT, talking);

        if speaking.changed().await.is_err() {
            return;
        }
    }
}

/// Forwards the call's health to the webview until it ends.
///
/// Both watches feed one event: a reconnect changes the state and the
/// participant id together, and a UI that learned them separately would spend a
/// frame unable to find itself in the roster.
async fn push_state(
    app: AppHandle,
    mut state: watch::Receiver<CallState>,
    mut self_id: watch::Receiver<String>,
) {
    loop {
        let health = CallHealth {
            state: state.borrow_and_update().clone(),
            self_id: self_id.borrow_and_update().clone(),
        };
        let ended = health.state.is_ended();
        let _ = app.emit(STATE_EVENT, health);
        if ended {
            return;
        }

        tokio::select! {
            changed = state.changed() => if changed.is_err() { return },
            changed = self_id.changed() => if changed.is_err() { return },
        }
    }
}

/// Builds and runs the Tauri application.
///
/// # Panics
///
/// Panics if the webview host cannot be created — there is no useful degraded
/// mode for a windowless GUI client.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(CurrentCall::default());
            app.manage(tray::Tray::default());

            // A host that will not give us a tray is not a reason to refuse to
            // run — it is a reason to keep a window that closes normally, which
            // is what `Tray` staying uninstalled means (task 4.1).
            if let Err(error) = tray::install(app.handle()) {
                eprintln!("no tray icon: {error}; the window will close rather than hide");
            }
            Ok(())
        })
        .on_window_event(tray::window_event)
        .invoke_handler(tauri::generate_handler![
            client_info,
            join_room,
            leave_room,
            set_muted,
            set_deafened,
            set_transmit_mode,
            set_talk_key
        ])
        .run(tauri::generate_context!())
        .expect("error while running goodvoice");
}

#[cfg(test)]
mod tests {
    use super::{ClientInfo, DEFAULT_SERVER};

    #[test]
    fn client_info_reports_crate_metadata() {
        let info = ClientInfo::current();
        assert_eq!(info.name, "goodvoice-client");
        assert!(!info.version.is_empty());
    }

    #[test]
    fn the_default_server_is_an_absolute_origin() {
        // A relative or empty value would fail at join time with a confusing
        // signalling error rather than here.
        assert!(
            DEFAULT_SERVER.starts_with("http://") || DEFAULT_SERVER.starts_with("https://"),
            "GOODVOICE_SERVER must be an absolute origin, got {DEFAULT_SERVER}"
        );
        assert!(
            !DEFAULT_SERVER.ends_with('/'),
            "trailing slash breaks paths"
        );
    }
}
