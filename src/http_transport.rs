//! HTTP / Streamable-HTTP transport for the adapter.
//!
//! Wraps `rmcp`'s [`StreamableHttpService`] in an `axum` router that
//! adds the unauthenticated `/healthz` probe and an optional static
//! bearer-token check on the `/mcp` endpoint (driven by
//! [`Config::http_bearer_token`]).
//!
//! Routes:
//!
//! - `GET /healthz` — 200 OK while the server is reachable.
//! - `POST /mcp` and `GET /mcp` — Streamable HTTP transport (single
//!   endpoint, JSON request / JSON or SSE response per the MCP
//!   2025-06-18 spec).
//!
//! Bearer-token discipline: when configured, every request to `/mcp`
//! must carry `Authorization: Bearer <token>`. `/healthz` is always
//! unauthenticated so container orchestration probes never need a
//! credential.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::StreamableHttpServerConfig;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::Config;
use crate::context::AdapterContext;
use crate::error::AdapterError;
use crate::server::DeribitMcpServer;

/// Bind an HTTP listener and serve the adapter over Streamable HTTP.
///
/// # Errors
///
/// Returns [`AdapterError::Internal`] if the listener cannot be bound
/// (port in use, permission denied, …) or if the underlying axum
/// server returns an error.
pub async fn serve(
    config: Arc<Config>,
    ctx: Arc<AdapterContext>,
    cancel: CancellationToken,
) -> Result<(), AdapterError> {
    let listen: SocketAddr = config.http_listen;
    let listener = TcpListener::bind(listen).await.map_err(|err| {
        tracing::error!(error = %err, addr = %listen, "failed to bind HTTP listener");
        AdapterError::internal("failed to bind HTTP listener")
    })?;
    serve_with_listener(config, ctx, listener, cancel).await
}

/// Same as [`serve`] but takes an already-bound [`TcpListener`].
///
/// Used by integration tests that need to know the chosen port
/// before the server starts. Same error semantics as [`serve`].
///
/// # Errors
///
/// See [`serve`].
pub async fn serve_with_listener(
    config: Arc<Config>,
    ctx: Arc<AdapterContext>,
    listener: TcpListener,
    cancel: CancellationToken,
) -> Result<(), AdapterError> {
    let bearer = config.http_bearer_token.clone();
    let allowed_hosts = config.allowed_hosts.clone();

    let mcp_service = build_streamable_service(ctx, cancel.clone(), allowed_hosts);

    // Layer the bearer-token middleware *only* on the `/mcp` service so
    // that unknown paths surface a natural 404 (rather than 401) and
    // `/healthz` stays unauthenticated by construction.
    let mcp_router =
        Router::new()
            .fallback_service(mcp_service)
            .layer(middleware::from_fn_with_state(
                Arc::new(BearerState { bearer }),
                bearer_auth,
            ));

    let app = Router::new()
        .route("/healthz", get(healthz))
        .nest_service("/mcp", mcp_router);

    let local = listener.local_addr().ok();
    if let Some(addr) = local {
        tracing::info!(addr = %addr, "HTTP transport listening");
    }

    let cancel_clone = cancel.clone();
    let serve = axum::serve(listener, app)
        .with_graceful_shutdown(async move { cancel_clone.cancelled().await });

    serve.await.map_err(|err| {
        tracing::error!(error = %err, "HTTP server exited with error");
        AdapterError::internal("HTTP server exited with error")
    })?;

    Ok(())
}

/// Build the rmcp Streamable HTTP `tower::Service`.
fn build_streamable_service(
    ctx: Arc<AdapterContext>,
    cancel: CancellationToken,
    allowed_hosts: Vec<String>,
) -> StreamableHttpService<DeribitMcpServer, LocalSessionManager> {
    // Allowed `Host` header values come from `Config::allowed_hosts`
    // (CLI `--allowed-hosts` or `DERIBIT_ALLOWED_HOSTS`). Default is
    // the loopback safe set; operators fronting the adapter under a
    // public DNS name must list it explicitly to defeat DNS-rebinding.
    let config = StreamableHttpServerConfig::default()
        .with_cancellation_token(cancel)
        .with_allowed_hosts(allowed_hosts);

    StreamableHttpService::new(
        move || Ok(DeribitMcpServer::new(ctx.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}

/// Liveness probe: 200 OK while the server is reachable.
async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// State shared with the bearer-token middleware.
#[derive(Debug, Clone)]
struct BearerState {
    /// Configured token. `None` disables the check.
    bearer: Option<String>,
}

/// Bearer-token middleware applied to the `/mcp` sub-router.
///
/// Every request must carry `Authorization: Bearer <token>` when
/// [`BearerState::bearer`] is configured; mismatches return `401`
/// with `WWW-Authenticate: Bearer`.
async fn bearer_auth(
    State(state): State<Arc<BearerState>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(expected) = state.bearer.as_deref() else {
        return next.run(request).await;
    };
    if !is_bearer_match(request.headers(), expected) {
        return (
            StatusCode::UNAUTHORIZED,
            [(axum::http::header::WWW_AUTHENTICATE, "Bearer")],
            "unauthorized",
        )
            .into_response();
    }
    next.run(request).await
}

fn is_bearer_match(headers: &HeaderMap, expected: &str) -> bool {
    let Some(value) = headers.get(axum::http::header::AUTHORIZATION) else {
        return false;
    };
    let Ok(text) = value.to_str() else {
        return false;
    };
    let Some(token) = text.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(token.as_bytes(), expected.as_bytes())
}

/// Constant-time byte comparison for the bearer-token check.
///
/// Always XOR-folds over `max(a.len(), b.len())` bytes (treating the
/// shorter side as zero-padded) and returns `false` if the lengths
/// differ. The comparison cost is therefore independent of where (or
/// whether) `a` and `b` differ, and has no early-exit shortcut.
#[inline]
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let len = a.len().max(b.len());
    let mut diff: u8 = (a.len() ^ b.len()) as u8;
    for i in 0..len {
        let x = *a.get(i).unwrap_or(&0);
        let y = *b.get(i).unwrap_or(&0);
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_match_accepts_correct_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer secret".parse().unwrap(),
        );
        assert!(is_bearer_match(&headers, "secret"));
    }

    #[test]
    fn bearer_match_rejects_wrong_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer wrong".parse().unwrap(),
        );
        assert!(!is_bearer_match(&headers, "secret"));
    }

    #[test]
    fn bearer_match_rejects_missing_header() {
        let headers = HeaderMap::new();
        assert!(!is_bearer_match(&headers, "secret"));
    }

    #[test]
    fn bearer_match_rejects_non_bearer_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Basic secret".parse().unwrap(),
        );
        assert!(!is_bearer_match(&headers, "secret"));
    }

    #[test]
    fn constant_time_eq_basic() {
        assert!(constant_time_eq(b"a", b"a"));
        assert!(!constant_time_eq(b"a", b"b"));
        assert!(!constant_time_eq(b"a", b"aa"));
    }
}
