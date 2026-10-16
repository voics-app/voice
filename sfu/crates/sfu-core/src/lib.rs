//! Discordy SFU core.
//!
//! - [`domain`]: pure media-routing rules (no IO, no str0m).
//! - [`application`]: use cases and the ports they drive.
//! - [`infrastructure`]: the str0m-based media engine and the UDP mux.

pub mod domain;
