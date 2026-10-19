//! Cloudflare DNS over the v4 REST API.

use std::net::Ipv4Addr;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::application::error::AppError;
use crate::application::ports::{DnsChange, DnsGateway, DnsRecord};

/// Production API base.
pub const DEFAULT_API_BASE: &str = "https://api.cloudflare.com/client/v4";
const RECORD_COMMENT: &str = "managed by regionctl";

/// Credentials from `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ZONE_ID`.
pub struct Credentials {
    /// API token with DNS edit permission on the zone.
    pub token: String,
    /// Zone id of the domain.
    pub zone_id: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("token", &"<redacted>")
            .field("zone_id", &self.zone_id)
            .finish()
    }
}

impl Credentials {
    /// Reads both variables; `None` when either is unset or empty.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let get = |name: &str| lookup(name).filter(|v| !v.trim().is_empty());
        Some(Self {
            token: get("CLOUDFLARE_API_TOKEN")?,
            zone_id: get("CLOUDFLARE_ZONE_ID")?,
        })
    }
}

/// Cloudflare-backed [`DnsGateway`].
pub struct Cloudflare {
    agent: ureq::Agent,
    base: String,
    credentials: Credentials,
}

impl std::fmt::Debug for Cloudflare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cloudflare")
            .field("base", &self.base)
            .field("credentials", &self.credentials)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct ApiError {
    code: i64,
    message: String,
}

#[derive(Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    errors: Vec<ApiError>,
    result: Option<T>,
}

#[derive(Deserialize)]
struct Record {
    id: String,
    content: String,
    #[serde(default)]
    proxied: bool,
}

fn dns_error(message: impl std::fmt::Display) -> AppError {
    AppError::Dns(message.to_string())
}

impl Cloudflare {
    /// Creates a client against `base` (the production API unless testing).
    pub fn new(base: impl Into<String>, credentials: Credentials) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into();
        Self {
            agent,
            base: base.into().trim_end_matches('/').to_owned(),
            credentials,
        }
    }

    fn records_url(&self) -> String {
        format!(
            "{}/zones/{}/dns_records",
            self.base, self.credentials.zone_id
        )
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.credentials.token)
    }

    fn unwrap<T>(mut response: ureq::http::Response<ureq::Body>) -> Result<T, AppError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let status = response.status();
        let envelope: Envelope<T> = response
            .body_mut()
            .read_json()
            .map_err(|e| dns_error(format!("unreadable response (HTTP {status}): {e}")))?;
        if !envelope.success {
            let detail = envelope
                .errors
                .iter()
                .map(|e| format!("{} (code {})", e.message, e.code))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(dns_error(format!("HTTP {status}: {detail}")));
        }
        envelope
            .result
            .ok_or_else(|| dns_error("response carried no result"))
    }

    fn records(&self, host: &str) -> Result<Vec<Record>, AppError> {
        let response = self
            .agent
            .get(&self.records_url())
            .query("type", "A")
            .query("name", host)
            .header("Authorization", self.bearer())
            .call()
            .map_err(dns_error)?;
        Self::unwrap(response)
    }
}

impl DnsGateway for Cloudflare {
    fn find_a_record(&self, host: &str) -> Result<Option<DnsRecord>, AppError> {
        Ok(self.records(host)?.into_iter().next().map(|r| DnsRecord {
            ip: r.content,
            proxied: r.proxied,
        }))
    }

    fn upsert_a_record(
        &self,
        host: &str,
        ip: Ipv4Addr,
        proxied: bool,
    ) -> Result<DnsChange, AppError> {
        let body = json!({
            "type": "A",
            "name": host,
            "content": ip.to_string(),
            "ttl": 1,
            "proxied": proxied,
            "comment": RECORD_COMMENT,
        });
        match self.records(host)?.into_iter().next() {
            None => {
                let response = self
                    .agent
                    .post(&self.records_url())
                    .header("Authorization", self.bearer())
                    .send_json(&body)
                    .map_err(dns_error)?;
                Self::unwrap::<serde_json::Value>(response)?;
                Ok(DnsChange::Created)
            }
            Some(existing) if existing.content == ip.to_string() && existing.proxied == proxied => {
                Ok(DnsChange::Unchanged)
            }
            Some(existing) => {
                let response = self
                    .agent
                    .put(&format!("{}/{}", self.records_url(), existing.id))
                    .header("Authorization", self.bearer())
                    .send_json(&body)
                    .map_err(dns_error)?;
                Self::unwrap::<serde_json::Value>(response)?;
                Ok(DnsChange::Updated)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_need_both_variables() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert!(Credentials::from_lookup(env(&[])).is_none());
        assert!(Credentials::from_lookup(env(&[("CLOUDFLARE_API_TOKEN", "t")])).is_none());
        assert!(
            Credentials::from_lookup(env(&[
                ("CLOUDFLARE_API_TOKEN", "t"),
                ("CLOUDFLARE_ZONE_ID", " ")
            ]))
            .is_none()
        );
        let both = Credentials::from_lookup(env(&[
            ("CLOUDFLARE_API_TOKEN", "t"),
            ("CLOUDFLARE_ZONE_ID", "z"),
        ]))
        .unwrap();
        assert_eq!(both.zone_id, "z");
    }

    #[test]
    fn debug_output_never_contains_the_token() {
        let credentials = Credentials {
            token: "super-secret".into(),
            zone_id: "zone".into(),
        };
        assert!(!format!("{credentials:?}").contains("super-secret"));
        assert!(
            !format!("{:?}", Cloudflare::new("http://x", credentials)).contains("super-secret")
        );
    }
}
