//! `rmcp::ServerHandler` scaffold for the Deribit adapter.
//!
//! Wires the handshake (`initialize`, `ping`, `tools/list`,
//! `resources/list`, `resources/templates/list`, `prompts/list`,
//! `prompts/get`) and pins the protocol revision to `2025-06-18`.
//! Tool / resource registries are populated by their per-family
//! builders in `tools::build` / `resources::build`. Prompts ride
//! on the same pattern via [`crate::prompts::PromptRegistry`]
//! (added in v0.5-01).
//!
//! Capabilities advertised match `doc/MCP-SPEC.md` §5:
//!
//! - `tools` (no `listChanged`).
//! - `resources` with `subscribe: true` (no `listChanged`).
//! - `prompts` (no `listChanged`) — added in v0.5-01.
//! - `logging`.
//!
//! `sampling` is deliberately not advertised.
//!
//! `listChanged` is left unset on the `tools`, `resources`, and
//! `prompts` capabilities — `rmcp` omits the field from the wire
//! JSON and the MCP spec treats an omitted `listChanged` as `false`.

use std::future::Future;
use std::sync::Arc;

use rmcp::model::{
    GetPromptRequestParams, GetPromptResult, Implementation, InitializeResult, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProtocolVersion, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};

use crate::context::AdapterContext;
use crate::error::AdapterError;
use crate::prompts::PromptRegistry;
use crate::resources::ResourceRegistry;
use crate::tools::ToolRegistry;

/// MCP protocol revision pinned by `deribit-mcp`. The crate is built
/// against the `2025-06-18` revision; `rmcp` 1.6 supports several
/// revisions and would otherwise default to its newest.
pub const MCP_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V_2025_06_18;

/// `rmcp::ServerHandler` for the Deribit adapter.
///
/// Holds the shared [`AdapterContext`], the registered
/// [`ToolRegistry`], the [`ResourceRegistry`], and the
/// [`PromptRegistry`] (added in v0.5-01). All four are kept
/// behind `Arc` so the handler can be cloned cheaply across
/// connections (HTTP transport) without rebuilding state.
#[derive(Debug, Clone)]
pub struct DeribitMcpServer {
    /// Shared upstream clients + configuration.
    pub ctx: Arc<AdapterContext>,
    /// Registered tools.
    pub tools: Arc<ToolRegistry>,
    /// Registered resources.
    pub resources: Arc<ResourceRegistry>,
    /// Registered prompts (added in v0.5-01).
    pub prompts: Arc<PromptRegistry>,
}

impl DeribitMcpServer {
    /// Construct a server scaffold with every registry built against
    /// the provided context. Tool families are gated by effect class
    /// (ADR-0003); the resource catalogue is populated from the
    /// `deribit://` template set; the prompt registry is populated
    /// by [`PromptRegistry::build`].
    #[must_use]
    pub fn new(ctx: Arc<AdapterContext>) -> Self {
        let tools = ToolRegistry::build(&ctx);
        let resources = ResourceRegistry::build();
        let prompts = PromptRegistry::build(&ctx);
        Self {
            ctx,
            tools: Arc::new(tools),
            resources: Arc::new(resources),
            prompts: Arc::new(prompts),
        }
    }

    /// Build the [`ServerInfo`] returned from `initialize` and used by
    /// `rmcp` to drive the handshake.
    #[must_use]
    pub fn server_info() -> ServerInfo {
        let capabilities = rmcp::model::ServerCapabilities::builder()
            .enable_logging()
            .enable_tools()
            .enable_resources()
            .enable_resources_subscribe()
            .enable_prompts()
            .build();

        InitializeResult::new(capabilities)
            .with_protocol_version(MCP_PROTOCOL_VERSION)
            .with_server_info(Implementation::new(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
            ))
    }
}

impl ServerHandler for DeribitMcpServer {
    fn get_info(&self) -> ServerInfo {
        Self::server_info()
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = self.tools.list();
        async move {
            Ok(ListToolsResult {
                tools,
                next_cursor: None,
                meta: None,
            })
        }
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        let resources = self.resources.resources();
        async move {
            Ok(ListResourcesResult {
                resources,
                next_cursor: None,
                meta: None,
            })
        }
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send + '_ {
        let resource_templates = self.resources.templates();
        async move {
            Ok(ListResourceTemplatesResult {
                resource_templates,
                next_cursor: None,
                meta: None,
            })
        }
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        let prompts = self.prompts.list();
        async move {
            Ok(ListPromptsResult {
                prompts,
                next_cursor: None,
                meta: None,
            })
        }
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResult, McpError>> + Send + '_ {
        let ctx = self.ctx.clone();
        let prompts = self.prompts.clone();
        async move {
            let args = request.arguments.unwrap_or_default();
            prompts
                .get(&ctx, &request.name, args)
                .await
                .map_err(map_adapter_error)
        }
    }
}

/// Translate an [`AdapterError`] into the rmcp `McpError` shape
/// that `rmcp` propagates over the wire.
///
/// Every [`AdapterError::Validation`] — including the
/// registry-miss path with `field == "name"` and per-handler
/// argument-validation failures — maps to
/// [`McpError::invalid_params`] so MCP clients see a uniform
/// "your input is wrong" signal and can correct the call. Other
/// adapter errors flow through [`McpError::internal_error`] with
/// the structured payload preserved so the LLM still sees the
/// `kind`-tagged JSON.
fn map_adapter_error(err: AdapterError) -> McpError {
    let payload = serde_json::to_value(&err).unwrap_or(serde_json::Value::Null);
    let message = err.to_string();
    match err {
        AdapterError::Validation { .. } => McpError::invalid_params(message, Some(payload)),
        _ => McpError::internal_error(message, Some(payload)),
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
    fn server_info_advertises_protocol_2025_06_18() {
        let info = DeribitMcpServer::server_info();
        assert_eq!(info.protocol_version.as_str(), "2025-06-18");
    }

    #[test]
    fn server_info_advertises_tools_resources_prompts_logging() {
        let info = DeribitMcpServer::server_info();
        assert!(info.capabilities.tools.is_some());
        let res = info
            .capabilities
            .resources
            .as_ref()
            .expect("resources capability");
        assert_eq!(res.subscribe, Some(true));
        assert_eq!(res.list_changed, None);
        let tools = info.capabilities.tools.as_ref().expect("tools capability");
        assert_eq!(tools.list_changed, None);
        assert!(info.capabilities.logging.is_some());
        // Prompts capability is advertised from v0.5-01 onward.
        let prompts = info
            .capabilities
            .prompts
            .as_ref()
            .expect("prompts capability");
        assert_eq!(prompts.list_changed, None);
    }

    #[test]
    fn server_info_carries_crate_metadata() {
        let info = DeribitMcpServer::server_info();
        assert_eq!(info.server_info.name, env!("CARGO_PKG_NAME"));
        assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn server_info_is_serde_round_trip_stable() {
        let info = DeribitMcpServer::server_info();
        let json = serde_json::to_value(&info).expect("serialize");
        // Spot-check the documented MCP envelope shape.
        assert_eq!(json["protocolVersion"], "2025-06-18");
        assert_eq!(json["serverInfo"]["name"], env!("CARGO_PKG_NAME"));
        assert_eq!(json["capabilities"]["resources"]["subscribe"], true);
    }

    #[test]
    fn server_holds_registries() {
        let server = DeribitMcpServer::new(ctx());
        // Without credentials only `Read` tools register; v0.1-10
        // contributes five per-instrument tools and v0.1-11 nine
        // summaries / meta tools.
        assert_eq!(server.tools.len(), 14);
        // Resource catalogue carries the static currencies entry plus
        // the four templates per the v0.1 roadmap.
        assert_eq!(server.resources.resources().len(), 1);
        assert_eq!(server.resources.templates().len(), 4);
        // v0.5-02 ships `daily_options_summary`. v0.5-03 / v0.5-04
        // append `funding_snapshot` and `position_review`.
        assert!(server.prompts.contains("daily_options_summary"));
    }
}
