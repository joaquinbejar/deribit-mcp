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
- `CHANGELOG.md` following Keep-a-Changelog.

### Repository

- `#![forbid(unsafe_code)]` at the crate root and the binary entry.

[Unreleased]: https://github.com/joaquinbejar/deribit-mcp/compare/HEAD...HEAD
