//! Run with an isolated endpoint: cargo test --test etcd_fences -- --ignored.
use serde_json::{Value, json};
use std::time::Duration;
use uuid::Uuid;
use voice_prober::domain::{Health, Registration, Status};
use voice_prober::infrastructure::etcd::{Error, Etcd, encode};

fn store() -> Etcd {
    Etcd::new(vec![
        std::env::var("VOICE_PROBER_TEST_ETCD")
            .expect("isolated VOICE_PROBER_TEST_ETCD is required"),
    ])
    .unwrap()
}

async fn register(store: &Etcd, id: &str) -> Registration {
    let value = json!({"node_id": id, "region": "probe-tests", "ws_url": "ws://localhost", "public_ip": "127.0.0.1", "udp_port": 50000});
    let lease = store
        .post("/v3/lease/grant", &json!({"TTL": "30"}))
        .await
        .unwrap()["ID"]
        .as_str()
        .unwrap()
        .to_owned();
    let key = format!("/discordy/voice/nodes/probe-tests/{id}");
    store
        .post(
            "/v3/kv/put",
            &json!({"key": encode(&key), "value": encode(&value.to_string()), "lease": lease}),
        )
        .await
        .unwrap();
    store
        .registrations()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.node.node_id == id)
        .unwrap()
}

async fn health(store: &Etcd, id: &str) -> Value {
    store
        .post(
            "/v3/kv/range",
            &json!({"key": encode(&format!("/discordy/voice/health/{id}"))}),
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires an explicitly configured disposable etcd"]
async fn concurrent_observers_and_stale_node_generations_cannot_publish_or_delete_successors() {
    let store = store();
    let id = Uuid::new_v4().to_string();
    let first = register(&store, &id).await;
    let owner = store.acquire(&first, 10).await.unwrap();
    assert!(matches!(
        store.acquire(&first, 10).await,
        Err(Error::OwnershipLost)
    ));
    let mut old_health = Health::new(&first, "old");
    old_health.status = Status::Healthy;
    store.publish(&first, &owner, &old_health).await.unwrap();
    assert_eq!(health(&store, &id).await["kvs"][0]["lease"], owner.lease);

    let replacement = register(&store, &id).await;
    assert_ne!(first.lease, replacement.lease);
    assert_eq!(
        store.publish(&first, &owner, &old_health).await,
        Err(Error::OwnershipLost)
    );
    store.release(&owner).await;
    assert_eq!(
        store
            .publish(&replacement, &owner, &Health::new(&replacement, "stale"))
            .await,
        Err(Error::OwnershipLost)
    );
    let successor = store.acquire(&replacement, 10).await.unwrap();
    store
        .publish(&replacement, &successor, &Health::new(&replacement, "new"))
        .await
        .unwrap();
    store.release(&owner).await;
    assert_eq!(
        health(&store, &id).await["kvs"][0]["lease"],
        successor.lease
    );
    store.release(&successor).await;
    for lease in [&first.lease, &replacement.lease] {
        store
            .post("/v3/lease/revoke", &json!({"ID": lease}))
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires an explicitly configured disposable etcd"]
async fn observer_keepalive_and_expiry_control_health_freshness() {
    let store = store();
    let id = Uuid::new_v4().to_string();
    let registration = register(&store, &id).await;
    let claim = store.acquire(&registration, 2).await.unwrap();
    store
        .publish(&registration, &claim, &Health::new(&registration, "test"))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    store.keep_alive(&claim).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(
        health(&store, &id).await["kvs"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        health(&store, &id).await["kvs"]
            .as_array()
            .is_none_or(Vec::is_empty)
    );
    assert_eq!(
        store
            .publish(&registration, &claim, &Health::new(&registration, "test"))
            .await,
        Err(Error::OwnershipLost)
    );
    store
        .post("/v3/lease/revoke", &json!({"ID": registration.lease}))
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires an explicitly configured disposable etcd"]
async fn blacklist_commands_target_exact_scope_and_never_delete_adjacent_keys() {
    let store = store();
    let id = Uuid::new_v4().to_string();
    store
        .blacklist("nodes", &id, "test", "test", 1)
        .await
        .unwrap();
    store
        .blacklist("regions", &id, "test", "test", 1)
        .await
        .unwrap();
    store.clear_blacklist("nodes", &id).await.unwrap();
    let remaining = store
        .post(
            "/v3/kv/range",
            &json!({"key": encode(&format!("/discordy/voice/blacklist/regions/{id}"))}),
        )
        .await
        .unwrap();
    assert_eq!(remaining["kvs"].as_array().unwrap().len(), 1);
    assert_eq!(
        store.clear_blacklist("nodes", "../regions").await,
        Err(Error::InvalidResponse)
    );
    store.clear_blacklist("regions", &id).await.unwrap();
}
