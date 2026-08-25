//! goodvoice client core.
//!
//! The Tauri shell is a thin host: every concern lives in its own module and
//! talks to the others only through the public API declared here. The commands
//! below are the whole surface the UI sees — join, leave, mute, deafen — and
//! each is a one-line forward into [`rtc::session`].

pub mod audio;
pub mod capture;
pub mod home;
pub mod invite;
pub mod rtc;
pub mod tray;

use std::{
    sync::{
        atomic::{self, AtomicU64},
        Arc, Mutex as StdMutex,
    },
    time::Instant,
};

use serde::Serialize;
use tauri::{AppHandle, Emitter as _, Manager as _, State};
use tokio::sync::{watch, Mutex};

use home::Home;

use audio::{
    device::AudioSink,
    hardware,
    prefs::{AudioPrefs, AudioSettings},
    vad::TransmitMode,
};
use rtc::{
    reconnect::CallState,
    screen::ShareState,
    session::{Call, CallOptions, Levels},
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

/// The room to join without being asked, if this variable names one.
///
/// For the runs that have to happen without a person clicking anything: the
/// cold-start timing (task 4.4) and the idle soak (4.5). It is also the shape
/// task 6.2's `goodvoice://join/<room>` link will need — a join that starts
/// before, and independently of, the window that would normally ask for it.
const AUTOJOIN_ENV: &str = "GOODVOICE_AUTOJOIN";

/// The event the UI listens on for room changes.
const ROSTER_EVENT: &str = "goodvoice://roster";

/// The event that says a call has begun, carrying the room it is in.
///
/// The window usually learns that from [`join_room`] returning, because the
/// window is what asked. Two things join without asking it: [`AUTOJOIN_ENV`],
/// and task 6.2's invite link. Neither has a window to hand the answer back
/// to, and none of the other events carries the room name — so without this a
/// client can be in a call while its window still shows the join form.
const CALL_EVENT: &str = "goodvoice://call";

/// The event the UI listens on for the call's own health: live, reconnecting,
/// or over. A dropped call must never look like a quiet one (prd.md §5 flow E).
const STATE_EVENT: &str = "goodvoice://state";

/// The event the UI listens on for mute and deafen.
///
/// Both can be changed from the tray now (task 4.2), so the window cannot be
/// the source of truth for either — it is told, the same as the tray is.
const CONTROLS_EVENT: &str = "goodvoice://controls";

/// The event the UI listens on for how loud everyone is, this client included.
///
/// Separate from the roster because the two move at completely different
/// rates: a roster changes when somebody joins or leaves, this changes with
/// every sentence. Sending them together would mean re-rendering the whole
/// roster twenty times a second.
const SPEAKING_EVENT: &str = "goodvoice://speaking";

/// The event a `goodvoice://` link produces when the client cannot simply act
/// on it: a room it is being asked to join while already in another, a link
/// for a server this client is not pointed at, or a join that failed.
///
/// A link that *can* be acted on, and does, produces no event of its own — it
/// joins, and the join tells the window through [`CALL_EVENT`] the same as any
/// other.
///
/// Emitted *and* kept, by [`offer_invite`]: a link is the thing that starts
/// this process, so this event is routinely sent before a webview exists to
/// hear it.
const INVITE_EVENT: &str = "goodvoice://invite";

/// The event that says what this client's own screen share is doing.
///
/// Separate from the roster, which says who is sharing: this one carries why a
/// share failed, and what it went live as — including whether the encoder is
/// in silicon, which prd.md §3 F3 requires be shown.
const SHARE_EVENT: &str = "goodvoice://share";

/// A link the window has to deal with, because the client would not.
#[derive(Debug, Clone, Serialize)]
pub struct InviteOffer {
    /// The room the link names.
    pub room: String,
    /// Why it was not simply joined, in the words the window will show.
    pub reason: String,
    /// Whether the window may offer to act on it. False for a link belonging
    /// to another deploy: acting on that would mean changing servers, which a
    /// link is never allowed to do (`invite`).
    pub joinable: bool,
}

/// Identity of the running client, surfaced to the UI on boot.
#[derive(Debug, Clone, Serialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
    /// Where this client joins rooms: what was chosen in the settings, or
    /// what the build shipped with.
    pub server: String,
    /// What the build shipped with, so the window can offer to go back to it
    /// and can say when it is already there.
    #[serde(rename = "defaultServer")]
    pub default_server: String,
    /// Whether [`Self::server`] was chosen by a person (prd.md §9) rather than
    /// inherited from the build.
    #[serde(rename = "serverChosen")]
    pub server_chosen: bool,
}

impl ClientInfo {
    #[must_use]
    pub fn of(home: &Home) -> Self {
        Self {
            name: env!("CARGO_PKG_NAME").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            server: home.server(),
            default_server: home.fallback().to_owned(),
            server_chosen: home.is_chosen(),
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

/// Everything a window that has just been built needs in order to catch up.
///
/// The other payloads are *changes*: `push_roster`, `push_state` and
/// `push_controls` emit when something moves and say nothing in between. That
/// was enough while a window was created once and lived as long as the process
/// — and it stopped being enough in task 4.6, which drops the webview while
/// goodvoice is in the tray and builds a new one when it comes back. A window
/// can now arrive in the middle of a call, and a window that arrives in the
/// middle of a call and is told nothing shows an empty room.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// The call, if there is one. `None` is a client sitting in no room, which
    /// is exactly what the window shows before anybody joins.
    pub call: Option<CallStatus>,
    pub controls: Controls,
    pub health: Option<CallHealth>,
    pub speaking: Levels,
    /// The audio settings. Not call state at all — they outlive any one call
    /// and are set before the first one — but a window that has just been
    /// built has to draw the switches somehow, and this is the one call it
    /// already makes.
    pub audio: AudioSettings,
    /// What this client's own screen share is doing. [`ShareState::Idle`]
    /// outside a call, which is also what it is inside one until somebody
    /// picks something.
    pub share: ShareState,
    /// A link this client would not act on, still waiting to be answered.
    ///
    /// Here for the same reason the call is: [`INVITE_EVENT`] carries the
    /// moment, and a window that did not exist at that moment never hears it.
    /// A link is the one thing that *starts* this app, so the offer is
    /// routinely made before there is a webview listening — see
    /// [`offer_invite`].
    pub invite: Option<InviteOffer>,
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
    /// The audio settings, held by the app rather than by a call.
    ///
    /// They have to outlive calls: the capture path reads them, the window
    /// sets them before anyone has joined anything, and a slider moved in one
    /// call is still where it was left in the next. The same `Arc` goes into
    /// `hardware::open` and into `CallOptions`.
    prefs: Arc<AudioPrefs>,
    /// Watched rather than read: [`push_controls`] forwards every change to
    /// the window and the tray, so whoever made it does not have to know who
    /// else is showing it.
    controls: watch::Sender<Controls>,
    /// Which opening of the viewer window is being fed, as
    /// [`Call::watch_screen`] named it.
    ///
    /// Plain and atomic rather than behind [`Self::call`]'s lock because the
    /// window event that reads it ([`viewer_closed`]) is synchronous and
    /// cannot wait for an async lock.
    viewer: AtomicU64,
    /// The last link this client would not act on, until a window answers it.
    ///
    /// Not a `watch` like [`Self::controls`]: nothing polls this, and it is
    /// read exactly once per window, by [`current_status`]. Cleared by
    /// [`dismiss_invite`] when the window has taken it or put it away, so that
    /// the webview being rebuilt out of the tray (task 4.6) does not bring a
    /// banner back from an hour ago.
    invite: StdMutex<Option<InviteOffer>>,
}

impl Default for CurrentCall {
    fn default() -> Self {
        Self {
            call: Mutex::default(),
            hotkey: Hotkey::default(),
            prefs: Arc::new(AudioPrefs::default()),
            controls: watch::Sender::new(Controls::default()),
            viewer: AtomicU64::new(0),
            invite: StdMutex::default(),
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
    server: String,
    room: String,
    name: String,
    mode: TransmitMode,
) -> Result<CallStatus, String> {
    let prefs = Arc::clone(&app.state::<CurrentCall>().prefs);
    join_call(
        &app,
        CallOptions {
            base: server,
            room,
            name,
            mode,
            prefs,
        },
    )
    .await
}

/// Opens the devices, joins the room, and wires everything that watches a call.
///
/// Behind both ways in: the window's button, and the room named in
/// [`AUTOJOIN_ENV`].
async fn join_call(app: &AppHandle, options: CallOptions) -> Result<CallStatus, String> {
    let state = app.state::<CurrentCall>();
    let mut current = state.call.lock().await;
    if current.is_some() {
        return Err("already in a call".to_owned());
    }

    let (microphone, speakers) =
        hardware::open(Arc::clone(&options.prefs)).map_err(|error| error.to_string())?;

    let call = Call::join(
        options,
        Box::new(microphone),
        Arc::new(speakers) as Arc<dyn AudioSink>,
    )
    .await
    .map_err(|error| error.to_string())?;

    // From the call rather than from `options`: the call is what `Snapshot`
    // will be asked about later, so both answers come from the same place.
    let status = CallStatus {
        self_id: call.self_id(),
        room: call.room().to_owned(),
        participants: call.roster().borrow().clone(),
    };

    // For any window that did not ask for this call: autojoin, and whatever
    // else joins without one (see `CALL_EVENT`). A window that *did* ask has
    // already set itself from the returned status and sets it again to the
    // same thing.
    let _ = app.emit(CALL_EVENT, status.clone());

    // Watch receivers, not the call: these outlive `leave_room` taking the
    // call apart, and they end on their own when its state is dropped.
    tauri::async_runtime::spawn(push_roster(app.clone(), call.roster()));
    tauri::async_runtime::spawn(push_speaking(app.clone(), call.speaking()));
    tauri::async_runtime::spawn(push_state(app.clone(), call.state(), call.self_id_watch()));
    tauri::async_runtime::spawn(push_share(app.clone(), call.share()));
    *current = Some(call);
    // A fresh call is unmuted and undeafened, whatever the last one ended as.
    state.controls.send_replace(Controls {
        in_call: true,
        muted: false,
        deafened: false,
    });
    drop(current);
    refresh_hotkey(app).await;

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

/// Sets the audio settings: sensitivity, noise suppression, echo cancellation.
///
/// Takes effect on the next frame — 20 ms — whether or not there is a call.
/// The capture path reads them straight from the shared prefs, and switching a
/// WebRTC stage on or off happens on the generation change rather than per
/// frame (see `audio::prefs`).
///
/// Returns what was actually stored, which is not always what was sent: a
/// threshold outside the slider's range is pulled back onto it, and the window
/// should draw where the value landed rather than where it aimed.
///
/// # Errors
///
/// Never. The `Result` is there so the UI can await it like the others.
#[tauri::command]
async fn set_audio_settings(
    state: State<'_, CurrentCall>,
    settings: AudioSettings,
) -> Result<AudioSettings, String> {
    Ok(state.prefs.set(settings))
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

/// The viewer window's label, and the one thing that says a window is the
/// viewer rather than the app.
///
/// `tray::window_event` reads it: the main window destroys itself into the
/// tray when it is minimised (task 4.6), and a viewer that did the same would
/// close a live share every time somebody put it out of the way.
pub const VIEWER_LABEL: &str = "screen";

/// Opens the viewer window, or brings it back to the front.
///
/// Building the window is all this does. The subscription belongs to the
/// window itself — it asks for [`watch_screen`] when it mounts and gives it up
/// when it unmounts — because that is what makes "viewers opt in" (prd.md §3
/// F3) true of a window that was destroyed rather than merely hidden.
///
/// # Errors
///
/// Returns an error when the window cannot be built.
#[tauri::command]
async fn open_screen_viewer(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(VIEWER_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        return window.set_focus().map_err(|error| error.to_string());
    }

    tauri::WebviewWindowBuilder::new(
        &app,
        VIEWER_LABEL,
        // The same bundle as the main window, told which half to render. A
        // fragment rather than a query so the dev server and the embedded
        // protocol resolve it identically.
        tauri::WebviewUrl::App("index.html#screen".into()),
    )
    .title("goodvoice — screen")
    // 16:9 at a size that fits on a laptop, and resizable from there. Not
    // aspect-locked: the picture is letterboxed inside whatever shape the
    // window is, which is the only way to be aspect-correct for a source that
    // can change shape mid-share.
    .inner_size(960.0, 540.0)
    .min_inner_size(320.0, 180.0)
    .resizable(true)
    .build()
    .map(|_| ())
    .map_err(|error| error.to_string())
}

/// Starts feeding this window the room's screen.
///
/// The channel carries raw bytes rather than JSON: an access unit is tens of
/// kilobytes and a keyframe is hundreds, and a `Vec<u8>` serialised as a JSON
/// array of numbers is four times the size and an allocation per byte.
///
/// **The first byte is the keyframe flag**, 1 or 0, and the rest is Annex B.
/// A decoder has to be told which chunks it can start on and RTP does not
/// carry that (`rtc::screen::starts_with_idr`), so it is prefixed here rather
/// than sent as a second message that could arrive out of order.
///
/// # Errors
///
/// Returns an error when there is no call to watch.
#[tauri::command]
async fn watch_screen(
    state: State<'_, CurrentCall>,
    frames: tauri::ipc::Channel<tauri::ipc::Response>,
) -> Result<(), String> {
    let call = state.call.lock().await;
    let call = call.as_ref().ok_or_else(|| "not in a call".to_owned())?;
    let generation = call.watch_screen(Arc::new(ChannelSink { frames }));
    state.viewer.store(generation, atomic::Ordering::Relaxed);
    Ok(())
}

/// Gives the subscription up when the viewer window goes away.
///
/// **There is no `stop_watching_screen` command**, and there was one until
/// this was measured: a window is *destroyed*, and a webview taken down with
/// its window never runs the cleanup that would have sent it. Every viewer
/// after the first one then opened onto a subscription whose frames were still
/// addressed to the window that closed, and sat on "nobody is sharing" while a
/// share was live (DR-33).
///
/// So the window's own end is the unsubscribe, which is also what makes
/// "viewers opt in" (prd.md §3 F3) a property of the window's lifetime rather
/// than of a message the window has to remember to send.
///
/// The generation is read here, synchronously, and honoured later: by the time
/// the spawned task runs, a person may already have opened the next viewer,
/// and taking *its* sink away would leave a window nothing could recover.
fn viewer_closed(app: &AppHandle) {
    let generation = app
        .state::<CurrentCall>()
        .viewer
        .load(atomic::Ordering::Relaxed);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let call = app.state::<CurrentCall>();
        let call = call.call.lock().await;
        if let Some(call) = call.as_ref() {
            call.unwatch_screen(generation);
        }
    });
}

/// A [`ScreenSink`] that forwards to one window.
struct ChannelSink {
    frames: tauri::ipc::Channel<tauri::ipc::Response>,
}

impl rtc::screen::ScreenSink for ChannelSink {
    fn accept(&self, unit: &[u8], keyframe: bool) {
        let mut payload = Vec::with_capacity(unit.len() + 1);
        payload.push(u8::from(keyframe));
        payload.extend_from_slice(unit);
        // A window that has gone away is the ordinary end of a viewer, and
        // the call gives the subscription up the moment the window is
        // destroyed (`viewer_closed`). Nothing to report and nothing to do.
        let _ = self.frames.send(tauri::ipc::Response::new(payload));
    }

    fn ended(&self) {
        // Zero bytes, which cannot be confused with a frame: every real
        // payload carries at least the flag byte. The window draws its
        // "nobody is sharing" state from it.
        let _ = self.frames.send(tauri::ipc::Response::new(Vec::new()));
    }
}

/// Everything this machine can share, monitors first.
///
/// Called when the picker opens rather than kept up to date: windows come and
/// go, and a list that is a second old is a list that is right often enough —
/// a target that has closed by the time it is picked fails at
/// [`start_share`], which is where it would have to be handled anyway.
///
/// # Errors
///
/// Returns an error when the screen cannot be enumerated at all, which on a
/// non-Windows host is always.
#[tauri::command]
fn share_targets() -> Result<Vec<capture::wgc::Target>, String> {
    #[cfg(windows)]
    {
        if !capture::wgc::is_supported() {
            return Err("screen capture is not available on this machine".to_owned());
        }
        let mut targets = capture::wgc::monitors().map_err(|error| error.to_string())?;
        targets.extend(capture::wgc::windows().map_err(|error| error.to_string())?);
        Ok(targets)
    }
    #[cfg(not(windows))]
    {
        Err("screen capture is Windows-only".to_owned())
    }
}

/// Start sharing `target` at `quality`.
///
/// Returns as soon as the intent is recorded. Whether it worked — and the
/// refusal when somebody else is already sharing — arrives on [`SHARE_EVENT`].
///
/// # Errors
///
/// Returns an error when there is no call to share into.
#[tauri::command]
async fn start_share(
    state: State<'_, CurrentCall>,
    target: capture::wgc::Target,
    quality: capture::encoder::Quality,
) -> Result<(), String> {
    let call = state.call.lock().await;
    let call = call.as_ref().ok_or_else(|| "not in a call".to_owned())?;
    #[cfg(windows)]
    {
        call.start_share(Arc::new(capture::share::ShareFactory::new(target, quality)));
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (call, target, quality);
        Err("screen capture is Windows-only".to_owned())
    }
}

/// Stop sharing. Does nothing if this client is not sharing.
///
/// # Errors
///
/// Returns an error when there is no call.
#[tauri::command]
async fn stop_share(state: State<'_, CurrentCall>) -> Result<(), String> {
    let call = state.call.lock().await;
    let call = call.as_ref().ok_or_else(|| "not in a call".to_owned())?;
    call.stop_share();
    Ok(())
}

/// # Errors
///
/// Never. The `Result` is what an async command owes its caller, and async is
/// what lets this take the state by value like every other command here.
#[tauri::command]
async fn client_info(home: State<'_, Home>) -> Result<ClientInfo, String> {
    Ok(ClientInfo::of(&home))
}

/// Points this client at a Worker of somebody's own (prd.md §9).
///
/// An empty string means "back to the one this build shipped with", which is
/// why this is not two commands: a window offering both needs one place where
/// what it sent becomes what the client will use.
///
/// Takes effect on the next join. A call in progress belongs to the server it
/// was made on, and moving it would mean leaving a room the other people are
/// still in.
///
/// # Errors
///
/// A sentence for the person who typed it, when what they typed is not an
/// origin — see [`home::normalise`].
#[tauri::command]
async fn set_server(home: State<'_, Home>, url: String) -> Result<ClientInfo, String> {
    home.choose(&url)?;
    Ok(ClientInfo::of(&home))
}

/// The link that brings somebody else into this room.
///
/// Built here rather than in the window because the server half of it is the
/// client's to know (DR-36), and a window that guessed would hand out invites
/// to the wrong deploy.
///
/// # Errors
///
/// Returns an error when there is no call to invite anybody to.
#[tauri::command]
async fn invite_link(
    home: State<'_, Home>,
    state: State<'_, CurrentCall>,
) -> Result<String, String> {
    let call = state.call.lock().await;
    let call = call.as_ref().ok_or_else(|| "not in a call".to_owned())?;
    Ok(invite::format(call.room(), &home.server()))
}

/// The call as it stands right now, for a window that has just been built.
///
/// Asked once, on mount. Everything after that arrives as events — see
/// [`Snapshot`] for why a window needs both.
///
/// # Errors
///
/// Never. The `Result` is there so the UI can await it like the others.
#[tauri::command]
async fn current_status(state: State<'_, CurrentCall>) -> Result<Snapshot, String> {
    let controls = *state.controls.borrow();
    // Read rather than taken: the window says when it has dealt with the offer
    // (`dismiss_invite`), and a window that asked and then died before drawing
    // it would otherwise have swallowed somebody's invite on the way out.
    let invite = state.invite.lock().ok().and_then(|pending| pending.clone());
    let call = state.call.lock().await;
    let Some(call) = call.as_ref() else {
        return Ok(Snapshot {
            call: None,
            controls,
            health: None,
            speaking: Levels::default(),
            audio: state.prefs.settings(),
            share: ShareState::Idle,
            invite,
        });
    };

    let self_id = call.self_id();
    Ok(Snapshot {
        call: Some(CallStatus {
            self_id: self_id.clone(),
            room: call.room().to_owned(),
            participants: call.roster().borrow().clone(),
        }),
        controls,
        health: Some(CallHealth {
            state: call.state().borrow().clone(),
            self_id,
        }),
        speaking: call.speaking().borrow().clone(),
        audio: state.prefs.settings(),
        share: call.share().borrow().clone(),
        invite,
    })
}

/// Forgets the link the window has just answered.
///
/// Called for both answers, because both are answers: taking the offer joins
/// the room, and dismissing it says no. What must not happen is the offer
/// outliving either — the webview is thrown away and rebuilt every time
/// goodvoice goes to the tray and comes back (task 4.6), and a kept offer
/// would greet somebody with an invite from an hour ago every time.
///
/// # Errors
///
/// Never. The `Result` is there so the UI can await it like the others.
#[tauri::command]
async fn dismiss_invite(state: State<'_, CurrentCall>) -> Result<(), String> {
    if let Ok(mut pending) = state.invite.lock() {
        *pending = None;
    }
    Ok(())
}

/// Joins the room named in [`AUTOJOIN_ENV`], if there is one.
///
/// Deliberately not waiting for the webview: the microphone, the transport and
/// the room have nothing to do with a window, and a cold start that waited for
/// one would be measuring the wrong thing (task 4.4). The window catches up on
/// its own — the roster and health events are already flowing by the time it
/// subscribes.
async fn autojoin(app: AppHandle, room: String, since_start: Instant) {
    println!("autojoin starting at {} ms", millis(since_start));
    let options = CallOptions {
        // The chosen server, not the built-in one: a self-hoster's client is
        // pointed at their Worker, and a join that ignored that would reach a
        // room nobody else is in (task 6.1).
        base: app.state::<Home>().server(),
        room,
        name: "coldstart".to_owned(),
        // Open, not push-to-talk: nobody is holding a key in a scripted run.
        mode: TransmitMode::Open,
        prefs: Arc::clone(&app.state::<CurrentCall>().prefs),
    };

    match join_call(&app, options).await {
        Ok(status) => println!(
            "autojoined {} as {} at {} ms",
            status.room,
            status.self_id,
            millis(since_start)
        ),
        Err(error) => eprintln!("autojoin failed: {error}"),
    }
}

/// Acts on a `goodvoice://join/<room>` link, or tells the window why not.
///
/// Called for every link this client is handed: at startup when one launched
/// it, and while it is running when Windows hands one to a second instance
/// that `single_instance` then folds into this one.
///
/// Four answers, and the window hears about every one that is not a plain
/// join:
///
/// - **not in a call** — join, exactly as if the room had been typed. The
///   window learns from [`CALL_EVENT`], which is how it learns about any join
///   it did not ask for.
/// - **already in one** — offer it. Dropping a call somebody is in because a
///   link arrived is not something to do on their behalf.
/// - **another deploy** — refuse it, with the address. A link must never move
///   a client to a server of the sender's choosing (`invite`).
/// - **the join failed** — say so, with what failed as the reason. A link is
///   the one way into a room with no form behind it: a microphone another
///   application is holding used to leave the window sitting on the join form
///   with nothing said, which reads as a link that did nothing at all. It is
///   offered back as joinable, because the usual cause is somebody else's
///   program and the usual fix is to try again.
async fn open_invite(app: AppHandle, url: String) {
    let asked = match invite::parse(&url) {
        Ok(asked) => asked,
        Err(reason) => {
            eprintln!("ignoring {url}: {reason}");
            return;
        }
    };

    let ours = app.state::<Home>().server();
    if let Some(theirs) = asked.server.as_deref() {
        if theirs != ours {
            offer_invite(
                &app,
                InviteOffer {
                    room: asked.room.clone(),
                    reason: format!("that invite is for {theirs}, and this client is on {ours}"),
                    joinable: false,
                },
            );
            return;
        }
    }

    if app.state::<CurrentCall>().call.lock().await.is_some() {
        offer_invite(
            &app,
            InviteOffer {
                room: asked.room.clone(),
                reason: "you are already in a call".to_owned(),
                joinable: true,
            },
        );
        return;
    }

    let options = CallOptions {
        base: ours,
        room: asked.room.clone(),
        // The window has a name in it and this path has no window to ask —
        // the same position autojoin is in, and the same answer.
        name: "anon".to_owned(),
        mode: TransmitMode::Open,
        prefs: Arc::clone(&app.state::<CurrentCall>().prefs),
    };
    if let Err(error) = join_call(&app, options).await {
        eprintln!("could not join from a link: {error}");
        offer_invite(
            &app,
            InviteOffer {
                room: asked.room,
                reason: error,
                joinable: true,
            },
        );
    }
}

/// Hands an offer to the window, whether or not there is a window yet.
///
/// Both halves matter, and the second one is the one that was missing. A
/// `goodvoice://` link is the only thing in this client that runs *before* a
/// person is looking: Windows starts the process for it, and [`open_invite`]
/// has usually finished — refused the link, or tried to join and failed on a
/// microphone somebody else's program is holding — while the webview is still
/// being built. An event emitted then reaches nobody, and the window that
/// arrives a second later shows a plain join form, which is indistinguishable
/// from a link that did nothing at all. That is what made the drill flaky
/// rather than the drill: it is the same race that [`Snapshot`] exists for.
///
/// So the offer is also *kept*, and [`current_status`] hands it to whichever
/// window asks first.
fn offer_invite(app: &AppHandle, offer: InviteOffer) {
    if let Ok(mut pending) = app.state::<CurrentCall>().invite.lock() {
        *pending = Some(offer.clone());
    }
    let _ = app.emit(INVITE_EVENT, offer);
}

/// Whole milliseconds since the app started running, for the startup marks.
fn millis(since_start: Instant) -> u128 {
    since_start.elapsed().as_millis()
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
async fn push_speaking(app: AppHandle, mut speaking: watch::Receiver<Levels>) {
    loop {
        let talking = speaking.borrow_and_update().clone();
        let _ = app.emit(SPEAKING_EVENT, talking);

        if speaking.changed().await.is_err() {
            return;
        }
    }
}

/// Forwards this client's own share state to the webview until the call ends.
///
/// One event rather than a poll, and separate from the roster: the roster says
/// who is sharing, this says what happened when *this* client tried — including
/// the refusal when somebody else got there first (prd.md §8).
async fn push_share(app: AppHandle, mut share: watch::Receiver<ShareState>) {
    loop {
        let current = share.borrow_and_update().clone();
        let _ = app.emit(SHARE_EVENT, current);

        if share.changed().await.is_err() {
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
    // Taken before anything else this process does, so the marks the autojoin
    // prints are measured from the earliest point Rust code owns (task 4.4).
    let since_start = Instant::now();

    tauri::Builder::default()
        // First, and before anything that could take time: Windows starts a
        // *new process* for a `goodvoice://` link, and without this the second
        // one would be a second client — its own tray icon, its own seat in
        // the room, and the microphone opened twice. The `deep-link` feature
        // is what turns that second process's command line into an
        // `on_open_url` on this one instead of dropping it.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // A person who clicked a link expects to see the app, and it may
            // have been in the tray for hours (task 4.6).
            tray::show(app);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .setup(move |app| {
            app.manage(CurrentCall::default());
            app.manage(tray::Tray::default());
            // Before anything that joins, autojoin included: what server this
            // client is pointed at is the first thing a join needs and the one
            // thing a self-hoster changes (task 6.1).
            let config = app.path().app_config_dir().ok();
            app.manage(Home::open(config.as_deref(), DEFAULT_SERVER));

            // A host that will not give us a tray is not a reason to refuse to
            // run — it is a reason to keep a window that closes normally, which
            // is what `Tray` staying uninstalled means (task 4.1).
            if let Err(error) = tray::install(app.handle()) {
                eprintln!("no tray icon: {error}; the window will close rather than hide");
            }

            let controls = app.state::<CurrentCall>().controls.subscribe();
            tauri::async_runtime::spawn(push_controls(app.handle().clone(), controls));

            // Every link this client is handed, whether it launched the app or
            // arrived while it was running (task 6.2).
            {
                use tauri_plugin_deep_link::DeepLinkExt as _;
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        tauri::async_runtime::spawn(open_invite(handle.clone(), url.to_string()));
                    }
                });
                // Only a bundled build has the scheme registered by its
                // installer; a `cargo run` has to ask for it, and asking twice
                // is harmless. Without this, testing a link means building an
                // installer first.
                #[cfg(debug_assertions)]
                if let Err(error) = app.deep_link().register_all() {
                    eprintln!("could not register {}: {error}", invite::SCHEME);
                }

                // **The link that started the app is not handled by anything
                // else.** `single_instance` forwards a *second* process's
                // arguments into the running one, which covers a link clicked
                // while goodvoice is open; the first process has to read its
                // own command line, and nothing in the plugin does that on its
                // own. Without this line the app opens on the join form and
                // the room in the link is simply lost — which is exactly what
                // it did, measured, before the line existed.
                //
                // After the handler above, because this is what feeds it.
                app.deep_link().handle_cli_arguments(std::env::args());
            }

            if let Ok(room) = std::env::var(AUTOJOIN_ENV) {
                // How much of a cold start is gone before any of this runs is
                // the first thing to know about it: `setup` happens after the
                // window and its webview exist.
                println!("setup at {} ms", millis(since_start));
                tauri::async_runtime::spawn(autojoin(app.handle().clone(), room, since_start));
            }
            Ok(())
        })
        .on_window_event(tray::window_event)
        .invoke_handler(tauri::generate_handler![
            client_info,
            set_server,
            invite_link,
            current_status,
            dismiss_invite,
            join_room,
            leave_room,
            set_muted,
            set_deafened,
            set_transmit_mode,
            set_audio_settings,
            set_talk_key,
            set_talk_binding,
            open_screen_viewer,
            share_targets,
            start_share,
            stop_share,
            watch_screen,
            talk_key_is_global
        ])
        .build(tauri::generate_context!())
        .expect("error while running goodvoice")
        .run(|app, event| {
            // The last window closing is not the app closing. Task 4.1 made
            // that true by hiding the window; 4.6 makes it true by destroying
            // the webview and rebuilding it on the way back, and a destroyed
            // window is indistinguishable from a quit unless this says
            // otherwise.
            //
            // `code: None` is the difference between the two: a quit from the
            // tray goes through `app.exit(0)` and arrives here carrying a code,
            // and that one is meant. And only while there is a tray to come
            // back from — an app with no icon and no window is a process
            // nobody can reach.
            if let tauri::RunEvent::ExitRequested {
                code: None, api, ..
            } = event
            {
                if app.state::<tray::Tray>().hides_the_window() {
                    api.prevent_exit();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{
        AudioSettings, CallHealth, CallState, CallStatus, ClientInfo, Controls, InviteOffer,
        Levels, ShareState, Snapshot, DEFAULT_SERVER,
    };
    use crate::rtc::session::Talker;

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

    /// The window reads these names out of `current_status` (`Snapshot` in
    /// App.tsx), and it reads them exactly once — on mount, to find out what it
    /// missed while it did not exist (task 4.6). A field renamed here without
    /// being renamed there is a window that comes back from the tray showing an
    /// empty room while a call is running.
    #[test]
    fn the_snapshot_a_rebuilt_window_catches_up_from() {
        let payload = serde_json::to_value(Snapshot {
            call: None,
            controls: Controls {
                in_call: false,
                muted: true,
                deafened: false,
            },
            health: None,
            speaking: Levels::default(),
            audio: AudioSettings::default(),
            share: ShareState::Idle,
            invite: None,
        })
        .expect("snapshot serialize");

        assert_eq!(
            payload,
            serde_json::json!({
                "call": null,
                "controls": { "in_call": false, "muted": true, "deafened": false },
                "health": null,
                "speaking": { "talking": [], "input": 0.0 },
                "audio": {
                    "automaticSensitivity": true,
                    "threshold": AudioSettings::default().threshold,
                    "noiseSuppression": true,
                    "echoCancellation": true,
                },
                "share": { "state": "idle" },
                "invite": null,
            })
        );
    }

    /// A snapshot with a call in it, in the shape App.tsx destructures: the
    /// health carries `self_id` alongside the state's own fields, because a
    /// reconnect changes both and a window that learned them separately would
    /// spend a frame unable to find itself in the roster.
    #[test]
    fn a_snapshot_of_a_live_call_carries_the_room_and_the_seat() {
        let payload = serde_json::to_value(Snapshot {
            call: Some(CallStatus {
                self_id: "seat-1".to_owned(),
                room: "friday".to_owned(),
                participants: Vec::new(),
            }),
            controls: Controls {
                in_call: true,
                muted: false,
                deafened: false,
            },
            health: Some(CallHealth {
                state: CallState::Live,
                self_id: "seat-1".to_owned(),
            }),
            speaking: Levels {
                talking: vec![Talker {
                    id: "seat-2".to_owned(),
                    level: 0.5,
                }],
                input: 0.25,
            },
            audio: AudioSettings::default(),
            share: ShareState::Sharing {
                target: "\\\\.\\DISPLAY1".to_owned(),
                width: 1280,
                height: 720,
                hardware: true,
            },
            invite: None,
        })
        .expect("snapshot serialize");

        assert_eq!(payload["call"]["room"], "friday");
        assert_eq!(payload["call"]["self_id"], "seat-1");
        assert_eq!(payload["health"]["state"], "live");
        assert_eq!(payload["health"]["self_id"], "seat-1");
        // A level, not a flag: App.tsx draws the dot from this number and a
        // rename would leave every dot dark in a room full of people talking.
        assert_eq!(payload["speaking"]["talking"][0]["id"], "seat-2");
        assert_eq!(payload["speaking"]["talking"][0]["level"], 0.5);
        assert_eq!(payload["speaking"]["input"], 0.25);
    }

    /// A link that arrived before the window did.
    ///
    /// This is the field the whole of `offer_invite` exists for: a
    /// `goodvoice://` link starts the process, so the client has usually
    /// decided about it — refused it, or tried to join and failed — before
    /// there is a webview subscribed to [`INVITE_EVENT`]. The window reads
    /// these three names out of the snapshot (`InviteOffer` in App.tsx).
    #[test]
    fn a_link_answered_before_the_window_existed_is_still_in_the_snapshot() {
        let payload = serde_json::to_value(Snapshot {
            call: None,
            controls: Controls::default(),
            health: None,
            speaking: Levels::default(),
            audio: AudioSettings::default(),
            share: ShareState::Idle,
            invite: Some(InviteOffer {
                room: "friday".to_owned(),
                reason: "the microphone is in use".to_owned(),
                joinable: true,
            }),
        })
        .expect("snapshot serialize");

        assert_eq!(payload["invite"]["room"], "friday");
        assert_eq!(payload["invite"]["reason"], "the microphone is in use");
        assert_eq!(payload["invite"]["joinable"], true);
    }

    /// The other half of that contract: the settings panel reads these four
    /// names and sends them straight back through `set_audio_settings`.
    #[test]
    fn the_audio_settings_the_panel_binds_to() {
        let payload = serde_json::to_value(AudioSettings {
            automatic_sensitivity: false,
            threshold: 0.08,
            noise_suppression: false,
            echo_cancellation: true,
        })
        .expect("settings serialize");

        assert_eq!(payload["automaticSensitivity"], false);
        // Through `as_f64`: the field is an `f32` and serde widens it, so the
        // JSON number is 0.08 as a double promoted from a narrower float.
        assert!(
            payload["threshold"]
                .as_f64()
                .is_some_and(|threshold| (threshold - 0.08).abs() < 1e-6),
            "threshold serialized as {}",
            payload["threshold"]
        );
        assert_eq!(payload["noiseSuppression"], false);
        assert_eq!(payload["echoCancellation"], true);

        // And back, because the window sends the same shape it was given.
        let round_trip: AudioSettings =
            serde_json::from_value(payload).expect("settings deserialize");
        assert!((round_trip.threshold - 0.08).abs() < f32::EPSILON);
        assert!(!round_trip.automatic_sensitivity);
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
        let info = ClientInfo::of(&crate::Home::open(None, DEFAULT_SERVER));
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
