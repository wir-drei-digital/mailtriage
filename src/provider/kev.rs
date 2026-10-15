//! A Kev server (<https://github.com/jaredpalmer/kev>): open-weight decision
//! models whose API is TypeSafe's System One, so the request and the
//! answers are OpenRouter's (`systemone`). It runs in one of two ways:
//! managed (`managed` is set; setup installs it and `crate::kev` starts it
//! on demand) or your own server at any URL. A key is needed, and sent,
//! only for a server off this machine.
use super::{systemone, Decision, DecisionProvider, DecisionRequest, ProviderUnavailable};
use crate::domain::{ManagedKev, ProviderConfig};
use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use std::{fmt, io::Read, time::Duration};

/// The provider's name in every error text.
pub const NAME: &str = "Kev server";
/// The name Kev serves its checkpoint under.
pub const DEFAULT_MODEL: &str = "kev-latest";
/// What a managed install serves (Kev 1.0's Kev-4B).
pub const DEFAULT_CHECKPOINT: &str = "jaredpalmer/kev-4b@v1.0";
pub const DEFAULT_PORT: u16 = 8009;
/// The variable setup offers for a remote server's key.
pub const DEFAULT_KEY_ENV: &str = "KEV_API_KEY";
const URL_RULE: &str = "provider.endpoint must be a Kev server URL ending in /v1/systemone";
const MAX_MODELS_BYTES: u64 = 1024 * 1024;

pub struct Kev;

/// A managed server's endpoint: `http://127.0.0.1:PORT/v1/systemone`.
/// Port 80 is rejected by validation because URL normalization removes it.
pub fn local_endpoint(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1/systemone")
}

/// Whether `endpoint` is a URL on this machine: `127.0.0.1`, `localhost`
/// or `[::1]`.
pub fn is_local(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint)
        .is_ok_and(|url| matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")))
}

/// The model list next to `endpoint`: `.../v1/systemone` becomes
/// `.../v1/models`.
pub fn models_url(endpoint: &str) -> Option<String> {
    endpoint
        .strip_suffix("/v1/systemone")
        .map(|base| format!("{base}/v1/models"))
}

impl DecisionProvider for Kev {
    fn validate(&self, config: &ProviderConfig) -> Result<()> {
        let url = reqwest::Url::parse(&config.endpoint).map_err(|_| anyhow!(URL_RULE))?;
        if !config.endpoint.ends_with("/v1/systemone")
            || !url.path().ends_with("/v1/systemone")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            bail!(URL_RULE);
        }
        if !url.username().is_empty() || url.password().is_some() {
            bail!("provider.endpoint must not contain a user name or password; use api_key_command for the key");
        }
        match url.scheme() {
            "https" => {}
            "http" if is_local(&config.endpoint) => {}
            "http" => bail!("provider.endpoint must use https for a server off this machine"),
            _ => bail!(URL_RULE),
        }
        if let Some(managed) = &config.managed {
            // The supervisor binds Kev to 127.0.0.1 on this port.
            if url.scheme() != "http"
                || url.host_str() != Some("127.0.0.1")
                || url.path() != "/v1/systemone"
            {
                bail!("a managed Kev endpoint must be http://127.0.0.1:PORT/v1/systemone");
            }
            // URL normalization erases :80. Runtime reads `url.port()`, so
            // a managed server requires a retained, explicit port.
            if url.port().is_none() {
                if config.endpoint.starts_with("http://127.0.0.1:80/") {
                    bail!("a managed Kev endpoint must use an explicit non-default port (port 80 is not supported)");
                }
                bail!("a managed Kev endpoint must be http://127.0.0.1:PORT/v1/systemone");
            }
            if managed.checkpoint.trim().is_empty() {
                bail!("provider.managed.checkpoint cannot be empty");
            }
            if managed
                .idle_seconds
                .is_some_and(|seconds| !(1..=86400).contains(&seconds))
            {
                bail!("provider.managed.idle_seconds must be between 1 and 86400");
            }
        }
        Ok(())
    }

    /// `kev` for a server off this machine; `None` on this machine, and for
    /// an endpoint that is no URL, which `validate` refuses.
    fn key_account(&self, config: &ProviderConfig) -> Option<&'static str> {
        reqwest::Url::parse(&config.endpoint)
            .ok()
            .filter(|_| !is_local(&config.endpoint))
            .map(|_| "kev")
    }

    fn label(&self) -> &'static str {
        "Kev server you run (enter its URL)"
    }

    /// A managed Kev-4B on port 8009; setup clears `managed` and sets the
    /// URL for a server you run.
    fn new_config(&self) -> ProviderConfig {
        ProviderConfig {
            kind: "kev".to_owned(),
            model: DEFAULT_MODEL.to_owned(),
            endpoint: local_endpoint(DEFAULT_PORT),
            api_key_command: None,
            api_key_env: String::new(),
            timeout_seconds: 60,
            managed: Some(ManagedKev {
                checkpoint: DEFAULT_CHECKPOINT.to_owned(),
                idle_seconds: None,
            }),
        }
    }

    /// Managed: Kev is installed and answers, started if needed. Your own
    /// server: `/v1/models` answers at all; anything it answers is left to
    /// the request, whose errors count an attempt.
    fn prepare(
        &self,
        config: &ProviderConfig,
        key: Option<&str>,
    ) -> Result<(), ProviderUnavailable> {
        if config.managed.is_some() {
            return crate::kev::runtime::ensure_running(config);
        }
        let url = models_url(&config.endpoint).unwrap_or_default();
        let timeout = Duration::from_secs(config.timeout_seconds.min(10));
        let key = if is_local(&config.endpoint) {
            None
        } else {
            key
        };
        match list_models(&url, key, timeout) {
            Err(ModelsError::Unreachable) => Err(ProviderUnavailable(format!(
                "{} at {url}",
                ModelsError::Unreachable
            ))),
            _ => Ok(()),
        }
    }

    fn decide(
        &self,
        config: &ProviderConfig,
        request: &DecisionRequest,
        key: Option<&str>,
    ) -> Result<Decision> {
        self.validate(config)?;
        let _activity = if config.managed.is_some() {
            // Its errors are `ProviderUnavailable`: the job goes back
            // without an attempt.
            Some(crate::kev::runtime::begin_request(config)?)
        } else {
            None
        };
        let key = if is_local(&config.endpoint) {
            None
        } else {
            key
        };
        let body = systemone::request_body(config, request);
        systemone::parse(
            systemone::send(config, &config.endpoint, &body, key, NAME)?,
            NAME,
        )
    }
}

/// One entry of `GET /v1/models`; Kev adds fields mailtriage ignores.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModelCard {
    pub name: String,
    /// The checkpoint as given to `--run`.
    #[serde(default)]
    pub run: Option<String>,
}

/// Why `list_models` has no list. Fixed texts: no response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelsError {
    /// No connection, or no answer within the timeout.
    Unreachable,
    Http(u16),
    /// An answer that is not a model list.
    Invalid,
}

impl fmt::Display for ModelsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelsError::Unreachable => f.write_str("the Kev server did not answer"),
            ModelsError::Http(code) => write!(f, "the Kev server returned HTTP {code}"),
            ModelsError::Invalid => f.write_str("the Kev server sent no model list"),
        }
    }
}

impl std::error::Error for ModelsError {}

/// `GET url` (a Kev server's `/v1/models`): the models it serves. Bearer
/// `key` when given; no redirects; at most 1 MiB.
pub fn list_models(
    url: &str,
    key: Option<&str>,
    timeout: Duration,
) -> Result<Vec<ModelCard>, ModelsError> {
    #[derive(Deserialize)]
    struct List {
        models: Vec<ModelCard>,
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ModelsError::Unreachable)?;
    let mut get = client.get(url);
    if let Some(key) = key {
        get = get.bearer_auth(key);
    }
    let response = get.send().map_err(|_| ModelsError::Unreachable)?;
    if !response.status().is_success() {
        return Err(ModelsError::Http(response.status().as_u16()));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_MODELS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ModelsError::Invalid)?;
    if bytes.len() as u64 > MAX_MODELS_BYTES {
        return Err(ModelsError::Invalid);
    }
    serde_json::from_slice::<List>(&bytes)
        .map(|list| list.models)
        .map_err(|_| ModelsError::Invalid)
}
