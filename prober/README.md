# Independent voice health observer

`voice-prober run` discovers leased voice registrations in etcd and starts a timed task
for each unambiguous node ID. Each observation opens two short-lived synthetic sessions
through the advertised WebSocket URL, completes ICE and DTLS, and sends 50 Opus-like
RTP packets at 20 ms intervals. The receiving peer echoes them through the SFU. Returned
packet count measures round-trip loss; the median elapsed time measures round-trip RTT
through both peers and two SFU forwarding hops. This is synthetic transport traffic,
not an audio-decoder or DAVE compatibility test.

Probe tokens use only `VOICE_PROBE_TOKEN_SECRET`. Signaling derives the private room
from the current registration lease and a per-run UUID. Server, channel and user IDs
do not grant access to a user call. Tokens have `iat=now-10` and `exp=now+50`: the strict
maximum lifetime remains 60 seconds and usable lifetime is shortened. This tolerates
a small observer clock lead without changing signaling's verifier; larger future skew
and expired tokens still fail. Keep normal clock synchronization in place.

No daemon, secret binding, deployment or required-health enforcement starts automatically.
Roll out the matching contracts, signaling admission and gateway reader first. Deploy
the prober at the network location whose client route must be measured, observe successful
results, then enable `VOICE_REQUIRE_PROBE` on selected nodes as a separate rollout.

## Configuration

| Environment variable | Default | Meaning |
|---|---|---|
| `VOICE_PROBER_ID` | required | Printable observer identity, at most 128 characters |
| `ETCD_ENDPOINTS` | required | One to five HTTP(S) origins; `host:port` accepts an `http://` prefix |
| `VOICE_PROBE_TOKEN_SECRET` | required | At least 32 bytes, matching signaling's separate probe key |
| `VOICE_PROBE_SERVER_ID` | required | Positive decimal synthetic server ID |
| `VOICE_PROBE_CHANNEL_ID` | required | Positive decimal synthetic channel ID |
| `VOICE_PROBE_USER_IDS` | required | Exactly two distinct positive decimal IDs, comma separated |
| `VOICE_PROBE_INTERVAL_MS` | `10000` | Time between observation starts; maximum 300000 ms |
| `VOICE_PROBE_TIMEOUT_MS` | `4000` | Total media budget, 1500..10000 ms and less than the interval |
| `VOICE_PROBE_FAILURES` | `2` | Consecutive failures before unhealthy |
| `VOICE_PROBE_PASSES` | `2` | Consecutive passes before healthy/recovery |
| `VOICE_PROBE_MAX_LOSS_PCT` | `10` | Maximum accepted round-trip packet loss, 0..100 |
| `VOICE_PROBE_MAX_RTT_MS` | `1000` | Maximum accepted median RTT; positive and less than timeout |
| `VOICE_PROBE_HTTP_BIND` | `127.0.0.1:4103` | Operational listener; image default is `0.0.0.0:4103` |

Do not give the daemon `VOICE_TOKEN_SECRET` or any real call token. Environment values,
JWTs and server-supplied close reasons are not included in diagnostics. WebSocket frames
are bounded to 64 KiB. The etcd adapter does not follow redirects or accept credentials
embedded in endpoint URLs. Use network access controls to restrict etcd and metrics;
the present deployment contract uses reachable HTTP(S) etcd endpoints, without an etcd
authentication-token or client-certificate option.

## State and operation

The observer claims `/discordy/voice/probers/<node_id>` exclusively under a lease whose
TTL is at most two configured intervals (whole seconds). It initially publishes `unknown`.
Every health write to `/discordy/voice/health/<node_id>` compares the node registration's
lease and the observer claim's lease and random ownership value. The health key shares
the observer lease. An old process cannot publish to or delete its successor's result.
An observer crash lets its lease expire; a graceful SIGINT/SIGTERM cancels media and
revokes owned leases. Timestamp fields are diagnostic; allocation freshness uses leases.

Two default failures publish `unhealthy`; two passes publish `healthy`. A single failure
does not undo a healthy state, and one recovery pass does not undo an unhealthy state.
The result is published in the same observation cycle as the threshold crossing. At
the defaults the second failed result follows the first by one 10-second interval.
An outage starting between probes can take up to two intervals plus the probe timeout
to produce the second failed result; gateway discovery polling adds its own delay.

Gateway allocation also considers node and region blacklists, draining, identity and
endpoint ambiguity. A healthy probe does not imply that allocation is eligible, and
existing calls are not disconnected by the observer. The gateway's reader must match
the result's `registration_lease` before using it.

```sh
cd prober
cargo run --locked -p voice-prober-bin -- run
cargo run --locked -p voice-prober-bin -- check eu-west-1
```

`check` emits one JSON result and a failing exit status on failure. It does not acquire
an observer lease or publish health, so it cannot alter allocation state.

Manual exclusion commands need only `ETCD_ENDPOINTS`:

```sh
voice-prober blacklist nodes eu-west-1 "UDP maintenance" "operator-name"
voice-prober blacklist regions eu-west "upstream outage" "operator-name"
voice-prober clear-blacklist nodes eu-west-1
voice-prober clear-blacklist regions eu-west
```

The CLI validates the scope and ID and writes/deletes only the exact key under
`/discordy/voice/blacklist/{nodes|regions}/<id>`. Inspect the existing reason before
clearing it. Key presence excludes allocations, including malformed values. The probe
loop never clears blacklists, and clearing one does not bypass other allocation checks.

## Metrics

`GET /healthz` is process liveness. `GET /metrics` exports Prometheus text with `node_id`
and `region` labels. It does not certify external reachability merely because the HTTP
listener answers.

| Metric | Meaning |
|---|---|
| `voice_probe_observer_owned` | Latest observation was successfully published under an owned lease |
| `voice_probe_state{state="unknown\|healthy\|unhealthy"}` | One-hot health state |
| `voice_probe_results_total{outcome="success\|signaling\|transport\|rtp_loss\|rtt\|timeout"}` | Published observations, by outcome |
| `voice_probe_consecutive_failures`, `voice_probe_consecutive_passes` | Current hysteresis counters |
| `voice_probe_last_checked_timestamp_seconds` | Diagnostic time of the latest completed observation |
| `voice_probe_packet_loss_ratio` | Latest measured fraction of missing round trips |
| `voice_probe_rtt_seconds` | Latest median synthetic round-trip duration |

Unavailable ownership/publishing exports unknown and omits stale loss/RTT samples.
Removed registrations remove their label sets. Counters reset when a label set is
removed or this process restarts. Metrics describe this observer's observations; the
gateway remains authoritative for actual allocation eligibility.

## Verification

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
VOICE_PROBER_TEST_ETCD=http://127.0.0.1:52379 cargo test --locked --test etcd_fences -- --ignored
```

The unit/runtime tests cover hysteresis, packet-loss/RTT thresholds, bounded configuration,
credential-safe diagnostics, bounded stalled adapters, cancellation on registration removal,
independent node progress and metrics. The ignored etcd suite checks real transactions,
competing observers, stale node/observer fencing, renewal/expiry and exact blacklist deletion.

The optional [two-node fixture](tests/fixtures/compose.yml) runs only on loopback. Give
`VOICE_PROBE_TOKEN_SECRET`, `VOICE_TOKEN_SECRET` and `INTERNAL_SERVICE_TOKEN` fresh,
independent disposable values in a local environment file, then:

```sh
docker compose -p voics-health-acceptance --env-file /private/fixture.env -f tests/fixtures/compose.yml up -d --build
```

Use the same disposable probe key in the test process; set `ETCD_ENDPOINTS` to
`http://127.0.0.1:52379`, `VOICE_PROBER_ID` to `acceptance`, synthetic IDs to `1`, `2`
and `3,4`, and the following explicit fixture selectors:

```sh
export VOICE_PROBER_TEST_PAUSE_CONTAINER="$(docker compose -p voics-health-acceptance --env-file /private/fixture.env -f tests/fixtures/compose.yml ps -q sfu-a)"
export VOICE_PROBER_TEST_FAILING_NODE=health-a
cargo test --locked --test live_media -- --ignored --nocapture
cargo test --locked --test live_runtime -- --ignored --nocapture
```

Run the real-etcd and live suites separately; the live runtime test requires exactly
the two fixture registrations. The media test completes both real transports and rejects
a stale registration lease. The runtime test pauses only a container carrying the
`voics.fixture=health-probe` label, proves the first/second failure and recovery transitions,
keeps the other node healthy, and restores the paused container even if an assertion fails.
It removes owned health keys on shutdown. Stop the disposable compose project when done.

The live acceptance proves local route behavior only. It does not prove WAN exposure,
cross-host NAT, a production placement, a released Linux image or an admin-dashboard panel.
