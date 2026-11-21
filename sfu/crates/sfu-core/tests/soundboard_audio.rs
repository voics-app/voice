//! Separate soundboard packets must be denied by default and forwarded only when granted.
mod common;
use common::Client;
use sfu_core::application::service::{Command, Outcome};
use sfu_core::infrastructure::engine::{self, EngineConfig};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

async fn check_soundboard(allowed: bool) {
    let (handle, _task) = engine::spawn(EngineConfig {
        udp_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        stats_interval: Duration::from_secs(1),
    })
    .await
    .unwrap();
    let mut alice = Client::new(1111, vec![]);
    alice.soundboard_ssrc = Some(3333);
    let mut bob = Client::new(2222, vec![]);
    let mut spec = alice.session_spec("alice", "room");
    spec.guarded_publish = true;
    let alice_sfu = match handle
        .execute(Command::CreateSession(Box::new(spec)))
        .await
        .unwrap()
    {
        Outcome::Created(local) => local,
        _ => panic!("create alice"),
    };
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 14_000;
    handle
        .execute(Command::SetPublishPermissions {
            session_id: "alice".into(),
            update: sfu_core::domain::permission_lease::PermissionUpdate {
                revision: 1,
                expires_at,
                can_publish_audio: true,
                can_publish_video: false,
                can_publish_soundboard: allowed,
            },
        })
        .await
        .unwrap();
    let bob_sfu = match handle
        .execute(Command::CreateSession(Box::new(
            bob.session_spec("bob", "room"),
        )))
        .await
        .unwrap()
    {
        Outcome::Created(local) => local,
        _ => panic!("create bob"),
    };
    let addr = handle.local_addr();
    alice.connect(&alice_sfu, addr, &[2222], &[]);
    bob.connect(&bob_sfu, addr, &[1111, 3333], &[]);
    let (alice, bob) = tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut next = None;
        let mut sent = 0;
        let mut done_at = None;
        while Instant::now() < deadline {
            alice.pump();
            bob.pump();
            if alice.connected && bob.connected && next.is_none() {
                next = Some(Instant::now() + Duration::from_millis(300));
            }
            if sent < 20 && next.is_some_and(|t| Instant::now() >= t) {
                alice.send_audio(&[0xa0, 1, 2, 3]);
                alice.send_soundboard(&[0xb0, 4, 5, 6]);
                sent += 1;
                next = Some(Instant::now() + Duration::from_millis(20));
                if sent == 20 {
                    done_at = Some(Instant::now() + Duration::from_millis(500));
                }
            }
            if done_at.is_some_and(|t| Instant::now() >= t) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        (alice, bob)
    })
    .await
    .unwrap();
    assert!(alice.connected && bob.connected);
    assert!(
        !bob.received_from(1111).is_empty(),
        "microphone still forwards"
    );
    let sounds = bob.received_from(3333);
    if allowed {
        assert!(!sounds.is_empty(), "authorized soundboard reaches peer");
        assert!(
            sounds
                .iter()
                .all(|p| p.payload.as_slice() == [0xb0, 4, 5, 6])
        );
    } else {
        assert!(
            sounds.is_empty(),
            "unauthorized soundboard must not reach peer"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soundboard_track_is_independently_gated() {
    check_soundboard(false).await;
    check_soundboard(true).await;
}
