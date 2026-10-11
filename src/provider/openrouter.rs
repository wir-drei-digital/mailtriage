//! OpenRouter's Decisions API, with any Decisions model. The endpoint is
//! pinned, so the key is sent nowhere else.
use super::{systemone, Decision, DecisionProvider, DecisionRequest};
use crate::domain::ProviderConfig;
use anyhow::{anyhow, bail, Context, Result};
pub const DECISIONS_ENDPOINT: &str = "https://openrouter.ai/api/alpha/decisions";
/// What a new setup writes: an alias OpenRouter moves to the newest Jev.
pub const DEFAULT_MODEL: &str = "typesafe/jev-latest";
pub const DEFAULT_KEY_ENV: &str = "OPENROUTER_API_KEY";

pub struct OpenRouter;

impl DecisionProvider for OpenRouter {
    fn validate(&self, config: &ProviderConfig) -> Result<()> {
        if config.endpoint.trim().is_empty() {
            bail!("OpenRouter Decisions endpoint is required");
        }
        let url = reqwest::Url::parse(&config.endpoint)
            .context("invalid OpenRouter Decisions endpoint")?;
        if url.as_str() != DECISIONS_ENDPOINT
            && !(url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.path() == "/api/alpha/decisions")
        {
            bail!("provider endpoint must be the OpenRouter Decisions endpoint");
        }
        Ok(())
    }

    fn key_account(&self, _config: &ProviderConfig) -> Option<&'static str> {
        Some("openrouter")
    }

    fn label(&self) -> &'static str {
        "OpenRouter (a Decisions model, Jev by default; needs an API key)"
    }

    fn new_config(&self) -> ProviderConfig {
        ProviderConfig {
            kind: "openrouter".to_owned(),
            model: DEFAULT_MODEL.to_owned(),
            endpoint: DECISIONS_ENDPOINT.to_owned(),
            api_key_command: None,
            api_key_env: DEFAULT_KEY_ENV.to_owned(),
            timeout_seconds: 30,
            managed: None,
        }
    }

    fn decide(
        &self,
        config: &ProviderConfig,
        request: &DecisionRequest,
        key: Option<&str>,
    ) -> Result<Decision> {
        self.validate(config)?;
        let key = key.ok_or_else(|| anyhow!("OpenRouter API key is missing"))?;
        let body = systemone::request_body(config, request);
        systemone::parse(
            systemone::send(config, &config.endpoint, &body, Some(key), NAME)?,
            NAME,
        )
    }
}

/// The provider's name in every error text, as before the split.
const NAME: &str = "OpenRouter Decisions";
