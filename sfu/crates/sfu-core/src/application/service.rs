//! Command dispatcher owning the room registry, the router and the event publisher.

use crate::application::close_session::close_session;
use crate::application::create_session::create_session;
use crate::application::error::AppError;
use crate::application::events::CloseReason;
use crate::application::ports::{EventPublisher, LocalDescription, MediaRouter};
use crate::application::set_sink_wants::set_sink_wants;
use crate::application::sync_sessions::sync_sessions;
use crate::domain::room::RoomRegistry;
use crate::domain::session::{SessionId, SessionSpec};

/// A command from signaling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Create a session.
    CreateSession(Box<SessionSpec>),
    /// Close a session.
    CloseSession(SessionId),
    /// Apply MEDIA_SINK_WANTS.
    SetSinkWants {
        /// Subscriber.
        session_id: SessionId,
        /// Publisher SSRC to quality.
        wants: Vec<(u32, u8)>,
    },
    /// Change audio forwarding permission.
    SetPublishAudio {
        /// Session to update.
        session_id: SessionId,
        /// Whether publishing is allowed.
        allowed: bool,
    },
    /// Reconcile sessions after a control reconnect.
    Sync(Vec<SessionId>),
}

/// Successful result of a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Session created.
    Created(LocalDescription),
    /// Close processed; false if the session was unknown.
    Closed(bool),
    /// Wants applied.
    WantsApplied,
    /// Audio publishing permission applied.
    PublishAudioApplied,
    /// Sessions to replay.
    Synced(Vec<SessionId>),
}

/// Application service of the SFU.
#[derive(Debug)]
pub struct SfuService<R, P> {
    router: R,
    events: P,
    rooms: RoomRegistry,
}

impl<R: MediaRouter, P: EventPublisher> SfuService<R, P> {
    /// Wires the service.
    pub fn new(router: R, events: P) -> Self {
        Self {
            router,
            events,
            rooms: RoomRegistry::new(),
        }
    }

    /// Executes a signaling command.
    pub fn handle(&mut self, command: Command) -> Result<Outcome, AppError> {
        match command {
            Command::CreateSession(spec) => {
                create_session(&mut self.router, &mut self.rooms, &self.events, *spec)
                    .map(Outcome::Created)
            }
            Command::CloseSession(id) => Ok(Outcome::Closed(close_session(
                &mut self.router,
                &mut self.rooms,
                &self.events,
                &id,
                CloseReason::Requested,
            ))),
            Command::SetSinkWants { session_id, wants } => {
                set_sink_wants(&mut self.router, &session_id, wants).map(|()| Outcome::WantsApplied)
            }
            Command::SetPublishAudio {
                session_id,
                allowed,
            } => self
                .router
                .set_publish_audio(&session_id, allowed)
                .map(|()| Outcome::PublishAudioApplied)
                .map_err(Into::into),
            Command::Sync(known) => Ok(Outcome::Synced(sync_sessions(
                &mut self.router,
                &mut self.rooms,
                &self.events,
                &known,
            ))),
        }
    }

    /// Closes a session whose transport died.
    pub fn on_transport_closed(&mut self, id: &SessionId) {
        close_session(
            &mut self.router,
            &mut self.rooms,
            &self.events,
            id,
            CloseReason::Disconnected,
        );
    }

    /// Mutable access to the router, for the media loop.
    pub fn router_mut(&mut self) -> &mut R {
        &mut self.router
    }

    /// The event publisher.
    pub fn events(&self) -> &P {
        &self.events
    }

    /// Live sessions and non-empty rooms.
    pub fn occupancy(&self) -> (usize, usize) {
        (self.rooms.session_count(), self.rooms.room_count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::events::SfuEvent;
    use crate::application::fakes::{FakeRouter, RecordingPublisher};
    use crate::domain::session::fixtures::spec;

    fn service() -> SfuService<FakeRouter, RecordingPublisher> {
        SfuService::new(FakeRouter::default(), RecordingPublisher::default())
    }

    #[test]
    fn dispatches_full_lifecycle() {
        let mut svc = service();
        let created = svc
            .handle(Command::CreateSession(Box::new(spec("a", "r", 10))))
            .unwrap();
        assert!(matches!(created, Outcome::Created(_)));
        assert_eq!(svc.occupancy(), (1, 1));

        let wants = svc
            .handle(Command::SetSinkWants {
                session_id: "a".into(),
                wants: vec![(1, 50)],
            })
            .unwrap();
        assert_eq!(wants, Outcome::WantsApplied);

        assert_eq!(
            svc.handle(Command::CloseSession("a".into())).unwrap(),
            Outcome::Closed(true)
        );
        assert_eq!(svc.occupancy(), (0, 0));
    }

    #[test]
    fn transport_closure_publishes_disconnected() {
        let mut svc = service();
        svc.handle(Command::CreateSession(Box::new(spec("a", "r", 10))))
            .unwrap();
        svc.on_transport_closed(&"a".into());
        assert_eq!(
            svc.events().events().last(),
            Some(&SfuEvent::SessionClosed {
                session_id: "a".into(),
                reason: CloseReason::Disconnected
            })
        );
    }

    #[test]
    fn sync_returns_missing() {
        let mut svc = service();
        assert_eq!(
            svc.handle(Command::Sync(vec!["x".into()])).unwrap(),
            Outcome::Synced(vec!["x".into()])
        );
    }
}
