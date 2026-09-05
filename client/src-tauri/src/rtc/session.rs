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
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering},
        Arc, Mutex, Weak,
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use rtc::{
    media::Sample,
    peer_connection::{
        configuration::media_engine::{MIME_TYPE_H264, MIME_TYPE_OPUS},
        transport::RTCDtlsRole,
    },
    rtp::{codec::h264::H264Packet, packetizer::Depacketizer as _},
    rtp_transceiver::{
        rtp_sender::{
            RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
            RtpCodecKind,
        },
        PayloadType,
    },
};
use serde::Serialize;
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
        PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceCandidateType,
        RTCIceGatheringState, RTCIceServer, RTCPeerConnectionIceEvent, RTCPeerConnectionState,
        RTCSessionDescription, Registry, SettingEngine, StatsSelector,
    },
    rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit, RtpTransceiver},
    runtime::default_runtime,
};

use super::{
    order::{Sequence, Step},
    reconnect::{Backoff, CallState, EndReason},
    screen::{starts_with_idr, ScreenSink, ScreenSource, ScreenSourceFactory, ShareState},
    signaling::{
        ClientMessage, IceServer, JoinResponse, Participant, ServerMessage, SfuOperation, Signaling,
    },
    wire::Wire,
    RtcError,
};
use crate::audio::{
    device::{AudioSink, AudioSource, MAX_REMOTE_SLOTS},
    mixer::{peak, Meter, MAX_GAIN},
    opus::{
        silent_frame, Frame, VoiceDecoder, VoiceEncoder, FRAME_MS, MAX_PACKET_BYTES, SAMPLE_RATE_HZ,
    },
    prefs::AudioPrefs,
    vad::{Gate, TransmitMode},
};

/// The track name a goodvoice client publishes its microphone under. A closed
/// vocabulary server-side (`TRACK_KINDS`), so this string is load-bearing.
const MIC_TRACK: &str = "mic";

/// The track name a screen goes out under (task 5.3). The same closed
/// vocabulary as [`MIC_TRACK`], and what the room's one-sharer rule keys off:
/// a `tracks/new` naming this is what the Durable Object refuses when somebody
/// else is already sharing.
const SCREEN_TRACK: &str = "screen";

/// H.264's payload type in webrtc-rs' default video set, and the number
/// Cloudflare answers with. Same reasoning as [`OPUS_PAYLOAD_TYPE`].
const H264_PAYLOAD_TYPE: PayloadType = 102;

/// The clock RTP counts video in. Fixed at 90 kHz for every video codec.
const VIDEO_CLOCK_RATE: u32 = 90_000;

/// Opus' de-facto payload type, and the one Cloudflare answers with. Offering
/// the same number keeps the two sides from renumbering mid-negotiation
/// (DR-7).
const OPUS_PAYLOAD_TYPE: PayloadType = 111;

/// Set this to anything and a join prints how long each of its phases took.
///
/// Task 4.4 is a budget, and a budget without a breakdown is a number nobody
/// can act on: the phases below are three server round trips and an ICE
/// gathering, and which of them is slow is not guessable. Off unless asked,
/// because a call that narrates itself five lines at a time is a call that is
/// hard to read.
const TRACE_ENV: &str = "GOODVOICE_TRACE_JOIN";

/// How long ICE and DTLS get before the join is called a failure.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(25);

/// How long gathering may stay silent before the SDP is taken as it stands.
///
/// Waiting for `Complete` alone is waiting on every ICE URL the room handed
/// out, including the ones this network cannot reach — one of them is enough
/// to hang the join for the whole [`CONNECT_TIMEOUT`] (DR-14). A candidate
/// that is coming arrives long before this: a server-reflexive one is a single
/// round trip, and a relay is an allocation and an authentication, so a
/// straggler at three quarters of a second is a straggler that is not coming.
///
/// It is a floor on every join that has to pay it, which is why it is not
/// two seconds any more (DR-19).
const GATHER_QUIET: Duration = Duration::from_millis(750);

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

/// How often the speaking indicator is recomputed.
///
/// 50 ms rather than 100: the indicator draws a *level* now, not a yes or no,
/// and at ten frames a second a level moves in visible steps. A quiet room
/// still costs nothing — [`Shared::refresh_speaking`] pushes only when the
/// numbers change, and in a room where nobody is talking they are all zero.
const METER_INTERVAL: Duration = Duration::from_millis(50);

/// Below this, a level is drawn as silence anyway, so it is left out of the
/// push entirely. What makes a quiet room free: every level under this rounds
/// to the same empty list, which compares equal to the last one.
const AUDIBLE_LEVEL: f32 = 0.004;

/// How finely a level is reported: 1/255 of full scale.
///
/// Quantised for one reason — a floating-point level jitters in its last bits
/// even from a still microphone, and an unquantised comparison would call that
/// "changed" and push it, fifty times a second, forever.
const LEVEL_STEPS: f32 = 255.0;

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
    /// How transmission is gated to begin with. Carried in rather than
    /// defaulted because the setting outlives any one call: the UI restores it
    /// from disk and hands it over, so a client that joins in push-to-talk
    /// never has a hot microphone for the first frame.
    pub mode: TransmitMode,
    /// The audio settings, shared with the capture path that was opened before
    /// this call existed and will outlive it. The same `Arc` the app holds, so
    /// a slider moved between two calls is still where the user left it.
    pub prefs: Arc<AudioPrefs>,
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
    let phase = Instant::now();
    let joined = signaling.join(&options.name).await?;
    trace_phase("room", phase.elapsed());

    // The roster socket and the peer connection both hang off the join
    // response and neither waits on the other, so they are opened together.
    // Serially it is two TLS handshakes and an ICE gathering end to end, and
    // every one of them is a person watching a window that says nothing
    // (task 4.4, DR-19).
    let (connected, established) = tokio::join!(
        signaling.connect(&joined.self_id),
        establish(signaling, &joined)
    );

    trace_phase("connected", phase.elapsed());

    match (connected, established) {
        (Ok((commands, inbound)), Ok((peer, signals, published))) => Ok(Session {
            self_id: joined.self_id,
            participants: joined.participants,
            peer,
            signals,
            published,
            commands,
            inbound,
        }),
        // The seat is handed back through whichever half came up, so a failure
        // on one side does not leave a ghost in the room (DR-5).
        (Ok((commands, _)), Err(error)) => {
            let _ = commands.send(ClientMessage::Leave).await;
            Err(error)
        }
        (Err(error), _) => Err(error),
    }
}

/// Builds the transport and gets the microphone onto it.
async fn establish(
    signaling: &Signaling,
    joined: &JoinResponse,
) -> Result<(Arc<dyn PeerConnection>, Signals, Published), RtcError> {
    let (events, signals) = Events::new();
    let peer: Arc<dyn PeerConnection> = Arc::new(open_peer(&joined.sfu.ice_servers, events).await?);

    let phase = Instant::now();
    let published = publish_mic(signaling, &joined.self_id, peer.as_ref(), &signals).await?;
    trace_phase("published", phase.elapsed());
    wait_for_connection(&signals).await?;
    trace_phase("connection", phase.elapsed());

    Ok((peer, signals, published))
}

// --- what survives a reconnect ---------------------------------------------

/// Everything a call keeps across the sessions it runs on.
///
/// A session is disposable — new participant id, new Realtime session, new peer
/// connection. What the user thinks of as "the call" is this: the room they are
/// in, whether they are muted, and what the UI is being told. Held in one place
/// so a rejoin can restore it rather than reset it.
/// One person and how loud they are right now, as the window draws them.
///
/// A level rather than a flag. The flag it replaces was true or false at
/// `mixer::SPEAKING_LEVEL` and nowhere in between, which is a light switch —
/// and a light switch cannot show the difference between someone talking and
/// someone breathing near a sensitive microphone.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Talker {
    /// The participant id, as the roster spells it.
    pub id: String,
    /// `0.0`–`1.0`, quantised to [`LEVEL_STEPS`].
    pub level: f32,
}

impl Talker {
    /// A talker, or `None` for a level nobody would see.
    fn new(id: &str, level: f32) -> Option<Self> {
        let level = quantise(level);
        if level == 0.0 {
            return None;
        }
        Some(Self {
            id: id.to_owned(),
            level,
        })
    }
}

/// A level to 1/255, or exactly zero for anything under [`AUDIBLE_LEVEL`].
///
/// Both halves earn their place. Quantising stops a still microphone's last
/// floating-point bits reading as a change fifty times a second; the floor
/// stops a room where nobody is talking from pushing anything at all, because
/// every level in it collapses to the same zero.
fn quantise(level: f32) -> f32 {
    // NaN spelled out rather than relying on a negated comparison: a level
    // that is not a number must read as silence, not as a dot nobody can
    // switch off.
    if level.is_nan() || level < AUDIBLE_LEVEL {
        return 0.0;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped into 0.0..=1.0 on the line below"
    )]
    let steps = (level.clamp(0.0, 1.0) * LEVEL_STEPS).round() as u8;
    f32::from(steps) / LEVEL_STEPS
}

/// Everything the window's meters are drawn from, in one push.
///
/// Two numbers that move together and are read together. Splitting them into
/// two channels would mean two wakeups and two events per tick for a window
/// that redraws once.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Levels {
    /// Everyone audible right now, this client included.
    pub talking: Vec<Talker>,
    /// The microphone *before* mute and before the gate — the only meter that
    /// still moves while a threshold is holding transmission shut, which is
    /// exactly when somebody is looking at it.
    pub input: f32,
}

struct Shared {
    muted: AtomicBool,
    deafened: AtomicBool,
    /// How transmission is gated (task 3.3). Mute is "never"; this is "right
    /// now". One byte because the publish loop reads it every frame — see
    /// [`TransmitMode::code`].
    transmit: AtomicU8,
    /// Whether the talk key is down. Only consulted under
    /// [`TransmitMode::PushToTalk`].
    talk_key: AtomicBool,
    /// Whether the last frame reached the wire. Written by the publish loop,
    /// which is the only place the gate's verdict exists, and read by the
    /// speaking indicator — a key that is up must stop the light immediately
    /// rather than at the speed the meter decays.
    gate_open: AtomicBool,
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
    speaking: watch::Sender<Levels>,
    /// How loud the microphone is *before* the gate has its say. The roster
    /// meter follows what the room hears; this one follows what the machine
    /// hears, which is the only thing worth showing to somebody setting a
    /// threshold.
    input: Meter,
    /// The settings the user can move mid-call. Shared with the capture path,
    /// which reconfigures WebRTC when the generation moves.
    prefs: Arc<AudioPrefs>,
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
    /// What the user wants shared, which outlives the session sharing it. A
    /// reconnect re-opens the capture from this rather than dropping the share
    /// (`rtc::screen`).
    share_intent: Mutex<Option<Arc<dyn ScreenSourceFactory>>>,
    /// What is actually happening, for the window.
    share: watch::Sender<ShareState>,
    /// Rung when [`Self::share_intent`] changes. `notify_one` for the same
    /// reason as [`Self::lost`]: a change that lands between two polls of the
    /// session loop has to be kept.
    share_changed: Notify,
    /// Where a remote screen goes, if anybody is watching one. `None` is the
    /// ordinary state, and it is what stops this client subscribing to video
    /// at all (prd.md §3 F3: viewers opt in).
    watch_sink: Mutex<Option<WatchSink>>,
    /// Rung when [`Self::watch_sink`] changes.
    watch_changed: Notify,
    /// The current session's transport, for [`Call::wire`] to count bytes on.
    ///
    /// `Weak`, because a measurement must not be what keeps a peer connection
    /// alive: the supervisor closes one on its way out of every session, and a
    /// harness holding a strong reference would leave the old one running
    /// beside the new one and count both.
    peer: Mutex<Option<Weak<dyn PeerConnection>>>,
}

/// A viewer's sink, and which viewer it belongs to.
struct WatchSink {
    /// Names one opening of the viewer window, process-wide and forever
    /// increasing. See [`Shared::clear_watch_sink`] for what it is for.
    generation: u64,
    sink: Arc<dyn ScreenSink>,
}

/// Hands out [`WatchSink::generation`]. Static rather than per-call: a
/// generation that restarted with each call could name a viewer from the
/// previous one.
static NEXT_WATCH: AtomicU64 = AtomicU64::new(0);

impl Shared {
    fn new(
        self_id: String,
        participants: Vec<Participant>,
        sink: Arc<dyn AudioSink>,
        mode: TransmitMode,
        prefs: Arc<AudioPrefs>,
    ) -> Arc<Self> {
        Arc::new(Self {
            muted: AtomicBool::new(false),
            deafened: AtomicBool::new(false),
            transmit: AtomicU8::new(mode.code()),
            talk_key: AtomicBool::new(false),
            gate_open: AtomicBool::new(true),
            leaving: AtomicBool::new(false),
            commands: Mutex::new(None),
            state: watch::Sender::new(CallState::Live),
            self_id: watch::Sender::new(self_id),
            roster: watch::Sender::new(participants),
            speaking: watch::Sender::new(Levels::default()),
            sink,
            slots: Mutex::new(HashMap::new()),
            microphone: Meter::new(),
            input: Meter::new(),
            prefs,
            lost: Notify::new(),
            share_intent: Mutex::new(None),
            share: watch::Sender::new(ShareState::Idle),
            share_changed: Notify::new(),
            watch_sink: Mutex::new(None),
            watch_changed: Notify::new(),
            peer: Mutex::new(None),
        })
    }

    fn is_leaving(&self) -> bool {
        self.leaving.load(Ordering::Relaxed)
    }

    /// What the user wants shared, if anything.
    fn share_intent(&self) -> Option<Arc<dyn ScreenSourceFactory>> {
        self.share_intent.lock().ok().and_then(|held| held.clone())
    }

    /// Record what the user wants and wake the session to act on it.
    fn set_share_intent(&self, factory: Option<Arc<dyn ScreenSourceFactory>>) {
        if let Ok(mut held) = self.share_intent.lock() {
            *held = factory;
        }
        // A call with no session is a call reconnecting; the next one reads
        // the intent when it starts, so the missed wake costs nothing.
        self.share_changed.notify_one();
    }

    /// Where a remote screen should go, if anywhere.
    fn watch_sink(&self) -> Option<Arc<dyn ScreenSink>> {
        self.watch_sink
            .lock()
            .ok()
            .and_then(|held| held.as_ref().map(|current| Arc::clone(&current.sink)))
    }

    /// A viewer opened. Same wake-up contract as the share intent.
    ///
    /// The generation it answers with is how the caller says *this* viewer
    /// later, when it closes: see [`Self::clear_watch_sink`].
    fn set_watch_sink(&self, sink: Arc<dyn ScreenSink>) -> u64 {
        let generation = NEXT_WATCH.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut held) = self.watch_sink.lock() {
            *held = Some(WatchSink { generation, sink });
        }
        self.watch_changed.notify_one();
        generation
    }

    /// A viewer closed — but only if it is still the one being fed.
    ///
    /// A window's closing is noticed asynchronously (`crate::viewer_closed`)
    /// and a person can open the next viewer before that lands. Unconditional,
    /// this would then take the *new* window's sink away and leave a viewer
    /// that never gets a picture; there is nothing in the window to notice
    /// that and nothing to recover it.
    fn clear_watch_sink(&self, generation: u64) {
        match self.watch_sink.lock() {
            Ok(mut held)
                if held
                    .as_ref()
                    .is_some_and(|current| current.generation == generation) =>
            {
                *held = None;
            }
            // Somebody else's sink, or a poisoned lock: either way there is
            // nothing here to wake the session about.
            _ => return,
        }
        self.watch_changed.notify_one();
    }

    /// Whether the user has left the microphone on at all. What reaches the
    /// wire is this *and* whatever [`Gate`] makes of the frame.
    fn is_transmitting(&self) -> bool {
        !self.muted.load(Ordering::Relaxed)
    }

    fn transmit_mode(&self) -> TransmitMode {
        TransmitMode::from_code(self.transmit.load(Ordering::Relaxed))
    }

    fn talk_key_down(&self) -> bool {
        self.talk_key.load(Ordering::Relaxed)
    }

    /// The playback slot a participant is being played in, if any.
    fn slot_of(&self, participant: &str) -> Option<usize> {
        self.slots.lock().ok()?.get(participant).copied()
    }

    /// Rebuilds how loud everybody is, and pushes it only if it moved.
    ///
    /// Sorted so two identical readings compare equal: the point of the
    /// comparison is to leave a quiet room pushing nothing at all.
    fn refresh_speaking(&self) {
        let mut talking: Vec<Talker> = self
            .slots
            .lock()
            .map(|slots| {
                slots
                    .iter()
                    .filter_map(|(id, &slot)| Talker::new(id, self.sink.level(slot)))
                    .collect()
            })
            .unwrap_or_default();

        // Muted is not talking, whatever the microphone says — and neither is
        // a push-to-talk key that is up or a voice gate that never opened. The
        // meter behind this one already reads zero in all three cases; this is
        // what makes it true on the very frame the key comes up, rather than
        // as fast as a meter can fall.
        if self.is_transmitting() && self.gate_open.load(Ordering::Relaxed) {
            talking.extend(Talker::new(
                self.self_id.borrow().as_str(),
                self.microphone.level(),
            ));
        }
        talking.sort_unstable_by(|left, right| left.id.cmp(&right.id));

        let levels = Levels {
            talking,
            input: quantise(self.input.level()),
        };
        if *self.speaking.borrow() != levels {
            self.speaking.send_replace(levels);
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
        if *self.speaking.borrow() != Levels::default() {
            self.speaking.send_replace(Levels::default());
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
        if let Ok(mut peer) = self.peer.lock() {
            *peer = Some(Arc::downgrade(&session.peer));
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
        if let Ok(mut peer) = self.peer.lock() {
            *peer = None;
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
    /// The room this call is in. A `Call` outlives the window that asked for
    /// it now (task 4.6), so it has to be able to say where it is rather than
    /// relying on whoever joined to still be around and remember.
    room: String,
    roster: watch::Receiver<Vec<Participant>>,
    state: watch::Receiver<CallState>,
    self_id: watch::Receiver<String>,
    speaking: watch::Receiver<Levels>,
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
                    crate::note!("call", "join attempt {attempt} failed ({error}); retrying");
                    last = error;
                    tokio::time::sleep(JOIN_BACKOFF * attempt).await;
                }
                Err(error) => {
                    // The last attempt, or one not worth repeating. Reported
                    // here rather than at the command boundary because this is
                    // the only place that still knows *which* of the four ways
                    // a join fails this was.
                    report_join_failure(&error, attempt);
                    return Err(error);
                }
            }
        }
        report_join_failure(&last, JOIN_ATTEMPTS);
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
        let room = options.room.clone();
        let shared = Shared::new(
            session.self_id.clone(),
            session.participants.clone(),
            sink,
            options.mode,
            Arc::clone(&options.prefs),
        );
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
            room,
            roster,
            state,
            self_id,
            speaking,
            supervisor: Some(supervisor),
            tasks,
        }
    }

    /// The room this call is in.
    #[must_use]
    pub fn room(&self) -> &str {
        &self.room
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
    pub fn speaking(&self) -> watch::Receiver<Levels> {
        self.speaking.clone()
    }

    /// How loud the microphone is, before mute or the gate. What a threshold
    /// is set against.
    #[must_use]
    pub fn input_level(&self) -> f32 {
        self.shared.input.level()
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

    /// Start sharing a screen.
    ///
    /// Returns as soon as the intent is recorded, not once the share is live:
    /// opening a capture and renegotiating with the SFU takes a moment, and
    /// the answer — including a refusal, such as somebody else already sharing
    /// — arrives on [`Self::share`]. Nothing about the voice path waits on
    /// this (prd.md §3 F3).
    ///
    /// The factory rather than a capture, because the share is restarted with
    /// every session: see [`super::screen`].
    pub fn start_share(&self, factory: Arc<dyn ScreenSourceFactory>) {
        self.shared.set_share_intent(Some(factory));
    }

    /// Stop sharing. Does nothing if this client is not sharing.
    pub fn stop_share(&self) {
        self.shared.set_share_intent(None);
    }

    /// Start receiving the room's screen, into `sink`.
    ///
    /// The only thing that subscribes this client to video: a call with no
    /// viewer open pulls nothing, whatever anybody else is sharing (prd.md §3
    /// F3). Calling it again swaps the sink — the frames follow, from the next
    /// access unit — which is what a second viewer window needs.
    ///
    /// The generation it returns names this viewer to [`Self::unwatch_screen`].
    pub fn watch_screen(&self, sink: Arc<dyn ScreenSink>) -> u64 {
        self.shared.set_watch_sink(sink)
    }

    /// Stop receiving the room's screen and drop the subscription.
    ///
    /// Does nothing if a later viewer has taken over since `generation` was
    /// handed out.
    pub fn unwatch_screen(&self, generation: u64) {
        self.shared.clear_watch_sink(generation);
    }

    /// What this client's own share is doing.
    #[must_use]
    pub fn share(&self) -> watch::Receiver<ShareState> {
        self.shared.share.subscribe()
    }

    /// Whether this client is publishing a screen right now.
    #[must_use]
    pub fn is_sharing(&self) -> bool {
        self.shared.share.borrow().is_sharing()
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

    /// Chooses how transmission is gated: always, on a held key, or on a
    /// detected voice.
    ///
    /// Not told to the room. Mute is a state peers see because it explains a
    /// silence they would otherwise wonder about; a talk key that is up
    /// explains nothing, and a roster that flickered with every syllable would
    /// be worse than one that says nothing at all.
    pub fn set_transmit_mode(&self, mode: TransmitMode) {
        self.shared.transmit.store(mode.code(), Ordering::Relaxed);
    }

    #[must_use]
    pub fn transmit_mode(&self) -> TransmitMode {
        self.shared.transmit_mode()
    }

    /// Reports the talk key going down or coming up.
    ///
    /// Only [`TransmitMode::PushToTalk`] reads it, but it is recorded in every
    /// mode: a key held across a switch into push-to-talk should already be
    /// down when the first frame arrives.
    pub fn set_talk_key(&self, down: bool) {
        self.shared.talk_key.store(down, Ordering::Relaxed);
    }

    /// What this call's transport has carried since the current session opened.
    ///
    /// Counted inside webrtc, at the point every datagram arrives and before
    /// anything decides what it is — so a track this client has stopped
    /// reading still shows up. That is the point of it: closing the viewer
    /// stops the read (`reconcile_watch`), so every instrument above the
    /// socket reports silence whether or not Cloudflare is still sending
    /// (plan.md §7.9). See [`super::wire`].
    ///
    /// `None` between sessions — while a reconnect is in flight there is no
    /// transport to count. The counters restart with each session, so a
    /// snapshot is only comparable with another from the same one;
    /// [`Transport::since`](super::wire::Transport::since) saturates rather
    /// than wrapping when they are not.
    pub async fn wire(&self) -> Option<Wire> {
        let peer = self
            .shared
            .peer
            .lock()
            .ok()
            .and_then(|held| held.as_ref().and_then(Weak::upgrade))?;

        Some(Wire::read(
            &peer.get_stats(Instant::now(), StatsSelector::None).await,
        ))
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
            crate::note!("call", "call dropped ({detail}); reconnecting");
        }

        match reconnect(&supervisor, &mut backoff).await {
            Ok(next) => {
                session = next;
                backoff.reset();
            }
            Err(reason) => {
                // The retry schedule ran out. `EndReason::Left` never reaches
                // here — leaving is not a failure — so both arms of this are
                // worth an issue.
                let (kind, detail) = match &reason {
                    EndReason::Refused { detail } => ("refused", detail.clone()),
                    EndReason::Unreachable { detail } => ("unreachable", detail.clone()),
                    EndReason::Left => ("left", String::new()),
                };
                if !matches!(reason, EndReason::Left) {
                    crate::report::failure(
                        "call-ended",
                        &format!("the call ended {kind}: {detail}"),
                        &[("end_reason", kind.to_owned())],
                    );
                }
                supervisor.shared.finish(reason);
                return;
            }
        }
    }
}

/// Reports a join that will not be retried.
///
/// The tag is the *variant*, not the message: `JoinRejected`, `Sfu`,
/// `NotConnected` and `Http` are four different problems — a full room, an SFU
/// that refused the track, a handshake that never completed, and a Worker that
/// is not there — and they read as one failure on the window's red line. Which
/// one it was is the whole of what makes the issue actionable.
fn report_join_failure(error: &RtcError, attempts: u32) {
    // Named one by one rather than with a wildcard: a variant added later
    // should fail to compile here and be given a name, not arrive as "other"
    // and be invisible in the issue list.
    let kind = match error {
        RtcError::JoinRejected { .. } => "join_rejected",
        RtcError::NotConnected => "not_connected",
        RtcError::Http(_) => "http",
        RtcError::Sfu(_) => "sfu",
        RtcError::Protocol(_) => "protocol",
        RtcError::Transport(_) => "transport",
        RtcError::Audio(_) => "audio",
    };
    let mut tags = vec![("join_error", kind.to_owned())];
    // The room's own code for it — `room_full`, `bad_request` — which is a
    // finer answer than `join_rejected` and the one that says whether this is
    // a bug at all.
    if let RtcError::JoinRejected {
        code: Some(code), ..
    } = error
    {
        tags.push(("room_code", code.clone()));
    }
    crate::report::failure(
        "join-failed",
        &format!("the join gave up after {attempts}: {error}"),
        &tags,
    );
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

/// Lets go of everything this seat was carrying.
///
/// Called on the way out of [`run_session`], however it ended. Playback tasks
/// stop before the next session hands their slots to somebody else, and the
/// capture goes with the seat — the *intent* does not, which is what makes a
/// share survive a reconnect (see [`reconcile_share`]).
fn stop_media(
    subscriber: &Subscriber,
    playing: &mut HashMap<String, Subscription>,
    sharing: Option<Sharing>,
    watching: Option<Watching>,
) {
    for (_, subscription) in playing.drain() {
        subscription.playback.abort();
        subscriber.shared.sink.clear(subscription.slot);
    }
    if let Some(sharing) = sharing {
        sharing.stop();
    }
    if let Some(watching) = watching {
        watching.playback.abort();
        if let Some(sink) = subscriber.shared.watch_sink() {
            sink.ended();
        }
    }
    subscriber.shared.silence();
}

/// The screen this session is publishing, if it is publishing one.
struct Sharing {
    task: JoinHandle<()>,
}

impl Sharing {
    fn stop(self) {
        self.task.abort();
    }
}

/// Bring what is being published in line with what the user asked for.
///
/// Called when the intent changes and once when a session starts, which is
/// what carries a share across a reconnect: the new session re-opens the
/// capture rather than the old one's encoder being expected to survive
/// (`rtc::screen`).
async fn reconcile_share(
    subscriber: &Subscriber,
    sharing: &mut Option<Sharing>,
    commands: &mpsc::Sender<ClientMessage>,
) {
    let wanted = subscriber.shared.share_intent();

    // Already in the right state. A share whose task has finished on its own —
    // the window closed — is not: it is dropped here and reported.
    if let Some(current) = sharing.as_ref() {
        if wanted.is_some() && !current.task.is_finished() {
            return;
        }
        // Finished while the user still wants it: the capture or the track
        // under it gave out and the re-open below is a restart, not a start.
        // Worth an issue of its own even though `capture::share` reports the
        // encoder error directly — this is the one that says the share is
        // *flapping*, and it is also the only report on the paths that end
        // without a `CaptureError` at all (the screen track going away).
        if wanted.is_some() && current.task.is_finished() {
            crate::report::failure(
                "share-restarted",
                "the share stopped on its own while it was still wanted",
                &[("share_stage", "running".to_owned())],
            );
        }
        if let Some(current) = sharing.take() {
            current.stop();
        }
        let _ = commands.send(ClientMessage::Share { sharing: false }).await;
        if wanted.is_none() {
            subscriber.shared.share.send_replace(ShareState::Idle);
            return;
        }
    }

    let Some(factory) = wanted else {
        return;
    };

    let source = match factory.open() {
        Ok(source) => source,
        Err(detail) => {
            // The capture itself refused — the window closed between the
            // picker and the share, or there is no encoder. Not worth
            // retrying on a timer: clear the intent so the user is asked
            // again rather than being reconnected into a failure.
            subscriber.shared.set_share_intent(None);
            // The window shows this in red and the client carries on, so
            // nothing else would ever say it happened. `where` separates the
            // capture refusing from the room refusing, which are different
            // bugs with the same red line.
            crate::report::failure(
                "share-failed",
                &format!("the capture would not open: {detail}"),
                &[("share_stage", "capture".to_owned())],
            );
            subscriber
                .shared
                .share
                .send_replace(ShareState::Failed { detail });
            return;
        }
    };

    let published = match publish_screen(
        &subscriber.signaling,
        &subscriber.self_id,
        subscriber.peer.as_ref(),
        &subscriber.signals,
        &subscriber.published,
    )
    .await
    {
        Ok(published) => published,
        Err(error) => {
            // `already_sharing` lands here, and it is the one refusal that is
            // about the room rather than this client (prd.md §8).
            subscriber.shared.set_share_intent(None);
            crate::report::failure(
                "share-failed",
                &format!("the room would not take the screen: {error}"),
                &[("share_stage", "publish".to_owned())],
            );
            subscriber.shared.share.send_replace(ShareState::Failed {
                detail: error.to_string(),
            });
            return;
        }
    };

    let (width, height) = source.size();
    let hardware = source.is_hardware();
    // A viewer subscribes as soon as the roster shows the share, and cannot
    // decode until a keyframe. Asking now costs one frame and saves everyone
    // watching the encoder's own interval.
    source.request_keyframe();

    *sharing = Some(Sharing {
        task: tokio::spawn(screen_loop(
            source,
            published,
            Arc::clone(&subscriber.shared),
        )),
    });
    let _ = commands.send(ClientMessage::Share { sharing: true }).await;
    subscriber.shared.share.send_replace(ShareState::Sharing {
        target: factory.describe(),
        width,
        height,
        hardware,
    });
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

    // A share the user started on the previous session is restarted on this
    // one, here — which is the whole of what "the share survives a reconnect"
    // means (`rtc::screen`). A no-op when nothing is being shared.
    let mut sharing: Option<Sharing> = None;
    reconcile_share(&subscriber, &mut sharing, &session.commands).await;

    // And the other direction: a viewer left open across a reconnect
    // resubscribes here.
    let mut watching: Option<Watching> = None;
    reconcile_watch(&subscriber, &mut watching, &latest).await;

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
                        // Somebody started or stopped sharing, or the sharer
                        // reconnected under a new session id.
                        reconcile_watch(&subscriber, &mut watching, &latest).await;
                    }
                    ServerMessage::Error { code, message } => {
                        crate::note!("call", "room error: {message} ({code})");
                    }
                }
            }
            _ = retry.tick() => {
                // The roster is pushed only when it changes, so a subscription
                // that failed would otherwise leave that speaker silent for the
                // rest of the call. A no-op when everyone is already subscribed.
                reconcile(&subscriber, &mut playing, &latest).await;
                reconcile_watch(&subscriber, &mut watching, &latest).await;
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
            () = subscriber.shared.watch_changed.notified() => {
                // A viewer opened or closed. This is the only thing that ever
                // subscribes this client to video.
                reconcile_watch(&subscriber, &mut watching, &latest).await;
            }
            () = subscriber.shared.share_changed.notified() => {
                // The user started or stopped sharing. Never a reason to end
                // the session: a share that fails is a share that failed, and
                // the call goes on without it (prd.md §3 F3: audio is never
                // gated on video).
                reconcile_share(&subscriber, &mut sharing, &session.commands).await;
            }
        }
    };

    stop_media(&subscriber, &mut playing, sharing.take(), watching.take());

    // Hand the seat back on the way out. A drop does not always mean the room
    // is unreachable — a dead sender or a stalled ICE leaves the WebSocket
    // working — and a seat nobody gave back is what makes the *next* join get
    // refused by a room that is full of this client's own ghosts (DR-5).
    let _ = session.commands.send(ClientMessage::Leave).await;
    let _ = subscriber.peer.close().await;

    end
}

/// Reports one phase of a join, if [`TRACE_ENV`] asked for it.
///
/// The times are cumulative within a phase group — "how far into the join is
/// this" — because that is the question a breakdown is read to answer.
fn trace_phase(phase: &str, elapsed: Duration) {
    static TRACING: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *TRACING.get_or_init(|| std::env::var(TRACE_ENV).is_ok()) {
        println!("join {phase} at {} ms", elapsed.as_millis());
    }
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
    /// Whether gathering has already been waited out once.
    ///
    /// Nothing here restarts ICE, so a candidate list that has gone quiet
    /// stays quiet — and every renegotiation after the first would otherwise
    /// pay [`GATHER_QUIET`] again for an answer that cannot change (DR-19).
    settled: Arc<AtomicBool>,
    /// The local candidates gathered so far. Watched rather than counted so a
    /// candidate arriving wakes the waiter: most of a join's ICE time is spent
    /// waiting for one more, and the rule for "enough" is about *which* ones
    /// have arrived rather than how many (DR-19).
    candidates: watch::Receiver<Gathered>,
    connection: watch::Receiver<RTCPeerConnectionState>,
    tracks: Arc<TrackInbox>,
}

/// What ICE has produced so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Gathered {
    count: usize,
    /// A server-reflexive candidate: this client knows its own public address,
    /// so a direct path to an ice-lite SFU exists.
    srflx: bool,
    /// A relay candidate: there is a way through even when the direct path is
    /// blocked.
    relay: bool,
}

impl Gathered {
    fn add(&mut self, kind: RTCIceCandidateType) {
        self.count += 1;
        match kind {
            RTCIceCandidateType::Srflx => self.srflx = true,
            RTCIceCandidateType::Relay => self.relay = true,
            _ => {}
        }
    }

    /// Whether anything left to gather is worth waiting for.
    ///
    /// One direct path and one fallback is all ICE has to choose between.
    /// Cloudflare hands out six TURN URLs — one relay on six ports, so a
    /// firewall that blocks one lets another through — and allocating on all of
    /// them takes about a second longer than allocating on the first. Those are
    /// more fallbacks of a kind already in hand, and only one is ever used
    /// (DR-19).
    const fn enough(self) -> bool {
        self.srflx && self.relay
    }
}

/// Remote tracks that have arrived but not yet been claimed by a subscription.
#[derive(Default)]
struct TrackInbox {
    waiting: Mutex<Vec<Arc<dyn TrackRemote>>>,
    arrived: Notify,
}

struct Events {
    gathering: watch::Sender<RTCIceGatheringState>,
    candidates: watch::Sender<Gathered>,
    connection: watch::Sender<RTCPeerConnectionState>,
    tracks: Arc<TrackInbox>,
}

impl Events {
    fn new() -> (Arc<Self>, Signals) {
        let (gathering_tx, gathering) = watch::channel(RTCIceGatheringState::New);
        let (connection_tx, connection) = watch::channel(RTCPeerConnectionState::New);
        let (candidates_tx, candidates) = watch::channel(Gathered::default());
        let tracks = Arc::new(TrackInbox::default());

        (
            Arc::new(Self {
                gathering: gathering_tx,
                candidates: candidates_tx,
                connection: connection_tx,
                tracks: Arc::clone(&tracks),
            }),
            Signals {
                gathering,
                candidates,
                settled: Arc::new(AtomicBool::new(false)),
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

    /// Nothing here trickles, so a candidate matters twice: as evidence that
    /// gathering is still making progress, and as one of the two kinds that
    /// make waiting for the rest pointless (see [`Gathered::enough`]).
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        self.candidates
            .send_modify(|gathered| gathered.add(event.candidate.typ));
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
/// The video codec, which is H.264 and only H.264 (prd.md §7: universal
/// hardware decode is the whole reason).
///
/// `packetization-mode=1` is what lets one access unit be fragmented across
/// RTP packets, which a 1080p keyframe always needs.
/// `level-asymmetry-allowed=1` matters because the level travels with the
/// resolution: a 720p share and a 1080p share are the same profile at
/// different levels, and without this the two ends would have to renegotiate
/// when the user changes quality.
fn h264_codec() -> RTCRtpCodec {
    RTCRtpCodec {
        mime_type: MIME_TYPE_H264.to_owned(),
        clock_rate: VIDEO_CLOCK_RATE,
        channels: 0,
        sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
            .to_owned(),
        rtcp_feedback: vec![],
    }
}

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
    // Registered whether or not this client will ever share: the codec has to
    // be in the engine before a transceiver can be added to a live peer
    // connection, and a share starts long after this.
    media_engine.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: h264_codec(),
            payload_type: H264_PAYLOAD_TYPE,
        },
        RtpCodecKind::Video,
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

/// Wait until the candidate list is as complete as it is going to get.
///
/// There are two ways out, and the tidy one is not the reliable one.
///
/// `Complete` is published once webrtc-rs has heard back from every STUN and
/// TURN client it opened. A STUN server that never answers is never dropped
/// from that set, so a single unreachable URL in the room's ICE list means the
/// state stays `New` forever — and Cloudflare hands out
/// `stun.cloudflare.com:53`, which any network that filters outbound UDP/53
/// swallows (DR-14).
///
/// So quiet counts too: no new candidate for [`GATHER_QUIET`] with at least
/// one already in hand. Leaving early costs only the `a=end-of-candidates`
/// line, since the candidates themselves are re-read out of the ICE agent
/// every time the local description is asked for — and against an ice-lite SFU
/// that runs no checks of its own (DR-7), the candidates that matter are ours
/// to send from, not theirs to choose between.
async fn wait_for_gathering(signals: &Signals) -> Result<(), RtcError> {
    let phase = Instant::now();
    let gathered = gather(signals).await;
    trace_phase("gather", phase.elapsed());
    gathered
}

async fn gather(signals: &Signals) -> Result<(), RtcError> {
    if signals.settled.load(Ordering::Acquire) {
        return Ok(());
    }

    let mut gathering = signals.gathering.clone();
    let mut candidates = signals.candidates.clone();
    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;

    loop {
        // Both are copied out of their watches straight away: the borrows they
        // hand back are not `Send`, and this runs inside a spawned task.
        let state = *gathering.borrow_and_update();
        let gathered = *candidates.borrow_and_update();
        if state == RTCIceGatheringState::Complete || gathered.enough() {
            signals.settled.store(true, Ordering::Release);
            return Ok(());
        }

        let moved = tokio::time::timeout(GATHER_QUIET, async {
            tokio::select! {
                changed = gathering.changed() => changed,
                changed = candidates.changed() => changed,
            }
        })
        .await;

        match moved {
            Ok(Ok(())) => continue,
            Ok(Err(_)) => {
                return Err(RtcError::Transport(
                    "the peer connection went away during ICE gathering".to_owned(),
                ))
            }
            // Nothing has moved for a whole window: what is in hand is what
            // there is.
            Err(_) => {
                if gathered.count > 0 {
                    signals.settled.store(true, Ordering::Release);
                    return Ok(());
                }
            }
        }

        if tokio::time::Instant::now() >= deadline {
            return Err(RtcError::Transport(
                "ICE gathering never completed".to_owned(),
            ));
        }
    }
}

/// Cloudflare negotiates without trickle, so the SDP that goes up has to carry
/// every candidate already.
async fn local_sdp(peer: &dyn PeerConnection, signals: &Signals) -> Result<String, RtcError> {
    wait_for_gathering(signals).await?;

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
        self.write(packet, Duration::from_millis(u64::from(FRAME_MS)))
            .await
    }
}

/// A transport error as something a log can hold.
///
/// `write_sample` failing because the track is gone surfaces as a channel
/// `SendError`, and its `Display` embeds the whole RTP packet it could not
/// deliver — header, payload bytes and all. That is several hundred characters
/// of hex describing a condition with exactly one meaning, and it prints on
/// every leave, because the last frame races the teardown. Anything else is
/// passed through untouched.
fn one_line(error: impl std::fmt::Display) -> String {
    let text = error.to_string();
    if text.starts_with("SendError") {
        return "the track is closed".to_owned();
    }
    text
}

impl Published {
    /// Puts one sample on the wire, whatever kind of media it is.
    ///
    /// The duration is what the packetizer turns into an RTP timestamp step,
    /// so it is per call rather than per track: audio frames are all
    /// [`FRAME_MS`], and a screen's are as irregular as the screen is
    /// (DR-31).
    async fn write(&self, payload: &[u8], duration: Duration) -> Result<(), String> {
        self.track
            .sample_writer(
                self.ssrc.load(Ordering::Relaxed),
                self.payload_type.load(Ordering::Relaxed),
            )
            .write_sample(&Sample {
                data: Bytes::copy_from_slice(payload),
                duration,
                ..Default::default()
            })
            .await
            .map_err(one_line)
    }

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
    payload_type_of(peer, MIC_TRACK).await
}

/// The payload type Cloudflare agreed to for one of our outgoing tracks.
///
/// By name rather than by position: a client that is sharing has two senders,
/// and which one `get_senders` returns first is not something to rely on.
async fn payload_type_of(peer: &dyn PeerConnection, track_id: &str) -> Option<PayloadType> {
    for sender in peer.get_senders().await {
        if sender.track().track_id().await != track_id {
            continue;
        }
        return sender
            .get_parameters()
            .await
            .ok()?
            .rtp_parameters
            .codecs
            .first()
            .map(|codec| codec.payload_type);
    }
    None
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

/// Adds an H.264 track to a live peer connection and tells the room about it.
///
/// Unlike [`publish_mic`] this happens mid-call, on a peer connection that is
/// already carrying audio: adding a transceiver renegotiates, and Cloudflare
/// answers the same `tracks/new` it answers a join with. The audio sender can
/// be rebuilt by that exchange, which is what [`Published::refresh`] on the
/// microphone is for at the end.
///
/// **This is where the room's one-sharer rule lands.** The Durable Object
/// refuses a `tracks/new` naming `screen` while somebody else is sharing
/// (`already_sharing`, server/src/room.ts), so a second sharer fails here
/// rather than getting a track nobody can see.
async fn publish_screen(
    signaling: &Signaling,
    self_id: &str,
    peer: &dyn PeerConnection,
    signals: &Signals,
    microphone: &Published,
) -> Result<Published, RtcError> {
    let track = Arc::new(TrackLocalStaticSample::new(MediaStreamTrack::new(
        "goodvoice".to_owned(),
        SCREEN_TRACK.to_owned(),
        "goodvoice screen".to_owned(),
        RtpCodecKind::Video,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(fresh_ssrc()),
                ..Default::default()
            },
            codec: h264_codec(),
            ..Default::default()
        }],
    ))?);

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
    trace_sdp("screen offer", &sdp);

    let mid = transceiver
        .mid()
        .await?
        .ok_or_else(|| RtcError::Protocol("screen transceiver has no mid".to_owned()))?;

    let answer = signaling
        .sfu(
            self_id,
            SfuOperation::TracksNew,
            &json!({
                "sessionDescription": { "type": "offer", "sdp": sdp },
                "tracks": [{ "location": "local", "mid": mid, "trackName": SCREEN_TRACK }],
            }),
        )
        .await?;

    // A `tracks/new` can come back 200 with the track itself refused, which is
    // how `already_sharing` arrives when the room let the request through and
    // Cloudflare did not.
    if let Some((code, description)) = track_error(&answer) {
        return Err(RtcError::Sfu(format!("{description} ({code})")));
    }

    peer.set_remote_description(RTCSessionDescription::answer(sdp_of(&answer)?)?)
        .await?;

    // The exchange above may have rebuilt the microphone's sender underneath
    // the encode loop, which would otherwise keep writing to the old one
    // (DR-8). Same fix as a pull's renegotiation.
    microphone.refresh(peer).await;

    let payload_type = payload_type_of(peer, SCREEN_TRACK)
        .await
        .ok_or_else(|| RtcError::Transport("screen sender has no negotiated codec".to_owned()))?;
    let ssrc = *track
        .ssrcs()
        .await
        .first()
        .ok_or_else(|| RtcError::Transport("published screen has no SSRC".to_owned()))?;

    Ok(Published {
        track,
        ssrc: Arc::new(AtomicU32::new(ssrc)),
        payload_type: Arc::new(AtomicU8::new(payload_type)),
    })
}

/// Streams encoded frames from one capture onto one track.
///
/// Ends when the capture ends — the window closed, the user stopped, or the
/// session went away — and says which, so the UI can tell the difference
/// between "you stopped" and "it stopped".
async fn screen_loop(mut source: Box<dyn ScreenSource>, published: Published, shared: Arc<Shared>) {
    while let Some(frame) = source.next_frame().await {
        if shared.is_leaving() {
            break;
        }
        if let Err(detail) = published.write(&frame.bytes, frame.duration).await {
            // The track is gone. The session is what rebuilds it, and the
            // share is restarted with the session.
            crate::note!("share", "screen track stopped: {detail}");
            break;
        }
    }
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
    let mut gate = Gate::new();

    while let Some(frame) = source.next_frame().await {
        // Mute stops packets rather than sending silence: the room should cost
        // nothing while nobody is talking (prd.md §3 F1). Nothing is encoded
        // either — a frame nobody will send is work nobody asked for. The gate
        // answers the same question one frame at a time: push-to-talk and
        // voice activity both live there (task 3.3), and neither is consulted
        // while muted, so a muted client does not run a detector either.
        // Before the gate, and unconditionally: this is the only meter that
        // still moves while the gate is shut, which is the whole use of it —
        // somebody setting a threshold is by definition below one.
        let loudness = peak(&frame);
        shared.input.observe(loudness);

        let open = shared.is_transmitting()
            && gate.admits(
                shared.transmit_mode(),
                shared.talk_key_down(),
                &frame,
                shared.prefs.sensitivity(),
            );
        shared.gate_open.store(open, Ordering::Relaxed);
        if !open {
            // The meter follows what the room hears, so a client whose audio
            // is going nowhere reads as silent however loudly they are talking.
            shared.microphone.observe(0);
            continue;
        }
        // This is the only place the outgoing signal exists as samples, so it
        // is the only place the "you are talking" indicator can come from.
        shared.microphone.observe(loudness);
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
                // A client on its way out has no working track by definition,
                // so the first failure is the expected one rather than news.
                if failures == 1 && !shared.leaving.load(Ordering::Relaxed) {
                    crate::note!("audio", "microphone frame not sent: {error}");
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
            crate::note!("call", "no playback slot left for {}", peer.name);
            continue;
        };

        match subscribe_to(subscriber, session_id, slot).await {
            Ok(subscription) => {
                playing.insert(peer.id.clone(), subscription);
            }
            Err(error) => crate::note!("call", "could not subscribe to {}: {error}", peer.name),
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

/// The remote screen this client is watching, if it is watching one.
struct Watching {
    /// Who is sharing. A different sharer means a different subscription.
    peer: String,
    /// Their Realtime session, which changes when they reconnect.
    session_id: String,
    /// The m-section Cloudflare put the pull on. What `tracks/close` names, and
    /// the only handle on this subscription the SFU recognises — `None` if the
    /// answer did not carry one, which leaves nothing to close.
    mid: Option<String>,
    playback: JoinHandle<()>,
}

/// Subscribe to the room's screen, or stop, to match what the viewer wants.
///
/// **Nothing happens here without a sink.** No viewer open means no
/// subscription, which means Cloudflare never sends this client the video —
/// prd.md §3 F3's opt-in, enforced by the absence of a destination rather than
/// by a flag (`rtc::screen`).
async fn reconcile_watch(
    subscriber: &Subscriber,
    watching: &mut Option<Watching>,
    participants: &[Participant],
) {
    let sink = subscriber.shared.watch_sink();

    // Who, if anyone, is publishing a screen right now. Never this client:
    // watching your own share is a loopback nobody asked for.
    let sharer = sink.as_ref().and_then(|_| {
        participants.iter().find(|peer| {
            peer.id != subscriber.self_id
                && peer.publishes(SCREEN_TRACK)
                && peer.session_id.is_some()
        })
    });

    // Still watching the same screen on the same session: nothing to do.
    if let (Some(current), Some(sharer)) = (watching.as_ref(), sharer) {
        if current.peer == sharer.id
            && Some(current.session_id.as_str()) == sharer.session_id.as_deref()
            && !current.playback.is_finished()
        {
            return;
        }
    }

    if let Some(current) = watching.take() {
        current.playback.abort();
        if let Some(sink) = sink.as_ref() {
            sink.ended();
        }
        // Aborting the playback stops this client *reading* the screen and
        // nothing else. Measured on 2026-08-27 (§7.9, docs/testing/viewer.md):
        // 62.7 of 62.9 kB/s still arrived after the viewer closed, because
        // nothing had told Cloudflare to stop. This is what tells them.
        if let Some(mid) = current.mid.as_deref() {
            if let Err(error) = close_pull(subscriber, mid).await {
                // Worth reporting and not worth ending the call for: the worst
                // case is the bandwidth this used to spend unconditionally.
                crate::note!(
                    "share",
                    "could not close the screen pull on m-line {mid}: {error}"
                );
            }
        }
    }

    let (Some(_), Some(sharer)) = (sink, sharer) else {
        return;
    };
    let Some(session_id) = sharer.session_id.as_deref() else {
        return;
    };

    match subscribe_to_screen(subscriber, session_id).await {
        Ok((playback, mid)) => {
            *watching = Some(Watching {
                peer: sharer.id.clone(),
                session_id: session_id.to_owned(),
                mid,
                playback,
            });
        }
        // Worth reporting and not worth ending the call for: audio is never
        // gated on video (prd.md §3 F3). The retry tick calls back here.
        Err(error) => crate::note!("share", "could not watch {}'s screen: {error}", sharer.name),
    }
}

/// Pulls the room's `screen` track and starts feeding whatever viewer is open.
///
/// Hands back the m-section the pull landed on as well as the playback task:
/// closing this subscription later is addressed by mid and by nothing else
/// (see [`close_pull`]).
async fn subscribe_to_screen(
    subscriber: &Subscriber,
    session_id: &str,
) -> Result<(JoinHandle<()>, Option<String>), RtcError> {
    let answer = pull_track(subscriber, session_id, SCREEN_TRACK).await?;

    if answer
        .get("requiresImmediateRenegotiation")
        .and_then(Value::as_bool)
        == Some(true)
    {
        renegotiate(subscriber, &answer).await?;
    }

    let mid = mid_of(&answer).map(ToOwned::to_owned);
    let ssrc = ssrc_of(&answer);
    let track = claim_track(&subscriber.signals.tracks, ssrc).await?;
    Ok((
        tokio::spawn(screen_playback_loop(track, Arc::clone(&subscriber.shared))),
        mid,
    ))
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
    let answer = pull_track(subscriber, session_id, MIC_TRACK).await?;

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
async fn pull_track(
    subscriber: &Subscriber,
    session_id: &str,
    track_name: &str,
) -> Result<Value, RtcError> {
    let body = json!({
        "tracks": [{
            "location": "remote",
            "sessionId": session_id,
            "trackName": track_name,
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

/// Tells Cloudflare to stop sending a track this client has stopped watching.
///
/// **Why this exists.** Giving up the viewer used to be a purely local act:
/// `reconcile_watch` aborted the playback task and nothing crossed the wire.
/// Measured on 2026-08-27 with `bin/watch-cost` (plan.md §7.9), that left
/// 62.7 kB/s of the 62.9 kB/s the open viewer was receiving still arriving —
/// the room paying for a picture nobody was looking at, on every client that
/// had ever opened one, for as long as the share lasted. prd.md §3 F3 asks for
/// opt-in viewing, and opt-in has to be revocable.
///
/// **The shape of it.** A pull is closed the way it was opened: by
/// renegotiation. The transceiver goes `Inactive` — the m-section stays,
/// against `stop()`, so re-watching can reuse it rather than making Cloudflare
/// mint a new one — this side offers, and `tracks/close` names the mid and
/// carries the offer. Unlike a pull, *this* exchange is ours to start, so the
/// SDP goes up as an offer and comes back as an answer.
///
/// The microphone's sender can be rebuilt by the exchange, exactly as in
/// [`renegotiate`], so the publish loop is told about it before its next frame
/// goes nowhere (DR-8).
async fn close_pull(subscriber: &Subscriber, mid: &str) -> Result<(), RtcError> {
    let transceiver = transceiver_for(subscriber.peer.as_ref(), mid)
        .await
        .ok_or_else(|| RtcError::Protocol(format!("no transceiver on m-line {mid} to close")))?;
    transceiver
        .set_direction(RTCRtpTransceiverDirection::Inactive)
        .await?;

    let offer = subscriber.peer.create_offer(None).await?;
    subscriber.peer.set_local_description(offer).await?;
    let sdp = local_sdp(subscriber.peer.as_ref(), &subscriber.signals).await?;
    trace_sdp("close offer", &sdp);

    let answer = subscriber
        .signaling
        .sfu(
            &subscriber.self_id,
            SfuOperation::TracksClose,
            &json!({
                "tracks": [{ "mid": mid }],
                "sessionDescription": { "type": "offer", "sdp": sdp },
                // The renegotiation above is the point; forcing would close the
                // track without one and leave the two ends disagreeing about
                // what the m-section is for.
                "force": false,
            }),
        )
        .await?;

    if let Some((code, description)) = track_error(&answer) {
        return Err(RtcError::Sfu(format!("{description} ({code})")));
    }

    let sdp = sdp_of(&answer)?;
    trace_sdp("close answer", &sdp);
    subscriber
        .peer
        .set_remote_description(RTCSessionDescription::answer(sdp)?)
        .await?;

    subscriber.published.refresh(subscriber.peer.as_ref()).await;
    Ok(())
}

/// The transceiver carrying `mid`, if the peer connection still has one.
async fn transceiver_for(peer: &dyn PeerConnection, mid: &str) -> Option<Arc<dyn RtpTransceiver>> {
    for transceiver in peer.get_transceivers().await {
        if transceiver.mid().await.ok().flatten().as_deref() == Some(mid) {
            return Some(transceiver);
        }
    }
    None
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
    ssrc_for_mid(&sdp, mid_of(answer)?)
}

/// The m-section Cloudflare put a pulled track on.
///
/// The pull is asked for by track name and answered by mid, and the mid is the
/// only name the SFU will take back: `tracks/close` has no other way to say
/// which subscription is being given up (server/src/room.ts, `tracks/close`
/// names the caller's own transceivers).
fn mid_of(answer: &Value) -> Option<&str> {
    answer
        .get("tracks")
        .and_then(Value::as_array)
        .and_then(|tracks| tracks.first())
        .and_then(|track| track.get("mid"))
        .and_then(Value::as_str)
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
///
/// What is *not* one step is the order they arrive in. Packets come off the
/// network reordered and with holes in them, and Opus carries state across
/// them, so [`super::order::Sequence`] stands between the two: it hands over
/// the packets in the order they were spoken, and names the ones that never
/// came so the decoder can conceal them instead of the ring playing a gap.
async fn playback_loop(track: Arc<dyn TrackRemote>, slot: usize, shared: Arc<Shared>) {
    let Ok(mut decoder) = VoiceDecoder::new() else {
        return;
    };
    let mut frame = silent_frame();
    let mut sequence = Sequence::new();
    // Reused across packets: in a stream with nothing wrong with it this holds
    // one `Step` and is cleared, and never allocates again.
    let mut steps: Vec<Step> = Vec::new();

    while let Some(event) = track.poll().await {
        match event {
            TrackRemoteEvent::OnRtpPacket(packet) => {
                steps.clear();
                sequence.accept(packet.header.sequence_number, &packet.payload, &mut steps);
                for step in &steps {
                    match step {
                        Step::Play(payload) => {
                            play_packet(&mut decoder, payload, &mut frame, slot, &shared);
                        }
                        Step::Lost => {
                            conceal_packet(&mut decoder, &mut frame, slot, &shared);
                        }
                    }
                }
            }
            TrackRemoteEvent::OnEnded => break,
            _ => {}
        }
    }

    shared.sink.clear(slot);
}

/// Reassembles one remote screen's RTP into access units and hands them over.
///
/// Two steps, and both are needed. `H264Packet` undoes RTP's own framing —
/// a fragmented keyframe arrives as dozens of FU-A packets and an aggregated
/// parameter set as one STAP-A — and the **marker bit** is what says the last
/// packet of a picture has arrived. A decoder fed per-packet instead of per
/// access unit shows nothing at all.
async fn screen_playback_loop(track: Arc<dyn TrackRemote>, shared: Arc<Shared>) {
    // Annex B, with start codes, which is what a Media Foundation decoder
    // takes and what the sharer's encoder produced in the first place. That is
    // the default, and it is set here anyway: the flag decides whether the
    // bytes coming out are usable, and a default is not a promise.
    let mut depacketizer = H264Packet::default();
    depacketizer.is_avc = false;
    let mut unit: Vec<u8> = Vec::new();

    while let Some(event) = track.poll().await {
        let TrackRemoteEvent::OnRtpPacket(packet) = event else {
            if matches!(event, TrackRemoteEvent::OnEnded) {
                break;
            }
            continue;
        };

        // One malformed or lost fragment loses the picture it belonged to.
        // The next keyframe repairs that; dropping the partial unit is what
        // stops a decoder being fed half of one.
        let Ok(nals) = depacketizer.depacketize(&packet.payload) else {
            unit.clear();
            continue;
        };
        unit.extend_from_slice(&nals);

        if !packet.header.marker || unit.is_empty() {
            continue;
        }
        // Looked up per unit, not captured when the subscription was made: a
        // viewer that closes and reopens is a *new* sink on the same
        // subscription, and a loop holding the old one would feed a window
        // that no longer exists while the new one waited for a picture.
        if let Some(sink) = shared.watch_sink() {
            sink.accept(&unit, starts_with_idr(&unit));
        }
        unit.clear();
    }

    // The track ended under us — the sharer stopped, or the session dropped.
    // Whoever is watching now is who has to be told.
    if let Some(sink) = shared.watch_sink() {
        sink.ended();
    }
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

/// Fills in 20 ms that never arrived.
///
/// Opus extrapolates from what it has already decoded, which is why this is a
/// call into the decoder rather than a frame of silence written here: silence
/// in the middle of a word is the click, and it is the same click the ring
/// produced on its own before anything noticed the packet was missing.
///
/// Deafened conceals for the reason it decodes — the decoder's state has to
/// stay level with the stream, or un-deafening starts in artefacts.
fn conceal_packet(decoder: &mut VoiceDecoder, frame: &mut Frame, slot: usize, shared: &Shared) {
    if decoder.conceal(frame).is_err() {
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
        conceal_packet, is_starting_up, play_packet, publish_loop, silent_frame, ssrc_for_mid,
        ssrc_of, track_error, wait_for_gathering, AudioPrefs, Events, PacketSink,
        PeerConnectionEventHandler, RTCIceCandidateType, RTCIceGatheringState,
        RTCPeerConnectionIceEvent, Shared, TransmitMode, CONNECT_TIMEOUT, GATHER_QUIET,
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

    /// One candidate of the given kind, which is all `Gathered` looks at.
    fn candidate_of(kind: RTCIceCandidateType) -> RTCPeerConnectionIceEvent {
        let mut event = RTCPeerConnectionIceEvent::default();
        event.candidate.typ = kind;
        event
    }

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

    // --- the viewer's subscription ---------------------------------------

    /// A sink that only remembers whether it was fed.
    struct Counting(AtomicUsize);

    impl super::ScreenSink for Counting {
        fn accept(&self, _unit: &[u8], _keyframe: bool) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn ended(&self) {}
    }

    /// The bug DR-33 is about, in the one line that caused it: a second viewer
    /// window installs a second sink, and what is already subscribed has to
    /// start feeding *that* one.
    #[test]
    fn a_second_viewer_takes_the_frames_over() {
        let shared = test_shared();
        let first = Arc::new(Counting(AtomicUsize::new(0)));
        let second = Arc::new(Counting(AtomicUsize::new(0)));

        shared.set_watch_sink(Arc::clone(&first) as Arc<dyn super::ScreenSink>);
        shared.watch_sink().expect("a viewer").accept(&[0], true);
        shared.set_watch_sink(Arc::clone(&second) as Arc<dyn super::ScreenSink>);
        shared.watch_sink().expect("a viewer").accept(&[0], true);

        assert_eq!(first.0.load(Ordering::Relaxed), 1);
        assert_eq!(second.0.load(Ordering::Relaxed), 1, "the frames moved on");
    }

    /// The race the generation exists for: a window's closing is noticed after
    /// the fact, and by then the next window may already be watching.
    #[test]
    fn a_closing_window_cannot_unsubscribe_the_one_after_it() {
        let shared = test_shared();
        let first = Arc::new(Counting(AtomicUsize::new(0)));
        let second = Arc::new(Counting(AtomicUsize::new(0)));

        let closing = shared.set_watch_sink(Arc::clone(&first) as Arc<dyn super::ScreenSink>);
        let current = shared.set_watch_sink(Arc::clone(&second) as Arc<dyn super::ScreenSink>);

        // The late arrival of the first window's destruction.
        shared.clear_watch_sink(closing);
        assert!(shared.watch_sink().is_some(), "the second viewer survived");

        shared.clear_watch_sink(current);
        assert!(
            shared.watch_sink().is_none(),
            "and its own close still works"
        );
    }

    // --- the playback path, without a network ----------------------------

    /// A call with nowhere to play: enough for anything that only cares about
    /// the encode side.
    fn test_shared() -> Arc<Shared> {
        Shared::new(
            "me".to_owned(),
            vec![],
            Arc::new(NullSink),
            TransmitMode::Open,
            Arc::new(AudioPrefs::default()),
        )
    }

    /// A call whose speakers can be inspected afterwards.
    fn listening() -> (Arc<Shared>, Arc<RecordingSink>) {
        let ears = RecordingSink::new();
        let shared = Shared::new(
            "me".to_owned(),
            vec![],
            Arc::clone(&ears) as Arc<dyn AudioSink>,
            TransmitMode::Open,
            Arc::new(AudioPrefs::default()),
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
    fn a_lost_packet_is_played_as_concealment_rather_than_a_gap() {
        // The whole point of `rtc::order`: a hole in the sequence reaches the
        // ring as 20 ms the decoder extrapolated, not as 20 ms of nothing.
        let (shared, ears) = listening();
        let mut decoder = VoiceDecoder::new().expect("decoder");
        let mut frame = silent_frame();
        let mut buffer = [0_u8; MAX_PACKET_BYTES];
        let written = packet(&mut buffer);

        // Something has to have been decoded first: concealment extrapolates
        // from the stream, and there is nothing to extrapolate from at the
        // very start of one.
        for _ in 0..5 {
            play_packet(&mut decoder, &buffer[..written], &mut frame, 0, &shared);
        }
        conceal_packet(&mut decoder, &mut frame, 0, &shared);

        assert_eq!(ears.slot(0).frames, 6, "the lost frame reached the ring");
        assert!(
            frame.iter().any(|&sample| sample != 0),
            "concealment produced silence, which is the artefact it replaces"
        );
    }

    #[test]
    fn concealment_still_runs_while_deafened() {
        // For the reason decoding does (see
        // `deafening_stops_playback_without_stopping_the_decoder`): the
        // decoder's state has to stay level with the stream, or un-deafening
        // starts in artefacts.
        let (shared, ears) = listening();
        let mut decoder = VoiceDecoder::new().expect("decoder");
        let mut frame = silent_frame();
        let mut buffer = [0_u8; MAX_PACKET_BYTES];
        let written = packet(&mut buffer);

        for _ in 0..5 {
            play_packet(&mut decoder, &buffer[..written], &mut frame, 0, &shared);
        }
        shared.deafened.store(true, Ordering::Relaxed);
        conceal_packet(&mut decoder, &mut frame, 0, &shared);

        assert_eq!(ears.slot(0).frames, 5, "concealment reached the speakers");
        assert!(
            frame.iter().any(|&sample| sample != 0),
            "the frame was never concealed while deafened"
        );
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
            TransmitMode::Open,
            Arc::new(AudioPrefs::default()),
        );
        shared
            .slots
            .lock()
            .expect("slots")
            .insert("them".to_owned(), 0);
        (shared, levels)
    }

    /// Who the last push named, in order.
    fn talkers(shared: &Shared) -> Vec<String> {
        shared
            .speaking
            .borrow()
            .talking
            .iter()
            .map(|talker| talker.id.clone())
            .collect()
    }

    #[test]
    fn a_loud_slot_puts_its_participant_in_the_speaking_set() {
        let (shared, levels) = metered();

        levels.set(0, SPEAKING_LEVEL * 2.0);
        shared.refresh_speaking();
        assert_eq!(talkers(&shared), vec!["them".to_owned()]);

        // Still above the floor the push uses, so they are still listed — at a
        // lower level. Half of "speaking" is a person trailing off, and the
        // indicator's whole job now is to show that rather than to blink out.
        levels.set(0, SPEAKING_LEVEL / 2.0);
        shared.refresh_speaking();
        assert_eq!(talkers(&shared), vec!["them".to_owned()]);

        levels.set(0, 0.0);
        shared.refresh_speaking();
        assert!(shared.speaking.borrow().talking.is_empty());
    }

    #[test]
    fn a_louder_slot_reports_a_higher_level() {
        let (shared, levels) = metered();

        levels.set(0, 0.2);
        shared.refresh_speaking();
        let quiet = shared.speaking.borrow().talking[0].level;

        levels.set(0, 0.8);
        shared.refresh_speaking();
        let loud = shared.speaking.borrow().talking[0].level;

        assert!(
            loud > quiet,
            "the meter reported {loud} for a loud speaker and {quiet} for a quiet one"
        );
    }

    #[test]
    fn the_input_meter_moves_while_the_gate_holds_transmission_shut() {
        // The reason it exists: somebody setting a threshold is, by
        // definition, below one. A meter that only moved when the gate was
        // open would be blank at exactly the moment it is being read.
        let (shared, _levels) = metered();
        shared.gate_open.store(false, Ordering::Relaxed);
        shared.input.observe(20_000);

        shared.refresh_speaking();

        assert!(shared.speaking.borrow().talking.is_empty());
        assert!(
            shared.speaking.borrow().input > 0.0,
            "the input meter read silent on a microphone that was not"
        );
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
        assert_eq!(talkers(&shared), vec!["me".to_owned()]);
    }

    #[test]
    fn a_muted_client_is_never_speaking_however_loud_they_are() {
        let (shared, _levels) = metered();
        shared.microphone.observe(30_000);
        shared.muted.store(true, Ordering::Relaxed);

        shared.refresh_speaking();

        assert!(
            shared.speaking.borrow().talking.is_empty(),
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
        assert_eq!(shared.speaking.borrow().talking.len(), 2);

        shared.silence();

        assert!(shared.speaking.borrow().talking.is_empty());
        assert!(
            shared.slot_of("them").is_none(),
            "a name still pointing at a slot the next seat will re-hand-out"
        );
        // The levels the sink reports have not moved; it is this client that
        // has stopped believing them.
        shared.refresh_speaking();
        assert!(shared.speaking.borrow().talking.is_empty());
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

    // --- the gate, without a network -------------------------------------

    /// Digital silence, for the mode that decides on the signal itself.
    struct Quiet {
        left: usize,
    }

    #[async_trait::async_trait]
    impl AudioSource for Quiet {
        async fn next_frame(&mut self) -> Option<Frame> {
            self.left = self.left.checked_sub(1)?;
            Some(silent_frame())
        }
    }

    /// Runs the encode loop with the gate set up, and hands back both what
    /// went out and what the call believed while it ran.
    async fn pump_gated(
        source: Box<dyn AudioSource>,
        mode: TransmitMode,
        key_down: bool,
    ) -> (Arc<Shared>, Arc<CountingSender>) {
        let shared = test_shared();
        shared.transmit.store(mode.code(), Ordering::Relaxed);
        shared.talk_key.store(key_down, Ordering::Relaxed);

        let sender = Arc::new(CountingSender::default());
        let (published, sinks) = watch::channel(Some(Arc::clone(&sender) as Arc<dyn PacketSink>));

        publish_loop(source, sinks, Arc::clone(&shared)).await;
        drop(published);
        (shared, sender)
    }

    /// What plan.md task 3.3 asks for: both modes demonstrably gate
    /// transmission, to the same bar as mute — packets stop, they do not turn
    /// into silence.
    #[tokio::test]
    async fn push_to_talk_sends_nothing_while_the_key_is_up() {
        let (shared, sender) = pump_gated(
            Box::new(Burst { left: 25 }),
            TransmitMode::PushToTalk,
            false,
        )
        .await;

        assert_eq!(sender.count(), 0, "a key nobody held put audio on the wire");
        // And the room is not told this client is talking either: the light
        // has to go out with the key, not at the speed the meter decays.
        shared.microphone.observe(20_000);
        shared.refresh_speaking();
        assert!(shared.speaking.borrow().talking.is_empty());
    }

    #[tokio::test]
    async fn push_to_talk_sends_every_frame_while_the_key_is_held() {
        let (_shared, sender) =
            pump_gated(Box::new(Burst { left: 25 }), TransmitMode::PushToTalk, true).await;

        assert_eq!(sender.count(), 25);
    }

    #[tokio::test]
    async fn voice_activity_sends_nothing_for_a_microphone_nobody_is_talking_into() {
        // Longer than the hangover, so a gate that opened once and never shut
        // would be caught here rather than looking like a slow one.
        let (_shared, sender) = pump_gated(
            Box::new(Quiet { left: 200 }),
            TransmitMode::VoiceActivity,
            false,
        )
        .await;

        assert_eq!(sender.count(), 0, "silence went out as packets");
    }

    #[tokio::test]
    async fn voice_activity_sends_while_there_is_something_to_hear() {
        let (_shared, sender) = pump_gated(
            Box::new(Burst { left: 25 }),
            TransmitMode::VoiceActivity,
            false,
        )
        .await;

        assert_eq!(sender.count(), 25);
    }

    /// Mute is the outer question and the mode is the inner one. A client who
    /// muted while holding the talk key must stay silent.
    #[tokio::test]
    async fn mute_wins_over_a_held_talk_key() {
        let shared = test_shared();
        shared.muted.store(true, Ordering::Relaxed);
        shared
            .transmit
            .store(TransmitMode::PushToTalk.code(), Ordering::Relaxed);
        shared.talk_key.store(true, Ordering::Relaxed);

        let sender = Arc::new(CountingSender::default());
        let (published, sinks) = watch::channel(Some(Arc::clone(&sender) as Arc<dyn PacketSink>));
        publish_loop(Box::new(Burst { left: 25 }), sinks, Arc::clone(&shared)).await;
        drop(published);

        assert_eq!(sender.count(), 0);
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
    /// A gathering that finishes the way webrtc-rs means it to.
    #[tokio::test(start_paused = true)]
    async fn a_complete_gathering_is_not_waited_on_further() {
        let (events, signals) = Events::new();
        events
            .on_ice_gathering_state_change(RTCIceGatheringState::Complete)
            .await;

        let started = tokio::time::Instant::now();
        wait_for_gathering(&signals)
            .await
            .expect("a complete gathering is not a failure");
        assert!(
            started.elapsed() < GATHER_QUIET,
            "a gathering that was already complete was waited on anyway"
        );
    }

    /// The DR-14 case: one ICE URL this network cannot reach keeps `Complete`
    /// from ever being published, and the join used to burn the whole connect
    /// timeout and then fail with candidates sitting in hand.
    #[tokio::test(start_paused = true)]
    async fn candidates_that_stop_arriving_end_the_wait() {
        let (events, signals) = Events::new();

        let arriving = tokio::spawn(async move {
            for _ in 0..3 {
                tokio::time::sleep(GATHER_QUIET / 2).await;
                events
                    .on_ice_candidate(RTCPeerConnectionIceEvent::default())
                    .await;
            }
            // ...and then the unreachable server's client hangs around
            // forever, so no `Complete` ever follows.
            std::future::pending::<()>().await;
        });

        let started = tokio::time::Instant::now();
        wait_for_gathering(&signals)
            .await
            .expect("gathering went quiet with candidates in hand; that is a usable SDP");
        assert!(
            started.elapsed() < CONNECT_TIMEOUT,
            "the wait ran to the connect timeout instead of stopping when the candidates did"
        );
        assert_eq!(
            signals.candidates.borrow().count,
            3,
            "the wait ended before the candidates that were coming had arrived"
        );

        arriving.abort();
    }

    /// A renegotiation must not pay the quiet window twice.
    ///
    /// The pull side answers an offer for every track it subscribes to, and
    /// each answer asks for the local description again. Nothing here restarts
    /// ICE, so the candidates cannot have changed — waiting for them again is
    /// [`GATHER_QUIET`] of silence per subscribe, on the path a person is
    /// waiting through (DR-19).
    #[tokio::test(start_paused = true)]
    async fn a_gathering_that_has_settled_once_is_not_waited_on_again() {
        let (events, signals) = Events::new();

        let arriving = tokio::spawn(async move {
            events
                .on_ice_candidate(RTCPeerConnectionIceEvent::default())
                .await;
            std::future::pending::<()>().await;
        });
        wait_for_gathering(&signals)
            .await
            .expect("one candidate and then quiet is a usable SDP");

        let again = tokio::time::Instant::now();
        wait_for_gathering(&signals)
            .await
            .expect("the second wait has nothing to wait for");
        assert!(
            again.elapsed() < GATHER_QUIET,
            "the second wait paid the quiet window again"
        );

        arriving.abort();
    }

    /// The rule that makes a join a second faster: once there is a direct path
    /// and a fallback, whatever is still allocating is a fallback of a kind
    /// already in hand (DR-19).
    #[tokio::test(start_paused = true)]
    async fn gathering_stops_once_there_is_a_path_and_a_fallback() {
        let (events, signals) = Events::new();
        let started = tokio::time::Instant::now();

        let arriving = tokio::spawn(async move {
            for kind in [
                RTCIceCandidateType::Host,
                RTCIceCandidateType::Srflx,
                RTCIceCandidateType::Relay,
            ] {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                events.on_ice_candidate(candidate_of(kind)).await;
            }
            // ...and five more relays, one per TURN port Cloudflare hands out,
            // none of them worth a person's time.
            std::future::pending::<()>().await;
        });

        wait_for_gathering(&signals)
            .await
            .expect("a path and a fallback are a usable SDP");
        assert!(
            started.elapsed() < GATHER_QUIET,
            "waited out the quiet window with everything it needed in hand"
        );

        arriving.abort();
    }

    /// A network with no relay to be had still waits for quiet: `enough` is a
    /// shortcut, not the only way out.
    #[tokio::test(start_paused = true)]
    async fn a_direct_path_alone_still_waits_for_the_stragglers() {
        let (events, signals) = Events::new();

        let arriving = tokio::spawn(async move {
            events
                .on_ice_candidate(candidate_of(RTCIceCandidateType::Srflx))
                .await;
            std::future::pending::<()>().await;
        });

        let started = tokio::time::Instant::now();
        wait_for_gathering(&signals)
            .await
            .expect("quiet with a candidate in hand is still a usable SDP");
        assert!(
            started.elapsed() >= GATHER_QUIET,
            "gave up on the relay before the quiet window was out"
        );

        arriving.abort();
    }

    /// Quiet is only an answer if something was gathered. A peer connection
    /// that produced nothing at all has no SDP worth sending, so it waits out
    /// the connect timeout and fails.
    #[tokio::test(start_paused = true)]
    async fn silence_with_no_candidates_is_still_a_failure() {
        let (_events, signals) = Events::new();

        let started = tokio::time::Instant::now();
        let failure = wait_for_gathering(&signals)
            .await
            .expect_err("no candidates and no completion is not a joinable connection");

        assert!(
            format!("{failure}").contains("never completed"),
            "unexpected failure: {failure}"
        );
        assert!(
            started.elapsed() >= CONNECT_TIMEOUT,
            "gave up before the connect timeout it was given"
        );
    }
}
