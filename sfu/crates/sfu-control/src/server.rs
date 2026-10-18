//! The TCP server: accepts signaling connections, executes commands on the engine and streams
//! unsolicited events (speaking, stats, closures) to every connection.

use std::io;

use sfu_core::infrastructure::engine::EngineHandle;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use crate::contracts::{Request, WireEvent};
use crate::mapping::{
    broadcast_for, error_reply, invalid_request_reply, reply_for, session_of, to_command,
};

/// Longest accepted line; a `create_session` is well below this.
const MAX_LINE_BYTES: u64 = 64 * 1024;
const OUTBOUND_BUFFER: usize = 1_024;

/// Serves control connections until the listener fails.
pub async fn serve(listener: TcpListener, engine: EngineHandle) -> io::Result<()> {
    info!(addr = %listener.local_addr()?, "control server listening");
    loop {
        let (stream, peer) = listener.accept().await?;
        info!(%peer, "signaling connected");
        let engine = engine.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, engine).await {
                debug!(%peer, %err, "control connection ended with error");
            }
            info!(%peer, "signaling disconnected");
        });
    }
}

async fn handle_connection(stream: TcpStream, engine: EngineHandle) -> io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let (outbound, mut outgoing) = mpsc::channel::<WireEvent>(OUTBOUND_BUFFER);

    let writer = tokio::spawn(async move {
        while let Some(event) = outgoing.recv().await {
            let Ok(mut line) = serde_json::to_vec(&event) else {
                continue;
            };
            line.push(b'\n');
            if write_half.write_all(&line).await.is_err() {
                break;
            }
        }
    });

    let forwarder = tokio::spawn(forward_events(engine.subscribe(), outbound.clone()));

    let result = read_requests(read_half, &engine, &outbound).await;
    forwarder.abort();
    drop(outbound);
    let _ = writer.await;
    result
}

async fn forward_events(
    mut events: broadcast::Receiver<sfu_core::application::events::SfuEvent>,
    outbound: mpsc::Sender<WireEvent>,
) {
    loop {
        match events.recv().await {
            Ok(event) => {
                if let Some(wire) = broadcast_for(event)
                    && outbound.send(wire).await.is_err()
                {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(skipped, "control connection lagged behind engine events");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn read_requests(
    read_half: tokio::net::tcp::OwnedReadHalf,
    engine: &EngineHandle,
    outbound: &mpsc::Sender<WireEvent>,
) -> io::Result<()> {
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    loop {
        line.clear();
        let read = (&mut reader)
            .take(MAX_LINE_BYTES + 1)
            .read_line(&mut line)
            .await?;
        if read == 0 {
            return Ok(());
        }
        if read as u64 > MAX_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control line too long",
            ));
        }
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        let reply = execute_line(text, engine).await;
        if outbound.send(reply).await.is_err() {
            return Ok(());
        }
    }
}

async fn execute_line(text: &str, engine: &EngineHandle) -> WireEvent {
    let request: Request = match serde_json::from_str(text) {
        Ok(request) => request,
        Err(err) => return invalid_request_reply(None, err.to_string()),
    };
    let req = request.req.clone();
    let command = match to_command(request) {
        Ok(command) => command,
        Err(err) => return invalid_request_reply(req, err.to_string()),
    };
    let session = session_of(&command);
    match engine.execute(command).await {
        Ok(outcome) => reply_for(req, session, outcome),
        Err(sfu_core::infrastructure::engine::EngineError::App(err)) => error_reply(req, &err),
        Err(sfu_core::infrastructure::engine::EngineError::Stopped) => WireEvent::Error {
            req,
            code: "engine_stopped".to_owned(),
            message: "media engine stopped".to_owned(),
        },
    }
}
