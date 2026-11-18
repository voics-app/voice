//! Probe configuration and independent, lease-fenced observation cycles.
use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Clone)]
pub struct Config {
    pub prober_id: String,
    pub endpoints: Vec<String>,
    pub probe_secret: String,
    pub server_id: String,
    pub channel_id: String,
    pub user_ids: [String; 2],
    pub interval: Duration,
    pub timeout: Duration,
    pub failures: u32,
    pub passes: u32,
    pub max_loss_pct: f64,
    pub max_rtt_ms: f64,
    pub http_bind: SocketAddr,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("prober_id", &self.prober_id)
            .field("interval", &self.interval)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Config {
    pub fn from_env(vars: &HashMap<String, String>) -> Result<Self, String> {
        let required = |key: &str| {
            vars.get(key)
                .filter(|v| !v.is_empty())
                .cloned()
                .ok_or_else(|| format!("{key} is required"))
        };
        let integer = |key: &str, default: u64| -> Result<u64, String> {
            match vars.get(key) {
                None => Ok(default),
                Some(value) => value
                    .parse()
                    .ok()
                    .filter(|v| *v > 0)
                    .ok_or_else(|| format!("{key} must be a positive integer")),
            }
        };
        let number = |key: &str, default: f64| -> Result<f64, String> {
            match vars.get(key) {
                None => Ok(default),
                Some(value) => value
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.0)
                    .ok_or_else(|| format!("{key} must be finite and nonnegative")),
            }
        };
        let probe_secret = required("VOICE_PROBE_TOKEN_SECRET")?;
        if probe_secret.len() < 32 {
            return Err("VOICE_PROBE_TOKEN_SECRET must be at least 32 bytes".into());
        }
        let prober_id = required("VOICE_PROBER_ID")?;
        if prober_id.chars().count() > 128 || prober_id.chars().any(char::is_control) {
            return Err("VOICE_PROBER_ID must contain at most 128 printable characters".into());
        }
        let server_id = required("VOICE_PROBE_SERVER_ID")?;
        let channel_id = required("VOICE_PROBE_CHANNEL_ID")?;
        let users = required("VOICE_PROBE_USER_IDS")?;
        let users: Vec<_> = users.split(',').map(str::trim).map(str::to_owned).collect();
        if !decimal(&server_id)
            || !decimal(&channel_id)
            || users.len() != 2
            || users[0] == users[1]
            || users.iter().any(|id| !decimal(id))
        {
            return Err(
                "probe server/channel/users must be decimal IDs with two distinct users".into(),
            );
        }
        let interval = Duration::from_millis(integer("VOICE_PROBE_INTERVAL_MS", 10_000)?);
        let timeout = Duration::from_millis(integer("VOICE_PROBE_TIMEOUT_MS", 4_000)?);
        if timeout < Duration::from_millis(1_500)
            || timeout > Duration::from_secs(10)
            || timeout >= interval
            || interval > Duration::from_secs(300)
        {
            return Err("probe timeout must be 1500..10000 ms and less than the interval; interval must be <=300000 ms".into());
        }
        let max_loss_pct = number("VOICE_PROBE_MAX_LOSS_PCT", 10.0)?;
        let max_rtt_ms = number("VOICE_PROBE_MAX_RTT_MS", 1_000.0)?;
        if max_loss_pct > 100.0 || max_rtt_ms == 0.0 || max_rtt_ms >= timeout.as_millis() as f64 {
            return Err(
                "probe loss must be <=100 and RTT must be positive and less than the timeout"
                    .into(),
            );
        }
        let endpoints = etcd_endpoints(vars)?;
        let http_bind = vars
            .get("VOICE_PROBE_HTTP_BIND")
            .map(String::as_str)
            .unwrap_or("127.0.0.1:4103")
            .parse()
            .map_err(|_| "VOICE_PROBE_HTTP_BIND must be an IP:port")?;
        let failures = integer("VOICE_PROBE_FAILURES", 2)?
            .try_into()
            .map_err(|_| "VOICE_PROBE_FAILURES is too large")?;
        let passes = integer("VOICE_PROBE_PASSES", 2)?
            .try_into()
            .map_err(|_| "VOICE_PROBE_PASSES is too large")?;
        Ok(Self {
            prober_id,
            endpoints,
            probe_secret,
            server_id,
            channel_id,
            user_ids: users.try_into().map_err(|_| "two probe users required")?,
            interval,
            timeout,
            failures,
            passes,
            max_loss_pct,
            max_rtt_ms,
            http_bind,
        })
    }

    pub fn lease_ttl_s(&self) -> u64 {
        (self.interval.as_millis() * 2 / 1000) as u64
    }
}

pub fn etcd_endpoints(vars: &HashMap<String, String>) -> Result<Vec<String>, String> {
    let raw = vars
        .get("ETCD_ENDPOINTS")
        .ok_or("ETCD_ENDPOINTS is required")?;
    let endpoints: Vec<_> = raw
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| {
            if v.contains("://") {
                v.to_owned()
            } else {
                format!("http://{v}")
            }
        })
        .collect();
    if endpoints.is_empty() || endpoints.len() > 5 {
        return Err("ETCD_ENDPOINTS must contain 1 to 5 endpoints".into());
    }
    for endpoint in &endpoints {
        let url =
            url::Url::parse(endpoint).map_err(|_| "ETCD_ENDPOINTS contains an invalid URL")?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
            || url.host_str().is_none()
        {
            return Err(
                "ETCD_ENDPOINTS requires HTTP(S) origins without embedded credentials".into(),
            );
        }
    }
    Ok(endpoints)
}

fn decimal(id: &str) -> bool {
    !id.starts_with('0')
        && id.parse::<u64>().is_ok_and(|id| id > 0)
        && id.bytes().all(|c| c.is_ascii_digit())
}
