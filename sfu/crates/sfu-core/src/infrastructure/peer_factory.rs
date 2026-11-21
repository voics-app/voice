//! Builds a str0m `Rtc` from a session spec, without any SDP.
//!
//! This is the server half of the "minimal join payload" design: the client sends ICE
//! credentials, DTLS fingerprint, codecs and SSRCs; the SFU configures its peer directly through
//! str0m's direct API and the client synthesizes its remote SDP from SESSION_DESCRIPTION.
//!
//! Conventions shared with the client (documented in `docs/voice-protocol.md`):
//! - every m-line's mid is `a<ssrc>` (audio) or `v<ssrc>` (video) of the track's publisher,
//!   using the publisher's audio SSRC or primary (first) video SSRC;
//! - RTP header extension ids: 1 = ssrc-audio-level, 2 = abs-send-time, 3 = transport-wide-cc;
//! - the SFU is ICE-lite (controlled) and DTLS passive.

use std::net::SocketAddr;
use std::time::Instant;

use str0m::config::DtlsCert;
use str0m::format::{Codec as RtcCodec, FormatParams};
use str0m::ice::IceCreds;
use str0m::media::{Frequency, MediaKind as RtcMediaKind, Mid};
use str0m::rtp::Extension;
use str0m::{Candidate, Rtc, RtcConfig};

use crate::domain::codec::{Codec, VideoCodec};
use crate::domain::session::{IceCredentials, SessionSpec};

/// Initial downlink estimate before BWE converges.
pub const INITIAL_BWE_KBPS: u64 = 1_500;

/// Header extension ids agreed with clients.
pub const EXT_AUDIO_LEVEL: u8 = 1;
/// abs-send-time extension id.
pub const EXT_ABS_SEND_TIME: u8 = 2;
/// transport-wide-cc extension id.
pub const EXT_TRANSPORT_CC: u8 = 3;

/// Mid of a publisher's audio track.
pub fn audio_mid(ssrc: u32) -> Mid {
    Mid::from(format!("a{ssrc}").as_str())
}

/// Mid of a publisher's video track (keyed by its primary video SSRC).
pub fn video_mid(ssrc: u32) -> Mid {
    Mid::from(format!("v{ssrc}").as_str())
}

/// Publisher SSRC encoded in a video mid.
pub fn ssrc_of_video_mid(mid: &Mid) -> Option<u32> {
    mid.strip_prefix('v')?.parse().ok()
}

/// Errors building a peer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerBuildError {
    /// The fingerprint could not be parsed.
    #[error("bad fingerprint: {0}")]
    Fingerprint(String),
    /// The local candidate address is unusable.
    #[error("bad candidate: {0}")]
    Candidate(String),
    /// str0m refused to start DTLS.
    #[error("dtls: {0}")]
    Dtls(String),
}

/// A freshly built peer with its local credentials.
#[derive(Debug)]
pub struct BuiltPeer {
    /// The str0m instance.
    pub rtc: Rtc,
    /// SFU-side ICE credentials.
    pub local_ice: IceCredentials,
    /// SFU DTLS fingerprint.
    pub fingerprint: String,
}

/// Creates ICE-lite peers sharing one DTLS certificate and one public candidate.
#[derive(Debug, Clone)]
pub struct PeerFactory {
    cert: DtlsCert,
    public_addr: SocketAddr,
}

impl PeerFactory {
    /// `public_addr` is the address clients send media to (public IP + mux port).
    pub fn new(cert: DtlsCert, public_addr: SocketAddr) -> Self {
        Self { cert, public_addr }
    }

    /// The public mux address.
    pub fn public_addr(&self) -> SocketAddr {
        self.public_addr
    }

    /// Builds the peer for `spec`, declaring its own inbound tracks.
    pub fn build(&self, spec: &SessionSpec, now: Instant) -> Result<BuiltPeer, PeerBuildError> {
        let mut config = RtcConfig::new()
            .set_ice_lite(true)
            .set_rtp_mode(true)
            .set_dtls_cert(self.cert.clone())
            .enable_bwe(Some(str0m::bwe::Bitrate::kbps(INITIAL_BWE_KBPS)))
            .set_stats_interval(None)
            .clear_codecs()
            .clear_extension_map()
            .set_extension(EXT_AUDIO_LEVEL, Extension::AudioLevel)
            .set_extension(EXT_ABS_SEND_TIME, Extension::AbsoluteSendTime)
            .set_extension(EXT_TRANSPORT_CC, Extension::TransportSequenceNumber);
        if let Some(local) = &spec.local_ice {
            config = config.set_local_ice_credentials(IceCreds {
                ufrag: local.ufrag.clone(),
                pass: local.pwd.clone(),
            });
        }
        for codec in &spec.codecs {
            let (rtc_codec, clock, channels, format) = rtc_codec(codec.codec);
            config.codec_config().add_config(
                codec.payload_type.into(),
                codec.rtx_payload_type.map(Into::into),
                rtc_codec,
                clock,
                channels,
                format,
            );
        }

        let mut rtc = config.build(now);
        let candidate = Candidate::host(self.public_addr, "udp")
            .map_err(|e| PeerBuildError::Candidate(e.to_string()))?;
        rtc.add_local_candidate(candidate);

        let fingerprint = spec
            .remote_fingerprint
            .parse()
            .map_err(PeerBuildError::Fingerprint)?;

        let mut api = rtc.direct_api();
        api.set_ice_controlling(false);
        api.set_remote_ice_credentials(IceCreds {
            ufrag: spec.remote_ice.ufrag.clone(),
            pass: spec.remote_ice.pwd.clone(),
        });
        api.set_remote_fingerprint(fingerprint);
        api.enable_twcc_feedback();
        api.start_dtls(false)
            .map_err(|e| PeerBuildError::Dtls(e.to_string()))?;

        let own_audio = audio_mid(spec.audio_ssrc);
        api.declare_media(own_audio, RtcMediaKind::Audio);
        api.expect_stream_rx(spec.audio_ssrc.into(), None, own_audio, None);
        if let Some(ssrc) = spec.soundboard_ssrc {
            let mid = audio_mid(ssrc);
            api.declare_media(mid, RtcMediaKind::Audio);
            api.expect_stream_rx(ssrc.into(), None, mid, None);
        }
        if let Some(primary) = spec.primary_video() {
            let own_video = video_mid(primary.ssrc);
            api.declare_media(own_video, RtcMediaKind::Video);
            for layer in &spec.video_layers {
                api.expect_stream_rx(
                    layer.ssrc.into(),
                    Some(layer.rtx_ssrc.into()),
                    own_video,
                    Some(layer.rid.as_str().into()),
                );
            }
        }
        if let Some(screen) = &spec.screen {
            let own_screen = video_mid(screen.ssrc);
            api.declare_media(own_screen, RtcMediaKind::Video);
            let rtx = (screen.rtx_ssrc != 0).then(|| screen.rtx_ssrc.into());
            api.expect_stream_rx(screen.ssrc.into(), rtx, own_screen, None);
        }

        let creds = api.local_ice_credentials();
        let fingerprint = api.local_dtls_fingerprint().to_string();
        Ok(BuiltPeer {
            rtc,
            local_ice: IceCredentials {
                ufrag: creds.ufrag,
                pwd: creds.pass,
            },
            fingerprint,
        })
    }
}

fn rtc_codec(codec: Codec) -> (RtcCodec, Frequency, Option<u8>, FormatParams) {
    match codec {
        Codec::Opus => (
            RtcCodec::Opus,
            Frequency::FORTY_EIGHT_KHZ,
            Some(2),
            FormatParams {
                min_p_time: Some(10),
                use_inband_fec: Some(true),
                use_dtx: Some(true),
                ..FormatParams::default()
            },
        ),
        Codec::Video(video) => {
            let rtc = match video {
                VideoCodec::Vp8 => RtcCodec::Vp8,
                VideoCodec::Vp9 => RtcCodec::Vp9,
                VideoCodec::H264 => RtcCodec::H264,
                VideoCodec::Av1 => RtcCodec::Av1,
            };
            let format = if video == VideoCodec::H264 {
                FormatParams {
                    level_asymmetry_allowed: Some(true),
                    packetization_mode: Some(1),
                    ..FormatParams::default()
                }
            } else {
                FormatParams::default()
            };
            (rtc, Frequency::NINETY_KHZ, None, format)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mids_round_trip() {
        assert_eq!(audio_mid(42).to_string(), "a42");
        assert_eq!(video_mid(4_294_967_295).to_string(), "v4294967295");
        assert_eq!(ssrc_of_video_mid(&video_mid(77)), Some(77));
        assert_eq!(ssrc_of_video_mid(&audio_mid(77)), None);
    }
}
