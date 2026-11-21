use probe_domain::Health;
use serde_json::{Value, json};

pub(crate) fn health(health: &Health) -> Value {
    let mut value = json!({"node_id": health.node_id, "registration_lease": health.registration_lease,
        "prober_id": health.prober_id, "status": health.status.label(), "consecutive_failures": health.consecutive_failures,
        "consecutive_passes": health.consecutive_passes, "checked_at_ms": health.checked_at_ms});
    if let Some(loss) = health.loss_pct {
        value["loss_pct"] = json!(loss);
    }
    if let Some(rtt) = health.rtt_ms {
        value["rtt_ms"] = json!(rtt);
    }
    if let Some(failure) = health.failure {
        value["failure"] = json!(failure.label());
    }
    value
}
