//! Tool registry and effect-class gating.
//!
//! Tools are partitioned by effect class — see ADR-0003:
//!
//! - [`public`] — `Read` tools with no auth requirement.
//! - [`account`] — `Account` tools that require credentials.
//! - [`trading`] — `Trading` tools gated by `--allow-trading`.
//!
//! The registry is built once at startup from the configured class set.
//! A tool absent from the registry is uninvokable; this is the first
//! line of defence for the trading opt-in (ADR-0010).

pub mod account;
pub mod public;
pub mod trading;

/// Effect class of an MCP tool.
///
/// Driven by ADR-0003. The class is part of the handler's type, not a
/// runtime field — the registry refuses to register a `Trading` tool
/// without the corresponding feature gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ToolClass {
    /// Read-only public market data. No auth required.
    Read,
    /// Authenticated account-scoped reads.
    Account,
    /// Trading-class actions. Requires `--allow-trading` and credentials.
    Trading,
}
