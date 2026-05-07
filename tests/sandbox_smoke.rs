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

use deribit_mcp::config::{Config, LogFormat, Transport};
use deribit_mcp::context::AdapterContext;
use deribit_mcp::tools::ToolRegistry;
use serde_json::{Value, json};

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
    };
    let ctx = Arc::new(AdapterContext::new(Arc::new(cfg)).expect("adapter context"));
    let registry = ToolRegistry::build(&ctx);

    // 1. Public — `get_server_time` (always available, no auth).
    let server_time = registry
        .call(&ctx, "get_server_time", json!({}))
        .await
        .expect("get_server_time");
    assert!(
        server_time.is_number() || server_time.is_object(),
        "get_server_time returned an unexpected JSON shape: {server_time}"
    );

    // 2. Public — `get_ticker BTC-PERPETUAL`.
    let ticker = registry
        .call(
            &ctx,
            "get_ticker",
            json!({"instrument_name": "BTC-PERPETUAL"}),
        )
        .await
        .expect("get_ticker BTC-PERPETUAL");
    let mark_price = ticker.get("mark_price").and_then(Value::as_f64);
    assert!(
        mark_price.is_some_and(|p| p > 0.0),
        "ticker mark_price not positive (response shape: {ticker})"
    );

    // 3. Account — `get_account_summary BTC`. Drives the lazy
    //    OAuth client-credentials flow on first call.
    let summary = registry
        .call(&ctx, "get_account_summary", json!({"currency": "BTC"}))
        .await
        .expect("get_account_summary BTC");
    assert!(
        summary.is_object(),
        "get_account_summary returned a non-object (shape: {summary})"
    );

    // 4. Account — `get_positions BTC`. Empty list is fine; the
    //    test asserts the shape is an array.
    let positions = registry
        .call(&ctx, "get_positions", json!({"currency": "BTC"}))
        .await
        .expect("get_positions BTC");
    assert!(
        positions.is_array(),
        "get_positions did not return an array (shape: {positions})"
    );

    // Deliberately no eprintln! of the response bodies — testnet
    // payloads still include account-identifying fields.
}
