//! Translation between wire shapes and the SFU application model.

use sfu_core::application::error::AppError;
use sfu_core::application::events::SfuEvent;
use sfu_core::application::service::{Command, Outcome};
use sfu_core::domain::codec::{Codec, MediaKind, UnsupportedCodec};
use sfu_core::domain::session::{CodecSpec, IceCredentials, SessionId, SessionSpec, VideoLayer};

use crate::contracts::{Request, WireCommand, WireEvent, WireSession};

/// A request that cannot be mapped to a command.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MappingError {
    /// A codec name or kind is unknown.
    #[error("{0}")]
    Codec(String),
    /// A sink-wants key is not an SSRC.
    #[error("sink wants key `{0}` is not an ssrc")]
    WantsKey(String),
}

/// Maps a wire request to an application command.
pub fn to_command(request: Request) -> Result<Command, MappingError> {
    match request.command {
        WireCommand::CreateSession(session) => {
            Ok(Command::CreateSession(Box::new(to_spec(session)?)))
        }
        WireCommand::CloseSession { session_id } => {
            Ok(Command::CloseSession(session_id.as_str().into()))
        }
        WireCommand::SetSinkWants { session_id, wants } => {
            let wants = wants
                .into_iter()
                .map(|(key, quality)| {
                    key.parse::<u32>()
                        .map(|ssrc| (ssrc, quality))
                        .map_err(|_| MappingError::WantsKey(key))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Command::SetSinkWants {
                session_id: session_id.as_str().into(),
                wants,
            })
        }
        WireCommand::Sync { session_ids } => Ok(Command::Sync(
            session_ids
                .iter()
                .map(|id| SessionId::from(id.as_str()))
                .collect(),
        )),
    }
}

/// The session a command addresses, echoed in its reply.
pub fn session_of(command: &Command) -> Option<String> {
    match command {
        Command::CreateSession(spec) => Some(spec.id.0.clone()),
        Command::CloseSession(id) => Some(id.0.clone()),
        Command::SetSinkWants { session_id, .. } => Some(session_id.0.clone()),
        Command::Sync(_) => None,
    }
}

fn to_spec(session: WireSession) -> Result<SessionSpec, MappingError> {
    let codecs = session
        .codecs
        .into_iter()
        .map(|c| {
            let codec: Codec = c
                .name
                .parse()
                .map_err(|e: UnsupportedCodec| MappingError::Codec(e.to_string()))?;
            let expected = match codec.kind() {
                MediaKind::Audio => "audio",
                MediaKind::Video => "video",
            };
            if c.kind != expected {
                return Err(MappingError::Codec(format!(
                    "codec `{}` is {expected}, not {}",
                    c.name, c.kind
                )));
            }
            Ok(CodecSpec {
                codec,
                payload_type: c.payload_type,
                rtx_payload_type: c.rtx_payload_type,
                priority: c.priority,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let local_ice = match (session.local_ufrag, session.local_pwd) {
        (Some(ufrag), Some(pwd)) => Some(IceCredentials { ufrag, pwd }),
        _ => None,
    };
    Ok(SessionSpec {
        id: session.session_id.as_str().into(),
        room: session.room_id.as_str().into(),
        user: session.user_id.as_str().into(),
        remote_ice: IceCredentials {
            ufrag: session.ice_ufrag,
            pwd: session.ice_pwd,
        },
        remote_fingerprint: session.remote_fingerprint,
        local_ice,
        codecs,
        audio_ssrc: session.audio_ssrc,
        soundboard_ssrc: session.soundboard_ssrc,
        can_publish_soundboard: session.can_publish_soundboard,
        video_layers: session
            .video_layers
            .into_iter()
            .map(|l| VideoLayer {
                rid: l.rid,
                quality: l.quality,
                ssrc: l.ssrc,
                rtx_ssrc: l.rtx_ssrc,
            })
            .collect(),
        screen: session.screen.map(|s| VideoLayer {
            rid: "screen".to_owned(),
            quality: 100,
            ssrc: s.ssrc,
            rtx_ssrc: s.rtx_ssrc,
        }),
    })
}

/// Maps a successful command outcome to its reply.
pub fn reply_for(req: Option<String>, session: Option<String>, outcome: Outcome) -> WireEvent {
    let session_id = session.unwrap_or_default();
    match outcome {
        Outcome::Created(local) => WireEvent::SessionCreated {
            req,
            session_id,
            local_ufrag: local.ice.ufrag,
            local_pwd: local.ice.pwd,
            fingerprint: local.fingerprint,
        },
        Outcome::Closed(existed) => WireEvent::Closed {
            req,
            session_id,
            existed,
        },
        Outcome::WantsApplied => WireEvent::SinkWantsApplied { req, session_id },
        Outcome::Synced(missing) => WireEvent::Synced {
            req,
            missing: missing.into_iter().map(|s| s.0).collect(),
        },
    }
}

/// Maps an application failure to an error reply.
pub fn error_reply(req: Option<String>, error: &AppError) -> WireEvent {
    let code = match error {
        AppError::InvalidSession(_) => "invalid_session",
        AppError::Room(_) => "room_conflict",
        AppError::Router(_) => "router_error",
    };
    WireEvent::Error {
        req,
        code: code.to_owned(),
        message: error.to_string(),
    }
}

/// An error reply for a line that is not a valid request.
pub fn invalid_request_reply(req: Option<String>, message: String) -> WireEvent {
    WireEvent::Error {
        req,
        code: "invalid_request".to_owned(),
        message,
    }
}

/// Maps an unsolicited engine event to the wire. Replies travel back on the requesting
/// connection only, so they are not broadcast.
pub fn broadcast_for(event: SfuEvent) -> Option<WireEvent> {
    match event {
        SfuEvent::SessionClosed { session_id, reason } => Some(WireEvent::SessionClosed {
            session_id: session_id.0,
            reason: reason.as_str().to_owned(),
        }),
        SfuEvent::Speaking {
            session_id,
            ssrc,
            speaking,
        } => Some(WireEvent::Speaking {
            session_id: session_id.0,
            ssrc,
            speaking,
        }),
        SfuEvent::StreamViewers {
            session_id,
            ssrc,
            viewers,
        } => Some(WireEvent::StreamViewers {
            session_id: session_id.0,
            ssrc,
            viewers: viewers.into_iter().map(|user| user.0).collect(),
        }),
        SfuEvent::Stats(stats) => Some(WireEvent::Stats {
            sessions: stats.sessions,
            rooms: stats.rooms,
            ingress_bps: stats.ingress_bps,
            egress_bps: stats.egress_bps,
        }),
        SfuEvent::SessionCreated { .. } | SfuEvent::Synced { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Request {
        serde_json::from_str(line).expect("valid request")
    }

    fn create_with_codec(codec: &str) -> Request {
        parse(&format!(
            r#"{{"cmd":"create_session","session_id":"s","room_id":"r","user_id":"u",
            "ice_ufrag":"abcd","ice_pwd":"0123456789012345678901",
            "remote_fingerprint":"sha-256 AB","audio_ssrc":5,"codecs":[{codec}]}}"#
        ))
    }

    #[test]
    fn maps_create_session() {
        let request = parse(
            r#"{"req":"1","cmd":"create_session","session_id":"s","room_id":"r","user_id":"u",
            "ice_ufrag":"abcd","ice_pwd":"0123456789012345678901",
            "remote_fingerprint":"sha-256 AB:CD","audio_ssrc":5,
            "codecs":[{"name":"opus","type":"audio","priority":1,"payload_type":111},
            {"name":"VP8","type":"video","priority":1,"payload_type":96,"rtx_payload_type":97}],
            "video_layers":[{"rid":"q","quality":50,"ssrc":6,"rtx_ssrc":7}]}"#,
        );
        assert_eq!(request.req.as_deref(), Some("1"));
        let Command::CreateSession(spec) = to_command(request).unwrap() else {
            panic!("expected create_session");
        };
        assert_eq!(spec.id, "s".into());
        assert!(!spec.can_publish_soundboard);
        assert_eq!(spec.codecs.len(), 2);
        assert_eq!(spec.video_layers[0].rtx_ssrc, 7);
        assert!(spec.local_ice.is_none());
    }

    #[test]
    fn rejects_codec_kind_mismatch_and_unknown_codec() {
        let mismatch =
            create_with_codec(r#"{"name":"opus","type":"video","priority":1,"payload_type":1}"#);
        let unknown =
            create_with_codec(r#"{"name":"speex","type":"audio","priority":1,"payload_type":1}"#);
        assert!(to_command(mismatch).is_err());
        assert!(to_command(unknown).is_err());
    }

    #[test]
    fn maps_sink_wants_keys_to_ssrcs() {
        let ok = to_command(parse(
            r#"{"cmd":"set_sink_wants","session_id":"s","wants":{"42":80}}"#,
        ))
        .unwrap();
        assert_eq!(
            ok,
            Command::SetSinkWants {
                session_id: "s".into(),
                wants: vec![(42, 80)]
            }
        );
        let bad = to_command(parse(
            r#"{"cmd":"set_sink_wants","session_id":"s","wants":{"x":80}}"#,
        ));
        assert_eq!(bad, Err(MappingError::WantsKey("x".into())));
    }

    #[test]
    fn replies_are_not_broadcast() {
        assert!(broadcast_for(SfuEvent::Synced { missing: vec![] }).is_none());
    }

    #[test]
    fn events_serialize_with_evt_tag() {
        let json = serde_json::to_string(&WireEvent::Speaking {
            session_id: "s".into(),
            ssrc: 9,
            speaking: true,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"evt":"speaking","session_id":"s","ssrc":9,"speaking":true}"#
        );
    }

    #[test]
    fn stream_viewers_serialize_user_ids() {
        let wire = broadcast_for(SfuEvent::StreamViewers {
            session_id: "s".into(),
            ssrc: 9,
            viewers: vec!["u1".into(), "u2".into()],
        })
        .unwrap();
        assert_eq!(
            serde_json::to_string(&wire).unwrap(),
            r#"{"evt":"stream_viewers","session_id":"s","ssrc":9,"viewers":["u1","u2"]}"#
        );
    }
}
