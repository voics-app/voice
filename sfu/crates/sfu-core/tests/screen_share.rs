//! A publisher's screen share reaches subscribers as its own stream, beside camera and audio.

mod common;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use common::Client;
use sfu_core::application::events::SfuEvent;
use sfu_core::application::service::{Command, Outcome};
use sfu_core::domain::session::VideoLayer;
use sfu_core::infrastructure::engine::{self, EngineConfig, EngineHandle};
use tokio::sync::broadcast::Receiver;

const ALICE_AUDIO: u32 = 3_111;
const ALICE_CAMERA: u32 = 3_222;
const ALICE_CAMERA_RTX: u32 = 3_223;
const ALICE_SCREEN: u32 = 3_333;
const ALICE_SCREEN_RTX: u32 = 3_334;
const BOB_AUDIO: u32 = 4_111;
const FRAMES: usize = 60;
const FRAME_INTERVAL: Duration = Duration::from_millis(15);
const WARMUP: Duration = Duration::from_millis(300);

/// A VP8 keyframe payload descriptor (S bit, partition 0) followed by a keyframe header byte.
const VP8_KEYFRAME: [u8; 4] = [0x10, 0x00, 0x9d, 0x01];

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

/// Runs Alice's screen share towards Bob; Bob opts in to watching it when `watch` is set.
async fn share_screen(
    watch: bool,
    frames_per_send: usize,
) -> (Client, EngineHandle, Receiver<SfuEvent>) {
    let handle = start_engine().await;
    let events = handle.subscribe();
    let sfu_addr = handle.local_addr();

    let camera = VideoLayer {
        rid: "100".to_owned(),
        quality: 100,
        ssrc: ALICE_CAMERA,
        rtx_ssrc: ALICE_CAMERA_RTX,
    };
    let mut alice = Client::new(ALICE_AUDIO, vec![camera]);
    alice.screen = Some(VideoLayer {
        rid: "screen".to_owned(),
        quality: 100,
        ssrc: ALICE_SCREEN,
        rtx_ssrc: ALICE_SCREEN_RTX,
    });
    let mut bob = Client::new(BOB_AUDIO, vec![]);
    let alice_sfu = create(&handle, alice.session_spec("alice", "room-s")).await;
    let bob_sfu = create(&handle, bob.session_spec("bob", "room-s")).await;
    alice.connect(&alice_sfu, sfu_addr, &[BOB_AUDIO], &[]);
    bob.connect(
        &bob_sfu,
        sfu_addr,
        &[ALICE_AUDIO],
        &[
            (ALICE_CAMERA, ALICE_CAMERA_RTX),
            (ALICE_SCREEN, ALICE_SCREEN_RTX),
        ],
    );

    if watch {
        handle
            .execute(Command::SetSinkWants {
                session_id: "bob".into(),
                wants: vec![(ALICE_SCREEN, 100)],
            })
            .await
            .expect("bob opts in to the screen share");
    }

    let bob = tokio::task::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_secs(if watch { 20 } else { 4 });
        let mut sent = 0usize;
        let mut next_send: Option<Instant> = None;
        while Instant::now() < deadline {
            alice.pump();
            bob.pump();
            if alice.connected && bob.connected && next_send.is_none() {
                next_send = Some(Instant::now() + WARMUP);
            }
            if sent < FRAMES && next_send.is_some_and(|t| Instant::now() >= t) {
                for _ in 0..frames_per_send.min(FRAMES - sent) {
                    let mut payload = VP8_KEYFRAME.to_vec();
                    payload.push(sent as u8);
                    alice.send_screen(5_000 + sent as u64, &payload, true);
                    sent += 1;
                }
                next_send = Some(Instant::now() + FRAME_INTERVAL);
            }
            if sent == FRAMES && bob.received_from(ALICE_SCREEN).len() >= FRAMES {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        bob
    })
    .await
    .expect("client thread");
    (bob, handle, events)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watched_screen_share_is_forwarded_as_its_own_stream() {
    let (bob, _handle, mut events) = share_screen(true, 1).await;

    // Every screen frame arrives under Alice's screen SSRC, contiguous, and none leaks onto the
    // camera stream.
    let screen = bob.received_from(ALICE_SCREEN);
    assert_eq!(screen.len(), FRAMES, "all screen packets forwarded");
    let mut seqs: Vec<u16> = screen.iter().map(|p| p.seq).collect();
    seqs.sort_unstable();
    assert!(
        seqs.windows(2).all(|w| w[1] == w[0].wrapping_add(1)),
        "rewritten sequence is contiguous: {seqs:?}"
    );
    assert!(screen.iter().all(|p| p.payload[..4] == VP8_KEYFRAME));
    assert!(bob.received_from(ALICE_CAMERA).is_empty());

    let mut reported = None;
    while let Ok(event) = events.try_recv() {
        if let SfuEvent::StreamViewers {
            session_id,
            ssrc,
            viewers,
        } = event
        {
            assert_eq!(session_id.0, "alice");
            assert_eq!(ssrc, ALICE_SCREEN);
            reported = Some(viewers);
        }
    }
    assert_eq!(reported, Some(vec!["user-bob".into()]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unwatched_screen_share_is_not_forwarded() {
    let (bob, _handle, mut events) = share_screen(false, 1).await;

    assert!(bob.received_from(ALICE_SCREEN).is_empty());
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, SfuEvent::StreamViewers { .. }));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn screen_packets_arriving_in_bursts_are_all_observed() {
    let (bob, _handle, _events) = share_screen(true, 8).await;
    let mut indices: Vec<u8> = bob
        .received_from(ALICE_SCREEN)
        .iter()
        .map(|packet| packet.payload[4])
        .collect();
    indices.sort_unstable();
    assert_eq!(indices, (0..FRAMES as u8).collect::<Vec<_>>());
}
