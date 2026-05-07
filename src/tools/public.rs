//! Public `Read` tool family.
//!
//! All tools in this module have [`ToolClass::Read`] and require no
//! credentials. The actual market-data tools (`get_ticker`,
//! `get_order_book`, `get_instruments`, …) are implemented in v0.1-10
//! and v0.1-11.
//!
//! [`ToolClass::Read`]: super::ToolClass::Read

use super::ToolRegistry;

/// Register every `Read` tool with the registry.
///
/// v0.1-06 ships an empty body — this is the registration hook for
/// v0.1-10 / v0.1-11.
pub fn register(_registry: &mut ToolRegistry) {
    // v0.1-10 / v0.1-11 will populate this hook.
}
