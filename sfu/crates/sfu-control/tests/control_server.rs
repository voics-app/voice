//! The control server speaks NDJSON over a real TCP connection to a real engine.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use serde_json::{Value, json};
use sfu_core::infrastructure::engine::{self, EngineConfig};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

struct Control {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
}

impl Control {
    async fn send(&mut self, value: Value) {
        let mut line = value.to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await.unwrap();
    }

    async fn next_with(&mut self, evt: &str) -> Value {
        loop {
            let mut line = String::new();
            let read =
                tokio::time::timeout(Duration::from_secs(5), self.reader.read_line(&mut line))
                    .await
                    .expect("reply in time")
                    .unwrap();
            assert!(read > 0, "connection closed");
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["evt"] == evt {
                return value;
            }
        }
    }
}

async fn start() -> Control {
    let config = EngineConfig {
        udp_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        stats_interval: Duration::from_millis(200),
    };
    let (handle, _task) = engine::spawn(config).await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(sfu_control::server::serve(listener, handle));
    let stream = TcpStream::connect(addr).await.unwrap();
    let (read, writer) = stream.into_split();
    Control {
        reader: BufReader::new(read),
        writer,
    }
}

fn create(session: &str, audio_ssrc: u32) -> Value {
    json!({
        "req": session, "cmd": "create_session", "session_id": session, "room_id": "room",
        "user_id": format!("user-{session}"), "ice_ufrag": "abcd",
        "ice_pwd": "0123456789012345678901", "remote_fingerprint": "sha-256 AB:CD:EF",
        "audio_ssrc": audio_ssrc,
        "codecs": [{"name": "opus", "type": "audio", "priority": 1000, "payload_type": 111}]
    })
}

#[tokio::test]
async fn create_close_and_stats_over_ndjson() {
    let mut control = start().await;

    control.send(create("alice", 10)).await;
    let created = control.next_with("session_created").await;
    assert_eq!(created["req"], "alice");
    assert_eq!(created["session_id"], "alice");
    assert!(created["local_ufrag"].as_str().unwrap().len() >= 4);
    assert!(
        created["fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("sha-256 ")
    );

    let stats = control.next_with("stats").await;
    assert_eq!(stats["sessions"], 1);

    control
        .send(
            json!({"req": "w", "cmd": "set_sink_wants", "session_id": "alice", "wants": {"10": 0}}),
        )
        .await;
    assert_eq!(control.next_with("sink_wants_applied").await["req"], "w");

    control
        .send(json!({"req": "c", "cmd": "close_session", "session_id": "alice"}))
        .await;
    let closed = control.next_with("closed").await;
    assert_eq!(closed["existed"], true);
    control
        .send(json!({"req": "c2", "cmd": "close_session", "session_id": "alice"}))
        .await;
    assert_eq!(control.next_with("closed").await["existed"], false);
}

#[tokio::test]
async fn sync_reports_sessions_to_replay_and_errors_are_coded() {
    let mut control = start().await;

    control
        .send(json!({"req": "s", "cmd": "sync", "session_ids": ["alice", "bob"]}))
        .await;
    let synced = control.next_with("synced").await;
    assert_eq!(synced["missing"], json!(["alice", "bob"]));

    control.send(create("alice", 10)).await;
    control.next_with("session_created").await;
    control.send(create("alice", 11)).await;
    let duplicate = control.next_with("error").await;
    assert_eq!(duplicate["req"], "alice");
    assert!(duplicate["code"].is_string());

    control.send(json!({"cmd": "teleport"})).await;
    assert_eq!(control.next_with("error").await["code"], "invalid_request");
}
