//! Binary entry point for `deribit-mcp`.
//!
//! Parses CLI arguments, selects the transport (stdio or HTTP/SSE), and
//! hands off to the `rmcp` runtime. The full wiring lands incrementally
//! across v0.1-02 (config), v0.1-03 (observability), v0.1-05 (`rmcp`
//! scaffold), v0.1-08 (stdio), v0.1-09 (HTTP/SSE), and v0.1-13
//! (graceful shutdown).
//!
//! `anyhow` is acceptable here — it is the only place in the crate that
//! is allowed to bubble up startup failures to a printed exit message
//! (`rules/global_rules.md` — Error Handling).

#![forbid(unsafe_code)]

use anyhow::Result;

fn main() -> Result<()> {
    // The async runtime + transport selection wiring lands in v0.1-08 /
    // v0.1-09. v0.1-01 only delivers the binary entry point so other
    // tickets can land independently.
    eprintln!(
        "deribit-mcp v{} — scaffolding only; transport wiring lands in v0.1-08/v0.1-09.",
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}
