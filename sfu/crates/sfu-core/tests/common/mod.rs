//! An in-process WebRTC client built on str0m, speaking the Discordy voice conventions:
//! direct API (no SDP), mids `a<ssrc>`/`v<ssrc>`, extension ids 1/2/3, DTLS active, ICE
//! controlling.

#![allow(dead_code, missing_debug_implementations)]

use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::time::Instant;

use sfu_core::application::ports::LocalDescription;
use sfu_core::domain::codec::{Codec, VideoCodec};
use sfu_core::domain::session::{CodecSpec, IceCredentials, SessionSpec, VideoLayer};
use str0m::format::{Codec as RtcCodec, FormatParams};
use str0m::ice::IceCreds;
use str0m::media::{Frequency, MediaKind, Mid};
use str0m::net::{Protocol, Receive};
use str0m::rtp::{Extension, ExtensionValues, RtpWrite};
use str0m::{Candidate, Event, Input, Output, Rtc, RtcConfig};

pub const OPUS_PT: u8 = 111;
pub const VP8_PT: u8 = 96;
pub const VP8_RTX_PT: u8 = 97;

/// A received RTP packet.
#[derive(Debug, Clone)]
pub struct Received {
    pub ssrc: u32,
    pub seq: u16,
    pub timestamp: u32,
    pub payload: Vec<u8>,
}

pub struct Client {
    pub rtc: Rtc,
    socket: UdpSocket,
    local: SocketAddr,
    pub audio_ssrc: u32,
    pub video_layers: Vec<VideoLayer>,
    pub screen: Option<VideoLayer>,
    pub received: Vec<Received>,
    pub connected: bool,
    /// Keyframe requests (PLI/FIR) received for our own video, not yet answered.
    pub keyframe_requests: usize,
    seq: u64,
    video_seq: Vec<u64>,
}

fn mid(prefix: char, ssrc: u32) -> Mid {
    Mid::from(format!("{prefix}{ssrc}").as_str())
}

impl Client {
    pub fn new(audio_ssrc: u32, video_layers: Vec<VideoLayer>) -> Self {
        let mut config = RtcConfig::new()
            .set_rtp_mode(true)
            .clear_codecs()
            .clear_extension_map()
            .set_extension(1, Extension::AudioLevel)
            .set_extension(2, Extension::AbsoluteSendTime)
            .set_extension(3, Extension::TransportSequenceNumber);
        config.codec_config().add_config(
            OPUS_PT.into(),
            None,
            RtcCodec::Opus,
            Frequency::FORTY_EIGHT_KHZ,
            Some(2),
            FormatParams::default(),
        );
        config.codec_config().add_config(
            VP8_PT.into(),
            Some(VP8_RTX_PT.into()),
            RtcCodec::Vp8,
            Frequency::NINETY_KHZ,
            None,
            FormatParams::default(),
        );
        let mut rtc = config.build(Instant::now());
        let socket = UdpSocket::bind("127.0.0.1:0").expect("bind client socket");
        socket.set_nonblocking(true).expect("nonblocking");
        let local = socket.local_addr().expect("local addr");
        rtc.add_local_candidate(Candidate::host(local, "udp").expect("candidate"));
        let layers = video_layers.len();
        Self {
            rtc,
            socket,
            local,
            audio_ssrc,
            video_layers,
            screen: None,
            received: Vec::new(),
            connected: false,
            keyframe_requests: 0,
            seq: 1,
            video_seq: vec![1_000; layers],
        }
    }

    /// The join payload signaling would forward to the SFU.
    pub fn session_spec(&mut self, session: &str, room: &str) -> SessionSpec {
        let api = self.rtc.direct_api();
        let creds = api.local_ice_credentials();
        let fingerprint = api.local_dtls_fingerprint().to_string();
        SessionSpec {
            id: session.into(),
            room: room.into(),
            user: format!("user-{session}").as_str().into(),
            remote_ice: IceCredentials {
                ufrag: creds.ufrag,
                pwd: creds.pass,
            },
            remote_fingerprint: fingerprint,
            local_ice: None,
            codecs: vec![
                CodecSpec {
                    codec: Codec::Opus,
                    payload_type: OPUS_PT,
                    rtx_payload_type: None,
                    priority: 1000,
                },
                CodecSpec {
                    codec: Codec::Video(VideoCodec::Vp8),
                    payload_type: VP8_PT,
                    rtx_payload_type: Some(VP8_RTX_PT),
                    priority: 1000,
                },
            ],
            audio_ssrc: self.audio_ssrc,
            video_layers: self.video_layers.clone(),
            screen: self.screen.clone(),
        }
    }

    /// Applies SESSION_DESCRIPTION and declares own and remote tracks.
    pub fn connect(
        &mut self,
        sfu: &LocalDescription,
        sfu_addr: SocketAddr,
        remote_audio: &[u32],
        remote_video: &[(u32, u32)],
    ) {
        self.rtc
            .add_remote_candidate(Candidate::host(sfu_addr, "udp").expect("sfu candidate"));
        let mut api = self.rtc.direct_api();
        api.set_remote_ice_credentials(IceCreds {
            ufrag: sfu.ice.ufrag.clone(),
            pass: sfu.ice.pwd.clone(),
        });
        api.set_remote_fingerprint(sfu.fingerprint.parse().expect("fingerprint"));
        api.set_ice_controlling(true);
        api.start_dtls(true).expect("start dtls");

        let own = mid('a', self.audio_ssrc);
        api.declare_media(own, MediaKind::Audio);
        api.declare_stream_tx(self.audio_ssrc.into(), None, own, None);
        if let Some(primary) = self.video_layers.first() {
            let vmid = mid('v', primary.ssrc);
            api.declare_media(vmid, MediaKind::Video);
            for layer in &self.video_layers {
                api.declare_stream_tx(
                    layer.ssrc.into(),
                    Some(layer.rtx_ssrc.into()),
                    vmid,
                    Some(layer.rid.as_str().into()),
                );
            }
        }
        if let Some(screen) = &self.screen {
            let smid = mid('v', screen.ssrc);
            api.declare_media(smid, MediaKind::Video);
            api.declare_stream_tx(screen.ssrc.into(), Some(screen.rtx_ssrc.into()), smid, None);
        }
        for &ssrc in remote_audio {
            let m = mid('a', ssrc);
            api.declare_media(m, MediaKind::Audio);
            api.expect_stream_rx(ssrc.into(), None, m, None);
        }
        for &(ssrc, rtx) in remote_video {
            let m = mid('v', ssrc);
            api.declare_media(m, MediaKind::Video);
            api.expect_stream_rx(ssrc.into(), Some(rtx.into()), m, None);
        }
    }

    /// Sends one Opus packet. Returns the payload sent.
    pub fn send_audio(&mut self, payload: &[u8]) {
        let now = Instant::now();
        let seq = self.seq;
        self.seq += 1;
        let ext = ExtensionValues {
            audio_level: Some(-20),
            voice_activity: Some(true),
            ..ExtensionValues::default()
        };
        if let Some(stream) = self.rtc.direct_api().stream_tx(&self.audio_ssrc.into()) {
            stream.write_rtp(
                RtpWrite::new(
                    OPUS_PT.into(),
                    seq.into(),
                    (seq * 960) as u32,
                    now,
                    payload.to_vec(),
                )
                .ext_vals(ext),
            );
        }
    }

    /// Sends one VP8 packet on a layer.
    pub fn send_video(&mut self, layer: usize, payload: &[u8], marker: bool) {
        let now = Instant::now();
        let seq = self.video_seq[layer];
        self.video_seq[layer] += 1;
        let ssrc = self.video_layers[layer].ssrc;
        if let Some(stream) = self.rtc.direct_api().stream_tx(&ssrc.into()) {
            stream.write_rtp(
                RtpWrite::new(
                    VP8_PT.into(),
                    seq.into(),
                    (seq * 3_000) as u32 + layer as u32 * 1_000_000,
                    now,
                    payload.to_vec(),
                )
                .marker(marker)
                .nackable(true),
            );
        }
    }

    /// Sends one VP8 packet on the screen share.
    pub fn send_screen(&mut self, seq: u64, payload: &[u8], marker: bool) {
        let now = Instant::now();
        let Some(ssrc) = self.screen.as_ref().map(|s| s.ssrc) else {
            return;
        };
        if let Some(stream) = self.rtc.direct_api().stream_tx(&ssrc.into()) {
            stream.write_rtp(
                RtpWrite::new(
                    VP8_PT.into(),
                    seq.into(),
                    (seq * 3_000) as u32,
                    now,
                    payload.to_vec(),
                )
                .marker(marker)
                .nackable(true),
            );
        }
    }

    /// Moves str0m forward: drains output, reads the socket.
    pub fn pump(&mut self) {
        loop {
            match self.rtc.poll_output().expect("poll") {
                Output::Timeout(_) => break,
                Output::Transmit(t) => {
                    let _ = self.socket.send_to(&t.contents, t.destination);
                }
                Output::Event(Event::Connected) => self.connected = true,
                Output::Event(Event::KeyframeRequest(_)) => self.keyframe_requests += 1,
                Output::Event(Event::RtpPacket(p)) => self.received.push(Received {
                    ssrc: *p.header.ssrc,
                    seq: p.header.sequence_number,
                    timestamp: p.header.timestamp,
                    payload: p.payload.to_vec(),
                }),
                Output::Event(_) => {}
            }
        }
        let mut buf = [0u8; 2048];
        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((n, source)) => {
                    let Ok(contents) = buf[..n].try_into() else {
                        continue;
                    };
                    let input = Input::Receive(
                        Instant::now(),
                        Receive {
                            proto: Protocol::Udp,
                            source,
                            destination: self.local,
                            contents,
                        },
                    );
                    self.rtc.handle_input(input).expect("handle input");
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
        self.rtc
            .handle_input(Input::Timeout(Instant::now()))
            .expect("timeout");
    }

    pub fn received_from(&self, ssrc: u32) -> Vec<&Received> {
        self.received.iter().filter(|r| r.ssrc == ssrc).collect()
    }
}
