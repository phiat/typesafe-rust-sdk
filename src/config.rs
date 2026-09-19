//! Client settings and per-call options.

use std::fmt;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::retry::RetryPolicy;

pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";
pub const DEFAULT_MODEL: &str = "jev-latest";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

pub const ENV_API_KEY: &str = "TYPESAFE_API_KEY";
pub const ENV_BASE_URL: &str = "TYPESAFE_BASE_URL";
pub const ENV_DEFAULT_MODEL: &str = "TYPESAFE_DEFAULT_MODEL";

/// Connection settings for a client.
///
/// | Setting | Environment variable | Default |
/// | --- | --- | --- |
/// | [`api_key`](Config::api_key) | `TYPESAFE_API_KEY` | none |
/// | [`base_url`](Config::base_url) | `TYPESAFE_BASE_URL` | `https://api.typesafe.ai` |
/// | [`model`](Config::model) | `TYPESAFE_DEFAULT_MODEL` | `jev-latest` |
/// | [`timeout`](Config::timeout) | | 10 s per response |
/// | [`retry`](Config::retry) | | [`RetryPolicy::default`] |
/// | [`header`](Config::header) | | extra request headers |
///
/// [`Config::from_env`] reads the environment variables, and the setters
/// override them. The API key is kept out of `Debug` output.
///
/// A missing key is not an error here, so an app can start without one. Calls
/// then fail with [`ErrorKind::NoApiKey`](crate::ErrorKind::NoApiKey) without
/// making a request.
///
/// ```
/// use std::time::Duration;
/// use typesafe::Config;
///
/// let config = Config::from_env()
///     .model("jev-1.13.0")
///     .timeout(Duration::from_secs(5));
/// ```
#[derive(Clone)]
pub struct Config {
    pub(crate) api_key: Option<String>,
    pub(crate) base_url: String,
    pub(crate) model: String,
    pub(crate) timeout: Duration,
    pub(crate) retry: RetryPolicy,
    pub(crate) headers: Vec<(String, String)>,
}

impl Default for Config {
    fn default() -> Self {
        Config::new()
    }
}

impl Config {
    /// The defaults, ignoring the environment.
    pub fn new() -> Self {
        Config {
            api_key: None,
            base_url: DEFAULT_BASE_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            timeout: DEFAULT_TIMEOUT,
            retry: RetryPolicy::default(),
            headers: Vec::new(),
        }
    }

    /// The defaults, with the key, base URL and model taken from the
    /// environment where set. Blank values count as unset.
    pub fn from_env() -> Self {
        Config::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Self {
        let get = |name: &str| {
            get(name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let mut config = Config::new();
        config.api_key = get(ENV_API_KEY);
        if let Some(url) = get(ENV_BASE_URL) {
            config = config.base_url(url);
        }
        if let Some(model) = get(ENV_DEFAULT_MODEL) {
            config.model = model;
        }
        config
    }

    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    /// The model calls use unless they name one. Pin a versioned model such as
    /// `jev-1.13.0` once thresholds are tuned; `jev-latest` moves when a new
    /// release ships.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// How long to wait for each response.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// An extra header sent with every request.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Whether there is an API key.
    pub fn is_configured(&self) -> bool {
        self.api_key.is_some()
    }

    /// The model calls use unless they name one.
    pub fn default_model(&self) -> &str {
        &self.model
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("api_key", &self.api_key.as_ref().map(|_| "[redacted]"))
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .field("retry", &self.retry)
            .field("headers", &self.headers)
            .finish()
    }
}

/// Overrides for a single call.
///
/// ```
/// use std::time::Duration;
/// use typesafe::{CallOptions, RetryPolicy};
///
/// let options = CallOptions::new()
///     .model("jev-1.13.0")
///     .timeout(Duration::from_secs(2))
///     .retry(RetryPolicy::disabled());
/// ```
#[derive(Clone, Debug, Default)]
pub struct CallOptions {
    pub(crate) model: Option<String>,
    pub(crate) timeout: Option<Duration>,
    pub(crate) retry: Option<RetryPolicy>,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) extra_body: Map<String, Value>,
}

impl CallOptions {
    pub fn new() -> Self {
        CallOptions::default()
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = Some(retry);
        self
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// An extra top-level request field, for API features this client
    /// predates. It can't replace `state`, `model` or `questions`.
    pub fn extra_body(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.extra_body.insert(key.into(), value.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn from(vars: &[(&str, &str)]) -> Config {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(|name| vars.get(name).cloned())
    }

    #[test]
    fn reads_the_environment() {
        let config = from(&[
            ("TYPESAFE_API_KEY", " key "),
            ("TYPESAFE_BASE_URL", "http://localhost:4000/"),
            ("TYPESAFE_DEFAULT_MODEL", "jev-1.13.0"),
        ]);
        assert_eq!(config.api_key.as_deref(), Some("key"));
        assert_eq!(config.base_url, "http://localhost:4000");
        assert_eq!(config.model, "jev-1.13.0");
    }

    #[test]
    fn blank_values_are_unset() {
        let config = from(&[("TYPESAFE_API_KEY", "  "), ("TYPESAFE_DEFAULT_MODEL", "")]);
        assert!(!config.is_configured());
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
    }

    #[test]
    fn setters_override_the_environment() {
        let config = from(&[("TYPESAFE_API_KEY", "env")])
            .api_key("given")
            .model("m");
        assert_eq!(config.api_key.as_deref(), Some("given"));
        assert_eq!(config.default_model(), "m");
    }

    #[test]
    fn debug_hides_the_key() {
        let shown = format!("{:?}", Config::new().api_key("sk-secret"));
        assert!(!shown.contains("sk-secret"));
        assert!(shown.contains("[redacted]"));
    }
}
