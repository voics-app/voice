//! Splits a subscriber's estimated downlink bandwidth across the video tracks it watches.
//!
//! Inputs per watched track: the quality the viewer asked for through MEDIA_SINK_WANTS
//! (0 = do not send, 100 = best) and the layers the publisher currently sends, ordered from
//! lowest to highest quality together with their measured bitrate. The output is the layer index
//! to forward for each track, or `None` to pause it.

/// One layer a publisher offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerOffer {
    /// Quality label of the layer (0-100), as announced in READY `streams[].quality`.
    pub quality: u8,
    /// Measured bitrate. Zero means the publisher is not sending this layer right now.
    pub bitrate_bps: u64,
}

impl LayerOffer {
    fn is_active(&self) -> bool {
        self.bitrate_bps > 0
    }
}

/// What a subscriber wants from one publisher's video track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackDemand {
    /// Requested quality, 0-100.
    pub wanted_quality: u8,
    /// Offered layers, ascending quality.
    pub layers: Vec<LayerOffer>,
}

impl TrackDemand {
    /// Active layer indices the viewer accepts, ascending. The lowest active layer is always
    /// acceptable for a non-zero request, even if its label exceeds the requested quality.
    fn acceptable_layers(&self) -> Vec<usize> {
        if self.wanted_quality == 0 {
            return Vec::new();
        }
        let active: Vec<usize> = (0..self.layers.len())
            .filter(|&i| self.layers[i].is_active())
            .collect();
        let within: Vec<usize> = active
            .iter()
            .copied()
            .filter(|&i| self.layers[i].quality <= self.wanted_quality)
            .collect();
        if within.is_empty() {
            active.into_iter().take(1).collect()
        } else {
            within
        }
    }
}

/// Allocates layers greedily: everybody gets a base layer first (highest requested quality
/// first), then tracks are upgraded one step at a time in priority order while the budget lasts.
pub fn allocate_layers(budget_bps: u64, demands: &[TrackDemand]) -> Vec<Option<usize>> {
    let ladders: Vec<Vec<usize>> = demands.iter().map(TrackDemand::acceptable_layers).collect();
    let mut priority: Vec<usize> = (0..demands.len()).collect();
    priority.sort_by_key(|&i| std::cmp::Reverse(demands[i].wanted_quality));

    // Position in each ladder, or None while unallocated.
    let mut steps: Vec<Option<usize>> = vec![None; demands.len()];
    let mut remaining = budget_bps;

    for &track in &priority {
        let Some(&base) = ladders[track].first() else {
            continue;
        };
        let cost = demands[track].layers[base].bitrate_bps;
        if cost <= remaining {
            remaining -= cost;
            steps[track] = Some(0);
        }
    }

    loop {
        let mut upgraded = false;
        for &track in &priority {
            let Some(step) = steps[track] else {
                continue;
            };
            let ladder = &ladders[track];
            let Some(&next) = ladder.get(step + 1) else {
                continue;
            };
            let layers = &demands[track].layers;
            let extra = layers[next]
                .bitrate_bps
                .saturating_sub(layers[ladder[step]].bitrate_bps);
            if extra <= remaining {
                remaining -= extra;
                steps[track] = Some(step + 1);
                upgraded = true;
            }
        }
        if !upgraded {
            break;
        }
    }

    steps
        .iter()
        .zip(&ladders)
        .map(|(step, ladder)| step.map(|s| ladder[s]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ladder() -> Vec<LayerOffer> {
        vec![
            LayerOffer {
                quality: 25,
                bitrate_bps: 150_000,
            },
            LayerOffer {
                quality: 50,
                bitrate_bps: 500_000,
            },
            LayerOffer {
                quality: 100,
                bitrate_bps: 1_500_000,
            },
        ]
    }

    fn demand(wanted_quality: u8) -> TrackDemand {
        TrackDemand {
            wanted_quality,
            layers: ladder(),
        }
    }

    #[test]
    fn plenty_of_bandwidth_gives_requested_quality() {
        assert_eq!(allocate_layers(10_000_000, &[demand(100)]), vec![Some(2)]);
        assert_eq!(allocate_layers(10_000_000, &[demand(50)]), vec![Some(1)]);
        assert_eq!(allocate_layers(10_000_000, &[demand(30)]), vec![Some(0)]);
    }

    #[test]
    fn zero_quality_pauses_the_track() {
        assert_eq!(allocate_layers(10_000_000, &[demand(0)]), vec![None]);
    }

    #[test]
    fn request_below_lowest_label_still_gets_the_base_layer() {
        assert_eq!(allocate_layers(10_000_000, &[demand(10)]), vec![Some(0)]);
    }

    #[test]
    fn bandwidth_caps_the_layer() {
        assert_eq!(allocate_layers(600_000, &[demand(100)]), vec![Some(1)]);
        assert_eq!(allocate_layers(200_000, &[demand(100)]), vec![Some(0)]);
    }

    #[test]
    fn not_enough_for_a_base_layer_pauses() {
        assert_eq!(allocate_layers(100_000, &[demand(100)]), vec![None]);
    }

    #[test]
    fn everyone_gets_a_base_layer_before_anyone_upgrades() {
        // 2 x 150k base = 300k; 400k cannot upgrade anybody to 500k.
        let out = allocate_layers(400_000, &[demand(100), demand(100)]);
        assert_eq!(out, vec![Some(0), Some(0)]);
    }

    #[test]
    fn higher_requested_quality_wins_scarce_budget() {
        // Budget for one base layer only: the focused (100) tile gets it.
        let out = allocate_layers(200_000, &[demand(25), demand(100)]);
        assert_eq!(out, vec![None, Some(0)]);
    }

    #[test]
    fn upgrades_round_robin_in_priority_order() {
        // bases 300k, +350k upgrade for the first priority = 650k, second upgrade would need 1000k.
        let out = allocate_layers(700_000, &[demand(50), demand(100)]);
        assert_eq!(out, vec![Some(0), Some(1)]);
    }

    #[test]
    fn inactive_layers_are_skipped() {
        let mut d = demand(100);
        d.layers[2].bitrate_bps = 0;
        assert_eq!(allocate_layers(10_000_000, &[d]), vec![Some(1)]);
        let mut low_off = demand(100);
        low_off.layers[0].bitrate_bps = 0;
        assert_eq!(allocate_layers(600_000, &[low_off]), vec![Some(1)]);
    }

    #[test]
    fn publisher_without_active_layers_is_paused() {
        let d = TrackDemand {
            wanted_quality: 100,
            layers: vec![LayerOffer {
                quality: 100,
                bitrate_bps: 0,
            }],
        };
        assert_eq!(allocate_layers(10_000_000, &[d]), vec![None]);
    }
}
