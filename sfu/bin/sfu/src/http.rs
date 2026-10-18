//! Minimal operational HTTP endpoints: `/healthz`, `/readyz` and Prometheus `/metrics`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use sfu_core::application::events::SfuEvent;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

/// Gauges mirrored from engine `stats` events.
#[derive(Debug, Default)]
pub struct Metrics {
    ready: AtomicBool,
    sessions: AtomicU64,
    rooms: AtomicU64,
    ingress_bps: AtomicU64,
    egress_bps: AtomicU64,
}

impl Metrics {
    /// Marks the engine and control server as accepting traffic.
    pub fn set_ready(&self) {
        self.ready.store(true, Ordering::Relaxed);
    }

    /// Follows engine statistics until the engine stops.
    pub async fn follow(self: Arc<Self>, mut events: broadcast::Receiver<SfuEvent>) {
        loop {
            match events.recv().await {
                Ok(SfuEvent::Stats(stats)) => {
                    self.sessions
                        .store(stats.sessions as u64, Ordering::Relaxed);
                    self.rooms.store(stats.rooms as u64, Ordering::Relaxed);
                    self.ingress_bps.store(stats.ingress_bps, Ordering::Relaxed);
                    self.egress_bps.store(stats.egress_bps, Ordering::Relaxed);
                }
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }

    /// Prometheus text exposition.
    pub fn render(&self) -> String {
        let gauge = |name: &str, help: &str, value: u64| {
            format!("# HELP {name} {help}\n# TYPE {name} gauge\n{name} {value}\n")
        };
        [
            gauge(
                "sfu_sessions",
                "Live media sessions.",
                self.sessions.load(Ordering::Relaxed),
            ),
            gauge(
                "sfu_rooms",
                "Non-empty media rooms.",
                self.rooms.load(Ordering::Relaxed),
            ),
            gauge(
                "sfu_ingress_bps",
                "Inbound media bitrate.",
                self.ingress_bps.load(Ordering::Relaxed),
            ),
            gauge(
                "sfu_egress_bps",
                "Outbound media bitrate.",
                self.egress_bps.load(Ordering::Relaxed),
            ),
        ]
        .concat()
    }
}

/// Serves the endpoints until the listener fails.
pub async fn serve(listener: TcpListener, metrics: Arc<Metrics>) -> std::io::Result<()> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let metrics = Arc::clone(&metrics);
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let Ok(read) = stream.read(&mut buf).await else {
                return;
            };
            let request = String::from_utf8_lossy(&buf[..read]);
            let path = request.split_whitespace().nth(1).unwrap_or("");
            let (status, content_type, body) = route(path, &metrics);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
    }
}

fn route(path: &str, metrics: &Metrics) -> (&'static str, &'static str, String) {
    match path {
        "/healthz" => ("200 OK", "text/plain", "ok".to_owned()),
        "/readyz" if metrics.ready.load(Ordering::Relaxed) => {
            ("200 OK", "text/plain", "ready".to_owned())
        }
        "/readyz" => (
            "503 Service Unavailable",
            "text/plain",
            "starting".to_owned(),
        ),
        "/metrics" => ("200 OK", "text/plain; version=0.0.4", metrics.render()),
        _ => ("404 Not Found", "text/plain", "not found".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_follows_the_flag() {
        let metrics = Metrics::default();
        assert_eq!(route("/readyz", &metrics).0, "503 Service Unavailable");
        metrics.set_ready();
        assert_eq!(route("/readyz", &metrics).0, "200 OK");
        assert_eq!(route("/healthz", &metrics).0, "200 OK");
        assert_eq!(route("/nope", &metrics).0, "404 Not Found");
    }

    #[test]
    fn metrics_are_prometheus_text() {
        let metrics = Metrics::default();
        metrics.sessions.store(3, Ordering::Relaxed);
        assert!(metrics.render().contains("sfu_sessions 3\n"));
    }
}
