//! Use case: apply a subscriber's MEDIA_SINK_WANTS.

use crate::application::error::AppError;
use crate::application::ports::MediaRouter;
use crate::domain::session::SessionId;
use crate::domain::sink_wants::SinkWants;

/// Replaces the subscriber's quality wishes, keyed by publisher SSRC.
pub fn set_sink_wants(
    router: &mut dyn MediaRouter,
    id: &SessionId,
    wants: impl IntoIterator<Item = (u32, u8)>,
) -> Result<(), AppError> {
    let mut sink_wants = SinkWants::new();
    sink_wants.replace(wants);
    router.set_sink_wants(id, sink_wants)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::FakeRouter;
    use crate::application::ports::RouterError;

    #[test]
    fn forwards_clamped_wants_to_router() {
        let mut router = FakeRouter::default();
        set_sink_wants(&mut router, &"a".into(), [(5, 200), (6, 0)]).unwrap();
        let (id, wants) = &router.wants[0];
        assert_eq!(id.0, "a");
        assert_eq!(wants.quality_for(5), 100);
        assert!(!wants.wants(6));
    }

    #[test]
    fn unknown_session_is_an_error() {
        let mut router = FakeRouter {
            known_only: true,
            ..FakeRouter::default()
        };
        let err = set_sink_wants(&mut router, &"a".into(), []).unwrap_err();
        assert_eq!(
            err,
            AppError::Router(RouterError::UnknownSession("a".into()))
        );
    }
}
