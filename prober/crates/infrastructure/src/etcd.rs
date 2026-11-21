//! etcd v3 JSON gateway; ownership writes always compare registration and observer leases.
use std::collections::HashMap;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use uuid::Uuid;

pub use probe_application::StoreError as Error;
use probe_domain::{Health, Node, Registration};

const NODES: &str = "/discordy/voice/nodes/";

#[derive(Debug, Clone)]
pub struct Etcd {
    endpoints: Vec<String>,
    client: reqwest::Client,
}

#[derive(Debug)]
pub struct Claim {
    pub lease: String,
    key: String,
    owner: String,
}

impl Etcd {
    pub fn new(endpoints: Vec<String>) -> Result<Self, Error> {
        if endpoints.is_empty()
            || endpoints.len() > 5
            || endpoints.iter().any(|endpoint| {
                !reqwest::Url::parse(endpoint).is_ok_and(|url| {
                    matches!(url.scheme(), "http" | "https")
                        && url.host_str().is_some()
                        && url.username().is_empty()
                        && url.password().is_none()
                        && url.query().is_none()
                        && url.fragment().is_none()
                        && matches!(url.path(), "" | "/")
                })
            })
        {
            return Err(Error::InvalidResponse);
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(750))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::Unavailable)?;
        Ok(Self { endpoints, client })
    }

    pub async fn post(&self, path: &str, payload: &Value) -> Result<Value, Error> {
        for endpoint in &self.endpoints {
            let response = self
                .client
                .post(format!("{}{}", endpoint.trim_end_matches('/'), path))
                .json(payload)
                .send()
                .await;
            let Ok(response) = response else {
                continue;
            };
            if !response.status().is_success() {
                continue;
            }
            if let Ok(value) = response.json().await {
                return Ok(value);
            }
        }
        Err(Error::Unavailable)
    }

    pub async fn registrations(&self) -> Result<Vec<Registration>, Error> {
        let response = self
            .post(
                "/v3/kv/range",
                &json!({"key": encode(NODES), "range_end": encode("/discordy/voice/nodes0")}),
            )
            .await?;
        let mut registrations = Vec::new();
        for kv in response["kvs"].as_array().into_iter().flatten() {
            let Some(registration) = parse_registration(kv) else {
                continue;
            };
            registrations.push(registration);
        }
        // A duplicated identity is not a target the observer can safely disambiguate.
        let mut counts = HashMap::new();
        for registration in &registrations {
            *counts.entry(registration.node.node_id.clone()).or_insert(0) += 1;
        }
        registrations.retain(|r| counts[&r.node.node_id] == 1);
        Ok(registrations)
    }

    pub async fn acquire(&self, registration: &Registration, ttl_s: u64) -> Result<Claim, Error> {
        let granted = self
            .post("/v3/lease/grant", &json!({"TTL": ttl_s.to_string()}))
            .await?;
        let lease = positive_decimal(&granted["ID"])
            .ok_or(Error::InvalidResponse)?
            .to_owned();
        let claim = Claim {
            lease,
            key: format!("/discordy/voice/probers/{}", registration.node.node_id),
            owner: Uuid::new_v4().to_string(),
        };
        let response = self.post("/v3/kv/txn", &json!({
            "compare": [lease_compare(&registration.key, &registration.lease), {"key": encode(&claim.key), "target": "VERSION", "version": "0", "result": "EQUAL"}],
            "success": [{"request_put": {"key": encode(&claim.key), "value": encode(&claim.owner), "lease": claim.lease}}]
        })).await;
        match response {
            Ok(value) if value["succeeded"] == true => Ok(claim),
            other => {
                let _ = self.revoke(&claim.lease).await;
                match other {
                    Err(error) => Err(error),
                    _ => Err(Error::OwnershipLost),
                }
            }
        }
    }

    pub async fn keep_alive(&self, claim: &Claim) -> Result<(), Error> {
        // keepalive is a stream. Read its first complete JSON result and drop the stream;
        // waiting for EOF would turn a valid renewal into an HTTP timeout.
        for endpoint in &self.endpoints {
            let response = self
                .client
                .post(format!(
                    "{}/v3/lease/keepalive",
                    endpoint.trim_end_matches('/')
                ))
                .json(&json!({"ID": claim.lease}))
                .send()
                .await;
            let Ok(mut response) = response else {
                continue;
            };
            if !response.status().is_success() {
                continue;
            }
            let mut body = Vec::new();
            while let Ok(Some(chunk)) = response.chunk().await {
                body.extend_from_slice(&chunk);
                if body.len() > 16_384 {
                    break;
                }
                if let Ok(value) = serde_json::from_slice::<Value>(&body) {
                    return if positive_decimal(&value["result"]["TTL"]).is_some() {
                        Ok(())
                    } else {
                        Err(Error::OwnershipLost)
                    };
                }
            }
        }
        Err(Error::Unavailable)
    }

    pub async fn publish(
        &self,
        registration: &Registration,
        claim: &Claim,
        health: &Health,
    ) -> Result<(), Error> {
        let mut comparisons = owner_comparisons(claim);
        comparisons.push(lease_compare(&registration.key, &registration.lease));
        let value = crate::wire::health(health).to_string();
        let response = self.post("/v3/kv/txn", &json!({"compare": comparisons,
            "success": [{"request_put": {"key": encode(&format!("/discordy/voice/health/{}", registration.node.node_id)),
                "value": encode(&value), "lease": claim.lease}}]})).await?;
        if response["succeeded"] == true {
            Ok(())
        } else {
            Err(Error::OwnershipLost)
        }
    }

    pub async fn release(&self, claim: &Claim) {
        // Revocation removes only keys attached to this claim's lease, never a successor.
        let _ = self.revoke(&claim.lease).await;
    }

    pub(crate) async fn revoke(&self, lease: &str) -> Result<(), Error> {
        self.post("/v3/lease/revoke", &json!({"ID": lease}))
            .await
            .map(|_| ())
    }

    pub async fn blacklist(
        &self,
        scope: &str,
        id: &str,
        reason: &str,
        operator: &str,
        now_ms: u64,
    ) -> Result<(), Error> {
        let key = blacklist_key(scope, id)?;
        if reason.is_empty()
            || reason.chars().count() > 512
            || operator.is_empty()
            || operator.chars().count() > 128
        {
            return Err(Error::InvalidResponse);
        }
        let value =
            json!({"reason": reason, "operator": operator, "created_at_ms": now_ms}).to_string();
        self.post(
            "/v3/kv/put",
            &json!({"key": encode(&key), "value": encode(&value)}),
        )
        .await
        .map(|_| ())
    }

    pub async fn clear_blacklist(&self, scope: &str, id: &str) -> Result<(), Error> {
        let key = blacklist_key(scope, id)?;
        self.post("/v3/kv/deleterange", &json!({"key": encode(&key)}))
            .await
            .map(|_| ())
    }
}

pub fn encode(value: &str) -> String {
    STANDARD.encode(value)
}

fn decode(value: &Value) -> Option<String> {
    String::from_utf8(STANDARD.decode(value.as_str()?).ok()?).ok()
}

fn positive_decimal(value: &Value) -> Option<&str> {
    let value = value.as_str()?;
    (value.parse::<i64>().is_ok_and(|n| n > 0)).then_some(value)
}

fn parse_registration(kv: &Value) -> Option<Registration> {
    let key = decode(&kv["key"])?;
    let raw: Value = serde_json::from_str(&decode(&kv["value"])?).ok()?;
    let node = Node {
        node_id: raw["node_id"].as_str()?.to_owned(),
        region: raw["region"].as_str()?.to_owned(),
        ws_url: raw["ws_url"].as_str()?.to_owned(),
        public_ip: raw["public_ip"].as_str()?.to_owned(),
        udp_port: raw["udp_port"].as_u64()?.try_into().ok()?,
    };
    let lease = positive_decimal(&kv["lease"])?.to_owned();
    if key != format!("{NODES}{}/{}", node.region, node.node_id)
        || !segment(&node.node_id)
        || !segment(&node.region)
        || node.udp_port == 0
        || node.public_ip.parse::<std::net::IpAddr>().is_err()
        || !reqwest::Url::parse(&node.ws_url).is_ok_and(|url| {
            matches!(url.scheme(), "ws" | "wss")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
        })
    {
        return None;
    }
    Some(Registration { key, lease, node })
}

fn segment(value: &str) -> bool {
    !value.is_empty() && !value.contains('/') && !value.chars().any(char::is_control)
}

fn owner_comparisons(claim: &Claim) -> Vec<Value> {
    vec![
        lease_compare(&claim.key, &claim.lease),
        json!({"key": encode(&claim.key), "target": "VALUE", "value": encode(&claim.owner), "result": "EQUAL"}),
    ]
}

fn lease_compare(key: &str, lease: &str) -> Value {
    json!({"key": encode(key), "target": "LEASE", "lease": lease, "result": "EQUAL"})
}

fn blacklist_key(scope: &str, id: &str) -> Result<String, Error> {
    if !matches!(scope, "nodes" | "regions")
        || id.is_empty()
        || id.contains('/')
        || id.chars().any(char::is_control)
    {
        return Err(Error::InvalidResponse);
    }
    Ok(format!("/discordy/voice/blacklist/{scope}/{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_requires_a_leased_unambiguous_path_and_usable_endpoints() {
        let valid = json!({"node_id": "node", "region": "region", "ws_url": "ws://127.0.0.1:4100/voice",
            "public_ip": "127.0.0.1", "udp_port": 50000});
        let kv = |value: &Value, lease: &str| {
            json!({"key": encode("/discordy/voice/nodes/region/node"),
            "value": encode(&value.to_string()), "lease": lease})
        };
        assert!(parse_registration(&kv(&valid, "123")).is_some());
        assert!(parse_registration(&kv(&valid, "0")).is_none());
        for (field, value) in [
            ("node_id", json!("other")),
            ("region", json!("region/nested")),
            ("public_ip", json!("not-an-address")),
            ("udp_port", json!(0)),
            ("ws_url", json!("https://127.0.0.1")),
            ("ws_url", json!("ws://user:secret@127.0.0.1")),
        ] {
            let mut invalid = valid.clone();
            invalid[field] = value;
            assert!(
                parse_registration(&kv(&invalid, "123")).is_none(),
                "accepted malformed {field}"
            );
        }
    }

    #[tokio::test]
    async fn blacklist_metadata_must_fit_the_contract_before_network_io() {
        let store = Etcd::new(vec!["http://127.0.0.1:1".into()]).unwrap();
        assert_eq!(
            store
                .blacklist("nodes", "node", &"x".repeat(513), "operator", 0)
                .await,
            Err(Error::InvalidResponse)
        );
        assert_eq!(
            store
                .blacklist("nodes", "node", "reason", &"x".repeat(129), 0)
                .await,
            Err(Error::InvalidResponse)
        );
    }
}
