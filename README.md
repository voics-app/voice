# voice

Voics voice: the signaling service (Elixir), the SFU (Rust, [str0m](https://github.com/algesten/str0m)) and `regionctl`, the CLI that adds and drains voice regions.

| Component | Path | Image | Role |
|---|---|---|---|
| voice-signaling | `signaling/` | `ghcr.io/discordy-voics/voice-signaling` | Voice WebSocket (`/voice`, port 4100), room registry, SFU control client, etcd discovery, drain |
| voice-sfu | `sfu/` | `ghcr.io/discordy-voics/voice-sfu` | UDP media mux (port 50000), ICE-lite and DTLS-SRTP, audio and simulcast/SVC video forwarding, NDJSON control (`127.0.0.1:4101`) |
| regionctl | `regionctl/` | none (release binary) | `add`, `list`, `drain`, `undrain` |

The design follows [How Discord handles two and a half million concurrent voice users using WebRTC](https://discord.com/blog/how-discord-handles-two-and-half-million-concurrent-voice-users-using-webrtc): one SFU per node, all channels of a guild on one node, signaling decoupled from media, and state rebuilt by replaying sessions when the SFU restarts. The wire contracts are the pinned interface **P8** of the master plan; see [ARCHITECTURE.md](ARCHITECTURE.md).

## Multiple voice nodes

Each signaling process and its SFU form one allocation target. Give it a unique `VOICE_NODE_ID`,
dedicated `VOICE_PUBLIC_WS_URL`, and reachable public IP/UDP-port pair. The gateway selects
that target once per guild or DM call and reuses it. A WebSocket Service that randomly routes
the node's URL to another signaling replica breaks token and room affinity.

Node discovery uses an atomic etcd claim. A duplicate live identity cannot overwrite the
existing owner; refresh and shutdown deletion compare the owning lease. A process with an
expired or lost registration reports unready and rejects new IDENTIFY requests. Existing media
sessions continue while discovery recovers. A replica that crashes leaves its lease to expire
before a replacement can acquire its ID. Signaling does not generate Snowflake IDs.

The ownership regression runs against real etcd as part of `signaling/test/etcd_integration_test.exs`.
For tests already running in a Docker network, set `ETCD_TEST_ENDPOINT=etcd-host:2379`; otherwise
the suite starts a disposable etcd on a random host port.

## Run locally

Prerequisites: Rust 1.99, Elixir 1.20 / OTP 29, an etcd on `localhost:2379` (the compose stack in `gitops/local` provides one).

```bash
# SFU: media on UDP 50000, control on 127.0.0.1:4101, ops HTTP on 4102
export SFU_PUBLIC_IP=127.0.0.1
make -C . dev-sfu

# signaling: WebSocket on 4100
cp signaling/.env.example signaling/.env   # dev-only values
set -a; . signaling/.env; set +a
make dev-signaling
```

`make dev` prints both commands. Clients connect to `ws://localhost:4100/voice` (through the local Caddy ingress as `/voice`).

## Test

```bash
make test    # cargo test (sfu + regionctl) and mix test
make lint    # cargo fmt --check, clippy -D warnings, mix format --check-formatted, mix credo --strict
make docker  # builds voice-signaling and voice-sfu
```

- `sfu/crates/sfu-core/tests/two_clients_audio.rs`: two in-process str0m clients exchange Opus through the SFU over real UDP.
- `sfu/crates/sfu-core/tests/screen_share.rs`: exact packet and payload delivery for paced and burst screen streams, plus opt-in isolation. The shared client drains str0m output after every received datagram and RTP write: RTP mode retains only one pending packet event, so batching inputs before polling silently loses observations in the fixture.
- `sfu/crates/sfu-core/tests/camera_rejoin.rs`: first/rejoin keyframe latency after transport connection, including a deliberately delayed viewer handshake. The first keyframe must arrive within 500ms; allocation uses a startup estimate so it does not wait for a complete one-second bitrate window.
- `sfu/crates/sfu-control/tests`: the NDJSON control server against a real engine over TCP.
- `signaling/test`: protocol use cases, rooms, the SFU client against a scripted SFU (including replay after a restart), etcd registration against a real etcd (Docker, random port; skipped without Docker) and full WebSocket handshakes with gun clients.
- `regionctl/tests`: the CLI against a temporary gitops checkout, a mock Cloudflare API and a mock gateway.

## Configuration

Names follow pinned interface P2. Configuration is read from the environment and validated at startup; the process refuses to boot on errors.

**voice-signaling**

| Variable | Required | Default | Meaning |
|---|---|---|---|
| `VOICE_REGION` | yes | | Region name (etcd path, allocation) |
| `VOICE_NODE_ID` | yes | | Unique node id; must equal the `node` claim of voice tokens |
| `VOICE_PUBLIC_WS_URL` | yes | | `wss://` URL clients use for the voice WebSocket |
| `VOICE_PUBLIC_IP` | yes | | IP advertised in READY and SESSION_DESCRIPTION candidates |
| `VOICE_LAN_IP` | no | | Node LAN IP added as a first ICE host candidate so clients behind the same NAT (router without hairpin) connect directly; set from `status.hostIP` in Kubernetes |
| `VOICE_UDP_PORT` | no | `50000` | Media port advertised to clients |
| `ETCD_ENDPOINTS` | yes | | Comma-separated `host:port` (a leading `http://` is accepted) |
| `VOICE_TOKEN_SECRET` | yes | | HS256 secret shared with the gateway, at least 16 bytes |
| `INTERNAL_SERVICE_TOKEN` | yes | | Protects `/internal/v1/*` (`X-Internal-Token`) |
| `SFU_CONTROL_ADDR` | no | `127.0.0.1:4101` | SFU control socket |
| `SFU_EXECUTABLE` | no | unset | When set, signaling runs and restarts the SFU itself (otherwise it is a sibling container) |
| `VOICE_DRAIN_ON_START` | no | `false` | Start in drain mode (set by `regionctl drain --region`) |
| `PORT` | no | `4100` | HTTP and WebSocket port |
| `DISCORDY_ENV` | no | `local` | `local` or `production` |
| `LOG_LEVEL` | no | `info` | `debug`, `info`, `notice`, `warning`, `error` |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | no | unset | Export traces over OTLP when set |

**voice-sfu**

| Variable | Required | Default | Meaning |
|---|---|---|---|
| `SFU_PUBLIC_IP` | yes | | IP in the ICE candidates the SFU hands out |
| `SFU_UDP_BIND` | no | `0.0.0.0:50000` | Media socket |
| `SFU_CONTROL_BIND` | no | `127.0.0.1:4101` | NDJSON control socket |
| `SFU_HTTP_BIND` | no | `0.0.0.0:4102` | `/healthz`, `/readyz`, `/metrics` (additive to P2: the SFU has no other HTTP port) |
| `LOG_LEVEL` | no | `info` | Filter for JSON logs |

**regionctl**: `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ZONE_ID` (DNS edit on the zone; required by `add`, optional for `list`), `REGIONCTL_GITOPS_DIR` (same as `--gitops-dir`), `INTERNAL_SERVICE_TOKEN` and `GATEWAY_INTERNAL_URL` (node drain).

## Operational endpoints

signaling (port 4100) and the SFU (port 4102) both serve `GET /healthz`, `GET /readyz` and `GET /metrics` (Prometheus) and write JSON logs to stdout. Signaling is ready when the SFU control link is up and the node is registered in etcd.

Signaling also serves `POST /internal/v1/drain`, `POST /internal/v1/undrain` and `GET /internal/v1/status` (token protected). The gateway can alternatively drain a node by writing `/discordy/voice/drain/<node_id>` in etcd; signaling polls it every 3 seconds.

## regionctl

```bash
export CLOUDFLARE_API_TOKEN=... CLOUDFLARE_ZONE_ID=...
regionctl add eu-west --public-ip 198.51.100.4 --public-host discordy-rtc-eu-west.voics.nl --gitops-dir ~/src/gitops
regionctl list --gitops-dir ~/src/gitops
regionctl drain eu-west-1                      # live: gateway API, needs INTERNAL_SERVICE_TOKEN
regionctl drain --region eu-west --gitops-dir ~/src/gitops   # durable: nodes start draining
regionctl undrain eu-west-1
```

`add` upserts a proxied A record for `--public-host` (`--dns-only` to skip the proxy), writes `regions/<name>/kustomization.yaml` from the template conventions of the gitops repository (next free `SNOWFLAKE_WORKER_ID` in 192-255, node id `<name>-1`), and registers the overlay in `regions/kustomization.yaml`. It never commits: review the diff and open the PR.

## Pending owner actions

- **GitHub Actions**: `ci.yml` calls the reusable workflows of `discordy-voics/.github@main`; they run once the repository is pushed and the runner scale set exists.
- **Cloudflare**: `regionctl add` needs a real `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ZONE_ID`. It was verified against a mock of the Cloudflare API only. A region reached through the Cloudflare tunnel also needs its tunnel ingress rule (see the voice-regions runbook).
- **UDP 50000** must be reachable at each region's public IP (router forward for `nl-home`).
- **Gateway integration** (`VOICE_TOKEN_SECRET`, etcd drain marker) was verified against the pinned interfaces, not against a running gateway.

## Git hooks

Tests run locally, not in CI. Install [lefthook](https://github.com/evilmartians/lefthook) once and run `make hooks` (or `lefthook install`) to enable the pre-push hook in `lefthook.yml`: it runs this repository's lint and unit tests before every push. CI repeats only the slim gate on pull requests. Skip once with `LEFTHOOK=0 git push`.
