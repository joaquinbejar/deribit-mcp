//! Binary entry point for `deribit-mcp`.
//!
//! Parses CLI arguments, selects the transport (stdio or HTTP/SSE),
//! and hands off to the `rmcp` runtime. Honors SIGINT / SIGTERM for
//! clean shutdown per ADR-0011.
//!
//! Exit codes:
//!
//! - `0` — clean shutdown (EOF on stdin, SIGINT, SIGTERM, or HTTP
//!   server exited cleanly via the cancellation token).
//! - `1` — startup-config error (failed to load `Config`, build
//!   `AdapterContext`, or bind a transport).
//! - `2` — reserved for upstream auth failure on the first
//!   authenticated call. v0.2 wires this when the first authenticated
//!   tool call surfaces an `AdapterError::Auth` at startup.
//!
//! `anyhow` is acceptable here — `main.rs` is the only place in the
//! crate that is allowed to bubble up startup failures to a printed
//! exit message; everywhere else uses `AdapterError`.

#![forbid(unsafe_code)]
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;
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

/// Process exit code for a startup-time configuration failure.
const EXIT_CONFIG_ERROR: u8 = 1;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::from(0),
        Err(err) => {
            // The error path is intentionally allowed to use stderr;
            // tracing may not be initialised yet (Config::load can
            // fail) so we print a structured one-liner directly.
            eprintln!("deribit-mcp: startup error: {err:#}");
            ExitCode::from(EXIT_CONFIG_ERROR)
        }
    }
}

async fn run() -> Result<()> {
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

    let shutdown = CancellationToken::new();
    install_signal_handlers(shutdown.clone());

    match config.transport {
        Transport::Stdio => {
            tracing::info!(
                target: "deribit_mcp::startup",
                env = env_label,
                endpoint = %endpoint,
                transport = "stdio",
                "starting on {env_label} ({endpoint}); transport=stdio"
            );
            // rmcp drives its own EOF handling; we additionally wire a
            // signal-driven cancel so SIGINT / SIGTERM trigger the same
            // clean exit (the spawned cancel-on-signal task above).
            let running = server
                .serve_with_ct(stdio(), shutdown.clone())
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

            let cfg = Arc::new(config);
            http_transport::serve(cfg, ctx, shutdown)
                .await
                .context("HTTP transport")?;
        }
    }

    Ok(())
}

/// Install a task that cancels `shutdown` when SIGINT (Ctrl-C) or
/// SIGTERM (Unix only) is received. Idempotent: the first signal
/// cancels; further signals are no-ops because the shared
/// `CancellationToken` only cancels once.
fn install_signal_handlers(shutdown: CancellationToken) {
    tokio::spawn(async move {
        let signal = first_termination_signal().await;
        tracing::info!(?signal, "shutdown signal received; cancelling");
        shutdown.cancel();
    });
}

/// Wait for whichever termination signal arrives first.
///
/// On Unix that's SIGINT or SIGTERM; on Windows we fall back to
/// SIGINT (Ctrl-C) only.
async fn first_termination_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(error = %err, "failed to install SIGTERM handler; SIGINT only");
                let _ = tokio::signal::ctrl_c().await;
                return "SIGINT";
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => "SIGINT",
            _ = sigterm.recv() => "SIGTERM",
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        "SIGINT"
    }
}
