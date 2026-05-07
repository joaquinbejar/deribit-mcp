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

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

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

    /// Snapshot of the rolling history (newest first). Capped at
    /// [`HISTORY_CAPACITY`] frames so a long-running subscription
    /// does not grow unbounded. Used by the trades resource to
    /// return the last N trades; book / ticker callers ignore it
    /// and rely on `latest()`.
    pub async fn history(&self) -> Vec<Value> {
        self.entry.history.lock().await.iter().cloned().collect()
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
    /// Upstream channel name (e.g. `book.BTC-PERPETUAL.raw`).
    pub channel: String,
    /// Number of [`SubscriptionHandle`]s currently held.
    pub refcount: AtomicU64,
    /// Latest decoded snapshot. Book / ticker use this; trades
    /// resource reads from [`Self::history`] instead.
    pub latest: Mutex<Option<Value>>,
    /// Rolling history of the last [`HISTORY_CAPACITY`] decoded
    /// snapshots, newest first. Trades-channel readers consume
    /// this; book / ticker callers ignore it and use `latest`.
    pub history: Mutex<VecDeque<Value>>,
    /// Fires on every new snapshot. Subscribers count is bounded by
    /// the registry's broadcast capacity.
    pub broadcast: broadcast::Sender<()>,
    /// Per-entry shutdown signal — flipped when the refcount hits
    /// zero so the upstream reader task drops cleanly.
    pub cancel: CancellationToken,
    /// Last instant the notification sink was fired for this URI.
    /// Used by the throttle gate.
    last_notified: Mutex<Option<Instant>>,
}

/// Sink that the live registry calls every time a subscribed URI
/// produces a new (post-throttle) snapshot.
///
/// The MCP server impl wires this to `Peer::notify_resource_updated`
/// so the connected client sees `notifications/resources/updated`
/// per the MCP 2025-06-18 spec; tests pass a stub that just counts
/// the calls.
pub trait NotificationSink: Send + Sync + 'static {
    /// Fired after the throttle gate. URI is borrowed to avoid
    /// per-frame clones; the sink can clone if it needs to own.
    fn notify(&self, uri: &ResourceUri);
}

/// Default per-URI notification rate. Live frames can fire much
/// faster — `book.<i>.raw` runs at the upstream's natural cadence
/// — and an MCP client almost never benefits from > 10 / s. The
/// throttle coalesces intermediate frames; the latest snapshot is
/// what `read` returns.
pub const DEFAULT_NOTIFY_INTERVAL: Duration = Duration::from_millis(100);

/// Maximum *frames* retained in [`SubscriptionEntry::history`].
///
/// The v0.3-04 trades-resource contract is "last ~32 trade events",
/// so 32 frames is conservative — each upstream `trades.*.raw`
/// frame typically carries 1..few trade objects, the resource
/// flattens across frames, and the read still truncates to the
/// same `HISTORY_CAPACITY`. Book / ticker subscribers do not
/// consult the history at all; the per-entry overhead is small.
pub const HISTORY_CAPACITY: usize = 32;

/// Shared inner state of [`LiveRegistry`]. Held behind an `Arc` so a
/// [`SubscriptionHandle`]'s `Drop` can spawn a teardown task that
/// outlives the calling stack frame.
struct LiveRegistryShared {
    entries: RwLock<HashMap<ResourceUri, Arc<SubscriptionEntry>>>,
    sink: RwLock<Option<Arc<dyn NotificationSink>>>,
    notify_interval: RwLock<Duration>,
}

impl Default for LiveRegistryShared {
    fn default() -> Self {
        Self {
            entries: RwLock::default(),
            sink: RwLock::new(None),
            notify_interval: RwLock::new(DEFAULT_NOTIFY_INTERVAL),
        }
    }
}

impl std::fmt::Debug for LiveRegistryShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveRegistryShared")
            .field("entries", &"<HashMap>")
            .field("sink", &"<Option<dyn NotificationSink>>")
            .field("notify_interval", &"<RwLock<Duration>>")
            .finish()
    }
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

    /// Install (or replace) the notification sink. Subsequent
    /// frames fire `sink.notify(uri)` post-throttle. Pass `None`
    /// to detach the sink (a fresh registry has none).
    pub async fn set_notification_sink(&self, sink: Option<Arc<dyn NotificationSink>>) {
        *self.inner.sink.write().await = sink;
    }

    /// Override the per-URI notification interval. The default is
    /// [`DEFAULT_NOTIFY_INTERVAL`] (100 ms ≈ 10 Hz). Smaller values
    /// produce more notifications; `Duration::ZERO` disables the
    /// throttle entirely.
    pub async fn set_notify_interval(&self, interval: Duration) {
        *self.inner.notify_interval.write().await = interval;
    }

    /// Snapshot the per-URI notification interval.
    pub async fn notify_interval(&self) -> Duration {
        *self.inner.notify_interval.read().await
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
            history: Mutex::new(VecDeque::with_capacity(HISTORY_CAPACITY)),
            broadcast: broadcast_tx.clone(),
            cancel: cancel.clone(),
            last_notified: Mutex::new(None),
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
        let task_uri = uri.clone();
        let task_shared = self.inner.clone();
        tokio::spawn(async move {
            use futures_util::StreamExt;
            loop {
                tokio::select! {
                    biased;
                    _ = task_entry.cancel.cancelled() => break,
                    item = stream.next() => match item {
                        Some(Ok(value)) => {
                            {
                                let mut history = task_entry.history.lock().await;
                                if history.len() == HISTORY_CAPACITY {
                                    history.pop_back();
                                }
                                history.push_front(value.clone());
                            }
                            *task_entry.latest.lock().await = Some(value);
                            // Errors from `send` mean no receivers — fine.
                            let _ = task_entry.broadcast.send(());

                            // Throttle the notification sink at
                            // `notify_interval`. Default 100 ms ≈
                            // 10 Hz; intermediate frames are
                            // coalesced — the next read returns
                            // the latest snapshot.
                            maybe_notify(&task_entry, &task_uri, &task_shared).await;
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

/// Apply the notification throttle and, if eligible, fire the sink.
async fn maybe_notify(entry: &SubscriptionEntry, uri: &ResourceUri, shared: &LiveRegistryShared) {
    let interval = *shared.notify_interval.read().await;
    let mut last = entry.last_notified.lock().await;
    let now = Instant::now();
    let should_emit = match *last {
        // `Duration::ZERO` disables the throttle.
        _ if interval.is_zero() => true,
        Some(prev) => now.saturating_duration_since(prev) >= interval,
        None => true,
    };
    if !should_emit {
        return;
    }
    *last = Some(now);
    drop(last);
    if let Some(sink) = shared.sink.read().await.as_ref().cloned() {
        sink.notify(uri);
    }
}

/// Map a live `ResourceUri` to its upstream Deribit channel name.
///
/// Per-resource defaults:
///
/// - `book` → `book.<instrument>.raw` (uncoalesced — v0.3-02 ships
///   the raw channel; v0.4+ may expose aggregated variants).
/// - `ticker` → `ticker.<instrument>.100ms` (throttled, the
///   appropriate cadence for LLM consumption).
/// - `trades` → `trades.<instrument>.raw` per the v0.3-04 spec.
pub fn channel_name_for(uri: &ResourceUri) -> String {
    match uri {
        ResourceUri::Book { instrument } => format!("book.{instrument}.raw"),
        ResourceUri::Ticker { instrument } => format!("ticker.{instrument}.100ms"),
        ResourceUri::Trades { instrument } => format!("trades.{instrument}.raw"),
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
    /// Snapshot timestamp, Unix epoch milliseconds. `None` when
    /// the upstream omits it (rare in practice — the documented
    /// channel always sets a timestamp — but a permissive decoder
    /// is safer than a fabricated `0`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
}

impl TickerSnapshot {
    /// Decode an upstream `ticker.<instrument>.100ms` WS frame
    /// payload. Permissive — every numeric field is optional and
    /// upstream omissions become `None` (including `timestamp`).
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
        let timestamp = obj.get("timestamp").and_then(Value::as_i64);
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

/// One trade event from `trades.<instrument>.raw`.
///
/// `direction` and `liquidation` carry the upstream string
/// verbatim; `tick_direction` is the upstream integer (`0..=3`)
/// also passed through unchanged. The resource layer does not
/// enumerate these as Rust enums to keep the wire shape
/// pass-through. Adding closed-set decoders is a straight
/// follow-up if the LLM client needs typed checks.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct TradeUpdate {
    /// Trade direction (`buy` / `sell`).
    pub direction: String,
    /// Trade price.
    pub price: f64,
    /// Trade amount (contracts or coins, per Deribit docs).
    pub amount: f64,
    /// Upstream trade id (string per Deribit spec).
    pub trade_id: String,
    /// Trade timestamp, Unix epoch milliseconds.
    pub timestamp: i64,
    /// Liquidation marker (`""` / `M` / `T` / `MT`) when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liquidation: Option<String>,
    /// Tick direction (`0` / `1` / `2` / `3`) when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick_direction: Option<i64>,
    /// Mark price at trade time, when carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_price: Option<f64>,
    /// Index price at trade time, when carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_price: Option<f64>,
}

impl TradeUpdate {
    /// Decode one upstream trade frame element. Permissive — the
    /// resource read layer skips elements that do not decode rather
    /// than failing the whole call.
    fn from_value(value: &Value) -> Option<Self> {
        let obj = value.as_object()?;
        let direction = obj.get("direction").and_then(Value::as_str)?.to_string();
        let price = obj.get("price").and_then(Value::as_f64)?;
        let amount = obj.get("amount").and_then(Value::as_f64)?;
        let trade_id = obj.get("trade_id").and_then(|v| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })?;
        let timestamp = obj.get("timestamp").and_then(Value::as_i64)?;
        Some(Self {
            direction,
            price,
            amount,
            trade_id,
            timestamp,
            liquidation: obj
                .get("liquidation")
                .and_then(Value::as_str)
                .map(str::to_string),
            tick_direction: obj.get("tick_direction").and_then(Value::as_i64),
            mark_price: obj.get("mark_price").and_then(Value::as_f64),
            index_price: obj.get("index_price").and_then(Value::as_f64),
        })
    }

    /// Decode an upstream `trades.<instrument>.raw` channel frame.
    /// The frame's payload is a JSON array of trade objects;
    /// elements that fail to decode are dropped (the LLM still
    /// sees the rest).
    ///
    /// # Errors
    ///
    /// Returns [`AdapterError::Validation`] when the payload is
    /// not a JSON array.
    pub fn batch_from_value(value: &Value) -> Result<Vec<Self>, AdapterError> {
        let array = value
            .as_array()
            .ok_or_else(|| AdapterError::validation("trades", "expected JSON array"))?;
        Ok(array.iter().filter_map(Self::from_value).collect())
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

    /// Sink that records every notify into a vec for the test
    /// to inspect.
    #[derive(Default, Clone)]
    struct CountingSink {
        calls: Arc<std::sync::Mutex<Vec<ResourceUri>>>,
    }
    impl NotificationSink for CountingSink {
        fn notify(&self, uri: &ResourceUri) {
            self.calls.lock().unwrap().push(uri.clone());
        }
    }

    #[tokio::test]
    async fn notification_sink_fires_after_first_frame() {
        let provider = StubProvider::with(book_btc(), vec![serde_json::json!({"v": 1})]);
        let registry = LiveRegistry::new();
        let sink = CountingSink::default();
        registry
            .set_notification_sink(Some(Arc::new(sink.clone())))
            .await;
        // Disable the throttle so the test does not have to wait.
        registry.set_notify_interval(Duration::ZERO).await;

        let _handle = registry.subscribe(&provider, &book_btc()).await.unwrap();
        for _ in 0..50 {
            if !sink.calls.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            !sink.calls.lock().unwrap().is_empty(),
            "expected at least one notification"
        );
    }

    #[tokio::test]
    async fn notification_throttle_coalesces_burst_into_single_emit() {
        // Provider emits a hundred frames in immediate succession.
        let frames: Vec<Value> = (0..100).map(|i| serde_json::json!({"v": i})).collect();
        let provider = StubProvider::with(book_btc(), frames);
        let registry = LiveRegistry::new();
        let sink = CountingSink::default();
        registry
            .set_notification_sink(Some(Arc::new(sink.clone())))
            .await;
        // 1 s interval — at default cadence the 100 frames would
        // fire 100 notifications. The throttle must coalesce them
        // into a single emit (the first frame wins; the rest are
        // suppressed within the window).
        registry.set_notify_interval(Duration::from_secs(1)).await;

        let _handle = registry.subscribe(&provider, &book_btc()).await.unwrap();
        // Give the reader task a beat to drain all 100 frames.
        for _ in 0..50 {
            if !sink.calls.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // Wait an extra ~50 ms to be sure the throttle gate does
        // NOT release a second emit within the 1 s window.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let n = sink.calls.lock().unwrap().len();
        assert_eq!(
            n, 1,
            "throttle must coalesce 100-frame burst into a single notification (got {n})"
        );
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
            "trades.BTC-31MAY24-50000-C.raw"
        );
    }
}
