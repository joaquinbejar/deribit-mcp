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

use deribit_http::{DeribitHttpClient, HttpConfig};
use deribit_websocket::client::DeribitWebSocketClient;
use deribit_websocket::config::WebSocketConfig;
use deribit_websocket::error::WebSocketError;
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
    /// endpoint is not a valid URL, and [`AdapterError::Internal`] for
    /// any other startup failure that the HTTP client surfaces.
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

    /// Lazily construct (or return) the WebSocket client.
    ///
    /// # Errors
    ///
    /// Returns [`AdapterError::Internal`] when the WebSocket URL cannot
    /// be parsed and [`AdapterError::Upstream`] when the upstream
    /// WebSocket crate refuses the configuration.
    pub async fn websocket(&self) -> Result<&DeribitWebSocketClient, AdapterError> {
        self.ws
            .get_or_try_init(|| async {
                let cfg = ws_config_from(&self.config).map_err(|_| {
                    WebSocketError::ConnectionFailed("invalid websocket URL".to_string())
                })?;
                DeribitWebSocketClient::new(&cfg)
            })
            .await
            .map_err(AdapterError::from)
    }
}

/// Build the upstream `HttpConfig` from our resolved `Config`.
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
    Ok(cfg)
}

/// Build the upstream `WebSocketConfig` from our resolved `Config`.
fn ws_config_from(config: &Config) -> Result<WebSocketConfig, url::ParseError> {
    let url = if endpoint_is_mainnet(&config.endpoint) {
        MAINNET_WS_URL
    } else {
        TESTNET_WS_URL
    };
    WebSocketConfig::with_url(url)
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
}
