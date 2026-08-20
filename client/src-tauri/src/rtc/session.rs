//! One call: join a room, publish the microphone, play everyone else.
//!
//! The session owns a single peer connection to the Realtime SFU. The
//! microphone goes up once at join; every remote `mic` track is pulled as the
//! roster reveals it, and each gets a playback slot for the length of its stay.
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
    sync::{mpsc, watch},
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
    signaling::{ClientMessage, IceServer, Participant, ServerMessage, SfuOperation, Signaling},
    RtcError,
};
use crate::audio::{
    device::{AudioSink, AudioSource, MAX_REMOTE_SLOTS},
    opus::{silent_frame, VoiceDecoder, VoiceEncoder, FRAME_MS, MAX_PACKET_BYTES, SAMPLE_RATE_HZ},
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

/// How many times to build a peer connection before giving up on the room, and
/// the unit of backoff between tries (multiplied by the attempt number).
const JOIN_ATTEMPTS: u32 = 3;
const JOIN_BACKOFF: Duration = Duration::from_millis(600);

/// Consecutive failed microphone frames before the publish loop stops trying.
/// 50 frames is one second — long enough to ride out a renegotiation, short
/// enough that a client which has genuinely lost its sender says so.
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
    joined: &crate::rtc::signaling::JoinResponse,
) -> Result<(Arc<dyn PeerConnection>, Signals, Published), RtcError> {
    let (events, signals) = Events::new();
    let peer: Arc<dyn PeerConnection> = Arc::new(open_peer(&joined.sfu.ice_servers, events).await?);

    let published = publish_mic(signaling, &joined.self_id, peer.as_ref(), &signals).await?;
    wait_for_connection(&signals).await?;

    Ok((peer, signals, published))
}

/// A live call. Dropping it leaves the room.
pub struct Call {
    self_id: String,
    commands: mpsc::Sender<ClientMessage>,
    roster: watch::Receiver<Vec<Participant>>,
    muted: Arc<AtomicBool>,
    deafened: Arc<AtomicBool>,
    /// Kept alive for the length of the call; closing it tears down ICE.
    peer: Arc<dyn PeerConnection>,
    tasks: Vec<JoinHandle<()>>,
}

impl Call {
    /// Joins `options.room` and starts sending and receiving audio.
    ///
    /// Returns once the microphone is published and the transport has
    /// connected — a `Call` that exists is a call you are audible on.
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
                Ok(session) => return Ok(Self::start(signaling, session, source, sink)),
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
        session: Session,
        source: Box<dyn AudioSource>,
        sink: Arc<dyn AudioSink>,
    ) -> Self {
        let muted = Arc::new(AtomicBool::new(false));
        let deafened = Arc::new(AtomicBool::new(false));
        let (roster_tx, roster) = watch::channel(session.participants.clone());

        let tasks = vec![
            tokio::spawn(publish_loop(
                source,
                session.published.clone(),
                Arc::clone(&muted),
            )),
            tokio::spawn(subscribe_loop(SubscribeLoop {
                signaling,
                self_id: session.self_id.clone(),
                peer: Arc::clone(&session.peer),
                signals: session.signals,
                published: session.published,
                sink,
                deafened: Arc::clone(&deafened),
                inbound: session.inbound,
                roster_tx,
                initial: session.participants,
            })),
        ];

        Self {
            self_id: session.self_id,
            commands: session.commands,
            roster,
            muted,
            deafened,
            peer: session.peer,
            tasks,
        }
    }

    /// This client's participant id, as everyone else sees it.
    #[must_use]
    pub fn self_id(&self) -> &str {
        &self.self_id
    }

    /// The room, updated as people come and go.
    #[must_use]
    pub fn roster(&self) -> watch::Receiver<Vec<Participant>> {
        self.roster.clone()
    }

    /// Stops sending audio. Packets stop entirely rather than carrying
    /// silence, so a muted client costs the room nothing (prd.md §3 F1).
    pub async fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
        let _ = self.commands.send(ClientMessage::Mute { muted }).await;
    }

    #[must_use]
    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    /// Stops playing audio. The tracks stay subscribed: re-deafening should be
    /// instant, and a renegotiation round trip would not be.
    pub async fn set_deafened(&self, deafened: bool) {
        self.deafened.store(deafened, Ordering::Relaxed);
        let _ = self.commands.send(ClientMessage::Deafen { deafened }).await;
    }

    #[must_use]
    pub fn is_deafened(&self) -> bool {
        self.deafened.load(Ordering::Relaxed)
    }

    /// Leaves the room and closes the transport.
    ///
    /// Telling the room first is what makes the departure instant for everyone
    /// else; dropping the `Call` without this works too, but the roster only
    /// catches up on the next heartbeat sweep.
    pub async fn leave(mut self) {
        let _ = self.commands.send(ClientMessage::Leave).await;
        let _ = self.peer.close().await;
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
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
    connection: watch::Receiver<RTCPeerConnectionState>,
    tracks: Arc<TrackInbox>,
}

/// Remote tracks that have arrived but not yet been claimed by a subscription.
#[derive(Default)]
struct TrackInbox {
    waiting: Mutex<Vec<Arc<dyn TrackRemote>>>,
    arrived: tokio::sync::Notify,
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
/// The allocation per frame (`Bytes::copy_from_slice`) is deliberate and safe:
/// this task is on the far side of the capture ring buffer, never inside a
/// device callback, which is where styleguide.md's no-allocation rule applies.
async fn publish_loop(
    mut source: Box<dyn AudioSource>,
    published: Published,
    muted: Arc<AtomicBool>,
) {
    let Ok(mut encoder) = VoiceEncoder::new() else {
        return;
    };
    let mut packet = [0_u8; MAX_PACKET_BYTES];
    let mut failures = 0_usize;

    while let Some(frame) = source.next_frame().await {
        // Mute stops packets rather than sending silence: the room should cost
        // nothing while nobody is talking (prd.md §3 F1).
        if muted.load(Ordering::Relaxed) {
            continue;
        }
        let Ok(written) = encoder.encode(&frame, &mut packet) else {
            continue;
        };
        let sent = published
            .track
            .sample_writer(
                published.ssrc.load(Ordering::Relaxed),
                published.payload_type.load(Ordering::Relaxed),
            )
            .write_sample(&Sample {
                data: Bytes::copy_from_slice(&packet[..written]),
                duration: Duration::from_millis(u64::from(FRAME_MS)),
                ..Default::default()
            })
            .await;

        // A failed write is not fatal on its own — the transport can be
        // between states — but a run of them means nobody can hear this
        // client, which is worth saying out loud rather than going quiet.
        match sent {
            Ok(()) => failures = 0,
            Err(error) => {
                failures += 1;
                if failures == 1 || failures == PUBLISH_FAILURE_LIMIT {
                    eprintln!("microphone frame not sent: {error}");
                }
                if failures >= PUBLISH_FAILURE_LIMIT {
                    eprintln!("giving up on publishing after {failures} failed frames");
                    return;
                }
            }
        }
    }
}

// --- subscribing -----------------------------------------------------------

struct SubscribeLoop {
    signaling: Arc<Signaling>,
    self_id: String,
    peer: Arc<dyn PeerConnection>,
    signals: Signals,
    /// Shared with the publish loop so a renegotiation can hand it the
    /// sender's new identity.
    published: Published,
    sink: Arc<dyn AudioSink>,
    deafened: Arc<AtomicBool>,
    inbound: mpsc::Receiver<ServerMessage>,
    roster_tx: watch::Sender<Vec<Participant>>,
    initial: Vec<Participant>,
}

/// One remote speaker, for as long as they are in the room.
struct Subscription {
    slot: usize,
    playback: JoinHandle<()>,
}

/// Follows the roster: pulls every `mic` that appears, drops every one that
/// goes away, and republishes the roster for the UI.
///
/// The roster is pushed only when it changes, so a subscription that fails
/// would otherwise leave that speaker silent for the rest of the call. The
/// ticker is what makes a failure temporary: reconciling against the roster we
/// already have costs nothing when everyone is already subscribed.
async fn subscribe_loop(mut loop_state: SubscribeLoop) {
    let mut subscribed: HashMap<String, Subscription> = HashMap::new();
    let mut latest = std::mem::take(&mut loop_state.initial);

    reconcile(&loop_state, &mut subscribed, &latest).await;

    let mut retry = tokio::time::interval(RESUBSCRIBE_INTERVAL);
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let pushed = tokio::select! {
            message = loop_state.inbound.recv() => message,
            _ = retry.tick() => {
                reconcile(&loop_state, &mut subscribed, &latest).await;
                continue;
            }
        };

        let Some(message) = pushed else {
            return;
        };

        let participants = match message {
            ServerMessage::Welcome { participants, .. }
            | ServerMessage::Roster { participants } => participants,
            ServerMessage::Error { code, message } => {
                eprintln!("room error: {message} ({code})");
                continue;
            }
        };

        latest = participants;
        let _ = loop_state.roster_tx.send(latest.clone());
        reconcile(&loop_state, &mut subscribed, &latest).await;
    }
}

async fn reconcile(
    loop_state: &SubscribeLoop,
    subscribed: &mut HashMap<String, Subscription>,
    participants: &[Participant],
) {
    // Gone, or stopped publishing: free the slot so the next arrival can have
    // it, and stop the audio immediately rather than draining the ring.
    subscribed.retain(|id, subscription| {
        let still_here = participants
            .iter()
            .any(|peer| &peer.id == id && peer.publishes(MIC_TRACK));
        if !still_here {
            subscription.playback.abort();
            loop_state.sink.clear(subscription.slot);
        }
        still_here
    });

    for peer in participants {
        if peer.id == loop_state.self_id || subscribed.contains_key(&peer.id) {
            continue;
        }
        if !peer.publishes(MIC_TRACK) {
            continue;
        }
        let Some(session_id) = peer.session_id.as_deref() else {
            continue;
        };

        let Some(slot) = free_slot(subscribed) else {
            // Only reachable if the server's cap and this client's slot count
            // ever disagree; being silent about one speaker beats crashing.
            eprintln!("no playback slot left for {}", peer.name);
            continue;
        };

        match subscribe_to(loop_state, session_id, slot).await {
            Ok(subscription) => {
                subscribed.insert(peer.id.clone(), subscription);
            }
            Err(error) => eprintln!("could not subscribe to {}: {error}", peer.name),
        }
    }
}

fn free_slot(subscribed: &HashMap<String, Subscription>) -> Option<usize> {
    (0..MAX_REMOTE_SLOTS).find(|slot| {
        !subscribed
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
    loop_state: &SubscribeLoop,
    session_id: &str,
    slot: usize,
) -> Result<Subscription, RtcError> {
    let answer = pull_track(loop_state, session_id).await?;

    // Cloudflare only offers when it needs a new m-section. Reusing one that
    // is already there is a valid answer with no SDP in it, and forcing a
    // renegotiation would be a round trip for nothing.
    if answer
        .get("requiresImmediateRenegotiation")
        .and_then(Value::as_bool)
        == Some(true)
    {
        renegotiate(loop_state, &answer).await?;
    }

    let ssrc = ssrc_of(&answer);
    let track = claim_track(&loop_state.signals.tracks, ssrc).await?;

    Ok(Subscription {
        slot,
        playback: tokio::spawn(playback_loop(
            track,
            slot,
            Arc::clone(&loop_state.sink),
            Arc::clone(&loop_state.deafened),
        )),
    })
}

/// Asks for a remote track, waiting out the window where the publisher has
/// negotiated but is not yet sending.
///
/// The roster learns about a track when the publisher's `tracks/new` is
/// accepted, which is before their ICE and DTLS finish — so a peer is
/// advertised a beat before Cloudflare will serve them. Retrying is the whole
/// fix; there is nothing wrong on either side (DR-8).
async fn pull_track(loop_state: &SubscribeLoop, session_id: &str) -> Result<Value, RtcError> {
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
        let answer = loop_state
            .signaling
            .sfu(&loop_state.self_id, SfuOperation::TracksNew, &body)
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
async fn renegotiate(loop_state: &SubscribeLoop, answer: &Value) -> Result<(), RtcError> {
    let offer = sdp_of(answer)?;
    trace_sdp("pull offer", &offer);

    loop_state
        .peer
        .set_remote_description(RTCSessionDescription::offer(offer)?)
        .await?;
    let local = loop_state.peer.create_answer(None).await?;
    loop_state.peer.set_local_description(local).await?;
    let sdp = local_sdp(loop_state.peer.as_ref(), &loop_state.signals).await?;
    trace_sdp("our answer", &sdp);

    loop_state
        .signaling
        .sfu(
            &loop_state.self_id,
            SfuOperation::Renegotiate,
            &json!({ "sessionDescription": { "type": "answer", "sdp": sdp } }),
        )
        .await?;

    // The microphone's sender may have been rebuilt by the exchange above; the
    // publish loop has to be told before its next frame goes nowhere.
    loop_state.published.refresh(loop_state.peer.as_ref()).await;
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
async fn playback_loop(
    track: Arc<dyn TrackRemote>,
    slot: usize,
    sink: Arc<dyn AudioSink>,
    deafened: Arc<AtomicBool>,
) {
    let Ok(mut decoder) = VoiceDecoder::new() else {
        return;
    };
    let mut frame = silent_frame();

    while let Some(event) = track.poll().await {
        match event {
            TrackRemoteEvent::OnRtpPacket(packet) => {
                if packet.payload.is_empty() {
                    continue;
                }
                // Deafened still decodes: the codec keeps state across packets,
                // and skipping them would leave it confused on un-deafen.
                let usable = decoder.decode(&packet.payload, &mut frame).is_ok();
                if usable && !deafened.load(Ordering::Relaxed) {
                    sink.play(slot, &frame);
                }
            }
            TrackRemoteEvent::OnEnded => break,
            _ => {}
        }
    }

    sink.clear(slot);
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
    use super::{is_starting_up, ssrc_for_mid, ssrc_of, track_error};
    use serde_json::json;

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
}
