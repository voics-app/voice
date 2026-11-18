//! Pure health hysteresis and discovery values; no network or clock calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub node_id: String,
    pub region: String,
    pub ws_url: String,
    pub public_ip: String,
    pub udp_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registration {
    pub key: String,
    pub lease: String,
    pub node: Node,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Signaling,
    Transport,
    RtpLoss,
    Rtt,
    Timeout,
    RegistrationChanged,
}

impl Failure {
    pub fn label(self) -> &'static str {
        match self {
            Self::Signaling => "signaling",
            Self::Transport => "transport",
            Self::RtpLoss => "rtp_loss",
            Self::Rtt => "rtt",
            Self::Timeout => "timeout",
            Self::RegistrationChanged => "registration_changed",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Observation {
    pub loss_pct: Option<f64>,
    pub rtt_ms: Option<f64>,
    pub failure: Option<Failure>,
}

impl Observation {
    pub fn failed(reason: Failure) -> Self {
        Self {
            loss_pct: None,
            rtt_ms: None,
            failure: Some(reason),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Unknown,
    Healthy,
    Unhealthy,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Healthy => "healthy",
            Self::Unhealthy => "unhealthy",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Health {
    pub node_id: String,
    pub registration_lease: String,
    pub prober_id: String,
    pub status: Status,
    pub consecutive_failures: u32,
    pub consecutive_passes: u32,
    pub checked_at_ms: u64,
    pub loss_pct: Option<f64>,
    pub rtt_ms: Option<f64>,
    pub failure: Option<Failure>,
}

impl Health {
    pub fn new(registration: &Registration, prober_id: &str) -> Self {
        Self {
            node_id: registration.node.node_id.clone(),
            registration_lease: registration.lease.clone(),
            prober_id: prober_id.to_owned(),
            status: Status::Unknown,
            consecutive_failures: 0,
            consecutive_passes: 0,
            checked_at_ms: 0,
            loss_pct: None,
            rtt_ms: None,
            failure: None,
        }
    }

    pub fn observe(&mut self, observation: Observation, failures: u32, passes: u32, now_ms: u64) {
        self.loss_pct = observation.loss_pct;
        self.rtt_ms = observation.rtt_ms;
        self.failure = observation.failure;
        self.checked_at_ms = now_ms;
        if observation.failure.is_some() {
            self.consecutive_passes = 0;
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            if self.consecutive_failures >= failures {
                self.status = Status::Unhealthy;
            }
        } else {
            self.consecutive_failures = 0;
            self.consecutive_passes = self.consecutive_passes.saturating_add(1);
            if self.consecutive_passes >= passes {
                self.status = Status::Healthy;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_failures_exclude_and_consecutive_passes_readmit() {
        let node = Node {
            node_id: "n".into(),
            region: "r".into(),
            ws_url: "ws://n".into(),
            public_ip: "127.0.0.1".into(),
            udp_port: 50000,
        };
        let mut health = Health::new(
            &Registration {
                key: "k".into(),
                lease: "1".into(),
                node,
            },
            "p",
        );
        let pass = Observation {
            loss_pct: Some(0.0),
            rtt_ms: Some(2.0),
            failure: None,
        };
        let fail = Observation::failed(Failure::Timeout);
        for (observation, expected) in [
            (pass, Status::Unknown),
            (pass, Status::Healthy),
            (fail, Status::Healthy),
            (pass, Status::Healthy),
            (fail, Status::Healthy),
            (fail, Status::Unhealthy),
            (pass, Status::Unhealthy),
            (fail, Status::Unhealthy),
            (pass, Status::Unhealthy),
            (pass, Status::Healthy),
        ] {
            health.observe(observation, 2, 2, 1);
            assert_eq!(health.status, expected);
        }
    }
}
