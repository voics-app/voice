//! The str0m media router: owns every peer, demultiplexes the UDP mux and forwards media.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use str0m::bwe::{Bitrate, BweKind};
use str0m::media::{KeyframeRequestKind, Mid};
use str0m::net::{DatagramRecv, Protocol, Receive};
use str0m::{Event, IceConnectionState, Input, Output};
use tracing::{debug, info, warn};

use super::peer::{InboundRtp, KeyframeNeed, Peer, TrackKind};
use super::peer_factory::{INITIAL_BWE_KBPS, PeerFactory, ssrc_of_video_mid};
use crate::application::ports::{LocalDescription, MediaRouter, RouterError};
use crate::domain::layer_allocation::{LayerOffer, TrackDemand, allocate_layers};
use crate::domain::opus;
use crate::domain::session::{SessionId, SessionSpec, UserId};
use crate::domain::sink_wants::SinkWants;
use crate::domain::stream_viewers::StreamViewers;

/// Share of the downlink estimate given to video (the rest is headroom for audio/RTCP).
const VIDEO_BUDGET_PERCENT: u64 = 85;
/// Audio bitrate reserved per remote speaker.
const AUDIO_RESERVE_BPS: u64 = 48_000;
/// Minimum interval between keyframe requests on one inbound layer.
const KEYFRAME_THROTTLE: Duration = Duration::from_millis(300);
/// Layer reallocation cadence (layer bitrates move continuously).
const REALLOCATION_INTERVAL: Duration = Duration::from_secs(1);
/// Housekeeping cadence (speaking hang-over, reallocation).
const HOUSEKEEPING_INTERVAL: Duration = Duration::from_millis(100);

/// Something the engine must tell the application about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouterNotification {
    /// Speaking state changed.
    Speaking {
        /// Publisher.
        session_id: SessionId,
        /// Its audio SSRC.
        ssrc: u32,
        /// New state.
        speaking: bool,
    },
    /// The users watching a publisher's screen share changed.
    StreamViewers {
        /// Publisher.
        session_id: SessionId,
        /// Its screen SSRC.
        ssrc: u32,
        /// Every user now watching, sorted.
        viewers: Vec<UserId>,
    },
    /// The transport of a session died.
    TransportClosed(SessionId),
}

/// The SFU media router.
#[derive(Debug)]
pub struct Str0mRouter {
    factory: PeerFactory,
    start: Instant,
    peers: HashMap<SessionId, Peer>,
    demux: super::demux::Demux,
    video_publishers: HashMap<u32, SessionId>,
    viewers: StreamViewers,
    notifications: Vec<RouterNotification>,
    last_reallocation: Instant,
    last_housekeeping: Instant,
}

impl Str0mRouter {
    /// Creates a router whose peers advertise `factory`'s public address.
    pub fn new(factory: PeerFactory, now: Instant) -> Self {
        Self {
            factory,
            start: now,
            peers: HashMap::new(),
            demux: super::demux::Demux::new(),
            video_publishers: HashMap::new(),
            viewers: StreamViewers::new(),
            notifications: Vec::new(),
            last_reallocation: now,
            last_housekeeping: now,
        }
    }

    /// Feeds one datagram received on the mux socket.
    pub fn handle_datagram(&mut self, now: Instant, source: SocketAddr, data: &[u8]) {
        let Some((id, by_ufrag)) = self.demux.route(source, data) else {
            debug!(%source, "datagram for no session");
            return;
        };
        let Some(peer) = self.peers.get_mut(&id).filter(|p| p.alive) else {
            return;
        };
        let Ok(contents) = DatagramRecv::try_from(data) else {
            return;
        };
        let input = Input::Receive(
            now,
            Receive {
                proto: Protocol::Udp,
                source,
                destination: self.factory.public_addr(),
                contents,
            },
        );
        if !peer.rtc.accepts(&input) {
            return;
        }
        if let Err(err) = peer.rtc.handle_input(input) {
            warn!(session = %id, %err, "peer input failed");
            peer.alive = false;
            return;
        }
        if by_ufrag {
            self.demux.learn(source, &id);
        }
    }

    /// Drives timers of all peers and periodic housekeeping.
    pub fn handle_timeout(&mut self, now: Instant) {
        for (id, peer) in self.peers.iter_mut().filter(|(_, p)| p.alive) {
            if peer.next_timeout <= now
                && let Err(err) = peer.rtc.handle_input(Input::Timeout(now))
            {
                warn!(session = %id, %err, "peer timeout failed");
                peer.alive = false;
            }
        }
        if now.duration_since(self.last_housekeeping) >= HOUSEKEEPING_INTERVAL {
            self.last_housekeeping = now;
            self.tick_speaking(now);
        }
        if now.duration_since(self.last_reallocation) >= REALLOCATION_INTERVAL {
            self.last_reallocation = now;
            let ids: Vec<SessionId> = self.peers.keys().cloned().collect();
            for id in ids {
                self.reallocate(&id, now);
            }
        }
    }

    /// Polls every peer until no peer has output left, transmitting datagrams via `transmit`.
    pub fn poll(&mut self, now: Instant, transmit: &mut dyn FnMut(SocketAddr, &[u8])) {
        loop {
            let mut progressed = false;
            let mut inbound: Vec<InboundRtp> = Vec::new();
            let mut keyframe_requests: Vec<(SessionId, Mid, KeyframeRequestKind)> = Vec::new();
            let mut new_estimates: Vec<SessionId> = Vec::new();
            let mut connected: Vec<SessionId> = Vec::new();
            let mut active_publishers = HashSet::new();

            for (id, peer) in self.peers.iter_mut().filter(|(_, p)| p.alive) {
                loop {
                    match peer.rtc.poll_output() {
                        Ok(Output::Timeout(t)) => {
                            peer.next_timeout = t;
                            break;
                        }
                        Ok(Output::Transmit(t)) => {
                            progressed = true;
                            transmit(t.destination, &t.contents);
                        }
                        Ok(Output::Event(event)) => {
                            progressed = true;
                            match event {
                                Event::Connected => {
                                    info!(session = %id, "peer connected");
                                    peer.restart_video();
                                    connected.push(id.clone());
                                }
                                Event::IceConnectionStateChange(
                                    IceConnectionState::Disconnected,
                                )
                                | Event::Closed => {
                                    peer.alive = false;
                                    break;
                                }
                                Event::RtpPacket(packet) => {
                                    if let Some(rtp) = classify(id, peer, packet, self.start, now) {
                                        if record_inbound(
                                            id,
                                            peer,
                                            &rtp,
                                            &mut self.notifications,
                                            self.start,
                                        ) {
                                            active_publishers.insert(id.clone());
                                        }
                                        inbound.push(rtp);
                                    }
                                }
                                Event::KeyframeRequest(req) => {
                                    keyframe_requests.push((id.clone(), req.mid, req.kind));
                                }
                                Event::EgressBitrateEstimate(
                                    BweKind::Twcc { estimate, .. } | BweKind::Remb { estimate, .. },
                                ) => {
                                    peer.downlink_bps = estimate.as_u64();
                                    new_estimates.push(id.clone());
                                }
                                _ => {}
                            }
                        }
                        Err(err) => {
                            warn!(session = %id, %err, "peer poll failed");
                            peer.alive = false;
                            break;
                        }
                    }
                }
            }

            // A new layer must be allocated when its first packet arrives, not at the next
            // periodic bitrate/reallocation tick. This also handles camera restart after idle.
            if !active_publishers.is_empty() {
                let subscribers: Vec<SessionId> = self
                    .peers
                    .iter()
                    .filter(|(_, peer)| {
                        active_publishers
                            .iter()
                            .any(|publisher| peer.subscriptions.contains_key(publisher))
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                for subscriber in subscribers {
                    self.reallocate(&subscriber, now);
                }
            }
            for packet in &inbound {
                self.fan_out(packet);
            }
            for (subscriber, mid, kind) in keyframe_requests {
                self.relay_keyframe_request(&subscriber, mid, kind, now);
            }
            for id in connected {
                self.request_join_keyframes(&id, now);
            }
            for id in new_estimates {
                self.reallocate(&id, now);
            }
            self.collect_dead();

            if !progressed {
                break;
            }
        }
    }

    /// Earliest time the router needs [`Self::handle_timeout`].
    pub fn next_timeout(&self, now: Instant) -> Instant {
        let housekeeping = now + HOUSEKEEPING_INTERVAL;
        self.peers
            .values()
            .filter(|p| p.alive)
            .map(|p| p.next_timeout)
            .min()
            .map_or(housekeeping, |t| t.min(housekeeping))
    }

    /// Drains pending notifications.
    pub fn take_notifications(&mut self) -> Vec<RouterNotification> {
        std::mem::take(&mut self.notifications)
    }

    fn collect_dead(&mut self) {
        for (id, peer) in &self.peers {
            if !peer.alive {
                let note = RouterNotification::TransportClosed(id.clone());
                if !self.notifications.contains(&note) {
                    self.notifications.push(note);
                }
            }
        }
    }

    fn tick_speaking(&mut self, now: Instant) {
        let now_us = micros(self.start, now);
        for (id, peer) in &mut self.peers {
            if let Some(speaking) = peer.speaking.tick(now_us) {
                self.notifications.push(RouterNotification::Speaking {
                    session_id: id.clone(),
                    ssrc: peer.spec.audio_ssrc,
                    speaking,
                });
            }
        }
    }

    fn fan_out(&mut self, packet: &InboundRtp) {
        let dtx = packet.track == TrackKind::Audio && opus::is_dtx_frame(&packet.payload);
        for (id, peer) in self.peers.iter_mut() {
            if *id != packet.publisher && peer.alive {
                peer.forward(packet, dtx);
            }
        }
    }

    fn relay_keyframe_request(
        &mut self,
        subscriber: &SessionId,
        mid: Mid,
        kind: KeyframeRequestKind,
        now: Instant,
    ) {
        let Some(ssrc) = ssrc_of_video_mid(&mid) else {
            return;
        };
        let Some(publisher) = self.video_publishers.get(&ssrc).cloned() else {
            return;
        };
        let is_screen = self
            .peers
            .get(&publisher)
            .is_some_and(|p| p.spec.is_screen(ssrc));
        let layer_ssrc = if is_screen {
            ssrc
        } else {
            let Some(layer_ssrc) = self.forwarded_layer_ssrc(subscriber, &publisher) else {
                return;
            };
            layer_ssrc
        };
        self.request_keyframe(
            KeyframeNeed {
                publisher,
                ssrc: layer_ssrc,
            },
            kind,
            now,
        );
    }

    /// Asks for a keyframe of every video `subscriber` receives. Keyframes requested while it
    /// was still connecting were forwarded into a transport that could not send them yet, so
    /// this request bypasses the throttle.
    fn request_join_keyframes(&mut self, subscriber: &SessionId, now: Instant) {
        let Some(peer) = self.peers.get(subscriber) else {
            return;
        };
        let needs: Vec<KeyframeNeed> = peer
            .subscriptions
            .iter()
            .flat_map(|(publisher, sub)| {
                let camera = sub
                    .video
                    .as_ref()
                    .and_then(|_| self.forwarded_layer_ssrc(subscriber, publisher));
                let screen = sub
                    .screen
                    .as_ref()
                    .filter(|s| peer.sink_wants.watches(s.out_ssrc))
                    .map(|s| s.out_ssrc);
                [camera, screen]
                    .into_iter()
                    .flatten()
                    .map(|ssrc| KeyframeNeed {
                        publisher: publisher.clone(),
                        ssrc,
                    })
            })
            .collect();
        for need in needs {
            if let Some(publisher) = self.peers.get_mut(&need.publisher) {
                publisher.last_keyframe_request.remove(&need.ssrc);
            }
            self.request_keyframe(need, KeyframeRequestKind::Pli, now);
        }
    }

    /// Inbound SSRC currently forwarded from `publisher` to `subscriber`.
    fn forwarded_layer_ssrc(&self, subscriber: &SessionId, publisher: &SessionId) -> Option<u32> {
        let video = self
            .peers
            .get(subscriber)?
            .subscriptions
            .get(publisher)?
            .video
            .as_ref()?;
        let layers = &self.peers.get(publisher)?.spec.video_layers;
        if video.svc {
            return layers.first().map(|l| l.ssrc);
        }
        let index = video.selector.current().or(video.selector.target())?;
        layers.get(index).map(|l| l.ssrc)
    }

    fn request_keyframe(&mut self, need: KeyframeNeed, kind: KeyframeRequestKind, now: Instant) {
        let Some(publisher) = self.peers.get_mut(&need.publisher) else {
            return;
        };
        if let Some(last) = publisher.last_keyframe_request.get(&need.ssrc)
            && now.duration_since(*last) < KEYFRAME_THROTTLE
        {
            return;
        }
        if let Some(stream) = publisher.rtc.direct_api().stream_rx(&need.ssrc.into()) {
            stream.request_keyframe(kind);
            publisher.last_keyframe_request.insert(need.ssrc, now);
        }
    }

    /// Recomputes the layers `subscriber` receives from its downlink estimate and wishes.
    fn reallocate(&mut self, subscriber: &SessionId, now: Instant) {
        let now_us = micros(self.start, now);
        let Some(peer) = self.peers.get(subscriber) else {
            return;
        };
        let video_publishers: Vec<SessionId> = peer
            .subscriptions
            .iter()
            .filter(|(_, s)| s.video.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        let speakers = peer.subscriptions.len() as u64;
        let budget = (peer.downlink_bps * VIDEO_BUDGET_PERCENT / 100)
            .saturating_sub(speakers * AUDIO_RESERVE_BPS);
        let sink_wants = peer.sink_wants.clone();

        let mut demands = Vec::with_capacity(video_publishers.len());
        for publisher_id in &video_publishers {
            let wanted = self.peers.get(publisher_id).map_or(0, |p| {
                p.spec
                    .primary_video()
                    .map_or(0, |v| sink_wants.quality_for(v.ssrc))
            });
            let layers = self.layer_offers(publisher_id, now_us);
            demands.push(TrackDemand {
                wanted_quality: wanted,
                layers,
            });
        }
        let allocation = allocate_layers(budget, &demands);
        let desired: u64 = demands
            .iter()
            .filter(|d| d.wanted_quality > 0)
            .filter_map(|d| d.layers.iter().map(|l| l.bitrate_bps).max())
            .sum();

        let mut needs = Vec::new();
        if let Some(peer) = self.peers.get_mut(subscriber) {
            for ((publisher_id, layer), demand) in
                video_publishers.iter().zip(allocation).zip(&demands)
            {
                if peer.set_video_target(publisher_id, layer, demand.wanted_quality) {
                    needs.push((publisher_id.clone(), layer));
                }
            }
            let desired = desired.max(INITIAL_BWE_KBPS * 1_000);
            peer.rtc.bwe().set_desired_bitrate(Bitrate::bps(desired));
        }
        for (publisher_id, layer) in needs {
            let ssrc = self.peers.get(&publisher_id).and_then(|p| {
                let layers = &p.spec.video_layers;
                layers.get(layer.unwrap_or(0)).map(|l| l.ssrc)
            });
            if let Some(ssrc) = ssrc {
                self.request_keyframe(
                    KeyframeNeed {
                        publisher: publisher_id,
                        ssrc,
                    },
                    KeyframeRequestKind::Pli,
                    now,
                );
            }
        }
    }

    /// Reports the users watching `publisher`'s screen share when the set changed.
    fn refresh_viewers(&mut self, publisher: &SessionId) {
        let Some(ssrc) = self
            .peers
            .get(publisher)
            .and_then(|p| p.spec.screen.as_ref())
            .map(|s| s.ssrc)
        else {
            return;
        };
        let watching = self
            .peers
            .iter()
            .filter(|(id, peer)| {
                *id != publisher
                    && peer.alive
                    && peer.sink_wants.watches(ssrc)
                    && peer
                        .subscriptions
                        .get(publisher)
                        .is_some_and(|s| s.screen.is_some())
            })
            .map(|(_, peer)| peer.spec.user.clone())
            .collect();
        if let Some(viewers) = self.viewers.update(publisher, watching) {
            self.notifications.push(RouterNotification::StreamViewers {
                session_id: publisher.clone(),
                ssrc,
                viewers,
            });
        }
    }

    /// Screen SSRCs of the publishers `subscriber` receives that it just started watching.
    fn newly_watched_screens(
        &self,
        subscriber: &SessionId,
        previous: &SinkWants,
    ) -> Vec<(SessionId, u32)> {
        let Some(peer) = self.peers.get(subscriber) else {
            return Vec::new();
        };
        peer.subscriptions
            .iter()
            .filter_map(|(publisher, sub)| {
                let screen = sub.screen.as_ref()?;
                (peer.sink_wants.watches(screen.out_ssrc) && !previous.watches(screen.out_ssrc))
                    .then(|| (publisher.clone(), screen.out_ssrc))
            })
            .collect()
    }

    fn layer_offers(&mut self, publisher: &SessionId, now_us: u64) -> Vec<LayerOffer> {
        let Some(peer) = self.peers.get_mut(publisher) else {
            return Vec::new();
        };
        let qualities: Vec<u8> = peer.spec.video_layers.iter().map(|l| l.quality).collect();
        qualities
            .into_iter()
            .zip(peer.layer_meters.iter_mut())
            .map(|(quality, meter)| LayerOffer {
                quality,
                bitrate_bps: meter.estimate_bps(now_us),
            })
            .collect()
    }
}

impl MediaRouter for Str0mRouter {
    fn open_session(
        &mut self,
        spec: SessionSpec,
        peers: &[SessionId],
    ) -> Result<LocalDescription, RouterError> {
        if self.peers.contains_key(&spec.id) {
            return Err(RouterError::DuplicateSession(spec.id));
        }
        let now = Instant::now();
        let built = self
            .factory
            .build(&spec, now)
            .map_err(|e| RouterError::Rejected(e.to_string()))?;
        let id = spec.id.clone();
        self.demux.register(&built.local_ice.ufrag, id.clone());
        if let Some(primary) = spec.primary_video() {
            self.video_publishers.insert(primary.ssrc, id.clone());
        }
        if let Some(screen) = &spec.screen {
            self.video_publishers.insert(screen.ssrc, id.clone());
        }

        let mut peer = Peer::new(spec, built.rtc, now, INITIAL_BWE_KBPS * 1_000);
        for other_id in peers {
            if let Some(other) = self.peers.get_mut(other_id) {
                peer.subscribe(other_id, &other.spec);
                other.subscribe(&id, &peer.spec);
            }
        }
        self.peers.insert(id.clone(), peer);
        for other_id in peers {
            self.reallocate(other_id, now);
        }
        self.reallocate(&id, now);
        // A screen share is not layer-allocated, so the joiner needs its own keyframe for each.
        let screens: Vec<KeyframeNeed> = peers
            .iter()
            .filter_map(|other_id| {
                let screen = self.peers.get(other_id)?.spec.screen.as_ref()?;
                Some(KeyframeNeed {
                    publisher: other_id.clone(),
                    ssrc: screen.ssrc,
                })
            })
            .collect();
        for need in screens {
            self.request_keyframe(need, KeyframeRequestKind::Pli, now);
        }
        info!(session = %id, peers = peers.len(), "session opened");

        Ok(LocalDescription {
            ice: built.local_ice,
            fingerprint: built.fingerprint,
        })
    }

    fn close_session(&mut self, id: &SessionId, remaining_peers: &[SessionId]) {
        if let Some(mut peer) = self.peers.remove(id) {
            peer.rtc.disconnect();
            if let Some(primary) = peer.spec.primary_video() {
                self.video_publishers.remove(&primary.ssrc);
            }
            if let Some(screen) = &peer.spec.screen {
                self.video_publishers.remove(&screen.ssrc);
            }
        }
        self.demux.remove(id);
        self.viewers.remove(id);
        self.notifications
            .retain(|n| n != &RouterNotification::TransportClosed(id.clone()));
        for other in remaining_peers {
            if let Some(peer) = self.peers.get_mut(other) {
                peer.unsubscribe(id);
            }
            self.refresh_viewers(other);
        }
        info!(session = %id, "session closed");
    }

    fn set_sink_wants(&mut self, id: &SessionId, wants: SinkWants) -> Result<(), RouterError> {
        let peer = self
            .peers
            .get_mut(id)
            .ok_or_else(|| RouterError::UnknownSession(id.clone()))?;
        let previous = std::mem::replace(&mut peer.sink_wants, wants);
        let now = Instant::now();
        self.reallocate(id, now);
        for (publisher, ssrc) in self.newly_watched_screens(id, &previous) {
            self.request_keyframe(
                KeyframeNeed { publisher, ssrc },
                KeyframeRequestKind::Pli,
                now,
            );
        }
        let publishers: Vec<SessionId> = self
            .peers
            .get(id)
            .map(|p| p.subscriptions.keys().cloned().collect())
            .unwrap_or_default();
        for publisher in publishers {
            self.refresh_viewers(&publisher);
        }
        Ok(())
    }
}

/// Turns a str0m packet into an [`InboundRtp`] if it belongs to a known track of `peer`.
fn classify(
    id: &SessionId,
    peer: &Peer,
    packet: str0m::rtp::RtpPacket,
    start: Instant,
    now: Instant,
) -> Option<InboundRtp> {
    let ssrc = *packet.header.ssrc;
    let track = peer.track_of(ssrc)?;
    let codec = peer
        .spec
        .codec_for_payload_type(*packet.header.payload_type)?;
    let arrival = packet.timestamp.min(now);
    Some(InboundRtp {
        publisher: id.clone(),
        track,
        codec,
        ssrc,
        seq: *packet.seq_no,
        timestamp: packet.header.timestamp,
        marker: packet.header.marker,
        audio_level: packet.header.ext_vals.audio_level,
        voice_activity: packet.header.ext_vals.voice_activity,
        payload: packet.payload,
        arrival,
        arrival_us: micros(start, arrival),
    })
}

/// Publisher-side bookkeeping. Returns whether a video layer has just become active.
fn record_inbound(
    id: &SessionId,
    peer: &mut Peer,
    rtp: &InboundRtp,
    notifications: &mut Vec<RouterNotification>,
    start: Instant,
) -> bool {
    match rtp.track {
        TrackKind::Audio => {
            let change = peer.speaking.on_packet(
                rtp.audio_level,
                rtp.voice_activity,
                micros(start, rtp.arrival),
            );
            if let Some(speaking) = change {
                notifications.push(RouterNotification::Speaking {
                    session_id: id.clone(),
                    ssrc: rtp.ssrc,
                    speaking,
                });
            }
        }
        TrackKind::Soundboard => {}
        TrackKind::VideoLayer(index) => {
            if let Some(meter) = peer.layer_meters.get_mut(index) {
                let was_inactive = meter.estimate_bps(rtp.arrival_us) == 0;
                meter.record(rtp.payload.len(), rtp.arrival_us);
                return was_inactive && !rtp.payload.is_empty();
            }
        }
        TrackKind::Screen => {}
    }
    false
}

fn micros(start: Instant, at: Instant) -> u64 {
    u64::try_from(at.saturating_duration_since(start).as_micros()).unwrap_or(u64::MAX)
}
