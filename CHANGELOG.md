# Changelog

All notable changes to `deribit-mcp` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Project skeleton, dependency set, and module tree per
  `doc/ARCHITECTURE.md` §2:
  - `Cargo.toml` with the v0.1 dependency set: `rmcp`, `tokio`,
    `tracing`, `tracing-subscriber`, `serde`, `serde_json`, `schemars`,
    `thiserror`, `clap`, `dotenvy`, `anyhow` (binary only),
    `deribit-base`, `deribit-http`, `deribit-websocket`.
  - `src/lib.rs` — public module re-exports.
  - `src/main.rs` — binary entry point placeholder.
  - `src/{config,context,error,observability,server}.rs` — module
    placeholders for upcoming v0.1 work.
  - `src/tools/{mod,public,account,trading}.rs` — tool family
    placeholders with the [`ToolClass`] enum (ADR-0003).
  - `src/resources/{mod,static_,live}.rs` — resource family placeholders.
  - `src/prelude.rs` — curated re-exports.
- Developer tooling: `justfile` with `check` (fmt + clippy + test +
  doc) and the same targets as `Makefile`; `clippy.toml` and
  `rustfmt.toml` mirroring sibling-crate conventions.
- `.gitignore` extended to cover IDE / editor / OS noise and `.env*`
  files.
- Configuration loader (`src/config.rs`): CLI + env + `.env` resolution
  per `doc/DERIBIT-INTEGRATION.md` §2 (ADR-0004, ADR-0009, ADR-0011):
  - [`Config`]: Deribit endpoint, credentials, trading flags, transport,
    HTTP listen address, bearer token, log format.
  - `--testnet` / `--mainnet` for endpoint selection (testnet default).
  - `--client-id`, `--allow-trading`, `--max-order-usd`, `--transport`,
    `--listen`, `--log-format`, `--env-file` CLI flags.
  - Environment variable fallbacks: `DERIBIT_ENDPOINT`,
    `DERIBIT_CLIENT_ID`, `DERIBIT_CLIENT_SECRET`,
    `DERIBIT_ALLOW_TRADING`, `DERIBIT_MAX_ORDER_USD`,
    `DERIBIT_MCP_TRANSPORT`, `DERIBIT_HTTP_LISTEN`,
    `DERIBIT_HTTP_BEARER_TOKEN`, `DERIBIT_LOG_FORMAT`.
  - Secrets (`client_secret`, `http_bearer_token`) env/`.env` only
    (no CLI flags to prevent argv leakage).
- Observability (`src/observability.rs`): `tracing` setup with secret
  redaction per `rules/global_rules.md` (Logging & Observability):
  - Dual log format: text (stderr for stdio mode) / JSON (stdout for HTTP).
  - `RUST_LOG` env filter (INFO default).
  - Field-level redaction layer for `client_secret`, `access_token`,
    `refresh_token`, `http_bearer_token`.
  - Structured logging via `FmtSpan::NEW | FmtSpan::CLOSE` for span events.
  - [`init(config)`] initializes the global subscriber from Config.
- `AdapterContext` (`src/context.rs`): the shared value every handler
  holds an `Arc<…>` of. Owns the resolved `Config`, the eagerly
  constructed `deribit_http::DeribitHttpClient`, and a lazy
  `tokio::sync::OnceCell<deribit_websocket::DeribitWebSocketClient>`
  for v0.3 live resources.
  - `AdapterContext::new(Arc<Config>) -> Result<Self, AdapterError>`.
  - `has_credentials()` gate for the `Account` / `Trading` tool families.
  - `websocket()` async accessor with single-init semantics.
- `AdapterError` (`src/error.rs`): the only error type that crosses the
  MCP boundary. Closed-set, structured variants — `_` arms forbidden.
  - Variants: `Auth`, `RateLimited`, `Upstream`, `Validation`,
    `SizeCapExceeded`, `NotEnabled`, `Internal`.
  - `AuthFailureReason` and `UpstreamErrorKind` closed-set helpers.
  - `From` impls for `deribit_http::HttpError`,
    `deribit_websocket::error::WebSocketError`, and `serde_json::Error`
    so upstream errors map at the boundary.
  - serde-tagged (`{"kind": "..."}`) representation for stable wire
    JSON; round-trip identity verified per variant.
  - Convenience constructors (`validation`, `rate_limited`,
    `not_enabled`, `internal`) marked `#[cold] #[inline(never)]`.
- New direct dependency: `url = "2"` (shared with the upstream HTTP
  / WebSocket crates so endpoint round-tripping uses the same
  parser).
- `rmcp` Server scaffold (`src/server.rs`): `DeribitMcpServer`
  implements `rmcp::ServerHandler`. Pins the MCP protocol revision
  to `2025-06-18` (exposed as `MCP_PROTOCOL_VERSION`).
  - `initialize` advertises the documented capabilities: `tools` (no
    `listChanged`), `resources` with `subscribe: true`, and
    `logging`. `prompts` and `sampling` are deliberately not
    advertised in v0.1.
  - `tools/list`, `resources/list`, and
    `resources/templates/list` return the (v0.1-05 empty) registry
    snapshots; v0.1-06 / v0.1-07 fill the registries.
  - `ping`, `shutdown`, and the rest of `ServerHandler` use the
    `rmcp` defaults.
- Stub `ToolRegistry` (`src/tools/mod.rs`) and `ResourceRegistry`
  (`src/resources/mod.rs`) — empty placeholders the server holds an
  `Arc<…>` of so v0.1-06 / v0.1-07 can land independently.
- Tool registry, effect-class gating, and dispatch
  (`src/tools/mod.rs`):
  - `ToolEntry` (descriptor + class + handler) and `ToolHandlerFn`
    (boxed async handler) are the building blocks every per-family
    `register()` adds.
  - `ToolRegistry::build(&AdapterContext)` is the canonical builder:
    `Read` always, `Account` only when credentials are present,
    `Trading` only when credentials are present *and*
    `--allow-trading` is set (ADR-0010).
  - `ToolRegistry::call` performs a defence-in-depth class
    re-check before dispatching to the handler — even if a future
    code path inserts a `Trading` entry without the flag, dispatch
    refuses with `AdapterError::NotEnabled`.
  - Unknown tool name returns `AdapterError::Validation`.
  - `ToolClass::flag()` exposes the human-readable enabling
    requirement for the `NotEnabled` payload.
- Per-family registration hooks (`src/tools/{public,account,trading}.rs`)
  are empty in v0.1-06 — v0.1-10 / v0.1-11 / v0.2 / v0.4 fill them.
- `CHANGELOG.md` following Keep-a-Changelog.

### Repository

- `#![forbid(unsafe_code)]` at the crate root and the binary entry.

[Unreleased]: https://github.com/joaquinbejar/deribit-mcp/compare/HEAD...HEAD
