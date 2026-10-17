//! Environment configuration (pinned interface P2), validated at startup.

use std::net::{IpAddr, SocketAddr};

/// Validated SFU configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// `SFU_UDP_BIND`.
    pub udp_bind: SocketAddr,
    /// `SFU_PUBLIC_IP`.
    pub public_ip: IpAddr,
    /// `SFU_CONTROL_BIND`.
    pub control_bind: SocketAddr,
    /// `SFU_HTTP_BIND`: liveness, readiness and metrics.
    pub http_bind: SocketAddr,
    /// `LOG_LEVEL`.
    pub log_level: String,
}

/// A configuration problem.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// A required variable is unset.
    #[error("{0} is required")]
    Missing(&'static str),
    /// A variable has an unparsable value.
    #[error("{name} is invalid: {value}")]
    Invalid {
        /// Variable name.
        name: &'static str,
        /// Offending value.
        value: String,
    },
}

impl Config {
    /// Reads the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads configuration through `lookup`.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let parse = |name: &'static str, default: Option<&str>| -> Result<String, ConfigError> {
            lookup(name)
                .filter(|v| !v.is_empty())
                .or_else(|| default.map(str::to_owned))
                .ok_or(ConfigError::Missing(name))
        };
        let addr = |name: &'static str, default: &str| -> Result<SocketAddr, ConfigError> {
            let value = parse(name, Some(default))?;
            value
                .parse()
                .map_err(|_| ConfigError::Invalid { name, value })
        };
        let public_ip = parse("SFU_PUBLIC_IP", None)?;
        Ok(Self {
            udp_bind: addr("SFU_UDP_BIND", "0.0.0.0:50000")?,
            public_ip: public_ip.parse().map_err(|_| ConfigError::Invalid {
                name: "SFU_PUBLIC_IP",
                value: public_ip,
            })?,
            control_bind: addr("SFU_CONTROL_BIND", "127.0.0.1:4101")?,
            http_bind: addr("SFU_HTTP_BIND", "0.0.0.0:4102")?,
            log_level: parse("LOG_LEVEL", Some("info"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn applies_defaults() {
        let config = Config::from_lookup(lookup(&[("SFU_PUBLIC_IP", "203.0.113.7")])).unwrap();
        assert_eq!(config.udp_bind.port(), 50_000);
        assert_eq!(config.control_bind.to_string(), "127.0.0.1:4101");
        assert_eq!(config.public_ip.to_string(), "203.0.113.7");
    }

    #[test]
    fn public_ip_is_required() {
        assert_eq!(
            Config::from_lookup(lookup(&[])),
            Err(ConfigError::Missing("SFU_PUBLIC_IP"))
        );
    }

    #[test]
    fn rejects_malformed_values() {
        let result = Config::from_lookup(lookup(&[
            ("SFU_PUBLIC_IP", "203.0.113.7"),
            ("SFU_UDP_BIND", "nope"),
        ]));
        assert!(matches!(
            result,
            Err(ConfigError::Invalid {
                name: "SFU_UDP_BIND",
                ..
            })
        ));
    }
}
