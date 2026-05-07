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
        let prev = self.entry.refcount.fetch_sub(1, Ordering::AcqRel);
        // `prev == 0` would mean we already underflowed; the
        // registry's `subscribe` guarantees we never decrement
        // below 1 here. Use `checked_sub` semantics by guarding the
        // teardown branch on `prev == 1` (i.e. count is now 0).
        if prev == 1 {
            self.entry.cancel.cancel();
            let registry = self.registry.clone();
            let uri = self.uri.clone();
            // Drop the map entry on a background task so we do not
            // hold the write lock from a `Drop` impl.
            tokio::spawn(async move {
                let mut map = registry.entries.write().await;
                if let Some(entry) = map.get(&uri) {
                    if entry.refcount.load(Ordering::Acquire) == 0 {
                        map.remove(&uri);
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
        // Fast path: entry already exists.
        if let Some(entry) = self.inner.entries.read().await.get(uri).cloned() {
            return Ok(self.attach(uri.clone(), entry));
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
                // Lost the race — drop our cancel + stream and
                // attach to the winner.
                cancel.cancel();
                drop(stream);
                return Ok(self.attach(uri.clone(), existing));
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
        // `checked_add` on a `u64` only saturates at `u64::MAX`; we
        // would have ~1.8e19 outstanding handles for that to fire.
        // Treat it as a hard panic — the only way to reach it is
        // a process-wide leak, and the panic surfaces it fast.
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
/// `book.<instrument>.100ms` / `ticker.<instrument>.100ms` /
/// `trades.<instrument>.100ms` per the Deribit channel taxonomy
/// (the `100ms` aggregation is the safe default; v0.3-02..04 may
/// expose alternative aggregations).
fn channel_name_for(uri: &ResourceUri) -> String {
    match uri {
        ResourceUri::Book { instrument } => format!("book.{instrument}.100ms"),
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
        let provider = StubProvider::with(
            book_btc(),
            vec![serde_json::json!({"v": 1}), serde_json::json!({"v": 2})],
        );
        let registry = LiveRegistry::new();

        let handle = registry.subscribe(&provider, &book_btc()).await.unwrap();
        let mut updates = handle.updates();

        // Wait for the reader task to drain the stub stream.
        for _ in 0..50 {
            if handle.latest().await.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let latest = handle.latest().await.expect("latest set");
        assert!(latest.get("v").is_some());

        // The broadcast channel buffers the historical signals
        // emitted before we attached, so we should receive at least
        // one without blocking.
        let signal = tokio::time::timeout(Duration::from_millis(200), updates.recv()).await;
        assert!(signal.is_ok(), "expected at least one update signal");
    }

    #[test]
    fn channel_names_match_deribit_taxonomy() {
        assert_eq!(
            channel_name_for(&ResourceUri::Book {
                instrument: "BTC-PERPETUAL".to_string()
            }),
            "book.BTC-PERPETUAL.100ms"
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
