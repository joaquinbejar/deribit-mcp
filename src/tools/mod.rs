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
//!
//! v0.1-05 ships an empty placeholder [`ToolRegistry`] so the `rmcp`
//! Server scaffold can hold an `Arc<ToolRegistry>`. v0.1-06 fills in
//! the real macro-driven registration plumbing.

use rmcp::model::Tool;

pub mod account;
pub mod public;
pub mod trading;

/// Effect class of an MCP tool.
///
/// Driven by ADR-0003. The class is part of the handler's type, not a
/// runtime field — the registry refuses to register a `Trading` tool
/// without the corresponding feature gate.
///
/// Marked `#[non_exhaustive]` so adding a new class in a future
/// milestone (e.g. an `Admin` class for operational tools) is not a
/// SemVer break for callers outside the crate. Matches inside the
/// adapter remain exhaustive — the project's coding rules only forbid
/// `_` arms on the closed-set enums explicitly enumerated there
/// (`AdapterError`, `Side`, `OrderType`, `OrderState`,
/// `InstrumentKind`).
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

/// Registry of MCP tools the server exposes.
///
/// Frozen for the lifetime of the process: built at startup, read
/// concurrently by every dispatch. The v0.1-05 implementation is an
/// empty stub; v0.1-06 replaces it with the real handler-keyed
/// dispatcher.
#[derive(Debug, Default, Clone)]
pub struct ToolRegistry {
    tools: Vec<Tool>,
}

impl ToolRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot the current tool list for a `tools/list` response.
    #[must_use]
    pub fn list(&self) -> Vec<Tool> {
        self.tools.clone()
    }

    /// Number of registered tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether the registry has any tools registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
