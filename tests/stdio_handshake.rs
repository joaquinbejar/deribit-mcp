//! Integration test for the stdio transport.
//!
//! Spins the server in-process over a `tokio::io::duplex` pair (the
//! adapter sees one end as its stdin/stdout, the test acts as the
//! client on the other end), sends a single `initialize` JSON-RPC
//! frame, and asserts the response shape and the EOF-driven graceful
//! shutdown.

use std::sync::Arc;
use std::time::Duration;

use deribit_mcp::config::{Config, LogFormat, OrderTransport, Transport};
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
        order_transport: OrderTransport::Http,
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

/// `tools/call` over the wire — exercises the `ServerHandler::call_tool`
/// override. Without the override `rmcp`'s default impl returns
/// `method_not_found`, which silently slips through the existing
/// integration suite (those tests call `ToolRegistry::call` directly).
/// This test catches that regression by going end-to-end through the
/// JSON-RPC envelope.
#[tokio::test]
async fn tools_call_round_trips_over_stdio() {
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

    // 1. initialize — required first frame.
    let init = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "0.0.0" }
        }
    });
    client_writer
        .write_all(format!("{init}\n").as_bytes())
        .await
        .expect("write init");
    // 2. notifications/initialized — handshake completion.
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    client_writer
        .write_all(format!("{initialized}\n").as_bytes())
        .await
        .expect("write initialized");
    // 3. tools/call get_server_time — name-only registry hit. The
    //    handler will fail at the upstream HTTP call (no network
    //    against test.deribit.com from CI) but the response shape
    //    must come back as a structured `CallToolResult` with
    //    `is_error: true`, NOT as a JSON-RPC -32601.
    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": "get_server_time", "arguments": {} }
    });
    client_writer
        .write_all(format!("{call}\n").as_bytes())
        .await
        .expect("write call");
    client_writer.flush().await.expect("flush");

    let init_response = read_one_response(&mut client_reader).await;
    assert_eq!(init_response["id"], 1);
    let call_response = read_one_response(&mut client_reader).await;
    assert_eq!(call_response["id"], 2, "id echoed for tools/call");

    // The key assertion: the server treated `tools/call` as a known
    // method and returned a `result` envelope. A missing override
    // would surface as `error.code = -32601`.
    assert!(
        call_response.get("result").is_some(),
        "tools/call must produce a `result` envelope (would be -32601 \
         method-not-found if `ServerHandler::call_tool` were unbound), \
         got: {call_response}",
    );
    let result = &call_response["result"];
    assert!(
        result.get("content").is_some(),
        "CallToolResult.content present: {result}",
    );
    // Either the upstream call succeeded (`isError: false`) or it
    // failed at the HTTP layer (`isError: true`); both are valid —
    // the test environment may or may not have network. What's NOT
    // valid is the absence of the `isError` discriminator entirely
    // (which would mean we used `CallToolResult::default`). The MCP
    // wire shape is camelCase per the spec.
    assert!(
        result.get("isError").is_some(),
        "CallToolResult.isError present: {result}",
    );
    assert!(
        result.get("structuredContent").is_some(),
        "CallToolResult.structuredContent present: {result}",
    );

    drop(client_writer);
    drop(client_reader);
    timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server completes within timeout on EOF")
        .expect("server task did not panic");
}

async fn read_one_response<R>(reader: &mut R) -> Value
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = BufReader::new(reader);
    let mut line = String::new();
    let n = timeout(Duration::from_secs(5), buf.read_line(&mut line))
        .await
        .expect("response within timeout")
        .expect("read_line");
    assert!(n > 0, "expected a response line on stdout");
    serde_json::from_str(line.trim()).expect("response is JSON")
}
