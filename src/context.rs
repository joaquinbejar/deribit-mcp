//! Shared adapter context — the single value every handler holds.
//!
//! `AdapterContext` owns the configuration snapshot, the upstream HTTP
//! client (built eagerly), and a lazy WebSocket client (constructed on
//! first use, since live resources only land in v0.3 — ADR-0006).
//!
//! Handlers receive an `Arc<AdapterContext>`. The context is built once
//! at startup and never mutated; the `OnceCell` guards single-init of
//! the WS client.

use std::sync::Arc;

use deribit_http::config::credentials::ApiCredentials;
use deribit_http::{DeribitHttpClient, HttpConfig};
use deribit_websocket::client::DeribitWebSocketClient;
use deribit_websocket::config::WebSocketConfig;
use tokio::sync::OnceCell;
use url::Url;

use crate::config::Config;
use crate::error::AdapterError;

const TESTNET_WS_URL: &str = "wss://test.deribit.com/ws/api/v2";
const MAINNET_WS_URL: &str = "wss://www.deribit.com/ws/api/v2";

/// Shared adapter context.
///
/// Cheap to clone via `Arc`; safe to share across tokio tasks. The
/// upstream HTTP client is constructed eagerly so a misconfiguration
/// surfaces at startup. The WebSocket client is lazy — most v0.1 tools
/// are HTTP-only.
#[derive(Debug)]
pub struct AdapterContext {
    /// Resolved configuration. Frozen for the lifetime of the process.
    pub config: Arc<Config>,
    /// Upstream HTTP client used by every `Read` / `Account` / `Trading`
    /// tool.
    pub http: DeribitHttpClient,
    /// Upstream WebSocket client. Built lazily on first
    /// `websocket()` access.
    ws: OnceCell<DeribitWebSocketClient>,
}

impl AdapterContext {
    /// Build the adapter context from a resolved [`Config`].
    ///
    /// # Errors
    ///
    /// Returns [`AdapterError::Validation`] when the configured Deribit
    /// endpoint is not a valid URL. The upstream HTTP client itself is
    /// infallible to construct.
    pub fn new(config: Arc<Config>) -> Result<Self, AdapterError> {
        let http_cfg = http_config_from(&config)?;
        let http = DeribitHttpClient::with_config(http_cfg);

        Ok(Self {
            config,
            http,
            ws: OnceCell::new(),
        })
    }

    /// Whether the configuration carries both an OAuth client id and
    /// secret. The tool registry uses this to gate the `Account` and
    /// `Trading` families (ADR-0003 / ADR-0010).
    #[must_use]
    pub fn has_credentials(&self) -> bool {
        self.config.client_id.is_some() && self.config.client_secret.is_some()
    }

    /// Snapshot of the OAuth state. Drives registry decisions
    /// (whether `Account` / `Trading` tools register at all) and
    /// gives downstream callers a stable enum to match on instead
    /// of a free-form `bool`.
    ///
    /// Auth is **lazy** — `Configured` does not imply that
    /// `deribit-http` has yet issued a `public/auth` call. The
    /// upstream `AuthManager` triggers OAuth on the first private
    /// endpoint hit and refreshes ~30 s before `expires_in`
    /// (handled inside `deribit-http`).
    #[must_use]
    pub fn auth_state(&self) -> AuthState {
        if self.has_credentials() {
            AuthState::Configured
        } else {
            AuthState::Anonymous
        }
    }

    /// Lazily construct (or return) the WebSocket client.
    ///
    /// # Errors
    ///
    /// Returns [`AdapterError::Upstream`] (with
    /// [`UpstreamErrorKind::Websocket`]) when the upstream WebSocket
    /// crate refuses the configuration — typically a transport
    /// failure on the very first connect attempt.
    ///
    /// [`UpstreamErrorKind::Websocket`]: crate::error::UpstreamErrorKind::Websocket
    pub async fn websocket(&self) -> Result<&DeribitWebSocketClient, AdapterError> {
        self.ws
            .get_or_try_init(|| async {
                let cfg = ws_config_from(&self.config);
                DeribitWebSocketClient::new(&cfg)
            })
            .await
            .map_err(AdapterError::from)
    }
}

/// Build the upstream `HttpConfig` from our resolved `Config`.
///
/// Forwards `client_id` / `client_secret` from our resolved `Config`
/// into the upstream `ApiCredentials`. Without this step, the upstream
/// `HttpConfig::testnet()` / `production()` constructors fall back to
/// `DERIBIT_CLIENT_ID` / `DERIBIT_CLIENT_SECRET` env vars — which may
/// already match, but only if dotenvy has populated the process
/// environment. Forwarding explicitly removes the dependency.
fn http_config_from(config: &Config) -> Result<HttpConfig, AdapterError> {
    let parsed = Url::parse(&config.endpoint)
        .map_err(|err| AdapterError::validation("endpoint", format!("invalid URL: {err}")))?;

    let testnet = !is_mainnet(&parsed);
    let mut cfg = if testnet {
        HttpConfig::testnet()
    } else {
        HttpConfig::production()
    };
    cfg.base_url = parsed;
    cfg.testnet = testnet;
    // Match on references first so we never clone the secret on the
    // partial-credential branch (where the clone would be discarded
    // and only inflate the number of in-memory copies of the secret
    // for `tracing`/heap dumps to potentially observe).
    cfg.credentials = match (config.client_id.as_ref(), config.client_secret.as_ref()) {
        (Some(client_id), Some(client_secret)) => Some(ApiCredentials {
            client_id: Some(client_id.clone()),
            client_secret: Some(client_secret.clone()),
        }),
        _ => None,
    };
    Ok(cfg)
}

/// OAuth posture the adapter advertises to its callers.
///
/// Returned by [`AdapterContext::auth_state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthState {
    /// No credentials configured — only public `Read` tools register.
    Anonymous,
    /// Credentials present in the config. The first private call
    /// triggers OAuth via the upstream `AuthManager`.
    Configured,
}

/// Build the upstream `WebSocketConfig` from our resolved `Config`.
///
/// Infallible: both URLs are compile-time constants and parse
/// successfully. The `expect` here would only fire if the upstream
/// crate's URL parser regressed.
fn ws_config_from(config: &Config) -> WebSocketConfig {
    let url = if endpoint_is_mainnet(&config.endpoint) {
        MAINNET_WS_URL
    } else {
        TESTNET_WS_URL
    };
    WebSocketConfig::with_url(url).expect("compile-time WS URL constant must parse")
}

fn endpoint_is_mainnet(endpoint: &str) -> bool {
    Url::parse(endpoint).ok().is_some_and(|u| is_mainnet(&u))
}

fn is_mainnet(url: &Url) -> bool {
    matches!(url.host_str(), Some(host) if host == "www.deribit.com" || host == "deribit.com")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LogFormat, Transport};
    use std::net::SocketAddr;

    fn cfg(endpoint: &str, with_creds: bool) -> Config {
        Config {
            endpoint: endpoint.to_string(),
            client_id: with_creds.then(|| "id".to_string()),
            client_secret: with_creds.then(|| "secret".to_string()),
            allow_trading: false,
            max_order_usd: None,
            transport: Transport::Stdio,
            http_listen: SocketAddr::from(([127, 0, 0, 1], 8723)),
            http_bearer_token: None,
            log_format: LogFormat::Text,
        }
    }

    #[test]
    fn context_builds_for_testnet_endpoint() {
        let ctx =
            AdapterContext::new(Arc::new(cfg("https://test.deribit.com", false))).expect("context");
        assert!(!ctx.has_credentials());
    }

    #[test]
    fn context_builds_for_mainnet_endpoint() {
        let ctx =
            AdapterContext::new(Arc::new(cfg("https://www.deribit.com", true))).expect("context");
        assert!(ctx.has_credentials());
    }

    #[test]
    fn context_rejects_invalid_endpoint() {
        let err = AdapterContext::new(Arc::new(cfg("not a url", false))).unwrap_err();
        assert!(matches!(
            err,
            AdapterError::Validation { ref field, .. } if field == "endpoint"
        ));
    }

    #[test]
    fn has_credentials_requires_both_id_and_secret() {
        let mut c = cfg("https://test.deribit.com", false);
        c.client_id = Some("id".into());
        let ctx = AdapterContext::new(Arc::new(c)).expect("context");
        assert!(!ctx.has_credentials());
    }

    #[test]
    fn auth_state_is_anonymous_without_credentials() {
        let ctx =
            AdapterContext::new(Arc::new(cfg("https://test.deribit.com", false))).expect("ctx");
        assert_eq!(ctx.auth_state(), AuthState::Anonymous);
    }

    #[test]
    fn auth_state_is_configured_with_credentials() {
        let ctx =
            AdapterContext::new(Arc::new(cfg("https://test.deribit.com", true))).expect("ctx");
        assert_eq!(ctx.auth_state(), AuthState::Configured);
    }

    #[test]
    fn http_config_carries_credentials_into_upstream() {
        // We can't observe `HttpConfig.credentials` from outside the
        // adapter (the field is `pub` but the client owns the value),
        // so this test pins the struct-level forwarding by building
        // the same config the constructor builds and asserting the
        // credentials it places on `HttpConfig`.
        let resolved = cfg("https://test.deribit.com", true);
        let http_cfg = http_config_from(&resolved).expect("http cfg");
        let creds = http_cfg.credentials.as_ref().expect("credentials present");
        assert_eq!(creds.client_id.as_deref(), Some("id"));
        assert_eq!(creds.client_secret.as_deref(), Some("secret"));
    }

    #[test]
    fn http_config_omits_credentials_without_both() {
        let mut resolved = cfg("https://test.deribit.com", false);
        resolved.client_id = Some("id".into());
        let http_cfg = http_config_from(&resolved).expect("http cfg");
        assert!(http_cfg.credentials.is_none());
    }
}
