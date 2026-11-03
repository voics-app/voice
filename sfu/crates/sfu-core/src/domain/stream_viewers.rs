//! Who watches each publisher's screen share, so changes can be reported to the streamer.

use std::collections::{BTreeSet, HashMap};

use crate::domain::session::{SessionId, UserId};

/// The last reported viewer set per publisher.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamViewers {
    by_publisher: HashMap<SessionId, BTreeSet<UserId>>,
}

impl StreamViewers {
    /// Creates a registry where nobody watches anything.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the current viewers of `publisher`; returns the new set when it changed.
    pub fn update(
        &mut self,
        publisher: &SessionId,
        viewers: BTreeSet<UserId>,
    ) -> Option<Vec<UserId>> {
        let previous = self.by_publisher.get(publisher);
        if previous.map_or(viewers.is_empty(), |p| *p == viewers) {
            return None;
        }
        let reported = viewers.iter().cloned().collect();
        if viewers.is_empty() {
            self.by_publisher.remove(publisher);
        } else {
            self.by_publisher.insert(publisher.clone(), viewers);
        }
        Some(reported)
    }

    /// Forgets a publisher that left.
    pub fn remove(&mut self, publisher: &SessionId) {
        self.by_publisher.remove(publisher);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn users(ids: &[&str]) -> BTreeSet<UserId> {
        ids.iter().map(|id| UserId::from(*id)).collect()
    }

    #[test]
    fn first_viewer_is_reported() {
        let mut v = StreamViewers::new();
        assert_eq!(
            v.update(&"p".into(), users(&["u2", "u1"])),
            Some(vec!["u1".into(), "u2".into()])
        );
    }

    #[test]
    fn unchanged_set_is_not_reported() {
        let mut v = StreamViewers::new();
        v.update(&"p".into(), users(&["u1"]));
        assert_eq!(v.update(&"p".into(), users(&["u1"])), None);
    }

    #[test]
    fn nobody_watching_initially_is_not_reported() {
        let mut v = StreamViewers::new();
        assert_eq!(v.update(&"p".into(), users(&[])), None);
    }

    #[test]
    fn last_viewer_leaving_reports_an_empty_list() {
        let mut v = StreamViewers::new();
        v.update(&"p".into(), users(&["u1"]));
        assert_eq!(v.update(&"p".into(), users(&[])), Some(vec![]));
        assert_eq!(v.update(&"p".into(), users(&[])), None);
    }

    #[test]
    fn publishers_are_tracked_independently() {
        let mut v = StreamViewers::new();
        v.update(&"p1".into(), users(&["u1"]));
        assert_eq!(
            v.update(&"p2".into(), users(&["u1"])),
            Some(vec!["u1".into()])
        );
    }

    #[test]
    fn removed_publisher_starts_fresh() {
        let mut v = StreamViewers::new();
        v.update(&"p".into(), users(&["u1"]));
        v.remove(&"p".into());
        assert_eq!(
            v.update(&"p".into(), users(&["u1"])),
            Some(vec!["u1".into()])
        );
    }
}
