//! In-process mock Deribit WebSocket server used by the live-
//! resource integration suite (`tests/integration_live.rs`).
//!
//! Speaks the subset of the Deribit JSON-RPC WS protocol the
//! adapter cares about:
//!
//! - `public/auth` → canned token response.
//! - `public/subscribe` / `public/unsubscribe` → canned ack.
//! - `set_heartbeat` / `test_request` → no-op acks.
//!
//! Tests script the frame sequence the mock should emit on top of
//! a subscription via [`MockWsServer::push_frame`]. The frames are
//! relayed to every connected client wrapped in the standard
//! `subscription` notification envelope.
//!
//! No global state. One instance per test. Dropping the server
//! flips the cancellation token and the listener task exits within
//! a few ms.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

#[allow(dead_code)] // used by future tests; kept here for the helper surface.
pub struct MockWsServer {
    addr: std::net::SocketAddr,
    push_tx: broadcast::Sender<Value>,
    cancel: CancellationToken,
    accept_handle: Mutex<Option<JoinHandle<()>>>,
}

#[allow(dead_code)]
impl MockWsServer {
    /// Bind a fresh ephemeral port on `127.0.0.1` and start
    /// accepting WebSocket upgrades.
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let (push_tx, _) = broadcast::channel::<Value>(64);
        let cancel = CancellationToken::new();
        let accept_cancel = cancel.clone();
        let accept_push = push_tx.clone();
        let accept_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = accept_cancel.cancelled() => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { continue };
                        let conn_cancel = accept_cancel.child_token();
                        let conn_push = accept_push.clone();
                        tokio::spawn(handle_connection(stream, conn_push, conn_cancel));
                    }
                }
            }
        });

        Self {
            addr,
            push_tx,
            cancel,
            accept_handle: Mutex::new(Some(accept_handle)),
        }
    }

    /// `ws://…` URL clients connect to.
    #[must_use]
    pub fn ws_url(&self) -> String {
        format!("ws://{}/ws/api/v2", self.addr)
    }

    /// Bound address.
    #[must_use]
    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }

    /// Push one decoded subscription frame. The mock wraps it in
    /// the standard JSON-RPC `subscription` envelope and relays it
    /// to every currently connected client.
    pub fn push_frame(&self, channel: &str, data: Value) {
        let envelope = json!({
            "jsonrpc": "2.0",
            "method": "subscription",
            "params": {
                "channel": channel,
                "data": data,
            }
        });
        // Errors mean no receivers; fine.
        let _ = self.push_tx.send(envelope);
    }
}

impl Drop for MockWsServer {
    fn drop(&mut self) {
        self.cancel.cancel();
        // Best-effort wait; tests should not block on this.
        if let Ok(mut guard) = self.accept_handle.try_lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
    }
}

/// Per-connection task: read JSON-RPC requests, send canned acks,
/// relay broadcasted subscription frames for the channels the
/// client subscribed to.
async fn handle_connection(
    stream: TcpStream,
    push_tx: broadcast::Sender<Value>,
    cancel: CancellationToken,
) {
    use futures_util::{SinkExt, StreamExt};

    let Ok(ws) = accept_async(stream).await else {
        return;
    };
    let (mut tx, mut rx) = ws.split();
    let mut push_rx = push_tx.subscribe();
    let mut subscribed: HashSet<String> = HashSet::new();

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,

            // Inbound: client requests / control frames.
            inbound = rx.next() => match inbound {
                Some(Ok(Message::Text(text))) => {
                    let Ok(req) = serde_json::from_str::<Value>(&text) else { continue };
                    let id = req.get("id").cloned().unwrap_or(json!(0));
                    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
                    let response = match method {
                        "public/auth" => json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "access_token": "mock-access",
                                "expires_in": 900,
                                "refresh_token": "mock-refresh",
                                "scope": "session:test",
                                "token_type": "Bearer",
                            }
                        }),
                        "public/subscribe" => {
                            let channels: Vec<String> = req
                                .get("params")
                                .and_then(|p| p.get("channels"))
                                .and_then(Value::as_array)
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|v| v.as_str().map(str::to_string))
                                        .collect()
                                })
                                .unwrap_or_default();
                            for c in &channels {
                                subscribed.insert(c.clone());
                            }
                            json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": channels
                            })
                        }
                        "public/unsubscribe" => {
                            let channels: Vec<String> = req
                                .get("params")
                                .and_then(|p| p.get("channels"))
                                .and_then(Value::as_array)
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|v| v.as_str().map(str::to_string))
                                        .collect()
                                })
                                .unwrap_or_default();
                            for c in &channels {
                                subscribed.remove(c);
                            }
                            json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "result": channels
                            })
                        }
                        "public/set_heartbeat" | "public/test" => json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": "ok"
                        }),
                        _ => json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": null
                        }),
                    };
                    if tx.send(Message::Text(response.to_string().into())).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Ping(p))) => {
                    let _ = tx.send(Message::Pong(p)).await;
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Err(_)) => break,
                _ => {}
            },

            // Outbound: scripted subscription frames.
            frame = push_rx.recv() => match frame {
                Ok(envelope) => {
                    let channel = envelope
                        .get("params")
                        .and_then(|p| p.get("channel"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if subscribed.contains(channel)
                        && tx
                            .send(Message::Text(envelope.to_string().into()))
                            .await
                            .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
    // Soft close — give the client a moment to drain.
    let _ = tx.close().await;
    drop(tx);
    drop(rx);
    tokio::time::sleep(Duration::from_millis(5)).await;
    // Suppress unused-binding lint on `subscribed` when the test
    // never subscribes to anything.
    let _ = &subscribed;
}
