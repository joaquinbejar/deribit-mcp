//! Authenticated `Account` tool family.
//!
//! All tools in this module have [`ToolClass::Account`] and require
//! credentials configured via `DERIBIT_CLIENT_ID` /
//! `DERIBIT_CLIENT_SECRET` (ADR-0004). The actual tools land in v0.2.
//!
//! [`ToolClass::Account`]: super::ToolClass::Account

use super::ToolRegistry;

/// Register every `Account` tool with the registry.
///
/// v0.1-06 ships an empty body; the real registrations land in v0.2.
pub fn register(_registry: &mut ToolRegistry) {
    // v0.2 will populate this hook.
}
