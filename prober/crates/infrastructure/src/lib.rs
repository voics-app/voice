//! Network adapters implement application ports; credentials never enter diagnostic values.
pub mod etcd;
pub mod media;
mod wire;

use etcd::{Claim, Error, Etcd};
use probe_application::{Clock, HealthStore, MediaProbe, NodeDiscovery};
use probe_domain::{Health, Observation, Registration};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy)]
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RtcProbe;
impl MediaProbe for RtcProbe {
    async fn probe(
        &self,
        registration: &Registration,
        config: &probe_application::Config,
    ) -> Observation {
        media::probe(registration, config).await
    }
}

impl HealthStore for Etcd {
    type Claim = Claim;
    async fn acquire(&self, registration: &Registration, ttl_s: u64) -> Result<Claim, Error> {
        self.acquire(registration, ttl_s).await
    }
    async fn renew(&self, claim: &Claim) -> Result<(), Error> {
        self.keep_alive(claim).await
    }
    async fn publish(
        &self,
        registration: &Registration,
        claim: &Claim,
        health: &Health,
    ) -> Result<(), Error> {
        self.publish(registration, claim, health).await
    }
    async fn release(&self, claim: &Claim) -> Result<(), Error> {
        self.revoke(&claim.lease).await
    }
}

impl NodeDiscovery for Etcd {
    async fn registrations(&self) -> Result<Vec<Registration>, Error> {
        self.registrations().await
    }
}
