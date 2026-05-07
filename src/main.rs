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

use deribit_mcp::config::{Config, Transport};
use deribit_mcp::context::AdapterContext;
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

    let server = DeribitMcpServer::new(ctx);

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
            tracing::error!("HTTP transport lands in v0.1-09");
            anyhow::bail!("HTTP transport not yet implemented (v0.1-09)");
        }
    }

    Ok(())
}
