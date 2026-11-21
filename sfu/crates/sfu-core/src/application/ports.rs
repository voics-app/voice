//! Ports driven by the use cases.

use crate::application::events::SfuEvent;
use crate::domain::session::{IceCredentials, SessionId, SessionSpec};
use crate::domain::sink_wants::SinkWants;

/// What the SFU tells the client about its side of the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDescription {
    /// SFU ICE credentials.
    pub ice: IceCredentials,
    /// SFU DTLS fingerprint.
    pub fingerprint: String,
}

/// Errors from the media router.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouterError {
    /// The session id is taken.
    #[error("session {0} already exists")]
    DuplicateSession(SessionId),
    /// No such session.
    #[error("unknown session {0}")]
    UnknownSession(SessionId),
    /// The media stack refused the parameters.
    #[error("media engine rejected the session: {0}")]
    Rejected(String),
}

/// Media plumbing: one WebRTC peer per session, wired to its room peers.
pub trait MediaRouter {
    /// Creates the peer for `spec` and connects it with `peers` (both directions).
    fn open_session(
        &mut self,
        spec: SessionSpec,
        peers: &[SessionId],
    ) -> Result<LocalDescription, RouterError>;

    /// Tears the peer down and removes its tracks from `remaining_peers`.
    fn close_session(&mut self, id: &SessionId, remaining_peers: &[SessionId]);

    /// Changes the audio publishing permission of an existing session.
    fn set_publish_audio(&mut self, id: &SessionId, allowed: bool) -> Result<(), RouterError>;

    /// Changes camera and screen forwarding permission for a session.
    fn set_publish_video(&mut self, id: &SessionId, allowed: bool) -> Result<(), RouterError>;

    /// Atomically applies a bounded capability update.
    fn set_publish_permissions(
        &mut self,
        id: &SessionId,
        update: crate::domain::permission_lease::PermissionUpdate,
    ) -> Result<(), RouterError>;

    /// Applies MEDIA_SINK_WANTS for a subscriber.
    fn set_sink_wants(&mut self, id: &SessionId, wants: SinkWants) -> Result<(), RouterError>;
}

/// Publishes events to signaling.
pub trait EventPublisher {
    /// Publishes one event. Must not block.
    fn publish(&self, event: SfuEvent);
}
