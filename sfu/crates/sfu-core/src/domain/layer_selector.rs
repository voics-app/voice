//! Keyframe-gated layer switching for one (subscriber, publisher track) pair.
//!
//! The allocator decides a *target* layer at any time; the forwarded *current* layer only changes
//! when a keyframe arrives on the target layer, so the subscriber's decoder never sees a broken
//! reference chain. Pausing (target `None`) takes effect immediately.

/// Decision for one inbound packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerDecision {
    /// Forward the packet; `switched` is true for the first packet after a layer switch.
    Forward {
        /// The packet starts a newly selected layer.
        switched: bool,
    },
    /// Do not forward the packet.
    Drop,
}

/// Simulcast layer selection state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LayerSelector {
    current: Option<usize>,
    target: Option<usize>,
}

impl LayerSelector {
    /// Creates a selector that forwards nothing until a target is set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Layer currently forwarded.
    pub fn current(&self) -> Option<usize> {
        self.current
    }

    /// Layer the selector is switching to.
    pub fn target(&self) -> Option<usize> {
        self.target
    }

    /// Sets a new target. Returns true when a keyframe must be requested on the target layer.
    pub fn set_target(&mut self, target: Option<usize>) -> bool {
        self.target = target;
        if target.is_none() {
            self.current = None;
        }
        self.awaiting_keyframe()
    }

    /// Forgets the forwarded layer so the next keyframe on the target layer starts the stream
    /// again. Returns true when a keyframe must be requested on the target layer.
    pub fn restart(&mut self) -> bool {
        self.current = None;
        self.awaiting_keyframe()
    }

    /// True while waiting for a keyframe on the target layer.
    pub fn awaiting_keyframe(&self) -> bool {
        self.target.is_some() && self.target != self.current
    }

    /// Decides what to do with a packet of `layer`.
    pub fn on_packet(&mut self, layer: usize, starts_keyframe: bool) -> LayerDecision {
        if self.target == Some(layer) && self.current != Some(layer) && starts_keyframe {
            self.current = Some(layer);
            return LayerDecision::Forward { switched: true };
        }
        if self.current == Some(layer) {
            LayerDecision::Forward { switched: false }
        } else {
            LayerDecision::Drop
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FWD: LayerDecision = LayerDecision::Forward { switched: false };
    const SWITCH: LayerDecision = LayerDecision::Forward { switched: true };

    #[test]
    fn nothing_is_forwarded_without_a_target() {
        let mut s = LayerSelector::new();
        assert_eq!(s.on_packet(0, true), LayerDecision::Drop);
    }

    #[test]
    fn first_layer_starts_on_a_keyframe() {
        let mut s = LayerSelector::new();
        assert!(s.set_target(Some(1)));
        assert_eq!(s.on_packet(1, false), LayerDecision::Drop);
        assert_eq!(s.on_packet(0, true), LayerDecision::Drop);
        assert_eq!(s.on_packet(1, true), SWITCH);
        assert_eq!(s.on_packet(1, false), FWD);
        assert!(!s.awaiting_keyframe());
    }

    #[test]
    fn keeps_forwarding_old_layer_until_target_keyframe() {
        let mut s = LayerSelector::new();
        s.set_target(Some(0));
        s.on_packet(0, true);
        assert!(s.set_target(Some(2)));
        assert_eq!(s.on_packet(0, false), FWD);
        assert_eq!(s.on_packet(2, false), LayerDecision::Drop);
        assert_eq!(s.on_packet(2, true), SWITCH);
        assert_eq!(s.on_packet(0, false), LayerDecision::Drop);
        assert_eq!(s.current(), Some(2));
    }

    #[test]
    fn restart_waits_for_a_new_keyframe_on_the_target() {
        let mut s = LayerSelector::new();
        s.set_target(Some(1));
        s.on_packet(1, true);
        assert!(s.restart());
        assert_eq!(s.on_packet(1, false), LayerDecision::Drop);
        assert_eq!(s.on_packet(1, true), SWITCH);
    }

    #[test]
    fn restart_without_a_target_needs_no_keyframe() {
        let mut s = LayerSelector::new();
        assert!(!s.restart());
    }

    #[test]
    fn pausing_is_immediate() {
        let mut s = LayerSelector::new();
        s.set_target(Some(0));
        s.on_packet(0, true);
        assert!(!s.set_target(None));
        assert_eq!(s.on_packet(0, true), LayerDecision::Drop);
        assert_eq!(s.current(), None);
    }

    #[test]
    fn retargeting_to_current_layer_needs_no_keyframe() {
        let mut s = LayerSelector::new();
        s.set_target(Some(1));
        s.on_packet(1, true);
        s.set_target(Some(2));
        assert!(!s.set_target(Some(1)));
        assert_eq!(s.target(), Some(1));
        assert_eq!(s.on_packet(1, false), FWD);
    }
}
