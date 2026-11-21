use probe_application::{Clock, Config, etcd_endpoints};
use probe_infrastructure::{RtcProbe, SystemClock, etcd::Etcd, media};
use probe_interface::{
    daemon::Runtime,
    metrics::{Metrics, router},
};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let vars = std::env::vars().collect::<HashMap<_, _>>();
    match args.as_slice() {
        [command] if command == "run" => run(Config::from_env(&vars)?).await,
        [command, node] if command == "check" => check(Config::from_env(&vars)?, node).await,
        [command, scope, id, reason, operator] if command == "blacklist" => {
            let store = store(&vars)?;
            store
                .blacklist(scope, id, reason, operator, SystemClock.now_ms())
                .await
                .map_err(|e| format!("blacklist failed: {e:?}"))
        }
        [command, scope, id] if command == "clear-blacklist" => {
            let store = store(&vars)?;
            store
                .clear_blacklist(scope, id)
                .await
                .map_err(|e| format!("blacklist removal failed: {e:?}"))
        }
        [] => {
            usage();
            Ok(())
        }
        _ => {
            usage();
            Err("invalid command".into())
        }
    }
}

fn usage() {
    println!(
        "voice-prober run\nvoice-prober check <node_id>\nvoice-prober blacklist <nodes|regions> <id> <reason> <operator>\nvoice-prober clear-blacklist <nodes|regions> <id>"
    );
}

fn store(vars: &HashMap<String, String>) -> Result<Etcd, String> {
    Etcd::new(etcd_endpoints(vars)?).map_err(|e| format!("invalid etcd configuration: {e:?}"))
}

async fn check(config: Config, node_id: &str) -> Result<(), String> {
    let store =
        Etcd::new(config.endpoints.clone()).map_err(|e| format!("etcd configuration: {e:?}"))?;
    let registration = store
        .registrations()
        .await
        .map_err(|e| format!("discovery failed: {e:?}"))?
        .into_iter()
        .find(|r| r.node.node_id == node_id)
        .ok_or("node is not uniquely registered")?;
    let result = media::probe(&registration, &config).await;
    println!(
        "{}",
        json!({"node_id": node_id, "loss_pct": result.loss_pct, "rtt_ms": result.rtt_ms,
        "failure": result.failure.map(|failure| failure.label())})
    );
    if result.failure.is_some() {
        Err("media probe failed".into())
    } else {
        Ok(())
    }
}

async fn run(config: Config) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(config.http_bind)
        .await
        .map_err(|_| "cannot bind probe metrics listener")?;
    println!(
        "voice prober metrics listening on {}",
        listener
            .local_addr()
            .map_err(|_| "metrics listener unavailable")?
    );
    let metrics = Arc::new(Metrics::default());
    let runtime = Arc::new(Runtime {
        store: Arc::new(
            Etcd::new(config.endpoints.clone())
                .map_err(|e| format!("etcd configuration: {e:?}"))?,
        ),
        config: Arc::new(config),
        media: Arc::new(RtcProbe),
        clock: Arc::new(SystemClock),
        sink: metrics.clone(),
    });
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let mut worker = tokio::spawn(runtime.run(receiver));
    let result = tokio::select! {
        result = axum::serve(listener, router(metrics)) => result.map_err(|_| "probe metrics server failed".to_owned()),
        result = shutdown_signal() => result,
        _ = &mut worker => return Err("probe daemon stopped unexpectedly".into()),
    };
    let _ = shutdown.send(true);
    worker
        .await
        .map_err(|_| "probe daemon stopped unexpectedly")?;
    result
}

async fn shutdown_signal() -> Result<(), String> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .map_err(|_| "signal listener failed")?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.map_err(|_| "signal listener failed".into()),
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .map_err(|_| "signal listener failed".into())
}
