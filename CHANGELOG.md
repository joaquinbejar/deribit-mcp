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
- `CHANGELOG.md` following Keep-a-Changelog.

### Repository

- `#![forbid(unsafe_code)]` at the crate root and the binary entry.

[Unreleased]: https://github.com/joaquinbejar/deribit-mcp/compare/HEAD...HEAD
