//! The gateway internal API (P4): `POST /internal/v1/voice/nodes/{node_id}/(drain|undrain)`.

use std::time::Duration;

use crate::application::error::AppError;
use crate::application::ports::NodeDrainGateway;
use crate::domain::region::NodeId;

/// Gateway client authenticated with `INTERNAL_SERVICE_TOKEN`.
pub struct GatewayClient {
    agent: ureq::Agent,
    base: String,
    token: String,
}

impl std::fmt::Debug for GatewayClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayClient")
            .field("base", &self.base)
            .field("token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl GatewayClient {
    /// Creates a client for the gateway at `base`.
    pub fn new(base: impl Into<String>, token: impl Into<String>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .into();
        Self {
            agent,
            base: base.into().trim_end_matches('/').to_owned(),
            token: token.into(),
        }
    }
}

impl NodeDrainGateway for GatewayClient {
    fn set_drained(&self, node: &NodeId, drained: bool) -> Result<(), AppError> {
        let action = if drained { "drain" } else { "undrain" };
        let url = format!("{}/internal/v1/voice/nodes/{node}/{action}", self.base);
        let response = self
            .agent
            .post(&url)
            .header("X-Internal-Token", &self.token)
            .send_empty()
            .map_err(|e| AppError::Gateway(e.to_string()))?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(AppError::Gateway(format!("{action} {node}: HTTP {status}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_hides_the_token() {
        let client = GatewayClient::new("http://gateway", "very-secret");
        assert!(!format!("{client:?}").contains("very-secret"));
    }
}
