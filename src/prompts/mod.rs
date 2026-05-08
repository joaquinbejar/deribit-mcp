//! Prompt registry, descriptor list, and dispatch.
//!
//! v0.5-01 ships the registry plumbing and handshake glue. The
//! concrete prompts (`daily_options_summary`, `funding_snapshot`,
//! `position_review`) land in v0.5-02 / v0.5-03 / v0.5-04.
//!
//! Prompts are registered into a [`PromptRegistry`] built once at
//! startup and frozen for the lifetime of the process. A prompt
//! absent from the registry is uninvokable; `prompts/get
//! unknown_name` returns a structured `Validation { field: "name" }`
//! [`AdapterError`] which the rmcp boundary translates into an MCP
//! error.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use rmcp::model::{GetPromptResult, JsonObject, Prompt};

use crate::context::AdapterContext;
use crate::error::AdapterError;

/// Boxed dynamic future returned by every prompt handler.
pub type PromptFuture<'a> =
    Pin<Box<dyn Future<Output = Result<GetPromptResult, AdapterError>> + Send + 'a>>;

/// A prompt handler invocation: takes the shared context and a JSON
/// object of caller arguments and returns the rendered MCP prompt
/// payload (or an [`AdapterError`]).
pub type PromptHandlerFn =
    Arc<dyn for<'a> Fn(&'a AdapterContext, JsonObject) -> PromptFuture<'a> + Send + Sync + 'static>;

/// One registered prompt: its MCP descriptor and async handler.
///
/// Fields are `pub(crate)` so external callers cannot bypass
/// [`PromptRegistry::get`] by invoking the handler directly.
/// Read-only accessors expose the bits external callers
/// (integration tests, listing) actually need.
#[derive(Clone)]
pub struct PromptEntry {
    pub(crate) descriptor: Prompt,
    pub(crate) handler: PromptHandlerFn,
}

impl PromptEntry {
    /// MCP `Prompt` descriptor returned by `prompts/list`.
    #[must_use]
    pub fn descriptor(&self) -> &Prompt {
        &self.descriptor
    }
}

impl std::fmt::Debug for PromptEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromptEntry")
            .field("descriptor", &self.descriptor)
            .field("handler", &"<dyn Fn>")
            .finish()
    }
}

/// Registry of MCP prompts the server exposes.
///
/// Frozen for the lifetime of the process: built at startup, read
/// concurrently by every dispatch.
#[derive(Debug, Default, Clone)]
pub struct PromptRegistry {
    entries: HashMap<String, PromptEntry>,
}

impl PromptRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the registry for a given context. v0.5-01 ships an
    /// empty body; v0.5-02 / v0.5-03 / v0.5-04 populate the three
    /// concrete prompts.
    #[must_use]
    pub fn build(_ctx: &AdapterContext) -> Self {
        Self::new()
    }

    /// Insert a prompt. Returns the previous entry under the same
    /// name, if any (caller treats that as a programmer error).
    ///
    /// `pub(crate)` because the registry's invariant is *frozen for
    /// the lifetime of the process after [`Self::build`]*.
    #[allow(dead_code)]
    pub(crate) fn insert(&mut self, entry: PromptEntry) -> Option<PromptEntry> {
        let name = entry.descriptor.name.clone();
        self.entries.insert(name, entry)
    }

    /// Snapshot the current prompt list for a `prompts/list`
    /// response. Sorted by name for deterministic output.
    #[must_use]
    pub fn list(&self) -> Vec<Prompt> {
        let mut prompts: Vec<Prompt> = self
            .entries
            .values()
            .map(|e| e.descriptor.clone())
            .collect();
        prompts.sort_by(|a, b| a.name.cmp(&b.name));
        prompts
    }

    /// Number of registered prompts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry has any prompts registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether a prompt with this name is registered.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Look up a registered entry by name.
    #[must_use]
    pub fn get_entry(&self, name: &str) -> Option<&PromptEntry> {
        self.entries.get(name)
    }

    /// Render a registered prompt.
    ///
    /// # Errors
    ///
    /// - [`AdapterError::Validation`] with `field = "name"` when no
    ///   prompt is registered under `name` — translated by the
    ///   `rmcp` boundary into an MCP "method not found"-style error.
    /// - Whatever the handler returns (validation, upstream).
    pub async fn get(
        &self,
        ctx: &AdapterContext,
        name: &str,
        args: JsonObject,
    ) -> Result<GetPromptResult, AdapterError> {
        let Some(entry) = self.entries.get(name) else {
            return Err(AdapterError::Validation {
                field: "name".to_string(),
                message: format!("prompt `{name}` is not registered"),
            });
        };
        (entry.handler)(ctx, args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, LogFormat, Transport};
    use std::net::SocketAddr;

    fn ctx() -> Arc<AdapterContext> {
        let cfg = Config {
            endpoint: "https://test.deribit.com".to_string(),
            client_id: None,
            client_secret: None,
            allow_trading: false,
            max_order_usd: None,
            transport: Transport::Stdio,
            http_listen: SocketAddr::from(([127, 0, 0, 1], 8723)),
            http_bearer_token: None,
            log_format: LogFormat::Text,
        };
        Arc::new(AdapterContext::new(Arc::new(cfg)).expect("ctx"))
    }

    #[test]
    fn empty_registry_lists_nothing() {
        let registry = PromptRegistry::build(&ctx());
        assert!(registry.is_empty());
        assert_eq!(registry.list().len(), 0);
    }

    #[tokio::test]
    async fn unknown_prompt_returns_validation() {
        let registry = PromptRegistry::build(&ctx());
        let err = registry
            .get(&ctx(), "no_such_prompt", JsonObject::new())
            .await
            .unwrap_err();
        match err {
            AdapterError::Validation { field, .. } => assert_eq!(field, "name"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
