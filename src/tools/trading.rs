//! `Trading` tool family.
//!
//! All tools in this module have [`ToolClass::Trading`]. The registry
//! refuses to register them unless `--allow-trading` is passed
//! (ADR-0010), and `--max-order-usd` caps notional size. Actual tools
//! land in v0.4.
//!
//! [`ToolClass::Trading`]: super::ToolClass::Trading

use super::ToolRegistry;

/// Register every `Trading` tool with the registry.
///
/// v0.1-06 ships an empty body; the real registrations land in v0.4.
pub fn register(_registry: &mut ToolRegistry) {
    // v0.4 will populate this hook.
}
