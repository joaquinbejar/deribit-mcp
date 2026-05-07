//! # `deribit-mcp`
//!
//! Model Context Protocol (MCP) server for the Deribit cryptocurrency
//! derivatives platform. Single binary crate that adapts the
//! `deribit-base`, `deribit-http`, and `deribit-websocket` stack
//! (plus `deribit-fix` in v0.6+) onto MCP's tool / resource / prompt
//! surface.
//!
//! This is a **thin adapter** — every MCP tool is a translation step over
//! an upstream call. Auth, rate limiting, reconnect, and wire codecs all
//! live in the sibling crates.
//!
//! See [`doc/ARCHITECTURE.md`](https://github.com/joaquinbejar/deribit-mcp)
//! for the module map and lifecycle. ADR-0001 explains the thin-adapter
//! decision; ADR-0007 explains the single-binary-crate decision.
//!
//! ## Crate layout
//!
//! - [`config`] — CLI argument and `.env` resolution.
//! - [`context`] — `AdapterContext` shared across handlers.
//! - [`error`] — `AdapterError` and `From` impls for upstream errors.
//! - [`server`] — `rmcp` Server scaffold.
//! - [`observability`] — `tracing` setup and secret redaction.
//! - [`tools`] — `Read` / `Account` / `Trading` tool families.
//! - [`resources`] — static and live resource families.
//! - [`prelude`] — curated re-exports for downstream consumers.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(rust_2018_idioms)]
#![warn(clippy::all)]

pub mod config;
pub mod context;
pub mod error;
pub mod http_transport;
pub mod observability;
pub mod prelude;
pub mod resources;
pub mod server;
pub mod tools;

pub use crate::context::AdapterContext;
pub use crate::error::{AdapterError, AuthFailureReason, UpstreamErrorKind};
pub use crate::resources::{
    ResourceContent, ResourceList, ResourceRegistry, ResourceUri, parse_resource_uri,
};
pub use crate::server::{DeribitMcpServer, MCP_PROTOCOL_VERSION};
pub use crate::tools::{ToolClass, ToolEntry, ToolRegistry};
