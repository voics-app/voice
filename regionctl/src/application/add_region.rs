//! Use case: add a voice region (DNS record, overlay, registration in the regions layer).

use std::net::Ipv4Addr;

use super::error::AppError;
use super::ports::{DnsChange, DnsGateway, RegionStore};
use crate::domain::region::{
    Region, RegionName, add_resource, next_worker_id, parse_overlay, parse_public_host,
    parse_public_ip, render_overlay,
};

/// Input of the use case, still unvalidated.
#[derive(Debug, Clone)]
pub struct AddRegion {
    /// Region name.
    pub name: String,
    /// Public IP of the region SFU.
    pub public_ip: String,
    /// Public host of the voice WebSocket.
    pub public_host: String,
    /// Proxy the host through Cloudflare (WebSocket over 443) instead of DNS only.
    pub proxied: bool,
}

/// What was done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    /// The region as written.
    pub region: Region,
    /// What happened to the DNS record.
    pub dns: DnsChange,
}

/// Adds the region. DNS goes first because it is idempotent: a failure after it is repaired by
/// running the command again.
pub fn add_region(
    store: &dyn RegionStore,
    dns: &dyn DnsGateway,
    request: &AddRegion,
) -> Result<Added, AppError> {
    let name = RegionName::parse(&request.name)?;
    let ip: Ipv4Addr = parse_public_ip(&request.public_ip)?;
    let host = parse_public_host(&request.public_host)?;
    if store.read_overlay(name.as_str())?.is_some() {
        return Err(AppError::AlreadyExists(name.to_string()));
    }

    let worker_id = next_worker_id(&used_worker_ids(store)?)?;
    let region = Region {
        name: name.to_string(),
        node_id: format!("{name}-1"),
        public_host: host.clone(),
        public_ip: ip.to_string(),
        worker_id,
        drained: false,
    };

    let change = dns.upsert_a_record(&host, ip, request.proxied)?;
    store.write_overlay(name.as_str(), &render_overlay(&region))?;
    let root = store.read_root()?;
    store.write_root(&add_resource(&root, name.as_str()))?;
    Ok(Added {
        region,
        dns: change,
    })
}

fn used_worker_ids(store: &dyn RegionStore) -> Result<Vec<u16>, AppError> {
    let mut used = Vec::new();
    for name in store.names()? {
        if let Some(overlay) = store.read_overlay(&name)?
            && let Ok(region) = parse_overlay(&name, &overlay)
        {
            used.push(region.worker_id);
        }
    }
    Ok(used)
}

#[cfg(test)]
pub(crate) mod fakes {
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    use super::*;
    use crate::application::ports::DnsRecord;

    #[derive(Default)]
    pub struct MemoryStore {
        pub overlays: RefCell<BTreeMap<String, String>>,
        pub root: RefCell<String>,
    }

    impl RegionStore for MemoryStore {
        fn names(&self) -> Result<Vec<String>, AppError> {
            Ok(self.overlays.borrow().keys().cloned().collect())
        }
        fn read_overlay(&self, name: &str) -> Result<Option<String>, AppError> {
            Ok(self.overlays.borrow().get(name).cloned())
        }
        fn write_overlay(&self, name: &str, content: &str) -> Result<(), AppError> {
            self.overlays
                .borrow_mut()
                .insert(name.to_owned(), content.to_owned());
            Ok(())
        }
        fn read_root(&self) -> Result<String, AppError> {
            Ok(self.root.borrow().clone())
        }
        fn write_root(&self, content: &str) -> Result<(), AppError> {
            *self.root.borrow_mut() = content.to_owned();
            Ok(())
        }
    }

    #[derive(Default)]
    pub struct MemoryDns {
        pub records: RefCell<BTreeMap<String, DnsRecord>>,
        pub fail: bool,
    }

    impl DnsGateway for MemoryDns {
        fn find_a_record(&self, host: &str) -> Result<Option<DnsRecord>, AppError> {
            Ok(self.records.borrow().get(host).cloned())
        }
        fn upsert_a_record(
            &self,
            host: &str,
            ip: Ipv4Addr,
            proxied: bool,
        ) -> Result<DnsChange, AppError> {
            if self.fail {
                return Err(AppError::Dns("boom".into()));
            }
            let record = DnsRecord {
                ip: ip.to_string(),
                proxied,
            };
            let previous = self
                .records
                .borrow_mut()
                .insert(host.to_owned(), record.clone());
            Ok(match previous {
                None => DnsChange::Created,
                Some(p) if p == record => DnsChange::Unchanged,
                Some(_) => DnsChange::Updated,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::{MemoryDns, MemoryStore};
    use super::*;

    fn request(name: &str) -> AddRegion {
        AddRegion {
            name: name.into(),
            public_ip: "198.51.100.4".into(),
            public_host: format!("discordy-rtc-{name}.voics.nl"),
            proxied: true,
        }
    }

    fn store_with_nl_home() -> MemoryStore {
        let store = MemoryStore::default();
        store.overlays.borrow_mut().insert(
            "nl-home".into(),
            "    literals:\n      - VOICE_REGION=nl-home\n      - VOICE_NODE_ID=n\n      - VOICE_PUBLIC_WS_URL=wss://h\n      - VOICE_PUBLIC_IP=1.1.1.1\n      - SNOWFLAKE_WORKER_ID=192\n".into(),
        );
        *store.root.borrow_mut() = "resources:\n  - common\n  - nl-home\n".into();
        store
    }

    #[test]
    fn writes_overlay_registers_it_and_creates_dns() {
        let store = store_with_nl_home();
        let dns = MemoryDns::default();

        let added = add_region(&store, &dns, &request("eu-west")).unwrap();

        assert_eq!(added.dns, DnsChange::Created);
        assert_eq!(
            added.region.worker_id, 193,
            "next free id after nl-home's 192"
        );
        assert_eq!(added.region.node_id, "eu-west-1");
        assert!(store.overlays.borrow()["eu-west"].contains("SNOWFLAKE_WORKER_ID=193"));
        assert_eq!(
            *store.root.borrow(),
            "resources:\n  - common\n  - nl-home\n  - eu-west\n"
        );
        let record = dns.records.borrow()["discordy-rtc-eu-west.voics.nl"].clone();
        assert_eq!((record.ip.as_str(), record.proxied), ("198.51.100.4", true));
    }

    #[test]
    fn refuses_an_existing_region_without_touching_dns() {
        let store = store_with_nl_home();
        let dns = MemoryDns::default();
        let result = add_region(&store, &dns, &request("nl-home"));
        assert_eq!(result, Err(AppError::AlreadyExists("nl-home".into())));
        assert!(dns.records.borrow().is_empty());
    }

    #[test]
    fn rejects_invalid_input_before_doing_anything() {
        let store = store_with_nl_home();
        let dns = MemoryDns::default();
        for bad in [
            AddRegion {
                name: "Bad Name".into(),
                ..request("x1")
            },
            AddRegion {
                public_ip: "nope".into(),
                ..request("eu-west")
            },
            AddRegion {
                public_host: "nodots".into(),
                ..request("eu-west")
            },
        ] {
            assert!(matches!(
                add_region(&store, &dns, &bad),
                Err(AppError::Region(_))
            ));
        }
        assert!(dns.records.borrow().is_empty());
        assert_eq!(store.overlays.borrow().len(), 1);
    }

    #[test]
    fn a_dns_failure_leaves_the_repository_untouched_and_the_command_repeatable() {
        let store = store_with_nl_home();
        let failing = MemoryDns {
            fail: true,
            ..MemoryDns::default()
        };
        assert!(matches!(
            add_region(&store, &failing, &request("eu-west")),
            Err(AppError::Dns(_))
        ));
        assert_eq!(store.overlays.borrow().len(), 1);

        assert!(add_region(&store, &MemoryDns::default(), &request("eu-west")).is_ok());
    }
}
