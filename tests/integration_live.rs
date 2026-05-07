//! Live-resource integration tests against the v0.3-06 mock
//! WebSocket server.
//!
//! These tests drive raw `tokio-tungstenite` clients against the
//! mock so we exercise the mock surface end-to-end without the full
//! adapter wiring (the real `SubscriptionProvider` over
//! `deribit-websocket` lands separately and reuses this mock).

mod support;

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use support::mock_ws::MockWsServer;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// Connect, subscribe to a channel, push a scripted book frame
/// from the test, assert the client receives the framed
/// envelope. Pin the channel name (`book.<i>.raw`) and the inner
/// `data` shape so the mock contract stays stable for the live-
/// resource provider tests.
#[tokio::test]
async fn mock_ws_relays_scripted_book_frame_to_subscribed_client() {
    let mock = MockWsServer::start().await;
    let url = mock.ws_url();
    let (ws, _) = connect_async(&url).await.expect("connect");
    let (mut tx, mut rx) = ws.split();

    // Subscribe.
    let sub = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "public/subscribe",
        "params": { "channels": ["book.BTC-PERPETUAL.raw"] }
    });
    tx.send(Message::Text(sub.to_string().into()))
        .await
        .expect("send subscribe");

    // Drain the subscribe ack.
    let ack = recv_text(&mut rx).await.expect("ack frame");
    let ack: Value = serde_json::from_str(&ack).expect("ack JSON");
    assert_eq!(ack["id"], 1);
    let chans = ack["result"].as_array().expect("ack result is array");
    assert!(
        chans
            .iter()
            .any(|c| c.as_str() == Some("book.BTC-PERPETUAL.raw")),
        "ack result must include subscribed channel; got {ack}"
    );

    // Push a frame.
    mock.push_frame(
        "book.BTC-PERPETUAL.raw",
        json!({
            "bids": [[50_000.0, 1.0]],
            "asks": [[50_001.0, 1.5]],
            "change_id": 42,
            "timestamp": 1_700_000_000_000_i64,
        }),
    );

    // Receive the relayed envelope.
    let frame = tokio::time::timeout(Duration::from_secs(2), recv_text(&mut rx))
        .await
        .expect("frame within timeout")
        .expect("frame");
    let envelope: Value = serde_json::from_str(&frame).expect("frame JSON");
    assert_eq!(envelope["method"], "subscription");
    assert_eq!(
        envelope["params"]["channel"].as_str(),
        Some("book.BTC-PERPETUAL.raw")
    );
    assert_eq!(envelope["params"]["data"]["change_id"].as_u64(), Some(42));
}

/// `public/auth` returns the canned mock token shape so the
/// adapter's `OAuth` flow can be exercised against the mock the
/// same way it is against `mockito` HTTP stubs.
#[tokio::test]
async fn mock_ws_responds_to_public_auth_with_canned_token() {
    let mock = MockWsServer::start().await;
    let (ws, _) = connect_async(mock.ws_url()).await.expect("connect");
    let (mut tx, mut rx) = ws.split();
    let auth = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "public/auth",
        "params": { "grant_type": "client_credentials", "client_id": "id", "client_secret": "secret" }
    });
    tx.send(Message::Text(auth.to_string().into()))
        .await
        .expect("send auth");
    let resp = recv_text(&mut rx).await.expect("auth response");
    let resp: Value = serde_json::from_str(&resp).expect("auth JSON");
    assert_eq!(resp["id"], 7);
    assert_eq!(resp["result"]["access_token"], "mock-access");
    assert_eq!(resp["result"]["token_type"], "Bearer");
}

/// `public/unsubscribe` removes the channel binding — subsequent
/// pushes for that channel are not relayed.
#[tokio::test]
async fn mock_ws_unsubscribe_stops_frame_relay() {
    let mock = MockWsServer::start().await;
    let (ws, _) = connect_async(mock.ws_url()).await.expect("connect");
    let (mut tx, mut rx) = ws.split();
    let sub = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "public/subscribe",
        "params": { "channels": ["ticker.BTC-PERPETUAL.100ms"] }
    });
    tx.send(Message::Text(sub.to_string().into()))
        .await
        .expect("send sub");
    let _ = recv_text(&mut rx).await; // ack

    // Unsubscribe.
    let unsub = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "public/unsubscribe",
        "params": { "channels": ["ticker.BTC-PERPETUAL.100ms"] }
    });
    tx.send(Message::Text(unsub.to_string().into()))
        .await
        .expect("send unsub");
    let _ = recv_text(&mut rx).await; // ack

    // Push a frame; the client must NOT receive it.
    mock.push_frame(
        "ticker.BTC-PERPETUAL.100ms",
        json!({"mark_price": 50_000.0, "timestamp": 0_i64}),
    );

    let recv = tokio::time::timeout(Duration::from_millis(150), recv_text(&mut rx)).await;
    assert!(
        recv.is_err(),
        "no frame should be relayed after unsubscribe; got {recv:?}"
    );
}

async fn recv_text<S>(rx: &mut S) -> Option<String>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    while let Some(msg) = rx.next().await {
        match msg.ok()? {
            Message::Text(t) => return Some(t.to_string()),
            Message::Close(_) => return None,
            _ => continue,
        }
    }
    None
}

/// Scenario 1 — first subscribe goes to the mock exactly once.
#[tokio::test]
async fn one_client_one_subscribe() {
    let mock = MockWsServer::start().await;
    let (ws, _) = connect_async(mock.ws_url()).await.expect("connect");
    let (mut tx, mut rx) = ws.split();
    let sub = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "public/subscribe",
        "params": { "channels": ["book.BTC-PERPETUAL.raw"] }
    });
    tx.send(Message::Text(sub.to_string().into()))
        .await
        .unwrap();
    let _ack = recv_text(&mut rx).await.expect("ack");
    assert_eq!(mock.subscribe_count(), 1);
    assert_eq!(mock.unsubscribe_count(), 0);
}

/// Scenario 2 — two distinct WS connections each subscribe once;
/// the mock observes two subscribes (one per connection). The
/// LiveRegistry-level "two MCP subscribers reuse one upstream
/// subscription" property is covered by the lib-level
/// `second_subscribe_reuses_entry` test in `src/resources/live.rs`.
#[tokio::test]
async fn two_clients_each_send_their_own_subscribe() {
    let mock = MockWsServer::start().await;
    for i in 0..2 {
        let (ws, _) = connect_async(mock.ws_url()).await.expect("connect");
        let (mut tx, mut rx) = ws.split();
        let sub = json!({
            "jsonrpc": "2.0",
            "id": i,
            "method": "public/subscribe",
            "params": { "channels": ["book.BTC-PERPETUAL.raw"] }
        });
        tx.send(Message::Text(sub.to_string().into()))
            .await
            .unwrap();
        let _ack = recv_text(&mut rx).await.expect("ack");
    }
    // Give the second connection's task a beat to bump the counter.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(mock.subscribe_count(), 2);
}

/// Scenario 3 — explicit unsubscribe surfaces on the mock.
#[tokio::test]
async fn explicit_unsubscribe_increments_unsubscribe_count() {
    let mock = MockWsServer::start().await;
    let (ws, _) = connect_async(mock.ws_url()).await.expect("connect");
    let (mut tx, mut rx) = ws.split();
    tx.send(Message::Text(
        json!({"jsonrpc":"2.0","id":1,"method":"public/subscribe","params":{"channels":["t.X.raw"]}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let _ = recv_text(&mut rx).await;
    tx.send(Message::Text(
        json!({"jsonrpc":"2.0","id":2,"method":"public/unsubscribe","params":{"channels":["t.X.raw"]}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let _ = recv_text(&mut rx).await;
    assert_eq!(mock.subscribe_count(), 1);
    assert_eq!(mock.unsubscribe_count(), 1);
}

/// Scenario 4 — the mock shuts the listener down, the client's
/// stream ends with a close frame. A real `WsSubscriptionProvider`
/// would respond with a reconnect + re-subscribe; this test pins
/// the *observable shutdown* signal end-to-end so the reconnect
/// implementation in `deribit-websocket` (or a future adapter
/// wrapper) has a deterministic trigger to test against.
#[tokio::test]
async fn mock_shutdown_terminates_client_stream() {
    let mock = MockWsServer::start().await;
    let url = mock.ws_url();
    let (ws, _) = connect_async(&url).await.expect("connect");
    let (mut tx, mut rx) = ws.split();
    tx.send(Message::Text(
        json!({"jsonrpc":"2.0","id":1,"method":"public/subscribe","params":{"channels":["t.X.raw"]}})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let _ = recv_text(&mut rx).await; // ack

    mock.shutdown();

    // Within a short window the per-connection task observes the
    // cancel, sends `close`, and the client stream ends.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut ended = false;
    while std::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(100), rx.next()).await {
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                ended = true;
                break;
            }
            Ok(Some(Err(_))) => {
                ended = true;
                break;
            }
            _ => continue,
        }
    }
    assert!(ended, "client stream should end after server shutdown");
}
