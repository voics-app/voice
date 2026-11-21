use std::collections::HashMap;
use voice_prober::application::Config;

fn environment() -> HashMap<String, String> {
    [
        ("VOICE_PROBER_ID", "test"),
        ("ETCD_ENDPOINTS", "http://localhost:2379"),
        (
            "VOICE_PROBE_TOKEN_SECRET",
            "test-probe-secret-0123456789abcdef",
        ),
        ("VOICE_PROBE_SERVER_ID", "1"),
        ("VOICE_PROBE_CHANNEL_ID", "2"),
        ("VOICE_PROBE_USER_IDS", "3,4"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect()
}

#[test]
fn defaults_are_bounded_and_debug_never_discloses_credentials() {
    let vars = environment();
    let config = Config::from_env(&vars).unwrap();
    assert_eq!(config.interval.as_millis(), 10_000);
    assert_eq!(config.timeout.as_millis(), 4_000);
    assert!(config.lease_ttl_s() * 1000 <= 2 * config.interval.as_millis() as u64);
    assert!(!format!("{config:?}").contains(&vars["VOICE_PROBE_TOKEN_SECRET"]));
}

#[test]
fn malformed_and_unbounded_configuration_is_rejected() {
    for (key, value) in [
        ("VOICE_PROBE_TOKEN_SECRET", "weak"),
        ("VOICE_PROBE_USER_IDS", "3,3"),
        ("VOICE_PROBE_USER_IDS", "3,4,5"),
        ("VOICE_PROBE_SERVER_ID", "user"),
        ("VOICE_PROBE_SERVER_ID", "01"),
        ("VOICE_PROBE_INTERVAL_MS", "18446744073709551615"),
        ("VOICE_PROBE_TIMEOUT_MS", "10000"),
        ("VOICE_PROBE_TIMEOUT_MS", "0"),
        ("VOICE_PROBE_FAILURES", "0"),
        ("VOICE_PROBE_PASSES", "0"),
        ("VOICE_PROBE_MAX_LOSS_PCT", "NaN"),
        ("VOICE_PROBE_MAX_RTT_MS", "-1"),
        ("VOICE_PROBE_MAX_RTT_MS", "4000"),
        ("ETCD_ENDPOINTS", "https://user:secret@host:2379"),
        ("ETCD_ENDPOINTS", "https://host:2379/path"),
        ("ETCD_ENDPOINTS", "https://host:2379/#fragment"),
    ] {
        let mut vars = environment();
        vars.insert(key.into(), value.into());
        assert!(Config::from_env(&vars).is_err(), "accepted {key}");
    }
}
