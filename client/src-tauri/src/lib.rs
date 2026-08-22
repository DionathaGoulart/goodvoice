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

use std::sync::{Arc, Mutex as StdMutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter as _, Manager as _, State};
use tokio::sync::{watch, Mutex};

use audio::{device::AudioSink, hardware, vad::TransmitMode};
use rtc::{
    reconnect::CallState,
    session::{Call, CallOptions},
    signaling::Participant,
};
use tray::hotkey;

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

/// The event the UI listens on for mute and deafen.
///
/// Both can be changed from the tray now (task 4.2), so the window cannot be
/// the source of truth for either — it is told, the same as the tray is.
const CONTROLS_EVENT: &str = "goodvoice://controls";

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

/// What the window and the tray menu both show, and neither owns.
///
/// The call is the source of truth; this is the copy that gets pushed to the
/// two things that display it. `in_call` is here because a tray menu offering
/// "Leave room" to somebody who is not in one is a menu that lies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Controls {
    pub in_call: bool,
    pub muted: bool,
    pub deafened: bool,
}

/// The global talk key: what the window says it is, and what is bound now.
///
/// The two are not the same thing. A key is configured whether or not there is
/// a call to use it in, and the desktop-wide hook only goes on while a call is
/// actually in push-to-talk mode — goodvoice has no business in the keyboard's
/// way the rest of the time (task 4.3).
#[derive(Default)]
struct Hotkey {
    code: StdMutex<Option<String>>,
    bound: StdMutex<Option<(String, hotkey::Listener)>>,
}

impl Hotkey {
    fn remember(&self, code: Option<String>) {
        if let Ok(mut held) = self.code.lock() {
            *held = code;
        }
    }

    fn code(&self) -> Option<String> {
        self.code.lock().ok().and_then(|code| code.clone())
    }

    /// Installs, replaces or removes the hook.
    ///
    /// Rebinding the key it is already bound to is left alone: this runs on
    /// every mode change and every join, and taking a keyboard hook off the
    /// desktop only to put the same one back is a way to lose a keystroke.
    fn bind(&self, app: &AppHandle, code: Option<String>) {
        let Ok(mut bound) = self.bound.lock() else {
            return;
        };
        if bound.as_ref().map(|(code, _)| code.as_str()) == code.as_deref() {
            return;
        }

        // Dropped first, and on purpose: one hook at a time, and the old one
        // owns the thread the new one wants to start.
        *bound = None;
        let Some(code) = code else {
            return;
        };

        let handle = app.clone();
        match hotkey::listen(&code, move |down| {
            // The hook callback is on the desktop's input path and holds no
            // lock worth waiting on; the call it has to reach is behind an
            // async mutex. So the edge is handed to the runtime, which is a
            // queue push and nothing else.
            let handle = handle.clone();
            tauri::async_runtime::spawn(async move { talk_key(&handle, down).await });
        }) {
            Ok(listener) => *bound = Some((code, listener)),
            Err(error) => {
                eprintln!("push to talk is window-only: {error}");
            }
        }
    }
}

/// The one call this process can be in.
///
/// A `Mutex` rather than a lock-free cell because join and leave must not
/// interleave: two joins racing would leave an orphaned `Call` publishing a
/// microphone nobody can mute. The `Call` is held directly rather than behind
/// an `Arc` so leaving can consume it — the tasks that push at the webview get
/// watch receivers, never a handle on the call itself.
struct CurrentCall {
    call: Mutex<Option<Call>>,
    hotkey: Hotkey,
    /// Watched rather than read: [`push_controls`] forwards every change to
    /// the window and the tray, so whoever made it does not have to know who
    /// else is showing it.
    controls: watch::Sender<Controls>,
}

impl Default for CurrentCall {
    fn default() -> Self {
        Self {
            call: Mutex::default(),
            hotkey: Hotkey::default(),
            controls: watch::Sender::new(Controls::default()),
        }
    }
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
    let app_for_hotkey = app.clone();
    tauri::async_runtime::spawn(push_roster(app.clone(), call.roster()));
    tauri::async_runtime::spawn(push_speaking(app.clone(), call.speaking()));
    tauri::async_runtime::spawn(push_state(app, call.state(), call.self_id_watch()));
    *current = Some(call);
    // A fresh call is unmuted and undeafened, whatever the last one ended as.
    state.controls.send_replace(Controls {
        in_call: true,
        muted: false,
        deafened: false,
    });
    drop(current);
    refresh_hotkey(&app_for_hotkey).await;

    Ok(status)
}

/// Leaves the room and closes the devices.
///
/// # Errors
///
/// Never fails in practice; the `Result` exists so the UI can await it the
/// same way it awaits the others.
#[tauri::command]
async fn leave_room(app: AppHandle) -> Result<(), String> {
    end_call(app).await;
    Ok(())
}

/// Ends whatever call is in progress, if any.
///
/// The UI leaves through [`leave_room`]; the tray's Leave and Quit come here
/// directly, because the window closing stopped meaning "goodbye" the moment
/// task 4.1 turned it into a hide.
pub async fn end_call(app: AppHandle) {
    let state = app.state::<CurrentCall>();
    let taken = state.call.lock().await.take();
    if let Some(call) = taken {
        call.leave().await;
    }
    // The window hears the call end through `push_state` as well; this is what
    // greys the tray menu out.
    state.controls.send_replace(Controls::default());
    // Nobody is talking to anybody, so nothing needs watching the keyboard.
    state.hotkey.bind(&app, None);
}

/// Puts the desktop-wide talk key in the state the call calls for.
///
/// The window says what the key *is*; whether it is watched for is decided
/// here, from one place, because three different things can change the answer:
/// joining, leaving, and switching transmit mode.
async fn refresh_hotkey(app: &AppHandle) {
    let state = app.state::<CurrentCall>();
    let wanted = {
        let call = state.call.lock().await;
        match call.as_ref() {
            Some(call) if call.transmit_mode() == TransmitMode::PushToTalk => state.hotkey.code(),
            _ => None,
        }
    };
    state.hotkey.bind(app, wanted);
}

/// Tells the call the talk key moved, from wherever it moved.
async fn talk_key(app: &AppHandle, down: bool) {
    let state = app.state::<CurrentCall>();
    let call = state.call.lock().await;
    if let Some(call) = call.as_ref() {
        call.set_talk_key(down);
    }
}

/// Flips mute, whichever side asked.
///
/// The call is read for the current value rather than the copy in
/// [`Controls`]: a toggle from the tray and one from the window can be in
/// flight at once, and only one of them is holding the call.
pub async fn toggle_muted(app: AppHandle) {
    let state = app.state::<CurrentCall>();
    let call = state.call.lock().await;
    let Some(call) = call.as_ref() else {
        return;
    };
    let muted = !call.is_muted();
    call.set_muted(muted).await;
    state
        .controls
        .send_modify(|controls| controls.muted = muted);
}

/// Flips deafen, whichever side asked.
pub async fn toggle_deafened(app: AppHandle) {
    let state = app.state::<CurrentCall>();
    let call = state.call.lock().await;
    let Some(call) = call.as_ref() else {
        return;
    };
    let deafened = !call.is_deafened();
    call.set_deafened(deafened).await;
    state
        .controls
        .send_modify(|controls| controls.deafened = deafened);
}

/// # Errors
///
/// Returns an error when there is no call to mute.
#[tauri::command]
async fn set_muted(state: State<'_, CurrentCall>, muted: bool) -> Result<(), String> {
    let call = state.call.lock().await;
    let call = call.as_ref().ok_or_else(|| "not in a call".to_owned())?;
    call.set_muted(muted).await;
    state
        .controls
        .send_modify(|controls| controls.muted = muted);
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
    state
        .controls
        .send_modify(|controls| controls.deafened = deafened);
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
    app: AppHandle,
    state: State<'_, CurrentCall>,
    mode: TransmitMode,
) -> Result<(), String> {
    {
        let call = state.call.lock().await;
        let call = call.as_ref().ok_or_else(|| "not in a call".to_owned())?;
        call.set_transmit_mode(mode);
    }
    // Leaving push-to-talk takes the hook off the desktop; arriving at it puts
    // one on.
    refresh_hotkey(&app).await;
    Ok(())
}

/// Remembers which key the window means by "the talk key".
///
/// Sent whenever the window's binding changes, and again on every join —
/// storage belongs to the webview (task 3.3) and this process learns it the
/// same way anyone else would.
///
/// # Errors
///
/// Never. The `Result` is there so the UI can await it like the others.
#[tauri::command]
async fn set_talk_binding(app: AppHandle, code: Option<String>) -> Result<(), String> {
    app.state::<CurrentCall>().hotkey.remember(code);
    refresh_hotkey(&app).await;
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

/// Whether the desktop-wide hook is on, for the window to say so.
///
/// Push to talk that only works while the window has focus is push to talk
/// that does not work, and the difference is invisible until someone is in a
/// game. So the window is told which one it has.
///
/// # Errors
///
/// Never. The `Result` is there so the UI can await it like the others.
#[tauri::command]
async fn talk_key_is_global(state: State<'_, CurrentCall>) -> Result<bool, String> {
    Ok(state.hotkey.bound.lock().is_ok_and(|bound| bound.is_some()))
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

/// Lets go of a call that has ended on its own.
///
/// A call that drops for good ends *underneath* the two commands that take it
/// apart, so without this the process keeps holding a `Call` nobody is in:
/// the next join is refused with "already in a call", and the tray menu goes on
/// offering Leave and Mute for a room that is not there. Leaving properly is
/// [`end_call`]'s job — by here there is nothing left to tell the room.
async fn forget_call(app: &AppHandle) {
    let state = app.state::<CurrentCall>();
    let _ = state.call.lock().await.take();
    state.controls.send_replace(Controls::default());
    state.hotkey.bind(app, None);
}

/// Forwards mute and deafen to both things that show them, for the life of the
/// process.
///
/// One task rather than one per call: the tray outlives every call, and a menu
/// left ticked by a call that ended is exactly the bug this is here to avoid.
async fn push_controls(app: AppHandle, mut controls: watch::Receiver<Controls>) {
    loop {
        let current = *controls.borrow_and_update();
        let _ = app.emit(CONTROLS_EVENT, current);
        tray::apply_controls(&app, current);

        if controls.changed().await.is_err() {
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
            forget_call(&app).await;
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

            let controls = app.state::<CurrentCall>().controls.subscribe();
            tauri::async_runtime::spawn(push_controls(app.handle().clone(), controls));
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
            set_talk_key,
            set_talk_binding,
            talk_key_is_global
        ])
        .run(tauri::generate_context!())
        .expect("error while running goodvoice");
}

#[cfg(test)]
mod tests {
    use super::{ClientInfo, Controls, DEFAULT_SERVER};

    /// The window reads these names out of the event (`Controls` in App.tsx).
    /// Renaming a field here without renaming it there is a control that
    /// silently stops following the call.
    #[test]
    fn the_controls_payload_is_the_one_the_window_reads() {
        let payload = serde_json::to_value(Controls {
            in_call: true,
            muted: true,
            deafened: false,
        })
        .expect("controls serialize");

        assert_eq!(
            payload,
            serde_json::json!({ "in_call": true, "muted": true, "deafened": false })
        );
    }

    #[test]
    fn a_client_that_is_not_in_a_call_offers_nothing_to_click() {
        // What the tray menu is greyed out by, and what the window shows before
        // anyone joins.
        let idle = Controls::default();
        assert!(!idle.in_call && !idle.muted && !idle.deafened);
    }

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
