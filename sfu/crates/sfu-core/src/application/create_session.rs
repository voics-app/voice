//! Use case: create a media session and wire it into its room.

use crate::application::error::AppError;
use crate::application::events::SfuEvent;
use crate::application::ports::{EventPublisher, LocalDescription, MediaRouter};
use crate::domain::room::RoomRegistry;
use crate::domain::session::SessionSpec;

/// Validates `spec`, joins its room and opens the peer. Publishes `SessionCreated`.
pub fn create_session(
    router: &mut dyn MediaRouter,
    rooms: &mut RoomRegistry,
    events: &dyn EventPublisher,
    spec: SessionSpec,
) -> Result<LocalDescription, AppError> {
    spec.validate()?;
    let id = spec.id.clone();
    let peers = rooms.join(spec.room.clone(), id.clone())?;
    let local = match router.open_session(spec, &peers) {
        Ok(local) => local,
        Err(err) => {
            rooms.leave(&id);
            return Err(err.into());
        }
    };
    events.publish(SfuEvent::SessionCreated {
        session_id: id,
        local_ice: local.ice.clone(),
        fingerprint: local.fingerprint.clone(),
    });
    Ok(local)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeRouter, RecordingPublisher};
    use crate::application::ports::RouterError;
    use crate::domain::room::RoomError;
    use crate::domain::session::SessionSpecError;
    use crate::domain::session::fixtures::spec;

    #[test]
    fn opens_peer_with_existing_room_members_and_publishes() {
        let mut router = FakeRouter::default();
        let mut rooms = RoomRegistry::new();
        let events = RecordingPublisher::default();

        create_session(&mut router, &mut rooms, &events, spec("a", "room", 10)).unwrap();
        let local =
            create_session(&mut router, &mut rooms, &events, spec("b", "room", 20)).unwrap();

        assert_eq!(
            router.opened,
            vec![("a".into(), vec![]), ("b".into(), vec!["a".into()])]
        );
        assert_eq!(local.fingerprint, "sha-256 00:11");
        assert_eq!(events.events().len(), 2);
        assert!(matches!(
            &events.events()[1],
            SfuEvent::SessionCreated { session_id, .. } if session_id.0 == "b"
        ));
    }

    #[test]
    fn invalid_spec_is_rejected_before_touching_rooms() {
        let mut router = FakeRouter::default();
        let mut rooms = RoomRegistry::new();
        let events = RecordingPublisher::default();
        let mut bad = spec("a", "room", 10);
        bad.audio_ssrc = 0;

        let err = create_session(&mut router, &mut rooms, &events, bad).unwrap_err();

        assert_eq!(err, AppError::InvalidSession(SessionSpecError::ZeroSsrc));
        assert_eq!(rooms.session_count(), 0);
        assert!(events.events().is_empty());
    }

    #[test]
    fn duplicate_session_is_rejected() {
        let mut router = FakeRouter::default();
        let mut rooms = RoomRegistry::new();
        let events = RecordingPublisher::default();
        create_session(&mut router, &mut rooms, &events, spec("a", "room", 10)).unwrap();

        let err =
            create_session(&mut router, &mut rooms, &events, spec("a", "room", 30)).unwrap_err();

        assert_eq!(err, AppError::Room(RoomError::AlreadyJoined("a".into())));
    }

    #[test]
    fn router_failure_rolls_back_room_membership() {
        let mut router = FakeRouter {
            fail_open: true,
            ..FakeRouter::default()
        };
        let mut rooms = RoomRegistry::new();
        let events = RecordingPublisher::default();

        let err =
            create_session(&mut router, &mut rooms, &events, spec("a", "room", 10)).unwrap_err();

        assert_eq!(err, AppError::Router(RouterError::Rejected("boom".into())));
        assert_eq!(rooms.session_count(), 0);
        assert!(events.events().is_empty());
    }
}
