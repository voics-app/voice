//! A viewer that leaves and rejoins gets a keyframe of every camera it receives, even though the
//! publisher only produces keyframes on request.

mod common;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use common::Client;
use sfu_core::application::ports::LocalDescription;
use sfu_core::application::service::{Command, Outcome};
use sfu_core::domain::session::{SessionSpec, VideoLayer};
use sfu_core::infrastructure::engine::{self, EngineConfig, EngineHandle};
use tokio::runtime::Handle;

const ALICE_AUDIO: u32 = 5_111;
const ALICE_CAMERA: u32 = 5_222;
const ALICE_CAMERA_RTX: u32 = 5_223;
const BOB_AUDIO: u32 = 6_111;
const FRAME_INTERVAL: Duration = Duration::from_millis(15);
/// Time the joiner needs before its ICE/DTLS starts answering; the SFU's first keyframe request
/// is answered by the publisher in the meantime.
const SLOW_JOIN: Duration = Duration::from_millis(500);
/// How long a rejoined viewer may wait for its first keyframe.
const KEYFRAME_DEADLINE: Duration = Duration::from_secs(2);

/// A VP8 keyframe payload descriptor (S bit, partition 0) followed by a keyframe header byte.
const VP8_KEYFRAME: [u8; 4] = [0x10, 0x00, 0x9d, 0x01];
/// A VP8 interframe: start of partition 0 with the "not a keyframe" bit set.
const VP8_DELTA: [u8; 4] = [0x10, 0x01, 0x00, 0x00];

async fn create(handle: &EngineHandle, spec: SessionSpec) -> LocalDescription {
    match handle
        .execute(Command::CreateSession(Box::new(spec)))
        .await
        .expect("create session")
    {
        Outcome::Created(local) => local,
        other => panic!("unexpected outcome {other:?}"),
    }
}

/// Alice publishes delta frames and answers keyframe requests, like a real encoder.
struct Publisher {
    client: Client,
    last_frame: Instant,
}

impl Publisher {
    fn pump(&mut self) {
        self.client.pump();
        if !self.client.connected || self.last_frame.elapsed() < FRAME_INTERVAL {
            return;
        }
        self.last_frame = Instant::now();
        let keyframe = std::mem::take(&mut self.client.keyframe_requests) > 0;
        let payload = if keyframe { VP8_KEYFRAME } else { VP8_DELTA };
        self.client.send_video(0, &payload, true);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejoining_viewer_receives_a_camera_keyframe() {
    let config = EngineConfig {
        udp_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        public_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        stats_interval: Duration::from_secs(1),
    };
    let (handle, _task) = engine::spawn(config).await.expect("engine starts");
    let sfu_addr = handle.local_addr();
    let rt = Handle::current();

    let camera = VideoLayer {
        rid: "100".to_owned(),
        quality: 100,
        ssrc: ALICE_CAMERA,
        rtx_ssrc: ALICE_CAMERA_RTX,
    };
    let mut alice = Client::new(ALICE_AUDIO, vec![camera]);
    let alice_spec = alice.session_spec("alice", "room-r");
    let alice_sfu = create(&handle, alice_spec).await;
    alice.connect(&alice_sfu, sfu_addr, &[BOB_AUDIO], &[]);
    let mut alice = Publisher {
        client: alice,
        last_frame: Instant::now(),
    };

    let (first_keyframe, second_keyframe) = tokio::task::spawn_blocking(move || {
        let wait_for_keyframe = |alice: &mut Publisher, session: &str| -> Option<Duration> {
            let mut bob = Client::new(BOB_AUDIO, vec![]);
            let spec = bob.session_spec(session, "room-r");
            let bob_sfu = rt.block_on(create(&handle, spec));
            bob.connect(
                &bob_sfu,
                sfu_addr,
                &[ALICE_AUDIO],
                &[(ALICE_CAMERA, ALICE_CAMERA_RTX)],
            );
            let started = Instant::now();
            let mut connected_at: Option<Instant> = None;
            let give_up = Instant::now() + Duration::from_secs(15);
            let mut found = None;
            while Instant::now() < give_up {
                alice.pump();
                if started.elapsed() >= SLOW_JOIN {
                    bob.pump();
                }
                if bob.connected && connected_at.is_none() {
                    connected_at = Some(Instant::now());
                }
                let keyframe = bob
                    .received_from(ALICE_CAMERA)
                    .iter()
                    .any(|p| p.payload[..4] == VP8_KEYFRAME);
                if keyframe {
                    found = connected_at.map(|t| t.elapsed());
                    break;
                }
                if connected_at.is_some_and(|t| t.elapsed() > KEYFRAME_DEADLINE) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            rt.block_on(handle.execute(Command::CloseSession(session.into())))
                .expect("close session");
            found
        };
        let first = wait_for_keyframe(&mut alice, "bob-1");
        let second = wait_for_keyframe(&mut alice, "bob-2");
        (first, second)
    })
    .await
    .expect("client thread");

    eprintln!(
        "camera keyframe after transport connection: first={first_keyframe:?}, rejoin={second_keyframe:?}"
    );
    assert!(first_keyframe.is_some(), "first join gets a keyframe");
    assert!(
        first_keyframe.is_some_and(|latency| latency < Duration::from_millis(500)),
        "first join must not wait for a complete bitrate window: {first_keyframe:?}"
    );
    assert!(
        second_keyframe.is_some(),
        "rejoined viewer gets a keyframe within {KEYFRAME_DEADLINE:?}"
    );
}
