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
  - **v0.1-10** (per-instrument):
    - `get_ticker { instrument_name }` — latest ticker.
    - `get_instrument { instrument_name }` — static metadata.
    - `list_instruments { currency, kind?, expired? }` — instruments
      by currency, filterable.
    - `get_order_book { instrument_name, depth? }` — order book
      snapshot.
    - `get_index_price { index_name }` — current index price.
  - **v0.1-11** (summaries & meta):
    - `get_book_summary_by_currency { currency, kind? }` —
      best-bid/ask + 24 h stats for every instrument of a currency.
    - `get_book_summary_by_instrument { instrument_name }` — same for
      one instrument.
    - `get_currencies` — supported-currencies catalogue.
    - `get_server_time` — Deribit server time (epoch ms).
    - `get_status` — platform-wide status (locked currencies).
    - `get_last_trades { instrument_name, count?, include_old? }` —
      recent trades.
    - `get_tradingview_chart_data { instrument_name, start_timestamp,
      end_timestamp, resolution }` — OHLCV bars.
    - `get_funding_rate_history { instrument_name, start_timestamp,
      end_timestamp }` — funding-rate time series.
    - `get_historical_volatility { currency }` — realised volatility
      `[ts_ms, value]` pairs.
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
  - `ResourceRegistry::read()` is the dispatch surface. Behaviour
    when shipped:
    - Static reads: every URI returned a structured `Validation`
      error pointing at v0.1-12.
    - Live reads: every URI returned a structured `Validation`
      error pointing at v0.3.
    Both branches are tightened in subsequent v0.1 issues — see the
    v0.1-12 bullet below for the static wiring.
- `DeribitMcpServer::new` now constructs both registries from the
  context (was empty stubs).
- Static resource reads (`src/resources/static_.rs`):
  - `read_currencies(ctx)` → upstream `get_currencies()`.
  - `read_instruments(ctx, currency)` → upstream
    `get_instruments(currency, None, None)` (kind unfiltered, expired
    excluded).
  - Wired into `ResourceRegistry::read`:
    `Currencies` / `Instruments` route to the upstream HTTP call;
    live URIs (`Book`, `Ticker`, `Trades`) return
    `AdapterError::Internal { reason: "live resources land in v0.3" }`
    so the LLM sees a stable error shape until v0.3 ships.
  - Live-template descriptions and module docs updated to match the
    new error shape.
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
- Tag-driven release workflow (`.github/workflows/release.yml`):
  - Triggered by `git push <vX.Y.Z>` tags only.
  - `image` job builds the container with Docker Buildx, pushes it
    to GHCR (`ghcr.io/<owner>/<repo>`) tagged `:vX.Y.Z` and
    `:latest`, attaches OCI labels (title / source / revision /
    version), and smoke-tests the published artefact via
    `docker run … --version`.
  - GHCR auth uses `secrets.GITHUB_TOKEN`; `permissions:
    packages: write` is granted on the image job only. The
    repo-wide read-only `permissions:` default stays put.
  - `cache-from` / `cache-to: type=gha,mode=max` reuses the
    Buildx cache across release runs.
  - `crates_publish` job runs after the image job is green
    (`needs: image`). It dry-runs `cargo publish` first to surface
    metadata errors before shipping, then publishes the real crate
    via `secrets.CARGO_REGISTRY_TOKEN`. A failing image build
    blocks the crate publish.
- GitHub Actions CI (`.github/workflows/ci.yml`,
  `.github/workflows/coverage.yml`):
  - `ci.yml` jobs: `fmt-check`, `lint` (`cargo clippy
    --all-targets --all-features -- -D warnings`), `test`
    (matrix `stable` + MSRV `1.85` against `cargo test
    --all-features --locked` and `--doc`), `schema` (snapshot
    suite), `integration` (mockito + transport tests),
    `build-release` (`RUSTFLAGS=-D warnings`), `doc`
    (`RUSTDOCFLAGS=-D warnings`).
  - `coverage.yml`: `cargo-tarpaulin` → Codecov upload guarded by
    `secrets.CODECOV_TOKEN`. Honours `codecov.yml` (35 % project,
    15 % patch).
  - All third-party actions pinned to a specific commit SHA with
    a friendly version comment (no `@master` / `@v*`). Every job
    (including `fmt-check`) caches via `Swatinem/rust-cache` with
    a per-job `shared-key` so the registry / git / target dirs are
    warm without cross-contaminating between matrix legs.
  - Top-level `permissions: { contents: read }` on both workflows
    — least-privilege `GITHUB_TOKEN` to reduce blast radius if a
    third-party action is ever compromised.
  - `concurrency: cancel-in-progress` per ref so a quick re-push
    cancels the previous run.
- Reference Compose recipe + env template (`docker-compose.yml`,
  `.env.example`):
  - `docker-compose.yml`: `image: ghcr.io/joaquinbejar/deribit-mcp:latest`,
    `restart: unless-stopped`, `command: --transport=http
    --listen=0.0.0.0:8723 --testnet` (operator flips to mainnet by
    swapping the last argument), `env_file: .env`,
    `DERIBIT_HTTP_BEARER_TOKEN` + `RUST_LOG` carried from the host
    shell, `127.0.0.1:8723:8723` loopback bind.
  - No in-container `HEALTHCHECK`: the runtime is distroless and
    has no shell / `curl` / `nc`; orchestrators probe `/healthz`
    from outside (k8s `httpGet`, Portainer health tab, reverse
    proxy). Inline comment in the Compose file documents this.
  - `.env.example`: documents `DERIBIT_CLIENT_ID`,
    `DERIBIT_CLIENT_SECRET`, `DERIBIT_HTTP_BEARER_TOKEN` (all
    empty placeholders) and a `RUST_LOG` default that operators can
    accept or override.
- `.gitignore` widened from `.env` + `.env.local` to `.env*` with
  `!.env.example` allow-listed, matching the documented policy in
  `.env.example` and ADR-0011: a populated `.env` is never tracked
  even when an operator chooses an unconventional filename.
- README's docker-compose recipe drops the in-container
  `wget` healthcheck — it never worked against the distroless image
  shipped by v0.1-16. The README now spells out the
  external-probe expectation, matching `docker-compose.yml`.
- Container packaging (`Dockerfile`, `.dockerignore`) per ADR-0011:
  - Multi-stage build: `rust:1.85-slim-bookworm` builder pinned to
    MSRV → distroless `gcr.io/distroless/cc-debian12:nonroot`
    runtime.
  - Builder pulls `ca-certificates`, `cmake`, `perl`, and
    `pkg-config` (the last three are required by `aws-lc-sys` from
    the rustls / `deribit-websocket` `rustls-aws-lc` feature). No
    `libssl-dev` — the dep graph is Rustls-based; `openssl-sys` is
    not in `Cargo.lock`.
  - BuildKit cache mounts on `~/.cargo/registry`, `~/.cargo/git`,
    and `target/` so a code-only edit reuses the warm registry +
    incremental target dir; the binary is copied out of the target
    cache before the layer closes.
  - Runtime stage runs as `nonroot:nonroot`, `EXPOSE 8723`, default
    CMD = `--transport=http --listen=0.0.0.0:8723` (testnet by
    default, ADR-0009).
  - OCI labels: title / description / license / source.
  - `.dockerignore` excludes `target/`, `.git/`, `.github/`,
    `.claude/`, `.idea/`, `.vscode/`, `doc/`, `rules/`, every
    `.env*` (with `!.env.example` allow-listed), test snapshots,
    and ad-hoc Markdown so the build context is the source tree
    only.
- End-to-end integration tests (`tests/integration.rs`):
  - `tools_list_without_creds_includes_only_read_class` — registry
    gating sanity check.
  - `tools_call_get_ticker_returns_upstream_payload` — drives a
    `mockito` HTTP server, exercises the full handler / upstream
    HTTP / JSON deserialise path.
  - `tools_call_unknown_returns_validation` — registry miss path.
  - `tools_call_trading_without_allow_trading_returns_not_enabled`
    — guards the ADR-0010 opt-in (Trading absent from registry
    when `--allow-trading` is unset).
  - `resources_read_currencies_returns_upstream_payload` — covers
    the static-resource → upstream HTTP path with mockito.
  - `resources_read_live_uri_returns_internal_until_v03` — `Book`
    URI yields the v0.3 placeholder error.
  - The MCP wire handshake is covered by the existing
    `tests/stdio_handshake.rs` and `tests/http_transport.rs`
    integration tests; this file focuses on the tool / resource
    registries behind a `call` / `read`.
- New dev-dependency: `mockito = "1"` for the upstream HTTP stub.
- Read-only accessors on `ToolEntry` (`descriptor()`, `class()`)
  expose what external integration tests need without giving up the
  frozen-after-build invariant on the underlying fields.
- Schema snapshot tests (`tests/schema.rs`, `tests/snapshots/`):
  - `tool_input_schemas_unchanged` snapshots every public-tool input
    schema produced by `schemars`, sorted by tool name.
  - `adapter_error_wire_shapes_unchanged` snapshots the JSON wire
    shape of every documented `AdapterError` variant (the
    serde-tagged `{"kind": "..."}` representation is part of the
    public contract).
  - `resource_catalogue_unchanged` snapshots the static-entry +
    URI-template payload of `resources/list` /
    `resources/templates/list`.
  - Snapshots live under `tests/snapshots/` and are reviewed
    deliberately on every PR; any change requires a CHANGELOG line
    flagging "additive" vs "breaking" per `doc/TESTING.md` §3.
- New dev-dependency: `insta = "1"` with the `yaml` and `redactions`
  features.
- Graceful shutdown + signal handling (`src/main.rs`):
  - Single shared `tokio_util::sync::CancellationToken` drives shutdown
    for both transports.
  - On Unix: SIGTERM **and** SIGINT (Ctrl-C) cancel the token via a
    `tokio::signal::unix::signal` task; on Windows we fall back to
    SIGINT only. The first signal cancels; subsequent signals are
    no-ops.
  - stdio path uses `serve_with_ct(stdio(), shutdown)` so a signal
    interrupts the running rmcp service alongside EOF on stdin.
  - HTTP path passes the same token to `http_transport::serve` so
    axum + rmcp both observe the cancellation.
  - `main` returns `ExitCode` directly: `0` on clean shutdown, `1`
    on a startup-config / build / bind error (with a single-line
    stderr message — `error: cause: cause` chain joined with `:`,
    no embedded newlines). `2` is reserved for "upstream auth
    failure on first authenticated call" and lands with v0.2.
- `tests/graceful_shutdown.rs`: spins the HTTP transport on a random
  local port, drives `cancel.cancel()` as the unit-test stand-in for
  SIGTERM, and asserts the server task exits within the 5 s grace
  period.
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
