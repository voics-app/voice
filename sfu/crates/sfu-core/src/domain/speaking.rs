//! Speaking detection from the RFC 6464 audio-level header extension.
//!
//! Each Opus packet carries the level of its audio in -dBov (0 = loudest, -127 = silence) and an
//! optional voice-activity flag set by the sender's VAD. A speaker starts speaking on the first
//! voiced packet and stops after a hang-over period without voiced packets. Because of DTX the
//! sender may stop sending entirely when silent, so the hang-over is also evaluated on timer
//! ticks, not only on packet arrival.

/// Level (dBov) at or above which a packet counts as voiced when no VAD flag is present.
pub const DEFAULT_VOICE_THRESHOLD_DBOV: i8 = -50;
/// Time without voiced packets after which speaking stops.
pub const DEFAULT_HANGOVER_US: u64 = 400_000;

/// Speaking state machine for one audio track.
#[derive(Debug, Clone)]
pub struct SpeakingDetector {
    threshold_dbov: i8,
    hangover_us: u64,
    speaking: bool,
    last_voiced_us: u64,
}

impl Default for SpeakingDetector {
    fn default() -> Self {
        Self::new(DEFAULT_VOICE_THRESHOLD_DBOV, DEFAULT_HANGOVER_US)
    }
}

impl SpeakingDetector {
    /// Creates a detector with a custom threshold and hang-over.
    pub fn new(threshold_dbov: i8, hangover_us: u64) -> Self {
        Self {
            threshold_dbov,
            hangover_us,
            speaking: false,
            last_voiced_us: 0,
        }
    }

    /// Current state.
    pub fn is_speaking(&self) -> bool {
        self.speaking
    }

    /// Feeds one packet's extension values. Returns `Some(new_state)` on a transition.
    pub fn on_packet(
        &mut self,
        audio_level_dbov: Option<i8>,
        voice_activity: Option<bool>,
        now_us: u64,
    ) -> Option<bool> {
        let voiced = match (voice_activity, audio_level_dbov) {
            (Some(vad), _) => vad,
            (None, Some(level)) => level >= self.threshold_dbov,
            (None, None) => false,
        };
        if voiced {
            self.last_voiced_us = now_us;
            if !self.speaking {
                self.speaking = true;
                return Some(true);
            }
            return None;
        }
        self.tick(now_us)
    }

    /// Evaluates the hang-over. Returns `Some(false)` when speaking stops.
    pub fn tick(&mut self, now_us: u64) -> Option<bool> {
        if self.speaking && now_us.saturating_sub(self.last_voiced_us) >= self.hangover_us {
            self.speaking = false;
            return Some(false);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000;

    #[test]
    fn loud_packet_starts_speaking_once() {
        let mut d = SpeakingDetector::default();
        assert_eq!(d.on_packet(Some(-30), None, 0), Some(true));
        assert_eq!(d.on_packet(Some(-30), None, 20 * MS), None);
        assert!(d.is_speaking());
    }

    #[test]
    fn quiet_packets_do_not_start_speaking() {
        let mut d = SpeakingDetector::default();
        assert_eq!(d.on_packet(Some(-90), None, 0), None);
        assert_eq!(d.on_packet(None, None, 20 * MS), None);
        assert!(!d.is_speaking());
    }

    #[test]
    fn vad_flag_overrides_level() {
        let mut d = SpeakingDetector::default();
        assert_eq!(d.on_packet(Some(-90), Some(true), 0), Some(true));
        let mut d = SpeakingDetector::default();
        assert_eq!(d.on_packet(Some(-10), Some(false), 0), None);
    }

    #[test]
    fn stops_after_hangover_of_quiet_packets() {
        let mut d = SpeakingDetector::default();
        d.on_packet(Some(-20), None, 0);
        assert_eq!(d.on_packet(Some(-100), None, 200 * MS), None);
        assert_eq!(d.on_packet(Some(-100), None, 400 * MS), Some(false));
    }

    #[test]
    fn dtx_silence_is_detected_by_ticks() {
        let mut d = SpeakingDetector::default();
        d.on_packet(Some(-20), None, 0);
        assert_eq!(d.tick(100 * MS), None);
        assert_eq!(d.tick(450 * MS), Some(false));
        assert_eq!(d.tick(900 * MS), None);
    }

    #[test]
    fn resumes_after_silence() {
        let mut d = SpeakingDetector::default();
        d.on_packet(Some(-20), None, 0);
        d.tick(500 * MS);
        assert_eq!(d.on_packet(Some(-20), None, 600 * MS), Some(true));
    }
}
