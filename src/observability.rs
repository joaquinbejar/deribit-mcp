//! Tracing setup and secret redaction — placeholder for v0.1-03.
//!
//! The full implementation initialises a `tracing-subscriber` with the
//! `RUST_LOG` env filter, a fmt layer (text on stderr for stdio mode,
//! JSON on stdout for HTTP mode), and a redaction filter that strips
//! `client_secret`, `access_token`, `refresh_token`, and
//! `http_bearer_token` from event fields.

/// Initialise tracing — placeholder.
///
/// Replaced in v0.1-03 with the real subscriber wiring described in
/// `doc/ARCHITECTURE.md` §6 (Observability).
pub fn init_placeholder() {}
