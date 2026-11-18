use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use voice_prober::application::{
    Clock, Config, HealthStore, MediaProbe, NodeDiscovery, StoreError,
};
use voice_prober::domain::{Health, Node, Observation, Registration};
use voice_prober::interface::{daemon::Runtime, metrics::Metrics};

#[derive(Default)]
struct Store {
    registrations: Mutex<Vec<Registration>>,
    published: Mutex<Vec<Health>>,
    released: Mutex<Vec<String>>,
}

impl HealthStore for Store {
    type Claim = String;
    async fn acquire(&self, registration: &Registration, _: u64) -> Result<String, StoreError> {
        Ok(registration.node.node_id.clone())
    }
    async fn renew(&self, _: &String) -> Result<(), StoreError> {
        Ok(())
    }
    async fn publish(
        &self,
        _: &Registration,
        _: &String,
        health: &Health,
    ) -> Result<(), StoreError> {
        self.published.lock().unwrap().push(health.clone());
        Ok(())
    }
    async fn release(&self, claim: &String) -> Result<(), StoreError> {
        self.released.lock().unwrap().push(claim.clone());
        Ok(())
    }
}

impl NodeDiscovery for Store {
    async fn registrations(&self) -> Result<Vec<Registration>, StoreError> {
        Ok(self.registrations.lock().unwrap().clone())
    }
}

struct Media;
impl MediaProbe for Media {
    async fn probe(&self, registration: &Registration, _: &Config) -> Observation {
        if registration.node.node_id == "stalled" {
            std::future::pending().await
        } else {
            Observation {
                loss_pct: Some(0.0),
                rtt_ms: Some(1.0),
                failure: None,
            }
        }
    }
}

struct TestClock;
impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        1
    }
}

fn registration(id: &str) -> Registration {
    Registration {
        key: format!("/discordy/voice/nodes/test/{id}"),
        lease: "1".into(),
        node: Node {
            node_id: id.into(),
            region: "test".into(),
            ws_url: "ws://127.0.0.1".into(),
            public_ip: "127.0.0.1".into(),
            udp_port: 50000,
        },
    }
}

fn config() -> Config {
    let vars: HashMap<_, _> = [
        ("VOICE_PROBER_ID", "test"),
        ("ETCD_ENDPOINTS", "127.0.0.1:2379"),
        (
            "VOICE_PROBE_TOKEN_SECRET",
            "test-only-probe-secret-0123456789abcdef",
        ),
        ("VOICE_PROBE_SERVER_ID", "1"),
        ("VOICE_PROBE_CHANNEL_ID", "2"),
        ("VOICE_PROBE_USER_IDS", "3,4"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();
    let mut config = Config::from_env(&vars).unwrap();
    config.interval = Duration::from_millis(30);
    config.timeout = Duration::from_millis(20);
    config
}

#[tokio::test]
async fn stalled_media_is_bounded_and_other_nodes_keep_publishing() {
    let store = Arc::new(Store::default());
    *store.registrations.lock().unwrap() = vec![registration("stalled"), registration("fast")];
    let runtime = Arc::new(Runtime {
        config: Arc::new(config()),
        store: store.clone(),
        media: Arc::new(Media),
        clock: Arc::new(TestClock),
        sink: Arc::new(Metrics::default()),
    });
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(runtime.run(receiver));
    tokio::time::sleep(Duration::from_millis(150)).await;
    let published = store.published.lock().unwrap().clone();
    assert!(
        published
            .iter()
            .filter(|h| h.node_id == "fast" && h.consecutive_passes > 0)
            .count()
            >= 2
    );
    assert!(
        published
            .iter()
            .any(|h| h.node_id == "stalled" && h.consecutive_failures >= 2)
    );
    shutdown.send(true).unwrap();
    tokio::time::timeout(Duration::from_millis(150), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn removed_stalled_node_releases_ownership_without_blocking_new_discovery() {
    let store = Arc::new(Store::default());
    *store.registrations.lock().unwrap() = vec![registration("stalled")];
    let mut configuration = config();
    configuration.timeout = Duration::from_secs(60);
    let runtime = Arc::new(Runtime {
        config: Arc::new(configuration),
        store: store.clone(),
        media: Arc::new(Media),
        clock: Arc::new(TestClock),
        sink: Arc::new(Metrics::default()),
    });
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(runtime.run(receiver));
    tokio::time::sleep(Duration::from_millis(40)).await;
    *store.registrations.lock().unwrap() = vec![registration("new")];
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        store
            .released
            .lock()
            .unwrap()
            .iter()
            .any(|id| id == "stalled")
    );
    assert!(
        store
            .published
            .lock()
            .unwrap()
            .iter()
            .any(|h| h.node_id == "new" && h.consecutive_passes > 0)
    );
    shutdown.send(true).unwrap();
    tokio::time::timeout(Duration::from_millis(150), task)
        .await
        .unwrap()
        .unwrap();
}
