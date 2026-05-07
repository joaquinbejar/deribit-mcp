//! Adapter error type — placeholder for v0.1-04.
//!
//! `AdapterError` is the only error type allowed to cross the MCP
//! boundary. The full enum (with `From` impls for upstream errors) lands
//! in v0.1-04.

use thiserror::Error;

/// Errors emitted by the adapter.
///
/// This is the canonical adapter error type. Upstream errors are mapped
/// into one of these variants via `From` impls so the MCP layer never
/// sees a raw `deribit_http::HttpError` or `deribit_websocket::WsError`.
///
/// The full variant set is defined in v0.1-04. The `Internal` variant
/// here is a temporary catch-all so the scaffold can be referenced.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AdapterError {
    /// A failure that does not fit any structured variant. Replaced in
    /// v0.1-04 by the structured set documented in
    /// `rules/global_rules.md` (`Auth`, `RateLimited`, `Validation`,
    /// `SizeCapExceeded`, `NotEnabled`, …).
    #[error("internal error: {message}")]
    Internal {
        /// Human-readable description; secrets are redacted upstream.
        message: String,
    },
}
