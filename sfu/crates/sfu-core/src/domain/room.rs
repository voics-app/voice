//! Room membership: which sessions exchange media with each other.

use std::collections::{BTreeSet, HashMap};

use super::session::{RoomId, SessionId};

/// Registry of rooms and their member sessions.
#[derive(Debug, Default, Clone)]
pub struct RoomRegistry {
    rooms: HashMap<RoomId, BTreeSet<SessionId>>,
    membership: HashMap<SessionId, RoomId>,
}

/// Errors when changing membership.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RoomError {
    /// The session is already in a room.
    #[error("session {0} already joined a room")]
    AlreadyJoined(SessionId),
}

impl RoomRegistry {
    /// Creates an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `session` to `room` and returns the members that were already there.
    pub fn join(&mut self, room: RoomId, session: SessionId) -> Result<Vec<SessionId>, RoomError> {
        if self.membership.contains_key(&session) {
            return Err(RoomError::AlreadyJoined(session));
        }
        let members = self.rooms.entry(room.clone()).or_default();
        let existing = members.iter().cloned().collect();
        members.insert(session.clone());
        self.membership.insert(session, room);
        Ok(existing)
    }

    /// Removes `session` and returns the members left in its room (empty rooms are dropped).
    pub fn leave(&mut self, session: &SessionId) -> Vec<SessionId> {
        let Some(room) = self.membership.remove(session) else {
            return Vec::new();
        };
        let Some(members) = self.rooms.get_mut(&room) else {
            return Vec::new();
        };
        members.remove(session);
        let rest: Vec<SessionId> = members.iter().cloned().collect();
        if rest.is_empty() {
            self.rooms.remove(&room);
        }
        rest
    }

    /// Other members of `session`'s room.
    pub fn peers_of(&self, session: &SessionId) -> Vec<SessionId> {
        self.membership
            .get(session)
            .and_then(|room| self.rooms.get(room))
            .map(|members| members.iter().filter(|m| *m != session).cloned().collect())
            .unwrap_or_default()
    }

    /// Number of non-empty rooms.
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// Number of sessions across all rooms.
    pub fn session_count(&self) -> usize {
        self.membership.len()
    }

    /// All sessions.
    pub fn sessions(&self) -> impl Iterator<Item = &SessionId> {
        self.membership.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str) -> SessionId {
        id.into()
    }

    #[test]
    fn join_returns_existing_members() {
        let mut r = RoomRegistry::new();
        assert_eq!(r.join("room".into(), s("a")), Ok(vec![]));
        assert_eq!(r.join("room".into(), s("b")), Ok(vec![s("a")]));
        assert_eq!(r.join("other".into(), s("c")), Ok(vec![]));
        assert_eq!(r.room_count(), 2);
        assert_eq!(r.session_count(), 3);
    }

    #[test]
    fn double_join_is_rejected() {
        let mut r = RoomRegistry::new();
        r.join("room".into(), s("a")).unwrap();
        assert_eq!(
            r.join("x".into(), s("a")),
            Err(RoomError::AlreadyJoined(s("a")))
        );
    }

    #[test]
    fn peers_exclude_self_and_other_rooms() {
        let mut r = RoomRegistry::new();
        r.join("room".into(), s("a")).unwrap();
        r.join("room".into(), s("b")).unwrap();
        r.join("other".into(), s("c")).unwrap();
        assert_eq!(r.peers_of(&s("a")), vec![s("b")]);
        assert_eq!(r.peers_of(&s("c")), Vec::<SessionId>::new());
        assert_eq!(r.peers_of(&s("zzz")), Vec::<SessionId>::new());
    }

    #[test]
    fn leave_returns_remaining_and_drops_empty_rooms() {
        let mut r = RoomRegistry::new();
        r.join("room".into(), s("a")).unwrap();
        r.join("room".into(), s("b")).unwrap();
        assert_eq!(r.leave(&s("a")), vec![s("b")]);
        assert_eq!(r.leave(&s("b")), Vec::<SessionId>::new());
        assert_eq!(r.room_count(), 0);
        assert_eq!(r.leave(&s("b")), Vec::<SessionId>::new());
    }
}
