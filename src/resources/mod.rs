//! Resource registry and `deribit://` URI parsing.
//!
//! Resources are partitioned by lifetime:
//!
//! - [`static_`] — refresh-on-read resources backed by `deribit-http`.
//! - [`live`] — WebSocket-backed subscribable resources (lands in v0.3
//!   per ADR-0006).
//!
//! v0.1-05 ships an empty placeholder [`ResourceRegistry`] so the
//! `rmcp` Server scaffold can hold an `Arc<ResourceRegistry>`. v0.1-07
//! fills in the URI parser and the static-resource catalogue.

use rmcp::model::{Resource, ResourceTemplate};

pub mod live;
pub mod static_;

/// Registry of MCP resources the server exposes.
///
/// Frozen for the lifetime of the process. The v0.1-05 implementation
/// is an empty stub; v0.1-07 replaces it with the URI-template
/// dispatcher.
#[derive(Debug, Default, Clone)]
pub struct ResourceRegistry {
    resources: Vec<Resource>,
    templates: Vec<ResourceTemplate>,
}

impl ResourceRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot the registered resources.
    #[must_use]
    pub fn resources(&self) -> Vec<Resource> {
        self.resources.clone()
    }

    /// Snapshot the registered resource templates.
    #[must_use]
    pub fn templates(&self) -> Vec<ResourceTemplate> {
        self.templates.clone()
    }

    /// Whether the registry has any resources or templates registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty() && self.templates.is_empty()
    }
}
