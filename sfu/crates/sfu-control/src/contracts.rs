//! Wire shapes of the control protocol, isolated here so they can be swapped for the generated
//! `contracts` package. Field names match pinned interface P8.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// One line from signaling: an optional correlation id plus the command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Request {
    /// Echoed on the reply so signaling can correlate it.
    #[serde(default)]
    pub req: Option<String>,
    /// The command.
    #[serde(flatten)]
    pub command: WireCommand,
}

/// Signaling to SFU commands.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum WireCommand {
    /// Create a media session.
    CreateSession(WireSession),
    /// Close a media session.
    CloseSession {
        /// Session to close.
        session_id: String,
    },
    /// MEDIA_SINK_WANTS of a subscriber.
    SetSinkWants {
        /// Subscriber session.
        session_id: String,
        /// Publisher SSRC (as a string, JSON object key) to quality 0-100.
        wants: HashMap<String, u8>,
    },
    /// Change whether a session may publish audio.
    SetPublishAudio {
        /// Session to update.
        session_id: String,
        /// Whether to forward its audio.
        allowed: bool,
    },
    /// Change whether a session may publish camera and screen video.
    SetPublishVideo {
        /// Session to update.
        session_id: String,
        /// Whether to forward its video.
        allowed: bool,
    },
    /// Reconcile after a control reconnect.
    Sync {
        /// Every session signaling believes is live.
        session_ids: Vec<String>,
    },
}

/// `create_session` payload.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WireSession {
    /// Session id.
    pub session_id: String,
    /// Voice channel (media room).
    pub room_id: String,
    /// Owner.
    pub user_id: String,
    /// Client ICE username fragment.
    pub ice_ufrag: String,
    /// Client ICE password.
    pub ice_pwd: String,
    /// Client DTLS fingerprint (`sha-256 AB:CD:..`).
    pub remote_fingerprint: String,
    /// Local ICE credentials to reuse when replaying after an SFU restart.
    #[serde(default)]
    pub local_ufrag: Option<String>,
    /// See `local_ufrag`.
    #[serde(default)]
    pub local_pwd: Option<String>,
    /// SSRC of the publisher's audio.
    pub audio_ssrc: u32,
    /// Whether audio is allowed; absent for older signaling versions.
    #[serde(default = "default_publish_audio")]
    pub can_publish_audio: bool,
    /// Whether video is allowed; absent for older signaling versions.
    #[serde(default = "default_publish_audio")]
    pub can_publish_video: bool,
    /// Negotiated codecs.
    pub codecs: Vec<WireCodec>,
    /// Simulcast layers, ascending quality.
    #[serde(default)]
    pub video_layers: Vec<WireLayer>,
    /// The screen share stream, when the client sends one beside the camera.
    #[serde(default)]
    pub screen: Option<WireStream>,
}

fn default_publish_audio() -> bool {
    true
}

/// A single-layer stream (the screen share).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WireStream {
    /// Media SSRC.
    pub ssrc: u32,
    /// Retransmission SSRC, 0 when none.
    #[serde(default)]
    pub rtx_ssrc: u32,
}

/// A negotiated codec.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WireCodec {
    /// `opus`, `VP8`, `VP9`, `H264` or `AV1`.
    pub name: String,
    /// `audio` or `video`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Client preference.
    pub priority: u32,
    /// RTP payload type.
    pub payload_type: u8,
    /// RTX payload type (video).
    #[serde(default)]
    pub rtx_payload_type: Option<u8>,
}

/// A simulcast layer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WireLayer {
    /// RTP stream id.
    pub rid: String,
    /// Quality 0-100.
    pub quality: u8,
    /// Media SSRC.
    pub ssrc: u32,
    /// Retransmission SSRC.
    pub rtx_ssrc: u32,
}

/// SFU to signaling messages: replies (carry `req`) and unsolicited events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "evt", rename_all = "snake_case")]
pub enum WireEvent {
    /// Reply to `create_session`.
    SessionCreated {
        /// Correlation id.
        req: Option<String>,
        /// Session id.
        session_id: String,
        /// SFU ICE username fragment.
        local_ufrag: String,
        /// SFU ICE password.
        local_pwd: String,
        /// SFU DTLS fingerprint.
        fingerprint: String,
    },
    /// A session ended (also sent unsolicited when its transport dies).
    SessionClosed {
        /// Session id.
        session_id: String,
        /// `requested`, `disconnected` or `not_in_sync`.
        reason: String,
    },
    /// Reply to `close_session`.
    Closed {
        /// Correlation id.
        req: Option<String>,
        /// Session id.
        session_id: String,
        /// False when the session did not exist.
        existed: bool,
    },
    /// Reply to `set_sink_wants`.
    SinkWantsApplied {
        /// Correlation id.
        req: Option<String>,
        /// Session id.
        session_id: String,
    },
    /// Reply to `set_publish_audio`.
    PublishAudioApplied {
        /// Correlation id.
        req: Option<String>,
        /// Session id.
        session_id: String,
    },
    /// Reply to `set_publish_video`.
    PublishVideoApplied {
        /// Correlation id.
        req: Option<String>,
        /// Session id.
        session_id: String,
    },
    /// Reply to `sync`.
    Synced {
        /// Correlation id.
        req: Option<String>,
        /// Sessions signaling must replay with `create_session`.
        missing: Vec<String>,
    },
    /// A publisher started or stopped speaking.
    Speaking {
        /// Session id.
        session_id: String,
        /// Audio SSRC.
        ssrc: u32,
        /// New state.
        speaking: bool,
    },
    /// The users watching a publisher's screen share changed.
    StreamViewers {
        /// Publisher session id.
        session_id: String,
        /// Screen SSRC.
        ssrc: u32,
        /// Ids of every user now watching, sorted.
        viewers: Vec<String>,
    },
    /// Periodic node statistics.
    Stats {
        /// Live sessions.
        sessions: usize,
        /// Non-empty rooms.
        rooms: usize,
        /// Inbound bits per second.
        ingress_bps: u64,
        /// Outbound bits per second.
        egress_bps: u64,
    },
    /// A command failed.
    Error {
        /// Correlation id.
        req: Option<String>,
        /// Stable machine-readable code.
        code: String,
        /// Human-readable detail.
        message: String,
    },
}
