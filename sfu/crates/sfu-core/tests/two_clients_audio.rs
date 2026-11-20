//! Two in-process str0m clients exchange Opus audio through the SFU engine over real UDP.

mod common;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use common::Client;
use sfu_core::application::events::SfuEvent;
use sfu_core::application::service::{Command, Outcome};
use sfu_core::infrastructure::engine::{self, EngineConfig, EngineHandle};

const ALICE_AUDIO: u32 = 1_111;
const BOB_AUDIO: u32 = 2_222;
const PACKETS: usize = 100;
/// Every 10th packet Alice sends is a 1-byte Opus DTX frame, which the SFU drops.
const DTX_EVERY: usize = 10;
/// Opus frames are 20 ms apart.
const FRAME_INTERVAL: Duration = Duration::from_millis(20);
const WARMUP: Duration = Duration::from_millis(300);

async fn start_engine() -> EngineHandle {
    let config = EngineConfig {
        udp_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        stats_interval: Duration::from_secs(1),
    };
    let (handle, _task) = engine::spawn(config).await.expect("engine starts");
    handle
}

async fn create(
    handle: &EngineHandle,
    spec: sfu_core::domain::session::SessionSpec,
) -> sfu_core::application::ports::LocalDescription {
    match handle
        .execute(Command::CreateSession(Box::new(spec)))
        .await
        .expect("create session")
    {
        Outcome::Created(local) => local,
        other => panic!("unexpected outcome {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_clients_exchange_audio_through_the_sfu() {
    let handle = start_engine().await;
    let mut events = handle.subscribe();
    let sfu_addr = handle.local_addr();

    let mut alice = Client::new(ALICE_AUDIO, vec![]);
    let mut bob = Client::new(BOB_AUDIO, vec![]);
    let alice_sfu = create(&handle, alice.session_spec("alice", "room-1")).await;
    let bob_sfu = create(&handle, bob.session_spec("bob", "room-1")).await;
    alice.connect(&alice_sfu, sfu_addr, &[BOB_AUDIO], &[]);
    bob.connect(&bob_sfu, sfu_addr, &[ALICE_AUDIO], &[]);

    let (alice, bob) = tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut sent = 0usize;
        let mut next_send: Option<Instant> = None;
        while Instant::now() < deadline {
            alice.pump();
            bob.pump();
            if alice.connected && bob.connected && next_send.is_none() {
                // Give the SFU side of both peers time to finish its DTLS handshake too.
                next_send = Some(Instant::now() + WARMUP);
            }
            if sent < PACKETS && next_send.is_some_and(|t| Instant::now() >= t) {
                let alice_payload = if sent % DTX_EVERY == DTX_EVERY - 1 {
                    vec![0xF8]
                } else {
                    vec![0xA0, sent as u8, 0x01, 0x02, 0x03]
                };
                alice.send_audio(&alice_payload);
                bob.send_audio(&[0xB0, sent as u8, 0x04, 0x05, 0x06]);
                sent += 1;
                next_send = Some(Instant::now() + FRAME_INTERVAL);
            }
            let alice_done = alice.received_from(BOB_AUDIO).len() >= PACKETS;
            let bob_done = bob.received_from(ALICE_AUDIO).len() >= PACKETS - PACKETS / DTX_EVERY;
            if sent == PACKETS && alice_done && bob_done {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        (alice, bob)
    })
    .await
    .expect("client thread");

    assert!(alice.connected && bob.connected, "both clients connected");

    // Bob hears Alice under Alice's audio SSRC, without the DTX frames, with no sequence gaps.
    let from_alice = bob.received_from(ALICE_AUDIO);
    assert_eq!(from_alice.len(), PACKETS - PACKETS / DTX_EVERY);
    assert!(
        from_alice
            .iter()
            .all(|p| p.payload[0] == 0xA0 && p.payload.len() == 5)
    );
    let mut seqs: Vec<u16> = from_alice.iter().map(|p| p.seq).collect();
    seqs.sort_unstable();
    assert!(
        seqs.windows(2).all(|w| w[1] == w[0].wrapping_add(1)),
        "rewritten sequence is contiguous: {seqs:?}"
    );

    // Alice hears Bob.
    let from_bob = alice.received_from(BOB_AUDIO);
    assert_eq!(from_bob.len(), PACKETS);
    assert!(from_bob.iter().all(|p| p.payload[0] == 0xB0));
    // Nobody receives their own audio.
    assert!(alice.received_from(ALICE_AUDIO).is_empty());
    assert!(bob.received_from(BOB_AUDIO).is_empty());

    // Speaking detection fired for both publishers from the audio-level extension.
    let mut speakers = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let SfuEvent::Speaking {
            session_id,
            speaking: true,
            ..
        } = event
        {
            speakers.push(session_id.0);
        }
    }
    speakers.sort();
    speakers.dedup();
    assert_eq!(speakers, vec!["alice".to_owned(), "bob".to_owned()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audio_publish_permission_blocks_and_restores_media() {
    let handle = start_engine().await;
    let sfu_addr = handle.local_addr();
    let mut alice = Client::new(ALICE_AUDIO, vec![]);
    let mut bob = Client::new(BOB_AUDIO, vec![]);
    let mut alice_spec = alice.session_spec("alice", "room-stage");
    alice_spec.can_publish_audio = false;
    let alice_sfu = create(&handle, alice_spec).await;
    let bob_sfu = create(&handle, bob.session_spec("bob", "room-stage")).await;
    alice.connect(&alice_sfu, sfu_addr, &[BOB_AUDIO], &[]);
    bob.connect(&bob_sfu, sfu_addr, &[ALICE_AUDIO], &[]);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let (promoted_tx, promoted_rx) = tokio::sync::oneshot::channel();
    let (demoted_tx, demoted_rx) = std::sync::mpsc::channel();
    let clients = tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && !(alice.connected && bob.connected) {
            alice.pump();
            bob.pump();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(alice.connected && bob.connected);
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(900) {
            alice.pump();
            bob.pump();
            alice.send_audio(&[0xA0, 1, 2, 3, 4]);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            bob.received_from(ALICE_AUDIO).is_empty(),
            "suppressed audio leaked"
        );
        ready_tx.send(()).unwrap();
        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            alice.pump();
            bob.pump();
            alice.send_audio(&[0xA0, 1, 2, 3, 4]);
            if !bob.received_from(ALICE_AUDIO).is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !bob.received_from(ALICE_AUDIO).is_empty(),
            "promoted speaker remained muted"
        );
        promoted_tx.send(()).unwrap();
        demoted_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(900) {
            alice.pump();
            bob.pump();
            alice.send_audio(&[0xC0, 9, 8, 7, 6]);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            bob.received_from(ALICE_AUDIO)
                .iter()
                .all(|packet| packet.payload[0] != 0xC0),
            "revoked audio leaked"
        );
    });
    ready_rx.await.unwrap();
    assert_eq!(
        handle
            .execute(Command::SetPublishAudio {
                session_id: "alice".into(),
                allowed: true
            })
            .await
            .unwrap(),
        Outcome::PublishAudioApplied
    );
    resume_tx.send(()).unwrap();
    promoted_rx.await.unwrap();
    assert_eq!(
        handle
            .execute(Command::SetPublishAudio {
                session_id: "alice".into(),
                allowed: false
            })
            .await
            .unwrap(),
        Outcome::PublishAudioApplied
    );
    demoted_tx.send(()).unwrap();
    clients.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guarded_audio_grant_expires_on_real_udp() {
    let handle = start_engine().await;
    let sfu_addr = handle.local_addr();
    let mut alice = Client::new(ALICE_AUDIO, vec![]);
    let mut bob = Client::new(BOB_AUDIO, vec![]);
    let mut alice_spec = alice.session_spec("alice", "room-stage");
    alice_spec.can_publish_audio = false;
    alice_spec.guarded_publish = true;
    let alice_sfu = create(&handle, alice_spec).await;
    let bob_sfu = create(&handle, bob.session_spec("bob", "room-stage")).await;
    alice.connect(&alice_sfu, sfu_addr, &[BOB_AUDIO], &[]);
    bob.connect(&bob_sfu, sfu_addr, &[ALICE_AUDIO], &[]);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let (promoted_tx, promoted_rx) = tokio::sync::oneshot::channel();
    let (demoted_tx, demoted_rx) = std::sync::mpsc::channel();
    let clients = tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && !(alice.connected && bob.connected) {
            alice.pump();
            bob.pump();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(alice.connected && bob.connected);
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(900) {
            alice.pump();
            bob.pump();
            alice.send_audio(&[0xA0, 1, 2, 3, 4]);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            bob.received_from(ALICE_AUDIO).is_empty(),
            "suppressed audio leaked"
        );
        ready_tx.send(()).unwrap();
        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            alice.pump();
            bob.pump();
            alice.send_audio(&[0xA0, 1, 2, 3, 4]);
            if !bob.received_from(ALICE_AUDIO).is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !bob.received_from(ALICE_AUDIO).is_empty(),
            "promoted speaker remained muted"
        );
        promoted_tx.send(()).unwrap();
        demoted_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(900) {
            alice.pump();
            bob.pump();
            alice.send_audio(&[0xC0, 9, 8, 7, 6]);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            bob.received_from(ALICE_AUDIO)
                .iter()
                .all(|packet| packet.payload[0] != 0xC0),
            "revoked audio leaked"
        );
    });
    ready_rx.await.unwrap();
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 1500;
    let update = sfu_core::domain::permission_lease::PermissionUpdate {
        revision: 1,
        expires_at,
        can_publish_audio: true,
        can_publish_video: false,
        can_publish_soundboard: false,
    };
    assert_eq!(
        handle
            .execute(Command::SetPublishPermissions {
                session_id: "alice".into(),
                update: update.clone()
            })
            .await
            .unwrap(),
        Outcome::PublishPermissionsApplied
    );
    resume_tx.send(()).unwrap();
    promoted_rx.await.unwrap();
    tokio::time::sleep(Duration::from_millis(1600)).await;
    assert!(
        handle
            .execute(Command::SetPublishPermissions {
                session_id: "alice".into(),
                update
            })
            .await
            .is_err(),
        "expired revision must not replay"
    );
    demoted_tx.send(()).unwrap();
    clients.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guarded_audio_atomic_revoke_on_real_udp() {
    let handle = start_engine().await;
    let sfu_addr = handle.local_addr();
    let mut alice = Client::new(ALICE_AUDIO, vec![]);
    let mut bob = Client::new(BOB_AUDIO, vec![]);
    let mut alice_spec = alice.session_spec("alice", "room-stage");
    alice_spec.can_publish_audio = false;
    alice_spec.guarded_publish = true;
    let alice_sfu = create(&handle, alice_spec).await;
    let bob_sfu = create(&handle, bob.session_spec("bob", "room-stage")).await;
    alice.connect(&alice_sfu, sfu_addr, &[BOB_AUDIO], &[]);
    bob.connect(&bob_sfu, sfu_addr, &[ALICE_AUDIO], &[]);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let (promoted_tx, promoted_rx) = tokio::sync::oneshot::channel();
    let (demoted_tx, demoted_rx) = std::sync::mpsc::channel();
    let clients = tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && !(alice.connected && bob.connected) {
            alice.pump();
            bob.pump();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(alice.connected && bob.connected);
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(900) {
            alice.pump();
            bob.pump();
            alice.send_audio(&[0xA0, 1, 2, 3, 4]);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            bob.received_from(ALICE_AUDIO).is_empty(),
            "suppressed audio leaked"
        );
        ready_tx.send(()).unwrap();
        resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            alice.pump();
            bob.pump();
            alice.send_audio(&[0xA0, 1, 2, 3, 4]);
            if !bob.received_from(ALICE_AUDIO).is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !bob.received_from(ALICE_AUDIO).is_empty(),
            "promoted speaker remained muted"
        );
        promoted_tx.send(()).unwrap();
        demoted_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(900) {
            alice.pump();
            bob.pump();
            alice.send_audio(&[0xC0, 9, 8, 7, 6]);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            bob.received_from(ALICE_AUDIO)
                .iter()
                .all(|packet| packet.payload[0] != 0xC0),
            "revoked audio leaked"
        );
    });
    ready_rx.await.unwrap();
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        + 10000;
    let update = sfu_core::domain::permission_lease::PermissionUpdate {
        revision: 1,
        expires_at,
        can_publish_audio: true,
        can_publish_video: false,
        can_publish_soundboard: false,
    };
    assert_eq!(
        handle
            .execute(Command::SetPublishPermissions {
                session_id: "alice".into(),
                update: update.clone()
            })
            .await
            .unwrap(),
        Outcome::PublishPermissionsApplied
    );
    resume_tx.send(()).unwrap();
    promoted_rx.await.unwrap();
    let revoked = sfu_core::domain::permission_lease::PermissionUpdate {
        revision: 2,
        can_publish_audio: false,
        can_publish_video: false,
        can_publish_soundboard: false,
        ..update.clone()
    };
    assert_eq!(
        handle
            .execute(Command::SetPublishPermissions {
                session_id: "alice".into(),
                update: revoked
            })
            .await
            .unwrap(),
        Outcome::PublishPermissionsApplied
    );
    assert!(
        handle
            .execute(Command::SetPublishPermissions {
                session_id: "alice".into(),
                update
            })
            .await
            .is_err(),
        "stale revision must not replay"
    );
    demoted_tx.send(()).unwrap();
    clients.await.unwrap();
}
