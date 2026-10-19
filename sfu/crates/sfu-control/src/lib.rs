//! The SFU control plane: signaling talks to the media engine over TCP, one JSON document per
//! line (pinned interface P8).

pub mod contracts;
pub mod mapping;
pub mod server;
