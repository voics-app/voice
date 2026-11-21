//! Mutates only the explicitly named, labelled disposable Docker fixture.
use serde_json::{Value, json};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use voice_prober::application::Config;
use voice_prober::infrastructure::{
    RtcProbe, SystemClock,
    etcd::{Etcd, encode},
};
use voice_prober::interface::{daemon::Runtime, metrics::Metrics};

fn docker(args: &[&str]) {
    assert!(
        Command::new("docker")
            .args(args)
            .output()
            .unwrap()
            .status
            .success(),
        "fixture Docker action failed"
    );
}

struct Paused(String);
impl Paused {
    fn new(name: String) -> Self {
        let label = Command::new("docker")
            .args([
                "inspect",
                "--format",
                "{{ index .Config.Labels \"voics.fixture\" }}",
                &name,
            ])
            .output()
            .unwrap();
        assert!(label.status.success());
        assert_eq!(
            String::from_utf8(label.stdout).unwrap().trim(),
            "health-probe"
        );
        docker(&["pause", &name]);
        Self(name)
    }
}
impl Drop for Paused {
    fn drop(&mut self) {
        // Also restore the disposable peer when a test assertion panics.
        let _ = Command::new("docker").args(["unpause", &self.0]).output();
    }
}

async fn health(store: &Etcd, node: &str) -> Option<Value> {
    let response = store
        .post(
            "/v3/kv/range",
            &json!({
                "key": encode(&format!("/discordy/voice/health/{node}")),
            }),
        )
        .await
        .unwrap();
    let value = response["kvs"][0]["value"].as_str()?;
    use base64::{Engine, engine::general_purpose::STANDARD};
    serde_json::from_slice(&STANDARD.decode(value).unwrap()).ok()
}

async fn wait_for(store: &Etcd, node: &str, predicate: impl Fn(&Value) -> bool) -> Value {
    tokio::time::timeout(Duration::from_secs(35), async {
        loop {
            if let Some(value) = health(store, node).await
                && predicate(&value)
            {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("expected leased health transition")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires two disposable peers and VOICE_PROBER_TEST_PAUSE_CONTAINER"]
async fn actual_runtime_excludes_after_two_failures_recovers_after_two_passes_and_keeps_other_node_live()
 {
    let mut config = Config::from_env(&std::env::vars().collect()).unwrap();
    // Default interval and thresholds; the failure-to-exclusion bound is measured below.
    config.interval = Duration::from_secs(10);
    config.timeout = Duration::from_secs(4);
    config.failures = 2;
    config.passes = 2;
    assert!(
        config
            .endpoints
            .iter()
            .all(|url| url.starts_with("http://127.0.0.1:"))
    );
    let name = std::env::var("VOICE_PROBER_TEST_PAUSE_CONTAINER")
        .expect("explicit disposable SFU required");
    let failing =
        std::env::var("VOICE_PROBER_TEST_FAILING_NODE").expect("explicit node ID required");
    let store = Arc::new(Etcd::new(config.endpoints.clone()).unwrap());
    let nodes = store.registrations().await.unwrap();
    assert_eq!(nodes.len(), 2, "use a dedicated, empty etcd fixture");
    assert!(
        nodes
            .iter()
            .all(|r| r.node.region == "health-fixture"
                && r.node.ws_url.starts_with("ws://127.0.0.1:"))
    );
    assert!(nodes.iter().any(|r| r.node.node_id == failing));
    let other = nodes
        .iter()
        .find(|r| r.node.node_id != failing)
        .unwrap()
        .node
        .node_id
        .clone();
    let metrics = Arc::new(Metrics::default());
    let runtime = Arc::new(Runtime {
        config: Arc::new(config),
        store: store.clone(),
        media: Arc::new(RtcProbe),
        clock: Arc::new(SystemClock),
        sink: metrics.clone(),
    });
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(runtime.run(receiver));
    wait_for(&store, &failing, |v| v["status"] == "healthy").await;
    let other_before = wait_for(&store, &other, |v| v["status"] == "healthy").await;

    let paused = Paused::new(name);
    let first_failure = wait_for(&store, &failing, |v| v["consecutive_failures"] == 1).await;
    assert_eq!(first_failure["status"], "healthy");
    let failure_detected = Instant::now();
    let unhealthy = wait_for(&store, &failing, |v| v["status"] == "unhealthy").await;
    assert_eq!(unhealthy["consecutive_failures"], 2);
    assert!(failure_detected.elapsed() < Duration::from_secs(20));
    let other_after = health(&store, &other).await.unwrap();
    assert_eq!(other_after["status"], "healthy");
    assert!(
        other_after["consecutive_passes"].as_u64() > other_before["consecutive_passes"].as_u64()
    );
    println!(
        "second failed probe published unhealthy after {:?} from first failure; other node stayed healthy",
        failure_detected.elapsed()
    );

    drop(paused);
    let recovering = wait_for(&store, &failing, |v| v["consecutive_passes"] == 1).await;
    assert_eq!(recovering["status"], "unhealthy");
    let recovered = wait_for(&store, &failing, |v| v["status"] == "healthy").await;
    assert_eq!(recovered["consecutive_passes"], 2);
    let rendered = metrics.render();
    assert!(rendered.contains("outcome=\"timeout\"} 2"));
    assert!(rendered.contains("voice_probe_packet_loss_ratio{"));
    assert!(rendered.contains("voice_probe_rtt_seconds{"));
    shutdown.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(health(&store, &failing).await.is_none());
    assert!(health(&store, &other).await.is_none());
}
