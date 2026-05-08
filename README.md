<div style="text-align: center;">
<img src="https://raw.githubusercontent.com/joaquinbejar/deribit-mcp/refs/heads/main/doc/images/logo.png" alt="deribit-mcp" style="width: 80%; height: 80%;">
</div>

[![License](https://img.shields.io/badge/license-MIT-blue)](./LICENSE)
[![Crates.io](https://img.shields.io/crates/v/deribit-mcp.svg)](https://crates.io/crates/deribit-mcp)
[![Downloads](https://img.shields.io/crates/d/deribit-mcp.svg)](https://crates.io/crates/deribit-mcp)
[![Stars](https://img.shields.io/github/stars/joaquinbejar/deribit-mcp.svg)](https://github.com/joaquinbejar/deribit-mcp/stargazers)
[![Issues](https://img.shields.io/github/issues/joaquinbejar/deribit-mcp.svg)](https://github.com/joaquinbejar/deribit-mcp/issues)
[![PRs](https://img.shields.io/github/issues-pr/joaquinbejar/deribit-mcp.svg)](https://github.com/joaquinbejar/deribit-mcp/pulls)
[![Build Status](https://img.shields.io/github/workflow/status/joaquinbejar/deribit-mcp/CI)](https://github.com/joaquinbejar/deribit-mcp/actions)
[![Coverage](https://img.shields.io/codecov/c/github/joaquinbejar/deribit-mcp)](https://codecov.io/gh/joaquinbejar/deribit-mcp)
[![Dependencies](https://img.shields.io/librariesio/github/joaquinbejar/deribit-mcp)](https://libraries.io/github/joaquinbejar/deribit-mcp)
[![Documentation](https://img.shields.io/badge/docs-latest-blue.svg)](https://docs.rs/deribit-mcp)
[![Wiki](https://img.shields.io/badge/wiki-latest-blue.svg)](https://deepwiki.com/joaquinbejar/deribit-mcp)

## `deribit-mcp`

Model Context Protocol (MCP) server for the Deribit cryptocurrency
derivatives platform. Single binary crate that adapts the
`deribit-base`, `deribit-http`, and `deribit-websocket` stack
(plus `deribit-fix` in v0.6+) onto MCP's tool / resource / prompt
surface.

This is a **thin adapter** — every MCP tool is a translation step over
an upstream call. Auth, rate limiting, reconnect, and wire codecs all
live in the sibling crates.

See [`doc/ARCHITECTURE.md`](https://github.com/joaquinbejar/deribit-mcp)
for the module map and lifecycle. ADR-0001 explains the thin-adapter
decision; ADR-0007 explains the single-binary-crate decision.

### Crate layout

- [`config`] — CLI argument and `.env` resolution.
- [`context`] — `AdapterContext` shared across handlers.
- [`error`] — `AdapterError` and `From` impls for upstream errors.
- [`server`] — `rmcp` Server scaffold.
- [`observability`] — `tracing` setup and secret redaction.
- [`tools`] — `Read` / `Account` / `Trading` tool families.
- [`resources`] — static and live resource families.
- [`prompts`] — curated MCP prompts (registry + handlers).
- [`prelude`] — curated re-exports for downstream consumers.

## Contribution and Contact

We welcome contributions to this project! If you would like to contribute, please follow these steps:

1. Fork the repository.
2. Create a new branch for your feature or bug fix.
3. Make your changes and ensure that the project still builds and all tests pass.
4. Commit your changes and push your branch to your forked repository.
5. Submit a pull request to the main repository.

If you have any questions, issues, or would like to provide feedback, please feel free to contact the project maintainer:

### **Contact Information**
- **Author**: Joaquín Béjar García
- **Email**: jb@taunais.com
- **Telegram**: [@joaquin_bejar](https://t.me/joaquin_bejar)
- **Repository**: <https://github.com/joaquinbejar/deribit-mcp>
- **Documentation**: <https://docs.rs/deribit-mcp>

We appreciate your interest and look forward to your contributions!

## ✍️ License

Licensed under MIT license

## Disclaimer

This software is not officially associated with Deribit. Trading financial instruments carries risk, and this library is provided as-is without any guarantees. Always test thoroughly with a demo account before using in a live trading environment. The MCP server exposes order entry only when explicitly enabled via `--allow-trading`; the operator is responsible for any trades placed through it.
