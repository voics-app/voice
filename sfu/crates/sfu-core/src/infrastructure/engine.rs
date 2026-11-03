//! The media engine task: one UDP mux socket, one router, commands in, events out.
//!
//! All media state lives in a single task, so there are no locks on the hot path. Commands
//! from the control server arrive over an mpsc channel with a oneshot reply; events leave on a
//! broadcast channel.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use super::peer_factory::PeerFactory;
use super::router::{RouterNotification, Str0mRouter};
use crate::application::error::AppError;
use crate::application::events::{NodeStats, SfuEvent};
use crate::application::ports::EventPublisher;
use crate::application::service::{Command, Outcome, SfuService};
use crate::domain::bitrate_meter::BitrateMeter;

const MAX_DATAGRAM: usize = 2_048;
const EVENT_BUFFER: usize = 4_096;
const COMMAND_BUFFER: usize = 256;

/// Engine configuration.
#[derive(Debug, Clone, Copy)]
pub struct EngineConfig {
    /// Address the UDP mux socket binds to.
    pub udp_bind: SocketAddr,
    /// Public IP clients send media to; the advertised port is the bound port.
    pub public_ip: IpAddr,
    /// Interval of `stats` events.
    pub stats_interval: Duration,
}

/// Errors of [`EngineHandle::execute`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    /// The command failed.
    #[error(transparent)]
    App(#[from] AppError),
    /// The engine task is gone.
    #[error("media engine stopped")]
    Stopped,
}

#[derive(Debug)]
struct EngineRequest {
    command: Command,
    reply: oneshot::Sender<Result<Outcome, AppError>>,
}

/// Cloneable handle to a running engine.
#[derive(Debug, Clone)]
pub struct EngineHandle {
    requests: mpsc::Sender<EngineRequest>,
    events: broadcast::Sender<SfuEvent>,
    local_addr: SocketAddr,
}

impl EngineHandle {
    /// Executes a command on the engine.
    pub async fn execute(&self, command: Command) -> Result<Outcome, EngineError> {
        let (reply, response) = oneshot::channel();
        self.requests
            .send(EngineRequest { command, reply })
            .await
            .map_err(|_| EngineError::Stopped)?;
        Ok(response.await.map_err(|_| EngineError::Stopped)??)
    }

    /// Subscribes to engine events.
    pub fn subscribe(&self) -> broadcast::Receiver<SfuEvent> {
        self.events.subscribe()
    }

    /// Local address of the UDP mux socket.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

/// Publishes application events on the broadcast channel.
#[derive(Debug, Clone)]
struct BroadcastPublisher(broadcast::Sender<SfuEvent>);

impl EventPublisher for BroadcastPublisher {
    fn publish(&self, event: SfuEvent) {
        // No subscriber (signaling not connected) is fine: sync replays state on connect.
        let _ = self.0.send(event);
    }
}

/// Binds the socket, generates the DTLS certificate and spawns the engine task.
pub async fn spawn(config: EngineConfig) -> io::Result<(EngineHandle, JoinHandle<()>)> {
    let socket = UdpSocket::bind(config.udp_bind).await?;
    let local_addr = socket.local_addr()?;
    let public_addr = SocketAddr::new(config.public_ip, local_addr.port());
    let cert = str0m::crypto::from_feature_flags()
        .dtls_provider
        .generate_certificate()
        .ok_or_else(|| io::Error::other("DTLS certificate generation failed"))?;

    let (events, _) = broadcast::channel(EVENT_BUFFER);
    let (requests, commands) = mpsc::channel(COMMAND_BUFFER);
    let now = Instant::now();
    let router = Str0mRouter::new(PeerFactory::new(cert, public_addr), now);
    let service = SfuService::new(router, BroadcastPublisher(events.clone()));
    info!(%local_addr, %public_addr, "media engine listening");

    let task = tokio::spawn(run(socket, service, commands, config.stats_interval));
    Ok((
        EngineHandle {
            requests,
            events,
            local_addr,
        },
        task,
    ))
}

struct Traffic {
    ingress: BitrateMeter,
    egress: BitrateMeter,
    start: Instant,
}

impl Traffic {
    fn micros(&self, now: Instant) -> u64 {
        u64::try_from(now.duration_since(self.start).as_micros()).unwrap_or(u64::MAX)
    }
}

async fn run(
    socket: UdpSocket,
    mut service: SfuService<Str0mRouter, BroadcastPublisher>,
    mut commands: mpsc::Receiver<EngineRequest>,
    stats_interval: Duration,
) {
    let mut buf = vec![0u8; MAX_DATAGRAM];
    let mut traffic = Traffic {
        ingress: BitrateMeter::new(),
        egress: BitrateMeter::new(),
        start: Instant::now(),
    };
    let mut stats_tick = tokio::time::interval(stats_interval);

    loop {
        let deadline = service.router_mut().next_timeout(Instant::now());
        tokio::select! {
            received = socket.recv_from(&mut buf) => match received {
                Ok((len, source)) => {
                    let now = Instant::now();
                    traffic.ingress.record(len, traffic.micros(now));
                    service.router_mut().handle_datagram(now, source, &buf[..len]);
                }
                // Windows reports ICMP port-unreachable as a receive error; keep serving.
                Err(err) => debug!(%err, "udp receive error"),
            },
            request = commands.recv() => {
                let Some(request) = request else {
                    info!("control handle dropped, stopping media engine");
                    return;
                };
                let outcome = service.handle(request.command);
                let _ = request.reply.send(outcome);
            }
            _ = tokio::time::sleep_until(deadline.into()) => {}
            _ = stats_tick.tick() => publish_stats(&mut service, &mut traffic),
        }

        let now = Instant::now();
        service.router_mut().handle_timeout(now);
        let egress = &mut traffic.egress;
        let start = traffic.start;
        service.router_mut().poll(now, &mut |destination, bytes| {
            let at = u64::try_from(now.duration_since(start).as_micros()).unwrap_or(u64::MAX);
            egress.record(bytes.len(), at);
            if let Err(err) = socket.try_send_to(bytes, destination) {
                debug!(%err, %destination, "udp send dropped");
            }
        });
        dispatch_notifications(&mut service);
    }
}

fn dispatch_notifications(service: &mut SfuService<Str0mRouter, BroadcastPublisher>) {
    for notification in service.router_mut().take_notifications() {
        match notification {
            RouterNotification::Speaking {
                session_id,
                ssrc,
                speaking,
            } => service.events().publish(SfuEvent::Speaking {
                session_id,
                ssrc,
                speaking,
            }),
            RouterNotification::StreamViewers {
                session_id,
                ssrc,
                viewers,
            } => service.events().publish(SfuEvent::StreamViewers {
                session_id,
                ssrc,
                viewers,
            }),
            RouterNotification::TransportClosed(id) => {
                warn!(session = %id, "transport closed");
                service.on_transport_closed(&id);
            }
        }
    }
}

fn publish_stats(service: &mut SfuService<Str0mRouter, BroadcastPublisher>, traffic: &mut Traffic) {
    let now_us = traffic.micros(Instant::now());
    let (sessions, rooms) = service.occupancy();
    service.events().publish(SfuEvent::Stats(NodeStats {
        sessions,
        rooms,
        ingress_bps: traffic.ingress.bps(now_us),
        egress_bps: traffic.egress.bps(now_us),
    }));
}
