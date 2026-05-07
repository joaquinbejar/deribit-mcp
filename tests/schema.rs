//! Schema snapshot tests.
//!
//! Snapshots every public-tool input schema and the
//! `AdapterError` / `UpstreamErrorKind` wire shape. Any change to
//! the snapshots requires a deliberate review at PR time and a
//! `CHANGELOG.md` entry under `[Unreleased]` describing whether the
//! change is additive (new tool / variant) or breaking (renamed
//! field, removed variant, schema tightened). The MCP wire shape is
//! a public contract.

use deribit_mcp::config::{Config, LogFormat, Transport};
use deribit_mcp::context::AdapterContext;
use deribit_mcp::error::{AdapterError, AuthFailureReason, UpstreamErrorKind};
use deribit_mcp::tools::ToolRegistry;
use std::net::SocketAddr;
use std::sync::Arc;

fn ctx(with_creds: bool, allow_trading: bool) -> AdapterContext {
    let cfg = Config {
        endpoint: "https://test.deribit.com".to_string(),
        client_id: with_creds.then(|| "id".to_string()),
        client_secret: with_creds.then(|| "secret".to_string()),
        allow_trading,
        max_order_usd: None,
        transport: Transport::Stdio,
        http_listen: SocketAddr::from(([127, 0, 0, 1], 8723)),
        http_bearer_token: None,
        log_format: LogFormat::Text,
    };
    AdapterContext::new(Arc::new(cfg)).expect("ctx")
}

/// Snapshot the input schema of every tool registered without
/// credentials (the v0.1 public surface). Schemas live under
/// `tests/snapshots/schema__tool_input_schemas.snap`.
#[test]
fn tool_input_schemas_unchanged() {
    let registry = ToolRegistry::build(&ctx(false, false));
    let mut tools = registry.list();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let schemas: Vec<(String, serde_json::Value)> = tools
        .into_iter()
        .map(|tool| {
            let name = tool.name.to_string();
            let schema = serde_json::Value::Object((*tool.input_schema).clone());
            (name, schema)
        })
        .collect();
    insta::assert_yaml_snapshot!("tool_input_schemas", schemas);
}

/// Snapshot the JSON wire shape of every documented `AdapterError`
/// variant. The serde-tagged representation is a public contract.
#[test]
fn adapter_error_wire_shapes_unchanged() {
    let cases: Vec<(&str, AdapterError)> = vec![
        (
            "auth_missing_credentials",
            AdapterError::Auth {
                reason: AuthFailureReason::MissingCredentials,
            },
        ),
        (
            "auth_unauthorized",
            AdapterError::Auth {
                reason: AuthFailureReason::Unauthorized,
            },
        ),
        (
            "auth_token_refresh_failed",
            AdapterError::Auth {
                reason: AuthFailureReason::TokenRefreshFailed,
            },
        ),
        (
            "auth_other",
            AdapterError::Auth {
                reason: AuthFailureReason::Other,
            },
        ),
        (
            "rate_limited",
            AdapterError::RateLimited {
                retry_after_ms: 2_000,
            },
        ),
        (
            "upstream_api",
            AdapterError::Upstream {
                inner: UpstreamErrorKind::Api {
                    code: Some(10_000),
                    message: "boom".to_string(),
                },
            },
        ),
        (
            "upstream_network",
            AdapterError::Upstream {
                inner: UpstreamErrorKind::Network {
                    message: "dns".to_string(),
                },
            },
        ),
        (
            "upstream_http",
            AdapterError::Upstream {
                inner: UpstreamErrorKind::Http {
                    message: "5xx".to_string(),
                },
            },
        ),
        (
            "upstream_websocket",
            AdapterError::Upstream {
                inner: UpstreamErrorKind::Websocket {
                    message: "closed".to_string(),
                },
            },
        ),
        (
            "validation",
            AdapterError::validation("instrument_name", "must be non-empty"),
        ),
        (
            "size_cap_exceeded",
            AdapterError::SizeCapExceeded {
                requested: 25_000.0,
                cap: 10_000.0,
            },
        ),
        (
            "not_enabled",
            AdapterError::not_enabled("place_order", "--allow-trading"),
        ),
        (
            "internal",
            AdapterError::internal("upstream payload schema mismatch"),
        ),
    ];
    let serialised: Vec<(&'static str, serde_json::Value)> = cases
        .iter()
        .map(|(name, err)| (*name, serde_json::to_value(err).expect("ser")))
        .collect();
    insta::assert_yaml_snapshot!("adapter_error_wire_shapes", serialised);
}

/// Snapshot the resource catalogue (concrete entries + URI templates)
/// returned by `resources/list` / `resources/templates/list`.
#[test]
fn resource_catalogue_unchanged() {
    let registry = deribit_mcp::resources::ResourceRegistry::build();
    let mut entries: Vec<serde_json::Value> = registry
        .resources()
        .into_iter()
        .map(|r| serde_json::to_value(&r).expect("ser"))
        .collect();
    entries.sort_by(|a, b| a["uri"].as_str().cmp(&b["uri"].as_str()));
    let mut templates: Vec<serde_json::Value> = registry
        .templates()
        .into_iter()
        .map(|t| serde_json::to_value(&t).expect("ser"))
        .collect();
    templates.sort_by(|a, b| a["uriTemplate"].as_str().cmp(&b["uriTemplate"].as_str()));

    insta::assert_yaml_snapshot!(
        "resource_catalogue",
        serde_json::json!({
            "resources": entries,
            "templates": templates,
        })
    );
}
