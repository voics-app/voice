//! Ports of the use cases.

use std::net::Ipv4Addr;

use super::error::AppError;

/// The gitops checkout: `regions/<name>/kustomization.yaml` and `regions/kustomization.yaml`.
pub trait RegionStore {
    /// Node overlay directories (excluding `_template`, `common` and the `dev` aggregate), sorted.
    fn names(&self) -> Result<Vec<String>, AppError>;
    /// The overlay of a region, if it exists.
    fn read_overlay(&self, name: &str) -> Result<Option<String>, AppError>;
    /// Creates or replaces the overlay of a region.
    fn write_overlay(&self, name: &str, content: &str) -> Result<(), AppError>;
    /// `regions/kustomization.yaml`.
    fn read_root(&self) -> Result<String, AppError>;
    /// Replaces `regions/kustomization.yaml`.
    fn write_root(&self, content: &str) -> Result<(), AppError>;
}

/// An A record as Cloudflare reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsRecord {
    /// Address the record points to.
    pub ip: String,
    /// Whether Cloudflare proxies the host.
    pub proxied: bool,
}

/// What `upsert_a_record` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsChange {
    /// A new record was created.
    Created,
    /// An existing record was updated.
    Updated,
    /// The record already matched.
    Unchanged,
}

/// Cloudflare DNS.
pub trait DnsGateway {
    /// The A record of `host`, if any.
    fn find_a_record(&self, host: &str) -> Result<Option<DnsRecord>, AppError>;
    /// Makes `host` an A record to `ip`.
    fn upsert_a_record(
        &self,
        host: &str,
        ip: Ipv4Addr,
        proxied: bool,
    ) -> Result<DnsChange, AppError>;
}

/// The gateway internal API that drains live voice nodes.
pub trait NodeDrainGateway {
    /// Drains (`true`) or undrains (`false`) the node.
    fn set_drained(
        &self,
        node: &crate::domain::region::NodeId,
        drained: bool,
    ) -> Result<(), AppError>;
}
