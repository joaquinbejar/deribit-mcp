//! Integration test for the stdio transport.
//!
//! Spins the server in-process over a `tokio::io::duplex` pair (the
//! adapter sees one end as its stdin/stdout, the test acts as the
//! client on the other end), sends a single `initialize` JSON-RPC
//! frame, and asserts the response shape and the EOF-driven graceful
//! shutdown.

use std::sync::Arc;
use std::time::Duration;

use deribit_mcp::config::{Config, LogFormat, Transport};
use deribit_mcp::context::AdapterContext;
use deribit_mcp::server::DeribitMcpServer;
use rmcp::ServiceExt;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::timeout;

fn test_config() -> Config {
    Config {
        endpoint: "https://test.deribit.com".to_string(),
        client_id: None,
        client_secret: None,
        allow_trading: false,
        max_order_usd: None,
        transport: Transport::Stdio,
        http_listen: "127.0.0.1:8723".parse().unwrap(),
        http_bearer_token: None,
        log_format: LogFormat::Text,
    }
}

#[tokio::test]
async fn initialize_handshake_round_trips_over_in_memory_stdio() {
    // Two duplex pipes: one for client→server (the server reads from
    // its "stdin"), one for server→client (the server writes to its
    // "stdout").
    let (server_stdin_reader, mut client_writer) = tokio::io::duplex(8192);
    let (mut client_reader, server_stdout_writer) = tokio::io::duplex(8192);

    let ctx = Arc::new(AdapterContext::new(Arc::new(test_config())).expect("adapter context"));
    let server = DeribitMcpServer::new(ctx);

    let server_task = tokio::spawn(async move {
        let running = server
            .serve((server_stdin_reader, server_stdout_writer))
            .await
            .expect("serve");
        running.waiting().await.expect("waiting");
    });

    // Send a single `initialize` request.
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "0.0.0" }
        }
    });
    let line = format!("{request}\n");
    client_writer
        .write_all(line.as_bytes())
        .await
        .expect("write");
    client_writer.flush().await.expect("flush");

    // Read one JSON-RPC line back. Scope the `BufReader` so its
    // mutable borrow on `client_reader` ends before we drop the
    // reader to signal EOF below.
    let response_line = {
        let mut reader = BufReader::new(&mut client_reader);
        let mut line = String::new();
        let n = timeout(Duration::from_secs(5), reader.read_line(&mut line))
            .await
            .expect("response line within timeout")
            .expect("read_line");
        assert!(n > 0, "expected a response line on stdout");
        line
    };

    let response: Value = serde_json::from_str(response_line.trim()).expect("response is JSON");

    assert_eq!(response["jsonrpc"], "2.0", "JSON-RPC envelope");
    assert_eq!(response["id"], 1, "id echoed");
    assert_eq!(
        response["result"]["protocolVersion"], "2025-06-18",
        "protocol version pinned"
    );
    assert_eq!(
        response["result"]["serverInfo"]["name"], "deribit-mcp",
        "serverInfo.name"
    );
    let resources = &response["result"]["capabilities"]["resources"];
    assert_eq!(
        resources["subscribe"], true,
        "resources.subscribe advertised"
    );
    assert!(
        response["result"]["capabilities"]["tools"].is_object(),
        "tools capability advertised"
    );
    assert!(
        response["result"]["capabilities"]["prompts"].is_object(),
        "prompts capability advertised from v0.5-01"
    );

    // Drop the client writer so the server's stdin sees EOF and the
    // running service finishes cleanly.
    drop(client_writer);
    drop(client_reader);

    timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server completes within timeout on EOF")
        .expect("server task did not panic");
}
