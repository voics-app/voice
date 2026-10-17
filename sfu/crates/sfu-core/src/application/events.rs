//! Events the SFU reports to signaling.

use crate::domain::session::{IceCredentials, SessionId};

/// Why a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// Signaling asked for it.
    Requested,
    /// ICE consent expired or DTLS closed.
    Disconnected,
    /// Signaling did not list the session in a `sync`.
    NotInSync,
}

impl CloseReason {
    /// Wire name of the reason.
    pub fn as_str(self) -> &'static str {
        match self {
            CloseReason::Requested => "requested",
            CloseReason::Disconnected => "disconnected",
            CloseReason::NotInSync => "not_in_sync",
        }
    }
}

/// Node-level traffic statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NodeStats {
    /// Live sessions.
    pub sessions: usize,
    /// Non-empty rooms.
    pub rooms: usize,
    /// Inbound UDP bitrate.
    pub ingress_bps: u64,
    /// Outbound UDP bitrate.
    pub egress_bps: u64,
}

/// Event published to signaling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SfuEvent {
    /// A session is ready; the client can now connect.
    SessionCreated {
        /// Session id.
        session_id: SessionId,
        /// SFU ICE credentials.
        local_ice: IceCredentials,
        /// SFU DTLS fingerprint.
        fingerprint: String,
    },
    /// A session ended.
    SessionClosed {
        /// Session id.
        session_id: SessionId,
        /// Why.
        reason: CloseReason,
    },
    /// A publisher started or stopped speaking.
    Speaking {
        /// Publisher session.
        session_id: SessionId,
        /// Its audio SSRC.
        ssrc: u32,
        /// New state.
        speaking: bool,
    },
    /// Periodic statistics.
    Stats(NodeStats),
    /// Reply to `sync`: sessions signaling knows about that the SFU does not have.
    Synced {
        /// Sessions to replay with `create_session`.
        missing: Vec<SessionId>,
    },
}
