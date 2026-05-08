//! Manual smoke test against the live Deribit testnet sandbox.
//!
//! Drives a small Read + Account round-trip end-to-end so the
//! per-tool integration tests (which rely on `mockito`) can be
//! sanity-checked against the real upstream periodically.
//!
//! ## Gating
//!
//! `#[ignore]` by default — skipped under `cargo test` and never
//! enumerated by the CI matrix. To run locally:
//!
//! ```bash
//! export DERIBIT_CLIENT_ID=<your testnet id>
//! export DERIBIT_CLIENT_SECRET=<your testnet secret>
//! export DERIBIT_MCP_SMOKE=1
//! cargo test --test sandbox_smoke -- --ignored
//! ```
//!
//! When `DERIBIT_MCP_SMOKE` is unset (or set to anything but `1`),
//! the test is **skipped, not failed**, so an `--ignored` rerun on
//! a developer laptop without secrets does not turn red.
//!
//! ## Secrets discipline
//!
//! Credentials are pulled from env at runtime; no value is printed
//! or logged from the test body. The adapter's tracing layer
//! redacts `client_secret` / `access_token` per v0.1-03.

use std::env;
use std::sync::Arc;
use std::time::Duration;

use deribit_mcp::config::{Config, LogFormat, OrderTransport, Transport};
use deribit_mcp::context::AdapterContext;
use deribit_mcp::error::AdapterError;
use deribit_mcp::tools::ToolRegistry;
use serde_json::{Value, json};
use tokio::time::timeout;

/// Per-call timeout; bigger than the testnet's typical p99 latency
/// but well below the upper bound of "test is hung". Wrapping
/// every `registry.call` keeps `cargo test --test sandbox_smoke
/// -- --ignored` from blocking indefinitely on network stalls.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Render a JSON `Value` as a *shape* string for assertion
/// messages: kind plus a coarse size hint. Avoids dumping
/// account-identifying fields into a panic message on the
/// unhappy path.
fn shape_of(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::String(s) => format!("string(len={})", s.len()),
        Value::Array(a) => format!("array(len={})", a.len()),
        Value::Object(o) => format!("object(keys={})", o.len()),
    }
}

/// Wrap an upstream `call` future with [`CALL_TIMEOUT`]. Panics
/// on either elapsed-deadline or upstream error so the test
/// surfaces the cause without printing the body.
async fn call_with_timeout(
    registry: &ToolRegistry,
    ctx: &AdapterContext,
    name: &str,
    input: Value,
) -> Value {
    let outcome = timeout(CALL_TIMEOUT, registry.call(ctx, name, input))
        .await
        .unwrap_or_else(|_| {
            panic!("{name}: timed out after {CALL_TIMEOUT:?}");
        });
    match outcome {
        Ok(value) => value,
        Err(AdapterError::Auth { reason }) => panic!("{name}: auth failed ({reason:?})"),
        Err(AdapterError::Validation { field, .. }) => {
            panic!("{name}: validation failed on field `{field}`")
        }
        Err(other) => panic!("{name}: upstream error ({other:?})"),
    }
}

/// Read a required env var or return `None` so the smoke test can
/// skip-not-fail. Pattern-matches `Result` to keep the credential
/// value out of any panic message even on the unhappy path.
fn required_env(key: &str) -> Option<String> {
    match env::var(key) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

/// Whether the operator opted in to live calls.
fn smoke_enabled() -> bool {
    matches!(env::var("DERIBIT_MCP_SMOKE").as_deref(), Ok("1"))
}

#[tokio::test]
#[ignore = "live network"]
async fn live_testnet_account_smoke() {
    if !smoke_enabled() {
        eprintln!("skipping: DERIBIT_MCP_SMOKE != 1");
        return;
    }
    let Some(client_id) = required_env("DERIBIT_CLIENT_ID") else {
        eprintln!("skipping: DERIBIT_CLIENT_ID not set");
        return;
    };
    let Some(client_secret) = required_env("DERIBIT_CLIENT_SECRET") else {
        eprintln!("skipping: DERIBIT_CLIENT_SECRET not set");
        return;
    };

    let cfg = Config {
        endpoint: "https://test.deribit.com".to_string(),
        client_id: Some(client_id),
        client_secret: Some(client_secret),
        allow_trading: false,
        max_order_usd: None,
        transport: Transport::Stdio,
        http_listen: "127.0.0.1:8723".parse().expect("default listen"),
        http_bearer_token: None,
        log_format: LogFormat::Text,
        order_transport: OrderTransport::Http,
    };
    let ctx = Arc::new(AdapterContext::new(Arc::new(cfg)).expect("adapter context"));
    let registry = ToolRegistry::build(&ctx);

    // 1. Public — `get_server_time` (always available, no auth).
    let server_time = call_with_timeout(&registry, &ctx, "get_server_time", json!({})).await;
    assert!(
        server_time.is_number() || server_time.is_object(),
        "get_server_time: unexpected shape ({})",
        shape_of(&server_time)
    );

    // 2. Public — `get_ticker BTC-PERPETUAL`.
    let ticker = call_with_timeout(
        &registry,
        &ctx,
        "get_ticker",
        json!({"instrument_name": "BTC-PERPETUAL"}),
    )
    .await;
    let mark_price = ticker.get("mark_price").and_then(Value::as_f64);
    assert!(
        mark_price.is_some_and(|p| p > 0.0),
        "ticker mark_price not positive (shape: {})",
        shape_of(&ticker)
    );

    // 3. Account — `get_account_summary BTC`. Drives the lazy
    //    OAuth client-credentials flow on first call.
    let summary = call_with_timeout(
        &registry,
        &ctx,
        "get_account_summary",
        json!({"currency": "BTC"}),
    )
    .await;
    assert!(
        summary.is_object(),
        "get_account_summary: non-object response (shape: {})",
        shape_of(&summary)
    );

    // 4. Account — `get_positions BTC`. Empty list is fine; the
    //    test asserts the shape is an array.
    let positions =
        call_with_timeout(&registry, &ctx, "get_positions", json!({"currency": "BTC"})).await;
    assert!(
        positions.is_array(),
        "get_positions: non-array response (shape: {})",
        shape_of(&positions)
    );

    // Deliberately no eprintln! / panic-format of the response
    // bodies — testnet payloads still include account-identifying
    // fields. Failure messages report only JSON shape (`shape_of`).
}

#[tokio::test]
#[ignore = "live network"]
async fn live_testnet_trading_smoke() {
    // Trading smoke is opt-in via `DERIBIT_MCP_TRADING_SMOKE=1`,
    // SEPARATE from the read-only `DERIBIT_MCP_SMOKE` flag — this
    // places a real (deeply out-of-the-money, post-only) limit
    // order on `test.deribit.com` and cancels it immediately.
    if !matches!(env::var("DERIBIT_MCP_TRADING_SMOKE").as_deref(), Ok("1")) {
        eprintln!("skipping: DERIBIT_MCP_TRADING_SMOKE != 1");
        return;
    }
    let Some(client_id) = required_env("DERIBIT_CLIENT_ID") else {
        eprintln!("skipping: DERIBIT_CLIENT_ID not set");
        return;
    };
    let Some(client_secret) = required_env("DERIBIT_CLIENT_SECRET") else {
        eprintln!("skipping: DERIBIT_CLIENT_SECRET not set");
        return;
    };

    let cfg = Config {
        endpoint: "https://test.deribit.com".to_string(),
        client_id: Some(client_id),
        client_secret: Some(client_secret),
        // The point of the smoke test — exercise the real
        // place_order + cancel_order round trip.
        allow_trading: true,
        // Smallest cap that still allows a 10-USD notional inverse
        // perpetual order; keeps the safety net in place.
        max_order_usd: Some(100),
        transport: Transport::Stdio,
        http_listen: "127.0.0.1:8723".parse().expect("default listen"),
        http_bearer_token: None,
        log_format: LogFormat::Text,
        order_transport: OrderTransport::Http,
    };
    let ctx = Arc::new(AdapterContext::new(Arc::new(cfg)).expect("adapter context"));
    let registry = ToolRegistry::build(&ctx);

    // 1. Read the index price so we can pick a clearly out-of-the-money
    //    limit price (no risk of execution).
    let ticker = call_with_timeout(
        &registry,
        &ctx,
        "get_ticker",
        json!({"instrument_name": "BTC-PERPETUAL"}),
    )
    .await;
    let mark_price = ticker
        .get("mark_price")
        .and_then(Value::as_f64)
        .expect("ticker mark_price");
    let safe_buy_price = (mark_price * 0.5).floor();

    // 2. place_order — buy 10 USD notional at half-mark (post-only).
    let placed = call_with_timeout(
        &registry,
        &ctx,
        "place_order",
        json!({
            "instrument_name": "BTC-PERPETUAL",
            "side": "buy",
            "amount": 10.0,
            "type": "limit",
            "price": safe_buy_price,
            "post_only": true,
            "label": "deribit-mcp-smoke"
        }),
    )
    .await;
    let order_id = placed["order"]["order_id"]
        .as_str()
        .expect("order_id missing")
        .to_string();

    // 3. cancel_order — clean up.
    let cancelled = call_with_timeout(
        &registry,
        &ctx,
        "cancel_order",
        json!({"order_id": order_id}),
    )
    .await;
    assert_eq!(
        cancelled.get("order_state").and_then(Value::as_str),
        Some("cancelled"),
        "cancel_order: not cancelled (shape: {})",
        shape_of(&cancelled)
    );
}
