//! Live (WebSocket-backed, subscribable) resources.
//!
//! Subscriptions are fanned out from one upstream `deribit-websocket`
//! channel reader to N MCP subscribers; reference-counted teardown.
//!
//! v0.3-01 ships the [`LiveRegistry`] — refcount + lifecycle scaffold.
//! The actual book / ticker / trades subscription wiring lands in
//! v0.3-02 / v0.3-03 / v0.3-04. The registry talks to a
//! [`SubscriptionProvider`] trait so unit tests can drive a stub
//! channel without standing up a real WebSocket.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use futures_core::Stream;
use serde_json::Value;
use tokio::sync::{Mutex, RwLock, broadcast};
use tokio_util::sync::CancellationToken;

use super::ResourceUri;
use crate::error::AdapterError;

/// Stream of decoded JSON snapshots emitted by an upstream
/// subscription channel. Each item is one channel message after
/// the upstream has decoded the JSON-RPC envelope and unwrapped
/// the per-channel payload.
pub type SubscriptionStream =
    Pin<Box<dyn Stream<Item = Result<Value, AdapterError>> + Send + 'static>>;

/// Opens an upstream WebSocket channel for the given URI and yields
/// decoded snapshots. The trait abstracts the v0.3-02 / -03 / -04
/// real `deribit-websocket` wiring so the registry can be unit-tested
/// against a stub.
///
/// Implementations must surface upstream auth / network / decode
/// failures as [`AdapterError`] variants — the registry treats the
/// stream as opaque otherwise.
pub trait SubscriptionProvider: Send + Sync + 'static {
    /// Open a channel for the given live URI.
    ///
    /// # Errors
    ///
    /// Returns whatever [`AdapterError`] the upstream surfaces
    /// (auth, network, validation if the URI is malformed, …).
    fn subscribe(
        &self,
        uri: ResourceUri,
    ) -> Pin<Box<dyn Future<Output = Result<SubscriptionStream, AdapterError>> + Send + '_>>;
}

/// A handle held by a subscriber. Increments the refcount on
/// construction, decrements on `Drop`. When the refcount reaches
/// zero the registry tears the upstream subscription down via the
/// per-entry [`CancellationToken`].
pub struct SubscriptionHandle {
    uri: ResourceUri,
    entry: Arc<SubscriptionEntry>,
    registry: Arc<LiveRegistryShared>,
}

impl SubscriptionHandle {
    /// URI this handle was opened against.
    #[must_use]
    pub fn uri(&self) -> &ResourceUri {
        &self.uri
    }

    /// Subscribe to the per-channel broadcast that fires every time
    /// the registry receives a new snapshot. The receiver fires a
    /// unit `()` per update; readers consult `latest()` for the
    /// payload.
    #[must_use]
    pub fn updates(&self) -> broadcast::Receiver<()> {
        self.entry.broadcast.subscribe()
    }

    /// Most recent decoded snapshot, if any has arrived.
    pub async fn latest(&self) -> Option<Value> {
        self.entry.latest.lock().await.clone()
    }
}

impl Drop for SubscriptionHandle {
    fn drop(&mut self) {
        // `fetch_sub` would wrap on a 0 → `u64::MAX` underflow;
        // assert the invariant up front so the failure is loud
        // rather than silent. The registry's `subscribe` always
        // hands a handle out at refcount ≥ 1, so the only way to
        // hit `prev == 0` is a programmer error (double-Drop /
        // forged handle).
        let prev = self.entry.refcount.fetch_sub(1, Ordering::AcqRel);
        assert!(
            prev >= 1,
            "live subscription refcount underflow on drop (uri = {:?})",
            self.uri
        );
        if prev == 1 {
            // Remove the map entry under the write lock BEFORE
            // signalling cancel so a racing `subscribe` cannot
            // attach to a cancelled entry between drop and the
            // deferred map cleanup. We can't `await` from `Drop`,
            // so the lock + cancel + reader teardown all run on a
            // spawned task — but the spawned task observes the
            // refcount-still-zero invariant under the write lock
            // and skips removal if a new subscribe has already
            // re-incremented past it.
            let registry = self.registry.clone();
            let uri = self.uri.clone();
            tokio::spawn(async move {
                let mut map = registry.entries.write().await;
                let still_zero = map
                    .get(&uri)
                    .is_some_and(|e| e.refcount.load(Ordering::Acquire) == 0);
                if still_zero {
                    if let Some(entry) = map.remove(&uri) {
                        entry.cancel.cancel();
                    }
                }
            });
        }
    }
}

/// One per-URI subscription. Public so v0.3-02 / -03 / -04 can build
/// the entry types they need (book diff state, ticker latest, trade
/// ring buffer) on top.
#[derive(Debug)]
pub struct SubscriptionEntry {
    /// Upstream channel name (e.g. `book.BTC-PERPETUAL.100ms`).
    pub channel: String,
    /// Number of [`SubscriptionHandle`]s currently held.
    pub refcount: AtomicU64,
    /// Latest decoded snapshot.
    pub latest: Mutex<Option<Value>>,
    /// Fires on every new snapshot. Subscribers count is bounded by
    /// the registry's broadcast capacity.
    pub broadcast: broadcast::Sender<()>,
    /// Per-entry shutdown signal — flipped when the refcount hits
    /// zero so the upstream reader task drops cleanly.
    pub cancel: CancellationToken,
}

/// Shared inner state of [`LiveRegistry`]. Held behind an `Arc` so a
/// [`SubscriptionHandle`]'s `Drop` can spawn a teardown task that
/// outlives the calling stack frame.
#[derive(Debug, Default)]
struct LiveRegistryShared {
    entries: RwLock<HashMap<ResourceUri, Arc<SubscriptionEntry>>>,
}

/// Refcounted registry of live resource subscriptions.
#[derive(Debug, Default, Clone)]
pub struct LiveRegistry {
    inner: Arc<LiveRegistryShared>,
}

impl LiveRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct URIs currently subscribed (live entries).
    pub async fn len(&self) -> usize {
        self.inner.entries.read().await.len()
    }

    /// Whether any URI is currently subscribed.
    pub async fn is_empty(&self) -> bool {
        self.inner.entries.read().await.is_empty()
    }

    /// Refcount for `uri`, or `0` if no entry exists.
    pub async fn refcount(&self, uri: &ResourceUri) -> u64 {
        match self.inner.entries.read().await.get(uri) {
            Some(entry) => entry.refcount.load(Ordering::Acquire),
            None => 0,
        }
    }

    /// Open (or attach to) a subscription for `uri`. Returns a
    /// [`SubscriptionHandle`] whose `Drop` decrements the refcount;
    /// the upstream channel closes when the count returns to zero.
    ///
    /// First reader spawns the upstream reader task via
    /// [`SubscriptionProvider::subscribe`]. Subsequent readers
    /// share the cached entry.
    ///
    /// # Errors
    ///
    /// Surfaces whatever [`AdapterError`] the provider returns when
    /// it has to open a fresh subscription. Subsequent attaches are
    /// infallible at the registry level.
    pub async fn subscribe<P: SubscriptionProvider + ?Sized>(
        &self,
        provider: &P,
        uri: &ResourceUri,
    ) -> Result<SubscriptionHandle, AdapterError> {
        // Fast path: entry already exists *and* is still live. An
        // entry whose `cancel` was fired (because Drop teardown is
        // mid-flight but the deferred map cleanup hasn't run yet)
        // does not count — we'd otherwise hand out a handle to a
        // dead reader task. Treat as absent and fall through.
        if let Some(entry) = self.inner.entries.read().await.get(uri).cloned() {
            if !entry.cancel.is_cancelled() {
                return Ok(self.attach(uri.clone(), entry));
            }
        }

        // Slow path: open the upstream stream first, then publish
        // the entry under the write lock. We accept that two callers
        // racing on a fresh URI may both open a stream; the second
        // detects the entry under the write lock and drops its own
        // freshly-opened stream.
        let mut stream = provider.subscribe(uri.clone()).await?;
        let cancel = CancellationToken::new();
        let (broadcast_tx, _) = broadcast::channel::<()>(BROADCAST_CAPACITY);

        let entry = Arc::new(SubscriptionEntry {
            channel: channel_name_for(uri),
            refcount: AtomicU64::new(0),
            latest: Mutex::new(None),
            broadcast: broadcast_tx.clone(),
            cancel: cancel.clone(),
        });

        {
            let mut map = self.inner.entries.write().await;
            if let Some(existing) = map.get(uri).cloned() {
                // Lost the race against another subscriber that
                // got the write lock first AND is still live.
                if !existing.cancel.is_cancelled() {
                    cancel.cancel();
                    drop(stream);
                    return Ok(self.attach(uri.clone(), existing));
                }
                // The existing entry is mid-teardown; replace it.
                map.remove(uri);
            }
            map.insert(uri.clone(), entry.clone());
        }

        // Spawn the reader. The task captures the entry's `latest`
        // / `broadcast` / `cancel` and exits cleanly on cancel or on
        // upstream stream end.
        let task_entry = entry.clone();
        tokio::spawn(async move {
            use futures_util::StreamExt;
            loop {
                tokio::select! {
                    biased;
                    _ = task_entry.cancel.cancelled() => break,
                    item = stream.next() => match item {
                        Some(Ok(value)) => {
                            *task_entry.latest.lock().await = Some(value);
                            // Errors from `send` mean no receivers — fine.
                            let _ = task_entry.broadcast.send(());
                        }
                        Some(Err(err)) => {
                            tracing::warn!(error = %err, "live subscription stream error; closing");
                            break;
                        }
                        None => break,
                    },
                }
            }
        });

        Ok(self.attach(uri.clone(), entry))
    }

    fn attach(&self, uri: ResourceUri, entry: Arc<SubscriptionEntry>) -> SubscriptionHandle {
        // We need ~1.8e19 outstanding handles to overflow a `u64`,
        // so this panic only fires on a process-wide leak — surface
        // it fast rather than silently wrap. `checked_add` returns
        // `None` (not a saturating value) on overflow, so the
        // assertion catches both directions.
        let prev = entry.refcount.fetch_add(1, Ordering::AcqRel);
        assert!(
            prev.checked_add(1).is_some(),
            "live subscription refcount overflowed u64"
        );
        SubscriptionHandle {
            uri,
            entry,
            registry: self.inner.clone(),
        }
    }
}

/// Map a live `ResourceUri` to its upstream Deribit channel name.
///
/// Per-resource defaults:
///
/// - `book` → `book.<instrument>.raw` (uncoalesced — v0.3-02 ships
///   the raw channel; v0.4+ may expose aggregated variants).
/// - `ticker` → `ticker.<instrument>.100ms`.
/// - `trades` → `trades.<instrument>.100ms`.
pub fn channel_name_for(uri: &ResourceUri) -> String {
    match uri {
        ResourceUri::Book { instrument } => format!("book.{instrument}.raw"),
        ResourceUri::Ticker { instrument } => format!("ticker.{instrument}.100ms"),
        ResourceUri::Trades { instrument } => format!("trades.{instrument}.100ms"),
        // Static URIs never reach the live registry; the parser
        // and dispatcher route them elsewhere. Use a stable string
        // so debug output is informative if a future refactor
        // hands a wrong URI in.
        ResourceUri::Currencies => "currencies".to_string(),
        ResourceUri::Instruments { currency } => format!("instruments.{currency}"),
    }
}

/// Order-book snapshot exposed via `deribit://book/{instrument}`.
///
/// Mirrors the `book.<instrument>.raw` Deribit WebSocket channel
/// payload; decoded snapshots are stored as `serde_json::Value` in
/// [`SubscriptionEntry::latest`] and structured into this DTO at the
/// resource read boundary.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct BookSnapshot {
    /// Instrument identifier (`BTC-PERPETUAL`, `BTC-31MAY24-50000-C`, …).
    pub instrument: String,
    /// Bid side as `(price, size)` pairs sorted high-to-low. May be
    /// empty on the first frame of a delta subscription.
    pub bids: Vec<(f64, f64)>,
    /// Ask side as `(price, size)` pairs sorted low-to-high.
    pub asks: Vec<(f64, f64)>,
    /// Sequence id of this snapshot. Monotonically increases per
    /// channel; use to dedupe / detect resync.
    pub change_id: u64,
    /// Snapshot timestamp, Unix epoch milliseconds.
    pub timestamp: i64,
}

impl BookSnapshot {
    /// Decode an upstream `book.<instrument>.raw` WS frame payload
    /// (the inner `data` object after the JSON-RPC envelope is
    /// stripped). Permissive: missing optional fields default to
    /// 0 / empty rather than failing the call — the LLM gets a
    /// best-effort snapshot.
    ///
    /// **Levels are skipped, not rejected.** A bid / ask entry
    /// that is not a 2- or 3-element array of numbers (e.g. a
    /// stray `null`, an op-only delta, …) is dropped silently and
    /// the rest of the side is decoded. The payload as a whole
    /// must still be a JSON object.
    ///
    /// # Errors
    ///
    /// Returns [`AdapterError::Validation`] only when the payload
    /// is not a JSON object.
    pub fn from_value(instrument: &str, value: &Value) -> Result<Self, AdapterError> {
        let obj = value
            .as_object()
            .ok_or_else(|| AdapterError::validation("book", "expected JSON object"))?;
        let bids = decode_levels(obj.get("bids"))?;
        let asks = decode_levels(obj.get("asks"))?;
        let change_id = obj
            .get("change_id")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let timestamp = obj
            .get("timestamp")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        Ok(Self {
            instrument: instrument.to_string(),
            bids,
            asks,
            change_id,
            timestamp,
        })
    }
}

/// Ticker snapshot exposed via `deribit://ticker/{instrument}`.
///
/// Mirrors the throttled `ticker.<instrument>.100ms` Deribit
/// WebSocket channel payload. Optional fields stay `None` when the
/// upstream omits them — perpetuals / futures do not carry the
/// greeks; spot may not carry `mark_price`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct TickerSnapshot {
    /// Instrument identifier.
    pub instrument: String,
    /// Mark price (perp / future / option).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_price: Option<f64>,
    /// Underlying index price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_price: Option<f64>,
    /// Best bid price on the order book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_bid_price: Option<f64>,
    /// Best ask price on the order book.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_ask_price: Option<f64>,
    /// Last traded price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_price: Option<f64>,
    /// Mark implied volatility (options only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_iv: Option<f64>,
    /// Black-Scholes delta (options only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<f64>,
    /// Black-Scholes gamma (options only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamma: Option<f64>,
    /// Black-Scholes vega (options only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vega: Option<f64>,
    /// Snapshot timestamp, Unix epoch milliseconds.
    pub timestamp: i64,
}

impl TickerSnapshot {
    /// Decode an upstream `ticker.<instrument>.100ms` WS frame
    /// payload. Permissive — every numeric field is optional and
    /// upstream omissions become `None`.
    ///
    /// **Greeks** (`delta`, `gamma`, `vega`) live inside an inner
    /// `greeks` object on options frames; this decoder pulls them
    /// up to the top level so the JSON shape the LLM sees is flat.
    ///
    /// # Errors
    ///
    /// Returns [`AdapterError::Validation`] only when the payload
    /// is not a JSON object.
    pub fn from_value(instrument: &str, value: &Value) -> Result<Self, AdapterError> {
        let obj = value
            .as_object()
            .ok_or_else(|| AdapterError::validation("ticker", "expected JSON object"))?;
        let f64_at = |key: &str| obj.get(key).and_then(Value::as_f64);
        let timestamp = obj
            .get("timestamp")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let greeks = obj.get("greeks").and_then(Value::as_object);
        let greek = |key: &str| greeks.and_then(|g| g.get(key)).and_then(Value::as_f64);
        Ok(Self {
            instrument: instrument.to_string(),
            mark_price: f64_at("mark_price"),
            index_price: f64_at("index_price"),
            best_bid_price: f64_at("best_bid_price"),
            best_ask_price: f64_at("best_ask_price"),
            last_price: f64_at("last_price"),
            mark_iv: f64_at("mark_iv"),
            delta: greek("delta"),
            gamma: greek("gamma"),
            vega: greek("vega"),
            timestamp,
        })
    }
}

/// Decode a side of the order book — Deribit emits each level as
/// `[op, price, size]` (delta) or `[price, size]` (snapshot). We
/// keep only `(price, size)` and discard the operation marker; the
/// LLM consumer just wants the current shape.
fn decode_levels(value: Option<&Value>) -> Result<Vec<(f64, f64)>, AdapterError> {
    let Some(array) = value.and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(array.len());
    for level in array {
        let Some(items) = level.as_array() else {
            continue;
        };
        let (price, size) = match items.as_slice() {
            [_op, price, size] => (price.as_f64(), size.as_f64()),
            [price, size] => (price.as_f64(), size.as_f64()),
            _ => continue,
        };
        if let (Some(price), Some(size)) = (price, size) {
            out.push((price, size));
        }
    }
    Ok(out)
}

/// Capacity of the per-entry broadcast channel. 64 is enough to
/// absorb a reader briefly stalling without dropping updates;
/// long-lived stalls do drop and the receiver sees `Lagged(n)` on
/// next recv.
const BROADCAST_CAPACITY: usize = 64;

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    /// In-memory provider that hands out a precomputed sequence of
    /// items per URI and counts how many times `subscribe` was
    /// called. Used to verify refcount → "subscribe-once" semantics.
    #[derive(Default)]
    struct StubProvider {
        items: StdMutex<HashMap<ResourceUri, Vec<Value>>>,
        opened: AtomicU64,
    }

    impl StubProvider {
        fn with(uri: ResourceUri, items: Vec<Value>) -> Self {
            let p = StubProvider::default();
            p.items.lock().unwrap().insert(uri, items);
            p
        }
    }

    impl SubscriptionProvider for StubProvider {
        fn subscribe(
            &self,
            uri: ResourceUri,
        ) -> Pin<Box<dyn Future<Output = Result<SubscriptionStream, AdapterError>> + Send + '_>>
        {
            self.opened.fetch_add(1, Ordering::AcqRel);
            let items = self
                .items
                .lock()
                .unwrap()
                .get(&uri)
                .cloned()
                .unwrap_or_default();
            Box::pin(async move {
                let s = stream::iter(items.into_iter().map(Ok::<_, AdapterError>));
                Ok(Box::pin(s) as SubscriptionStream)
            })
        }
    }

    fn book_btc() -> ResourceUri {
        ResourceUri::Book {
            instrument: "BTC-PERPETUAL".to_string(),
        }
    }

    #[tokio::test]
    async fn first_subscribe_opens_upstream() {
        let provider = StubProvider::with(book_btc(), vec![serde_json::json!({"snap": 1})]);
        let registry = LiveRegistry::new();

        let _handle = registry
            .subscribe(&provider, &book_btc())
            .await
            .expect("subscribe");
        assert_eq!(registry.refcount(&book_btc()).await, 1);
        assert_eq!(provider.opened.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn second_subscribe_reuses_entry() {
        let provider = StubProvider::with(book_btc(), vec![]);
        let registry = LiveRegistry::new();

        let _h1 = registry.subscribe(&provider, &book_btc()).await.unwrap();
        let _h2 = registry.subscribe(&provider, &book_btc()).await.unwrap();

        assert_eq!(registry.refcount(&book_btc()).await, 2);
        assert_eq!(
            provider.opened.load(Ordering::Acquire),
            1,
            "upstream should have been opened exactly once"
        );
    }

    #[tokio::test]
    async fn dropping_last_handle_closes_upstream_and_removes_entry() {
        let provider = StubProvider::with(book_btc(), vec![]);
        let registry = LiveRegistry::new();

        let h1 = registry.subscribe(&provider, &book_btc()).await.unwrap();
        let h2 = registry.subscribe(&provider, &book_btc()).await.unwrap();
        assert_eq!(registry.refcount(&book_btc()).await, 2);

        drop(h2);
        // Drop is sync; refcount is updated synchronously.
        assert_eq!(registry.refcount(&book_btc()).await, 1);

        drop(h1);
        // The map-removal is spawned; give the scheduler a beat.
        for _ in 0..20 {
            if registry.is_empty().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(registry.is_empty().await);
        assert_eq!(registry.refcount(&book_btc()).await, 0);
    }

    #[tokio::test]
    async fn updates_broadcast_fires_and_latest_carries_payload() {
        // Provider that opens the stream but suspends until we
        // explicitly release it, so the test can attach the
        // broadcast receiver *before* any updates can fire.
        struct GatedProvider {
            release: Arc<tokio::sync::Notify>,
        }
        impl SubscriptionProvider for GatedProvider {
            fn subscribe(
                &self,
                _uri: ResourceUri,
            ) -> Pin<Box<dyn Future<Output = Result<SubscriptionStream, AdapterError>> + Send + '_>>
            {
                let release = self.release.clone();
                Box::pin(async move {
                    let s = async_stream::stream! {
                        release.notified().await;
                        yield Ok::<_, AdapterError>(serde_json::json!({"v": 1}));
                        yield Ok::<_, AdapterError>(serde_json::json!({"v": 2}));
                    };
                    Ok(Box::pin(s) as SubscriptionStream)
                })
            }
        }

        let release = Arc::new(tokio::sync::Notify::new());
        let provider = GatedProvider {
            release: release.clone(),
        };
        let registry = LiveRegistry::new();

        let handle = registry.subscribe(&provider, &book_btc()).await.unwrap();
        // Receiver must be in place BEFORE the reader fires its
        // first send — broadcast::Receiver does not buffer past
        // messages, so a pre-attach send is missed.
        let mut updates = handle.updates();
        release.notify_one();

        // Wait for the reader task to drain the stub stream.
        for _ in 0..50 {
            if handle.latest().await.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let latest = handle.latest().await.expect("latest set");
        assert!(latest.get("v").is_some());

        let signal = tokio::time::timeout(Duration::from_millis(500), updates.recv()).await;
        assert!(signal.is_ok(), "expected at least one update signal");
    }

    #[test]
    fn channel_names_match_deribit_taxonomy() {
        assert_eq!(
            channel_name_for(&ResourceUri::Book {
                instrument: "BTC-PERPETUAL".to_string()
            }),
            "book.BTC-PERPETUAL.raw"
        );
        assert_eq!(
            channel_name_for(&ResourceUri::Ticker {
                instrument: "ETH-PERPETUAL".to_string()
            }),
            "ticker.ETH-PERPETUAL.100ms"
        );
        assert_eq!(
            channel_name_for(&ResourceUri::Trades {
                instrument: "BTC-31MAY24-50000-C".to_string()
            }),
            "trades.BTC-31MAY24-50000-C.100ms"
        );
    }
}
