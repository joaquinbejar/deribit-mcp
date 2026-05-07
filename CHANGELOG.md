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
- Public `Read` tools — market data (`src/tools/public.rs`):
  - `get_ticker { instrument_name }` — latest ticker.
  - `get_instrument { instrument_name }` — static metadata.
  - `list_instruments { currency, kind?, expired? }` — instruments
    by currency, filterable.
  - `get_order_book { instrument_name, depth? }` — order book
    snapshot.
  - `get_index_price { index_name }` — current index price.
  - Each tool: typed `Input` with `JsonSchema`, `Read` effect class,
    `serde_json::Value` output carrying the upstream JSON verbatim.
  - Bad input → `AdapterError::Validation { field: "arguments", ... }`
    with the upstream serde error verbatim, so the LLM sees what's
    wrong.
- `schemars` rolled forward to `1` to align with the major `rmcp`
  re-exports (`rmcp::schemars`) so tool input structs interoperate
  with `Tool::with_input_schema::<T>()` without two-major linkage.
- Static resource registry, `deribit://` URI parser, and read
  dispatch (`src/resources/mod.rs`):
  - `ResourceUri` strongly-typed variants for every documented
    template: `Currencies`, `Instruments { currency }`,
    `Book { instrument }`, `Ticker { instrument }`,
    `Trades { instrument }`.
  - `parse_resource_uri()` accepts the full template set and
    returns `AdapterError::Validation` on anything else (wrong
    scheme, unknown head, missing tail, malformed currency or
    instrument segment).
  - `ResourceUri::to_uri()` round-trips back to the canonical
    string form.
  - `ResourceRegistry::build()` produces the v0.1 catalogue: one
    static entry (`deribit://currencies`) plus four templates
    (`instruments/{currency}`, `book/{instrument}`,
    `ticker/{instrument}`, `trades/{instrument}`).
  - `ResourceRegistry::read()` is the dispatch surface; v0.1-12
    fills the static reads, v0.3 fills the live reads. Until then
    every URI returns a structured `Validation` error.
- `DeribitMcpServer::new` now constructs both registries from the
  context (was empty stubs).
- stdio transport wiring (`src/main.rs`):
  - Async `tokio::main` runtime; `Config::load` →
    `observability::init` → `AdapterContext::new` →
    `DeribitMcpServer::new` → `serve(stdio())`.
  - INFO banner on startup with the resolved environment label
    (`TESTNET` / `MAINNET`), endpoint, and transport.
  - EOF on stdin propagates through `rmcp`'s service runner into a
    clean exit (`QuitReason::Closed`), exit code 0.
  - HTTP transport branch logs a tracing error and bails with
    `anyhow!("HTTP transport not yet implemented (v0.1-09)")` so the
    binary fails fast — the wiring lands in the next issue.
- `tests/stdio_handshake.rs`: in-process integration test that drives
  the server over a pair of `tokio::io::duplex` pipes, sends one
  `initialize` request, asserts the response shape (protocol version,
  serverInfo, capabilities), and verifies graceful EOF shutdown.
- HTTP / Streamable-HTTP transport (`src/http_transport.rs`):
  - axum router fronts `rmcp::transport::streamable_http_server::StreamableHttpService`
    at `/mcp` and exposes `GET /healthz` (always unauthenticated).
  - Optional static bearer-token auth read only from
    `DERIBIT_HTTP_BEARER_TOKEN` (env / `.env`, no CLI flag, per
    ADR-0004): every `/mcp` request must carry
    `Authorization: Bearer <token>`; mismatches return `401` with a
    `WWW-Authenticate: Bearer` hint. Comparison is constant-time.
  - The middleware is layered on the `/mcp` sub-router only, so
    unknown paths surface a natural 404 and `/healthz` stays
    unauthenticated by construction.
  - Loopback-only host allow-list by default; reverse proxies pre-bind
    a public hostname.
  - Graceful shutdown via a `tokio_util::sync::CancellationToken`
    propagated to both axum and rmcp.
- `src/main.rs` `--transport=http` branch: builds `AdapterContext`,
  spawns a SIGINT handler that cancels the token, and serves the HTTP
  router. INFO banner mirrors the stdio path: env, endpoint, listen
  addr, bearer status (`set` / `none`).
- New direct dependencies: `axum = "0.8"`, `tower = "0.5"`,
  `hyper = "1"` (server + http1 features), `hyper-util = "0.1"`,
  `tokio-util = "0.7"` (rt feature for `CancellationToken`).
- `tests/http_transport.rs`: spins the HTTP server on a random
  loopback port and asserts:
  - `GET /healthz` returns 200 even when a bearer token is configured.
  - `POST /mcp` without an `Authorization` header returns 401.
  - `POST /mcp` with the correct `Bearer <token>` header is not 401.
  - With no bearer configured, `/mcp` is reachable without
    credentials.
- `CHANGELOG.md` following Keep-a-Changelog.

### Repository

- `#![forbid(unsafe_code)]` at the crate root and the binary entry.

[Unreleased]: https://github.com/joaquinbejar/deribit-mcp/compare/HEAD...HEAD
