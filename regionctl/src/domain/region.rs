//! A voice region and its gitops overlay (`regions/<name>/kustomization.yaml`).

use std::collections::BTreeMap;
use std::fmt;
use std::net::Ipv4Addr;

/// Voice nodes draw their Snowflake worker id from this range (pinned interface P5).
pub const WORKER_ID_RANGE: std::ops::RangeInclusive<u16> = 192..=255;

const DRAIN_LITERAL: &str = "VOICE_DRAIN_ON_START=true";
const RESERVED_NAMES: [&str; 4] = ["common", "dev", "template", "kustomization"];

/// Why a region operation was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegionError {
    /// The region name is not a valid DNS label fragment.
    #[error(
        "invalid region name `{0}`: use 2-24 lowercase letters, digits and hyphens, starting with a letter"
    )]
    InvalidName(String),
    /// The public IP is not an IPv4 address.
    #[error("invalid public IP `{0}`: expected an IPv4 address")]
    InvalidIp(String),
    /// The public host is not a hostname.
    #[error("invalid public host `{0}`: expected a hostname such as discordy-rtc-eu-west.voics.nl")]
    InvalidHost(String),
    /// The overlay lacks a literal this tool needs.
    #[error("overlay is missing `{0}`")]
    MissingLiteral(&'static str),
    /// All voice worker ids are taken.
    #[error("no free SNOWFLAKE_WORKER_ID left in 192-255")]
    NoWorkerId,
    /// The node id is not a valid DNS label.
    #[error("invalid node id `{0}`: use 1-63 lowercase letters, digits and hyphens")]
    InvalidNodeId(String),
    /// The overlay has no `literals:` block to edit.
    #[error("overlay has no configMapGenerator literals block")]
    NoLiteralsBlock,
}

/// A validated region name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegionName(String);

impl RegionName {
    /// Validates `value`.
    pub fn parse(value: &str) -> Result<Self, RegionError> {
        let valid = (2..=24).contains(&value.len())
            && value.starts_with(|c: char| c.is_ascii_lowercase())
            && !value.ends_with('-')
            && value
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !RESERVED_NAMES.contains(&value);
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(RegionError::InvalidName(value.to_owned()))
        }
    }

    /// The name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RegionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A validated voice node id: a DNS label, so it is safe inside a URL path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeId(String);

impl NodeId {
    /// Validates `value`.
    pub fn parse(value: &str) -> Result<Self, RegionError> {
        let valid = (1..=63).contains(&value.len())
            && !value.starts_with('-')
            && !value.ends_with('-')
            && value
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(RegionError::InvalidNodeId(value.to_owned()))
        }
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Validates a public IPv4 address.
pub fn parse_public_ip(value: &str) -> Result<Ipv4Addr, RegionError> {
    value
        .parse()
        .map_err(|_| RegionError::InvalidIp(value.to_owned()))
}

/// Validates a hostname (lowercase labels of letters, digits and hyphens, at least two labels).
pub fn parse_public_host(value: &str) -> Result<String, RegionError> {
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    let labels: Vec<&str> = value.split('.').collect();
    if labels.len() >= 2 && labels.iter().all(|l| label_ok(l)) {
        Ok(value.to_owned())
    } else {
        Err(RegionError::InvalidHost(value.to_owned()))
    }
}

/// The facts about a region that tooling reads or writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// Region name.
    pub name: String,
    /// Voice node id.
    pub node_id: String,
    /// Public host of the voice WebSocket.
    pub public_host: String,
    /// Public IP the SFU advertises (may be a `${VAR}` substitution in hand-written overlays).
    pub public_ip: String,
    /// Snowflake worker id.
    pub worker_id: u16,
    /// Whether nodes start in drain mode.
    pub drained: bool,
}

/// Renders the overlay of a new region.
pub fn render_overlay(region: &Region) -> String {
    let drain = if region.drained {
        format!("      - {DRAIN_LITERAL}\n")
    } else {
        String::new()
    };
    format!(
        "# Voice region {name}, scaffolded by `regionctl add`.\n\
apiVersion: kustomize.config.k8s.io/v1beta1\n\
kind: Kustomization\n\
resources:\n\
\x20 - ../_template\n\
nameSuffix: -{name}\n\
labels:\n\
\x20 - pairs:\n\
\x20     discordy.voics.nl/region: {name}\n\
\x20   includeSelectors: true\n\
configMapGenerator:\n\
\x20 - name: voice-region\n\
\x20   behavior: merge\n\
\x20   literals:\n\
\x20     - VOICE_REGION={name}\n\
\x20     - VOICE_NODE_ID={node_id}\n\
\x20     - VOICE_PUBLIC_WS_URL=wss://{host}\n\
\x20     - VOICE_PUBLIC_IP={ip}\n\
\x20     - SFU_PUBLIC_IP={ip}\n\
\x20     # Unique per region within the voice worker range 192-255 (P5).\n\
\x20     - SNOWFLAKE_WORKER_ID={worker}\n\
{drain}\
generatorOptions:\n\
\x20 disableNameSuffixHash: true\n\
patches:\n\
\x20 - target:\n\
\x20     kind: Ingress\n\
\x20     name: voice\n\
\x20   patch: |\n\
\x20     - op: replace\n\
\x20       path: /spec/rules/0/host\n\
\x20       value: {host}\n\
images:\n\
\x20 - name: ghcr.io/discordy-voics/voice-signaling\n\
\x20   newTag: 0.1.0 # {{\"$imagepolicy\": \"flux-system:voice-signaling:tag\"}}\n\
\x20 - name: ghcr.io/discordy-voics/voice-sfu\n\
\x20   newTag: 0.1.0 # {{\"$imagepolicy\": \"flux-system:voice-sfu:tag\"}}\n",
        name = region.name,
        node_id = region.node_id,
        host = region.public_host,
        ip = region.public_ip,
        worker = region.worker_id,
    )
}

fn literals(overlay: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let mut in_block = false;
    for line in overlay.lines() {
        let trimmed = line.trim();
        if trimmed == "literals:" {
            in_block = true;
            continue;
        }
        if !in_block {
            continue;
        }
        match trimmed.strip_prefix("- ") {
            Some(entry) => {
                if let Some((key, value)) = entry.split_once('=') {
                    map.insert(key.to_owned(), value.to_owned());
                }
            }
            None if trimmed.starts_with('#') => {}
            None => in_block = false,
        }
    }
    map
}

/// Reads a region from an overlay, tolerating hand-written ones (`${VAR}` values stay as text).
pub fn parse_overlay(name: &str, overlay: &str) -> Result<Region, RegionError> {
    let values = literals(overlay);
    let get = |key: &'static str| {
        values
            .get(key)
            .cloned()
            .ok_or(RegionError::MissingLiteral(key))
    };
    let ws_url = get("VOICE_PUBLIC_WS_URL")?;
    let worker_id = get("SNOWFLAKE_WORKER_ID")?
        .parse()
        .map_err(|_| RegionError::MissingLiteral("SNOWFLAKE_WORKER_ID"))?;
    Ok(Region {
        name: name.to_owned(),
        node_id: get("VOICE_NODE_ID")?,
        public_host: ws_url
            .strip_prefix("wss://")
            .or_else(|| ws_url.strip_prefix("ws://"))
            .unwrap_or(&ws_url)
            .to_owned(),
        public_ip: get("VOICE_PUBLIC_IP")?,
        worker_id,
        drained: values.contains_key("VOICE_DRAIN_ON_START"),
    })
}

/// Adds or removes the drain literal in an existing overlay, leaving everything else untouched.
pub fn set_drained(overlay: &str, drained: bool) -> Result<String, RegionError> {
    let lines: Vec<&str> = overlay.lines().collect();
    let is_drain = |l: &&str| l.trim() == format!("- {DRAIN_LITERAL}");
    let mut out: Vec<String> = Vec::new();

    if !drained {
        out.extend(
            lines
                .iter()
                .filter(|l| !is_drain(l))
                .map(|l| (*l).to_owned()),
        );
    } else if lines.iter().any(is_drain) {
        out.extend(lines.iter().map(|l| (*l).to_owned()));
    } else {
        let start = lines
            .iter()
            .position(|l| l.trim() == "literals:")
            .ok_or(RegionError::NoLiteralsBlock)?;
        let indent = lines[start + 1..]
            .iter()
            .find(|l| l.trim().starts_with("- "))
            .map(|l| &l[..l.len() - l.trim_start().len()])
            .ok_or(RegionError::NoLiteralsBlock)?;
        let mut end = start + 1;
        while end < lines.len()
            && (lines[end].trim().starts_with("- ") || lines[end].trim().starts_with('#'))
        {
            end += 1;
        }
        out.extend(lines[..end].iter().map(|l| (*l).to_owned()));
        out.push(format!("{indent}- {DRAIN_LITERAL}"));
        out.extend(lines[end..].iter().map(|l| (*l).to_owned()));
    }
    let mut text = out.join("\n");
    text.push('\n');
    Ok(text)
}

/// The lowest worker id in the voice range that `used` does not contain.
pub fn next_worker_id(used: &[u16]) -> Result<u16, RegionError> {
    WORKER_ID_RANGE
        .clone()
        .find(|id| !used.contains(id))
        .ok_or(RegionError::NoWorkerId)
}

/// Lists a resource in `regions/kustomization.yaml` (idempotent). Returns the new text.
pub fn add_resource(root: &str, name: &str) -> String {
    let entry = format!("  - {name}");
    if root.lines().any(|l| l.trim_end() == entry) {
        return root.to_owned();
    }
    let mut lines: Vec<&str> = root.lines().collect();
    let last_resource = lines
        .iter()
        .rposition(|l| l.starts_with("  - "))
        .map_or(lines.len(), |i| i + 1);
    lines.insert(last_resource, &entry);
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region() -> Region {
        Region {
            name: "eu-west".into(),
            node_id: "eu-west-1".into(),
            public_host: "discordy-rtc-eu-west.voics.nl".into(),
            public_ip: "198.51.100.4".into(),
            worker_id: 193,
            drained: false,
        }
    }

    #[test]
    fn names_are_validated() {
        for ok in ["nl-home", "eu-west", "us1", "ab"] {
            assert!(RegionName::parse(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "a",
            "Nl",
            "-x",
            "x-",
            "1abc",
            "has space",
            "common",
            "dev",
            "template",
            "a_b",
            &"x".repeat(25),
        ] {
            assert!(RegionName::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ip_and_host_are_validated() {
        assert!(parse_public_ip("198.51.100.4").is_ok());
        assert!(parse_public_ip("::1").is_err());
        assert!(parse_public_ip("300.1.1.1").is_err());
        assert!(parse_public_host("discordy-rtc-eu.voics.nl").is_ok());
        for bad in [
            "nodots",
            "UPPER.example.nl",
            "a..b",
            "-a.b",
            "a b.nl",
            "https://a.nl",
        ] {
            assert!(parse_public_host(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn rendered_overlay_parses_back() {
        let text = render_overlay(&region());
        assert_eq!(parse_overlay("eu-west", &text), Ok(region()));
        assert!(text.contains("nameSuffix: -eu-west"));
        assert!(text.contains("value: discordy-rtc-eu-west.voics.nl"));
        assert!(text.contains("VOICE_PUBLIC_WS_URL=wss://discordy-rtc-eu-west.voics.nl"));
        assert!(
            text.contains(r#"newTag: 0.1.0 # {"$imagepolicy": "flux-system:voice-signaling:tag"}"#)
        );
    }

    #[test]
    fn parses_the_hand_written_nl_home_overlay() {
        let overlay = "\
configMapGenerator:
  - name: voice-region
    behavior: merge
    literals:
      - VOICE_REGION=nl-home
      - VOICE_NODE_ID=nl-home-gen-serv
      - VOICE_PUBLIC_WS_URL=wss://${RTC_HOST_PREFIX}-nl-home.${DISCORDY_DOMAIN}
      - VOICE_PUBLIC_IP=${HOME_PUBLIC_IP}
      - SFU_PUBLIC_IP=${HOME_PUBLIC_IP}
      # Unique per region
      - SNOWFLAKE_WORKER_ID=192
generatorOptions:
  disableNameSuffixHash: true
";
        let region = parse_overlay("nl-home", overlay).unwrap();
        assert_eq!(region.worker_id, 192);
        assert_eq!(region.public_ip, "${HOME_PUBLIC_IP}");
        assert_eq!(region.node_id, "nl-home-gen-serv");
        assert!(!region.drained);
    }

    #[test]
    fn missing_literals_are_reported() {
        assert_eq!(
            parse_overlay("x", "resources: []\n"),
            Err(RegionError::MissingLiteral("VOICE_PUBLIC_WS_URL"))
        );
    }

    #[test]
    fn drain_adds_and_removes_one_literal_without_touching_the_rest() {
        let original = render_overlay(&region());
        let drained = set_drained(&original, true).unwrap();
        assert!(parse_overlay("eu-west", &drained).unwrap().drained);
        assert_eq!(drained.matches("VOICE_DRAIN_ON_START").count(), 1);
        assert_eq!(set_drained(&drained, true).unwrap(), drained, "idempotent");

        let restored = set_drained(&drained, false).unwrap();
        assert_eq!(restored, original);
        assert_eq!(set_drained(&original, false).unwrap(), original);
    }

    #[test]
    fn drain_inserts_after_trailing_comments_in_hand_written_overlays() {
        let overlay = "a:\n    literals:\n      - X=1\n      # note\nb: 1\n";
        let drained = set_drained(overlay, true).unwrap();
        assert_eq!(
            drained,
            "a:\n    literals:\n      - X=1\n      # note\n      - VOICE_DRAIN_ON_START=true\nb: 1\n"
        );
        assert_eq!(
            set_drained("no block\n", true),
            Err(RegionError::NoLiteralsBlock)
        );
    }

    #[test]
    fn worker_ids_are_allocated_from_the_voice_range() {
        assert_eq!(next_worker_id(&[]), Ok(192));
        assert_eq!(next_worker_id(&[192, 194]), Ok(193));
        let all: Vec<u16> = WORKER_ID_RANGE.collect();
        assert_eq!(next_worker_id(&all), Err(RegionError::NoWorkerId));
    }

    #[test]
    fn resources_are_added_once_after_the_last_entry() {
        let root = "# Layer\napiVersion: v1\nresources:\n  - common\n  - nl-home\n";
        let added = add_resource(root, "eu-west");
        assert_eq!(
            added,
            "# Layer\napiVersion: v1\nresources:\n  - common\n  - nl-home\n  - eu-west\n"
        );
        assert_eq!(add_resource(&added, "eu-west"), added);
    }
}
