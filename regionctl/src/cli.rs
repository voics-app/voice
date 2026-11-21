//! Command line: parses arguments, wires adapters and prints results.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::application::add_region::{AddRegion, add_region};
use crate::application::drain_node::set_node_drain;
use crate::application::list_regions::{DnsStatus, list_regions};
use crate::application::ports::{DnsChange, DnsGateway, RegionStore};
use crate::application::set_drain::{DrainChange, set_region_drain};
use crate::domain::region::{
    NodeId, RegionName, next_worker_id, parse_overlay, parse_public_host, parse_public_ip,
};
use crate::infrastructure::cloudflare::{Cloudflare, Credentials, DEFAULT_API_BASE};
use crate::infrastructure::fs_store::FsRegionStore;
use crate::infrastructure::gateway::GatewayClient;

/// Voice region control.
#[derive(Debug, Parser)]
#[command(name = "regionctl", version, about)]
struct Cli {
    /// Checkout of the `discordy-voics/gitops` repository (add, list, drain --region).
    #[arg(long, global = true, env = "REGIONCTL_GITOPS_DIR")]
    gitops_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

/// What to drain: a live node through the gateway, or a whole region in gitops.
#[derive(Debug, Args)]
#[command(group = clap::ArgGroup::new("target").required(true).multiple(false))]
struct DrainTarget {
    /// Voice node id: drains the running node through the gateway (needs INTERNAL_SERVICE_TOKEN).
    #[arg(group = "target")]
    node_id: Option<String>,
    /// Region name: records the drain in gitops so the region's nodes start draining.
    #[arg(long, group = "target")]
    region: Option<String>,
    /// Gateway internal API.
    #[arg(
        long,
        env = "GATEWAY_INTERNAL_URL",
        default_value = "http://localhost:4000"
    )]
    gateway_url: String,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Scaffold a region overlay and create its Cloudflare DNS record.
    ///
    /// Needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ZONE_ID.
    Add {
        /// Region name, e.g. eu-west.
        name: String,
        /// Public IPv4 address of the region (UDP 50000 must reach it).
        #[arg(long)]
        public_ip: String,
        /// Public hostname of the voice WebSocket, e.g. discordy-rtc-eu-west.voics.nl.
        #[arg(long)]
        public_host: String,
        /// Create the record DNS-only instead of proxied through Cloudflare.
        #[arg(long)]
        dns_only: bool,
    },
    /// Preview the manual work for another node in an existing region (offline, read-only).
    PlanNode {
        /// Existing logical region.
        region: String,
        /// Unique voice node ID, also used as the proposed overlay directory and name suffix.
        #[arg(long)]
        node_id: String,
        /// Public IPv4 address advertised by this node.
        #[arg(long)]
        public_ip: String,
        /// Distinct public WebSocket hostname for this node.
        #[arg(long)]
        public_host: String,
        /// Public and local UDP media port.
        #[arg(long, value_parser = clap::value_parser!(u16).range(1..))]
        udp_port: u16,
        /// Dedicated Kubernetes node name (kubernetes.io/hostname).
        #[arg(long)]
        k8s_node: String,
    },
    /// List regions and the state of their DNS records.
    List,
    /// Stop a voice node (or a whole region) from accepting new sessions.
    Drain(DrainTarget),
    /// Let a drained voice node (or region) accept new sessions again.
    Undrain(DrainTarget),
}

/// Runs the CLI. `env` looks up environment variables.
pub fn run<I, T>(args: I, env: impl Fn(&str) -> Option<String>) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => {
            let _ = err.print();
            return ExitCode::from(if err.use_stderr() { 2 } else { 0 });
        }
    };
    match execute(cli, &env) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn cloudflare(env: &impl Fn(&str) -> Option<String>) -> Option<Cloudflare> {
    let credentials = Credentials::from_lookup(env)?;
    let base = env("CLOUDFLARE_API_BASE").unwrap_or_else(|| DEFAULT_API_BASE.to_owned());
    Some(Cloudflare::new(base, credentials))
}

fn open_store(gitops_dir: Option<&PathBuf>) -> Result<FsRegionStore, String> {
    let dir = gitops_dir.ok_or("--gitops-dir (or REGIONCTL_GITOPS_DIR) is required")?;
    FsRegionStore::open(dir).map_err(|e| e.to_string())
}

fn execute(cli: Cli, env: &impl Fn(&str) -> Option<String>) -> Result<(), String> {
    match cli.command {
        Command::Add {
            name,
            public_ip,
            public_host,
            dns_only,
        } => {
            let store = open_store(cli.gitops_dir.as_ref())?;
            let dns = cloudflare(env).ok_or(
                "CLOUDFLARE_API_TOKEN and CLOUDFLARE_ZONE_ID must both be set to create the DNS record",
            )?;
            let request = AddRegion {
                name,
                public_ip,
                public_host,
                proxied: !dns_only,
            };
            let added = add_region(&store, &dns, &request).map_err(|e| e.to_string())?;
            let dns_note = match added.dns {
                DnsChange::Created => "created",
                DnsChange::Updated => "updated",
                DnsChange::Unchanged => "already up to date",
            };
            println!(
                "region {name} added\n  host      {host} -> {ip} (DNS record {dns_note}, {mode})\n  node id   {node}\n  worker id {worker}\n  overlay   regions/{name}/kustomization.yaml\n\nNext: review the diff, commit it to gitops and let Flux reconcile; the node then registers in etcd.",
                name = added.region.name,
                host = added.region.public_host,
                ip = added.region.public_ip,
                mode = if request.proxied {
                    "proxied"
                } else {
                    "DNS only"
                },
                node = added.region.node_id,
                worker = added.region.worker_id,
            );
            Ok(())
        }
        Command::PlanNode {
            region,
            node_id,
            public_ip,
            public_host,
            udp_port,
            k8s_node,
        } => {
            let store = open_store(cli.gitops_dir.as_ref())?;
            plan_node(
                &store,
                &region,
                &node_id,
                &public_ip,
                &public_host,
                udp_port,
                &k8s_node,
            )
        }
        Command::List => {
            let store = open_store(cli.gitops_dir.as_ref())?;
            let dns = cloudflare(env);
            let rows = list_regions(&store, dns.as_ref().map(|d| d as &dyn DnsGateway))
                .map_err(|e| e.to_string())?;
            if dns.is_none() {
                eprintln!("note: CLOUDFLARE_API_TOKEN/CLOUDFLARE_ZONE_ID not set, DNS not checked");
            }
            println!(
                "{:<14} {:<34} {:<16} {:<8} {:<8} DNS",
                "REGION", "HOST", "PUBLIC IP", "WORKER", "STATE"
            );
            for row in rows {
                let dns_text = match row.dns {
                    DnsStatus::Unchecked => "unchecked".to_owned(),
                    DnsStatus::Missing => "missing".to_owned(),
                    DnsStatus::Present { ip, proxied } => {
                        format!("{ip} ({})", if proxied { "proxied" } else { "DNS only" })
                    }
                    DnsStatus::Failed(reason) => format!("lookup failed: {reason}"),
                };
                println!(
                    "{:<14} {:<34} {:<16} {:<8} {:<8} {}",
                    row.region.name,
                    row.region.public_host,
                    row.region.public_ip,
                    row.region.worker_id,
                    if row.region.drained {
                        "drained"
                    } else {
                        "active"
                    },
                    dns_text
                );
            }
            Ok(())
        }
        Command::Drain(target) => drain(target, true, &cli.gitops_dir, env),
        Command::Undrain(target) => drain(target, false, &cli.gitops_dir, env),
    }
}

fn drain(
    target: DrainTarget,
    drained: bool,
    gitops_dir: &Option<PathBuf>,
    env: &impl Fn(&str) -> Option<String>,
) -> Result<(), String> {
    let state = if drained { "drained" } else { "active" };
    if let Some(region) = target.region {
        let store = open_store(gitops_dir.as_ref())?;
        let change = set_region_drain(&store, &region, drained).map_err(|e| e.to_string())?;
        match change {
            DrainChange::Changed => println!(
                "region {region} is now {state} in gitops; commit it and let Flux reconcile (restart the voice pod to apply it to a running node)"
            ),
            DrainChange::AlreadyInState => println!("region {region} is already {state}"),
        }
        return Ok(());
    }

    let node_id = target.node_id.ok_or("a node id or --region is required")?;
    let token = env("INTERNAL_SERVICE_TOKEN")
        .filter(|t| !t.is_empty())
        .ok_or("INTERNAL_SERVICE_TOKEN must be set to call the gateway")?;
    let gateway = GatewayClient::new(target.gateway_url, token);
    set_node_drain(&gateway, &node_id, drained).map_err(|e| e.to_string())?;
    println!(
        "node {node_id} is now {state}; confirm with `etcdctl get /discordy/voice/nodes/<region>/{node_id}`"
    );
    Ok(())
}

// This deliberately prints a checklist, not a manifest: the existing region overlay cannot be
// copied blindly because its selectors and resource names belong to the first node.
fn plan_node(
    store: &dyn RegionStore,
    region: &str,
    node_id: &str,
    ip: &str,
    host: &str,
    port: u16,
    k8s_node: &str,
) -> Result<(), String> {
    RegionName::parse(region).map_err(|e| e.to_string())?;
    NodeId::parse(node_id).map_err(|e| e.to_string())?;
    parse_public_ip(ip).map_err(|e| e.to_string())?;
    parse_public_host(host).map_err(|e| e.to_string())?;
    // Kubernetes hostname labels are DNS subdomains; reject unsafe/unusable input.
    if k8s_node.len() > 253 || k8s_node.split('.').any(|part| NodeId::parse(part).is_err()) {
        return Err("invalid Kubernetes node hostname".into());
    }
    let names = store.names().map_err(|e| e.to_string())?;
    if !names.iter().any(|n| n == region) {
        return Err(format!("region `{region}` does not exist"));
    }
    if store.entry_exists(node_id).map_err(|e| e.to_string())? {
        return Err(format!("overlay `{node_id}` already exists"));
    }
    let mut used = Vec::new();
    for name in names {
        let text = store
            .read_overlay(&name)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                format!("overlay `{name}` has no kustomization.yaml; cannot plan safely")
            })?;
        let existing = parse_overlay(&name, &text)
            .map_err(|e| format!("cannot read overlay `{name}`: {e}"))?;
        if existing.node_id == node_id {
            return Err(format!("node ID `{node_id}` already used by `{name}`"));
        }
        if existing.public_host == host {
            return Err(format!("host `{host}` already used by `{name}`"));
        }
        used.push(existing.worker_id);
    }
    let worker = next_worker_id(&used).map_err(|e| e.to_string())?;
    println!(
        "Offline preview only: no DNS, files, or infrastructure changed.\n\
Proposed overlay: regions/{node_id}/kustomization.yaml (register in regions/kustomization.yaml after review)\n\
  VOICE_REGION={region}\n  VOICE_NODE_ID={node_id}\n  VOICE_PUBLIC_WS_URL=wss://{host}\n  VOICE_PUBLIC_IP={ip}\n  SFU_PUBLIC_IP={ip}\n  SNOWFLAKE_WORKER_ID={worker}\n  VOICE_UDP_PORT={port}\n  SFU_UDP_BIND=0.0.0.0:{port}\n\
Manual preparation:\n\
  - Base the new overlay on ../_template, with nameSuffix: -{node_id}; do not copy the existing region overlay unchanged.\n\
  - Add a unique node label with includeSelectors: true, in addition to the logical region label, so each Deployment and Service selects only its own pod.\n\
  - Pin the Deployment to kubernetes.io/hostname={k8s_node} using nodeSelector. Use a dedicated node: hostNetwork also binds TCP 4100 and 127.0.0.1:4101.\n\
  - Set the SFU containerPort and hostPort to {port}/UDP; forward public {ip}:{port}/UDP to that node and port.\n\
  - Set the Ingress host to {host}; prepare a separate DNS A record to {ip} and confirm the ingress routes to the new node's Service.\n\
  - Review resource names, selectors, port availability, NAT, DNS and rendered kustomize output before committing. This offline scan cannot resolve Flux substitutions or inspect DNS, cluster state, other manifest layers, or concurrent plans; verify uniqueness there and recheck the worker ID before applying."
    );
    Ok(())
}
