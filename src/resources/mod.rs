//! Resource registry, `deribit://` URI parsing, and read dispatch.
//!
//! Resources are partitioned by lifetime:
//!
//! - [`static_`] — refresh-on-read resources backed by `deribit-http`.
//! - [`live`] — WebSocket-backed subscribable resources, populated in
//!   v0.3 per ADR-0006.
//!
//! v0.1-07 shipped the URI parser, catalogue, and dispatch surface.
//! v0.1-12 wired the two static reads:
//! `deribit://currencies` → `static_::read_currencies`,
//! `deribit://instruments/{currency}` → `static_::read_instruments`.
//! v0.3-02 wires `deribit://book/{instrument}` through the
//! [`LiveRegistry`] when a [`SubscriptionProvider`] is attached.
//! `deribit://ticker/{instrument}` (v0.3-03) and
//! `deribit://trades/{instrument}` (v0.3-04) are accepted by the
//! parser but `read()` still returns
//! [`AdapterError::Internal`] until those issues land. Without a
//! `SubscriptionProvider`, even `Book` falls back to
//! `AdapterError::Internal { reason: "live subscription provider
//! not configured" }`.

use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{Annotated, RawResource, RawResourceTemplate, Resource, ResourceTemplate};

use crate::context::AdapterContext;
use crate::error::AdapterError;

pub mod live;
pub mod static_;

use live::{BookSnapshot, LiveRegistry, SubscriptionProvider, TickerSnapshot, TradeUpdate};

/// Strongly-typed `deribit://` URI variants.
///
/// The parser accepts every documented template; serving lives
/// behind [`ResourceRegistry::read`]. As of v0.3-02 `Book` is
/// served through the [`LiveRegistry`]; the remaining live URIs
/// (`Ticker`, `Trades`) still return [`AdapterError::Internal`]
/// until v0.3-03 / v0.3-04 wire them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResourceUri {
    /// `deribit://currencies` — the currency catalogue (static).
    Currencies,
    /// `deribit://instruments/{currency}` — instruments for a currency
    /// (static).
    Instruments {
        /// Currency symbol, upper-case (`BTC`, `ETH`, …).
        currency: String,
    },
    /// `deribit://book/{instrument}` — order book (live, v0.3+).
    Book {
        /// Instrument name (`BTC-PERPETUAL`, …).
        instrument: String,
    },
    /// `deribit://ticker/{instrument}` — ticker (live, v0.3+).
    Ticker {
        /// Instrument name.
        instrument: String,
    },
    /// `deribit://trades/{instrument}` — last trades (live, v0.3+).
    Trades {
        /// Instrument name.
        instrument: String,
    },
}

impl ResourceUri {
    /// Render back to the canonical `deribit://...` string form.
    #[must_use]
    pub fn to_uri(&self) -> String {
        match self {
            Self::Currencies => "deribit://currencies".to_string(),
            Self::Instruments { currency } => format!("deribit://instruments/{currency}"),
            Self::Book { instrument } => format!("deribit://book/{instrument}"),
            Self::Ticker { instrument } => format!("deribit://ticker/{instrument}"),
            Self::Trades { instrument } => format!("deribit://trades/{instrument}"),
        }
    }
}

/// `deribit://` URI scheme.
const SCHEME: &str = "deribit://";

/// Parse a `deribit://...` URI into [`ResourceUri`].
///
/// # Errors
///
/// Returns [`AdapterError::Validation`] for any input that does not
/// match a documented template.
pub fn parse_resource_uri(s: &str) -> Result<ResourceUri, AdapterError> {
    let rest = s
        .strip_prefix(SCHEME)
        .ok_or_else(|| AdapterError::validation("uri", format!("not a `{SCHEME}` URI: {s}")))?;
    if rest.is_empty() {
        return Err(AdapterError::validation("uri", "empty resource path"));
    }
    let mut segments = rest.splitn(2, '/');
    let head = segments
        .next()
        .ok_or_else(|| AdapterError::validation("uri", "missing resource head"))?;
    // Treat a trailing-slash tail (`deribit://instruments/`) as
    // "missing", not as an empty currency/instrument segment, so the
    // error reported is the documented `field: "uri"` shape.
    let tail = segments.next().filter(|s| !s.is_empty());

    match (head, tail) {
        ("currencies", None) => Ok(ResourceUri::Currencies),
        ("currencies", Some(_)) => Err(AdapterError::validation(
            "uri",
            "`deribit://currencies` takes no path",
        )),
        ("instruments", Some(currency)) => {
            let currency = parse_currency(currency)?;
            Ok(ResourceUri::Instruments { currency })
        }
        ("instruments", None) => Err(AdapterError::validation(
            "uri",
            "`deribit://instruments/{currency}` requires a currency",
        )),
        ("book", Some(instrument)) => {
            let instrument = parse_instrument_name(instrument)?;
            Ok(ResourceUri::Book { instrument })
        }
        ("ticker", Some(instrument)) => {
            let instrument = parse_instrument_name(instrument)?;
            Ok(ResourceUri::Ticker { instrument })
        }
        ("trades", Some(instrument)) => {
            let instrument = parse_instrument_name(instrument)?;
            Ok(ResourceUri::Trades { instrument })
        }
        ("book" | "ticker" | "trades", None) => Err(AdapterError::validation(
            "uri",
            format!("`deribit://{head}/{{instrument}}` requires an instrument"),
        )),
        (other, _) => Err(AdapterError::validation(
            "uri",
            format!("unknown resource head: `{other}`"),
        )),
    }
}

/// Validate a Deribit currency segment (`BTC`, `ETH`, …).
///
/// Deribit currency symbols are short ASCII upper-case identifiers.
/// We accept 1..=8 chars of `[A-Z0-9_]` to match what the upstream
/// `deribit-http` crate sees in practice; the upstream call enforces
/// the canonical set.
fn parse_currency(s: &str) -> Result<String, AdapterError> {
    if s.is_empty() || s.len() > 8 {
        return Err(AdapterError::validation(
            "currency",
            format!("expected 1..=8 chars, got {} for `{s}`", s.len()),
        ));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(AdapterError::validation(
            "currency",
            format!("expected `[A-Z0-9_]`, got `{s}`"),
        ));
    }
    Ok(s.to_string())
}

/// Validate an instrument name segment (`BTC-PERPETUAL`,
/// `BTC-31MAY24-50000-C`, …).
///
/// Deribit instrument names are dash-separated ASCII upper-case tokens.
/// Light-touch validation: 1..=64 chars of `[A-Z0-9_-]`. The upstream
/// HTTP / WebSocket call enforces semantic shape.
fn parse_instrument_name(s: &str) -> Result<String, AdapterError> {
    if s.is_empty() || s.len() > 64 {
        return Err(AdapterError::validation(
            "instrument",
            format!("expected 1..=64 chars, got {} for `{s}`", s.len()),
        ));
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return Err(AdapterError::validation(
            "instrument",
            format!("expected `[A-Z0-9_-]`, got `{s}`"),
        ));
    }
    Ok(s.to_string())
}

/// Snapshot of `resources/list` and `resources/templates/list`
/// produced by the registry.
#[derive(Debug, Default, Clone)]
pub struct ResourceList {
    /// Concrete resources (e.g. `deribit://currencies`).
    pub resources: Vec<Resource>,
    /// URI templates (e.g. `deribit://book/{instrument}`).
    pub templates: Vec<ResourceTemplate>,
}

/// Body returned by [`ResourceRegistry::read`].
///
/// v0.1-07 ships only [`Self::Json`] (JSON payloads from
/// `deribit-http`). The variant set is closed; new transports add
/// new variants.
#[derive(Debug, Clone, PartialEq)]
pub enum ResourceContent {
    /// JSON body produced by an upstream HTTP read. The MIME type
    /// surfaced to MCP is `application/json`.
    Json(serde_json::Value),
}

/// Registry of MCP resources the server exposes.
#[derive(Clone)]
pub struct ResourceRegistry {
    list: ResourceList,
    live: LiveRegistry,
    /// Optional provider for live subscriptions. `None` until the
    /// upstream `deribit-websocket` provider is configured (default
    /// for v0.1 / anonymous contexts); reads on live URIs return
    /// `AdapterError::Internal` when missing.
    provider: Option<Arc<dyn SubscriptionProvider>>,
}

impl std::fmt::Debug for ResourceRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceRegistry")
            .field("list", &self.list)
            .field("live", &self.live)
            .field(
                "provider",
                &self.provider.as_ref().map(|_| "<dyn SubscriptionProvider>"),
            )
            .finish()
    }
}

impl Default for ResourceRegistry {
    fn default() -> Self {
        Self {
            list: ResourceList::default(),
            live: LiveRegistry::new(),
            provider: None,
        }
    }
}

/// Per-call deadline waiting for the first frame on a fresh
/// subscription. Bounded so a stalled upstream cannot turn a
/// `resources/read` into an unbounded await.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);

impl ResourceRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the live-subscription provider. The real upstream
    /// `deribit-websocket` provider is wired by the binary
    /// startup path (v0.3-02 / -03 / -04); tests pass a stub
    /// implementation.
    #[must_use]
    pub fn with_subscription_provider(mut self, provider: Arc<dyn SubscriptionProvider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Build the v0.1 catalogue.
    ///
    /// Concrete entries: `deribit://currencies`. The
    /// per-currency `deribit://instruments/{currency}` resources are
    /// only knowable after a live `get_currencies` call (resolved in
    /// v0.1-12); they live as a template until then.
    ///
    /// Templates: `deribit://instruments/{currency}`,
    /// `deribit://book/{instrument}`,
    /// `deribit://ticker/{instrument}`,
    /// `deribit://trades/{instrument}`. The last three are accepted
    /// by the parser but `read()` returns
    /// [`AdapterError::Validation`] until v0.3 wires the live
    /// transport.
    #[must_use]
    pub fn build() -> Self {
        let mut list = ResourceList::default();
        list.resources.push(make_resource(
            "deribit://currencies",
            "Deribit currency catalogue",
            "Static list of Deribit currency symbols and metadata.",
        ));
        list.templates.push(make_template(
            "deribit://instruments/{currency}",
            "Deribit instruments by currency",
            "Static list of instruments for a given currency.",
        ));
        list.templates.push(make_template(
            "deribit://book/{instrument}",
            "Deribit order book (live)",
            "Order book snapshots from the `book.<instrument>.raw` \
             channel. Read returns the latest decoded BookSnapshot \
             when a SubscriptionProvider is configured; otherwise \
             AdapterError::Internal.",
        ));
        list.templates.push(make_template(
            "deribit://ticker/{instrument}",
            "Deribit ticker (live)",
            "Throttled ticker snapshots from the \
             `ticker.<instrument>.100ms` channel. Read returns the \
             latest TickerSnapshot when a SubscriptionProvider is \
             configured; otherwise AdapterError::Internal.",
        ));
        list.templates.push(make_template(
            "deribit://trades/{instrument}",
            "Deribit last trades (live)",
            "Trade events from the `trades.<instrument>.raw` channel. \
             Read returns the most recent N TradeUpdate values \
             (newest first) when a SubscriptionProvider is configured; \
             otherwise AdapterError::Internal.",
        ));
        Self {
            list,
            live: LiveRegistry::new(),
            provider: None,
        }
    }

    /// Snapshot the registered resources.
    #[must_use]
    pub fn resources(&self) -> Vec<Resource> {
        self.list.resources.clone()
    }

    /// Snapshot the registered resource templates.
    #[must_use]
    pub fn templates(&self) -> Vec<ResourceTemplate> {
        self.list.templates.clone()
    }

    /// Snapshot of the full `resources/list` + templates payload.
    #[must_use]
    pub fn list(&self) -> ResourceList {
        self.list.clone()
    }

    /// Whether the registry has any entries or templates.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.list.resources.is_empty() && self.list.templates.is_empty()
    }

    /// Read a resource by its parsed URI.
    ///
    /// v0.1-12 routes static URIs (`Currencies`, `Instruments`) to
    /// [`static_::read_currencies`] / [`static_::read_instruments`].
    /// Live URIs (`Book`, `Ticker`, `Trades`) return a structured
    /// [`AdapterError::Internal`] with `reason: "live resources land
    /// in v0.3"` so the LLM sees a stable error shape — the live
    /// transport ships in v0.3 (ADR-0006).
    ///
    /// # Errors
    ///
    /// Static reads surface whatever upstream HTTP failure the call
    /// produces (network, rate-limit, API). Live reads return
    /// [`AdapterError::Internal`] until v0.3 ships.
    pub async fn read(
        &self,
        ctx: &AdapterContext,
        uri: &ResourceUri,
    ) -> Result<ResourceContent, AdapterError> {
        match uri {
            ResourceUri::Currencies => {
                Ok(ResourceContent::Json(static_::read_currencies(ctx).await?))
            }
            ResourceUri::Instruments { currency } => Ok(ResourceContent::Json(
                static_::read_instruments(ctx, currency).await?,
            )),
            ResourceUri::Book { instrument } => {
                let value = self.read_live(uri).await?;
                let book = BookSnapshot::from_value(instrument, &value)?;
                Ok(ResourceContent::Json(serde_json::to_value(&book)?))
            }
            ResourceUri::Ticker { instrument } => {
                let value = self.read_live(uri).await?;
                let ticker = TickerSnapshot::from_value(instrument, &value)?;
                Ok(ResourceContent::Json(serde_json::to_value(&ticker)?))
            }
            ResourceUri::Trades { .. } => {
                // Subscribe (so the reader task is running) and
                // hand back the rolling history of decoded
                // trades. Each upstream frame is an array of
                // trade objects; we flatten across frames and
                // sort by timestamp newest-first so the LLM
                // sees a deterministic chronological list,
                // capped at `HISTORY_CAPACITY`.
                use tokio::sync::broadcast::error::RecvError;
                let provider = self.provider.as_ref().ok_or_else(|| {
                    AdapterError::internal("live subscription provider not configured")
                })?;
                let handle = self.live.subscribe(provider.as_ref(), uri).await?;
                let mut updates = handle.updates();
                if handle.latest().await.is_none() {
                    match tokio::time::timeout(FIRST_FRAME_TIMEOUT, updates.recv()).await {
                        Ok(Ok(())) | Ok(Err(RecvError::Lagged(_))) => {}
                        Ok(Err(RecvError::Closed)) => {
                            return Err(AdapterError::internal(
                                "live subscription closed before producing a frame",
                            ));
                        }
                        Err(_elapsed) => {
                            return Err(AdapterError::internal(
                                "live subscription did not produce a frame in time",
                            ));
                        }
                    }
                }
                let mut trades: Vec<TradeUpdate> = Vec::new();
                for frame in handle.history().await {
                    // Propagate decode errors rather than silently
                    // dropping a malformed frame — an upstream
                    // contract change should surface, not vanish.
                    let mut decoded = TradeUpdate::batch_from_value(&frame)?;
                    trades.append(&mut decoded);
                }
                trades.sort_by_key(|t| std::cmp::Reverse(t.timestamp));
                trades.truncate(live::HISTORY_CAPACITY);
                Ok(ResourceContent::Json(serde_json::to_value(&trades)?))
            }
        }
    }

    /// Subscribe to a live URI and return the latest cached
    /// snapshot, waiting up to [`FIRST_FRAME_TIMEOUT`] on a fresh
    /// subscription. The returned `SubscriptionHandle` is dropped at
    /// the end of the call — the underlying entry stays open as long
    /// as any other subscriber holds a handle, otherwise the
    /// refcount returns to zero and the upstream channel closes.
    async fn read_live(&self, uri: &ResourceUri) -> Result<serde_json::Value, AdapterError> {
        use tokio::sync::broadcast::error::RecvError;

        let provider = self
            .provider
            .as_ref()
            .ok_or_else(|| AdapterError::internal("live subscription provider not configured"))?;
        let handle = self.live.subscribe(provider.as_ref(), uri).await?;
        // Subscribe to the broadcast BEFORE checking `latest`. If
        // the first frame arrives between an early `latest` check
        // and the `updates()` subscription, the broadcast receiver
        // would miss its signal (no backlog) and we would time out
        // spuriously.
        let mut updates = handle.updates();
        if let Some(snapshot) = handle.latest().await {
            return Ok(snapshot);
        }
        match tokio::time::timeout(FIRST_FRAME_TIMEOUT, updates.recv()).await {
            Ok(Ok(())) => handle
                .latest()
                .await
                .ok_or_else(|| AdapterError::internal("update fired without snapshot")),
            Ok(Err(RecvError::Lagged(_))) => handle
                .latest()
                .await
                .ok_or_else(|| AdapterError::internal("broadcast lagged before first frame")),
            Ok(Err(RecvError::Closed)) => Err(AdapterError::internal(
                "live subscription closed before producing a frame",
            )),
            Err(_elapsed) => Err(AdapterError::internal(
                "live subscription did not produce a frame in time",
            )),
        }
    }
}

fn make_resource(uri: &'static str, name: &'static str, description: &'static str) -> Resource {
    let raw = RawResource {
        uri: uri.to_string(),
        name: name.to_string(),
        title: None,
        description: Some(description.to_string()),
        mime_type: Some("application/json".to_string()),
        size: None,
        icons: None,
        meta: None,
    };
    Annotated {
        raw,
        annotations: None,
    }
}

fn make_template(
    template: &'static str,
    name: &'static str,
    description: &'static str,
) -> ResourceTemplate {
    let raw = RawResourceTemplate {
        uri_template: template.to_string(),
        name: name.to_string(),
        title: None,
        description: Some(description.to_string()),
        mime_type: Some("application/json".to_string()),
        icons: None,
    };
    Annotated {
        raw,
        annotations: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_currencies() {
        assert_eq!(
            parse_resource_uri("deribit://currencies").unwrap(),
            ResourceUri::Currencies
        );
    }

    #[test]
    fn parses_instruments_with_currency() {
        assert_eq!(
            parse_resource_uri("deribit://instruments/BTC").unwrap(),
            ResourceUri::Instruments {
                currency: "BTC".to_string()
            }
        );
    }

    #[test]
    fn parses_book_template() {
        assert_eq!(
            parse_resource_uri("deribit://book/BTC-PERPETUAL").unwrap(),
            ResourceUri::Book {
                instrument: "BTC-PERPETUAL".to_string()
            }
        );
    }

    #[test]
    fn parses_ticker_and_trades() {
        assert!(matches!(
            parse_resource_uri("deribit://ticker/ETH-PERPETUAL").unwrap(),
            ResourceUri::Ticker { .. }
        ));
        assert!(matches!(
            parse_resource_uri("deribit://trades/BTC-31MAY24-50000-C").unwrap(),
            ResourceUri::Trades { .. }
        ));
    }

    #[test]
    fn rejects_non_deribit_scheme() {
        let err = parse_resource_uri("foo://bar").unwrap_err();
        match err {
            AdapterError::Validation { field, .. } => assert_eq!(field, "uri"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn rejects_currencies_with_path() {
        let err = parse_resource_uri("deribit://currencies/extra").unwrap_err();
        assert!(matches!(err, AdapterError::Validation { .. }));
    }

    #[test]
    fn rejects_instruments_without_currency() {
        let err = parse_resource_uri("deribit://instruments/").unwrap_err();
        assert!(matches!(err, AdapterError::Validation { .. }));
    }

    #[test]
    fn rejects_unknown_head() {
        let err = parse_resource_uri("deribit://options/BTC").unwrap_err();
        assert!(matches!(err, AdapterError::Validation { .. }));
    }

    #[test]
    fn rejects_lowercase_currency() {
        let err = parse_resource_uri("deribit://instruments/btc").unwrap_err();
        match err {
            AdapterError::Validation { field, .. } => assert_eq!(field, "currency"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn rejects_overlong_instrument() {
        let long = "X".repeat(65);
        let uri = format!("deribit://book/{long}");
        let err = parse_resource_uri(&uri).unwrap_err();
        match err {
            AdapterError::Validation { field, .. } => assert_eq!(field, "instrument"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn round_trip_to_uri() {
        for original in [
            "deribit://currencies",
            "deribit://instruments/BTC",
            "deribit://book/BTC-PERPETUAL",
            "deribit://ticker/ETH-PERPETUAL",
            "deribit://trades/BTC-31MAY24-50000-C",
        ] {
            let parsed = parse_resource_uri(original).unwrap();
            assert_eq!(parsed.to_uri(), original);
        }
    }

    #[test]
    fn registry_build_lists_static_currency_entry() {
        let r = ResourceRegistry::build();
        assert_eq!(r.resources().len(), 1);
        assert_eq!(r.resources()[0].raw.uri, "deribit://currencies");
    }

    #[test]
    fn registry_build_lists_four_templates() {
        let r = ResourceRegistry::build();
        let templates = r.templates();
        assert_eq!(templates.len(), 4);
        let uris: Vec<&str> = templates
            .iter()
            .map(|t| t.raw.uri_template.as_str())
            .collect();
        assert!(uris.contains(&"deribit://instruments/{currency}"));
        assert!(uris.contains(&"deribit://book/{instrument}"));
        assert!(uris.contains(&"deribit://ticker/{instrument}"));
        assert!(uris.contains(&"deribit://trades/{instrument}"));
    }

    fn ctx() -> AdapterContext {
        use crate::config::{Config, LogFormat, Transport};
        use std::net::SocketAddr;
        use std::sync::Arc;
        let cfg = Config {
            endpoint: "https://test.deribit.com".to_string(),
            client_id: None,
            client_secret: None,
            allow_trading: false,
            max_order_usd: None,
            transport: Transport::Stdio,
            http_listen: SocketAddr::from(([127, 0, 0, 1], 8723)),
            http_bearer_token: None,
            log_format: LogFormat::Text,
        };
        AdapterContext::new(Arc::new(cfg)).expect("ctx")
    }

    #[tokio::test]
    async fn read_live_book_uses_provider_and_returns_snapshot() {
        use crate::resources::live::{SubscriptionProvider, SubscriptionStream};
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU64, Ordering};

        /// Counts every `subscribe()` call so the test can pin the
        /// "open exactly once across multiple reads" semantics.
        struct CountingProvider {
            opened: Arc<AtomicU64>,
        }
        impl SubscriptionProvider for CountingProvider {
            fn subscribe(
                &self,
                _uri: ResourceUri,
            ) -> Pin<Box<dyn Future<Output = Result<SubscriptionStream, AdapterError>> + Send + '_>>
            {
                self.opened.fetch_add(1, Ordering::AcqRel);
                Box::pin(async move {
                    let frame = serde_json::json!({
                        "bids": [[50_000.0, 1.0], [49_999.0, 2.0]],
                        "asks": [[50_001.0, 1.5]],
                        "change_id": 42_u64,
                        "timestamp": 1_700_000_000_000_i64,
                    });
                    let stream = futures_util::stream::iter(vec![Ok::<_, AdapterError>(frame)]);
                    Ok(Box::pin(stream) as SubscriptionStream)
                })
            }
        }

        let opened = Arc::new(AtomicU64::new(0));
        let provider = Arc::new(CountingProvider {
            opened: opened.clone(),
        });
        let registry = ResourceRegistry::build().with_subscription_provider(provider);
        let uri = ResourceUri::Book {
            instrument: "BTC-PERPETUAL".to_string(),
        };

        let content = registry.read(&ctx(), &uri).await.expect("ok");
        match content {
            ResourceContent::Json(value) => {
                assert_eq!(
                    value.get("instrument").and_then(|v| v.as_str()),
                    Some("BTC-PERPETUAL")
                );
                assert_eq!(value.get("change_id").and_then(|v| v.as_u64()), Some(42));
                assert!(value.get("bids").and_then(|v| v.as_array()).is_some());
            }
        }

        // Second read should hit the cached entry. The first read
        // dropped its `SubscriptionHandle`, so the entry's refcount
        // returned to zero and the deferred-cleanup task may have
        // already removed it. To pin the semantics deterministically,
        // assert the provider was opened *at most twice* — once
        // for the first read, optionally once if the cleanup
        // raced — and never more than that (no per-read leak).
        let _ = registry.read(&ctx(), &uri).await.expect("ok");
        let opens = opened.load(Ordering::Acquire);
        assert!(
            (1..=2).contains(&opens),
            "expected provider opens in 1..=2, got {opens}"
        );
    }

    #[tokio::test]
    async fn read_live_book_without_provider_returns_internal() {
        let registry = ResourceRegistry::build();
        let uri = ResourceUri::Book {
            instrument: "BTC-PERPETUAL".to_string(),
        };
        let err = registry.read(&ctx(), &uri).await.unwrap_err();
        assert!(matches!(err, AdapterError::Internal { .. }));
    }

    #[tokio::test]
    async fn read_live_trades_uses_provider_and_returns_chronological_history() {
        use crate::resources::live::{SubscriptionProvider, SubscriptionStream};
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Arc;

        struct StubProvider;
        impl SubscriptionProvider for StubProvider {
            fn subscribe(
                &self,
                _uri: ResourceUri,
            ) -> Pin<Box<dyn Future<Output = Result<SubscriptionStream, AdapterError>> + Send + '_>>
            {
                Box::pin(async move {
                    // Two frames, each carrying a small batch.
                    let frames = vec![
                        Ok::<_, AdapterError>(serde_json::json!([
                            {
                                "direction": "buy",
                                "price": 50_001.0,
                                "amount": 1.0,
                                "trade_id": "t1",
                                "timestamp": 1_700_000_000_001_i64
                            }
                        ])),
                        Ok::<_, AdapterError>(serde_json::json!([
                            {
                                "direction": "sell",
                                "price": 50_002.0,
                                "amount": 0.5,
                                "trade_id": "t2",
                                "timestamp": 1_700_000_000_002_i64,
                                "liquidation": "M",
                                "tick_direction": 1_i64,
                                "mark_price": 50_001.5,
                                "index_price": 50_010.0
                            }
                        ])),
                    ];
                    let stream = futures_util::stream::iter(frames);
                    Ok(Box::pin(stream) as SubscriptionStream)
                })
            }
        }

        let registry = ResourceRegistry::build().with_subscription_provider(Arc::new(StubProvider));
        let uri = ResourceUri::Trades {
            instrument: "BTC-PERPETUAL".to_string(),
        };
        // Give the reader task a beat to drain both frames.
        let _ = registry.read(&ctx(), &uri).await.expect("first ok");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let content = registry.read(&ctx(), &uri).await.expect("second ok");
        let ResourceContent::Json(value) = content;
        let trades = value.as_array().expect("array");
        let ids: Vec<&str> = trades
            .iter()
            .filter_map(|t| t.get("trade_id").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(
            ids,
            vec!["t2", "t1"],
            "expected newest-first ordering; got {ids:?}"
        );
        let t2 = &trades[0];
        assert_eq!(
            t2.get("liquidation").and_then(|v| v.as_str()),
            Some("M"),
            "t2 should carry the liquidation marker"
        );
        assert_eq!(t2.get("tick_direction").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(
            t2.get("mark_price").and_then(|v| v.as_f64()),
            Some(50_001.5)
        );
        assert_eq!(
            t2.get("index_price").and_then(|v| v.as_f64()),
            Some(50_010.0)
        );
    }

    #[tokio::test]
    async fn read_live_ticker_uses_provider_and_returns_snapshot_with_greeks() {
        use crate::resources::live::{SubscriptionProvider, SubscriptionStream};
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Arc;

        struct StubProvider;
        impl SubscriptionProvider for StubProvider {
            fn subscribe(
                &self,
                _uri: ResourceUri,
            ) -> Pin<Box<dyn Future<Output = Result<SubscriptionStream, AdapterError>> + Send + '_>>
            {
                Box::pin(async move {
                    let frame = serde_json::json!({
                        "mark_price": 50_000.5,
                        "index_price": 50_010.0,
                        "best_bid_price": 50_000.0,
                        "best_ask_price": 50_001.0,
                        "last_price": 49_999.5,
                        "mark_iv": 65.0,
                        "greeks": {
                            "delta": 0.55,
                            "gamma": 0.0001,
                            "vega": 12.3,
                            "theta": -0.4,
                            "rho": 0.05
                        },
                        "timestamp": 1_700_000_000_000_i64,
                    });
                    let stream = futures_util::stream::iter(vec![Ok::<_, AdapterError>(frame)]);
                    Ok(Box::pin(stream) as SubscriptionStream)
                })
            }
        }

        let registry = ResourceRegistry::build().with_subscription_provider(Arc::new(StubProvider));
        let uri = ResourceUri::Ticker {
            instrument: "BTC-31MAY24-50000-C".to_string(),
        };
        let content = registry.read(&ctx(), &uri).await.expect("ok");
        let ResourceContent::Json(value) = content;
        assert_eq!(
            value.get("instrument").and_then(|v| v.as_str()),
            Some("BTC-31MAY24-50000-C")
        );
        assert_eq!(
            value.get("mark_price").and_then(|v| v.as_f64()),
            Some(50_000.5)
        );
        assert_eq!(value.get("delta").and_then(|v| v.as_f64()), Some(0.55));
        assert_eq!(value.get("gamma").and_then(|v| v.as_f64()), Some(0.0001));
        assert_eq!(value.get("vega").and_then(|v| v.as_f64()), Some(12.3));
    }

    #[tokio::test]
    async fn read_live_ticker_perp_omits_greeks() {
        use crate::resources::live::{SubscriptionProvider, SubscriptionStream};
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Arc;

        struct StubProvider;
        impl SubscriptionProvider for StubProvider {
            fn subscribe(
                &self,
                _uri: ResourceUri,
            ) -> Pin<Box<dyn Future<Output = Result<SubscriptionStream, AdapterError>> + Send + '_>>
            {
                Box::pin(async move {
                    let frame = serde_json::json!({
                        "mark_price": 50_000.5,
                        "best_bid_price": 50_000.0,
                        "best_ask_price": 50_001.0,
                        "last_price": 49_999.5,
                        "timestamp": 1_700_000_000_000_i64,
                    });
                    let stream = futures_util::stream::iter(vec![Ok::<_, AdapterError>(frame)]);
                    Ok(Box::pin(stream) as SubscriptionStream)
                })
            }
        }

        let registry = ResourceRegistry::build().with_subscription_provider(Arc::new(StubProvider));
        let uri = ResourceUri::Ticker {
            instrument: "BTC-PERPETUAL".to_string(),
        };
        let content = registry.read(&ctx(), &uri).await.expect("ok");
        let ResourceContent::Json(value) = content;
        // `skip_serializing_if = "Option::is_none"` keeps absent
        // fields out of the JSON payload entirely.
        assert!(value.get("delta").is_none());
        assert!(value.get("gamma").is_none());
        assert!(value.get("vega").is_none());
        assert!(value.get("mark_iv").is_none());
    }
}
