//! Use case: close a media session.

use crate::application::events::{CloseReason, SfuEvent};
use crate::application::ports::{EventPublisher, MediaRouter};
use crate::domain::room::RoomRegistry;
use crate::domain::session::SessionId;

/// Leaves the room, tears down the peer and publishes `SessionClosed`.
/// Returns false (and does nothing) for unknown sessions, so closing is idempotent.
pub fn close_session(
    router: &mut dyn MediaRouter,
    rooms: &mut RoomRegistry,
    events: &dyn EventPublisher,
    id: &SessionId,
    reason: CloseReason,
) -> bool {
    if !rooms.sessions().any(|s| s == id) {
        return false;
    }
    let remaining = rooms.leave(id);
    router.close_session(id, &remaining);
    events.publish(SfuEvent::SessionClosed {
        session_id: id.clone(),
        reason,
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::create_session::create_session;
    use crate::application::fakes::{FakeRouter, RecordingPublisher};
    use crate::domain::session::fixtures::spec;

    #[test]
    fn closes_peer_and_tells_router_who_remains() {
        let mut router = FakeRouter::default();
        let mut rooms = RoomRegistry::new();
        let events = RecordingPublisher::default();
        create_session(&mut router, &mut rooms, &events, spec("a", "room", 10)).unwrap();
        create_session(&mut router, &mut rooms, &events, spec("b", "room", 20)).unwrap();

        let closed = close_session(
            &mut router,
            &mut rooms,
            &events,
            &"a".into(),
            CloseReason::Requested,
        );

        assert!(closed);
        assert_eq!(router.closed, vec![("a".into(), vec!["b".into()])]);
        assert_eq!(
            events.events().last(),
            Some(&SfuEvent::SessionClosed {
                session_id: "a".into(),
                reason: CloseReason::Requested
            })
        );
    }

    #[test]
    fn closing_unknown_session_is_a_no_op() {
        let mut router = FakeRouter::default();
        let mut rooms = RoomRegistry::new();
        let events = RecordingPublisher::default();

        let closed = close_session(
            &mut router,
            &mut rooms,
            &events,
            &"x".into(),
            CloseReason::Disconnected,
        );

        assert!(!closed);
        assert!(router.closed.is_empty());
        assert!(events.events().is_empty());
    }
}
