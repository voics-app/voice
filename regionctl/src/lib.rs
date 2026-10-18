//! `regionctl`: scaffolds voice regions in the gitops repository, manages their Cloudflare DNS
//! records and drains them.

pub mod application;
pub mod cli;
pub mod domain;
pub mod infrastructure;
