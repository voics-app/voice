//! Monotone, bounded publishing grants for guarded media sessions.
use std::time::{Duration, Instant};

/// Maximum lifetime of an individual grant. The gateway must renew while it remains authorized.
pub const MAX_GRANT_MS: u64 = 15_000;

/// The complete server-authorized capability set for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionUpdate {
    pub revision: u64,
    pub expires_at: u64,
    pub can_publish_audio: bool,
    pub can_publish_video: bool,
    pub can_publish_soundboard: bool,
}

/// A granted capability, guarded by an independent monotonic deadline.
#[derive(Debug, Clone, Default)]
pub struct PermissionLease {
    last: Option<PermissionUpdate>,
    deadline: Option<Instant>,
}

impl PermissionLease {
    /// Applies a strictly newer revision or an identical still-live retry.
    pub fn apply(
        &mut self,
        update: PermissionUpdate,
        wall_ms: u64,
        now: Instant,
    ) -> Result<(), &'static str> {
        if update.can_publish_soundboard && !update.can_publish_audio {
            return Err("soundboard requires audio permission");
        }
        if let Some(last) = &self.last {
            if update.revision < last.revision {
                return Err("stale revision");
            }
            if update.revision == last.revision {
                return if &update == last && self.active(now) {
                    Ok(())
                } else {
                    Err("conflicting or expired revision")
                };
            }
        }
        let remaining = update
            .expires_at
            .checked_sub(wall_ms)
            .ok_or("expired grant")?;
        if remaining == 0 || remaining > MAX_GRANT_MS {
            return Err("invalid grant lifetime");
        }
        let deadline = now
            .checked_add(Duration::from_millis(remaining))
            .ok_or("invalid deadline")?;
        self.deadline = Some(deadline);
        self.last = Some(update);
        Ok(())
    }

    /// The last grant while its deadline is live; expiry always denies media.
    pub fn current(&self, now: Instant) -> Option<&PermissionUpdate> {
        if self.active(now) {
            self.last.as_ref()
        } else {
            None
        }
    }

    fn active(&self, now: Instant) -> bool {
        self.deadline.is_some_and(|deadline| now < deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn update(revision: u64, expires_at: u64) -> PermissionUpdate {
        PermissionUpdate {
            revision,
            expires_at,
            can_publish_audio: true,
            can_publish_video: false,
            can_publish_soundboard: true,
        }
    }
    #[test]
    fn expires_and_rejects_stale_conflicting_and_overlong_updates() {
        let now = Instant::now();
        let mut lease = PermissionLease::default();
        assert!(lease.current(now).is_none());
        lease.apply(update(1, 2_000), 1_000, now).unwrap();
        assert!(lease.current(now).unwrap().can_publish_soundboard);
        assert!(lease.apply(update(1, 2_000), 1_000, now).is_ok());
        assert!(lease.apply(update(1, 2_001), 1_000, now).is_err());
        assert!(lease.apply(update(0, 2_000), 1_000, now).is_err());
        assert!(lease.current(now + Duration::from_millis(1_000)).is_none());
        assert!(
            lease
                .apply(update(1, 2_000), 1_000, now + Duration::from_millis(1_000))
                .is_err()
        );
        assert!(lease.apply(update(2, 20_000), 1_000, now).is_err());
        let mut soundboard_only = update(2, 2_000);
        soundboard_only.can_publish_audio = false;
        assert!(lease.apply(soundboard_only, 1_000, now).is_err());
    }
}
