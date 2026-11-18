# 0002: Independent leased media observations

Status: implemented, deployment disabled by default. Date: 2026-11-18.

Central ADR-0025 and P8 define the probe token, discovery health and blacklist contracts.
This implementation keeps the observer outside signaling so that a functioning etcd
heartbeat cannot hide a broken advertised media path. Two synthetic str0m peers exercise
WebSocket admission, ICE, DTLS and bidirectional RTP forwarding in a private probe room.

An exclusive etcd observer claim and each result share a bounded lease. Every result
transaction compares the node registration lease and both the observer lease and random
ownership value. Lease freshness, rather than wall-clock timestamps, governs allocation.
Removing a registration cancels its in-flight network operation. Replacements start
unknown and must earn the configured consecutive passes. Independent timed tasks and
an application-level media deadline prevent one stalled adapter from blocking another node.

N failures and M passes drive a pure hysteresis state machine. Persistent manual blacklist
keys remain gateway overrides and are never removed by observations. Exact-key CLI commands
provide the operator path. Prometheus exposes health/outcome counts and measured loss/RTT;
it deliberately does not label media health as actual allocation eligibility.

The strict probe-token verifier remains unchanged. The issuer backdates `iat` by ten
seconds while setting `exp=iat+60`, shortening usable lifetime and tolerating small clock
differences between independently placed hosts. Excess future skew and expiry still fail.

Validation uses real local signaling/SFU pairs and etcd, including SFU pause/recovery,
stale owners, lease expiry, exact blacklist operations and runtime cancellation. The
[prober README](../../prober/README.md) records commands, metrics and timing bounds. Build
and release integration prepares the image; it does not create a deployment or turn on
`VOICE_REQUIRE_PROBE`. Network placement, dashboard integration and rollout remain separate.
