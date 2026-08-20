//! Task 2.3 spike: can `webrtc-rs` complete ICE/DTLS with the Cloudflare
//! Realtime SFU, and does an Opus track pushed by one session come back out of
//! another?
//!
//! Two participants join one room against a live deploy. The speaker publishes
//! a 440 Hz tone as `mic`; the listener reads the roster to find it, pulls it
//! through the Worker's SFU proxy, and runs a Goertzel bin over the decoded
//! audio. A tone that arrives at the right frequency exercises the whole path
//! at once — signalling, the proxy (DR-2), ICE, DTLS, SRTP, and the roster's
//! read-don't-announce track bookkeeping (DR-6).
//!
//! Nothing here is Windows-specific: `webrtc-rs` is pure Rust and the SFU
//! handshake is the same on every host, so the project's biggest unknown can be
//! answered from the dev machine. Real capture devices are task 2.1's problem.
//!
//! ```text
//! cargo run --bin rtc-spike -- --room test
//! cargo run --bin rtc-spike -- --room test --base http://localhost:8787
//! ```

use std::{
    env,
    f32::consts::TAU,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, bail, Context as _, Result};
use bytes::Bytes;
use goodvoice_client_lib::audio::opus::{
    silent_frame, Frame, VoiceDecoder, VoiceEncoder, FRAME_MS, FRAME_SAMPLES, MAX_PACKET_BYTES,
    SAMPLE_RATE_HZ,
};
use rtc::{
    media::Sample,
    peer_connection::configuration::media_engine::MIME_TYPE_OPUS,
    rtp_transceiver::{
        rtp_sender::{
            RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
            RtpCodecKind,
        },
        PayloadType,
    },
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use webrtc::{
    media_stream::{
        track_local::{static_sample::TrackLocalStaticSample, TrackLocal},
        track_remote::{TrackRemote, TrackRemoteEvent},
        MediaStreamTrack, Track as _,
    },
    peer_connection::{
        register_default_interceptors, MediaEngine, PeerConnection, PeerConnectionBuilder,
        PeerConnectionEventHandler, RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer,
        RTCPeerConnectionState, RTCSessionDescription, Registry,
    },
    rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit},
    runtime::default_runtime,
};

/// The deploy from plan.md task 1.5.
const DEFAULT_BASE: &str = "https://goodvoice.goodvoice-server.workers.dev";

/// The tone the speaker sends, and a bin far enough away to compare it against.
const TONE_HZ: f32 = 440.0;
const OFF_TONE_HZ: f32 = 1_500.0;

/// Opus' de-facto payload type. Cloudflare answers with this one, and offering
/// the same number keeps the two sides from renumbering mid-negotiation.
const OPUS_PAYLOAD_TYPE: PayloadType = 111;

/// How much audio the listener collects before judging it. 100 packets is two
/// seconds — long enough to outlast the codec's lookahead and any jitter at the
/// head of the stream.
const TARGET_PACKETS: usize = 100;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(25);
const MEDIA_TIMEOUT: Duration = Duration::from_secs(20);

// --- signalling ------------------------------------------------------------

/// `POST /rooms/:code/join`.
#[derive(Debug, Deserialize)]
struct JoinResponse {
    #[serde(rename = "self")]
    self_id: String,
    participants: Vec<Participant>,
    sfu: SfuCredentials,
}

#[derive(Debug, Deserialize)]
struct Participant {
    id: String,
    name: String,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default)]
    tracks: Vec<PublishedTrack>,
}

#[derive(Debug, Deserialize)]
struct PublishedTrack {
    name: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
struct SfuCredentials {
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "iceServers")]
    ice_servers: Vec<IceServerJson>,
}

#[derive(Debug, Deserialize)]
struct IceServerJson {
    urls: Urls,
    username: Option<String>,
    credential: Option<String>,
}

/// Cloudflare answers `urls` as either a string or a list; both are legal ICE.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Urls {
    One(String),
    Many(Vec<String>),
}

/// Borrows rather than consumes: the join answer stays whole afterwards, and
/// both participants build their peer connection the same way.
fn ice_servers(raw: &[IceServerJson]) -> Vec<RTCIceServer> {
    raw.iter()
        .map(|server| RTCIceServer {
            urls: match &server.urls {
                Urls::One(url) => vec![url.clone()],
                Urls::Many(urls) => urls.clone(),
            },
            username: server.username.clone().unwrap_or_default(),
            credential: server.credential.clone().unwrap_or_default(),
        })
        .collect()
}

/// The Worker, as the spike talks to it.
struct Signaling {
    http: reqwest::Client,
    base: String,
    room: String,
}

impl Signaling {
    async fn join(&self, name: &str) -> Result<JoinResponse> {
        let response = self
            .http
            .post(format!("{}/rooms/{}/join", self.base, self.room))
            .json(&json!({ "name": name }))
            .send()
            .await
            .context("join request failed")?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("join refused ({status}): {body}");
        }
        serde_json::from_str(&body).with_context(|| format!("unreadable join answer: {body}"))
    }

    /// One proxied `/rooms/:code/sfu/*` call. The method has to match what the
    /// room allowlists for the operation, or it answers 400 before Cloudflare
    /// ever sees the request.
    async fn sfu(
        &self,
        participant: &str,
        method: reqwest::Method,
        operation: &str,
        body: &Value,
    ) -> Result<Value> {
        let url = format!(
            "{}/rooms/{}/sfu/{operation}?p={participant}",
            self.base, self.room
        );
        let response = self
            .http
            .request(method, url)
            .json(body)
            .send()
            .await
            .with_context(|| format!("{operation} request failed"))?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("{operation} refused ({status}): {text}");
        }
        serde_json::from_str(&text)
            .with_context(|| format!("unreadable {operation} answer: {text}"))
    }
}

// --- peer connection -------------------------------------------------------

/// The three events the spike waits on, forwarded off the driver thread.
struct Events {
    who: &'static str,
    gathered: mpsc::Sender<()>,
    connected: mpsc::Sender<()>,
    tracks: mpsc::Sender<Arc<dyn TrackRemote>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Events {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gathered.try_send(());
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        println!("  [{}] connection state: {state}", self.who);
        if state == RTCPeerConnectionState::Connected {
            let _ = self.connected.try_send(());
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        println!("  [{}] remote track arrived", self.who);
        let _ = self.tracks.try_send(track);
    }
}

/// Receiving ends of [`Events`], kept together so a caller waits on one value.
struct Waits {
    gathered: mpsc::Receiver<()>,
    connected: mpsc::Receiver<()>,
    tracks: mpsc::Receiver<Arc<dyn TrackRemote>>,
}

fn events(who: &'static str) -> (Arc<Events>, Waits) {
    let (gathered_tx, gathered) = mpsc::channel(1);
    let (connected_tx, connected) = mpsc::channel(1);
    let (tracks_tx, tracks) = mpsc::channel(4);
    (
        Arc::new(Events {
            who,
            gathered: gathered_tx,
            connected: connected_tx,
            tracks: tracks_tx,
        }),
        Waits {
            gathered,
            connected,
            tracks,
        },
    )
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
    ice_servers: Vec<RTCIceServer>,
    handler: Arc<Events>,
) -> Result<impl PeerConnection> {
    let mut media_engine = MediaEngine::default();
    media_engine.register_codec(
        RTCRtpCodecParameters {
            rtp_codec: opus_codec(),
            payload_type: OPUS_PAYLOAD_TYPE,
        },
        RtpCodecKind::Audio,
    )?;
    let registry = register_default_interceptors(Registry::new(), &mut media_engine)?;

    let runtime =
        default_runtime().ok_or_else(|| anyhow!("webrtc was built without a runtime feature"))?;

    // Boxed: building a peer connection carries the whole media engine and
    // interceptor chain, so the future is ~21 kB and would otherwise sit in
    // every caller's stack frame.
    let peer = Box::pin(
        PeerConnectionBuilder::new()
            .with_configuration(
                RTCConfigurationBuilder::new()
                    .with_ice_servers(ice_servers)
                    .build(),
            )
            .with_media_engine(media_engine)
            .with_interceptor_registry(registry)
            .with_handler(handler)
            .with_runtime(runtime)
            .with_udp_addrs(vec!["0.0.0.0:0"])
            .build(),
    )
    .await?;
    Ok(peer)
}

/// Waits for one signal, turning a silent timeout into a readable error.
async fn wait(receiver: &mut mpsc::Receiver<()>, limit: Duration, what: &str) -> Result<()> {
    tokio::time::timeout(limit, receiver.recv())
        .await
        .map_err(|_| anyhow!("timed out waiting for {what}"))?
        .ok_or_else(|| anyhow!("{what} channel closed"))
}

/// Cloudflare negotiates without trickle: the offer it receives has to carry
/// every candidate already.
async fn local_sdp_once_gathered(
    peer: &impl PeerConnection,
    gathered: &mut mpsc::Receiver<()>,
) -> Result<String> {
    wait(gathered, CONNECT_TIMEOUT, "ICE gathering").await?;
    peer.local_description()
        .await
        .map(|description| description.sdp)
        .ok_or_else(|| anyhow!("no local description after gathering"))
}

// --- the speaker -----------------------------------------------------------

/// A published track, plus what the frame loop needs to keep writing to it.
struct Published {
    track: Arc<TrackLocalStaticSample>,
    ssrc: u32,
    payload_type: PayloadType,
}

async fn publish_mic(
    signaling: &Signaling,
    join: &JoinResponse,
    peer: &impl PeerConnection,
    gathered: &mut mpsc::Receiver<()>,
) -> Result<Published> {
    let ssrc = seed();
    let track = Arc::new(TrackLocalStaticSample::new(MediaStreamTrack::new(
        "goodvoice".to_owned(),
        "mic".to_owned(),
        "goodvoice mic".to_owned(),
        RtpCodecKind::Audio,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(ssrc),
                ..Default::default()
            },
            codec: opus_codec(),
            ..Default::default()
        }],
    ))?);

    // Sendonly, not sendrecv: the speaker has nothing to receive on this
    // transceiver, and offering to would have the SFU reserve a slot for media
    // that never comes.
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
    let sdp = local_sdp_once_gathered(peer, gathered).await?;

    // The mid is assigned by `create_offer`; it is how Cloudflare pairs the
    // m-section in the SDP with the name the room stores.
    let mid = transceiver
        .mid()
        .await?
        .ok_or_else(|| anyhow!("transceiver has no mid after set_local_description"))?;

    let answer = signaling
        .sfu(
            &join.self_id,
            reqwest::Method::POST,
            "tracks/new",
            &json!({
                "sessionDescription": { "type": "offer", "sdp": sdp },
                "tracks": [{ "location": "local", "mid": mid, "trackName": "mic" }],
            }),
        )
        .await?;

    println!("  [speaker] tracks/new answer: {answer}");
    report_track_errors(&answer);

    let sdp = answer
        .get("sessionDescription")
        .and_then(|description| description.get("sdp"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tracks/new answered without an SDP: {answer}"))?;
    peer.set_remote_description(RTCSessionDescription::answer(sdp.to_owned())?)
        .await?;

    let payload_type = peer
        .get_senders()
        .await
        .first()
        .ok_or_else(|| anyhow!("no sender after negotiation"))?
        .get_parameters()
        .await?
        .rtp_parameters
        .codecs
        .first()
        .map(|codec| codec.payload_type)
        .ok_or_else(|| anyhow!("sender has no negotiated codec"))?;

    let ssrc = *track
        .ssrcs()
        .await
        .first()
        .ok_or_else(|| anyhow!("track has no SSRC"))?;

    Ok(Published {
        track,
        ssrc,
        payload_type,
    })
}

/// Streams a 440 Hz tone as 20 ms Opus frames until the task is dropped.
///
/// `Bytes::copy_from_slice` allocates per frame, which the styleguide bans on
/// the real voice path — a spike may pay it, task 2.4 has to move to a pooled
/// buffer or `TrackLocalStaticRTP`.
async fn stream_tone(published: Published) -> Result<()> {
    let mut encoder = VoiceEncoder::new()?;
    let mut packet = [0_u8; MAX_PACKET_BYTES];
    let mut phase = 0.0_f32;
    let mut ticker = tokio::time::interval(Duration::from_millis(u64::from(FRAME_MS)));

    loop {
        ticker.tick().await;
        let frame = tone(TONE_HZ, &mut phase);
        let written = encoder.encode(&frame, &mut packet)?;
        published
            .track
            .sample_writer(published.ssrc, published.payload_type)
            .write_sample(&Sample {
                data: Bytes::copy_from_slice(&packet[..written]),
                duration: Duration::from_millis(u64::from(FRAME_MS)),
                ..Default::default()
            })
            .await?;
    }
}

// --- the listener ----------------------------------------------------------

async fn pull_mic(
    signaling: &Signaling,
    listener: &JoinResponse,
    speaker_session: &str,
    peer: &impl PeerConnection,
    gathered: &mut mpsc::Receiver<()>,
) -> Result<()> {
    // No local SDP goes up here: the puller has nothing to offer yet, so
    // Cloudflare is the one that offers and this side answers.
    let answer = signaling
        .sfu(
            &listener.self_id,
            reqwest::Method::POST,
            "tracks/new",
            &json!({
                "tracks": [{
                    "location": "remote",
                    "sessionId": speaker_session,
                    "trackName": "mic",
                }],
            }),
        )
        .await?;

    println!("  [listener] tracks/new answer: {answer}");
    report_track_errors(&answer);

    let offer = answer
        .get("sessionDescription")
        .and_then(|description| description.get("sdp"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("pull answered without an SDP: {answer}"))?;

    peer.set_remote_description(RTCSessionDescription::offer(offer.to_owned())?)
        .await?;
    let local = peer.create_answer(None).await?;
    peer.set_local_description(local).await?;
    let sdp = local_sdp_once_gathered(peer, gathered).await?;

    signaling
        .sfu(
            &listener.self_id,
            reqwest::Method::PUT,
            "renegotiate",
            &json!({ "sessionDescription": { "type": "answer", "sdp": sdp } }),
        )
        .await?;

    Ok(())
}

/// What the listener actually heard.
struct Heard {
    packets: usize,
    decoded_frames: usize,
    tone_energy: f32,
    off_tone_energy: f32,
    rms: f64,
}

/// Reads RTP off the remote track and decodes it back to PCM.
///
/// The RTP payload of an Opus stream *is* the Opus packet — there is no
/// aggregation header to strip — so the depacketiser and the decoder are the
/// same step here.
async fn listen(track: &Arc<dyn TrackRemote>) -> Result<Heard> {
    let mut decoder = VoiceDecoder::new()?;
    let mut frame = silent_frame();
    let mut last = silent_frame();
    let mut packets = 0_usize;
    let mut decoded_frames = 0_usize;

    let deadline = tokio::time::Instant::now() + MEDIA_TIMEOUT;
    while packets < TARGET_PACKETS {
        let event = tokio::time::timeout_at(deadline, track.poll())
            .await
            .map_err(|_| anyhow!("only {packets} RTP packets arrived before the deadline"))?;

        match event {
            Some(TrackRemoteEvent::OnRtpPacket(rtp)) => {
                packets += 1;
                if rtp.payload.is_empty() {
                    continue;
                }
                if decoder.decode(&rtp.payload, &mut frame).is_ok() {
                    decoded_frames += 1;
                    last = frame;
                }
            }
            Some(TrackRemoteEvent::OnEnded) | None => {
                bail!("the remote track ended after {packets} packets")
            }
            Some(_) => {}
        }
    }

    Ok(Heard {
        packets,
        decoded_frames,
        tone_energy: bin_energy(&last, TONE_HZ),
        off_tone_energy: bin_energy(&last, OFF_TONE_HZ),
        rms: rms(&last),
    })
}

// --- signal helpers --------------------------------------------------------

/// One frame of a sine wave, continuous across frames via `phase`.
#[allow(
    clippy::cast_possible_truncation,
    reason = "amplitude is bounded to i16 range by construction"
)]
#[allow(
    clippy::cast_precision_loss,
    reason = "48000 is exactly representable as f32"
)]
fn tone(frequency_hz: f32, phase: &mut f32) -> Frame {
    let mut frame = silent_frame();
    for sample in &mut frame {
        *sample = (phase.sin() * 8_000.0) as i16;
        *phase += TAU * frequency_hz / SAMPLE_RATE_HZ as f32;
    }
    frame
}

/// Goertzel: energy in one frequency bin, enough to tell 440 Hz from noise
/// without pulling in an FFT.
#[allow(
    clippy::cast_precision_loss,
    reason = "constants and sample values are exact in f32"
)]
fn bin_energy(frame: &Frame, frequency_hz: f32) -> f32 {
    let coefficient = 2.0 * (TAU * frequency_hz / SAMPLE_RATE_HZ as f32).cos();
    let (mut previous, mut previous2) = (0.0_f32, 0.0_f32);
    for &sample in frame {
        let current = f32::from(sample) + coefficient * previous - previous2;
        previous2 = previous;
        previous = current;
    }
    previous.mul_add(previous, previous2 * previous2) - coefficient * previous * previous2
}

#[allow(
    clippy::cast_precision_loss,
    reason = "FRAME_SAMPLES is 960 — exact in f64"
)]
fn rms(frame: &Frame) -> f64 {
    let sum: f64 = frame.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum / FRAME_SAMPLES as f64).sqrt()
}

/// Nanoseconds since the epoch, folded into 32 bits. Enough entropy for an
/// SSRC and a room suffix in a spike; not a substitute for an RNG.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the low bits are the point — this is a nonce, not a clock"
)]
fn seed() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |elapsed| {
            elapsed.subsec_nanos() ^ elapsed.as_secs() as u32
        })
}

/// DR-6 says a `tracks/new` can come back 200 with individual tracks rejected,
/// but the shape of that rejection was inferred rather than observed. Print
/// whatever Cloudflare actually sends so the DR can be corrected.
fn report_track_errors(answer: &Value) {
    let Some(tracks) = answer.get("tracks").and_then(Value::as_array) else {
        return;
    };
    for track in tracks {
        if track.get("error").is_some() || track.get("errorCode").is_some() {
            println!("  !! Cloudflare rejected a track: {track}");
        }
    }
}

// --- entry point -----------------------------------------------------------

struct Args {
    base: String,
    room: String,
}

fn args() -> Args {
    let mut base = env::var("GOODVOICE_BASE").unwrap_or_else(|_| DEFAULT_BASE.to_owned());
    // A fresh room per run by default: participants only leave on a heartbeat
    // sweep, so reusing one code would walk the room into its 8-person cap.
    let mut room = format!("spike-{:08x}", seed());

    let mut argv = env::args().skip(1);
    while let Some(flag) = argv.next() {
        match flag.as_str() {
            "--base" => {
                if let Some(value) = argv.next() {
                    base = value;
                }
            }
            "--room" => {
                if let Some(value) = argv.next() {
                    room = value;
                }
            }
            other => eprintln!("ignoring unknown argument {other}"),
        }
    }

    Args {
        base: base.trim_end_matches('/').to_owned(),
        room,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let Args { base, room } = args();
    let signaling = Signaling {
        http: reqwest::Client::new(),
        base,
        room,
    };

    println!("room {} on {}", signaling.room, signaling.base);

    // --- speaker ---------------------------------------------------------
    println!("\n1. speaker joins");
    let speaker = signaling.join("speaker").await?;
    println!(
        "  session {} · {} ICE server(s)",
        speaker.sfu.session_id,
        speaker.sfu.ice_servers.len()
    );

    let (speaker_events, mut speaker_waits) = events("speaker");
    let speaker_peer = Box::pin(open_peer(
        ice_servers(&speaker.sfu.ice_servers),
        speaker_events,
    ))
    .await?;

    println!("\n2. speaker publishes `mic`");
    let published = publish_mic(
        &signaling,
        &speaker,
        &speaker_peer,
        &mut speaker_waits.gathered,
    )
    .await?;
    wait(
        &mut speaker_waits.connected,
        CONNECT_TIMEOUT,
        "the speaker to connect",
    )
    .await?;
    println!("  speaker connected — ICE + DTLS complete against the SFU");

    let streamer = tokio::spawn(async move {
        if let Err(error) = stream_tone(published).await {
            eprintln!("  !! tone streaming stopped: {error}");
        }
    });

    // --- listener --------------------------------------------------------
    println!("\n3. listener joins and reads the roster");
    let listener = signaling.join("listener").await?;
    let speaker_session = roster_lookup(&listener, &speaker.self_id)?;
    println!("  roster says speaker publishes `mic` from session {speaker_session}");

    let (listener_events, mut listener_waits) = events("listener");
    let listener_peer = Box::pin(open_peer(
        ice_servers(&listener.sfu.ice_servers),
        listener_events,
    ))
    .await?;

    println!("\n4. listener pulls `mic`");
    pull_mic(
        &signaling,
        &listener,
        &speaker_session,
        &listener_peer,
        &mut listener_waits.gathered,
    )
    .await?;
    wait(
        &mut listener_waits.connected,
        CONNECT_TIMEOUT,
        "the listener to connect",
    )
    .await?;

    let track = tokio::time::timeout(MEDIA_TIMEOUT, listener_waits.tracks.recv())
        .await
        .map_err(|_| anyhow!("no remote track arrived"))?
        .ok_or_else(|| anyhow!("track channel closed"))?;

    println!("\n5. listener decodes what arrived");
    let heard = listen(&track).await?;

    streamer.abort();
    speaker_peer.close().await?;
    listener_peer.close().await?;

    verdict(&heard)
}

/// Finds the speaker in the listener's own join answer. Nothing announced this
/// — the room read it out of the SFU proxy (DR-6), so a roster that carries it
/// is that design working end to end.
fn roster_lookup(listener: &JoinResponse, speaker_id: &str) -> Result<String> {
    let speaker = listener
        .participants
        .iter()
        .find(|participant| participant.id == speaker_id)
        .ok_or_else(|| anyhow!("the speaker is not in the listener's roster"))?;

    if !speaker
        .tracks
        .iter()
        .any(|track| track.name == "mic" && track.kind == "audio")
    {
        bail!(
            "roster shows no `mic` track for {} — the room did not learn the publish",
            speaker.name
        );
    }

    speaker
        .session_id
        .clone()
        .ok_or_else(|| anyhow!("the speaker has no sessionId in the roster"))
}

fn verdict(heard: &Heard) -> Result<()> {
    println!(
        "\n  {} RTP packets · {} decoded frames · RMS {:.0}",
        heard.packets, heard.decoded_frames, heard.rms
    );
    println!(
        "  {TONE_HZ} Hz energy {:.3e} vs {OFF_TONE_HZ} Hz energy {:.3e}",
        heard.tone_energy, heard.off_tone_energy
    );

    if heard.decoded_frames == 0 {
        bail!("packets arrived but none decoded as Opus");
    }
    if heard.tone_energy <= heard.off_tone_energy * 10.0 {
        bail!("the decoded audio is not the tone that was sent");
    }

    println!("\nPASS — webrtc-rs published to the Realtime SFU and pulled it back.");
    Ok(())
}
