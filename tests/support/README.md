# `tests/support`

Shared helpers for the integration suite. One module per concern.

## `mock_ws`

In-process mock Deribit WebSocket server used by
`tests/integration_live.rs`.

An integration-test binary picks the helpers up via a regular
sibling-module declaration — there is no `tests` crate / module by
default, so the import path is `support::mock_ws::MockWsServer`
(or `crate::support::…` from inside the test binary's own root).

```rust
// tests/integration_live.rs
mod support;

use support::mock_ws::MockWsServer;

#[tokio::test]
async fn ...() {
    let mock = MockWsServer::start().await;
    let url = mock.ws_url();          // ws://127.0.0.1:<port>/ws/api/v2

    // Drive one of the live-resource paths against `url`.
    // ...

    // Push scripted frames.
    mock.push_frame(
        "book.BTC-PERPETUAL.raw",
        serde_json::json!({"bids": [], "asks": [], "change_id": 1, "timestamp": 0}),
    );

    // Mock cleans up on drop.
}
```

What the mock honours:

- `public/auth` → canned `mock-access` token, `expires_in = 900`.
- `public/subscribe` / `public/unsubscribe` → ack with the channel
  list. Tracked per connection so `push_frame` only relays to
  clients that asked for the channel.
- `public/set_heartbeat` / `public/test` → no-op `ok` acks.
- Anything else → `result: null` ack (so a chatty client does not
  trip the connection).

What the mock does **not** honour:

- Authenticated channels (`user.*`, `private.*`).
- Subscription-id round-trip semantics beyond ack-on-subscribe.
- `set_heartbeat`-driven server-side pings.

These can be added when a test needs them.
