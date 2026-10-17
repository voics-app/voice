//! Use case: reconcile the SFU's sessions with the list signaling holds.
//!
//! Signaling sends `sync` whenever its control connection (re)connects. Sessions the SFU has
//! but signaling does not are orphans (signaling restarted) and get closed. Sessions signaling
//! has but the SFU does not (the SFU restarted) are reported as missing, so signaling replays
//! `create_session` for them and state is rebuilt without the clients rejoining.

use std::collections::HashSet;

use crate::application::close_session::close_session;
use crate::application::events::{CloseReason, SfuEvent};
use crate::application::ports::{EventPublisher, MediaRouter};
use crate::domain::room::RoomRegistry;
use crate::domain::session::SessionId;

/// Closes orphans and publishes `Synced` with the sessions to replay.
pub fn sync_sessions(
    router: &mut dyn MediaRouter,
    rooms: &mut RoomRegistry,
    events: &dyn EventPublisher,
    known_to_signaling: &[SessionId],
) -> Vec<SessionId> {
    let wanted: HashSet<&SessionId> = known_to_signaling.iter().collect();
    let orphans: Vec<SessionId> = rooms
        .sessions()
        .filter(|s| !wanted.contains(s))
        .cloned()
        .collect();
    for orphan in &orphans {
        close_session(router, rooms, events, orphan, CloseReason::NotInSync);
    }
    let present: HashSet<&SessionId> = rooms.sessions().collect();
    let missing: Vec<SessionId> = known_to_signaling
        .iter()
        .filter(|s| !present.contains(s))
        .cloned()
        .collect();
    events.publish(SfuEvent::Synced {
        missing: missing.clone(),
    });
    missing
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::create_session::create_session;
    use crate::application::fakes::{FakeRouter, RecordingPublisher};
    use crate::domain::session::fixtures::spec;

    #[test]
    fn closes_orphans_and_reports_missing() {
        let mut router = FakeRouter::default();
        let mut rooms = RoomRegistry::new();
        let events = RecordingPublisher::default();
        create_session(&mut router, &mut rooms, &events, spec("keep", "r", 10)).unwrap();
        create_session(&mut router, &mut rooms, &events, spec("orphan", "r", 20)).unwrap();

        let missing = sync_sessions(
            &mut router,
            &mut rooms,
            &events,
            &["keep".into(), "lost".into()],
        );

        assert_eq!(missing, vec![SessionId::from("lost")]);
        assert_eq!(router.closed.len(), 1);
        assert_eq!(router.closed[0].0, SessionId::from("orphan"));
        assert_eq!(
            events.events().last(),
            Some(&SfuEvent::Synced {
                missing: vec!["lost".into()]
            })
        );
    }

    #[test]
    fn fresh_sfu_reports_everything_missing() {
        let mut router = FakeRouter::default();
        let mut rooms = RoomRegistry::new();
        let events = RecordingPublisher::default();
        let missing = sync_sessions(&mut router, &mut rooms, &events, &["a".into(), "b".into()]);
        assert_eq!(missing.len(), 2);
    }
}
