//! Binary entry point for `deribit-mcp`.
//!
//! Parses CLI arguments, selects the transport (stdio in v0.1-08;
//! HTTP/SSE in v0.1-09), and hands off to the `rmcp` runtime. Treats
//! stdin EOF (or `rmcp`'s reported `QuitReason::Closed`) as a clean
//! shutdown signal.
//!
//! `anyhow` is acceptable here — `main.rs` is the only place in the
//! crate that is allowed to bubble up startup failures to a printed
//! exit message; everywhere else uses `AdapterError`.

#![forbid(unsafe_code)]
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::sync::Arc;

use anyhow::{Context, Result};
use rmcp::ServiceExt;
use rmcp::transport::io::stdio;
use tokio_util::sync::CancellationToken;

use deribit_mcp::config::{Config, Transport};
use deribit_mcp::context::AdapterContext;
use deribit_mcp::http_transport;
use deribit_mcp::observability;
use deribit_mcp::server::DeribitMcpServer;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::load().context("loading configuration")?;

    observability::init(&config);

    let endpoint = config.endpoint.clone();
    let env_label = if endpoint.contains("test.deribit.com") {
        "TESTNET"
    } else {
        "MAINNET"
    };

    let ctx = Arc::new(
        AdapterContext::new(Arc::new(config.clone())).context("building adapter context")?,
    );

    let server = DeribitMcpServer::new(ctx.clone());

    match config.transport {
        Transport::Stdio => {
            tracing::info!(
                target: "deribit_mcp::startup",
                env = env_label,
                endpoint = %endpoint,
                transport = "stdio",
                "starting on {env_label} ({endpoint}); transport=stdio"
            );
            let running = server
                .serve(stdio())
                .await
                .context("starting stdio transport")?;
            let reason = running.waiting().await.context("stdio service exited")?;
            tracing::info!(?reason, "stdio service stopped");
        }
        Transport::Http => {
            let listen = config.http_listen;
            let bearer_status = if config.http_bearer_token.is_some() {
                "set"
            } else {
                "none"
            };
            tracing::info!(
                target: "deribit_mcp::startup",
                env = env_label,
                endpoint = %endpoint,
                transport = "http",
                listen = %listen,
                bearer = bearer_status,
                "starting on {env_label} ({endpoint}); transport=http; listen={listen}; bearer={bearer_status}"
            );

            let cancel = CancellationToken::new();
            let cancel_signal = cancel.clone();
            let cfg = Arc::new(config);
            tokio::spawn(async move {
                if let Ok(()) = tokio::signal::ctrl_c().await {
                    tracing::info!("ctrl-c received; shutting down HTTP transport");
                    cancel_signal.cancel();
                }
            });

            http_transport::serve(cfg, ctx, cancel)
                .await
                .context("HTTP transport")?;
        }
    }

    Ok(())
}
