//! `rmcp::ServerHandler` scaffold for the Deribit adapter.
//!
//! v0.1-05 wires the handshake (`initialize`, `ping`, empty
//! `tools/list`, empty `resources/list`, empty
//! `resources/templates/list`) and pins the protocol revision to
//! `2025-06-18`. The tool / resource registries are populated in
//! v0.1-06 / v0.1-07; the dispatcher hooks in `call_tool` and
//! `read_resource` arrive then.
//!
//! Capabilities advertised match `doc/MCP-SPEC.md` §5:
//!
//! - `tools` (no `listChanged`).
//! - `resources` with `subscribe: true` (no `listChanged`).
//! - `logging`.
//!
//! `prompts` and `sampling` are deliberately not advertised in v0.1.
//!
//! `listChanged` is left unset on the `tools` and `resources`
//! capabilities — `rmcp` omits the field from the wire JSON and the MCP
//! spec treats an omitted `listChanged` as `false`.

use std::future::Future;
use std::sync::Arc;

use rmcp::model::{
    Implementation, InitializeResult, ListResourceTemplatesResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};

use crate::context::AdapterContext;
use crate::resources::ResourceRegistry;
use crate::tools::ToolRegistry;

/// MCP protocol revision pinned by `deribit-mcp`. The crate is built
/// against the `2025-06-18` revision; `rmcp` 1.6 supports several
/// revisions and would otherwise default to its newest.
pub const MCP_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V_2025_06_18;

/// `rmcp::ServerHandler` for the Deribit adapter.
///
/// Holds the shared [`AdapterContext`], the registered [`ToolRegistry`]
/// (built by v0.1-06), and the [`ResourceRegistry`] (built by v0.1-07).
/// All three are kept behind `Arc` so the handler can be cloned cheaply
/// across connections (HTTP transport) without rebuilding state.
#[derive(Debug, Clone)]
pub struct DeribitMcpServer {
    /// Shared upstream clients + configuration.
    pub ctx: Arc<AdapterContext>,
    /// Registered tools — empty in v0.1-05, populated by v0.1-06.
    pub tools: Arc<ToolRegistry>,
    /// Registered resources — empty in v0.1-05, populated by v0.1-07.
    pub resources: Arc<ResourceRegistry>,
}

impl DeribitMcpServer {
    /// Construct a server scaffold with the v0.1 registries built
    /// against the provided context. Tool families are gated by
    /// effect class (ADR-0003); the resource catalogue is populated
    /// from the `deribit://` template set.
    #[must_use]
    pub fn new(ctx: Arc<AdapterContext>) -> Self {
        let tools = ToolRegistry::build(&ctx);
        let resources = ResourceRegistry::build();
        Self {
            ctx,
            tools: Arc::new(tools),
            resources: Arc::new(resources),
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
    fn server_info_advertises_tools_resources_logging() {
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
        // Prompts and sampling are deliberately not advertised in v0.1.
        assert!(info.capabilities.prompts.is_none());
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
        // Tools are empty until v0.1-10/-11 populate the families.
        assert!(server.tools.is_empty());
        // Resource catalogue carries the static currencies entry plus
        // the four templates per the v0.1 roadmap.
        assert_eq!(server.resources.resources().len(), 1);
        assert_eq!(server.resources.templates().len(), 4);
    }
}
