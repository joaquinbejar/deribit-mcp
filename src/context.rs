//! Shared adapter context — placeholder for v0.1-04.
//!
//! `AdapterContext` will hold the upstream `deribit-http` client, the
//! optional `deribit-websocket` handle, the configuration snapshot, and
//! the shared tracing handle. The full implementation lands in v0.1-04.

/// Shared adapter context placeholder.
///
/// Replaced in v0.1-04 with the real `AdapterContext` that owns
/// upstream clients and configuration. Kept here so the scaffold
/// compiles without the implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct AdapterContext;

impl AdapterContext {
    /// Construct an empty placeholder context.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}
