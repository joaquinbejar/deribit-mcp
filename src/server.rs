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
    AnnotateAble, CallToolRequestParams, CallToolResult, Content, GetPromptRequestParams,
    GetPromptResult, Implementation, InitializeResult, ListPromptsResult,
    ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ProtocolVersion, RawContent, ReadResourceRequestParams, ReadResourceResult, ResourceContents,
    ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};

use crate::context::AdapterContext;
use crate::error::AdapterError;
use crate::prompts::PromptRegistry;
use crate::resources::{ResourceContent, ResourceRegistry, parse_resource_uri};
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
            tracing::info!(target: "deribit_mcp::request", prompt = %request.name, "prompts/get");
            match prompts.get(&ctx, &request.name, args).await {
                Ok(result) => {
                    tracing::info!(
                        target: "deribit_mcp::request",
                        prompt = %request.name,
                        "prompts/get ok"
                    );
                    Ok(result)
                }
                Err(err) => {
                    tracing::warn!(
                        target: "deribit_mcp::request",
                        prompt = %request.name,
                        error = %err,
                        "prompts/get error"
                    );
                    Err(map_adapter_error(err))
                }
            }
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let ctx = self.ctx.clone();
        let tools = self.tools.clone();
        async move {
            let name = request.name.into_owned();
            let arguments = serde_json::Value::Object(request.arguments.unwrap_or_default());
            tracing::info!(target: "deribit_mcp::request", tool = %name, "tools/call");
            let started = std::time::Instant::now();
            let outcome = tools.call(&ctx, &name, arguments).await;
            let elapsed_ms = started.elapsed().as_millis() as u64;
            match outcome {
                Ok(value) => {
                    tracing::info!(
                        target: "deribit_mcp::request",
                        tool = %name,
                        elapsed_ms,
                        "tools/call ok"
                    );
                    Ok(call_tool_result_from_value(&value))
                }
                Err(err) => {
                    tracing::warn!(
                        target: "deribit_mcp::request",
                        tool = %name,
                        elapsed_ms,
                        error = %err,
                        "tools/call error"
                    );
                    Ok(call_tool_result_from_error(&err))
                }
            }
        }
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, McpError>> + Send + '_ {
        let ctx = self.ctx.clone();
        let resources = self.resources.clone();
        async move {
            tracing::info!(target: "deribit_mcp::request", uri = %request.uri, "resources/read");
            let started = std::time::Instant::now();
            let uri = match parse_resource_uri(&request.uri) {
                Ok(uri) => uri,
                Err(err) => {
                    tracing::warn!(
                        target: "deribit_mcp::request",
                        uri = %request.uri,
                        error = %err,
                        "resources/read parse error"
                    );
                    return Err(map_adapter_error(err));
                }
            };
            match resources.read(&ctx, &uri).await {
                Ok(content) => {
                    let elapsed_ms = started.elapsed().as_millis() as u64;
                    tracing::info!(
                        target: "deribit_mcp::request",
                        uri = %request.uri,
                        elapsed_ms,
                        "resources/read ok"
                    );
                    Ok(read_resource_result_from_content(&request.uri, content))
                }
                Err(err) => {
                    let elapsed_ms = started.elapsed().as_millis() as u64;
                    tracing::warn!(
                        target: "deribit_mcp::request",
                        uri = %request.uri,
                        elapsed_ms,
                        error = %err,
                        "resources/read error"
                    );
                    Err(map_adapter_error(err))
                }
            }
        }
    }
}

/// Wrap a successful tool JSON payload into the MCP `CallToolResult`
/// envelope.
///
/// The MCP spec (2025-06-18) constrains `structuredContent` to a
/// JSON **object** — clients (e.g. `mcp-remote`) reject scalars
/// and arrays at the schema-validation step. So:
///
/// - Object values → set both `structuredContent` (verbatim) and
///   `content[0]` (stringified) so old + new clients see the data.
/// - Scalar / array values → wrap into `{"value": <…>}` so the
///   structured shape stays a valid object while the original
///   payload is still recoverable from `content[0]`.
fn call_tool_result_from_value(value: &serde_json::Value) -> CallToolResult {
    let structured = if value.is_object() {
        value.clone()
    } else {
        serde_json::json!({ "value": value })
    };
    let mut result = CallToolResult::structured(structured);
    // Replace the auto-generated stringified content with the
    // original payload (un-wrapped) so clients that read
    // `content[0].text` still get the raw value, not the
    // `{"value": …}` wrapper.
    let raw = RawContent::text(serde_json::to_string(value).unwrap_or_else(|_| value.to_string()));
    result.content = vec![Content::from(raw.no_annotation())];
    result
}

/// Wrap an [`AdapterError`] into a `CallToolResult` with `is_error =
/// true`. The error's `kind`-tagged JSON is preserved in
/// `structured_content` (already an object) plus a human-readable
/// summary in `content[0]`. Tool errors are surfaced as a
/// successful JSON-RPC response (not an `McpError`) so the LLM
/// sees the full structured payload — see the MCP spec §`tools/call`
/// errors.
fn call_tool_result_from_error(err: &AdapterError) -> CallToolResult {
    let payload = serde_json::to_value(err).unwrap_or(serde_json::Value::Null);
    CallToolResult::structured_error(payload)
}

/// Convert a [`ResourceContent`] read into the MCP `ReadResourceResult`
/// envelope.
fn read_resource_result_from_content(uri: &str, content: ResourceContent) -> ReadResourceResult {
    match content {
        ResourceContent::Json(value) => {
            let text = serde_json::to_string(&value).unwrap_or_else(|_| value.to_string());
            ReadResourceResult::new(vec![ResourceContents::TextResourceContents {
                uri: uri.to_string(),
                mime_type: Some("application/json".to_string()),
                text,
                meta: None,
            }])
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
    use crate::config::{Config, LogFormat, OrderTransport, Transport};
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
            order_transport: OrderTransport::Http,
        };
        Arc::new(AdapterContext::new(Arc::new(cfg)).expect("ctx"))
    }

    #[test]
    fn call_tool_result_wraps_scalar_in_object_for_structured_content() {
        // MCP 2025-06-18 spec: `structuredContent` MUST be an object.
        // `get_server_time` returns a bare `u64`; if we emitted
        // `structuredContent: 1778230818914` mcp-remote /
        // Claude Desktop would reject the response with a schema
        // validation error.
        let scalar = serde_json::json!(1_778_230_818_914_i64);
        let result = call_tool_result_from_value(&scalar);
        let sc = result.structured_content.expect("structured_content set");
        assert!(sc.is_object(), "structured_content must be an object: {sc}");
        assert_eq!(sc["value"], scalar, "scalar preserved under `value` key");
        // The original payload is still recoverable from content[0].
        let rmcp::model::RawContent::Text(ref text) = result.content[0].raw else {
            panic!("expected text content");
        };
        assert_eq!(text.text, "1778230818914");
    }

    #[test]
    fn call_tool_result_passes_through_object_unchanged() {
        let value = serde_json::json!({"instrument_name":"BTC-PERPETUAL","mark_price":50_000.0});
        let result = call_tool_result_from_value(&value);
        let sc = result.structured_content.expect("structured_content set");
        assert_eq!(sc, value, "object payload not wrapped");
    }

    #[test]
    fn call_tool_result_wraps_array_in_object() {
        let value = serde_json::json!([1, 2, 3]);
        let result = call_tool_result_from_value(&value);
        let sc = result.structured_content.expect("structured_content set");
        assert!(sc.is_object());
        assert_eq!(sc["value"], value);
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
