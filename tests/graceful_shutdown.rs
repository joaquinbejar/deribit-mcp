//! Integration test for graceful shutdown.
//!
//! Spins the HTTP transport on a random local port, drives the
//! `CancellationToken` directly (a stand-in for `SIGTERM` /
//! `SIGINT`), and asserts the server task exits within the grace
//! period.

use std::sync::Arc;
use std::time::{Duration, Instant};

use deribit_mcp::config::{Config, LogFormat, OrderTransport, Transport};
use deribit_mcp::context::AdapterContext;
use deribit_mcp::http_transport;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

fn cfg(listen: std::net::SocketAddr) -> Config {
    Config {
        endpoint: "https://test.deribit.com".to_string(),
        client_id: None,
        client_secret: None,
        allow_trading: false,
        max_order_usd: None,
        transport: Transport::Http,
        http_listen: listen,
        http_bearer_token: None,
        allowed_hosts: Vec::new(),
        log_format: LogFormat::Json,
        order_transport: OrderTransport::Http,
    }
}

#[tokio::test]
async fn cancellation_token_drives_clean_exit_within_grace_period() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    let cancel = CancellationToken::new();
    let cfg_arc = Arc::new(cfg(addr));
    let ctx = Arc::new(AdapterContext::new(cfg_arc.clone()).expect("ctx"));
    let serve_cancel = cancel.clone();
    let handle = tokio::spawn(async move {
        http_transport::serve_with_listener(cfg_arc, ctx, listener, serve_cancel).await
    });

    // Wait for the server to actually serve `/healthz` — a TCP
    // connect alone is not sufficient (the kernel completes the
    // handshake against the bound listener even before axum has
    // begun accepting), so we drive a real application-level probe.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if healthz_ok(addr).await {
            break;
        }
        if Instant::now() >= deadline {
            panic!("HTTP server at {addr} did not respond to /healthz within 5s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Trigger the cancellation as the unit test stand-in for SIGTERM.
    let started = Instant::now();
    cancel.cancel();

    // The server task should observe the cancellation and exit
    // cleanly within a generous grace window.
    let outcome = tokio::time::timeout(Duration::from_secs(5), handle).await;
    let elapsed = started.elapsed();

    let join = outcome.expect("server did not exit within 5s of cancel");
    let result = join.expect("server task panicked");
    result.expect("server returned an AdapterError");

    assert!(
        elapsed < Duration::from_secs(5),
        "graceful shutdown took {elapsed:?}, exceeded the 5s grace period"
    );
}

/// Returns `true` if `GET /healthz` responds with `200 OK`.
async fn healthz_ok(addr: std::net::SocketAddr) -> bool {
    let Ok(mut stream) = TcpStream::connect(addr).await else {
        return false;
    };
    let request = "GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut buf = Vec::with_capacity(64);
    if tokio::time::timeout(Duration::from_secs(1), stream.read_to_end(&mut buf))
        .await
        .is_err()
    {
        return false;
    }
    let text = String::from_utf8_lossy(&buf);
    text.lines()
        .next()
        .is_some_and(|l| l.starts_with("HTTP/1.1 200"))
}
