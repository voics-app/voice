//! Per-subscriber RTP sequence number and timestamp rewriting.
//!
//! A subscriber must see one continuous RTP stream per remote publisher even though:
//! - the SFU switches between simulcast layers, each with its own SSRC, sequence space and
//!   timestamp base;
//! - the SFU deliberately drops packets (higher SVC layers, Opus DTX comfort frames), which
//!   would otherwise show up as losses and trigger useless NACKs;
//! - a publisher can restart its encoder (big sequence jump on the same SSRC).
//!
//! Sequence numbers are handled in their extended (64 bit, roll-over free) form; the caller
//! truncates to 16 bits when writing the header. Timestamps are 32 bit and wrap naturally.

/// Largest forward sequence jump still treated as "the same stream". Anything larger is a
/// publisher restart and re-anchors the mapping just like a layer switch.
const MAX_SEQUENCE_JUMP: u64 = 3_000;

/// An inbound RTP packet, reduced to what the rewriter needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InboundPacket {
    /// SSRC the packet arrived on (identifies the simulcast layer / source).
    pub source: u32,
    /// Extended sequence number.
    pub seq: u64,
    /// RTP timestamp.
    pub timestamp: u32,
    /// Arrival time in microseconds on any monotonic clock.
    pub arrival_us: u64,
}

/// The rewritten header values to put on the forwarded packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundHeader {
    /// Extended outbound sequence number.
    pub seq: u64,
    /// Outbound RTP timestamp.
    pub timestamp: u32,
}

#[derive(Debug, Clone, Copy)]
struct Mapping {
    source: u32,
    seq_delta: u64,
    ts_delta: u32,
    /// Inbound packets at or below this sequence number predate the mapping and are dropped.
    floor_in_seq: u64,
    highest_in_seq: u64,
    highest_out_seq: u64,
    newest_out_ts: u32,
    newest_arrival_us: u64,
}

/// Rewrites one outbound stream (one publisher track towards one subscriber).
#[derive(Debug, Clone)]
pub struct RtpRewriter {
    clock_rate: u32,
    mapping: Option<Mapping>,
}

impl RtpRewriter {
    /// Creates a rewriter for a codec running at `clock_rate` Hz (48 000 for Opus, 90 000 for video).
    pub fn new(clock_rate: u32) -> Self {
        Self {
            clock_rate,
            mapping: None,
        }
    }

    /// Maps a packet that will be forwarded. Returns `None` when the packet is stale (it predates
    /// the current mapping) and must not be forwarded.
    pub fn rewrite(&mut self, packet: InboundPacket) -> Option<OutboundHeader> {
        let mapping = match self.mapping {
            None => self.mapping.insert(Self::identity(packet)),
            Some(ref m) if self.needs_reanchor(m, packet) => {
                let anchored = self.anchor_after(m, packet);
                self.mapping.insert(anchored)
            }
            Some(ref mut m) => m,
        };

        if packet.seq <= mapping.floor_in_seq {
            return None;
        }

        let header = OutboundHeader {
            seq: packet.seq.wrapping_add(mapping.seq_delta),
            timestamp: packet.timestamp.wrapping_add(mapping.ts_delta),
        };

        if packet.seq >= mapping.highest_in_seq {
            mapping.highest_in_seq = packet.seq;
            mapping.highest_out_seq = header.seq;
            mapping.newest_out_ts = header.timestamp;
            mapping.newest_arrival_us = packet.arrival_us;
        }

        Some(header)
    }

    /// Records a packet that is deliberately not forwarded so it leaves no hole in the
    /// outbound sequence space.
    ///
    /// The next forwarded packet continues right after the last forwarded one; inbound packets
    /// at or before the skipped one can no longer be forwarded without colliding, so they become
    /// stale.
    pub fn skip(&mut self, packet: InboundPacket) {
        let Some(mapping) = self.mapping.as_mut() else {
            return;
        };
        if packet.source != mapping.source || packet.seq <= mapping.highest_in_seq {
            return;
        }
        mapping.seq_delta = mapping.highest_out_seq.wrapping_sub(packet.seq);
        mapping.floor_in_seq = packet.seq;
        mapping.highest_in_seq = packet.seq;
    }

    fn identity(packet: InboundPacket) -> Mapping {
        Mapping {
            source: packet.source,
            seq_delta: 0,
            ts_delta: 0,
            floor_in_seq: packet.seq.saturating_sub(1),
            highest_in_seq: packet.seq,
            highest_out_seq: packet.seq,
            newest_out_ts: packet.timestamp,
            newest_arrival_us: packet.arrival_us,
        }
    }

    fn needs_reanchor(&self, mapping: &Mapping, packet: InboundPacket) -> bool {
        packet.source != mapping.source
            || packet.seq > mapping.highest_in_seq.saturating_add(MAX_SEQUENCE_JUMP)
    }

    /// Builds a mapping that makes `packet` the immediate successor of the last forwarded packet.
    fn anchor_after(&self, previous: &Mapping, packet: InboundPacket) -> Mapping {
        let out_seq = previous.highest_out_seq.wrapping_add(1);
        let elapsed_us = packet
            .arrival_us
            .saturating_sub(previous.newest_arrival_us);
        let advance = (elapsed_us.saturating_mul(u64::from(self.clock_rate)) / 1_000_000).max(1);
        // Truncation is intended: RTP timestamps are modulo 2^32.
        let out_ts = previous.newest_out_ts.wrapping_add(advance as u32);
        Mapping {
            source: packet.source,
            seq_delta: out_seq.wrapping_sub(packet.seq),
            ts_delta: out_ts.wrapping_sub(packet.timestamp),
            floor_in_seq: packet.seq.saturating_sub(1),
            highest_in_seq: packet.seq,
            highest_out_seq: out_seq,
            newest_out_ts: out_ts,
            newest_arrival_us: packet.arrival_us,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPUS: u32 = 48_000;
    const VIDEO: u32 = 90_000;

    fn pkt(source: u32, seq: u64, timestamp: u32, arrival_ms: u64) -> InboundPacket {
        InboundPacket {
            source,
            seq,
            timestamp,
            arrival_us: arrival_ms * 1_000,
        }
    }

    fn out(seq: u64, timestamp: u32) -> Option<OutboundHeader> {
        Some(OutboundHeader { seq, timestamp })
    }

    #[test]
    fn first_packet_keeps_its_original_numbers() {
        let mut rw = RtpRewriter::new(OPUS);
        assert_eq!(rw.rewrite(pkt(1, 500, 9_000, 0)), out(500, 9_000));
    }

    #[test]
    fn contiguous_packets_pass_through_unchanged() {
        let mut rw = RtpRewriter::new(OPUS);
        for i in 0..10u64 {
            let ts = 9_000 + i as u32 * 960;
            assert_eq!(rw.rewrite(pkt(1, 500 + i, ts, i * 20)), out(500 + i, ts));
        }
    }

    #[test]
    fn skipped_packet_leaves_no_sequence_gap() {
        let mut rw = RtpRewriter::new(OPUS);
        assert_eq!(rw.rewrite(pkt(1, 10, 0, 0)), out(10, 0));
        assert_eq!(rw.rewrite(pkt(1, 11, 960, 20)), out(11, 960));
        rw.skip(pkt(1, 12, 1_920, 40));
        assert_eq!(rw.rewrite(pkt(1, 13, 2_880, 60)), out(12, 2_880));
    }

    #[test]
    fn dtx_gap_keeps_sequence_contiguous_and_timestamps_real() {
        // Opus DTX: 50 comfort frames are dropped, speech resumes one second later.
        let mut rw = RtpRewriter::new(OPUS);
        assert_eq!(rw.rewrite(pkt(1, 100, 0, 0)), out(100, 0));
        for i in 1..=50u64 {
            rw.skip(pkt(1, 100 + i, i as u32 * 960, i * 20));
        }
        // Timestamp keeps its real-time distance so the jitter buffer plays silence.
        assert_eq!(rw.rewrite(pkt(1, 151, 48_960, 1_020)), out(101, 48_960));
        assert_eq!(rw.rewrite(pkt(1, 152, 49_920, 1_040)), out(102, 49_920));
    }

    #[test]
    fn many_interleaved_skips_accumulate() {
        let mut rw = RtpRewriter::new(VIDEO);
        let mut expected = 1_000u64;
        for seq in 1_000..1_100u64 {
            let p = pkt(7, seq, seq as u32 * 10, seq);
            if seq % 3 == 0 {
                rw.skip(p);
            } else {
                assert_eq!(rw.rewrite(p).map(|h| h.seq), Some(expected));
                expected += 1;
            }
        }
    }

    #[test]
    fn reordered_packet_within_mapping_gets_its_own_slot() {
        let mut rw = RtpRewriter::new(VIDEO);
        assert_eq!(rw.rewrite(pkt(1, 20, 100, 0)), out(20, 100));
        assert_eq!(rw.rewrite(pkt(1, 22, 300, 2)), out(22, 300));
        assert_eq!(rw.rewrite(pkt(1, 21, 200, 3)), out(21, 200));
        assert_eq!(rw.rewrite(pkt(1, 23, 400, 4)), out(23, 400));
    }

    #[test]
    fn late_packet_from_before_a_skip_is_dropped() {
        let mut rw = RtpRewriter::new(VIDEO);
        assert_eq!(rw.rewrite(pkt(1, 20, 100, 0)), out(20, 100));
        rw.skip(pkt(1, 22, 300, 2));
        // 21 would now collide with the slot taken by 23, so it is dropped.
        assert_eq!(rw.rewrite(pkt(1, 21, 200, 3)), None);
        assert_eq!(rw.rewrite(pkt(1, 23, 400, 4)), out(21, 400));
    }

    #[test]
    fn late_skip_does_not_shift_the_mapping() {
        let mut rw = RtpRewriter::new(VIDEO);
        assert_eq!(rw.rewrite(pkt(1, 20, 100, 0)), out(20, 100));
        assert_eq!(rw.rewrite(pkt(1, 22, 300, 2)), out(22, 300));
        rw.skip(pkt(1, 21, 200, 3));
        assert_eq!(rw.rewrite(pkt(1, 23, 400, 4)), out(23, 400));
    }

    #[test]
    fn skips_before_anything_was_forwarded_are_ignored() {
        let mut rw = RtpRewriter::new(VIDEO);
        rw.skip(pkt(1, 5, 0, 0));
        rw.skip(pkt(1, 6, 0, 0));
        assert_eq!(rw.rewrite(pkt(1, 7, 50, 1)), out(7, 50));
    }

    #[test]
    fn layer_switch_continues_sequence_and_advances_timestamp_by_elapsed_time() {
        let mut rw = RtpRewriter::new(VIDEO);
        assert_eq!(rw.rewrite(pkt(1, 100, 1_000, 0)), out(100, 1_000));
        assert_eq!(rw.rewrite(pkt(1, 101, 4_000, 33)), out(101, 4_000));
        // Switch to layer with SSRC 2 and completely different bases, 33 ms later.
        let switched = rw.rewrite(pkt(2, 60_000, 777_777, 66)).unwrap();
        assert_eq!(switched.seq, 102);
        assert_eq!(switched.timestamp, 4_000 + 33 * 90);
        // Subsequent packets of the new layer follow the new mapping.
        assert_eq!(
            rw.rewrite(pkt(2, 60_001, 777_777 + 3_000, 99)),
            out(103, 4_000 + 33 * 90 + 3_000)
        );
    }

    #[test]
    fn layer_switch_without_elapsed_time_still_moves_timestamp_forward() {
        let mut rw = RtpRewriter::new(VIDEO);
        assert_eq!(rw.rewrite(pkt(1, 1, 500, 10)), out(1, 500));
        let switched = rw.rewrite(pkt(2, 9, 1, 10)).unwrap();
        assert_eq!(switched.seq, 2);
        assert_eq!(switched.timestamp, 501);
    }

    #[test]
    fn packets_older_than_a_switch_point_are_dropped() {
        let mut rw = RtpRewriter::new(VIDEO);
        rw.rewrite(pkt(1, 10, 0, 0));
        rw.rewrite(pkt(2, 500, 0, 10));
        assert_eq!(rw.rewrite(pkt(2, 499, 0, 11)), None);
        assert_eq!(rw.rewrite(pkt(2, 501, 90, 12)).map(|h| h.seq), Some(12));
    }

    #[test]
    fn switching_back_and_forth_never_reuses_a_sequence_number() {
        let mut rw = RtpRewriter::new(VIDEO);
        let mut seen = Vec::new();
        let mut t = 0;
        for round in 0..6u64 {
            let source = (round % 2) as u32 + 1;
            for i in 0..5u64 {
                t += 10;
                let seq = 1_000 * (source as u64) + round * 5 + i;
                let h = rw.rewrite(pkt(source, seq, seq as u32 * 900, t)).unwrap();
                seen.push(h.seq);
            }
        }
        let mut sorted = seen.clone();
        sorted.dedup();
        assert_eq!(sorted, seen, "strictly increasing, no duplicates");
        assert_eq!(seen.windows(2).filter(|w| w[1] != w[0] + 1).count(), 0);
    }

    #[test]
    fn publisher_restart_with_huge_jump_reanchors() {
        let mut rw = RtpRewriter::new(OPUS);
        rw.rewrite(pkt(1, 100, 1_000, 0));
        let h = rw.rewrite(pkt(1, 40_000, 123, 20)).unwrap();
        assert_eq!(h.seq, 101);
        assert_eq!(h.timestamp, 1_000 + 20 * 48);
    }

    #[test]
    fn outbound_sequence_crosses_the_16_bit_boundary() {
        let mut rw = RtpRewriter::new(OPUS);
        rw.rewrite(pkt(1, 65_534, 0, 0));
        rw.skip(pkt(1, 65_535, 960, 20));
        let h = rw.rewrite(pkt(1, 65_536, 1_920, 40)).unwrap();
        assert_eq!(h.seq, 65_535);
        let h = rw.rewrite(pkt(1, 65_537, 2_880, 60)).unwrap();
        assert_eq!(h.seq, 65_536);
        assert_eq!(h.seq as u16, 0);
    }

    #[test]
    fn timestamp_wraps_around_u32() {
        let mut rw = RtpRewriter::new(OPUS);
        rw.rewrite(pkt(1, 1, u32::MAX - 100, 0));
        let switched = rw.rewrite(pkt(2, 1, 5, 20)).unwrap();
        assert_eq!(switched.timestamp, (u32::MAX - 100).wrapping_add(20 * 48));
        let h = rw.rewrite(pkt(2, 2, 965, 40)).unwrap();
        assert_eq!(h.timestamp, switched.timestamp.wrapping_add(960));
    }

    #[test]
    fn skip_from_a_source_that_is_not_forwarded_is_ignored() {
        let mut rw = RtpRewriter::new(VIDEO);
        rw.rewrite(pkt(1, 10, 0, 0));
        rw.skip(pkt(2, 11, 0, 1));
        assert_eq!(rw.rewrite(pkt(1, 11, 90, 2)).map(|h| h.seq), Some(11));
    }

    #[test]
    fn duplicate_packet_maps_to_the_same_slot() {
        let mut rw = RtpRewriter::new(VIDEO);
        rw.rewrite(pkt(1, 10, 0, 0));
        let a = rw.rewrite(pkt(1, 11, 90, 1));
        let b = rw.rewrite(pkt(1, 11, 90, 2));
        assert_eq!(a, b);
        assert_eq!(rw.rewrite(pkt(1, 12, 180, 3)).map(|h| h.seq), Some(12));
    }
}
