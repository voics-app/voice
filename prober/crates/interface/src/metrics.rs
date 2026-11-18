use axum::{Router, extract::State, http::header, routing::get};
use probe_application::ObservationSink;
use probe_domain::{Health, Registration, Status};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
struct Sample {
    registration: Registration,
    health: Option<Health>,
    results: BTreeMap<&'static str, u64>,
}

#[derive(Debug, Default)]
pub struct Metrics {
    samples: RwLock<BTreeMap<String, Sample>>,
}

impl ObservationSink for Metrics {
    fn report(&self, registration: &Registration, health: Option<&Health>) {
        let mut samples = self.samples.write().expect("metrics writer");
        let sample = samples
            .entry(registration.node.node_id.clone())
            .or_insert_with(|| Sample {
                registration: registration.clone(),
                health: None,
                results: BTreeMap::new(),
            });
        sample.registration = registration.clone();
        sample.health = health.cloned();
        if let Some(health) = health {
            let outcome = health.failure.map_or("success", |failure| failure.label());
            let count = sample.results.entry(outcome).or_default();
            *count = count.saturating_add(1);
        }
    }
    fn remove(&self, node_id: &str) {
        self.samples
            .write()
            .expect("metrics writer")
            .remove(node_id);
    }
}

impl Metrics {
    pub fn render(&self) -> String {
        let mut out = String::from(
            "# HELP voice_probe_observer_owned Latest observation was published under an owned lease.\n\
             # TYPE voice_probe_observer_owned gauge\n\
             # TYPE voice_probe_state gauge\n\
             # TYPE voice_probe_results_total counter\n\
             # TYPE voice_probe_consecutive_failures gauge\n\
             # TYPE voice_probe_consecutive_passes gauge\n\
             # TYPE voice_probe_last_checked_timestamp_seconds gauge\n\
             # HELP voice_probe_packet_loss_ratio Fraction of synthetic round trips not received.\n\
             # TYPE voice_probe_packet_loss_ratio gauge\n\
             # HELP voice_probe_rtt_seconds Median synthetic round trip through both peers and the SFU.\n\
             # TYPE voice_probe_rtt_seconds gauge\n",
        );
        for sample in self.samples.read().expect("metrics reader").values() {
            let labels = format!(
                "node_id=\"{}\",region=\"{}\"",
                escape(&sample.registration.node.node_id),
                escape(&sample.registration.node.region)
            );
            let _ = writeln!(
                out,
                "voice_probe_observer_owned{{{labels}}} {}",
                u8::from(sample.health.is_some())
            );
            let status = sample.health.as_ref().map_or(Status::Unknown, |h| h.status);
            for candidate in [Status::Unknown, Status::Healthy, Status::Unhealthy] {
                let _ = writeln!(
                    out,
                    "voice_probe_state{{{labels},state=\"{}\"}} {}",
                    candidate.label(),
                    u8::from(status == candidate)
                );
            }
            for (outcome, count) in &sample.results {
                let _ = writeln!(
                    out,
                    "voice_probe_results_total{{{labels},outcome=\"{outcome}\"}} {count}"
                );
            }
            if let Some(health) = &sample.health {
                let _ = writeln!(
                    out,
                    "voice_probe_consecutive_failures{{{labels}}} {}\nvoice_probe_consecutive_passes{{{labels}}} {}",
                    health.consecutive_failures, health.consecutive_passes
                );
                let _ = writeln!(
                    out,
                    "voice_probe_last_checked_timestamp_seconds{{{labels}}} {}",
                    health.checked_at_ms as f64 / 1000.0
                );
                if let Some(loss) = health.loss_pct {
                    let _ = writeln!(
                        out,
                        "voice_probe_packet_loss_ratio{{{labels}}} {}",
                        loss / 100.0
                    );
                }
                if let Some(rtt) = health.rtt_ms {
                    let _ = writeln!(out, "voice_probe_rtt_seconds{{{labels}}} {}", rtt / 1000.0);
                }
            }
        }
        out
    }
}

pub fn router(metrics: Arc<Metrics>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok\n" }))
        .route(
            "/metrics",
            get(|State(metrics): State<Arc<Metrics>>| async move {
                (
                    [(
                        header::CONTENT_TYPE,
                        "text/plain; version=0.0.4; charset=utf-8",
                    )],
                    metrics.render(),
                )
            }),
        )
        .with_state(metrics)
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use probe_domain::Node;
    #[test]
    fn exports_unknown_without_a_claim_and_escapes_node_labels() {
        let metrics = Metrics::default();
        let registration = Registration {
            key: "k".into(),
            lease: "1".into(),
            node: Node {
                node_id: "node\"\nforged".into(),
                region: "r".into(),
                ws_url: "ws://n".into(),
                public_ip: "127.0.0.1".into(),
                udp_port: 50000,
            },
        };
        metrics.report(&registration, None);
        let text = metrics.render();
        assert!(text.contains("node_id=\"node\\\"\\nforged\""));
        assert!(text.contains("state=\"unknown\"} 1"));
        assert!(
            text.contains(
                "voice_probe_observer_owned{node_id=\"node\\\"\\nforged\",region=\"r\"} 0"
            )
        );
        let mut health = Health::new(&registration, "observer");
        health.observe(
            probe_domain::Observation::failed(probe_domain::Failure::Timeout),
            2,
            2,
            1,
        );
        metrics.report(&registration, Some(&health));
        metrics.report(&registration, Some(&health));
        metrics.report(&registration, None);
        let text = metrics.render();
        assert!(text.contains("outcome=\"timeout\"} 2"));
        assert!(text.contains("state=\"unknown\"} 1"));
        assert!(!text.contains("voice_probe_last_checked_timestamp_seconds{"));
        metrics.remove(&registration.node.node_id);
        assert!(!metrics.render().contains("node_id="));
    }
}
