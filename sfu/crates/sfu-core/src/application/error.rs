//! Application errors.

use crate::application::ports::RouterError;
use crate::domain::room::RoomError;
use crate::domain::session::SessionSpecError;

/// Errors returned by the use cases.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppError {
    /// The session spec is invalid.
    #[error("invalid session: {0}")]
    InvalidSession(#[from] SessionSpecError),
    /// Room membership conflict.
    #[error(transparent)]
    Room(#[from] RoomError),
    /// The media router refused the operation.
    #[error(transparent)]
    Router(#[from] RouterError),
}
