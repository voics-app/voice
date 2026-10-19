//! Use cases: drain and undrain a region. A drained region starts its nodes in drain mode
//! (`VOICE_DRAIN_ON_START`): they keep existing sessions but accept no new ones, so the gateway
//! allocates other regions. Live, per-node drain goes through the gateway internal API.

use super::error::AppError;
use super::ports::RegionStore;
use crate::domain::region::{RegionName, set_drained};

/// Whether the overlay changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainChange {
    /// The overlay was rewritten.
    Changed,
    /// The region was already in the requested state.
    AlreadyInState,
}

/// Sets the drain state of `name`.
pub fn set_region_drain(
    store: &dyn RegionStore,
    name: &str,
    drained: bool,
) -> Result<DrainChange, AppError> {
    let name = RegionName::parse(name)?;
    let overlay = store
        .read_overlay(name.as_str())?
        .ok_or_else(|| AppError::NotFound(name.to_string()))?;
    let updated = set_drained(&overlay, drained)?;
    if updated == overlay {
        return Ok(DrainChange::AlreadyInState);
    }
    store.write_overlay(name.as_str(), &updated)?;
    Ok(DrainChange::Changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::add_region::fakes::MemoryStore;

    fn store() -> MemoryStore {
        let store = MemoryStore::default();
        store.overlays.borrow_mut().insert(
            "eu-west".into(),
            "    literals:\n      - SNOWFLAKE_WORKER_ID=193\nrest: 1\n".into(),
        );
        store
    }

    #[test]
    fn drain_then_undrain_round_trips() {
        let store = store();
        assert_eq!(
            set_region_drain(&store, "eu-west", true),
            Ok(DrainChange::Changed)
        );
        assert!(store.overlays.borrow()["eu-west"].contains("VOICE_DRAIN_ON_START=true"));
        assert_eq!(
            set_region_drain(&store, "eu-west", true),
            Ok(DrainChange::AlreadyInState)
        );

        assert_eq!(
            set_region_drain(&store, "eu-west", false),
            Ok(DrainChange::Changed)
        );
        assert!(!store.overlays.borrow()["eu-west"].contains("DRAIN"));
        assert_eq!(
            set_region_drain(&store, "eu-west", false),
            Ok(DrainChange::AlreadyInState)
        );
    }

    #[test]
    fn unknown_regions_are_not_found() {
        assert_eq!(
            set_region_drain(&store(), "mars", true),
            Err(AppError::NotFound("mars".into()))
        );
    }
}
