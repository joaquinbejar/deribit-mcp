//! Configuration surface — CLI + env + `.env` loader.
//!
//! Priority order (first wins):
//! 1. CLI flags
//! 2. Process environment
//! 3. `.env` file via `dotenvy`
//! 4. Built-in defaults

use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;

/// Resolved configuration for `deribit-mcp`.
#[derive(Debug, Clone)]
pub struct Config {
    /// Deribit API endpoint (testnet by default).
    pub endpoint: String,
    /// Client ID for OAuth flow.
    pub client_id: Option<String>,
    /// Client secret for OAuth flow (env/`.env` only).
    pub client_secret: Option<String>,
    /// Enable trading tools (off by default).
    pub allow_trading: bool,
    /// Max order notional in USD (unlimited by default).
    pub max_order_usd: Option<u64>,
    /// MCP transport: `stdio` or `http` (stdio default).
    pub transport: Transport,
    /// HTTP listen address (only used if transport is HTTP).
    pub http_listen: SocketAddr,
    /// HTTP bearer token for auth (optional, env/`.env` only).
    pub http_bearer_token: Option<String>,
    /// Log format: `text` or `json`.
    pub log_format: LogFormat,
}

/// MCP transport selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Standard input/output (default).
    Stdio,
    /// HTTP/SSE.
    Http,
}

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable text (default for stdio).
    Text,
    /// JSON structured logs (default for http).
    Json,
}

impl Config {
    /// Load configuration from CLI args, env, and `.env` file.
    ///
    /// # Errors
    ///
    /// Returns error if parsing fails (invalid addresses, numbers, etc).
    pub fn load() -> anyhow::Result<Self> {
        let args = Args::parse();

        // Load `.env` file first (doesn't override existing env vars).
        if let Some(ref env_file) = args.env_file {
            dotenvy::from_path(env_file).ok(); // Ignore if file doesn't exist.
        } else if std::path::Path::new(".env").exists() {
            dotenvy::dotenv().ok();
        }

        // Resolve each setting in priority order: CLI, env, default.
        let endpoint = args
            .endpoint()
            .or_else(|| std::env::var("DERIBIT_ENDPOINT").ok())
            .unwrap_or_else(|| "https://test.deribit.com".to_string());

        let client_id = args
            .client_id
            .clone()
            .or_else(|| std::env::var("DERIBIT_CLIENT_ID").ok());

        let client_secret = std::env::var("DERIBIT_CLIENT_SECRET").ok();

        let allow_trading = args.allow_trading
            || std::env::var("DERIBIT_ALLOW_TRADING")
                .map(|v| v == "1")
                .unwrap_or(false);

        let max_order_usd = args.max_order_usd.or_else(|| {
            std::env::var("DERIBIT_MAX_ORDER_USD")
                .ok()
                .and_then(|v| v.parse().ok())
        });

        let transport = args
            .transport()
            .or_else(|| {
                std::env::var("DERIBIT_MCP_TRANSPORT")
                    .ok()
                    .and_then(|v| match v.as_str() {
                        "stdio" => Some(Transport::Stdio),
                        "http" => Some(Transport::Http),
                        _ => None,
                    })
            })
            .unwrap_or(Transport::Stdio);

        let http_listen = args
            .listen
            .or_else(|| {
                std::env::var("DERIBIT_HTTP_LISTEN")
                    .ok()
                    .and_then(|v| v.parse().ok())
            })
            .unwrap_or_else(|| {
                "127.0.0.1:8723"
                    .parse()
                    .expect("invalid default listen addr")
            });

        let http_bearer_token = std::env::var("DERIBIT_HTTP_BEARER_TOKEN").ok();

        #[allow(clippy::unnecessary_lazy_evaluations)]
        let log_format = args
            .log_format()
            .or_else(|| {
                std::env::var("DERIBIT_LOG_FORMAT")
                    .ok()
                    .and_then(|v| match v.as_str() {
                        "text" => Some(LogFormat::Text),
                        "json" => Some(LogFormat::Json),
                        _ => None,
                    })
            })
            .unwrap_or_else(|| match transport {
                Transport::Stdio => LogFormat::Text,
                Transport::Http => LogFormat::Json,
            });

        Ok(Self {
            endpoint,
            client_id,
            client_secret,
            allow_trading,
            max_order_usd,
            transport,
            http_listen,
            http_bearer_token,
            log_format,
        })
    }
}

/// CLI arguments parsed via `clap`.
#[derive(Debug, Parser)]
#[command(name = "deribit-mcp")]
#[command(about = "Model Context Protocol server for Deribit")]
#[command(version)]
struct Args {
    /// Deribit endpoint: use --testnet (default) or --mainnet.
    #[arg(long, help = "Use testnet endpoint (default)")]
    testnet: bool,

    /// Use mainnet endpoint instead of testnet.
    #[arg(long, help = "Use mainnet endpoint")]
    mainnet: bool,

    /// Deribit client ID (or DERIBIT_CLIENT_ID env var).
    #[arg(long, help = "Client ID for OAuth")]
    client_id: Option<String>,

    /// Enable trading tools (off by default).
    #[arg(long, help = "Enable trading tools")]
    allow_trading: bool,

    /// Max order notional in USD (unlimited by default).
    #[arg(long, help = "Max order notional in USD")]
    max_order_usd: Option<u64>,

    /// MCP transport: stdio (default) or http.
    #[arg(long, help = "Transport: stdio or http")]
    transport: Option<String>,

    /// HTTP listen address (only for http transport).
    #[arg(long, help = "HTTP listen address")]
    listen: Option<SocketAddr>,

    /// Log format: text or json.
    #[arg(long, help = "Log format: text or json")]
    log_format: Option<String>,

    /// Path to `.env` file (default: `./.env` if exists).
    #[arg(long, help = "Path to .env file")]
    env_file: Option<PathBuf>,
}

impl Args {
    /// Parse CLI arguments.
    fn parse() -> Self {
        <Self as Parser>::parse()
    }

    /// Resolve endpoint from testnet/mainnet flags.
    fn endpoint(&self) -> Option<String> {
        if self.mainnet {
            Some("https://www.deribit.com".to_string())
        } else if self.testnet {
            Some("https://test.deribit.com".to_string())
        } else {
            None
        }
    }

    /// Parse transport flag.
    fn transport(&self) -> Option<Transport> {
        self.transport.as_ref().and_then(|t| match t.as_str() {
            "stdio" => Some(Transport::Stdio),
            "http" => Some(Transport::Http),
            _ => None,
        })
    }

    /// Parse log format flag.
    fn log_format(&self) -> Option<LogFormat> {
        self.log_format.as_ref().and_then(|f| match f.as_str() {
            "text" => Some(LogFormat::Text),
            "json" => Some(LogFormat::Json),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_format_matches_transport() {
        // Text for stdio, JSON for http (default when not specified).
        let stdio_default = match Transport::Stdio {
            Transport::Stdio => LogFormat::Text,
            Transport::Http => LogFormat::Json,
        };
        let http_default = match Transport::Http {
            Transport::Stdio => LogFormat::Text,
            Transport::Http => LogFormat::Json,
        };
        assert_eq!(stdio_default, LogFormat::Text);
        assert_eq!(http_default, LogFormat::Json);
    }
}
