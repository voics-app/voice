//! The CLI end to end: a gitops checkout in a temp directory and a mock Cloudflare API on a
//! local socket.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};

const NL_HOME: &str = "\
# Voice region nl-home
apiVersion: kustomize.config.k8s.io/v1beta1
kind: Kustomization
resources:
  - ../_template
nameSuffix: -nl-home
configMapGenerator:
  - name: voice-region
    behavior: merge
    literals:
      - VOICE_REGION=nl-home
      - VOICE_NODE_ID=nl-home-gen-serv
      - VOICE_PUBLIC_WS_URL=wss://${RTC_HOST_PREFIX}-nl-home.${DISCORDY_DOMAIN}
      - VOICE_PUBLIC_IP=${HOME_PUBLIC_IP}
      - SFU_PUBLIC_IP=${HOME_PUBLIC_IP}
      # Unique per region within the voice worker range 192-255 (P5).
      - SNOWFLAKE_WORKER_ID=192
generatorOptions:
  disableNameSuffixHash: true
";

#[derive(Default)]
struct Cloudflare {
    records: BTreeMap<String, Value>,
    requests: Vec<(String, String, Option<String>)>,
    next_id: u32,
}

struct MockApi {
    base: String,
    state: Arc<Mutex<Cloudflare>>,
}

fn respond(stream: &mut TcpStream, status: u16, body: &Value) {
    let body = body.to_string();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn serve(mut stream: TcpStream, state: &Mutex<Cloudflare>) {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            return;
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&data[..header_end]).into_owned();
    let length = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while data.len() < header_end + length {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
    }
    let body: Option<Value> = serde_json::from_slice(&data[header_end..]).ok();

    let first = head.lines().next().unwrap_or_default().to_owned();
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let auth = head
        .lines()
        .find_map(|l| {
            l.strip_prefix("authorization: ")
                .or_else(|| l.strip_prefix("Authorization: "))
        })
        .unwrap_or_default()
        .to_owned();

    let mut state = state.lock().unwrap();
    state
        .requests
        .push((method.clone(), target.clone(), Some(auth.clone())));
    if auth != "Bearer test-token" {
        respond(
            &mut stream,
            403,
            &json!({"success": false, "errors": [{"code": 9109, "message": "Invalid access token"}], "result": null}),
        );
        return;
    }
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    assert!(
        path.starts_with("/zones/zone-1/dns_records"),
        "unexpected path {path}"
    );

    match method.as_str() {
        "GET" => {
            let name = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("name="))
                .unwrap_or_default()
                .to_owned();
            let found: Vec<Value> = state.records.get(&name).cloned().into_iter().collect();
            respond(
                &mut stream,
                200,
                &json!({"success": true, "errors": [], "result": found}),
            );
        }
        "POST" => {
            let mut record = body.expect("json body");
            state.next_id += 1;
            record["id"] = json!(format!("rec-{}", state.next_id));
            let name = record["name"].as_str().unwrap().to_owned();
            state.records.insert(name, record.clone());
            respond(
                &mut stream,
                200,
                &json!({"success": true, "errors": [], "result": record}),
            );
        }
        "PUT" => {
            let mut record = body.expect("json body");
            let id = path.rsplit('/').next().unwrap().to_owned();
            record["id"] = json!(id);
            let name = record["name"].as_str().unwrap().to_owned();
            state.records.insert(name, record.clone());
            respond(
                &mut stream,
                200,
                &json!({"success": true, "errors": [], "result": record}),
            );
        }
        other => panic!("unexpected method {other}"),
    }
}

fn start_mock() -> MockApi {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(Mutex::new(Cloudflare::default()));
    let shared = Arc::clone(&state);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            serve(stream, &shared);
        }
    });
    MockApi { base, state }
}

fn gitops_checkout() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let regions = dir.path().join("regions");
    fs::create_dir_all(regions.join("_template")).unwrap();
    fs::create_dir_all(regions.join("common")).unwrap();
    fs::create_dir_all(regions.join("nl-home")).unwrap();
    fs::write(regions.join("nl-home/kustomization.yaml"), NL_HOME).unwrap();
    fs::write(
        regions.join("kustomization.yaml"),
        "# Layer 5b\napiVersion: kustomize.config.k8s.io/v1beta1\nkind: Kustomization\nresources:\n  - common\n  - nl-home\n",
    )
    .unwrap();
    dir
}

fn run(args: &[&str], env: &[(&str, &str)]) -> ExitCode {
    let env: BTreeMap<String, String> = env
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    regionctl::cli::run(
        std::iter::once("regionctl").chain(args.iter().copied()),
        move |name| env.get(name).cloned(),
    )
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap()
}

fn cf_env(api: &MockApi) -> Vec<(&'static str, String)> {
    vec![
        ("CLOUDFLARE_API_TOKEN", "test-token".to_owned()),
        ("CLOUDFLARE_ZONE_ID", "zone-1".to_owned()),
        ("CLOUDFLARE_API_BASE", api.base.clone()),
    ]
}

fn run_with(args: &[&str], env: &[(&'static str, String)]) -> ExitCode {
    let pairs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    run(args, &pairs)
}

#[test]
fn add_creates_dns_overlay_and_registration() {
    let api = start_mock();
    let dir = gitops_checkout();
    let gitops = dir.path().to_str().unwrap();

    let code = run_with(
        &[
            "add",
            "eu-west",
            "--public-ip",
            "198.51.100.4",
            "--public-host",
            "discordy-rtc-eu-west.voics.nl",
            "--gitops-dir",
            gitops,
        ],
        &cf_env(&api),
    );
    assert_eq!(code, ExitCode::SUCCESS);

    let record = api.state.lock().unwrap().records["discordy-rtc-eu-west.voics.nl"].clone();
    assert_eq!(record["type"], "A");
    assert_eq!(record["content"], "198.51.100.4");
    assert_eq!(record["proxied"], true);

    let overlay = read(dir.path(), "regions/eu-west/kustomization.yaml");
    assert!(overlay.contains("nameSuffix: -eu-west"));
    assert!(overlay.contains("VOICE_PUBLIC_WS_URL=wss://discordy-rtc-eu-west.voics.nl"));
    assert!(
        overlay.contains("SNOWFLAKE_WORKER_ID=193"),
        "nl-home owns 192"
    );
    assert!(read(dir.path(), "regions/kustomization.yaml").contains("  - nl-home\n  - eu-west\n"));
}

#[test]
fn adding_twice_fails_without_touching_dns_again() {
    let api = start_mock();
    let dir = gitops_checkout();
    let gitops = dir.path().to_str().unwrap();
    let env = cf_env(&api);

    let args = [
        "add",
        "eu-west",
        "--public-ip",
        "198.51.100.4",
        "--public-host",
        "discordy-rtc-eu-west.voics.nl",
        "--gitops-dir",
        gitops,
        "--dns-only",
    ];
    assert_eq!(run_with(&args, &env), ExitCode::SUCCESS);
    assert_eq!(
        api.state.lock().unwrap().records["discordy-rtc-eu-west.voics.nl"]["proxied"],
        false
    );

    // The region exists now: a second add fails and must not touch DNS again.
    let before = api.state.lock().unwrap().requests.len();
    assert_eq!(run_with(&args, &env), ExitCode::FAILURE);
    assert_eq!(api.state.lock().unwrap().requests.len(), before);
}

#[test]
fn add_fails_without_cloudflare_credentials_and_changes_nothing() {
    let dir = gitops_checkout();
    let gitops = dir.path().to_str().unwrap();

    let code = run(
        &[
            "add",
            "eu-west",
            "--public-ip",
            "198.51.100.4",
            "--public-host",
            "discordy-rtc-eu-west.voics.nl",
            "--gitops-dir",
            gitops,
        ],
        &[("CLOUDFLARE_API_TOKEN", "test-token")],
    );

    assert_eq!(code, ExitCode::FAILURE);
    assert!(!dir.path().join("regions/eu-west").exists());
}

#[test]
fn add_reports_cloudflare_rejections_and_writes_nothing() {
    let api = start_mock();
    let dir = gitops_checkout();
    let mut env = cf_env(&api);
    env[0].1 = "wrong-token".to_owned();

    let code = run_with(
        &[
            "add",
            "eu-west",
            "--public-ip",
            "198.51.100.4",
            "--public-host",
            "discordy-rtc-eu-west.voics.nl",
            "--gitops-dir",
            dir.path().to_str().unwrap(),
        ],
        &env,
    );

    assert_eq!(code, ExitCode::FAILURE);
    assert!(!dir.path().join("regions/eu-west").exists());
}

#[test]
fn add_rejects_invalid_arguments() {
    let api = start_mock();
    let dir = gitops_checkout();
    let gitops = dir.path().to_str().unwrap();
    let env = cf_env(&api);

    assert_eq!(
        run_with(
            &[
                "add",
                "Bad",
                "--public-ip",
                "198.51.100.4",
                "--public-host",
                "a.voics.nl",
                "--gitops-dir",
                gitops
            ],
            &env
        ),
        ExitCode::FAILURE
    );
    assert_eq!(
        run_with(
            &[
                "add",
                "eu-west",
                "--public-ip",
                "nope",
                "--public-host",
                "a.voics.nl",
                "--gitops-dir",
                gitops
            ],
            &env
        ),
        ExitCode::FAILURE
    );
    assert_eq!(
        run_with(
            &[
                "add",
                "eu-west",
                "--public-host",
                "a.voics.nl",
                "--gitops-dir",
                gitops
            ],
            &env
        ),
        ExitCode::from(2)
    );
    assert!(api.state.lock().unwrap().requests.is_empty());
}

#[test]
fn drain_and_undrain_edit_one_literal() {
    let dir = gitops_checkout();
    let gitops = dir.path().to_str().unwrap();

    assert_eq!(
        run(
            &["drain", "--region", "nl-home", "--gitops-dir", gitops],
            &[]
        ),
        ExitCode::SUCCESS
    );
    let drained = read(dir.path(), "regions/nl-home/kustomization.yaml");
    assert!(drained.contains("      - VOICE_DRAIN_ON_START=true\ngeneratorOptions:"));
    assert_eq!(
        drained.replace("      - VOICE_DRAIN_ON_START=true\n", ""),
        NL_HOME
    );

    assert_eq!(
        run(
            &["undrain", "--region", "nl-home", "--gitops-dir", gitops],
            &[]
        ),
        ExitCode::SUCCESS
    );
    assert_eq!(
        read(dir.path(), "regions/nl-home/kustomization.yaml"),
        NL_HOME
    );

    assert_eq!(
        run(&["drain", "--region", "mars", "--gitops-dir", gitops], &[]),
        ExitCode::FAILURE
    );
}

#[test]
fn list_works_with_and_without_cloudflare() {
    let api = start_mock();
    let dir = gitops_checkout();
    let gitops = dir.path().to_str().unwrap();

    assert_eq!(
        run(&["list", "--gitops-dir", gitops], &[]),
        ExitCode::SUCCESS
    );
    assert!(api.state.lock().unwrap().requests.is_empty());

    assert_eq!(
        run_with(&["list", "--gitops-dir", gitops], &cf_env(&api)),
        ExitCode::SUCCESS
    );
    // nl-home carries Flux substitutions in its host, so there is nothing to look up.
    assert!(api.state.lock().unwrap().requests.is_empty());

    let env = cf_env(&api);
    let add = [
        "add",
        "eu-west",
        "--public-ip",
        "198.51.100.4",
        "--public-host",
        "discordy-rtc-eu-west.voics.nl",
        "--gitops-dir",
        gitops,
    ];
    assert_eq!(run_with(&add, &env), ExitCode::SUCCESS);
    api.state.lock().unwrap().requests.clear();
    assert_eq!(
        run_with(&["list", "--gitops-dir", gitops], &env),
        ExitCode::SUCCESS
    );
    let requests = api.state.lock().unwrap().requests.clone();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].1.contains("name=discordy-rtc-eu-west.voics.nl"));
}

#[test]
fn the_gitops_dir_is_required_and_must_be_a_checkout() {
    assert_eq!(run(&["list"], &[]), ExitCode::FAILURE);
    let empty = tempfile::tempdir().unwrap();
    assert_eq!(
        run(
            &["list", "--gitops-dir", empty.path().to_str().unwrap()],
            &[]
        ),
        ExitCode::FAILURE
    );
}

/// Records the requests a fake gateway receives and answers `status`.
fn start_gateway(status: u16) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let shared = Arc::clone(&seen);
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            shared
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..n]).into_owned());
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        }
    });
    (base, seen)
}

#[test]
fn node_drain_and_undrain_call_the_gateway_with_the_internal_token() {
    let (base, seen) = start_gateway(204);
    let env = [("INTERNAL_SERVICE_TOKEN", "internal-secret")];

    assert_eq!(
        run(&["drain", "eu-west-1", "--gateway-url", &base], &env),
        ExitCode::SUCCESS
    );
    assert_eq!(
        run(&["undrain", "eu-west-1", "--gateway-url", &base], &env),
        ExitCode::SUCCESS
    );

    let requests = seen.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("POST /internal/v1/voice/nodes/eu-west-1/drain HTTP/1.1"));
    assert!(requests[1].starts_with("POST /internal/v1/voice/nodes/eu-west-1/undrain HTTP/1.1"));
    assert!(
        requests[0]
            .to_ascii_lowercase()
            .contains("x-internal-token: internal-secret")
    );
}

#[test]
fn node_drain_reports_gateway_errors_and_missing_tokens() {
    let (base, seen) = start_gateway(401);
    assert_eq!(
        run(
            &["drain", "eu-west-1", "--gateway-url", &base],
            &[("INTERNAL_SERVICE_TOKEN", "wrong")]
        ),
        ExitCode::FAILURE
    );
    assert_eq!(
        run(&["drain", "eu-west-1", "--gateway-url", &base], &[]),
        ExitCode::FAILURE
    );
    assert_eq!(seen.lock().unwrap().len(), 1, "no request without a token");

    let env = [("INTERNAL_SERVICE_TOKEN", "t")];
    assert_eq!(
        run(&["drain", "../etc", "--gateway-url", &base], &env),
        ExitCode::FAILURE
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "invalid ids never reach the gateway"
    );
}

#[test]
fn drain_needs_exactly_one_target() {
    assert_eq!(run(&["drain"], &[]), ExitCode::from(2));
    assert_eq!(
        run(&["drain", "node-1", "--region", "eu-west"], &[]),
        ExitCode::from(2)
    );
}

#[test]
fn plan_node_is_offline_read_only_and_checks_collisions() {
    let dir = gitops_checkout();
    let gitops = dir.path().to_str().unwrap();
    let before = read(dir.path(), "regions/nl-home/kustomization.yaml");
    let root = read(dir.path(), "regions/kustomization.yaml");
    let args = [
        "plan-node",
        "nl-home",
        "--node-id",
        "nl-home-2",
        "--public-ip",
        "198.51.100.4",
        "--public-host",
        "rtc-2.voics.nl",
        "--udp-port",
        "50001",
        "--k8s-node",
        "node-2",
        "--gitops-dir",
        gitops,
    ];
    let api = start_mock();
    assert_eq!(run_with(&args, &cf_env(&api)), ExitCode::SUCCESS);
    assert!(api.state.lock().unwrap().requests.is_empty());
    assert_eq!(
        read(dir.path(), "regions/nl-home/kustomization.yaml"),
        before
    );
    assert_eq!(read(dir.path(), "regions/kustomization.yaml"), root);
    assert!(!dir.path().join("regions/nl-home-2").exists());
    let mut duplicate = args;
    duplicate[3] = "nl-home-gen-serv";
    assert_eq!(run(&duplicate, &[]), ExitCode::FAILURE);
    let mut missing = args;
    missing[1] = "unknown";
    assert_eq!(run(&missing, &[]), ExitCode::FAILURE);
    let mut invalid_port = args;
    invalid_port[9] = "0";
    assert_eq!(run(&invalid_port, &[]), ExitCode::from(2));
    fs::create_dir(dir.path().join("regions/broken")).unwrap();
    assert_eq!(run(&args, &[]), ExitCode::FAILURE);
}
