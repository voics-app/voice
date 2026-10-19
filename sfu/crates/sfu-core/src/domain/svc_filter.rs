//! Per-subscriber VP9 SVC layer filtering on a single SSRC.
//!
//! With SVC the publisher sends all spatial/temporal layers on one stream. The SFU forwards only
//! the packets at or below the subscriber's target layers. Dropped packets are reported to the
//! RTP rewriter as skipped so the subscriber sees a gap-free sequence, and the marker bit is set
//! on the last forwarded packet of each picture.

use super::vp9_descriptor::Vp9Descriptor;

/// Target spatial and temporal layer (inclusive maxima).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SvcTarget {
    /// Highest spatial layer id to forward.
    pub spatial: u8,
    /// Highest temporal layer id to forward.
    pub temporal: u8,
}

/// Decision for one SVC packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SvcDecision {
    /// Forward with the given marker bit.
    Forward {
        /// Marker bit to write on the forwarded packet.
        marker: bool,
    },
    /// Drop (report it to the rewriter as skipped).
    Drop,
}

/// SVC filter state for one subscriber of one publisher.
#[derive(Debug, Clone, Default)]
pub struct SvcFilter {
    current: Option<SvcTarget>,
    target: Option<SvcTarget>,
}

impl SvcFilter {
    /// Creates a filter that forwards nothing until a target is set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the target. Returns true if a keyframe is needed to reach it (spatial up-switch).
    pub fn set_target(&mut self, target: Option<SvcTarget>) -> bool {
        self.target = target;
        match (self.current, target) {
            (_, None) => {
                self.current = None;
                false
            }
            (None, Some(_)) => true,
            (Some(current), Some(t)) => t.spatial > current.spatial,
        }
    }

    /// Decides what to do with a packet given its descriptor and original marker bit.
    pub fn on_packet(&mut self, descriptor: &Vp9Descriptor, marker: bool) -> SvcDecision {
        self.apply_pending_switch(descriptor);
        let Some(current) = self.current else {
            return SvcDecision::Drop;
        };
        if descriptor.spatial_id > current.spatial || descriptor.temporal_id > current.temporal {
            return SvcDecision::Drop;
        }
        let top_of_picture = descriptor.ends_frame && descriptor.spatial_id == current.spatial;
        SvcDecision::Forward {
            marker: marker || top_of_picture,
        }
    }

    /// Spatial up-switches (and the very first selection) wait for a keyframe; spatial
    /// down-switches and temporal changes apply at the next picture start.
    fn apply_pending_switch(&mut self, descriptor: &Vp9Descriptor) {
        let Some(target) = self.target else {
            return;
        };
        if self.current == Some(target) {
            return;
        }
        let picture_start = descriptor.begins_frame && descriptor.spatial_id == 0;
        let needs_keyframe = self.current.is_none_or(|c| target.spatial > c.spatial);
        if needs_keyframe {
            if descriptor.starts_keyframe() {
                self.current = Some(target);
            }
        } else if picture_start {
            self.current = Some(target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(sid: u8, tid: u8, begin: bool, end: bool, inter: bool) -> Vp9Descriptor {
        Vp9Descriptor {
            inter_picture: inter,
            begins_frame: begin,
            ends_frame: end,
            switching_up: false,
            spatial_id: sid,
            temporal_id: tid,
        }
    }

    const LOW: SvcTarget = SvcTarget {
        spatial: 0,
        temporal: 2,
    };
    const HIGH: SvcTarget = SvcTarget {
        spatial: 2,
        temporal: 2,
    };

    #[test]
    fn waits_for_keyframe_before_first_forward() {
        let mut f = SvcFilter::new();
        assert!(f.set_target(Some(LOW)));
        assert_eq!(
            f.on_packet(&desc(0, 0, true, true, true), false),
            SvcDecision::Drop
        );
        assert_eq!(
            f.on_packet(&desc(0, 0, true, true, false), false),
            SvcDecision::Forward { marker: true }
        );
    }

    #[test]
    fn drops_layers_above_target_and_sets_marker_on_top_layer() {
        let mut f = SvcFilter::new();
        f.set_target(Some(SvcTarget {
            spatial: 1,
            temporal: 2,
        }));
        assert_eq!(
            f.on_packet(&desc(0, 0, true, true, false), false),
            SvcDecision::Forward { marker: false }
        );
        assert_eq!(
            f.on_packet(&desc(1, 0, true, true, false), false),
            SvcDecision::Forward { marker: true }
        );
        assert_eq!(
            f.on_packet(&desc(2, 0, true, true, false), true),
            SvcDecision::Drop
        );
    }

    #[test]
    fn temporal_layers_above_target_are_dropped() {
        let mut f = SvcFilter::new();
        f.set_target(Some(SvcTarget {
            spatial: 0,
            temporal: 0,
        }));
        f.on_packet(&desc(0, 0, true, true, false), false);
        assert_eq!(
            f.on_packet(&desc(0, 1, true, true, true), true),
            SvcDecision::Drop
        );
        assert!(matches!(
            f.on_packet(&desc(0, 0, true, true, true), true),
            SvcDecision::Forward { .. }
        ));
    }

    #[test]
    fn spatial_upswitch_waits_for_keyframe_downswitch_at_picture_start() {
        let mut f = SvcFilter::new();
        f.set_target(Some(LOW));
        f.on_packet(&desc(0, 0, true, true, false), false);
        assert!(f.set_target(Some(HIGH)));
        // Inter picture: still low only.
        assert!(matches!(
            f.on_packet(&desc(0, 0, true, true, true), false),
            SvcDecision::Forward { .. }
        ));
        assert_eq!(
            f.on_packet(&desc(2, 0, true, true, true), true),
            SvcDecision::Drop
        );
        // Keyframe: switch up.
        f.on_packet(&desc(0, 0, true, false, false), false);
        assert_eq!(
            f.on_packet(&desc(2, 0, true, true, false), true),
            SvcDecision::Forward { marker: true }
        );
        // Down-switch needs no keyframe, applies at next picture start.
        assert!(!f.set_target(Some(LOW)));
        assert_eq!(
            f.on_packet(&desc(0, 0, true, true, true), false),
            SvcDecision::Forward { marker: true }
        );
        assert_eq!(
            f.on_packet(&desc(1, 0, true, true, true), false),
            SvcDecision::Drop
        );
    }

    #[test]
    fn pause_drops_everything() {
        let mut f = SvcFilter::new();
        f.set_target(Some(LOW));
        f.on_packet(&desc(0, 0, true, true, false), false);
        f.set_target(None);
        assert_eq!(
            f.on_packet(&desc(0, 0, true, true, false), true),
            SvcDecision::Drop
        );
    }
}
