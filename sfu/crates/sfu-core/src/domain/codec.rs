//! Codecs the SFU knows how to route.

use std::fmt;
use std::str::FromStr;

/// Media kind of a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaKind {
    /// Opus audio.
    Audio,
    /// Simulcast or SVC video.
    Video,
}

/// Video codecs supported for forwarding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoCodec {
    /// VP8 (simulcast).
    Vp8,
    /// VP9 (simulcast or SVC).
    Vp9,
    /// H.264 (simulcast).
    H264,
    /// AV1 (simulcast).
    Av1,
}

/// A codec as announced by the client in SELECT_PROTOCOL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    /// Opus at 48 kHz, stereo signalling.
    Opus,
    /// A video codec.
    Video(VideoCodec),
}

impl Codec {
    /// RTP clock rate in Hz.
    pub fn clock_rate(self) -> u32 {
        match self {
            Codec::Opus => 48_000,
            Codec::Video(_) => 90_000,
        }
    }

    /// Media kind carried by the codec.
    pub fn kind(self) -> MediaKind {
        match self {
            Codec::Opus => MediaKind::Audio,
            Codec::Video(_) => MediaKind::Video,
        }
    }
}

/// Error for an unsupported codec name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unsupported codec `{0}`")]
pub struct UnsupportedCodec(pub String);

impl FromStr for Codec {
    type Err = UnsupportedCodec;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name.to_ascii_uppercase().as_str() {
            "OPUS" => Ok(Codec::Opus),
            "VP8" => Ok(Codec::Video(VideoCodec::Vp8)),
            "VP9" => Ok(Codec::Video(VideoCodec::Vp9)),
            "H264" => Ok(Codec::Video(VideoCodec::H264)),
            "AV1" => Ok(Codec::Video(VideoCodec::Av1)),
            _ => Err(UnsupportedCodec(name.to_owned())),
        }
    }
}

impl fmt::Display for Codec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Codec::Opus => "opus",
            Codec::Video(VideoCodec::Vp8) => "VP8",
            Codec::Video(VideoCodec::Vp9) => "VP9",
            Codec::Video(VideoCodec::H264) => "H264",
            Codec::Video(VideoCodec::Av1) => "AV1",
        };
        f.write_str(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_case_insensitively() {
        assert_eq!("opus".parse(), Ok(Codec::Opus));
        assert_eq!("vp9".parse(), Ok(Codec::Video(VideoCodec::Vp9)));
        assert_eq!("H264".parse(), Ok(Codec::Video(VideoCodec::H264)));
        assert_eq!("Av1".parse(), Ok(Codec::Video(VideoCodec::Av1)));
        assert!("G722".parse::<Codec>().is_err());
    }

    #[test]
    fn display_round_trips() {
        for c in [
            Codec::Opus,
            Codec::Video(VideoCodec::Vp8),
            Codec::Video(VideoCodec::Vp9),
            Codec::Video(VideoCodec::H264),
            Codec::Video(VideoCodec::Av1),
        ] {
            assert_eq!(c.to_string().parse(), Ok(c));
        }
    }

    #[test]
    fn clock_rates_and_kinds() {
        assert_eq!(Codec::Opus.clock_rate(), 48_000);
        assert_eq!(Codec::Video(VideoCodec::Vp8).clock_rate(), 90_000);
        assert_eq!(Codec::Opus.kind(), MediaKind::Audio);
        assert_eq!(Codec::Video(VideoCodec::Av1).kind(), MediaKind::Video);
    }
}
