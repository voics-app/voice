//! Windowed bitrate measurement for incoming layers and node traffic.

const WINDOW_US: u64 = 1_000_000;
/// Avoid treating a single startup packet as an arbitrarily large instantaneous bitrate.
const MIN_ESTIMATE_US: u64 = 100_000;

/// Measures bits per second over consecutive one-second windows.
#[derive(Debug, Clone, Default)]
pub struct BitrateMeter {
    window_start_us: Option<u64>,
    window_bytes: u64,
    last_bps: u64,
    last_activity_us: u64,
}

impl BitrateMeter {
    /// Creates an idle meter.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `bytes` observed at `now_us`.
    pub fn record(&mut self, bytes: usize, now_us: u64) {
        self.roll(now_us);
        self.window_start_us.get_or_insert(now_us);
        self.window_bytes += bytes as u64;
        self.last_activity_us = now_us;
    }

    /// Bitrate of the last complete window; zero when traffic stopped for over two windows.
    pub fn bps(&mut self, now_us: u64) -> u64 {
        self.roll(now_us);
        if self.window_start_us.is_none()
            || now_us.saturating_sub(self.last_activity_us) > 2 * WINDOW_US
        {
            return 0;
        }
        self.last_bps
    }

    /// Allocation estimate, including traffic before the first complete measurement window.
    /// Once a full window is available its stable measurement takes precedence. Operational
    /// traffic counters can keep using [`Self::bps`] without changing their window semantics.
    pub fn estimate_bps(&mut self, now_us: u64) -> u64 {
        let measured = self.bps(now_us);
        if measured > 0 {
            return measured;
        }
        let Some(start) = self.window_start_us else {
            return 0;
        };
        let elapsed = now_us.saturating_sub(start).max(MIN_ESTIMATE_US);
        self.window_bytes * 8 * 1_000_000 / elapsed
    }

    fn roll(&mut self, now_us: u64) {
        let Some(start) = self.window_start_us else {
            return;
        };
        let elapsed = now_us.saturating_sub(start);
        if elapsed >= WINDOW_US {
            self.last_bps = self.window_bytes * 8 * 1_000_000 / elapsed;
            self.window_bytes = 0;
            self.window_start_us = Some(now_us);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_meter_reports_zero() {
        let mut m = BitrateMeter::new();
        assert_eq!(m.bps(5_000_000), 0);
    }

    #[test]
    fn measures_steady_stream() {
        let mut m = BitrateMeter::new();
        // 50 packets/s of 125 bytes = 50 kbit/s
        for i in 0..=100u64 {
            m.record(125, i * 20_000);
        }
        let bps = m.bps(2_000_000);
        assert!((45_000..=55_000).contains(&bps), "{bps}");
    }

    #[test]
    fn stale_stream_drops_to_zero() {
        let mut m = BitrateMeter::new();
        for i in 0..=60u64 {
            m.record(1_000, i * 20_000);
        }
        assert!(m.bps(1_300_000) > 0);
        assert_eq!(m.bps(5_000_000), 0);
    }

    #[test]
    fn a_new_layer_is_allocatable_before_its_first_complete_window() {
        let mut m = BitrateMeter::new();
        assert_eq!(m.estimate_bps(0), 0);
        m.record(1_000, 100);
        assert_eq!(m.bps(100), 0, "completed-window metric stays unchanged");
        assert_eq!(m.estimate_bps(100), 80_000);
        m.record(1_000, 100_100);
        assert_eq!(m.estimate_bps(100_100), 160_000);
        let measured = m.bps(1_000_100);
        assert_eq!(m.estimate_bps(1_000_100), measured);
        assert_eq!(m.estimate_bps(4_000_100), 0);
    }

    #[test]
    fn a_restarted_layer_is_allocatable_without_waiting_another_window() {
        let mut m = BitrateMeter::new();
        m.record(1_000, 0);
        assert_eq!(m.estimate_bps(4_000_000), 0);
        m.record(1_000, 4_000_000);
        assert!(m.estimate_bps(4_000_000) > 0);
    }
}
