//! Application errors.

use crate::domain::region::RegionError;

/// Everything a use case can fail with.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppError {
    /// The input violates a region rule.
    #[error(transparent)]
    Region(#[from] RegionError),
    /// The region already has an overlay.
    #[error("region `{0}` already exists")]
    AlreadyExists(String),
    /// The region has no overlay.
    #[error("region `{0}` does not exist")]
    NotFound(String),
    /// The gitops directory could not be read or written.
    #[error("gitops directory: {0}")]
    Store(String),
    /// Cloudflare refused or could not be reached.
    #[error("cloudflare: {0}")]
    Dns(String),
}
