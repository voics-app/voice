//! The description of one media session, as requested by signaling with `create_session`.

use std::collections::HashSet;
use std::fmt;

use super::codec::{Codec, MediaKind};

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub String);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
    };
}

string_id!(
    /// Voice session id (the gateway session id of the user).
    SessionId
);
string_id!(
    /// Media room: one voice channel.
    RoomId
);
string_id!(
    /// User id (Snowflake as a string).
    UserId
);

/// ICE username fragment and password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IceCredentials {
    /// `ice-ufrag`.
    pub ufrag: String,
    /// `ice-pwd`.
    pub pwd: String,
}

/// A codec with the payload types the client chose for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecSpec {
    /// Codec.
    pub codec: Codec,
    /// RTP payload type.
    pub payload_type: u8,
    /// RTX payload type (video only).
    pub rtx_payload_type: Option<u8>,
    /// Client preference, higher is preferred.
    pub priority: u32,
}

/// One simulcast layer the publisher may send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoLayer {
    /// RTP stream id.
    pub rid: String,
    /// Quality label 0-100.
    pub quality: u8,
    /// Media SSRC.
    pub ssrc: u32,
    /// Retransmission SSRC.
    pub rtx_ssrc: u32,
}

/// Everything needed to create a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec {
    /// Session id.
    pub id: SessionId,
    /// Room the session joins.
    pub room: RoomId,
    /// Owner of the session.
    pub user: UserId,
    /// The client's ICE credentials.
    pub remote_ice: IceCredentials,
    /// The client's DTLS fingerprint, e.g. `sha-256 AB:CD:...`.
    pub remote_fingerprint: String,
    /// Local ICE credentials to reuse (crash recovery replay); generated when absent.
    pub local_ice: Option<IceCredentials>,
    /// Negotiated codecs.
    pub codecs: Vec<CodecSpec>,
    /// SSRC of the publisher's audio.
    pub audio_ssrc: u32,
    /// Whether audio from this session may be forwarded.
    pub can_publish_audio: bool,
    /// Whether camera and screen media may be forwarded.
    pub can_publish_video: bool,
    /// Video layers, ascending quality. Empty when the client does not send video.
    pub video_layers: Vec<VideoLayer>,
    /// The publisher's screen share: a separate single-layer stream beside the camera, forwarded
    /// to subscribers under its own SSRC.
    pub screen: Option<VideoLayer>,
}

/// Why a session spec was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionSpecError {
    /// ICE credentials violate RFC 8445 length rules.
    #[error("invalid ICE credentials")]
    InvalidIceCredentials,
    /// The fingerprint is not `<hash-func> <hex:pairs>`.
    #[error("invalid DTLS fingerprint")]
    InvalidFingerprint,
    /// Opus is mandatory.
    #[error("opus codec missing")]
    MissingOpus,
    /// Video layers were declared without a video codec.
    #[error("video layers declared without a video codec")]
    MissingVideoCodec,
    /// Two codecs or streams share a payload type or SSRC.
    #[error("duplicate {0}")]
    Duplicate(&'static str),
    /// A quality label above 100.
    #[error("quality must be within 0..=100")]
    InvalidQuality,
    /// SSRC 0 is reserved for probing.
    #[error("ssrc 0 is not allowed")]
    ZeroSsrc,
}

impl SessionSpec {
    /// Validates the invariants of a session.
    pub fn validate(&self) -> Result<(), SessionSpecError> {
        validate_ice(&self.remote_ice)?;
        if let Some(local) = &self.local_ice {
            validate_ice(local)?;
        }
        validate_fingerprint(&self.remote_fingerprint)?;
        if self.audio_codec().is_none() {
            return Err(SessionSpecError::MissingOpus);
        }
        if (!self.video_layers.is_empty() || self.screen.is_some()) && self.video_codec().is_none()
        {
            return Err(SessionSpecError::MissingVideoCodec);
        }
        let mut pts = HashSet::new();
        for c in &self.codecs {
            for pt in std::iter::once(c.payload_type).chain(c.rtx_payload_type) {
                if !pts.insert(pt) {
                    return Err(SessionSpecError::Duplicate("payload type"));
                }
            }
        }
        let mut ssrcs = HashSet::new();
        for ssrc in self.all_ssrcs() {
            if ssrc == 0 {
                return Err(SessionSpecError::ZeroSsrc);
            }
            if !ssrcs.insert(ssrc) {
                return Err(SessionSpecError::Duplicate("ssrc"));
            }
        }
        if self.video_layers.iter().any(|l| l.quality > 100) {
            return Err(SessionSpecError::InvalidQuality);
        }
        Ok(())
    }

    /// The Opus codec spec.
    pub fn audio_codec(&self) -> Option<&CodecSpec> {
        self.codecs.iter().find(|c| c.codec == Codec::Opus)
    }

    /// The preferred video codec spec.
    pub fn video_codec(&self) -> Option<&CodecSpec> {
        self.codecs
            .iter()
            .filter(|c| c.codec.kind() == MediaKind::Video)
            .max_by_key(|c| c.priority)
    }

    /// Payload type this session uses for `codec`.
    pub fn payload_type_for(&self, codec: Codec) -> Option<u8> {
        self.codecs
            .iter()
            .find(|c| c.codec == codec)
            .map(|c| c.payload_type)
    }

    /// Codec of an inbound payload type.
    pub fn codec_for_payload_type(&self, pt: u8) -> Option<Codec> {
        self.codecs
            .iter()
            .find(|c| c.payload_type == pt)
            .map(|c| c.codec)
    }

    /// SSRC under which subscribers receive this publisher's video: the first layer's SSRC
    /// (and its RTX SSRC), matching the VIDEO opcode's `video_ssrc`/`rtx_ssrc`.
    pub fn primary_video(&self) -> Option<&VideoLayer> {
        self.video_layers.first()
    }

    /// Whether `ssrc` is this publisher's screen share.
    pub fn is_screen(&self, ssrc: u32) -> bool {
        self.screen.as_ref().is_some_and(|s| s.ssrc == ssrc)
    }

    /// Layer index of an inbound video SSRC.
    pub fn layer_index(&self, ssrc: u32) -> Option<usize> {
        self.video_layers.iter().position(|l| l.ssrc == ssrc)
    }

    fn all_ssrcs(&self) -> impl Iterator<Item = u32> + '_ {
        std::iter::once(self.audio_ssrc)
            .chain(self.video_layers.iter().flat_map(|l| [l.ssrc, l.rtx_ssrc]))
            .chain(self.screen.iter().map(|s| s.ssrc))
            .chain(
                self.screen
                    .iter()
                    .map(|s| s.rtx_ssrc)
                    .filter(|&rtx| rtx != 0),
            )
    }
}

fn validate_ice(creds: &IceCredentials) -> Result<(), SessionSpecError> {
    let ice_char = |c: char| c.is_ascii_alphanumeric() || c == '+' || c == '/';
    let ufrag_ok = (4..=256).contains(&creds.ufrag.len()) && creds.ufrag.chars().all(ice_char);
    let pwd_ok = (22..=256).contains(&creds.pwd.len()) && creds.pwd.chars().all(ice_char);
    if ufrag_ok && pwd_ok {
        Ok(())
    } else {
        Err(SessionSpecError::InvalidIceCredentials)
    }
}

fn validate_fingerprint(fingerprint: &str) -> Result<(), SessionSpecError> {
    let Some((hash, hex)) = fingerprint.split_once(' ') else {
        return Err(SessionSpecError::InvalidFingerprint);
    };
    let hex_ok = hex
        .split(':')
        .all(|pair| pair.len() == 2 && pair.chars().all(|c| c.is_ascii_hexdigit()));
    if hash.is_empty() || !hex_ok {
        return Err(SessionSpecError::InvalidFingerprint);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::domain::codec::VideoCodec;

    pub fn spec(id: &str, room: &str, audio_ssrc: u32) -> SessionSpec {
        SessionSpec {
            id: id.into(),
            room: room.into(),
            user: format!("user-{id}").as_str().into(),
            remote_ice: IceCredentials {
                ufrag: "abcd".into(),
                pwd: "0123456789012345678901".into(),
            },
            remote_fingerprint: "sha-256 AB:CD:EF".into(),
            local_ice: None,
            codecs: vec![
                CodecSpec {
                    codec: Codec::Opus,
                    payload_type: 111,
                    rtx_payload_type: None,
                    priority: 1000,
                },
                CodecSpec {
                    codec: Codec::Video(VideoCodec::Vp8),
                    payload_type: 96,
                    rtx_payload_type: Some(97),
                    priority: 1000,
                },
            ],
            audio_ssrc,
            can_publish_audio: true,
            can_publish_video: true,
            video_layers: vec![
                VideoLayer {
                    rid: "q".into(),
                    quality: 25,
                    ssrc: audio_ssrc + 1,
                    rtx_ssrc: audio_ssrc + 2,
                },
                VideoLayer {
                    rid: "f".into(),
                    quality: 100,
                    ssrc: audio_ssrc + 3,
                    rtx_ssrc: audio_ssrc + 4,
                },
            ],
            screen: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::spec;
    use super::*;
    use crate::domain::codec::VideoCodec;

    #[test]
    fn valid_spec_passes() {
        assert_eq!(spec("s1", "r", 100).validate(), Ok(()));
    }

    #[test]
    fn short_ice_password_is_rejected() {
        let mut s = spec("s1", "r", 100);
        s.remote_ice.pwd = "short".into();
        assert_eq!(s.validate(), Err(SessionSpecError::InvalidIceCredentials));
    }

    #[test]
    fn malformed_fingerprint_is_rejected() {
        let mut s = spec("s1", "r", 100);
        s.remote_fingerprint = "sha-256 ZZ:01".into();
        assert_eq!(s.validate(), Err(SessionSpecError::InvalidFingerprint));
        s.remote_fingerprint = "nofingerprint".into();
        assert_eq!(s.validate(), Err(SessionSpecError::InvalidFingerprint));
    }

    #[test]
    fn opus_is_mandatory() {
        let mut s = spec("s1", "r", 100);
        s.codecs.retain(|c| c.codec != Codec::Opus);
        assert_eq!(s.validate(), Err(SessionSpecError::MissingOpus));
    }

    #[test]
    fn video_layers_need_a_video_codec() {
        let mut s = spec("s1", "r", 100);
        s.codecs.retain(|c| c.codec == Codec::Opus);
        assert_eq!(s.validate(), Err(SessionSpecError::MissingVideoCodec));
    }

    #[test]
    fn duplicate_ssrc_and_payload_types_are_rejected() {
        let mut s = spec("s1", "r", 100);
        s.video_layers[1].ssrc = 100;
        assert_eq!(s.validate(), Err(SessionSpecError::Duplicate("ssrc")));
        let mut s = spec("s1", "r", 100);
        s.codecs[1].rtx_payload_type = Some(111);
        assert_eq!(
            s.validate(),
            Err(SessionSpecError::Duplicate("payload type"))
        );
    }

    #[test]
    fn zero_ssrc_is_rejected() {
        let mut s = spec("s1", "r", 100);
        s.audio_ssrc = 0;
        assert_eq!(s.validate(), Err(SessionSpecError::ZeroSsrc));
    }

    #[test]
    fn lookups() {
        let s = spec("s1", "r", 100);
        assert_eq!(s.payload_type_for(Codec::Opus), Some(111));
        assert_eq!(
            s.codec_for_payload_type(96),
            Some(Codec::Video(VideoCodec::Vp8))
        );
        assert_eq!(s.layer_index(103), Some(1));
        assert_eq!(s.layer_index(104), None);
        assert_eq!(s.primary_video().map(|l| l.ssrc), Some(101));
    }

    #[test]
    fn preferred_video_codec_by_priority() {
        let mut s = spec("s1", "r", 100);
        s.codecs.push(CodecSpec {
            codec: Codec::Video(VideoCodec::Vp9),
            payload_type: 98,
            rtx_payload_type: Some(99),
            priority: 2000,
        });
        assert_eq!(
            s.video_codec().map(|c| c.codec),
            Some(Codec::Video(VideoCodec::Vp9))
        );
    }
}
