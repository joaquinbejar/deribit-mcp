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

#[cfg(feature = "fix")]
use deribit_fix::DeribitFixClient;
#[cfg(feature = "fix")]
use deribit_fix::config::DeribitFixConfig;
use deribit_http::config::credentials::ApiCredentials;
use deribit_http::{DeribitHttpClient, HttpConfig};
use deribit_websocket::client::DeribitWebSocketClient;
use deribit_websocket::config::WebSocketConfig;
#[cfg(feature = "fix")]
use tokio::sync::Mutex;
use tokio::sync::OnceCell;
use url::Url;

use crate::config::Config;
#[cfg(feature = "fix")]
use crate::config::OrderTransport;
use crate::error::AdapterError;

const TESTNET_WS_URL: &str = "wss://test.deribit.com/ws/api/v2";
const MAINNET_WS_URL: &str = "wss://www.deribit.com/ws/api/v2";

/// Shared adapter context.
///
/// Cheap to clone via `Arc`; safe to share across tokio tasks. The
/// upstream HTTP client is constructed eagerly so a misconfiguration
/// surfaces at startup. The WebSocket client is lazy — most v0.1 tools
/// are HTTP-only.
///
/// `Debug` is implemented manually below so the upstream
/// `DeribitFixClient` (which does not derive `Debug`) doesn't leak
/// into the bound; the FIX field is rendered as a redacted
/// `<fix client>` placeholder.
pub struct AdapterContext {
    /// Resolved configuration. Frozen for the lifetime of the process.
    pub config: Arc<Config>,
    /// Upstream HTTP client used by every `Read` / `Account` / `Trading`
    /// tool.
    pub http: DeribitHttpClient,
    /// Upstream WebSocket client. Built lazily on first
    /// `websocket()` access.
    ws: OnceCell<DeribitWebSocketClient>,
    /// Upstream FIX 4.4 client. Built lazily on first
    /// [`ensure_fix`](Self::ensure_fix) call when
    /// `--order-transport=fix` is configured. Wrapped in a tokio
    /// [`Mutex`] because [`deribit_fix::DeribitFixClient`] takes
    /// `&mut self` for `connect` / `disconnect` / order operations.
    #[cfg(feature = "fix")]
    fix: OnceCell<Arc<Mutex<DeribitFixClient>>>,
}

impl std::fmt::Debug for AdapterContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("AdapterContext");
        s.field("config", &self.config)
            .field("http", &"<DeribitHttpClient>")
            .field("ws", &self.ws);
        #[cfg(feature = "fix")]
        s.field(
            "fix",
            &if self.fix.initialized() {
                "<fix client>"
            } else {
                "<not initialized>"
            },
        );
        s.finish()
    }
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
            #[cfg(feature = "fix")]
            fix: OnceCell::new(),
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

    /// Lazily construct, log on, and return a shared handle to the
    /// FIX 4.4 client.
    ///
    /// First call drives `DeribitFixClient::new` + `connect()`,
    /// which performs the FIX `Logon (A)` and starts the heartbeat
    /// task. Subsequent calls return the same `Arc<Mutex<…>>` so
    /// callers reuse a single session across the process lifetime.
    /// SIGTERM should drive [`shutdown_fix`](Self::shutdown_fix) so
    /// the session ends with a proper FIX `Logout (5)`.
    ///
    /// # Errors
    ///
    /// - [`AdapterError::Validation`] with `field = "order_transport"`
    ///   when the configuration does not select the FIX transport
    ///   (`OrderTransport::Http`); calling `ensure_fix` in that
    ///   state is a programmer error.
    /// - [`AdapterError::Auth`] with the upstream FIX rejection
    ///   reason when `Logon (A)` is rejected.
    /// - [`AdapterError::Upstream`] with [`UpstreamErrorKind::Fix`]
    ///   for transport, session, config, and protocol errors.
    ///
    /// [`UpstreamErrorKind::Fix`]: crate::error::UpstreamErrorKind::Fix
    #[cfg(feature = "fix")]
    pub async fn ensure_fix(&self) -> Result<Arc<Mutex<DeribitFixClient>>, AdapterError> {
        match self.config.order_transport {
            OrderTransport::Fix => {}
            OrderTransport::Http => {
                return Err(AdapterError::validation(
                    "order_transport",
                    "ensure_fix called but configured order_transport is `http`",
                ));
            }
        }
        let handle = self
            .fix
            .get_or_try_init(|| async {
                let cfg = fix_config_from(&self.config)?;
                let mut client = DeribitFixClient::new(&cfg).await?;
                client.connect().await?;
                Ok::<_, AdapterError>(Arc::new(Mutex::new(client)))
            })
            .await?;
        Ok(handle.clone())
    }

    /// Issue a FIX `Logout (5)` and tear down the session, if one
    /// has been established. No-op when the FIX session was never
    /// opened. Called from the SIGTERM handler at process shutdown.
    ///
    /// # Errors
    ///
    /// Surfaces any [`AdapterError`] that the upstream
    /// `disconnect` call produces. Best-effort — callers should
    /// log the error rather than abort the shutdown.
    #[cfg(feature = "fix")]
    pub async fn shutdown_fix(&self) -> Result<(), AdapterError> {
        if let Some(handle) = self.fix.get() {
            let mut guard = handle.lock().await;
            guard.disconnect().await?;
        }
        Ok(())
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

/// Build the upstream `DeribitFixConfig` from our resolved `Config`.
///
/// `client_id` becomes the FIX `Username` field; `client_secret`
/// is the password material the upstream library uses to sign the
/// logon (HMAC-SHA-256 with timestamp + nonce, per the Deribit
/// FIX spec). The host / port pair is picked by environment:
/// testnet → `fix-test.deribit.com:9881`, mainnet →
/// `fix.deribit.com:9881`.
#[cfg(feature = "fix")]
fn fix_config_from(config: &Config) -> Result<DeribitFixConfig, AdapterError> {
    let (Some(client_id), Some(client_secret)) =
        (config.client_id.as_ref(), config.client_secret.as_ref())
    else {
        return Err(AdapterError::validation(
            "credentials",
            "FIX transport requires DERIBIT_CLIENT_ID + DERIBIT_CLIENT_SECRET",
        ));
    };
    let mainnet = endpoint_is_mainnet(&config.endpoint);
    let (host, port) = if mainnet {
        ("fix.deribit.com", 9881_u16)
    } else {
        ("fix-test.deribit.com", 9881_u16)
    };
    let mut fix_cfg =
        DeribitFixConfig::new().with_credentials(client_id.clone(), client_secret.clone());
    fix_cfg.host = host.to_string();
    fix_cfg.port = port;
    fix_cfg.use_ssl = false;
    Ok(fix_cfg)
}

fn is_mainnet(url: &Url) -> bool {
    matches!(url.host_str(), Some(host) if host == "www.deribit.com" || host == "deribit.com")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LogFormat, OrderTransport, Transport};
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
            order_transport: OrderTransport::Http,
        }
    }

    #[cfg(feature = "fix")]
    #[tokio::test]
    async fn ensure_fix_when_transport_is_http_returns_validation() {
        // Default `cfg(...)` builds with `OrderTransport::Http`. The
        // ensure_fix call must short-circuit with a structured
        // Validation error rather than attempt a network connect.
        let ctx =
            AdapterContext::new(Arc::new(cfg("https://test.deribit.com", true))).expect("ctx");
        // `Arc<Mutex<DeribitFixClient>>` doesn't derive `Debug`, so
        // we destructure the result manually instead of going
        // through `unwrap_err`.
        match ctx.ensure_fix().await {
            Ok(_) => panic!("expected Validation error, got Ok"),
            Err(AdapterError::Validation { field, .. }) => {
                assert_eq!(field, "order_transport");
            }
            Err(other) => panic!("unexpected: {other:?}"),
        }
    }

    #[cfg(feature = "fix")]
    #[tokio::test]
    async fn ensure_fix_without_credentials_returns_validation() {
        // Configure the FIX transport but with no creds; the
        // upstream `DeribitFixClient::new` would otherwise be
        // exercised. Adapter rejects up-front.
        let mut config = cfg("https://test.deribit.com", false);
        config.order_transport = OrderTransport::Fix;
        config.allow_trading = true;
        let ctx = AdapterContext::new(Arc::new(config)).expect("ctx");
        match ctx.ensure_fix().await {
            Ok(_) => panic!("expected Validation error, got Ok"),
            Err(AdapterError::Validation { field, .. }) => {
                assert_eq!(field, "credentials");
            }
            Err(other) => panic!("unexpected: {other:?}"),
        }
    }

    #[cfg(feature = "fix")]
    #[tokio::test]
    async fn shutdown_fix_when_never_opened_is_noop() {
        let ctx =
            AdapterContext::new(Arc::new(cfg("https://test.deribit.com", true))).expect("ctx");
        ctx.shutdown_fix().await.expect("noop ok");
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
