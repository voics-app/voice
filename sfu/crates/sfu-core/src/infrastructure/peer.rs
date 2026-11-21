//! One session's str0m peer plus its per-publisher outbound state.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use str0m::Rtc;
use str0m::media::MediaKind as RtcMediaKind;
use str0m::rtp::{ExtensionValues, RtpWrite};

use super::peer_factory::{audio_mid, video_mid};
use crate::domain::bitrate_meter::BitrateMeter;
use crate::domain::codec::{Codec, VideoCodec};
use crate::domain::layer_selector::{LayerDecision, LayerSelector};
use crate::domain::rtp_rewriter::{InboundPacket, RtpRewriter};
use crate::domain::session::{SessionId, SessionSpec};
use crate::domain::sink_wants::SinkWants;
use crate::domain::speaking::SpeakingDetector;
use crate::domain::svc_filter::{SvcDecision, SvcFilter, SvcTarget};
use crate::domain::vp9_descriptor::Vp9Descriptor;

/// What an inbound packet is, from the publisher's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    /// The publisher's audio.
    Audio,
    /// Separate soundboard audio.
    Soundboard,
    /// One video layer (index into the publisher's layers).
    VideoLayer(usize),
    /// The publisher's screen share.
    Screen,
}

/// An inbound RTP packet ready to be fanned out.
#[derive(Debug, Clone)]
pub struct InboundRtp {
    /// Publisher session.
    pub publisher: SessionId,
    /// Track of the publisher the packet belongs to.
    pub track: TrackKind,
    /// Codec of the payload.
    pub codec: Codec,
    /// SSRC the packet arrived on.
    pub ssrc: u32,
    /// Extended sequence number.
    pub seq: u64,
    /// RTP timestamp.
    pub timestamp: u32,
    /// Marker bit.
    pub marker: bool,
    /// Audio level (-dBov) extension.
    pub audio_level: Option<i8>,
    /// Voice activity flag.
    pub voice_activity: Option<bool>,
    /// Payload without header.
    pub payload: Arc<[u8]>,
    /// Arrival time.
    pub arrival: Instant,
    /// Arrival time in microseconds since the router started.
    pub arrival_us: u64,
}

impl InboundRtp {
    fn rewriter_input(&self) -> InboundPacket {
        InboundPacket {
            source: self.ssrc,
            seq: self.seq,
            timestamp: self.timestamp,
            arrival_us: self.arrival_us,
        }
    }
}

/// How a subscriber receives one publisher's video.
#[derive(Debug)]
pub struct VideoSubscription {
    /// SSRC used towards the subscriber (publisher's primary layer SSRC).
    pub out_ssrc: u32,
    /// Codec forwarded.
    pub codec: VideoCodec,
    /// Single-SSRC VP9 SVC instead of simulcast.
    pub svc: bool,
    rewriter: RtpRewriter,
    /// Simulcast layer selection.
    pub selector: LayerSelector,
    /// SVC filtering.
    pub svc_filter: SvcFilter,
}

/// How a subscriber receives one publisher's screen share: forwarded as is, one layer.
#[derive(Debug)]
pub struct ScreenSubscription {
    /// SSRC used towards the subscriber (the publisher's screen SSRC).
    pub out_ssrc: u32,
    /// Codec forwarded.
    pub codec: VideoCodec,
    rewriter: RtpRewriter,
}

/// A subscriber's outbound state for one publisher.
#[derive(Debug)]
pub struct Subscription {
    /// Publisher audio SSRC (also the outbound SSRC).
    pub audio_ssrc: u32,
    audio: RtpRewriter,
    /// Separate soundboard SSRC and sequence rewriter.
    soundboard: Option<(u32, RtpRewriter)>,
    /// Video, when both sides support the publisher's codec.
    pub video: Option<VideoSubscription>,
    /// Screen share, when the publisher sends one and both sides support the codec.
    pub screen: Option<ScreenSubscription>,
}

/// A keyframe the forwarder needs from a publisher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyframeNeed {
    /// Publisher session.
    pub publisher: SessionId,
    /// Inbound SSRC (layer) a keyframe is needed on.
    pub ssrc: u32,
}

/// A session's peer.
#[derive(Debug)]
pub struct Peer {
    /// The session description.
    pub spec: SessionSpec,
    /// The str0m instance.
    pub rtc: Rtc,
    /// Next time str0m wants a timeout input.
    pub next_timeout: Instant,
    /// False once the transport died.
    pub alive: bool,
    /// MEDIA_SINK_WANTS of this subscriber.
    pub sink_wants: SinkWants,
    /// Latest downlink bandwidth estimate.
    pub downlink_bps: u64,
    /// Speaking detection on this publisher's audio.
    pub speaking: SpeakingDetector,
    /// Measured inbound bitrate per own video layer.
    pub layer_meters: Vec<BitrateMeter>,
    /// Outbound state per publisher this peer receives.
    pub subscriptions: HashMap<SessionId, Subscription>,
    /// Throttle for keyframe requests sent to this publisher, per SSRC.
    pub last_keyframe_request: HashMap<u32, Instant>,
}

impl Peer {
    /// Wraps a built rtc.
    pub fn new(spec: SessionSpec, rtc: Rtc, now: Instant, initial_downlink_bps: u64) -> Self {
        let layers = spec.video_layers.len();
        Self {
            spec,
            rtc,
            next_timeout: now,
            alive: true,
            sink_wants: SinkWants::new(),
            downlink_bps: initial_downlink_bps,
            speaking: SpeakingDetector::default(),
            layer_meters: vec![BitrateMeter::new(); layers],
            subscriptions: HashMap::new(),
            last_keyframe_request: HashMap::new(),
        }
    }

    /// Classifies an inbound SSRC of this publisher.
    pub fn track_of(&self, ssrc: u32) -> Option<TrackKind> {
        if ssrc == self.spec.audio_ssrc {
            return Some(TrackKind::Audio);
        }
        if self.spec.soundboard_ssrc == Some(ssrc) {
            return Some(TrackKind::Soundboard);
        }
        if self.spec.is_screen(ssrc) {
            return Some(TrackKind::Screen);
        }
        self.spec.layer_index(ssrc).map(TrackKind::VideoLayer)
    }

    /// Starts receiving `publisher`'s tracks.
    pub fn subscribe(&mut self, publisher_id: &SessionId, publisher: &SessionSpec) {
        let mut api = self.rtc.direct_api();
        let amid = audio_mid(publisher.audio_ssrc);
        api.declare_media(amid, RtcMediaKind::Audio);
        api.declare_stream_tx(publisher.audio_ssrc.into(), None, amid, None);
        if let Some(ssrc) = publisher.soundboard_ssrc {
            let mid = audio_mid(ssrc);
            api.declare_media(mid, RtcMediaKind::Audio);
            api.declare_stream_tx(ssrc.into(), None, mid, None);
        }

        let video = match (publisher.primary_video(), publisher.video_codec()) {
            (Some(primary), Some(codec_spec)) => match codec_spec.codec {
                Codec::Video(codec) if self.spec.payload_type_for(codec_spec.codec).is_some() => {
                    let vmid = video_mid(primary.ssrc);
                    api.declare_media(vmid, RtcMediaKind::Video);
                    api.declare_stream_tx(
                        primary.ssrc.into(),
                        Some(primary.rtx_ssrc.into()),
                        vmid,
                        None,
                    );
                    Some(VideoSubscription {
                        out_ssrc: primary.ssrc,
                        codec,
                        svc: codec == VideoCodec::Vp9 && publisher.video_layers.len() == 1,
                        rewriter: RtpRewriter::new(Codec::Video(codec).clock_rate()),
                        selector: LayerSelector::new(),
                        svc_filter: SvcFilter::new(),
                    })
                }
                _ => None,
            },
            _ => None,
        };

        let screen = match (&publisher.screen, publisher.video_codec()) {
            (Some(stream), Some(codec_spec)) => match codec_spec.codec {
                Codec::Video(codec) if self.spec.payload_type_for(codec_spec.codec).is_some() => {
                    let smid = video_mid(stream.ssrc);
                    api.declare_media(smid, RtcMediaKind::Video);
                    let rtx = (stream.rtx_ssrc != 0).then(|| stream.rtx_ssrc.into());
                    api.declare_stream_tx(stream.ssrc.into(), rtx, smid, None);
                    Some(ScreenSubscription {
                        out_ssrc: stream.ssrc,
                        codec,
                        rewriter: RtpRewriter::new(Codec::Video(codec).clock_rate()),
                    })
                }
                _ => None,
            },
            _ => None,
        };

        self.subscriptions.insert(
            publisher_id.clone(),
            Subscription {
                audio_ssrc: publisher.audio_ssrc,
                audio: RtpRewriter::new(Codec::Opus.clock_rate()),
                soundboard: publisher
                    .soundboard_ssrc
                    .map(|ssrc| (ssrc, RtpRewriter::new(Codec::Opus.clock_rate()))),
                video,
                screen,
            },
        );
    }

    /// Stops receiving `publisher`'s tracks.
    pub fn unsubscribe(&mut self, publisher_id: &SessionId) {
        let Some(sub) = self.subscriptions.remove(publisher_id) else {
            return;
        };
        let mut api = self.rtc.direct_api();
        api.remove_media(audio_mid(sub.audio_ssrc));
        if let Some((ssrc, _)) = sub.soundboard {
            api.remove_media(audio_mid(ssrc));
        }
        if let Some(video) = sub.video {
            api.remove_media(video_mid(video.out_ssrc));
        }
        if let Some(screen) = sub.screen {
            api.remove_media(video_mid(screen.out_ssrc));
        }
    }

    /// Forwards one inbound packet to this subscriber, rewriting sequence numbers and
    /// timestamps. `dtx` marks Opus comfort frames, which are dropped.
    pub fn forward(&mut self, packet: &InboundRtp, dtx: bool) {
        let Some(sub) = self.subscriptions.get_mut(&packet.publisher) else {
            return;
        };
        match packet.track {
            TrackKind::Audio => {
                if dtx || !self.sink_wants.wants(sub.audio_ssrc) {
                    sub.audio.skip(packet.rewriter_input());
                    return;
                }
                let Some(out) = sub.audio.rewrite(packet.rewriter_input()) else {
                    return;
                };
                let Some(pt) = self.spec.payload_type_for(Codec::Opus) else {
                    return;
                };
                let ext = ExtensionValues {
                    audio_level: packet.audio_level,
                    voice_activity: packet.voice_activity,
                    ..ExtensionValues::default()
                };
                let write = RtpWrite::new(
                    pt.into(),
                    out.seq.into(),
                    out.timestamp,
                    packet.arrival,
                    packet.payload.clone(),
                )
                .marker(packet.marker)
                .ext_vals(ext);
                if let Some(stream) = self.rtc.direct_api().stream_tx(&sub.audio_ssrc.into()) {
                    stream.write_rtp(write);
                }
            }
            TrackKind::Soundboard => {
                let Some((ssrc, rewriter)) = sub.soundboard.as_mut() else {
                    return;
                };
                if !self.sink_wants.wants(*ssrc) {
                    rewriter.skip(packet.rewriter_input());
                    return;
                }
                let Some(out) = rewriter.rewrite(packet.rewriter_input()) else {
                    return;
                };
                let Some(pt) = self.spec.payload_type_for(Codec::Opus) else {
                    return;
                };
                let write = RtpWrite::new(
                    pt.into(),
                    out.seq.into(),
                    out.timestamp,
                    packet.arrival,
                    packet.payload.clone(),
                )
                .marker(packet.marker);
                if let Some(stream) = self.rtc.direct_api().stream_tx(&(*ssrc).into()) {
                    stream.write_rtp(write);
                }
            }
            TrackKind::VideoLayer(layer) => {
                let Some(video) = sub.video.as_mut() else {
                    return;
                };
                if Codec::Video(video.codec) != packet.codec {
                    return;
                }
                let marker = if video.svc {
                    let Some(descriptor) = Vp9Descriptor::parse(&packet.payload) else {
                        return;
                    };
                    match video.svc_filter.on_packet(&descriptor, packet.marker) {
                        SvcDecision::Forward { marker } => marker,
                        SvcDecision::Drop => {
                            video.rewriter.skip(packet.rewriter_input());
                            return;
                        }
                    }
                } else {
                    let keyframe =
                        crate::domain::keyframe::starts_keyframe(video.codec, &packet.payload);
                    match video.selector.on_packet(layer, keyframe) {
                        LayerDecision::Forward { .. } => packet.marker,
                        LayerDecision::Drop => return,
                    }
                };
                let Some(out) = video.rewriter.rewrite(packet.rewriter_input()) else {
                    return;
                };
                let Some(pt) = self.spec.payload_type_for(packet.codec) else {
                    return;
                };
                let write = RtpWrite::new(
                    pt.into(),
                    out.seq.into(),
                    out.timestamp,
                    packet.arrival,
                    packet.payload.clone(),
                )
                .marker(marker)
                .nackable(true);
                if let Some(stream) = self.rtc.direct_api().stream_tx(&video.out_ssrc.into()) {
                    stream.write_rtp(write);
                }
            }
            TrackKind::Screen => {
                let Some(screen) = sub.screen.as_mut() else {
                    return;
                };
                if Codec::Video(screen.codec) != packet.codec {
                    return;
                }
                if !self.sink_wants.watches(screen.out_ssrc) {
                    screen.rewriter.skip(packet.rewriter_input());
                    return;
                }
                let Some(out) = screen.rewriter.rewrite(packet.rewriter_input()) else {
                    return;
                };
                let Some(pt) = self.spec.payload_type_for(packet.codec) else {
                    return;
                };
                let write = RtpWrite::new(
                    pt.into(),
                    out.seq.into(),
                    out.timestamp,
                    packet.arrival,
                    packet.payload.clone(),
                )
                .marker(packet.marker)
                .nackable(true);
                if let Some(stream) = self.rtc.direct_api().stream_tx(&screen.out_ssrc.into()) {
                    stream.write_rtp(write);
                }
            }
        }
    }

    /// Restarts every simulcast video so it waits for a fresh keyframe. Used when the transport
    /// becomes connected: whatever was forwarded before never reached the subscriber.
    pub fn restart_video(&mut self) {
        for video in self
            .subscriptions
            .values_mut()
            .filter_map(|s| s.video.as_mut())
            .filter(|v| !v.svc)
        {
            video.selector.restart();
        }
    }

    /// Applies a layer target for one publisher's video. Returns whether a keyframe is needed.
    pub fn set_video_target(
        &mut self,
        publisher: &SessionId,
        layer: Option<usize>,
        quality: u8,
    ) -> bool {
        let Some(video) = self
            .subscriptions
            .get_mut(publisher)
            .and_then(|s| s.video.as_mut())
        else {
            return false;
        };
        if video.svc {
            video
                .svc_filter
                .set_target(layer.map(|_| svc_target_for_quality(quality)))
        } else {
            video.selector.set_target(layer)
        }
    }
}

/// Maps a requested quality to VP9 SVC layers (three spatial, three temporal layers).
fn svc_target_for_quality(quality: u8) -> SvcTarget {
    let spatial = match quality {
        67..=u8::MAX => 2,
        34..=66 => 1,
        _ => 0,
    };
    SvcTarget {
        spatial,
        temporal: 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_maps_to_svc_layers() {
        assert_eq!(svc_target_for_quality(100).spatial, 2);
        assert_eq!(svc_target_for_quality(50).spatial, 1);
        assert_eq!(svc_target_for_quality(10).spatial, 0);
        assert_eq!(svc_target_for_quality(10).temporal, 2);
    }
}
