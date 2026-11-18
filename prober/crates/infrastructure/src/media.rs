//! Public WebSocket signaling and paired ICE/DTLS/RTP round trips.
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{EncodingKey, Header};
use serde_json::{Value, json};
use str0m::format::{Codec, FormatParams};
use str0m::ice::IceCreds;
use str0m::media::{Frequency, MediaKind, Mid};
use str0m::net::{Protocol, Receive};
use str0m::rtp::{Extension, ExtensionValues, RtpWrite};
use str0m::{Candidate, Event, Input, Output, Rtc, RtcConfig};
use tokio::net::{TcpStream, UdpSocket};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

use crate::SystemClock;
use probe_application::{Clock, Config};
use probe_domain::{Failure, Observation, Registration};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
const PACKETS: u16 = 50;
const OPUS: &[u8] = &[0xf8, 0xff, 0xfe];
const TOKEN_TTL_S: u64 = 60;
const TOKEN_BACKDATE_S: u64 = 10;

pub async fn probe(registration: &Registration, config: &Config) -> Observation {
    match tokio::time::timeout(config.timeout, run_pair(registration, config)).await {
        Ok(Ok(observation)) => observation,
        Ok(Err(failure)) => Observation::failed(failure),
        Err(_) => Observation::failed(Failure::Timeout),
    }
}

async fn run_pair(registration: &Registration, config: &Config) -> Result<Observation, Failure> {
    let probe_id = Uuid::new_v4().to_string();
    let (mut socket_a, ssrc_a, remote_a) = identify(registration, config, &probe_id, 0).await?;
    let (mut socket_b, ssrc_b, remote_b) = identify(registration, config, &probe_id, 1).await?;
    if ssrc_a == ssrc_b {
        return Err(Failure::Signaling);
    }
    let mut a = Media::new(ssrc_a, remote_a)?;
    let mut b = Media::new(ssrc_b, remote_b)?;
    negotiate(&mut socket_a, &mut a, ssrc_b).await?;
    negotiate(&mut socket_b, &mut b, ssrc_a).await?;
    let observation = exchange(&mut a, &mut b, &mut socket_a, &mut socket_b, config).await?;
    // Close both synthetic rooms promptly; cancellation also drops both underlying sockets.
    let _ = socket_a.close(None).await;
    let _ = socket_b.close(None).await;
    Ok(observation)
}

async fn identify(
    registration: &Registration,
    config: &Config,
    probe_id: &str,
    peer: usize,
) -> Result<(Socket, u32, SocketAddr), Failure> {
    let mut url = reqwest::Url::parse(&registration.node.ws_url).map_err(|_| Failure::Signaling)?;
    if url.path().is_empty() || url.path() == "/" {
        url.set_path("/voice");
    }
    let socket_config = WebSocketConfig::default()
        .max_message_size(Some(65_536))
        .max_frame_size(Some(65_536));
    let (mut socket, _) =
        tokio_tungstenite::connect_async_with_config(url.as_str(), Some(socket_config), false)
            .await
            .map_err(|_| Failure::Signaling)?;
    expect_op(&mut socket, 8).await?;
    let session = Uuid::new_v4().to_string();
    let now = SystemClock.now_ms() / 1000;
    // Independent observers can run slightly ahead of a node's wall clock. Backdating
    // shortens usable lifetime while retaining the verifier's strict 60-second maximum.
    let (issued_at, expires_at) = token_window(now);
    let key =
        EncodingKey::from_secret(config.probe_secret.as_bytes()).map_err(|_| Failure::Signaling)?;
    let token = jsonwebtoken::encode(
        &Header::default(),
        &json!({
            "sub": config.user_ids[peer], "sid": session, "srv": config.server_id,
            "chn": config.channel_id, "node": registration.node.node_id, "e2ee": false,
            "iat": issued_at, "exp": expires_at, "purpose": "probe", "node_lease": registration.lease,
            "probe_id": probe_id
        }),
        &key,
    )
    .map_err(|_| Failure::Signaling)?;
    send_op(
        &mut socket,
        0,
        json!({"server_id": config.server_id, "user_id": config.user_ids[peer],
        "session_id": session, "token": token, "video": false}),
    )
    .await?;
    let ready = expect_op(&mut socket, 2).await?;
    let ip: IpAddr = ready["ip"]
        .as_str()
        .ok_or(Failure::Signaling)?
        .parse()
        .map_err(|_| Failure::Signaling)?;
    let expected_ip: IpAddr = registration
        .node
        .public_ip
        .parse()
        .map_err(|_| Failure::Signaling)?;
    let port = ready["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .ok_or(Failure::Signaling)?;
    let ssrc = ready["ssrc"]
        .as_u64()
        .and_then(|ssrc| u32::try_from(ssrc).ok())
        .filter(|ssrc| *ssrc > 0)
        .ok_or(Failure::Signaling)?;
    if ip != expected_ip || port != registration.node.udp_port {
        return Err(Failure::Signaling);
    }
    Ok((socket, ssrc, SocketAddr::new(ip, port)))
}

async fn negotiate(
    socket: &mut Socket,
    media: &mut Media,
    remote_ssrc: u32,
) -> Result<(), Failure> {
    let direct = media.rtc.direct_api();
    let ice = direct.local_ice_credentials();
    let fingerprint = direct.local_dtls_fingerprint().to_string();
    send_op(socket, 1, json!({"protocol": "webrtc", "data": {"ice_ufrag": ice.ufrag,
        "ice_pwd": ice.pass, "fingerprint": fingerprint, "setup": "actpass", "ssrcs": {"audio": media.ssrc}},
        "codecs": [{"name": "opus", "type": "audio", "priority": 1000, "payload_type": 111}]})).await?;
    let answer = expect_op(socket, 4).await?;
    media.connect(&answer, remote_ssrc)?;
    media.drain().await
}

async fn expect_op(socket: &mut Socket, op: u64) -> Result<Value, Failure> {
    while let Some(message) = socket.next().await {
        match message.map_err(|_| Failure::Signaling)? {
            Message::Text(text) => {
                let frame: Value =
                    serde_json::from_str(text.as_str()).map_err(|_| Failure::Signaling)?;
                if frame["op"].as_u64() == Some(op) {
                    return Ok(frame["d"].clone());
                }
            }
            Message::Close(_) => return Err(Failure::Signaling),
            _ => {}
        }
    }
    Err(Failure::Signaling)
}

async fn send_op(socket: &mut Socket, op: u64, data: Value) -> Result<(), Failure> {
    socket
        .send(Message::Text(
            json!({"op": op, "d": data}).to_string().into(),
        ))
        .await
        .map_err(|_| Failure::Signaling)
}

#[derive(Debug)]
struct Packet {
    ssrc: u32,
    seq: u16,
    timestamp: u32,
    payload: Vec<u8>,
}

struct Media {
    rtc: Rtc,
    socket: UdpSocket,
    local: SocketAddr,
    remote: SocketAddr,
    ssrc: u32,
    connected: bool,
    timeout: Instant,
    received: Vec<Packet>,
}

impl Media {
    fn new(ssrc: u32, remote: SocketAddr) -> Result<Self, Failure> {
        let mut config = RtcConfig::new()
            .set_rtp_mode(true)
            .clear_codecs()
            .clear_extension_map()
            .set_extension(1, Extension::AudioLevel)
            .set_extension(2, Extension::AbsoluteSendTime)
            .set_extension(3, Extension::TransportSequenceNumber);
        config.codec_config().add_config(
            111.into(),
            None,
            Codec::Opus,
            Frequency::FORTY_EIGHT_KHZ,
            Some(2),
            FormatParams::default(),
        );
        let mut rtc = config.build(Instant::now());
        let bind = if remote.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = std::net::UdpSocket::bind(bind).map_err(|_| Failure::Transport)?;
        socket.connect(remote).map_err(|_| Failure::Transport)?;
        socket
            .set_nonblocking(true)
            .map_err(|_| Failure::Transport)?;
        let local = socket.local_addr().map_err(|_| Failure::Transport)?;
        rtc.add_local_candidate(Candidate::host(local, "udp").map_err(|_| Failure::Transport)?);
        Ok(Self {
            rtc,
            socket: UdpSocket::from_std(socket).map_err(|_| Failure::Transport)?,
            local,
            remote,
            ssrc,
            connected: false,
            timeout: Instant::now(),
            received: Vec::new(),
        })
    }

    fn connect(&mut self, answer: &Value, remote_ssrc: u32) -> Result<(), Failure> {
        if answer["audio_codec"] != "opus" || !answer["e2ee"].is_null() {
            return Err(Failure::Signaling);
        }
        let text = |key: &str| {
            answer[key]
                .as_str()
                .map(str::to_owned)
                .ok_or(Failure::Signaling)
        };
        self.rtc.add_remote_candidate(
            Candidate::host(self.remote, "udp").map_err(|_| Failure::Transport)?,
        );
        let mut direct = self.rtc.direct_api();
        direct.set_remote_ice_credentials(IceCreds {
            ufrag: text("ice_ufrag")?,
            pass: text("ice_pwd")?,
        });
        direct.set_remote_fingerprint(
            text("fingerprint")?
                .parse()
                .map_err(|_| Failure::Signaling)?,
        );
        direct.set_ice_controlling(true);
        direct.start_dtls(true).map_err(|_| Failure::Transport)?;
        let own = Mid::from(format!("a{}", self.ssrc).as_str());
        direct.declare_media(own, MediaKind::Audio);
        direct.declare_stream_tx(self.ssrc.into(), None, own, None);
        let remote = Mid::from(format!("a{remote_ssrc}").as_str());
        direct.declare_media(remote, MediaKind::Audio);
        direct.expect_stream_rx(remote_ssrc.into(), None, remote, None);
        Ok(())
    }

    async fn receive(&mut self, data: &[u8], source: SocketAddr) -> Result<(), Failure> {
        let Ok(contents) = data.try_into() else {
            return Ok(());
        };
        self.rtc
            .handle_input(Input::Receive(
                Instant::now(),
                Receive {
                    proto: Protocol::Udp,
                    source,
                    destination: self.local,
                    contents,
                },
            ))
            .map_err(|_| Failure::Transport)?;
        self.drain().await
    }

    async fn tick(&mut self) -> Result<(), Failure> {
        self.rtc
            .handle_input(Input::Timeout(Instant::now()))
            .map_err(|_| Failure::Transport)?;
        self.drain().await
    }

    async fn send(&mut self, seq: u16, timestamp: u32, payload: Vec<u8>) -> Result<(), Failure> {
        let mut direct = self.rtc.direct_api();
        let stream = direct
            .stream_tx(&self.ssrc.into())
            .ok_or(Failure::Transport)?;
        stream.write_rtp(
            RtpWrite::new(
                111.into(),
                u64::from(seq).into(),
                timestamp,
                Instant::now(),
                payload,
            )
            .ext_vals(ExtensionValues {
                audio_level: Some(-20),
                voice_activity: Some(true),
                ..Default::default()
            }),
        );
        self.drain().await
    }

    // str0m permits one pending RTP event: drain after every mutation, especially a UDP burst.
    async fn drain(&mut self) -> Result<(), Failure> {
        loop {
            match self.rtc.poll_output().map_err(|_| Failure::Transport)? {
                Output::Timeout(timeout) => {
                    self.timeout = timeout;
                    return Ok(());
                }
                Output::Transmit(packet) => {
                    self.socket
                        .send_to(&packet.contents, packet.destination)
                        .await
                        .map_err(|_| Failure::Transport)?;
                }
                Output::Event(Event::Connected) => self.connected = true,
                Output::Event(Event::RtpPacket(packet)) => self.received.push(Packet {
                    ssrc: *packet.header.ssrc,
                    seq: packet.header.sequence_number,
                    timestamp: packet.header.timestamp,
                    payload: packet.payload.to_vec(),
                }),
                Output::Event(_) => {}
            }
        }
    }
}

async fn exchange(
    a: &mut Media,
    b: &mut Media,
    ws_a: &mut Socket,
    ws_b: &mut Socket,
    config: &Config,
) -> Result<Observation, Failure> {
    let mut sent = BTreeMap::new();
    let mut received = BTreeMap::new();
    let mut next_send = Instant::now();
    let mut buf_a = [0; 2048];
    let mut buf_b = [0; 2048];
    loop {
        for packet in std::mem::take(&mut b.received) {
            if packet.ssrc == a.ssrc
                && packet.payload == OPUS
                && (1..=PACKETS).contains(&packet.seq)
            {
                b.send(packet.seq, packet.timestamp, packet.payload).await?;
            }
        }
        for packet in std::mem::take(&mut a.received) {
            if packet.ssrc == b.ssrc
                && packet.payload == OPUS
                && let Some(start) = sent.get(&packet.seq)
            {
                received
                    .entry(packet.seq)
                    .or_insert(Instant::now().duration_since(*start).as_secs_f64() * 1000.0);
            }
        }
        let now = Instant::now();
        if a.connected && b.connected && sent.len() < PACKETS as usize && now >= next_send {
            let seq = sent.len() as u16 + 1;
            sent.insert(seq, now);
            a.send(seq, u32::from(seq) * 960, OPUS.to_vec()).await?;
            next_send = now + Duration::from_millis(20);
        }
        if sent.len() == PACKETS as usize
            && (received.len() == PACKETS as usize
                || now
                    .duration_since(*sent.last_key_value().expect("sent packet").1)
                    .as_secs_f64()
                    * 1000.0
                    >= config.max_rtt_ms)
        {
            return Ok(summarize(
                received.values().copied().collect(),
                config.max_loss_pct,
                config.max_rtt_ms,
            ));
        }
        let mut wake = a.timeout.min(b.timeout);
        if a.connected && b.connected {
            let media_wake = if sent.len() < PACKETS as usize {
                next_send
            } else {
                *sent.last_key_value().expect("sent packet").1
                    + Duration::from_secs_f64(config.max_rtt_ms / 1000.0)
            };
            wake = wake.min(media_wake);
        }
        tokio::select! {
            packet = a.socket.recv_from(&mut buf_a) => {
                let (size, source) = packet.map_err(|_| Failure::Transport)?;
                a.receive(&buf_a[..size], source).await?;
            }
            packet = b.socket.recv_from(&mut buf_b) => {
                let (size, source) = packet.map_err(|_| Failure::Transport)?;
                b.receive(&buf_b[..size], source).await?;
            }
            message = ws_a.next() => check_open(message)?,
            message = ws_b.next() => check_open(message)?,
            _ = tokio::time::sleep_until(wake.into()) => {
                if Instant::now() >= a.timeout { a.tick().await?; }
                if Instant::now() >= b.timeout { b.tick().await?; }
            }
        }
    }
}

fn check_open(
    message: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
) -> Result<(), Failure> {
    match message {
        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => Err(Failure::Signaling),
        _ => Ok(()),
    }
}

fn summarize(mut rtts: Vec<f64>, max_loss_pct: f64, max_rtt_ms: f64) -> Observation {
    let loss = 100.0 * (f64::from(PACKETS) - rtts.len() as f64) / f64::from(PACKETS);
    rtts.sort_by(f64::total_cmp);
    let rtt = rtts.get(rtts.len() / 2).copied();
    let failure = if rtts.is_empty() || loss > max_loss_pct {
        Some(Failure::RtpLoss)
    } else if rtt.is_some_and(|rtt| rtt > max_rtt_ms) {
        Some(Failure::Rtt)
    } else {
        None
    };
    Observation {
        loss_pct: Some(loss),
        rtt_ms: rtt,
        failure,
    }
}

fn token_window(now: u64) -> (u64, u64) {
    let issued_at = now.saturating_sub(TOKEN_BACKDATE_S);
    (issued_at, issued_at + TOKEN_TTL_S)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loss_and_median_round_trip_measure_the_returned_packet_set() {
        let pass = summarize(vec![3.0; 45], 10.0, 10.0);
        assert_eq!(pass.loss_pct, Some(10.0));
        assert_eq!(pass.rtt_ms, Some(3.0));
        assert_eq!(pass.failure, None);
        assert_eq!(
            summarize(vec![3.0; 44], 10.0, 10.0).failure,
            Some(Failure::RtpLoss)
        );
        assert_eq!(
            summarize(Vec::new(), 100.0, 10.0).failure,
            Some(Failure::RtpLoss)
        );
        assert_eq!(
            summarize(vec![11.0; 50], 10.0, 10.0).failure,
            Some(Failure::Rtt)
        );
    }

    #[test]
    fn backdating_preserves_strict_sixty_second_lifetime_and_rejects_larger_skew() {
        let node_now = 1_000;
        let (iat, exp) = token_window(node_now + 5);
        assert_eq!(exp - iat, 60);
        assert!(iat <= node_now && exp > node_now);
        let (iat, _) = token_window(node_now + 11);
        assert!(
            iat > node_now,
            "the unchanged verifier must reject future-issued tokens"
        );
        let (_, exp) = token_window(node_now - 50);
        assert!(
            exp <= node_now,
            "the unchanged verifier must reject expired tokens"
        );
    }
}
