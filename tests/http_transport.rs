//! Integration test for the HTTP transport.
//!
//! Spins the HTTP server on an OS-chosen loopback port (passed in as
//! a pre-bound `tokio::net::TcpListener` so there is no race between
//! `pick_free_port` and the server binding the same port), and
//! exercises:
//!
//! - `GET /healthz` → 200 OK (always unauthenticated).
//! - `POST /mcp` without a bearer token (when the server requires
//!   one) → 401.
//! - `POST /mcp` with the configured bearer token → not 401 (the
//!   `initialize` round-trip itself is verified by the broader
//!   integration suite in v0.1-15).
//! - Unknown paths surface a natural 404 even when bearer auth is
//!   configured (the middleware scoped only to `/mcp`).

use std::sync::Arc;
use std::time::Duration;

use deribit_mcp::config::{Config, LogFormat, OrderTransport, Transport};
use deribit_mcp::context::AdapterContext;
use deribit_mcp::http_transport;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
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
        allowed_hosts: Vec::new(),
        log_format: LogFormat::Json,
        order_transport: OrderTransport::Http,
    }
}

/// Spin a server bound to a `127.0.0.1:0` listener and return the
/// chosen address plus a join handle and a cancellation token.
async fn spawn_server(
    bearer: Option<&'static str>,
) -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<()>,
    CancellationToken,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let cancel = CancellationToken::new();
    let cfg_arc = Arc::new(cfg(addr, bearer));
    let ctx = Arc::new(AdapterContext::new(cfg_arc.clone()).expect("ctx"));
    let serve_cancel = cancel.clone();
    let handle = tokio::spawn(async move {
        let _ = http_transport::serve_with_listener(cfg_arc, ctx, listener, serve_cancel).await;
    });

    // Wait until the server actually accepts connections, with a
    // hard timeout that surfaces a clear failure if it never binds.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if TcpStream::connect(addr).await.is_ok() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("HTTP server at {addr} did not become reachable within 5s");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (addr, handle, cancel)
}

async fn shutdown(handle: tokio::task::JoinHandle<()>, cancel: CancellationToken) {
    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
}

#[tokio::test]
async fn healthz_is_always_200() {
    let (addr, handle, cancel) = spawn_server(Some("supersecret")).await;
    let response = simple_get(addr, "/healthz", None).await;
    assert_eq!(response.status, 200, "healthz returns 200");
    shutdown(handle, cancel).await;
}

#[tokio::test]
async fn mcp_without_bearer_returns_401() {
    let (addr, handle, cancel) = spawn_server(Some("supersecret")).await;
    let response = simple_post(addr, "/mcp", None, "{}").await;
    assert_eq!(response.status, 401);
    shutdown(handle, cancel).await;
}

#[tokio::test]
async fn mcp_with_correct_bearer_is_not_401() {
    let (addr, handle, cancel) = spawn_server(Some("supersecret")).await;
    let response = simple_post(addr, "/mcp", Some("supersecret"), "{}").await;
    assert_ne!(response.status, 401, "correct bearer must not be 401");
    shutdown(handle, cancel).await;
}

#[tokio::test]
async fn mcp_without_bearer_token_disabled_passes_through() {
    let (addr, handle, cancel) = spawn_server(None).await;
    let response = simple_post(addr, "/mcp", None, "{}").await;
    assert_ne!(
        response.status, 401,
        "no bearer configured → no 401 enforcement"
    );
    shutdown(handle, cancel).await;
}

#[tokio::test]
async fn unknown_path_is_404_not_401_even_with_bearer() {
    let (addr, handle, cancel) = spawn_server(Some("supersecret")).await;
    let response = simple_get(addr, "/no-such-route", None).await;
    assert_eq!(
        response.status, 404,
        "unknown paths surface 404, not the bearer-auth 401"
    );
    shutdown(handle, cancel).await;
}

// -------- minimal HTTP/1.1 client over tokio TCP --------
//
// We avoid `reqwest` because rmcp pulls it transitively and
// dev-pinning it would add an ambiguous resolver bump. The client
// only speaks the bare HTTP/1.1 features the test needs.

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
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf)).await;
    let read = read
        .expect("HTTP response body did not arrive within 5s")
        .expect("read");
    assert!(read > 0, "expected at least the status line");
    let text = String::from_utf8_lossy(&buf);
    let status_line = text
        .lines()
        .next()
        .expect("HTTP response carried no status line");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("malformed status line: {status_line:?}"));
    Resp { status }
}
