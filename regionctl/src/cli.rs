//! Command line: parses arguments, wires adapters and prints results.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::application::add_region::{AddRegion, add_region};
use crate::application::error::AppError;
use crate::application::list_regions::{DnsStatus, list_regions};
use crate::application::ports::{DnsChange, DnsGateway};
use crate::application::set_drain::{DrainChange, set_region_drain};
use crate::infrastructure::cloudflare::{Cloudflare, Credentials, DEFAULT_API_BASE};
use crate::infrastructure::fs_store::FsRegionStore;

/// Voice region control.
#[derive(Debug, Parser)]
#[command(name = "regionctl", version, about)]
struct Cli {
    /// Checkout of the `discordy-voics/gitops` repository.
    #[arg(long, global = true, env = "REGIONCTL_GITOPS_DIR")]
    gitops_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
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
    /// List regions and the state of their DNS records.
    List,
    /// Start a region's nodes in drain mode: no new voice sessions.
    Drain {
        /// Region name.
        name: String,
    },
    /// Let a drained region accept new voice sessions again.
    Undrain {
        /// Region name.
        name: String,
    },
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

fn execute(cli: Cli, env: &impl Fn(&str) -> Option<String>) -> Result<(), String> {
    let gitops_dir = cli
        .gitops_dir
        .ok_or("--gitops-dir (or REGIONCTL_GITOPS_DIR) is required")?;
    let store = FsRegionStore::open(&gitops_dir).map_err(|e: AppError| e.to_string())?;

    match cli.command {
        Command::Add {
            name,
            public_ip,
            public_host,
            dns_only,
        } => {
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
                "region {name} added\n  host      {host} -> {ip} (DNS record {dns_note}, {mode})\n  node id   {node}\n  worker id {worker}\n  overlay   {dir}/regions/{name}/kustomization.yaml\n\nNext: review the diff, commit it to gitops and let Flux reconcile; the node then registers in etcd.",
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
                dir = gitops_dir.display(),
            );
            Ok(())
        }
        Command::List => {
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
        Command::Drain { name } => drain(&store, &name, true),
        Command::Undrain { name } => drain(&store, &name, false),
    }
}

fn drain(store: &FsRegionStore, name: &str, drained: bool) -> Result<(), String> {
    let change = set_region_drain(store, name, drained).map_err(|e| e.to_string())?;
    let state = if drained { "drained" } else { "active" };
    match change {
        DrainChange::Changed => println!(
            "region {name} is now {state} in gitops; commit it and let Flux reconcile (restart the voice pod to apply it to a running node)"
        ),
        DrainChange::AlreadyInState => println!("region {name} is already {state}"),
    }
    Ok(())
}
