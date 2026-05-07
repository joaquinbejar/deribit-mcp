//! Integration test for the HTTP transport.
//!
//! Spins the HTTP server on a random local port, exercises:
//!
//! - `GET /healthz` → 200 OK (always unauthenticated).
//! - `POST /mcp` without a bearer token (when the server requires
//!   one) → 401.
//! - `POST /mcp` with the configured bearer token → not 401 (the
//!   `initialize` round-trip itself is verified by the broader
//!   integration suite in v0.1-15).

use std::sync::Arc;
use std::time::Duration;

use deribit_mcp::config::{Config, LogFormat, Transport};
use deribit_mcp::context::AdapterContext;
use deribit_mcp::http_transport;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

fn cfg(listen: std::net::SocketAddr, bearer: Option<&str>) -> Config {
    Config {
        endpoint: "https://test.deribit.com".to_string(),
        client_id: None,
        client_secret: None,
        allow_trading: false,
        max_order_usd: None,
        transport: Transport::Http,
        http_listen: listen,
        http_bearer_token: bearer.map(str::to_string),
        log_format: LogFormat::Json,
    }
}

async fn pick_free_port() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);
    addr
}

#[tokio::test]
async fn healthz_is_always_200() {
    let addr = pick_free_port().await;
    let cancel = CancellationToken::new();
    let cfg = cfg(addr, Some("supersecret"));

    let ctx = Arc::new(AdapterContext::new(Arc::new(cfg.clone())).expect("ctx"));
    let cfg_arc = Arc::new(cfg);
    let serve_cancel = cancel.clone();
    let server =
        tokio::spawn(async move { http_transport::serve(cfg_arc, ctx, serve_cancel).await });

    // Give the server a moment to bind.
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let response = simple_get(addr, "/healthz", None).await;
    assert_eq!(response.status, 200, "healthz returns 200");

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
}

#[tokio::test]
async fn mcp_without_bearer_returns_401() {
    let addr = pick_free_port().await;
    let cancel = CancellationToken::new();
    let cfg = cfg(addr, Some("supersecret"));

    let ctx = Arc::new(AdapterContext::new(Arc::new(cfg.clone())).expect("ctx"));
    let cfg_arc = Arc::new(cfg);
    let serve_cancel = cancel.clone();
    let server =
        tokio::spawn(async move { http_transport::serve(cfg_arc, ctx, serve_cancel).await });
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let response = simple_post(addr, "/mcp", None, "{}").await;
    assert_eq!(response.status, 401);

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
}

#[tokio::test]
async fn mcp_with_correct_bearer_is_not_401() {
    let addr = pick_free_port().await;
    let cancel = CancellationToken::new();
    let cfg = cfg(addr, Some("supersecret"));

    let ctx = Arc::new(AdapterContext::new(Arc::new(cfg.clone())).expect("ctx"));
    let cfg_arc = Arc::new(cfg);
    let serve_cancel = cancel.clone();
    let server =
        tokio::spawn(async move { http_transport::serve(cfg_arc, ctx, serve_cancel).await });
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let response = simple_post(addr, "/mcp", Some("supersecret"), "{}").await;
    assert_ne!(response.status, 401, "correct bearer must not be 401");

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
}

#[tokio::test]
async fn mcp_without_bearer_token_disabled_passes_through() {
    let addr = pick_free_port().await;
    let cancel = CancellationToken::new();
    let cfg = cfg(addr, None); // no bearer configured

    let ctx = Arc::new(AdapterContext::new(Arc::new(cfg.clone())).expect("ctx"));
    let cfg_arc = Arc::new(cfg);
    let serve_cancel = cancel.clone();
    let server =
        tokio::spawn(async move { http_transport::serve(cfg_arc, ctx, serve_cancel).await });
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let response = simple_post(addr, "/mcp", None, "{}").await;
    assert_ne!(
        response.status, 401,
        "no bearer configured → no 401 enforcement"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
}

// -------- minimal blocking-ish HTTP/1.1 client over tokio TCP --------
//
// We intentionally avoid `reqwest` in tests because rmcp already pulls
// it transitively and the dev-dependency would add an ambiguous
// resolver bump. The client only speaks the bare HTTP/1.1 features
// the test needs.

struct Resp {
    status: u16,
}

async fn simple_get(addr: std::net::SocketAddr, path: &str, bearer: Option<&str>) -> Resp {
    raw_request(addr, "GET", path, bearer, "").await
}

async fn simple_post(
    addr: std::net::SocketAddr,
    path: &str,
    bearer: Option<&str>,
    body: &str,
) -> Resp {
    raw_request(addr, "POST", path, bearer, body).await
}

async fn raw_request(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: &str,
) -> Resp {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = TcpStream::connect(addr).await.expect("connect");
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: application/json\r\n");
    if let Some(token) = bearer {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if !body.is_empty() {
        request.push_str("Content-Type: application/json\r\n");
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("Connection: close\r\n\r\n");
    request.push_str(body);

    stream.write_all(request.as_bytes()).await.expect("write");
    stream.flush().await.expect("flush");

    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf)).await;
    let text = String::from_utf8_lossy(&buf);
    let status_line = text.lines().next().unwrap_or("HTTP/1.1 0 ?");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Resp { status }
}

use tokio::net::TcpStream;
