//! Tool registry, effect-class gating, and dispatch.
//!
//! Tools are partitioned by effect class (ADR-0003):
//!
//! - [`public`] — `Read` tools with no auth requirement.
//! - [`account`] — `Account` tools that require credentials.
//! - [`trading`] — `Trading` tools that require credentials **and**
//!   `--allow-trading`.
//!
//! The registry is built once at startup from the configured class set
//! and frozen for the lifetime of the process. A tool absent from the
//! registry is uninvokable; this is the first line of defence for the
//! trading opt-in (ADR-0010). Dispatch performs a defence-in-depth
//! class re-check before calling the handler.
//!
//! v0.1-06 ships the registry plumbing and dispatch glue. The actual
//! `Read` tools land in v0.1-10 / v0.1-11; `Account` in v0.2;
//! `Trading` in v0.4.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use rmcp::model::Tool;
use serde_json::Value;

use crate::context::AdapterContext;
use crate::error::AdapterError;

pub mod account;
pub mod public;
pub mod schema;
pub mod trading;

/// Effect class of an MCP tool.
///
/// Driven by ADR-0003. The class is part of the handler's type, not a
/// runtime field — the registry refuses to register a `Trading` tool
/// without the corresponding feature gate.
///
/// Marked `#[non_exhaustive]` so adding a new class in a future
/// milestone (e.g. an `Admin` class) is not a SemVer break for callers
/// outside the crate. Internal matches stay exhaustive.
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

impl ToolClass {
    /// CLI flag that enables this class. Used in
    /// [`AdapterError::NotEnabled`] payloads so the LLM knows what is
    /// missing.
    #[must_use]
    pub const fn flag(self) -> &'static str {
        match self {
            Self::Read => "(always enabled)",
            Self::Account => "DERIBIT_CLIENT_ID + DERIBIT_CLIENT_SECRET",
            Self::Trading => "--allow-trading",
        }
    }
}

/// Boxed dynamic future returned by every tool handler.
///
/// We use a `Pin<Box<dyn Future>>` rather than `impl Future` so all
/// handlers share one type and can live in the same registry map.
pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, AdapterError>> + Send + 'a>>;

/// A handler invocation: takes the shared context and the JSON
/// arguments and returns the JSON output (or an [`AdapterError`]).
pub type ToolHandlerFn =
    Arc<dyn for<'a> Fn(&'a AdapterContext, Value) -> ToolFuture<'a> + Send + Sync + 'static>;

/// One registered tool: its MCP descriptor, effect class, and handler.
///
/// Fields are `pub(crate)` so external callers cannot bypass
/// [`ToolRegistry::call`]'s class gate by invoking the handler
/// directly. Read-only accessors expose the bits external callers
/// (integration tests, listing) actually need.
#[derive(Clone)]
pub struct ToolEntry {
    /// MCP `Tool` descriptor (name, description, schemas) returned by
    /// `tools/list`.
    pub(crate) descriptor: Tool,
    /// Effect class. Re-checked at dispatch time.
    pub(crate) class: ToolClass,
    /// Async handler invoked by `tools/call`.
    pub(crate) handler: ToolHandlerFn,
}

impl ToolEntry {
    /// MCP `Tool` descriptor returned by `tools/list`.
    #[must_use]
    pub fn descriptor(&self) -> &Tool {
        &self.descriptor
    }

    /// Effect class of this tool.
    #[must_use]
    pub fn class(&self) -> ToolClass {
        self.class
    }
}

impl std::fmt::Debug for ToolEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolEntry")
            .field("descriptor", &self.descriptor)
            .field("class", &self.class)
            .field("handler", &"<dyn Fn>")
            .finish()
    }
}

/// Registry of MCP tools the server exposes.
///
/// Frozen for the lifetime of the process: built at startup, read
/// concurrently by every dispatch.
#[derive(Debug, Default, Clone)]
pub struct ToolRegistry {
    entries: HashMap<String, ToolEntry>,
}

impl ToolRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the registry for a given context.
    ///
    /// Class-driven gating:
    ///
    /// - `Read` tools are always registered.
    /// - `Account` tools are registered only when the configured
    ///   credentials are present.
    /// - `Trading` tools are registered only when both credentials are
    ///   present **and** `--allow-trading` is set.
    ///
    /// Tools whose class is not currently enabled are simply not
    /// inserted; defence-in-depth at dispatch time covers any future
    /// path that might add them outside this builder.
    #[must_use]
    pub fn build(ctx: &AdapterContext) -> Self {
        let mut registry = Self::new();
        public::register(&mut registry);
        if ctx.has_credentials() {
            account::register(&mut registry);
        }
        if ctx.has_credentials() && ctx.config.allow_trading {
            trading::register(&mut registry);
        }
        registry
    }

    /// Insert a tool. Returns the previous entry under the same name,
    /// if any (caller treats that as a programmer error).
    ///
    /// `pub(crate)` because the registry's invariant is *frozen for
    /// the lifetime of the process after [`Self::build`]*. The only
    /// callers are the per-family `register()` hooks invoked from
    /// [`Self::build`]. External callers go through `build` so the
    /// class gating is always applied.
    ///
    /// `allow(dead_code)` because the per-family `register()` hooks
    /// are empty in v0.1-06 — they fill in over v0.1-10 (`Read`),
    /// v0.2 (`Account`), and v0.4 (`Trading`).
    #[allow(dead_code)]
    pub(crate) fn insert(&mut self, entry: ToolEntry) -> Option<ToolEntry> {
        let name = entry.descriptor.name.to_string();
        self.entries.insert(name, entry)
    }

    /// Snapshot the current tool list for a `tools/list` response.
    #[must_use]
    pub fn list(&self) -> Vec<Tool> {
        let mut tools: Vec<Tool> = self
            .entries
            .values()
            .map(|e| e.descriptor.clone())
            .collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        tools
    }

    /// Number of registered tools.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry has any tools registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up a tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ToolEntry> {
        self.entries.get(name)
    }

    /// Whether a tool of the given name is registered.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Dispatch a `tools/call`.
    ///
    /// Returns:
    ///
    /// - `Ok(Value)` on success.
    /// - [`AdapterError::NotEnabled`] when the tool is registered but
    ///   the class precondition is not currently satisfied
    ///   (defence-in-depth re-check after registration).
    /// - [`AdapterError::Validation`] when no tool by that name is
    ///   registered.
    ///
    /// # Errors
    ///
    /// Surfaces any [`AdapterError`] returned by the handler.
    pub async fn call(
        &self,
        ctx: &AdapterContext,
        name: &str,
        input: Value,
    ) -> Result<Value, AdapterError> {
        let entry = self
            .get(name)
            .ok_or_else(|| AdapterError::validation("name", format!("unknown tool: `{name}`")))?;

        check_class_enabled(entry.class, ctx, &entry.descriptor.name)?;

        (entry.handler)(ctx, input).await
    }
}

/// Defence-in-depth gate: even if a tool of a higher-effect class
/// somehow lands in the registry, dispatch refuses to invoke it
/// without the matching configuration.
///
/// The `flag` returned in [`AdapterError::NotEnabled`] reflects which
/// precondition is actually missing — credentials, the trading flag,
/// or both — so the LLM client gets actionable feedback.
#[inline(never)]
fn check_class_enabled(
    class: ToolClass,
    ctx: &AdapterContext,
    name: &str,
) -> Result<(), AdapterError> {
    match class {
        ToolClass::Read => Ok(()),
        ToolClass::Account => {
            if ctx.has_credentials() {
                Ok(())
            } else {
                Err(AdapterError::NotEnabled {
                    tool: name.to_string(),
                    flag: ToolClass::Account.flag().to_string(),
                })
            }
        }
        ToolClass::Trading => {
            let creds = ctx.has_credentials();
            let trading = ctx.config.allow_trading;
            if creds && trading {
                return Ok(());
            }
            let flag = match (creds, trading) {
                (false, false) => "DERIBIT_CLIENT_ID + DERIBIT_CLIENT_SECRET + --allow-trading",
                (false, true) => ToolClass::Account.flag(),
                (true, false) => "--allow-trading",
                (true, true) => unreachable!("returned Ok above"),
            };
            Err(AdapterError::NotEnabled {
                tool: name.to_string(),
                flag: flag.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, LogFormat, OrderTransport, Transport};
    use rmcp::model::Tool;
    use serde_json::json;
    use std::net::SocketAddr;
    use std::sync::Arc;

    fn cfg(with_creds: bool, allow_trading: bool) -> Config {
        Config {
            endpoint: "https://test.deribit.com".to_string(),
            client_id: with_creds.then(|| "id".to_string()),
            client_secret: with_creds.then(|| "secret".to_string()),
            allow_trading,
            max_order_usd: None,
            transport: Transport::Stdio,
            http_listen: SocketAddr::from(([127, 0, 0, 1], 8723)),
            http_bearer_token: None,
            allowed_hosts: Vec::new(),
            log_format: LogFormat::Text,
            order_transport: OrderTransport::Http,
        }
    }

    fn ctx(with_creds: bool, allow_trading: bool) -> AdapterContext {
        AdapterContext::new(Arc::new(cfg(with_creds, allow_trading))).expect("ctx")
    }

    fn empty_schema() -> Arc<serde_json::Map<String, Value>> {
        Arc::new(serde_json::Map::new())
    }

    fn make_entry(name: &'static str, class: ToolClass) -> ToolEntry {
        let descriptor = Tool::new(
            std::borrow::Cow::Borrowed(name),
            "test tool",
            empty_schema(),
        );
        let handler: ToolHandlerFn =
            Arc::new(|_ctx, _input| Box::pin(async move { Ok(json!({"ok": true})) }));
        ToolEntry {
            descriptor,
            class,
            handler,
        }
    }

    #[test]
    fn class_flags_match_documentation() {
        assert_eq!(ToolClass::Read.flag(), "(always enabled)");
        assert_eq!(
            ToolClass::Account.flag(),
            "DERIBIT_CLIENT_ID + DERIBIT_CLIENT_SECRET"
        );
        assert_eq!(ToolClass::Trading.flag(), "--allow-trading");
    }

    #[test]
    fn registry_starts_empty() {
        let r = ToolRegistry::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert!(r.list().is_empty());
    }

    #[test]
    fn registry_lists_sorted_by_name() {
        let mut r = ToolRegistry::new();
        r.insert(make_entry("get_ticker", ToolClass::Read));
        r.insert(make_entry("get_book", ToolClass::Read));
        let listed = r.list();
        let names: Vec<&str> = listed.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(names, vec!["get_book", "get_ticker"]);
    }

    #[test]
    fn build_without_creds_includes_only_read() {
        let registry = ToolRegistry::build(&ctx(false, false));
        // v0.1-10 ships 5 per-instrument tools and v0.1-11 ships 9
        // summary / meta tools — 14 Read-class tools total. Account /
        // Trading families remain empty because v0.2 / v0.4 have not
        // landed.
        assert_eq!(registry.len(), 14);
        for tool in registry.list() {
            let entry = registry.get(tool.name.as_ref()).expect("entry");
            assert_eq!(entry.class, ToolClass::Read, "{}", tool.name);
        }
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_validation() {
        let registry = ToolRegistry::new();
        let ctx = ctx(false, false);
        let err = registry
            .call(&ctx, "no_such_tool", Value::Null)
            .await
            .unwrap_err();
        match err {
            AdapterError::Validation { field, .. } => assert_eq!(field, "name"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_read_class_succeeds_without_creds() {
        let mut registry = ToolRegistry::new();
        registry.insert(make_entry("ping", ToolClass::Read));
        let ctx = ctx(false, false);
        let out = registry.call(&ctx, "ping", Value::Null).await.expect("ok");
        assert_eq!(out, json!({"ok": true}));
    }

    #[tokio::test]
    async fn dispatch_account_class_requires_credentials() {
        let mut registry = ToolRegistry::new();
        registry.insert(make_entry("get_account_summary", ToolClass::Account));
        let ctx = ctx(false, false);
        let err = registry
            .call(&ctx, "get_account_summary", Value::Null)
            .await
            .unwrap_err();
        match err {
            AdapterError::NotEnabled { tool, flag } => {
                assert_eq!(tool, "get_account_summary");
                assert_eq!(flag, ToolClass::Account.flag());
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_account_class_succeeds_with_credentials() {
        let mut registry = ToolRegistry::new();
        registry.insert(make_entry("get_account_summary", ToolClass::Account));
        let ctx = ctx(true, false);
        registry
            .call(&ctx, "get_account_summary", Value::Null)
            .await
            .expect("ok");
    }

    #[tokio::test]
    async fn dispatch_trading_class_requires_allow_trading_flag() {
        let mut registry = ToolRegistry::new();
        registry.insert(make_entry("place_order", ToolClass::Trading));
        let ctx = ctx(true, false);
        let err = registry
            .call(&ctx, "place_order", Value::Null)
            .await
            .unwrap_err();
        match err {
            AdapterError::NotEnabled { tool, flag } => {
                assert_eq!(tool, "place_order");
                assert_eq!(flag, "--allow-trading");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_trading_class_succeeds_with_creds_and_flag() {
        let mut registry = ToolRegistry::new();
        registry.insert(make_entry("place_order", ToolClass::Trading));
        let ctx = ctx(true, true);
        registry
            .call(&ctx, "place_order", Value::Null)
            .await
            .expect("ok");
    }
}
