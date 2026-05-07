//! End-to-end integration tests for v0.1.
//!
//! Drives the public adapter surface against a `mockito` HTTP
//! server — `DeribitHttpClient` is pointed at the mock and every
//! tool / resource that exercises HTTP runs in-process. Avoids the
//! live testnet so the suite stays deterministic and offline-safe.
//!
//! The MCP transport handshake is covered in
//! `tests/stdio_handshake.rs` (stdio) and `tests/http_transport.rs`
//! (HTTP/healthz/bearer); this file focuses on the tool / resource
//! handlers behind a `ToolRegistry::call` and a
//! `ResourceRegistry::read` call.

use std::sync::Arc;

use deribit_http::{DeribitHttpClient, HttpConfig};
use deribit_mcp::config::{Config, LogFormat, Transport};
use deribit_mcp::context::AdapterContext;
use deribit_mcp::error::AdapterError;
use deribit_mcp::resources::{ResourceContent, ResourceRegistry, ResourceUri};
use deribit_mcp::tools::ToolRegistry;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use url::Url;

fn cfg(endpoint: &str, with_creds: bool, allow_trading: bool) -> Config {
    Config {
        endpoint: endpoint.to_string(),
        client_id: with_creds.then(|| "id".to_string()),
        client_secret: with_creds.then(|| "secret".to_string()),
        allow_trading,
        max_order_usd: None,
        transport: Transport::Stdio,
        http_listen: SocketAddr::from(([127, 0, 0, 1], 8723)),
        http_bearer_token: None,
        log_format: LogFormat::Text,
    }
}

/// Build an `AdapterContext` whose upstream HTTP client points at
/// the given `mockito` server URL. The `deribit-http` client builds
/// each request URL as `{base_url}{endpoint}{query}` (no path-join
/// normalisation), so we suffix the mock root with `/api/v2` —
/// matching the upstream constants — and mockito matches against
/// `/api/v2/public/...` directly.
fn ctx_with_mock(server_url: &str) -> Arc<AdapterContext> {
    let with_prefix = format!("{server_url}/api/v2");
    let parsed = Url::parse(&with_prefix).expect("mock URL");
    let mut http_cfg = HttpConfig::testnet();
    http_cfg.base_url = parsed;
    http_cfg.testnet = true;
    http_cfg.timeout = Duration::from_secs(2);
    let http = DeribitHttpClient::with_config(http_cfg);

    // Build a normal context, then swap in the http client.
    let cfg = Arc::new(cfg(&with_prefix, false, false));
    let mut ctx = AdapterContext::new(cfg).expect("ctx");
    ctx.http = http;
    Arc::new(ctx)
}

#[tokio::test]
async fn tools_list_without_creds_includes_only_read_class() {
    let ctx = ctx_with_mock("http://127.0.0.1:0/");
    // We don't need the mock for this scenario; just exercise the
    // registry + class gating that drives `tools/list`.
    let registry = ToolRegistry::build(&ctx);
    assert_eq!(registry.len(), 14);
    for tool in registry.list() {
        let entry = registry.get(tool.name.as_ref()).expect("entry");
        assert_eq!(
            entry.class(),
            deribit_mcp::tools::ToolClass::Read,
            "{}",
            tool.name
        );
    }
}

#[tokio::test]
async fn tools_call_get_ticker_returns_upstream_payload() {
    let mut server = mockito::Server::new_async().await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "result": {
            "instrument_name": "BTC-PERPETUAL",
            "best_bid_price": 50_000.0,
            "best_ask_price": 50_001.0,
            "best_bid_amount": 1.0,
            "best_ask_amount": 1.5,
            "mark_price": 50_000.5,
            "last_price": 49_999.0,
            "timestamp": 1_700_000_000_000_u64,
            "state": "open",
            "stats": {
                "high": 51_000.0,
                "low": 49_000.0,
                "volume": 100.0,
                "volume_usd": 5_000_000.0,
                "price_change": 200.0
            }
        }
    });
    let _m = server
        .mock("GET", "/api/v2/public/ticker?instrument_name=BTC-PERPETUAL")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await;

    let ctx = ctx_with_mock(&server.url());
    let registry = ToolRegistry::build(&ctx);

    let out = registry
        .call(
            &ctx,
            "get_ticker",
            json!({"instrument_name": "BTC-PERPETUAL"}),
        )
        .await
        .expect("ok");

    // The handler returns the upstream JSON verbatim (the
    // `TickerData` struct serialised). Just assert one stable field
    // to avoid coupling to the exact upstream serde shape.
    assert!(
        out.get("instrument_name").and_then(Value::as_str) == Some("BTC-PERPETUAL")
            || out.get("mark_price").is_some(),
        "expected ticker fields in payload, got {out}"
    );
}

#[tokio::test]
async fn tools_call_unknown_returns_validation() {
    let ctx = ctx_with_mock("http://127.0.0.1:0/");
    let registry = ToolRegistry::build(&ctx);
    let err = registry
        .call(&ctx, "no_such_tool", json!({}))
        .await
        .unwrap_err();
    match err {
        AdapterError::Validation { field, .. } => assert_eq!(field, "name"),
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test]
async fn tools_call_trading_without_allow_trading_returns_not_enabled() {
    let ctx = ctx_with_mock("http://127.0.0.1:0/");
    let registry = ToolRegistry::build(&ctx);
    // `place_order` (Trading) is not registered without the flag,
    // so dispatch returns `Validation { field: "name" }` — which is
    // the user-facing equivalent of "method not found" at the MCP
    // layer (rmcp converts it). This guards the absence-from-registry
    // path; the defence-in-depth `NotEnabled` path is covered by
    // unit tests in `src/tools/mod.rs`.
    let err = registry
        .call(&ctx, "place_order", json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, AdapterError::Validation { .. }));
}

#[tokio::test]
async fn resources_read_currencies_returns_upstream_payload() {
    let mut server = mockito::Server::new_async().await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 0,
        "result": [
            {
                "currency": "BTC",
                "currency_long": "Bitcoin",
                "fee_precision": 4,
                "min_confirmations": 1,
                "min_withdrawal_fee": 0.0,
                "withdrawal_fee": 0.0,
                "withdrawal_priorities": [],
            }
        ]
    });
    let _m = server
        .mock("GET", "/api/v2/public/get_currencies")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body.to_string())
        .create_async()
        .await;

    let ctx = ctx_with_mock(&server.url());
    let registry = ResourceRegistry::build();

    let content = registry
        .read(&ctx, &ResourceUri::Currencies)
        .await
        .expect("ok");

    match content {
        ResourceContent::Json(value) => {
            let array = value.as_array().expect("array");
            assert!(array.iter().any(|v| v["currency"] == "BTC"));
        }
    }
}

#[tokio::test]
async fn resources_read_live_uri_returns_internal_until_v03() {
    let ctx = ctx_with_mock("http://127.0.0.1:0/");
    let registry = ResourceRegistry::build();
    let err = registry
        .read(
            &ctx,
            &ResourceUri::Book {
                instrument: "BTC-PERPETUAL".to_string(),
            },
        )
        .await
        .unwrap_err();
    match err {
        AdapterError::Internal { ref reason } => {
            assert_eq!(reason, "live resources land in v0.3");
        }
        other => panic!("unexpected: {other:?}"),
    }
}
