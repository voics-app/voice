//! MEDIA_SINK_WANTS: the qualities a subscriber asked for, per publisher SSRC.

use std::collections::HashMap;

/// Quality used for publishers the subscriber did not mention.
pub const DEFAULT_QUALITY: u8 = 100;

/// Per-subscriber quality wishes, keyed by the publisher's audio or primary video SSRC.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SinkWants {
    by_ssrc: HashMap<u32, u8>,
}

impl SinkWants {
    /// Creates empty wishes (everything at default quality).
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the wishes; qualities above 100 are clamped.
    pub fn replace(&mut self, wants: impl IntoIterator<Item = (u32, u8)>) {
        self.by_ssrc = wants.into_iter().map(|(s, q)| (s, q.min(100))).collect();
    }

    /// Quality wanted for a publisher track.
    pub fn quality_for(&self, ssrc: u32) -> u8 {
        self.by_ssrc.get(&ssrc).copied().unwrap_or(DEFAULT_QUALITY)
    }

    /// Whether the subscriber wants the track at all.
    pub fn wants(&self, ssrc: u32) -> bool {
        self.quality_for(ssrc) > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_ssrc_gets_default() {
        assert_eq!(SinkWants::new().quality_for(7), DEFAULT_QUALITY);
    }

    #[test]
    fn replace_clamps_and_overrides() {
        let mut w = SinkWants::new();
        w.replace([(1, 0), (2, 250), (3, 50)]);
        assert!(!w.wants(1));
        assert_eq!(w.quality_for(2), 100);
        assert_eq!(w.quality_for(3), 50);
        w.replace([(3, 10)]);
        assert_eq!(w.quality_for(1), DEFAULT_QUALITY);
    }
}
