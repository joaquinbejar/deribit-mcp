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
/// the given `mockito` server URL with the given gating flags. The
/// `deribit-http` client builds each request URL as
/// `{base_url}{endpoint}{query}` (no path-join normalisation), so we
/// suffix the mock root with `/api/v2` — matching the upstream
/// constants — and mockito matches against `/api/v2/public/...`
/// directly. We trim a trailing slash from `server_url` to avoid a
/// `//api/v2` double-slash when callers pass a normalised URL.
fn ctx_with_mock_creds(
    server_url: &str,
    with_creds: bool,
    allow_trading: bool,
) -> Arc<AdapterContext> {
    use deribit_http::config::credentials::ApiCredentials;

    let server_url = server_url.trim_end_matches('/');
    let with_prefix = format!("{server_url}/api/v2");
    let parsed = Url::parse(&with_prefix).expect("mock URL");
    let mut http_cfg = HttpConfig::testnet();
    http_cfg.base_url = parsed;
    http_cfg.testnet = true;
    http_cfg.timeout = Duration::from_secs(2);
    if with_creds {
        http_cfg.credentials = Some(ApiCredentials {
            client_id: Some("id".to_string()),
            client_secret: Some("secret".to_string()),
        });
    } else {
        http_cfg.credentials = None;
    }
    let http = DeribitHttpClient::with_config(http_cfg);

    // Build a normal context, then swap in the http client.
    let cfg = Arc::new(cfg(&with_prefix, with_creds, allow_trading));
    let mut ctx = AdapterContext::new(cfg).expect("ctx");
    ctx.http = http;
    Arc::new(ctx)
}

/// Anonymous-context shorthand.
fn ctx_with_mock(server_url: &str) -> Arc<AdapterContext> {
    ctx_with_mock_creds(server_url, false, false)
}

/// Same as [`ctx_with_mock_creds`] but routes the upstream HTTP
/// client through the mock server. Used by the OAuth-flow tests.
fn ctx_with_mock_authenticated(server_url: &str) -> Arc<AdapterContext> {
    ctx_with_mock_creds(server_url, true, false)
}

#[tokio::test]
async fn tools_list_without_creds_includes_only_read_class() {
    let ctx = ctx_with_mock("http://127.0.0.1:0/");
    // We don't need the mock for this scenario; just exercise the
    // registry + class gating that drives `tools/list`. Asserting
    // on a specific tool count would couple the test to whichever
    // milestone added the most-recent tool — instead, assert every
    // registered tool is `Read` and the well-known v0.1 names show
    // up.
    let registry = ToolRegistry::build(&ctx);
    assert!(!registry.is_empty(), "Read tools register without creds");
    for tool in registry.list() {
        let entry = registry.get(tool.name.as_ref()).expect("entry");
        assert_eq!(
            entry.class(),
            deribit_mcp::tools::ToolClass::Read,
            "{}",
            tool.name
        );
    }
    for expected in ["get_ticker", "get_currencies", "get_order_book"] {
        assert!(
            registry.contains(expected),
            "expected `{expected}` in tool list"
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
    // `TickerData` struct serialised). Tighten the assertion to a
    // specific value so a wrong-instrument response can't pass.
    assert_eq!(
        out.get("instrument_name").and_then(Value::as_str),
        Some("BTC-PERPETUAL"),
        "expected instrument_name in payload, got {out}"
    );
    assert_eq!(
        out.get("mark_price").and_then(Value::as_f64),
        Some(50_000.5),
        "expected mark_price in payload, got {out}"
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
    // Credentials present, `--allow-trading` NOT set: this is the
    // exact scenario ADR-0010 gates. The Trading family is omitted
    // from the registry, so `place_order` is absent and dispatch
    // returns `Validation { field: "name" }` (the user-facing "tool
    // not registered" path). The defence-in-depth `NotEnabled` path
    // is exercised by the unit tests in `src/tools/mod.rs` — the
    // integration test guards the *absence-from-registry* branch.
    let ctx = ctx_with_mock_creds("http://127.0.0.1:0/", true, false);
    let registry = ToolRegistry::build(&ctx);
    assert!(
        !registry.contains("place_order"),
        "Trading tools must NOT register without --allow-trading"
    );
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
async fn first_private_call_triggers_oauth_against_mock() {
    let mut server = mockito::Server::new_async().await;

    let auth_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "result": {
            "access_token": "test-access",
            "expires_in": 900,
            "refresh_token": "test-refresh",
            "scope": "session:test",
            "token_type": "Bearer"
        }
    });
    let auth_mock = server
        .mock("GET", "/api/v2/public/auth")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(auth_body.to_string())
        .expect_at_least(1)
        .create_async()
        .await;

    // `AccountSummaryResponse` defaults every field via `#[serde(default)]`,
    // so an empty `result` object deserialises cleanly. We only need to
    // observe that the call hit `/private/get_account_summary` carrying a
    // `Bearer` header — that proves OAuth flowed through the upstream
    // `AuthManager`.
    let summary_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "result": {}
    });
    let summary_mock = server
        .mock("GET", "/api/v2/private/get_account_summary")
        .match_query(mockito::Matcher::Any)
        .match_header(
            "authorization",
            mockito::Matcher::Regex("^Bearer ".to_string()),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(summary_body.to_string())
        .expect(1)
        .create_async()
        .await;

    let ctx = ctx_with_mock_authenticated(&server.url());
    assert_eq!(ctx.auth_state(), deribit_mcp::AuthState::Configured);

    // Drive a private call. The upstream `AuthManager` hits
    // `/public/auth` lazily, then issues the private call with a
    // `Bearer` header.
    ctx.http
        .get_account_summary("BTC", Some(false))
        .await
        .expect("get_account_summary");

    auth_mock.assert_async().await;
    summary_mock.assert_async().await;
}

#[tokio::test]
async fn account_tools_register_only_with_credentials() {
    let anon = ToolRegistry::build(&ctx_with_mock("http://127.0.0.1:0/"));
    assert!(!anon.contains("get_account_summary"));
    assert!(!anon.contains("get_positions"));
    assert!(!anon.contains("get_subaccounts"));

    let with_creds = ToolRegistry::build(&ctx_with_mock_authenticated("http://127.0.0.1:0/"));
    assert!(with_creds.contains("get_account_summary"));
    assert!(with_creds.contains("get_positions"));
    assert!(with_creds.contains("get_subaccounts"));
    for name in ["get_account_summary", "get_positions", "get_subaccounts"] {
        let entry = with_creds.get(name).expect("entry");
        assert_eq!(entry.class(), deribit_mcp::tools::ToolClass::Account);
    }
}

#[tokio::test]
async fn account_summary_tool_dispatches_through_registry() {
    let mut server = mockito::Server::new_async().await;

    let auth_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "result": {
            "access_token": "test-access",
            "expires_in": 900,
            "refresh_token": "test-refresh",
            "scope": "session:test",
            "token_type": "Bearer"
        }
    });
    server
        .mock("GET", "/api/v2/public/auth")
        .match_query(mockito::Matcher::Any)
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(auth_body.to_string())
        .expect_at_least(1)
        .create_async()
        .await;

    let summary_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "result": { "id": 42, "username": "test-user" }
    });
    let summary_mock = server
        .mock("GET", "/api/v2/private/get_account_summary")
        .match_query(mockito::Matcher::Any)
        .match_header(
            "authorization",
            mockito::Matcher::Regex("^Bearer ".to_string()),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(summary_body.to_string())
        .expect(1)
        .create_async()
        .await;

    let ctx = ctx_with_mock_authenticated(&server.url());
    let registry = ToolRegistry::build(&ctx);
    let out = registry
        .call(
            &ctx,
            "get_account_summary",
            serde_json::json!({"currency": "BTC"}),
        )
        .await
        .expect("ok");

    assert_eq!(out.get("id").and_then(Value::as_u64), Some(42));
    assert_eq!(
        out.get("username").and_then(Value::as_str),
        Some("test-user")
    );
    summary_mock.assert_async().await;
}

#[tokio::test]
async fn account_summary_without_credentials_is_validation_error() {
    let ctx = ctx_with_mock("http://127.0.0.1:0/");
    let registry = ToolRegistry::build(&ctx);
    let err = registry
        .call(
            &ctx,
            "get_account_summary",
            serde_json::json!({"currency": "BTC"}),
        )
        .await
        .unwrap_err();
    // Account tool is absent from the registry without credentials,
    // so dispatch surfaces the registry-miss path.
    match err {
        AdapterError::Validation { field, .. } => assert_eq!(field, "name"),
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test]
async fn second_private_call_reuses_token() {
    let mut server = mockito::Server::new_async().await;

    let auth_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "result": {
            "access_token": "test-access",
            "expires_in": 900,
            "refresh_token": "test-refresh",
            "scope": "session:test",
            "token_type": "Bearer"
        }
    });
    let auth_mock = server
        .mock("GET", "/api/v2/public/auth")
        .match_query(mockito::Matcher::UrlEncoded(
            "grant_type".into(),
            "client_credentials".into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(auth_body.to_string())
        // `expect(1)` asserts the auth endpoint is hit exactly once
        // even after multiple private calls — the token is cached
        // by `deribit-http`'s `AuthManager`.
        .expect(1)
        .create_async()
        .await;

    let summary_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "result": {}
    });
    let summary_mock = server
        .mock("GET", "/api/v2/private/get_account_summary")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded("currency".into(), "BTC".into()),
            mockito::Matcher::UrlEncoded("extended".into(), "false".into()),
        ]))
        .match_header(
            "authorization",
            mockito::Matcher::Regex("^Bearer ".to_string()),
        )
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(summary_body.to_string())
        .expect(2)
        .create_async()
        .await;

    let ctx = ctx_with_mock_authenticated(&server.url());

    for _ in 0..2 {
        ctx.http
            .get_account_summary("BTC", Some(false))
            .await
            .expect("get_account_summary");
    }

    auth_mock.assert_async().await;
    summary_mock.assert_async().await;
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
