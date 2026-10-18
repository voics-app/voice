//! Composition root of the Discordy SFU.

mod config;
mod http;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use sfu_core::infrastructure::engine::{self, EngineConfig};
use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::http::Metrics;

const STATS_INTERVAL: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("invalid configuration: {err}");
            return ExitCode::from(2);
        }
    };
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::new(&config.log_level))
        .init();

    match run(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            error!(%err, "sfu stopped");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: Config) -> std::io::Result<()> {
    let (engine, engine_task) = engine::spawn(EngineConfig {
        udp_bind: config.udp_bind,
        public_ip: config.public_ip,
        stats_interval: STATS_INTERVAL,
    })
    .await?;
    let control = TcpListener::bind(config.control_bind).await?;
    let http = TcpListener::bind(config.http_bind).await?;

    let metrics = Arc::new(Metrics::default());
    tokio::spawn(Arc::clone(&metrics).follow(engine.subscribe()));
    tokio::spawn(http::serve(http, Arc::clone(&metrics)));
    tokio::spawn(sfu_control::server::serve(control, engine));
    metrics.set_ready();
    info!(udp = %config.udp_bind, control = %config.control_bind, "sfu ready");

    tokio::select! {
        _ = shutdown_signal() => info!("shutting down"),
        _ = engine_task => error!("media engine task ended"),
    }
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
            return;
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}
