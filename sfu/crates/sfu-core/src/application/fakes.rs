//! Test doubles for the application ports.

use std::cell::RefCell;

use crate::application::events::SfuEvent;
use crate::application::ports::{EventPublisher, LocalDescription, MediaRouter, RouterError};
use crate::domain::session::{IceCredentials, SessionId, SessionSpec};
use crate::domain::sink_wants::SinkWants;

#[derive(Debug, Default)]
pub struct FakeRouter {
    pub fail_open: bool,
    pub known_only: bool,
    pub opened: Vec<(SessionId, Vec<SessionId>)>,
    pub closed: Vec<(SessionId, Vec<SessionId>)>,
    pub wants: Vec<(SessionId, SinkWants)>,
}

impl MediaRouter for FakeRouter {
    fn open_session(
        &mut self,
        spec: SessionSpec,
        peers: &[SessionId],
    ) -> Result<LocalDescription, RouterError> {
        if self.fail_open {
            return Err(RouterError::Rejected("boom".into()));
        }
        self.opened.push((spec.id, peers.to_vec()));
        Ok(LocalDescription {
            ice: IceCredentials {
                ufrag: "local".into(),
                pwd: "localpasswordlocalpassword".into(),
            },
            fingerprint: "sha-256 00:11".into(),
        })
    }

    fn close_session(&mut self, id: &SessionId, remaining_peers: &[SessionId]) {
        self.closed.push((id.clone(), remaining_peers.to_vec()));
    }

    fn set_publish_permissions(
        &mut self,
        id: &SessionId,
        _update: crate::domain::permission_lease::PermissionUpdate,
    ) -> Result<(), RouterError> {
        if self.opened.iter().any(|(known, _)| known == id) {
            Ok(())
        } else {
            Err(RouterError::UnknownSession(id.clone()))
        }
    }

    fn set_publish_audio(&mut self, id: &SessionId, _allowed: bool) -> Result<(), RouterError> {
        if self.opened.iter().any(|(known, _)| known == id) {
            Ok(())
        } else {
            Err(RouterError::UnknownSession(id.clone()))
        }
    }

    fn set_publish_video(&mut self, id: &SessionId, _allowed: bool) -> Result<(), RouterError> {
        if self.opened.iter().any(|(known, _)| known == id) {
            Ok(())
        } else {
            Err(RouterError::UnknownSession(id.clone()))
        }
    }

    fn set_sink_wants(&mut self, id: &SessionId, wants: SinkWants) -> Result<(), RouterError> {
        let known = self.opened.iter().any(|(s, _)| s == id);
        if self.known_only && !known {
            return Err(RouterError::UnknownSession(id.clone()));
        }
        self.wants.push((id.clone(), wants));
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct RecordingPublisher {
    events: RefCell<Vec<SfuEvent>>,
}

impl RecordingPublisher {
    pub fn events(&self) -> Vec<SfuEvent> {
        self.events.borrow().clone()
    }
}

impl EventPublisher for RecordingPublisher {
    fn publish(&self, event: SfuEvent) {
        self.events.borrow_mut().push(event);
    }
}
