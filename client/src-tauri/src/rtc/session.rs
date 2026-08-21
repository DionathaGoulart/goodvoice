//! One call: join a room, publish the microphone, play everyone else.
//!
//! A call outlives the connection it runs on. [`Call`] is a supervisor: it owns
//! the microphone and the room's state for as long as the user is in the room,
//! and underneath it sessions come and go. A session is one seat in the room —
//! one participant id, one Realtime session, one peer connection — and when it
//! dies the supervisor takes another one and republishes onto it (task 3.5,
//! [`super::reconnect`]).
//!
//! Audio reaches this module only through [`crate::audio::device`], so the same
//! code drives real hardware, a synthetic tone, or nothing at all.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use bytes::Bytes;
use rtc::{
    media::Sample,
    peer_connection::{configuration::media_engine::MIME_TYPE_OPUS, transport::RTCDtlsRole},
    rtp_transceiver::{
        rtp_sender::{
            RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
            RtpCodecKind,
        },
        PayloadType,
    },
};
use serde_json::{json, Value};
use tokio::{
    sync::{mpsc, watch, Notify},
    task::JoinHandle,
};
use webrtc::{
    media_stream::{
        track_local::{static_sample::TrackLocalStaticSample, TrackLocal},
        track_remote::{TrackRemote, TrackRemoteEvent},
        MediaStreamTrack, Track as _,
    },
    peer_connection::{
        register_default_interceptors, MediaEngine, PeerConnection, PeerConnectionBuilder,
        PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer,
        RTCPeerConnectionState, RTCSessionDescription, Registry, SettingEngine,
    },
    rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit},
    runtime::default_runtime,
};

use super::{
    reconnect::{Backoff, CallState, EndReason},
    signaling::{
        ClientMessage, IceServer, JoinResponse, Participant, ServerMessage, SfuOperation, Signaling,
    },
    RtcError,
};
use crate::audio::{
    device::{AudioSink, AudioSource, MAX_REMOTE_SLOTS},
    mixer::{peak, Meter, MAX_GAIN, SPEAKING_LEVEL},
    opus::{
        silent_frame, Frame, VoiceDecoder, VoiceEncoder, FRAME_MS, MAX_PACKET_BYTES, SAMPLE_RATE_HZ,
    },
};

/// The track name a goodvoice client publishes its microphone under. A closed
/// vocabulary server-side (`TRACK_KINDS`), so this string is load-bearing.
const MIC_TRACK: &str = "mic";

/// Opus' de-facto payload type, and the one Cloudflare answers with. Offering
/// the same number keeps the two sides from renumbering mid-negotiation
/// (DR-7).
const OPUS_PAYLOAD_TYPE: PayloadType = 111;

/// How long ICE and DTLS get before the join is called a failure.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(25);

/// How long a freshly pulled track gets to actually show up.
const TRACK_TIMEOUT: Duration = Duration::from_secs(15);

/// How many times to ask for a track whose publisher is still starting up, and
/// how long to wait before the first retry. The delay doubles each time.
const SUBSCRIBE_ATTEMPTS: usize = 4;
const SUBSCRIBE_BACKOFF: Duration = Duration::from_millis(500);

/// How often the subscribe loop re-reconciles against the roster it already
/// has, so a peer whose subscription failed is picked up rather than left
/// silent until the roster happens to change again.
const RESUBSCRIBE_INTERVAL: Duration = Duration::from_secs(2);

/// How often the speaking indicator is recomputed. Fast enough that the roster
/// lights up with the first syllable rather than after it, slow enough that a
/// room full of listeners is doing nothing ten times a second.
const METER_INTERVAL: Duration = Duration::from_millis(100);

/// How many times to build a peer connection before giving up on the room, and
/// the unit of backoff between tries (multiplied by the attempt number).
const JOIN_ATTEMPTS: u32 = 3;
const JOIN_BACKOFF: Duration = Duration::from_millis(600);

/// How long ICE may sit in `Disconnected` before the session is written off.
///
/// `Disconnected` is recoverable in principle — but not against an ice-lite SFU
/// that offers one candidate and runs no checks of its own (DR-7), where there
/// is nothing left to re-pair with. Three seconds is long enough that a blip
/// does not cost a rejoin and short enough that the user is not talking to
/// nobody for the ~30 s webrtc-rs takes to declare `Failed`.
const DISCONNECT_GRACE: Duration = Duration::from_secs(3);

/// Consecutive failed microphone frames before the session is declared lost.
/// 50 frames is one second — long enough to ride out a renegotiation, short
/// enough that a client which has genuinely lost its sender reconnects rather
/// than talking into a closed socket.
const PUBLISH_FAILURE_LIMIT: usize = 50;

/// Where to call, and as whom.
#[derive(Debug, Clone)]
pub struct CallOptions {
    /// Origin of the goodvoice Worker, e.g. `https://…workers.dev`.
    pub base: String,
    /// Room code. Alphanumeric and hyphens, 4–24 characters.
    pub room: String,
    /// Display name, as the rest of the room will see it.
    pub name: String,
}

/// One connected seat in the room, before any of the call's tasks are running.
struct Session {
    self_id: String,
    participants: Vec<Participant>,
    peer: Arc<dyn PeerConnection>,
    signals: Signals,
    published: Published,
    commands: mpsc::Sender<ClientMessage>,
    inbound: mpsc::Receiver<ServerMessage>,
}

/// One try at taking a seat and getting media flowing on it.
///
/// The room WebSocket is opened before the peer connection rather than after,
/// so a failed attempt can hand its seat straight back. Without that, every
/// retry would leave a phantom participant occupying one of the room's eight
/// slots until the heartbeat sweep noticed (DR-5).
async fn connect_once(
    signaling: &Arc<Signaling>,
    options: &CallOptions,
) -> Result<Session, RtcError> {
    let joined = signaling.join(&options.name).await?;
    let (commands, inbound) = signaling.connect(&joined.self_id).await?;

    match establish(signaling, &joined).await {
        Ok((peer, signals, published)) => Ok(Session {
            self_id: joined.self_id,
            participants: joined.participants,
            peer,
            signals,
            published,
            commands,
            inbound,
        }),
        Err(error) => {
            let _ = commands.send(ClientMessage::Leave).await;
            Err(error)
        }
    }
}

/// Builds the transport and gets the microphone onto it.
async fn establish(
    signaling: &Signaling,
    joined: &JoinResponse,
) -> Result<(Arc<dyn PeerConnection>, Signals, Published), RtcError> {
    let (events, signals) = Events::new();
    let peer: Arc<dyn PeerConnection> = Arc::new(open_peer(&joined.sfu.ice_servers, events).await?);

    let published = publish_mic(signaling, &joined.self_id, peer.as_ref(), &signals).await?;
    wait_for_connection(&signals).await?;

    Ok((peer, signals, published))
}

// --- what survives a reconnect ---------------------------------------------

/// Everything a call keeps across the sessions it runs on.
///
/// A session is disposable — new participant id, new Realtime session, new peer
/// connection. What the user thinks of as "the call" is this: the room they are
/// in, whether they are muted, and what the UI is being told. Held in one place
/// so a rejoin can restore it rather than reset it.
struct Shared {
    muted: AtomicBool,
    deafened: AtomicBool,
    /// Set by [`Call::leave`]. Every loop checks it before treating a closed
    /// socket as a failure — a call the user ended is not a call that dropped.
    leaving: AtomicBool,
    /// The room's command channel for whichever session is current. Replaced on
    /// every rejoin; `None` while there is no session to talk to.
    commands: Mutex<Option<mpsc::Sender<ClientMessage>>>,
    state: watch::Sender<CallState>,
    self_id: watch::Sender<String>,
    roster: watch::Sender<Vec<Participant>>,
    /// Who is talking right now, this client included. Pushed only when the set
    /// changes, so a quiet room costs the UI nothing.
    speaking: watch::Sender<Vec<String>>,
    /// Where remote audio goes, and where its levels come from.
    sink: Arc<dyn AudioSink>,
    /// Which playback slot each participant is being played in. Written by
    /// `reconcile`, read by anything that wants to meter or re-mix one peer by
    /// name rather than by slot.
    slots: Mutex<HashMap<String, usize>>,
    /// How loud this client's own microphone has been. Fed by the encode loop,
    /// which is the only place the outgoing signal exists.
    microphone: Meter,
    /// Rung when the microphone's sender stops working. `notify_one` rather
    /// than `notify_waiters` so a report that lands between two polls of the
    /// session loop is kept rather than dropped.
    lost: Notify,
}

impl Shared {
    fn new(self_id: String, participants: Vec<Participant>, sink: Arc<dyn AudioSink>) -> Arc<Self> {
        Arc::new(Self {
            muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            leaving: AtomicBool::new(false),
            commands: Mutex::new(None),
            state: watch::Sender::new(CallState::Live),
            self_id: watch::Sender::new(self_id),
            roster: watch::Sender::new(participants),
            speaking: watch::Sender::new(Vec::new()),
            sink,
            slots: Mutex::new(HashMap::new()),
            microphone: Meter::new(),
            lost: Notify::new(),
        })
    }

    fn is_leaving(&self) -> bool {
        self.leaving.load(Ordering::Relaxed)
    }

    /// Whether the encode loop should be putting packets on the wire.
    fn is_transmitting(&self) -> bool {
        !self.muted.load(Ordering::Relaxed)
    }

    /// The playback slot a participant is being played in, if any.
    fn slot_of(&self, participant: &str) -> Option<usize> {
        self.slots.lock().ok()?.get(participant).copied()
    }

    /// Rebuilds the set of people who are talking, and pushes it only if it
    /// moved.
    ///
    /// Sorted so two identical sets compare equal: the point of the comparison
    /// is to leave a quiet room pushing nothing at all.
    fn refresh_speaking(&self) {
        let mut talking: Vec<String> = self
            .slots
            .lock()
            .map(|slots| {
                slots
                    .iter()
                    .filter(|(_, &slot)| self.sink.level(slot) >= SPEAKING_LEVEL)
                    .map(|(id, _)| id.clone())
                    .collect()
            })
            .unwrap_or_default();

        // Muted is not talking, whatever the microphone says.
        if self.microphone.is_speaking() && self.is_transmitting() {
            talking.push(self.self_id.borrow().clone());
        }
        talking.sort_unstable();

        if *self.speaking.borrow() != talking {
            self.speaking.send_replace(talking);
        }
    }

    /// Forgets who was talking, for a seat that is over.
    ///
    /// The meter loop lives inside the session, so without this the last set
    /// it pushed would stand for the whole of a reconnect — a roster lit up
    /// with people who are, by then, not being heard at all. The slot map goes
    /// with it: the next seat hands out its own slots, and a name still
    /// pointing at an old one would meter whoever inherited it.
    fn silence(&self) {
        if let Ok(mut slots) = self.slots.lock() {
            slots.clear();
        }
        self.microphone.reset();
        if !self.speaking.borrow().is_empty() {
            self.speaking.send_replace(Vec::new());
        }
    }

    /// Hands a message to whichever session is current. Silently dropped when
    /// there is none: a mute pressed while reconnecting is replayed by
    /// [`Self::adopt`] when the new seat comes up.
    async fn tell_room(&self, message: ClientMessage) {
        let sender = self
            .commands
            .lock()
            .ok()
            .and_then(|commands| commands.clone());
        if let Some(sender) = sender {
            let _ = sender.send(message).await;
        }
    }

    /// Points the call at a freshly connected session.
    async fn adopt(
        &self,
        session: &Session,
        published: &watch::Sender<Option<Arc<dyn PacketSink>>>,
    ) {
        if let Ok(mut commands) = self.commands.lock() {
            *commands = Some(session.commands.clone());
        }
        self.self_id.send_replace(session.self_id.clone());
        self.roster.send_replace(session.participants.clone());
        published.send_replace(Some(
            Arc::new(session.published.clone()) as Arc<dyn PacketSink>
        ));
        self.state.send_replace(CallState::Live);

        // The room has no memory of this client, so anything the user set
        // before the drop has to be said again. Local state is the truth here;
        // the roster everyone else sees is the copy.
        if self.muted.load(Ordering::Relaxed) {
            self.tell_room(ClientMessage::Mute { muted: true }).await;
        }
        if self.deafened.load(Ordering::Relaxed) {
            self.tell_room(ClientMessage::Deafen { deafened: true })
                .await;
        }
    }

    /// Marks the call over and stops talking to a room that is no longer there.
    fn finish(&self, reason: EndReason) {
        if let Ok(mut commands) = self.commands.lock() {
            *commands = None;
        }
        self.state.send_replace(CallState::Ended(reason));
    }

    /// Throws away a failure report left over from the session that just
    /// ended, so it cannot end the next one before it starts.
    ///
    /// `notified()` resolves immediately when a permit is waiting and never
    /// otherwise; racing it against a future that is already ready is what
    /// turns "take the permit if there is one" into a single poll.
    async fn drain_lost(&self) {
        tokio::select! {
            biased;
            () = self.lost.notified() => {}
            () = std::future::ready(()) => {}
        }
    }
}

/// A live call. Dropping it leaves the room.
pub struct Call {
    shared: Arc<Shared>,
    roster: watch::Receiver<Vec<Participant>>,
    state: watch::Receiver<CallState>,
    self_id: watch::Receiver<String>,
    speaking: watch::Receiver<Vec<String>>,
    /// The supervisor, kept apart from the rest so [`Self::leave`] can wait for
    /// it to close the transport before the process moves on.
    supervisor: Option<JoinHandle<()>>,
    tasks: Vec<JoinHandle<()>>,
}

impl Call {
    /// Joins `options.room` and starts sending and receiving audio.
    ///
    /// Returns once the microphone is published and the transport has
    /// connected — a `Call` that exists is a call you are audible on. After
    /// that the call keeps itself alive: a session that drops is rebuilt in the
    /// background and [`Self::state`] reports what is happening.
    ///
    /// # Errors
    ///
    /// [`RtcError::JoinRejected`] if the room refuses, [`RtcError::Sfu`] if
    /// Cloudflare refuses the track, [`RtcError::NotConnected`] if ICE or DTLS
    /// never completes, and [`RtcError::Http`] if the Worker is unreachable.
    pub async fn join(
        options: CallOptions,
        source: Box<dyn AudioSource>,
        sink: Arc<dyn AudioSink>,
    ) -> Result<Self, RtcError> {
        let signaling = Arc::new(Signaling::new(&options.base, &options.room)?);

        // The ICE/DTLS handshake with Realtime does not always complete on the
        // first try — measured at roughly one attempt in six from this network,
        // and present since the 2.3 spike (DR-8). A seat that never connected
        // is useless, so the whole exchange is thrown away and repeated rather
        // than nursed.
        let mut last = RtcError::NotConnected;
        for attempt in 1..=JOIN_ATTEMPTS {
            match connect_once(&signaling, &options).await {
                Ok(session) => return Ok(Self::start(signaling, options, session, source, sink)),
                Err(error) if attempt < JOIN_ATTEMPTS && error.is_worth_retrying() => {
                    eprintln!("join attempt {attempt} failed ({error}); retrying");
                    last = error;
                    tokio::time::sleep(JOIN_BACKOFF * attempt).await;
                }
                Err(error) => return Err(error),
            }
        }
        Err(last)
    }

    /// Turns a connected session into a running call.
    fn start(
        signaling: Arc<Signaling>,
        options: CallOptions,
        session: Session,
        source: Box<dyn AudioSource>,
        sink: Arc<dyn AudioSink>,
    ) -> Self {
        let shared = Shared::new(session.self_id.clone(), session.participants.clone(), sink);
        let (roster, state, self_id, speaking) = (
            shared.roster.subscribe(),
            shared.state.subscribe(),
            shared.self_id.subscribe(),
            shared.speaking.subscribe(),
        );

        // The microphone is opened once and read for the life of the call. What
        // changes underneath it is where the packets go: `None` while there is
        // no session, which is exactly what a client should be putting on the
        // wire while it has no seat in the room.
        let (published, sinks) = watch::channel(None);

        let tasks = vec![tokio::spawn(publish_loop(
            source,
            sinks,
            Arc::clone(&shared),
        ))];
        let supervisor = tokio::spawn(supervise(
            Supervisor {
                signaling,
                options,
                shared: Arc::clone(&shared),
                published,
            },
            session,
        ));

        Self {
            shared,
            roster,
            state,
            self_id,
            speaking,
            supervisor: Some(supervisor),
            tasks,
        }
    }

    /// This client's participant id, as everyone else sees it.
    ///
    /// It changes when a dropped call reconnects: the seat is new, and so is
    /// the identity attached to it.
    #[must_use]
    pub fn self_id(&self) -> String {
        self.self_id.borrow().clone()
    }

    /// This client's participant id, updated on every rejoin.
    #[must_use]
    pub fn self_id_watch(&self) -> watch::Receiver<String> {
        self.self_id.clone()
    }

    /// The room, updated as people come and go.
    #[must_use]
    pub fn roster(&self) -> watch::Receiver<Vec<Participant>> {
        self.roster.clone()
    }

    /// Live, reconnecting, or over — see [`CallState`].
    #[must_use]
    pub fn state(&self) -> watch::Receiver<CallState> {
        self.state.clone()
    }

    /// Who is talking, this client included.
    ///
    /// Pushed only when the set changes, so a room full of people listening
    /// costs nothing at all — which is the point, given the client has to idle
    /// under 2% CPU (prd.md §4).
    #[must_use]
    pub fn speaking(&self) -> watch::Receiver<Vec<String>> {
        self.speaking.clone()
    }

    /// How loud one participant is right now, `0.0`–`1.0`. Unknown ids and
    /// participants with no audio yet read as silence.
    #[must_use]
    pub fn level_of(&self, participant: &str) -> f32 {
        if *self.self_id.borrow() == participant {
            return self.shared.microphone.level();
        }
        self.shared
            .slot_of(participant)
            .map_or(0.0, |slot| self.shared.sink.level(slot))
    }

    /// Turns one participant up or down for this listener only, `0.0`–4.0.
    ///
    /// Per-listener because that is the only kind that needs nobody's
    /// agreement: a speaker who is too quiet for you may be fine for everyone
    /// else, and asking them to change is a conversation, not a feature.
    pub fn set_gain_of(&self, participant: &str, gain: f32) {
        if let Some(slot) = self.shared.slot_of(participant) {
            self.shared.sink.set_gain(slot, gain.clamp(0.0, MAX_GAIN));
        }
    }

    /// Stops sending audio. Packets stop entirely rather than carrying
    /// silence, so a muted client costs the room nothing (prd.md §3 F1).
    pub async fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
        self.shared.tell_room(ClientMessage::Mute { muted }).await;
    }

    #[must_use]
    pub fn is_muted(&self) -> bool {
        self.shared.muted.load(Ordering::Relaxed)
    }

    /// Stops playing audio. The tracks stay subscribed: re-deafening should be
    /// instant, and a renegotiation round trip would not be.
    pub async fn set_deafened(&self, deafened: bool) {
        self.shared.deafened.store(deafened, Ordering::Relaxed);
        self.shared
            .tell_room(ClientMessage::Deafen { deafened })
            .await;
    }

    #[must_use]
    pub fn is_deafened(&self) -> bool {
        self.shared.deafened.load(Ordering::Relaxed)
    }

    /// Throws the current seat away and takes another one, as if the transport
    /// had died underneath it.
    ///
    /// This is the drill in `bin/reconnect-drill.rs`: killing a real network
    /// needs a privileged tool and a second machine, and neither is available
    /// to a test that has to run anywhere. What it exercises is the same code
    /// a real drop runs — rejoin, republish, resubscribe — from the point the
    /// session is declared lost. See docs/testing/reconnect.md for the run that
    /// takes the network away for real.
    pub fn drop_session(&self) {
        self.shared.lost.notify_one();
    }

    /// Leaves the room and closes the transport.
    ///
    /// Telling the room first is what makes the departure instant for everyone
    /// else; dropping the `Call` without this works too, but the roster only
    /// catches up on the next heartbeat sweep.
    pub async fn leave(mut self) {
        // Before the message, not after: a socket closing on the way out must
        // read as an ending rather than as the drop that starts a reconnect.
        self.shared.leaving.store(true, Ordering::Relaxed);
        self.shared.tell_room(ClientMessage::Leave).await;
        self.shared.lost.notify_one();

        if let Some(supervisor) = self.supervisor.take() {
            // Bounded: the supervisor closes the peer connection on its way
            // out, which is worth waiting for, but not worth hanging on.
            let _ = tokio::time::timeout(Duration::from_secs(2), supervisor).await;
        }
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.shared.leaving.store(true, Ordering::Relaxed);
        for task in self.tasks.iter().chain(self.supervisor.iter()) {
            task.abort();
        }
    }
}

// --- the supervisor --------------------------------------------------------

/// Everything the supervisor needs for the life of the call.
struct Supervisor {
    signaling: Arc<Signaling>,
    options: CallOptions,
    shared: Arc<Shared>,
    published: watch::Sender<Option<Arc<dyn PacketSink>>>,
}

/// Why a session stopped.
enum SessionEnd {
    /// The user left, or the call is being torn down.
    Left,
    /// The seat is gone. The reason is for the log, not for a decision.
    Dropped(String),
}

/// Runs sessions back to back for as long as the user stays in the room.
///
/// The room keeps nothing (DR-5), so there is nothing to resume: every
/// reconnect is a fresh join that republishes the microphone and pulls every
/// roommate again. What the user typed — the room code — is the only thing
/// carried across.
async fn supervise(supervisor: Supervisor, first: Session) {
    let mut session = first;
    let mut backoff = Backoff::new();

    loop {
        supervisor
            .shared
            .adopt(&session, &supervisor.published)
            .await;
        supervisor.shared.drain_lost().await;

        let end = run_session(&supervisor, session).await;
        // Nowhere to send audio until there is a new seat. The encode loop
        // keeps draining the microphone so the capture ring cannot overflow.
        supervisor.published.send_replace(None);

        if supervisor.shared.is_leaving() || matches!(end, SessionEnd::Left) {
            supervisor.shared.finish(EndReason::Left);
            return;
        }

        if let SessionEnd::Dropped(detail) = end {
            eprintln!("call dropped ({detail}); reconnecting");
        }

        match reconnect(&supervisor, &mut backoff).await {
            Ok(next) => {
                session = next;
                backoff.reset();
            }
            Err(reason) => {
                supervisor.shared.finish(reason);
                return;
            }
        }
    }
}

/// Takes a new seat in the same room, on the schedule in [`super::reconnect`].
async fn reconnect(supervisor: &Supervisor, backoff: &mut Backoff) -> Result<Session, EndReason> {
    let mut detail = "the room stopped answering".to_owned();

    loop {
        let Some(delay) = backoff.next_delay() else {
            return Err(EndReason::Unreachable { detail });
        };
        supervisor
            .shared
            .state
            .send_replace(CallState::Reconnecting {
                attempt: backoff.attempt(),
            });

        tokio::time::sleep(delay).await;
        if supervisor.shared.is_leaving() {
            return Err(EndReason::Left);
        }

        match connect_once(&supervisor.signaling, &supervisor.options).await {
            Ok(session) => return Ok(session),
            // A full room is worth waiting out here and not on a first join:
            // the seat that filled it may be this client's own, held by the
            // room until the heartbeat sweep clears it (DR-5).
            Err(error) if error.is_worth_retrying() || error.is_room_full() => {
                detail = error.to_string();
            }
            Err(error) => {
                return Err(EndReason::Refused {
                    detail: error.to_string(),
                })
            }
        }
    }
}

/// Follows one seat until it stops working.
///
/// Pulls every `mic` the roster reveals, drops every one that goes away,
/// republishes the roster for the UI, and watches the four ways a session can
/// end: the transport failing, the room hanging up, the microphone's sender
/// dying, or the user leaving.
async fn run_session(supervisor: &Supervisor, session: Session) -> SessionEnd {
    let mut inbound = session.inbound;
    let subscriber = Subscriber {
        signaling: Arc::clone(&supervisor.signaling),
        shared: Arc::clone(&supervisor.shared),
        self_id: session.self_id,
        peer: session.peer,
        signals: session.signals,
        published: session.published,
    };

    let mut playing: HashMap<String, Subscription> = HashMap::new();
    let mut latest = session.participants;
    reconcile(&subscriber, &mut playing, &latest).await;

    let mut retry = tokio::time::interval(RESUBSCRIBE_INTERVAL);
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut meter = tokio::time::interval(METER_INTERVAL);
    meter.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut connection = subscriber.signals.connection.clone();

    // Armed when ICE first reports `Disconnected` and disarmed if it comes
    // back; firing is what turns a stall into a reconnect.
    let grace = tokio::time::sleep(Duration::from_secs(0));
    tokio::pin!(grace);
    let mut stalled = false;

    let end = loop {
        tokio::select! {
            message = inbound.recv() => {
                let Some(message) = message else {
                    break SessionEnd::Dropped("the room closed the connection".to_owned());
                };
                match message {
                    ServerMessage::Welcome { participants, .. }
                    | ServerMessage::Roster { participants } => {
                        latest = participants;
                        subscriber.shared.roster.send_replace(latest.clone());
                        reconcile(&subscriber, &mut playing, &latest).await;
                    }
                    ServerMessage::Error { code, message } => {
                        eprintln!("room error: {message} ({code})");
                    }
                }
            }
            _ = retry.tick() => {
                // The roster is pushed only when it changes, so a subscription
                // that failed would otherwise leave that speaker silent for the
                // rest of the call. A no-op when everyone is already subscribed.
                reconcile(&subscriber, &mut playing, &latest).await;
            }
            _ = meter.tick() => {
                // Reading eight atomics; the push after it happens only when
                // somebody started or stopped talking.
                subscriber.shared.refresh_speaking();
            }
            changed = connection.changed() => {
                if changed.is_err() {
                    break SessionEnd::Dropped("the peer connection went away".to_owned());
                }
                let state = *connection.borrow_and_update();
                match state {
                    RTCPeerConnectionState::Connected => stalled = false,
                    RTCPeerConnectionState::Disconnected if !stalled => {
                        stalled = true;
                        grace.as_mut().reset(tokio::time::Instant::now() + DISCONNECT_GRACE);
                    }
                    RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
                        break SessionEnd::Dropped(format!("the transport went {state}"));
                    }
                    _ => {}
                }
            }
            () = &mut grace, if stalled => {
                break SessionEnd::Dropped("the transport stalled".to_owned());
            }
            () = subscriber.shared.lost.notified() => {
                if subscriber.shared.is_leaving() {
                    break SessionEnd::Left;
                }
                break SessionEnd::Dropped("the microphone stopped reaching the SFU".to_owned());
            }
        }
    };

    // Whatever went wrong, this seat is finished with: stop the playback tasks
    // before they write into slots the next session is about to hand out, and
    // close the transport rather than leaving ICE running on a dead session.
    for (_, subscription) in playing.drain() {
        subscription.playback.abort();
        subscriber.shared.sink.clear(subscription.slot);
    }
    subscriber.shared.silence();

    // Hand the seat back on the way out. A drop does not always mean the room
    // is unreachable — a dead sender or a stalled ICE leaves the WebSocket
    // working — and a seat nobody gave back is what makes the *next* join get
    // refused by a room that is full of this client's own ghosts (DR-5).
    let _ = session.commands.send(ClientMessage::Leave).await;
    let _ = subscriber.peer.close().await;

    end
}

// --- peer connection -------------------------------------------------------

/// Everything the session waits on, published as state rather than as edges.
///
/// A renegotiation does not re-run ICE gathering, so a listener for the
/// *transition* to `Complete` would wait forever on the second pull. Watching
/// the value instead makes "already complete" the same answer as "just became
/// complete".
struct Signals {
    gathering: watch::Receiver<RTCIceGatheringState>,
    connection: watch::Receiver<RTCPeerConnectionState>,
    tracks: Arc<TrackInbox>,
}

/// Remote tracks that have arrived but not yet been claimed by a subscription.
#[derive(Default)]
struct TrackInbox {
    waiting: Mutex<Vec<Arc<dyn TrackRemote>>>,
    arrived: Notify,
}

struct Events {
    gathering: watch::Sender<RTCIceGatheringState>,
    connection: watch::Sender<RTCPeerConnectionState>,
    tracks: Arc<TrackInbox>,
}

impl Events {
    fn new() -> (Arc<Self>, Signals) {
        let (gathering_tx, gathering) = watch::channel(RTCIceGatheringState::New);
        let (connection_tx, connection) = watch::channel(RTCPeerConnectionState::New);
        let tracks = Arc::new(TrackInbox::default());

        (
            Arc::new(Self {
                gathering: gathering_tx,
                connection: connection_tx,
                tracks: Arc::clone(&tracks),
            }),
            Signals {
                gathering,
                connection,
                tracks,
            },
        )
    }
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Events {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        let _ = self.gathering.send(state);
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.connection.send(state);
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        if let Ok(mut waiting) = self.tracks.waiting.lock() {
            waiting.push(track);
        }
        self.tracks.arrived.notify_waiters();
    }
}

/// The one codec on this path. RFC 7587 fixes the rtpmap encoding parameter at
/// 2 even for a mono stream, so the SDP says stereo while the payload stays
/// mono — Opus packets carry their own channel count.
fn opus_codec() -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: MIME_TYPE_OPUS.to_owned(),
        clock_rate: SAMPLE_RATE_HZ,
        channels: 2,
        sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
        rtcp_feedback: vec![],
    }
}

async fn open_peer(
    ice_servers: &[IceServer],
    handler: Arc<Events>,
) -> Result<impl PeerConnection, RtcError> {
    let mut media_engine = MediaEngine::default();
    media_engine.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: opus_codec(),
            payload_type: OPUS_PAYLOAD_TYPE,
        },
        RtpCodecKind::Audio,
    )?;
    let registry = register_default_interceptors(Registry::new(), &mut media_engine)?;
    let runtime = default_runtime()
        .ok_or_else(|| RtcError::Transport("webrtc was built without a runtime".to_owned()))?;

    // Keep the DTLS role we were given at publish time. Cloudflare answers our
    // first offer with `a=setup:passive`, making them the DTLS server; every
    // pull after that is *their* offer with `a=setup:actpass`, and answering it
    // with the default `passive` would claim the server role for ourselves. The
    // handshake restarts, and the sender that was carrying the microphone dies
    // with it — see DR-8.
    let mut setting_engine = SettingEngine::default();
    setting_engine.set_answering_dtls_role(RTCDtlsRole::Client)?;

    let servers = ice_servers
        .iter()
        .map(|server| RTCIceServer {
            urls: server.urls.to_vec(),
            username: server.username.clone().unwrap_or_default(),
            credential: server.credential.clone().unwrap_or_default(),
        })
        .collect();

    // Boxed: the builder carries the media engine and interceptor chain, so
    // its future is ~21 kB and would otherwise sit in every caller's frame.
    Ok(Box::pin(
        PeerConnectionBuilder::new()
            .with_configuration(
                RTCConfigurationBuilder::new()
                    .with_ice_servers(servers)
                    .build(),
            )
            .with_media_engine(media_engine)
            .with_setting_engine(setting_engine)
            .with_interceptor_registry(registry)
            .with_handler(handler)
            .with_runtime(runtime)
            .with_udp_addrs(vec![format!("{}:0", local_ip())])
            .build(),
    )
    .await?)
}

/// The address ICE should gather from.
///
/// Binding the wildcard makes `webrtc-rs` offer a candidate per interface, and
/// a machine with a VPN, a container bridge or a virtual adapter has several
/// that cannot reach Cloudflare at all. Since the SFU is ice-lite — one remote
/// candidate, no connectivity checks of its own (DR-7) — a bad local pick has
/// nothing to fall back to and the handshake simply times out.
///
/// The socket connects but sends nothing: it exists so the routing table
/// answers "which interface reaches the internet".
fn local_ip() -> String {
    const WILDCARD: &str = "0.0.0.0";

    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            // Cloudflare's resolver, chosen only because it is a routable
            // address that is not us.
            socket.connect("1.1.1.1:53")?;
            socket.local_addr()
        })
        .map_or_else(|_| WILDCARD.to_owned(), |address| address.ip().to_string())
}

/// Cloudflare negotiates without trickle, so the SDP that goes up has to carry
/// every candidate already.
async fn local_sdp(peer: &dyn PeerConnection, signals: &Signals) -> Result<String, RtcError> {
    let mut gathering = signals.gathering.clone();
    // The state is copied out of the watch straight away: the borrow it hands
    // back is not `Send`, and this runs inside a spawned task.
    let gathered = tokio::time::timeout(
        CONNECT_TIMEOUT,
        gathering.wait_for(|state| *state == RTCIceGatheringState::Complete),
    )
    .await
    .map(|result| result.map(|state| *state));

    match gathered {
        Ok(Ok(_)) => {}
        Ok(Err(_)) => {
            return Err(RtcError::Transport(
                "the peer connection went away during ICE gathering".to_owned(),
            ))
        }
        Err(_) => {
            return Err(RtcError::Transport(
                "ICE gathering never completed".to_owned(),
            ))
        }
    }

    peer.local_description()
        .await
        .map(|description| description.sdp)
        .ok_or_else(|| RtcError::Transport("no local description to send".to_owned()))
}

async fn wait_for_connection(signals: &Signals) -> Result<(), RtcError> {
    let mut connection = signals.connection.clone();
    // Copied out of the watch immediately: the borrow it hands back is neither
    // `Send` nor outlives this receiver.
    let settled = tokio::time::timeout(
        CONNECT_TIMEOUT,
        connection.wait_for(|state| {
            matches!(
                state,
                RTCPeerConnectionState::Connected
                    | RTCPeerConnectionState::Failed
                    | RTCPeerConnectionState::Closed
            )
        }),
    )
    .await
    .map(|result| result.map(|state| *state));

    let Ok(Ok(state)) = settled else {
        return Err(RtcError::NotConnected);
    };

    if state == RTCPeerConnectionState::Connected {
        Ok(())
    } else {
        Err(RtcError::NotConnected)
    }
}

// --- publishing ------------------------------------------------------------

/// Where an encoded frame goes.
///
/// The encode loop outlives any one session, so it writes through this rather
/// than at a track it captured at join time: reconnecting swaps the
/// implementation underneath it, and the microphone never stops being read.
#[async_trait::async_trait]
trait PacketSink: Send + Sync {
    /// Puts one Opus packet on the wire.
    async fn send(&self, packet: &[u8]) -> Result<(), String>;
}

/// A published track, plus what the encode loop needs to keep writing to it.
///
/// The SSRC and payload type are shared and re-read every frame rather than
/// captured once: a renegotiation can rebuild the sender underneath the track,
/// and a loop still addressing the old one writes into a closed channel (DR-8).
#[derive(Clone)]
struct Published {
    track: Arc<TrackLocalStaticSample>,
    ssrc: Arc<AtomicU32>,
    payload_type: Arc<AtomicU8>,
}

#[async_trait::async_trait]
impl PacketSink for Published {
    /// The allocation per frame (`Bytes::copy_from_slice`) is deliberate and
    /// safe: this runs on the far side of the capture ring buffer, never inside
    /// a device callback, which is where styleguide.md's no-allocation rule
    /// applies.
    async fn send(&self, packet: &[u8]) -> Result<(), String> {
        self.track
            .sample_writer(
                self.ssrc.load(Ordering::Relaxed),
                self.payload_type.load(Ordering::Relaxed),
            )
            .write_sample(&Sample {
                data: Bytes::copy_from_slice(packet),
                duration: Duration::from_millis(u64::from(FRAME_MS)),
                ..Default::default()
            })
            .await
            .map_err(|error| error.to_string())
    }
}

impl Published {
    /// Re-reads the sender's identity from the peer connection.
    ///
    /// Called after every renegotiation. A failure here is not fatal: the
    /// previous values stay in place, and the next renegotiation tries again.
    async fn refresh(&self, peer: &dyn PeerConnection) {
        if let Some(payload_type) = negotiated_payload_type(peer).await {
            self.payload_type.store(payload_type, Ordering::Relaxed);
        }
        if let Some(&ssrc) = self.track.ssrcs().await.first() {
            self.ssrc.store(ssrc, Ordering::Relaxed);
        }
    }
}

/// The payload type Cloudflare agreed to for our outgoing audio.
async fn negotiated_payload_type(peer: &dyn PeerConnection) -> Option<PayloadType> {
    peer.get_senders()
        .await
        .first()?
        .get_parameters()
        .await
        .ok()?
        .rtp_parameters
        .codecs
        .first()
        .map(|codec| codec.payload_type)
}

async fn publish_mic(
    signaling: &Signaling,
    self_id: &str,
    peer: &dyn PeerConnection,
    signals: &Signals,
) -> Result<Published, RtcError> {
    let track = Arc::new(TrackLocalStaticSample::new(MediaStreamTrack::new(
        "goodvoice".to_owned(),
        MIC_TRACK.to_owned(),
        "goodvoice microphone".to_owned(),
        RtpCodecKind::Audio,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(fresh_ssrc()),
                ..Default::default()
            },
            codec: opus_codec(),
            ..Default::default()
        }],
    ))?);

    // Sendonly, not sendrecv: this transceiver has nothing to receive, and
    // offering to would have the SFU reserve a slot for media that never comes.
    let transceiver = peer
        .add_transceiver_from_track(
            Arc::clone(&track) as Arc<dyn TrackLocal>,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Sendonly,
                ..Default::default()
            }),
        )
        .await?;

    let offer = peer.create_offer(None).await?;
    peer.set_local_description(offer).await?;
    let sdp = local_sdp(peer, signals).await?;

    // The mid is assigned by `create_offer`; it is how Cloudflare pairs the
    // m-section in the SDP with the name the room stores against this
    // participant (DR-6).
    let mid = transceiver
        .mid()
        .await?
        .ok_or_else(|| RtcError::Protocol("transceiver has no mid".to_owned()))?;

    let answer = signaling
        .sfu(
            self_id,
            SfuOperation::TracksNew,
            &json!({
                "sessionDescription": { "type": "offer", "sdp": sdp },
                "tracks": [{ "location": "local", "mid": mid, "trackName": MIC_TRACK }],
            }),
        )
        .await?;

    peer.set_remote_description(RTCSessionDescription::answer(sdp_of(&answer)?)?)
        .await?;

    let payload_type = negotiated_payload_type(peer)
        .await
        .ok_or_else(|| RtcError::Transport("sender has no negotiated codec".to_owned()))?;

    let ssrc = *track
        .ssrcs()
        .await
        .first()
        .ok_or_else(|| RtcError::Transport("published track has no SSRC".to_owned()))?;

    Ok(Published {
        track,
        ssrc: Arc::new(AtomicU32::new(ssrc)),
        payload_type: Arc::new(AtomicU8::new(payload_type)),
    })
}

/// A synchronisation source for the outgoing stream. Nanoseconds since the
/// epoch folded into 32 bits: enough to keep two clients in a room apart, which
/// is all an SSRC has to do here.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the low bits are the point — this is a nonce, not a clock"
)]
fn fresh_ssrc() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |elapsed| {
            elapsed.subsec_nanos() ^ elapsed.as_secs() as u32
        })
}

/// Streams encoded microphone audio for as long as the source produces it.
///
/// Runs for the whole call, not for one session. Between sessions there is
/// nowhere to send: the frames are still read — a microphone nobody drains
/// overruns its ring — encoded, and dropped.
async fn publish_loop(
    mut source: Box<dyn AudioSource>,
    sinks: watch::Receiver<Option<Arc<dyn PacketSink>>>,
    shared: Arc<Shared>,
) {
    let Ok(mut encoder) = VoiceEncoder::new() else {
        return;
    };
    let mut packet = [0_u8; MAX_PACKET_BYTES];
    let mut failures = 0_usize;

    while let Some(frame) = source.next_frame().await {
        // Mute stops packets rather than sending silence: the room should cost
        // nothing while nobody is talking (prd.md §3 F1). Nothing is encoded
        // either — a frame nobody will send is work nobody asked for.
        if !shared.is_transmitting() {
            // The meter follows what the room hears, so a muted client reads
            // as silent however loudly they are talking.
            shared.microphone.observe(0);
            continue;
        }
        // This is the only place the outgoing signal exists as samples, so it
        // is the only place the "you are talking" indicator can come from.
        shared.microphone.observe(peak(&frame));
        // Cloned out of the watch immediately: the borrow it hands back is not
        // `Send` and this is about to await.
        let sink = sinks.borrow().clone();
        let Some(sink) = sink else {
            continue;
        };

        let Ok(written) = encoder.encode(&frame, &mut packet) else {
            continue;
        };

        // A failed write is not fatal on its own — the transport can be
        // between states — but a run of them means nobody can hear this
        // client, which is a dropped session rather than something to sit in.
        match sink.send(&packet[..written]).await {
            Ok(()) => failures = 0,
            Err(error) => {
                failures += 1;
                if failures == 1 {
                    eprintln!("microphone frame not sent: {error}");
                }
                if failures >= PUBLISH_FAILURE_LIMIT {
                    failures = 0;
                    shared.lost.notify_one();
                }
            }
        }
    }
}

// --- subscribing -----------------------------------------------------------

/// One session's view of the room, and what it takes to pull from it.
struct Subscriber {
    signaling: Arc<Signaling>,
    shared: Arc<Shared>,
    self_id: String,
    peer: Arc<dyn PeerConnection>,
    signals: Signals,
    /// Shared with the encode loop so a renegotiation can hand it the sender's
    /// new identity.
    published: Published,
}

/// One remote speaker, for as long as they are in the room.
struct Subscription {
    slot: usize,
    /// The Realtime session this pull was addressed to. A peer who rejoined has
    /// a new one, and the old address is refused by the proxy (DR-8).
    session_id: String,
    playback: JoinHandle<()>,
}

async fn reconcile(
    subscriber: &Subscriber,
    playing: &mut HashMap<String, Subscription>,
    participants: &[Participant],
) {
    // Gone, stopped publishing, or back under a new session: free the slot so
    // the next arrival can have it, and stop the audio immediately rather than
    // draining the ring.
    playing.retain(|id, subscription| {
        let still_here = participants.iter().any(|peer| {
            &peer.id == id
                && peer.publishes(MIC_TRACK)
                && peer.session_id.as_deref() == Some(subscription.session_id.as_str())
        });
        if !still_here {
            subscription.playback.abort();
            subscriber.shared.sink.clear(subscription.slot);
        }
        still_here
    });

    for peer in participants {
        if peer.id == subscriber.self_id || playing.contains_key(&peer.id) {
            continue;
        }
        if !peer.publishes(MIC_TRACK) {
            continue;
        }
        let Some(session_id) = peer.session_id.as_deref() else {
            continue;
        };

        let Some(slot) = free_slot(playing) else {
            // Only reachable if the server's cap and this client's slot count
            // ever disagree; being silent about one speaker beats crashing.
            eprintln!("no playback slot left for {}", peer.name);
            continue;
        };

        match subscribe_to(subscriber, session_id, slot).await {
            Ok(subscription) => {
                playing.insert(peer.id.clone(), subscription);
            }
            Err(error) => eprintln!("could not subscribe to {}: {error}", peer.name),
        }
    }

    // The slot a participant is played in is what turns a level into a name.
    // Rebuilt rather than patched: `playing` is the only truth, and the map is
    // at most eight entries.
    if let Ok(mut slots) = subscriber.shared.slots.lock() {
        slots.clear();
        for (id, subscription) in playing.iter() {
            slots.insert(id.clone(), subscription.slot);
        }
    }
}

fn free_slot(playing: &HashMap<String, Subscription>) -> Option<usize> {
    (0..MAX_REMOTE_SLOTS).find(|slot| {
        !playing
            .values()
            .any(|subscription| subscription.slot == *slot)
    })
}

/// Pulls one remote `mic` and starts playing it.
///
/// Pulling is the mirror of publishing: the client sends no SDP, Cloudflare
/// answers with an **offer**, and this side answers it through `renegotiate`
/// (DR-7).
async fn subscribe_to(
    subscriber: &Subscriber,
    session_id: &str,
    slot: usize,
) -> Result<Subscription, RtcError> {
    let answer = pull_track(subscriber, session_id).await?;

    // Cloudflare only offers when it needs a new m-section. Reusing one that
    // is already there is a valid answer with no SDP in it, and forcing a
    // renegotiation would be a round trip for nothing.
    if answer
        .get("requiresImmediateRenegotiation")
        .and_then(Value::as_bool)
        == Some(true)
    {
        renegotiate(subscriber, &answer).await?;
    }

    let ssrc = ssrc_of(&answer);
    let track = claim_track(&subscriber.signals.tracks, ssrc).await?;

    Ok(Subscription {
        slot,
        session_id: session_id.to_owned(),
        playback: tokio::spawn(playback_loop(track, slot, Arc::clone(&subscriber.shared))),
    })
}

/// Asks for a remote track, waiting out the window where the publisher has
/// negotiated but is not yet sending.
///
/// The roster learns about a track when the publisher's `tracks/new` is
/// accepted, which is before their ICE and DTLS finish — so a peer is
/// advertised a beat before Cloudflare will serve them. Retrying is the whole
/// fix; there is nothing wrong on either side (DR-8).
async fn pull_track(subscriber: &Subscriber, session_id: &str) -> Result<Value, RtcError> {
    let body = json!({
        "tracks": [{
            "location": "remote",
            "sessionId": session_id,
            "trackName": MIC_TRACK,
        }],
    });

    let mut delay = SUBSCRIBE_BACKOFF;
    let mut last = String::new();

    for attempt in 0..SUBSCRIBE_ATTEMPTS {
        let answer = subscriber
            .signaling
            .sfu(&subscriber.self_id, SfuOperation::TracksNew, &body)
            .await?;

        let Some((code, description)) = track_error(&answer) else {
            return Ok(answer);
        };
        if !is_starting_up(&code) {
            return Err(RtcError::Sfu(format!("{description} ({code})")));
        }

        last = format!("{description} ({code})");
        if attempt + 1 < SUBSCRIBE_ATTEMPTS {
            tokio::time::sleep(delay).await;
            delay *= 2;
        }
    }

    Err(RtcError::Sfu(format!(
        "the publisher never started sending: {last}"
    )))
}

/// Answers the offer Cloudflare sent with the pull.
async fn renegotiate(subscriber: &Subscriber, answer: &Value) -> Result<(), RtcError> {
    let offer = sdp_of(answer)?;
    trace_sdp("pull offer", &offer);

    subscriber
        .peer
        .set_remote_description(RTCSessionDescription::offer(offer)?)
        .await?;
    let local = subscriber.peer.create_answer(None).await?;
    subscriber.peer.set_local_description(local).await?;
    let sdp = local_sdp(subscriber.peer.as_ref(), &subscriber.signals).await?;
    trace_sdp("our answer", &sdp);

    subscriber
        .signaling
        .sfu(
            &subscriber.self_id,
            SfuOperation::Renegotiate,
            &json!({ "sessionDescription": { "type": "answer", "sdp": sdp } }),
        )
        .await?;

    // The microphone's sender may have been rebuilt by the exchange above; the
    // publish loop has to be told before its next frame goes nowhere.
    subscriber.published.refresh(subscriber.peer.as_ref()).await;
    Ok(())
}

/// Dumps one SDP when `GOODVOICE_TRACE_SDP` is set.
///
/// Every hard problem on this path so far has been visible in the SDP and
/// nowhere else — the DTLS role reversal behind DR-8 was found this way — so
/// the hook stays rather than being reinvented next time.
fn trace_sdp(label: &str, sdp: &str) {
    if std::env::var_os("GOODVOICE_TRACE_SDP").is_some() {
        eprintln!("=== {label} ===\n{sdp}");
    }
}

/// The per-track failure Cloudflare reports inside an otherwise-successful
/// answer, if there is one.
///
/// Spelled `errorCode`/`errorDescription`, never `error` — observed live, see
/// DR-8. The Worker's own roster bookkeeping keys off the same field.
fn track_error(answer: &Value) -> Option<(String, String)> {
    let track = answer
        .get("tracks")
        .and_then(Value::as_array)
        .and_then(|tracks| tracks.first())?;

    let code = track.get("errorCode").and_then(Value::as_str)?;
    let description = track
        .get("errorDescription")
        .and_then(Value::as_str)
        .unwrap_or("no description");
    Some((code.to_owned(), description.to_owned()))
}

/// Whether a per-track failure means "not yet" rather than "no".
///
/// All three are states a peer passes through on the way into a room: the
/// roster announces a track when its publisher's `tracks/new` is accepted,
/// which is before their DTLS finishes and well before their first RTP packet.
/// Observed live, see DR-8.
fn is_starting_up(code: &str) -> bool {
    matches!(
        code,
        // The publisher's session has no such track yet.
        "not_found_track_error"
            // The track exists but no packets have arrived on it.
            | "empty_track_error"
            // Cloudflare's own transport is not ready to serve the pull.
            | "transport_unavailable_error"
    )
}

/// The SSRC the pulled track will arrive on, when the answer names one.
fn ssrc_of(answer: &Value) -> Option<u32> {
    let sdp = sdp_of(answer).ok()?;
    let mid = answer
        .get("tracks")
        .and_then(Value::as_array)
        .and_then(|tracks| tracks.first())
        .and_then(|track| track.get("mid"))
        .and_then(Value::as_str)?;
    ssrc_for_mid(&sdp, mid)
}

/// Takes the track this subscription is waiting for out of the inbox.
///
/// Matched by SSRC when the offer named one, so several pulls in flight cannot
/// hand each other the wrong stream. Without one, the oldest unclaimed track is
/// the only reasonable guess.
async fn claim_track(
    inbox: &Arc<TrackInbox>,
    ssrc: Option<u32>,
) -> Result<Arc<dyn TrackRemote>, RtcError> {
    let deadline = tokio::time::Instant::now() + TRACK_TIMEOUT;

    loop {
        // Registered before the scan, so a track arriving between the two is
        // not missed.
        let arrived = inbox.arrived.notified();

        if let Some(track) = take_matching(inbox, ssrc).await {
            return Ok(track);
        }

        if tokio::time::timeout_at(deadline, arrived).await.is_err() {
            return Err(RtcError::NotConnected);
        }
    }
}

async fn take_matching(inbox: &Arc<TrackInbox>, ssrc: Option<u32>) -> Option<Arc<dyn TrackRemote>> {
    let candidates = {
        let waiting = inbox.waiting.lock().ok()?;
        waiting.clone()
    };

    for candidate in candidates {
        let matches = match ssrc {
            Some(wanted) => candidate.ssrcs().await.contains(&wanted),
            None => true,
        };
        if !matches {
            continue;
        }
        let mut waiting = inbox.waiting.lock().ok()?;
        if let Some(index) = waiting
            .iter()
            .position(|held| Arc::ptr_eq(held, &candidate))
        {
            return Some(waiting.remove(index));
        }
    }
    None
}

/// Decodes one remote track into its playback slot until it ends.
///
/// The RTP payload of an Opus stream *is* the Opus packet — there is no
/// aggregation header to strip — so depacketising and decoding are one step.
async fn playback_loop(track: Arc<dyn TrackRemote>, slot: usize, shared: Arc<Shared>) {
    let Ok(mut decoder) = VoiceDecoder::new() else {
        return;
    };
    let mut frame = silent_frame();

    while let Some(event) = track.poll().await {
        match event {
            TrackRemoteEvent::OnRtpPacket(packet) => {
                play_packet(&mut decoder, &packet.payload, &mut frame, slot, &shared);
            }
            TrackRemoteEvent::OnEnded => break,
            _ => {}
        }
    }

    shared.sink.clear(slot);
}

/// Turns one RTP payload into 20 ms of playback, unless the user is deafened.
///
/// Deafened still decodes. Opus carries state across packets, so skipping them
/// would leave the decoder mid-stream and the first seconds after un-deafening
/// would be artefacts. What deafen stops is the last step — nothing reaches the
/// speakers (prd.md §3 F1).
fn play_packet(
    decoder: &mut VoiceDecoder,
    payload: &[u8],
    frame: &mut Frame,
    slot: usize,
    shared: &Shared,
) {
    if payload.is_empty() {
        return;
    }
    if decoder.decode(payload, frame).is_err() {
        return;
    }
    if shared.deafened.load(Ordering::Relaxed) {
        return;
    }
    shared.sink.play(slot, frame);
}

// --- SDP ------------------------------------------------------------------

fn sdp_of(answer: &Value) -> Result<String, RtcError> {
    answer
        .get("sessionDescription")
        .and_then(|description| description.get("sdp"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        // The whole answer goes into the message: Cloudflare reports per-track
        // failures inside a 200, so the body is the only place the reason is.
        .ok_or_else(|| RtcError::Protocol(format!("no SDP in the SFU's answer: {answer}")))
}

/// The first SSRC advertised in the m-section carrying `mid`.
///
/// Cloudflare puts one `a=ssrc:` per pulled track, so this is the identity of
/// the stream that is about to arrive on `on_track`. Absent in an offer that
/// does not name one, in which case the caller falls back to arrival order.
fn ssrc_for_mid(sdp: &str, mid: &str) -> Option<u32> {
    for section in sdp.split("\nm=").skip(1) {
        let mut section_mid = None;
        let mut section_ssrc = None;

        for line in section.lines() {
            let line = line.trim_end();
            if let Some(value) = line.strip_prefix("a=mid:") {
                section_mid = Some(value);
            } else if let Some(value) = line.strip_prefix("a=ssrc:") {
                if section_ssrc.is_none() {
                    section_ssrc = value.split_whitespace().next()?.parse::<u32>().ok();
                }
            }
        }

        if section_mid == Some(mid) {
            return section_ssrc;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{
        is_starting_up, play_packet, publish_loop, silent_frame, ssrc_for_mid, ssrc_of,
        track_error, PacketSink, Shared,
    };
    use crate::audio::{
        device::{AudioSink, AudioSource, NullSink, RecordingSink, ToneSource},
        mixer::SPEAKING_LEVEL,
        opus::{Frame, VoiceDecoder, VoiceEncoder, MAX_PACKET_BYTES},
    };
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use tokio::sync::watch;

    /// Trimmed from a real Cloudflare pull offer (DR-7).
    const PULL_OFFER: &str = "v=0\r\n\
        o=- 3273753808686040790 1787244308 IN IP4 0.0.0.0\r\n\
        s=-\r\n\
        t=0 0\r\n\
        a=group:BUNDLE 0\r\n\
        m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
        c=IN IP4 0.0.0.0\r\n\
        a=mid:0\r\n\
        a=rtpmap:111 opus/48000/2\r\n\
        a=ssrc:1045349574 cname:CKsJGAxk\r\n\
        a=ssrc:1045349574 msid:CKsJGAxk hmqzhHaNhhKObzUU\r\n\
        a=sendonly\r\n";

    #[test]
    fn the_ssrc_is_read_out_of_the_matching_section() {
        assert_eq!(ssrc_for_mid(PULL_OFFER, "0"), Some(1_045_349_574));
    }

    #[test]
    fn a_mid_that_is_not_there_has_no_ssrc() {
        assert_eq!(ssrc_for_mid(PULL_OFFER, "1"), None);
    }

    #[test]
    fn sections_do_not_leak_ssrcs_into_each_other() {
        let two = "v=0\r\n\
            m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
            a=mid:0\r\n\
            a=ssrc:111 cname:a\r\n\
            m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
            a=mid:1\r\n\
            a=ssrc:222 cname:b\r\n";

        assert_eq!(ssrc_for_mid(two, "0"), Some(111));
        assert_eq!(ssrc_for_mid(two, "1"), Some(222));
    }

    #[test]
    fn a_section_without_an_ssrc_is_not_an_error() {
        // A publish answer names the mid but no SSRC; the caller falls back to
        // arrival order rather than failing the subscription.
        let no_ssrc = "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=recvonly\r\n";
        assert_eq!(ssrc_for_mid(no_ssrc, "0"), None);
    }

    #[test]
    fn a_good_answer_carries_no_track_error() {
        let answer = json!({
            "requiresImmediateRenegotiation": true,
            "sessionDescription": { "type": "offer", "sdp": PULL_OFFER },
            "tracks": [{ "mid": "0", "sessionId": "sess", "trackName": "mic" }],
        });

        assert!(track_error(&answer).is_none());
        assert_eq!(ssrc_of(&answer), Some(1_045_349_574));
    }

    /// Verbatim from a live run: this is the shape DR-6 could only infer.
    #[test]
    fn a_publisher_that_is_not_sending_yet_is_reported_per_track() {
        let answer = json!({
            "requiresImmediateRenegotiation": false,
            "tracks": [{
                "errorCode": "not_found_track_error",
                "errorDescription": "Track not found on remote peer. Make sure the publisher peer is connected and sending packets for this track",
                "mid": "",
                "sessionId": "643ad191638ece3c",
                "trackName": "mic",
            }],
        });

        let (code, description) = track_error(&answer).expect("a per-track error");
        assert_eq!(code, "not_found_track_error");
        assert!(description.contains("sending packets"));
        assert!(is_starting_up(&code), "this one is worth retrying");
        // No SDP came with it, so there is nothing to derive an SSRC from.
        assert_eq!(ssrc_of(&answer), None);
    }

    #[test]
    fn an_empty_track_is_also_worth_retrying() {
        // The other half of the same race: the session exists, the packets
        // have not started.
        assert!(is_starting_up("empty_track_error"));
    }

    #[test]
    fn an_unknown_failure_is_not_retried() {
        // Retrying something that will never succeed only delays the report.
        assert!(!is_starting_up("some_other_error"));
        assert!(!is_starting_up(""));
    }

    #[test]
    fn a_track_error_without_a_description_still_reports_its_code() {
        let answer = json!({ "tracks": [{ "errorCode": "mystery" }] });
        let (code, description) = track_error(&answer).expect("a per-track error");
        assert_eq!(code, "mystery");
        assert!(!description.is_empty());
    }

    // --- the playback path, without a network ----------------------------

    /// A call with nowhere to play: enough for anything that only cares about
    /// the encode side.
    fn test_shared() -> Arc<Shared> {
        Shared::new("me".to_owned(), vec![], Arc::new(NullSink))
    }

    /// A call whose speakers can be inspected afterwards.
    fn listening() -> (Arc<Shared>, Arc<RecordingSink>) {
        let ears = RecordingSink::new();
        let shared = Shared::new(
            "me".to_owned(),
            vec![],
            Arc::clone(&ears) as Arc<dyn AudioSink>,
        );
        (shared, ears)
    }

    /// One real Opus packet, encoded the way a roommate would send it.
    fn packet(into: &mut [u8; MAX_PACKET_BYTES]) -> usize {
        let mut encoder = VoiceEncoder::new().expect("encoder");
        let mut frame = silent_frame();
        for (index, sample) in frame.iter_mut().enumerate() {
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            {
                *sample = ((index as i32 % 120) * 60 - 3_600) as i16;
            }
        }
        encoder.encode(&frame, into).expect("encoded")
    }

    /// prd.md §3 F1: "deafen stops playback". The tracks stay subscribed and
    /// the packets keep arriving — what stops is the last step.
    #[test]
    fn deafening_stops_playback_without_stopping_the_decoder() {
        let (shared, ears) = listening();
        let mut decoder = VoiceDecoder::new().expect("decoder");
        let mut frame = silent_frame();
        let mut buffer = [0_u8; MAX_PACKET_BYTES];
        let written = packet(&mut buffer);

        play_packet(&mut decoder, &buffer[..written], &mut frame, 0, &shared);
        assert_eq!(ears.slot(0).frames, 1, "audible before deafening");

        shared.deafened.store(true, Ordering::Relaxed);
        for _ in 0..5 {
            play_packet(&mut decoder, &buffer[..written], &mut frame, 0, &shared);
        }
        assert_eq!(
            ears.slot(0).frames,
            1,
            "deafened, so nothing new was played"
        );
        // Decoded anyway: the frame the decoder wrote is the audio that would
        // have been played, not silence. Skipping the decode would leave the
        // codec mid-stream and un-deafening would start with artefacts.
        assert!(
            frame.iter().any(|&sample| sample != 0),
            "the packet was never decoded while deafened"
        );

        shared.deafened.store(false, Ordering::Relaxed);
        play_packet(&mut decoder, &buffer[..written], &mut frame, 0, &shared);
        assert_eq!(
            ears.slot(0).frames,
            2,
            "un-deafening did not resume playback"
        );
    }

    #[test]
    fn an_empty_payload_is_not_played_as_a_frame() {
        // Cloudflare sends padding-only packets; concealment is the decoder's
        // job on loss, not something to invent from an empty payload.
        let (shared, ears) = listening();
        let mut decoder = VoiceDecoder::new().expect("decoder");
        let mut frame = silent_frame();

        play_packet(&mut decoder, &[], &mut frame, 0, &shared);

        assert_eq!(ears.slot(0).frames, 0);
    }

    #[test]
    fn a_packet_the_decoder_refuses_is_dropped_rather_than_played() {
        let (shared, ears) = listening();
        let mut decoder = VoiceDecoder::new().expect("decoder");
        let mut frame = silent_frame();

        play_packet(&mut decoder, &[0xff; 8], &mut frame, 0, &shared);

        assert_eq!(ears.slot(0).frames, 0);
    }

    // --- who is talking ---------------------------------------------------

    /// A sink whose levels a test sets directly, standing in for the mixer's.
    #[derive(Default)]
    struct FakeLevels {
        levels: Mutex<HashMap<usize, f32>>,
    }

    impl FakeLevels {
        fn set(&self, slot: usize, level: f32) {
            if let Ok(mut levels) = self.levels.lock() {
                levels.insert(slot, level);
            }
        }
    }

    impl AudioSink for FakeLevels {
        fn play(&self, _slot: usize, _frame: &Frame) {}
        fn clear(&self, _slot: usize) {}
        fn level(&self, slot: usize) -> f32 {
            self.levels
                .lock()
                .ok()
                .and_then(|levels| levels.get(&slot).copied())
                .unwrap_or(0.0)
        }
    }

    /// A call with one roommate in slot 0.
    fn metered() -> (Arc<Shared>, Arc<FakeLevels>) {
        let levels = Arc::new(FakeLevels::default());
        let shared = Shared::new(
            "me".to_owned(),
            vec![],
            Arc::clone(&levels) as Arc<dyn AudioSink>,
        );
        shared
            .slots
            .lock()
            .expect("slots")
            .insert("them".to_owned(), 0);
        (shared, levels)
    }

    #[test]
    fn a_loud_slot_puts_its_participant_in_the_speaking_set() {
        let (shared, levels) = metered();

        levels.set(0, SPEAKING_LEVEL * 2.0);
        shared.refresh_speaking();
        assert_eq!(*shared.speaking.borrow(), vec!["them".to_owned()]);

        levels.set(0, SPEAKING_LEVEL / 2.0);
        shared.refresh_speaking();
        assert!(shared.speaking.borrow().is_empty());
    }

    #[test]
    fn a_quiet_room_pushes_nothing_at_all() {
        // The speaking set feeds an event the UI listens to. A room full of
        // people listening must cost nothing (prd.md §4: under 2% idle).
        let (shared, _levels) = metered();
        let watcher = shared.speaking.subscribe();

        for _ in 0..50 {
            shared.refresh_speaking();
        }

        assert!(!watcher.has_changed().unwrap_or(true));
    }

    #[test]
    fn this_client_is_in_the_set_when_its_own_microphone_is_loud() {
        let (shared, _levels) = metered();

        shared.microphone.observe(20_000);
        shared.refresh_speaking();
        assert_eq!(*shared.speaking.borrow(), vec!["me".to_owned()]);
    }

    #[test]
    fn a_muted_client_is_never_speaking_however_loud_they_are() {
        let (shared, _levels) = metered();
        shared.microphone.observe(30_000);
        shared.muted.store(true, Ordering::Relaxed);

        shared.refresh_speaking();

        assert!(
            shared.speaking.borrow().is_empty(),
            "a muted client showed up as talking; nobody can hear them"
        );
    }

    #[test]
    fn a_seat_that_ends_takes_its_speaking_set_with_it() {
        // Otherwise the last set the meter loop pushed stands for the whole of
        // a reconnect, and the roster shows people talking who are not being
        // heard at all (prd.md §5 flow E).
        let (shared, levels) = metered();
        levels.set(0, SPEAKING_LEVEL * 2.0);
        shared.microphone.observe(20_000);
        shared.refresh_speaking();
        assert_eq!(shared.speaking.borrow().len(), 2);

        shared.silence();

        assert!(shared.speaking.borrow().is_empty());
        assert!(
            shared.slot_of("them").is_none(),
            "a name still pointing at a slot the next seat will re-hand-out"
        );
        // The levels the sink reports have not moved; it is this client that
        // has stopped believing them.
        shared.refresh_speaking();
        assert!(shared.speaking.borrow().is_empty());
    }

    // --- the encode loop, without a network ------------------------------

    /// A [`PacketSink`] that counts instead of transmitting.
    #[derive(Default)]
    struct CountingSender {
        packets: AtomicUsize,
        last: Mutex<Vec<u8>>,
    }

    #[async_trait::async_trait]
    impl PacketSink for CountingSender {
        async fn send(&self, packet: &[u8]) -> Result<(), String> {
            self.packets.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut last) = self.last.lock() {
                last.clear();
                last.extend_from_slice(packet);
            }
            Ok(())
        }
    }

    impl CountingSender {
        fn count(&self) -> usize {
            self.packets.load(Ordering::Relaxed)
        }
    }

    /// A source that hands out a fixed number of loud frames as fast as it is
    /// asked, then ends. No timer, so the test does not wait on one.
    struct Burst {
        left: usize,
    }

    #[async_trait::async_trait]
    impl AudioSource for Burst {
        async fn next_frame(&mut self) -> Option<Frame> {
            self.left = self.left.checked_sub(1)?;
            let mut frame = silent_frame();
            for (index, sample) in frame.iter_mut().enumerate() {
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_possible_wrap,
                    reason = "a deterministic loud-ish waveform, not a signal"
                )]
                {
                    *sample = ((index as i32 % 200) * 40 - 4_000) as i16;
                }
            }
            Some(frame)
        }
    }

    /// Runs the encode loop to exhaustion against a counting sender.
    async fn pump(frames: usize, muted: bool) -> Arc<CountingSender> {
        let shared = test_shared();
        shared.muted.store(muted, Ordering::Relaxed);

        let sender = Arc::new(CountingSender::default());
        let (published, sinks) = watch::channel(Some(Arc::clone(&sender) as Arc<dyn PacketSink>));

        publish_loop(Box::new(Burst { left: frames }), sinks, shared).await;
        drop(published);
        sender
    }

    #[tokio::test]
    async fn every_captured_frame_becomes_a_packet() {
        let sender = pump(25, false).await;
        assert_eq!(sender.count(), 25);
        assert!(
            !sender.last.lock().expect("packet").is_empty(),
            "an empty packet is not a packet"
        );
    }

    /// prd.md §3 F1: "Mute stops sending packets entirely (not zeroed
    /// samples)". A muted client that still sent silence would cost the room
    /// the same bandwidth as a talking one.
    #[tokio::test]
    async fn muting_stops_packets_rather_than_sending_silence() {
        let sender = pump(25, true).await;
        assert_eq!(
            sender.count(),
            0,
            "a muted client put packets on the wire; mute must stop them, not zero them"
        );
    }

    #[tokio::test]
    async fn unmuting_starts_the_packets_again() {
        let shared = test_shared();
        let sender = Arc::new(CountingSender::default());
        let (published, sinks) = watch::channel(Some(Arc::clone(&sender) as Arc<dyn PacketSink>));

        shared.muted.store(true, Ordering::Relaxed);
        let loop_shared = Arc::clone(&shared);
        let pump = tokio::spawn(async move {
            publish_loop(Box::new(ToneSource::new(440.0)), sinks, loop_shared).await;
        });

        // Long enough for several 20 ms frames to have been offered and
        // refused.
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        assert_eq!(sender.count(), 0, "muted, so nothing should have gone out");

        shared.muted.store(false, Ordering::Relaxed);
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        assert!(sender.count() > 0, "unmuting did not resume transmission");

        pump.abort();
        drop(published);
    }

    /// Between sessions there is nowhere to send. The microphone still has to
    /// be read — a source nobody drains overruns its ring — but nothing may go
    /// out, and the loop must survive to use the next session's sender.
    #[tokio::test]
    async fn a_call_with_no_session_drains_the_microphone_and_sends_nothing() {
        let shared = test_shared();
        let sender = Arc::new(CountingSender::default());
        let (published, sinks) = watch::channel(None);

        let loop_shared = Arc::clone(&shared);
        let pump = tokio::spawn(async move {
            publish_loop(Box::new(ToneSource::new(440.0)), sinks, loop_shared).await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        assert_eq!(sender.count(), 0, "there was no session to send on");

        // The reconnect lands: the same loop must pick the new sender up.
        published.send_replace(Some(Arc::clone(&sender) as Arc<dyn PacketSink>));
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        assert!(
            sender.count() > 0,
            "the encode loop did not resume after reconnecting"
        );

        pump.abort();
    }
}
