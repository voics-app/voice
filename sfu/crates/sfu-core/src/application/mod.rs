//! Use cases of the SFU and the ports they drive.
//!
//! Signaling commands enter through [`service::SfuService`], which dispatches to one use case
//! per file. Media plumbing is behind the [`ports::MediaRouter`] port; notifications leave
//! through [`ports::EventPublisher`].

pub mod close_session;
pub mod create_session;
pub mod error;
pub mod events;
pub mod ports;
pub mod service;
pub mod set_sink_wants;
pub mod sync_sessions;

#[cfg(test)]
pub(crate) mod fakes;
