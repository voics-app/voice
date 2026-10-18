//! Use case: list the regions in the gitops checkout with the state of their DNS record.

use super::error::AppError;
use super::ports::{DnsGateway, RegionStore};
use crate::domain::region::{Region, parse_overlay};

/// What Cloudflare says about a region host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsStatus {
    /// Cloudflare credentials were not provided, so DNS was not checked.
    Unchecked,
    /// No A record exists.
    Missing,
    /// The record exists.
    Present {
        /// Address it points to.
        ip: String,
        /// Whether the host is proxied.
        proxied: bool,
    },
    /// The lookup failed.
    Failed(String),
}

/// One row of the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegionRow {
    /// The region.
    pub region: Region,
    /// Its DNS state.
    pub dns: DnsStatus,
}

/// Lists regions; `dns` is `None` when Cloudflare is not configured. Overlays that cannot be
/// parsed are reported as errors rather than skipped.
pub fn list_regions(
    store: &dyn RegionStore,
    dns: Option<&dyn DnsGateway>,
) -> Result<Vec<RegionRow>, AppError> {
    let mut rows = Vec::new();
    for name in store.names()? {
        let Some(overlay) = store.read_overlay(&name)? else {
            continue;
        };
        let region = parse_overlay(&name, &overlay)?;
        let status = match dns {
            None => DnsStatus::Unchecked,
            Some(gateway) => match gateway.find_a_record(&region.public_host) {
                Ok(None) => DnsStatus::Missing,
                Ok(Some(record)) => DnsStatus::Present {
                    ip: record.ip,
                    proxied: record.proxied,
                },
                Err(err) => DnsStatus::Failed(err.to_string()),
            },
        };
        rows.push(RegionRow {
            region,
            dns: status,
        });
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::add_region::fakes::{MemoryDns, MemoryStore};
    use crate::application::ports::DnsRecord;

    fn overlay(name: &str, host: &str, worker: u16) -> String {
        format!(
            "    literals:\n      - VOICE_REGION={name}\n      - VOICE_NODE_ID={name}-1\n      - VOICE_PUBLIC_WS_URL=wss://{host}\n      - VOICE_PUBLIC_IP=1.1.1.1\n      - SNOWFLAKE_WORKER_ID={worker}\n"
        )
    }

    #[test]
    fn reports_dns_state_per_region() {
        let store = MemoryStore::default();
        store
            .overlays
            .borrow_mut()
            .insert("a-one".into(), overlay("a-one", "a.voics.nl", 192));
        store
            .overlays
            .borrow_mut()
            .insert("b-two".into(), overlay("b-two", "b.voics.nl", 193));
        let dns = MemoryDns::default();
        dns.records.borrow_mut().insert(
            "a.voics.nl".into(),
            DnsRecord {
                ip: "1.1.1.1".into(),
                proxied: true,
            },
        );

        let rows = list_regions(&store, Some(&dns)).unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].dns,
            DnsStatus::Present {
                ip: "1.1.1.1".into(),
                proxied: true
            }
        );
        assert_eq!(rows[1].dns, DnsStatus::Missing);
    }

    #[test]
    fn without_cloudflare_dns_is_unchecked() {
        let store = MemoryStore::default();
        store
            .overlays
            .borrow_mut()
            .insert("a-one".into(), overlay("a-one", "a.voics.nl", 192));
        assert_eq!(
            list_regions(&store, None).unwrap()[0].dns,
            DnsStatus::Unchecked
        );
    }

    #[test]
    fn unparsable_overlays_are_errors() {
        let store = MemoryStore::default();
        store
            .overlays
            .borrow_mut()
            .insert("broken".into(), "resources: []\n".into());
        assert!(list_regions(&store, None).is_err());
    }
}
