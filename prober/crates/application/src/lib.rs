//! Use cases depend only on domain values and explicit ports.
mod config;
pub use config::{Config, etcd_endpoints};
mod observer;
pub use observer::{Observer, Services};

use probe_domain::{Health, Observation, Registration};
use std::fmt::Debug;
use std::future::Future;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    Unavailable,
    InvalidResponse,
    OwnershipLost,
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

pub trait HealthStore: Send + Sync {
    type Claim: Debug + Send + Sync;
    fn acquire<'a>(
        &'a self,
        registration: &'a Registration,
        ttl_s: u64,
    ) -> impl Future<Output = Result<Self::Claim, StoreError>> + Send + 'a;
    fn renew<'a>(
        &'a self,
        claim: &'a Self::Claim,
    ) -> impl Future<Output = Result<(), StoreError>> + Send + 'a;
    fn publish<'a>(
        &'a self,
        registration: &'a Registration,
        claim: &'a Self::Claim,
        health: &'a Health,
    ) -> impl Future<Output = Result<(), StoreError>> + Send + 'a;
    fn release<'a>(
        &'a self,
        claim: &'a Self::Claim,
    ) -> impl Future<Output = Result<(), StoreError>> + Send + 'a;
}

pub trait MediaProbe: Send + Sync {
    fn probe<'a>(
        &'a self,
        registration: &'a Registration,
        config: &'a Config,
    ) -> impl Future<Output = Observation> + Send + 'a;
}

pub trait NodeDiscovery: HealthStore {
    fn registrations(&self) -> impl Future<Output = Result<Vec<Registration>, StoreError>> + Send;
}

pub trait ObservationSink: Send + Sync + Debug {
    fn report(&self, registration: &Registration, health: Option<&Health>);
    fn remove(&self, node_id: &str);
}
