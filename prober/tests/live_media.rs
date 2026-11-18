//! Requires the explicitly isolated signaling/SFU fixture; never points at production.
use voice_prober::application::Config;
use voice_prober::domain::Failure;
use voice_prober::infrastructure::{etcd::Etcd, media};

#[tokio::test]
#[ignore = "requires disposable signaling/SFU nodes and a dedicated probe key"]
async fn two_distinct_targets_complete_real_ice_dtls_and_bidirectional_rtp() {
    let config = Config::from_env(&std::env::vars().collect()).unwrap();
    let store = Etcd::new(config.endpoints.clone()).unwrap();
    let mut nodes = Vec::new();
    for _ in 0..10 {
        if let Ok(registrations) = store.registrations().await {
            nodes = registrations
                .into_iter()
                .filter(|r| r.node.region == "health-fixture")
                .collect();
            if nodes.len() == 2 {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    assert_eq!(nodes.len(), 2);
    assert_ne!(nodes[0].node.ws_url, nodes[1].node.ws_url);
    assert_ne!(nodes[0].node.udp_port, nodes[1].node.udp_port);
    for registration in &nodes {
        let observation = media::probe(registration, &config).await;
        assert!(
            observation.failure.is_none(),
            "{}: {observation:?}",
            registration.node.node_id
        );
        assert_eq!(observation.loss_pct, Some(0.0));
        assert!(
            observation
                .rtt_ms
                .is_some_and(|rtt| rtt < config.max_rtt_ms)
        );
        println!(
            "{}: loss={:?}% round-trip={:?}ms",
            registration.node.node_id, observation.loss_pct, observation.rtt_ms
        );

        let mut stale = registration.clone();
        stale.lease = "1".into();
        assert_eq!(
            media::probe(&stale, &config).await.failure,
            Some(Failure::Signaling)
        );
    }
}
