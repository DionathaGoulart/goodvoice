//! The Worker, as the client talks to it.
//!
//! Two channels, both defined by `server/src/protocol.ts`:
//!
//! - HTTP for the things that need an answer — `POST /rooms/:code/join` and the
//!   SFU proxy under `/rooms/:code/sfu/*` (DR-2: the app secret stays server
//!   side, so track negotiation is proxied rather than direct).
//! - a WebSocket for the roster, which the room pushes and the client never
//!   polls.
//!
//! Every type here mirrors one over there. When they disagree the server wins:
//! it validates with Zod at the boundary and trusts nothing this file sends.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use super::RtcError;

/// How often the client proves it is still there. The room evicts after 30 s
/// (`HEARTBEAT_TIMEOUT_MS`), so this leaves room for two to go missing.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// Depth of the roster and command queues. Roster pushes are small and rare;
/// anything deeper would only add staleness.
const CHANNEL_DEPTH: usize = 16;

// --- protocol --------------------------------------------------------------

/// One live track, as the rest of the room sees it.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PublishedTrack {
    pub name: String,
    pub kind: String,
}

/// A participant as broadcast to everyone in the room.
///
/// `session_id` plus a track name is the pull address for their media. Both
/// are deliberately visible: the proxy refuses to pull from a session outside
/// the room, so a roommate's session id grants nothing beyond the audio you
/// joined to hear (DR-6).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Participant {
    pub id: String,
    pub name: String,
    #[serde(rename = "joinedAt")]
    pub joined_at: u64,
    pub muted: bool,
    pub deafened: bool,
    pub sharing: bool,
    /// Their Realtime session; `None` until the credential exchange lands.
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub tracks: Vec<PublishedTrack>,
}

impl Participant {
    /// Whether this participant is publishing a track called `name`.
    #[must_use]
    pub fn publishes(&self, name: &str) -> bool {
        self.tracks.iter().any(|track| track.name == name)
    }
}

/// Everything the client needs to reach the SFU, and nothing more.
#[derive(Debug, Clone, Deserialize)]
pub struct SfuCredentials {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "iceServers")]
    pub ice_servers: Vec<IceServer>,
    #[serde(rename = "maxAudioBitrate")]
    pub max_audio_bitrate: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IceServer {
    pub urls: Urls,
    pub username: Option<String>,
    pub credential: Option<String>,
}

/// Cloudflare answers `urls` as either a string or a list; both are legal ICE.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Urls {
    One(String),
    Many(Vec<String>),
}

impl Urls {
    #[must_use]
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            Self::One(url) => vec![url.clone()],
            Self::Many(urls) => urls.clone(),
        }
    }
}

/// What `POST /rooms/:code/join` answers with.
#[derive(Debug, Clone, Deserialize)]
pub struct JoinResponse {
    #[serde(rename = "self")]
    pub self_id: String,
    pub participants: Vec<Participant>,
    pub sfu: SfuCredentials,
}

/// What the room pushes down the WebSocket.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ServerMessage {
    Welcome {
        #[serde(rename = "self")]
        self_id: String,
        participants: Vec<Participant>,
    },
    Roster {
        participants: Vec<Participant>,
    },
    Error {
        code: String,
        message: String,
    },
}

/// What the client may send back.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientMessage {
    Heartbeat,
    Mute { muted: bool },
    Deafen { deafened: bool },
    Share { sharing: bool },
    Leave,
}

/// The three Realtime operations the Worker will sign, each pinned to the one
/// method it accepts. Mirrors the server's allowlist; anything else is a 400
/// before Cloudflare ever sees it.
#[derive(Debug, Clone, Copy)]
pub enum SfuOperation {
    TracksNew,
    Renegotiate,
    TracksClose,
}

impl SfuOperation {
    const fn path(self) -> &'static str {
        match self {
            Self::TracksNew => "tracks/new",
            Self::Renegotiate => "renegotiate",
            Self::TracksClose => "tracks/close",
        }
    }

    fn method(self) -> reqwest::Method {
        match self {
            Self::TracksNew => reqwest::Method::POST,
            Self::Renegotiate | Self::TracksClose => reqwest::Method::PUT,
        }
    }
}

// --- the client ------------------------------------------------------------

/// A handle on one room at one deployment.
pub struct Signaling {
    http: reqwest::Client,
    base: String,
    room: String,
}

impl Signaling {
    /// Points the client at `base` (the Worker's origin) and a room code.
    ///
    /// # Errors
    ///
    /// Returns [`RtcError::Http`] if the HTTP client cannot be built.
    pub fn new(base: &str, room: &str) -> Result<Self, RtcError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .map_err(|error| RtcError::Http(error.to_string()))?,
            base: base.trim_end_matches('/').to_owned(),
            room: room.to_owned(),
        })
    }

    #[must_use]
    pub fn room(&self) -> &str {
        &self.room
    }

    /// Takes a seat in the room and collects SFU credentials for it.
    ///
    /// # Errors
    ///
    /// Returns [`RtcError::JoinRejected`] when the room refuses (full, bad
    /// code, SFU unavailable) and [`RtcError::Http`] when it cannot be reached.
    pub async fn join(&self, name: &str) -> Result<JoinResponse, RtcError> {
        let response = self
            .http
            .post(format!("{}/rooms/{}/join", self.base, self.room))
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await
            .map_err(|error| RtcError::Http(error.to_string()))?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(RtcError::JoinRejected(describe(status, &body)));
        }
        serde_json::from_str(&body).map_err(|error| RtcError::Protocol(error.to_string()))
    }

    /// Forwards one track-negotiation call through the Worker's proxy.
    ///
    /// # Errors
    ///
    /// Returns [`RtcError::Sfu`] when the room or Cloudflare refuses, and
    /// [`RtcError::Http`] when the Worker cannot be reached.
    pub async fn sfu(
        &self,
        participant: &str,
        operation: SfuOperation,
        body: &Value,
    ) -> Result<Value, RtcError> {
        let url = format!(
            "{}/rooms/{}/sfu/{}?p={participant}",
            self.base,
            self.room,
            operation.path()
        );
        let response = self
            .http
            .request(operation.method(), url)
            .json(body)
            .send()
            .await
            .map_err(|error| RtcError::Http(error.to_string()))?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(RtcError::Sfu(describe(status, &text)));
        }
        serde_json::from_str(&text).map_err(|error| RtcError::Protocol(error.to_string()))
    }

    /// Opens the roster WebSocket and starts the heartbeat.
    ///
    /// The returned receiver carries every roster push; the sender carries
    /// mute, deafen, share and leave. Dropping either end closes the socket,
    /// after which the room evicts this participant on the next sweep.
    ///
    /// # Errors
    ///
    /// Returns [`RtcError::Http`] if the upgrade fails.
    pub async fn connect(
        &self,
        participant: &str,
    ) -> Result<(mpsc::Sender<ClientMessage>, mpsc::Receiver<ServerMessage>), RtcError> {
        let url = format!(
            "{}/rooms/{}/ws?p={participant}",
            websocket_origin(&self.base),
            self.room
        );

        let (socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .map_err(|error| RtcError::Http(error.to_string()))?;
        let (mut writer, mut reader) = socket.split();

        let (outbound_tx, mut outbound_rx) = mpsc::channel::<ClientMessage>(CHANNEL_DEPTH);
        let (inbound_tx, inbound_rx) = mpsc::channel::<ServerMessage>(CHANNEL_DEPTH);

        // One task owns the write half so the heartbeat and the UI's commands
        // cannot interleave halfway through a frame.
        tokio::spawn(async move {
            let mut beat = tokio::time::interval(HEARTBEAT_INTERVAL);
            loop {
                let message = tokio::select! {
                    () = async { beat.tick().await; } => ClientMessage::Heartbeat,
                    command = outbound_rx.recv() => match command {
                        Some(command) => command,
                        None => break,
                    },
                };

                let leaving = matches!(message, ClientMessage::Leave);
                let Ok(text) = serde_json::to_string(&message) else {
                    continue;
                };
                if writer.send(Message::text(text)).await.is_err() {
                    break;
                }
                if leaving {
                    break;
                }
            }
            let _ = writer.close().await;
        });

        tokio::spawn(async move {
            while let Some(Ok(message)) = reader.next().await {
                let Message::Text(text) = message else {
                    continue;
                };
                let Ok(parsed) = serde_json::from_str::<ServerMessage>(&text) else {
                    // An unknown message is the server being newer than this
                    // client, not a reason to drop the call.
                    continue;
                };
                if inbound_tx.send(parsed).await.is_err() {
                    break;
                }
            }
        });

        Ok((outbound_tx, inbound_rx))
    }
}

/// `http` → `ws`, `https` → `wss`; anything else is passed through so a caller
/// may hand in a `ws://` origin directly.
fn websocket_origin(base: &str) -> String {
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_owned()
    }
}

/// Turns the server's typed error body back into one line. The room answers
/// `{ error, message }`; anything else is quoted as-is rather than guessed at.
fn describe(status: reqwest::StatusCode, body: &str) -> String {
    #[derive(Deserialize)]
    struct RoomErrorBody {
        error: String,
        message: String,
    }

    serde_json::from_str::<RoomErrorBody>(body).map_or_else(
        |_| format!("{status}: {body}"),
        |parsed| format!("{} ({})", parsed.message, parsed.error),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        websocket_origin, ClientMessage, Participant, PublishedTrack, ServerMessage, SfuOperation,
    };

    #[test]
    fn websocket_origins_follow_the_http_scheme() {
        assert_eq!(
            websocket_origin("https://goodvoice.workers.dev"),
            "wss://goodvoice.workers.dev"
        );
        assert_eq!(
            websocket_origin("http://localhost:8787"),
            "ws://localhost:8787"
        );
        // Already a WebSocket origin: leave it alone rather than mangling it.
        assert_eq!(
            websocket_origin("ws://localhost:8787"),
            "ws://localhost:8787"
        );
    }

    #[test]
    fn sfu_operations_keep_the_methods_the_room_allowlists() {
        assert_eq!(SfuOperation::TracksNew.method(), reqwest::Method::POST);
        assert_eq!(SfuOperation::Renegotiate.method(), reqwest::Method::PUT);
        assert_eq!(SfuOperation::TracksClose.method(), reqwest::Method::PUT);
        assert_eq!(SfuOperation::TracksNew.path(), "tracks/new");
    }

    #[test]
    fn client_messages_serialise_the_way_the_room_parses_them() {
        let encode = |message: &ClientMessage| serde_json::to_string(message).expect("json");

        assert_eq!(encode(&ClientMessage::Heartbeat), r#"{"type":"heartbeat"}"#);
        assert_eq!(
            encode(&ClientMessage::Mute { muted: true }),
            r#"{"type":"mute","muted":true}"#
        );
        assert_eq!(
            encode(&ClientMessage::Deafen { deafened: false }),
            r#"{"type":"deafen","deafened":false}"#
        );
        assert_eq!(encode(&ClientMessage::Leave), r#"{"type":"leave"}"#);
    }

    #[test]
    fn a_roster_push_parses_into_pull_addresses() {
        let raw = r#"{
            "type": "roster",
            "participants": [{
                "id": "abc",
                "name": "rafael",
                "joinedAt": 1,
                "muted": false,
                "deafened": false,
                "sharing": false,
                "sessionId": "sess-1",
                "tracks": [{ "name": "mic", "kind": "audio" }]
            }]
        }"#;

        let ServerMessage::Roster { participants } =
            serde_json::from_str::<ServerMessage>(raw).expect("roster")
        else {
            panic!("expected a roster push");
        };

        let peer = &participants[0];
        assert_eq!(peer.session_id.as_deref(), Some("sess-1"));
        assert!(peer.publishes("mic"));
        assert!(!peer.publishes("screen"));
        assert_eq!(
            peer.tracks,
            vec![PublishedTrack {
                name: "mic".to_owned(),
                kind: "audio".to_owned()
            }]
        );
    }

    #[test]
    fn a_participant_without_a_session_is_not_yet_pullable() {
        // `join()` announces a participant before their SFU session exists, so
        // this shape is normal and must not be an error.
        let raw = r#"{
            "id": "abc", "name": "rafael", "joinedAt": 1,
            "muted": false, "deafened": false, "sharing": false,
            "sessionId": null, "tracks": []
        }"#;

        let peer = serde_json::from_str::<Participant>(raw).expect("participant");
        assert!(peer.session_id.is_none());
        assert!(!peer.publishes("mic"));
    }

    #[test]
    fn a_welcome_carries_the_same_roster_as_a_push() {
        let raw = r#"{"type":"welcome","self":"me","participants":[]}"#;
        let ServerMessage::Welcome {
            self_id,
            participants,
        } = serde_json::from_str::<ServerMessage>(raw).expect("welcome")
        else {
            panic!("expected a welcome");
        };
        assert_eq!(self_id, "me");
        assert!(participants.is_empty());
    }
}
