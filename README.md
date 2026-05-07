<div style="text-align: center;">
<img src="https://raw.githubusercontent.com/joaquinbejar/deribit-mcp/refs/heads/main/doc/images/logo.png" alt="deribit-mcp" style="width: 80%; height: 80%;">
</div>

[![License](https://img.shields.io/badge/license-MIT-blue)](./LICENSE)
[![Crates.io](https://img.shields.io/crates/v/deribit-mcp.svg)](https://crates.io/crates/deribit-mcp)
[![Downloads](https://img.shields.io/crates/d/deribit-mcp.svg)](https://crates.io/crates/deribit-mcp)
[![Stars](https://img.shields.io/github/stars/joaquinbejar/deribit-mcp.svg)](https://github.com/joaquinbejar/deribit-mcp/stargazers)
[![Issues](https://img.shields.io/github/issues/joaquinbejar/deribit-mcp.svg)](https://github.com/joaquinbejar/deribit-mcp/issues)
[![PRs](https://img.shields.io/github/issues-pr/joaquinbejar/deribit-mcp.svg)](https://github.com/joaquinbejar/deribit-mcp/pulls)
[![CI](https://img.shields.io/github/actions/workflow/status/joaquinbejar/deribit-mcp/ci.yml?branch=main&label=CI)](https://github.com/joaquinbejar/deribit-mcp/actions/workflows/ci.yml)
[![Coverage](https://img.shields.io/codecov/c/github/joaquinbejar/deribit-mcp)](https://codecov.io/gh/joaquinbejar/deribit-mcp)
[![Dependencies](https://img.shields.io/librariesio/github/joaquinbejar/deribit-mcp)](https://libraries.io/github/joaquinbejar/deribit-mcp)
[![Documentation](https://img.shields.io/badge/docs-latest-blue.svg)](https://docs.rs/deribit-mcp)
[![Wiki](https://img.shields.io/badge/wiki-latest-blue.svg)](https://deepwiki.com/joaquinbejar/deribit-mcp)

## Deribit MCP Server (deribit-mcp)

**Model Context Protocol** server for the Deribit cryptocurrency derivatives
platform. Single binary that exposes Deribit market data, account state, and
(opt-in) order management as MCP tools and resources, ready to plug into any
MCP client (Claude Desktop, Claude Code, custom agents).

The server is a **thin adapter** over the rest of the Deribit Rust family
([`deribit-base`](https://crates.io/crates/deribit-base),
[`deribit-http`](https://crates.io/crates/deribit-http),
[`deribit-websocket`](https://crates.io/crates/deribit-websocket),
[`deribit-fix`](https://crates.io/crates/deribit-fix)). It does not duplicate
auth, rate limiting, reconnect, or wire codecs — it forwards.

### Key features (v0.1)

- **MCP server with both transports**: `stdio` (desktop MCP clients) and
  Streamable HTTP / SSE (daemon / container deployments).
- **Public read-only tools** backed by `deribit-http`: `get_ticker`,
  `get_instrument`, `list_instruments`, `get_order_book`,
  `get_index_price`, `get_book_summary_by_currency`,
  `get_book_summary_by_instrument`, `get_currencies`, `get_server_time`,
  `get_status`, `get_last_trades`, `get_tradingview_chart_data`,
  `get_funding_rate_history`, `get_historical_volatility`.
- **Read-only resources**: `deribit://currencies` and
  `deribit://instruments/{currency}` (refresh-on-read). Live
  resources (`book`, `ticker`, `trades`) land in v0.3.
- **Container-friendly**: stateless distroless image, env-only configuration,
  `/healthz` endpoint, SIGTERM / SIGINT graceful shutdown.

### Authentication, accounts, and trading (later milestones)

- **v0.2** — Authenticated read tools (`get_account_summary`, `get_positions`,
  `get_subaccounts`, `get_transaction_log`, `get_deposits`, `get_withdrawals`).
  Credentials via env / `.env`; **never** via tool arguments (ADR-0004).
- **v0.3** — Live resources (`deribit://book/{instrument}`,
  `deribit://ticker/{instrument}`, `deribit://trades/{instrument}`)
  backed by `deribit-websocket`.
- **v0.4** — Trading tools (`place_order`, `edit_order`, `cancel_order`,
  `cancel_all_*`) gated by `--allow-trading` (ADR-0010). Optional notional
  cap via `--max-order-usd`.
- **v0.5** — MCP `prompts` capability for curated workflows.
- **v0.6** — Optional FIX path for low-latency order entry via
  `--order-transport=fix`.

### Installation

From `crates.io`:

```bash
cargo install deribit-mcp
```

Or build from source:

```bash
git clone https://github.com/joaquinbejar/deribit-mcp
cd deribit-mcp
cargo build --release
```

### Quick start — Claude Desktop (stdio)

Add to your Claude Desktop MCP config (typically
`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS):

```jsonc
{
  "mcpServers": {
    "deribit": {
      "command": "deribit-mcp",
      "args": ["--testnet"],
      "env": {
        "DERIBIT_CLIENT_ID":     "${DERIBIT_CLIENT_ID}",
        "DERIBIT_CLIENT_SECRET": "${DERIBIT_CLIENT_SECRET}"
      }
    }
  }
}
```

`--allow-trading` is **off** by default; only public read-only tools are
visible until v0.2 ships authenticated reads.

#### Verify (Claude Desktop)

After restarting Claude Desktop, open a chat and ask the agent to
`tools/list`. You should see the v0.1 public Read tools (`get_ticker`,
`get_instrument`, …). Try:

> Use the `deribit` server's `get_ticker` tool to look up
> `BTC-PERPETUAL`.

The agent should return a structured ticker payload with
`mark_price`, `best_bid_price`, `best_ask_price`, …

### Quick start — Docker / Portainer (http)

Reference `docker-compose.yml`:

```yaml
services:
  deribit-mcp:
    image: ghcr.io/joaquinbejar/deribit-mcp:latest
    restart: unless-stopped
    command:
      - --transport=http
      - --listen=0.0.0.0:8723
      - --testnet
    env_file: .env
    ports:
      - "127.0.0.1:8723:8723"
```

> The image is distroless (no shell / `curl` / `wget` / `nc`), so
> there is no in-container `HEALTHCHECK`. Probe `GET /healthz` from
> outside the container — k8s `httpGet`, Portainer's health tab, or
> the upstream reverse proxy. `/healthz` is always anonymous.

Matching `.env` (gitignored under the `.env*` rule, with
`!.env.example` allow-listed):

```env
DERIBIT_CLIENT_ID=...
DERIBIT_CLIENT_SECRET=...
DERIBIT_HTTP_BEARER_TOKEN=...
RUST_LOG=info,deribit_mcp=debug
```

Reference templates ship in this repo at `docker-compose.yml` and
`.env.example`.

#### Verify (Docker / Portainer)

Once the stack is up, the unauthenticated liveness probe answers
200 from outside the container:

```bash
curl -sf http://127.0.0.1:8723/healthz
# → ok
```

When `DERIBIT_HTTP_BEARER_TOKEN` is set, every request to `/mcp`
must carry `Authorization: Bearer <token>` (mismatches return
`401`). `/healthz` is always anonymous so container orchestration
probes never need a credential, and unknown paths surface a normal
`404` without requiring authentication either.

### Configuration

| Setting           | CLI flag                  | Env var                       | Default              |
|-------------------|---------------------------|-------------------------------|----------------------|
| Endpoint (testnet)| `--testnet`               | `DERIBIT_ENDPOINT`            | `test.deribit.com`   |
| Endpoint (mainnet)| `--mainnet`               | `DERIBIT_ENDPOINT=…`          | (off)                |
| Client ID         | `--client-id ID`          | `DERIBIT_CLIENT_ID`           | (none)               |
| Client secret     | (env / `.env` only)       | `DERIBIT_CLIENT_SECRET`       | (none)               |
| Trading enabled   | `--allow-trading`         | `DERIBIT_ALLOW_TRADING=1`     | off                  |
| Max order size    | `--max-order-usd N`       | `DERIBIT_MAX_ORDER_USD`       | unlimited            |
| MCP transport     | `--transport=stdio\|http` | `DERIBIT_MCP_TRANSPORT`       | `stdio`              |
| HTTP listen       | `--listen=0.0.0.0:8723`   | `DERIBIT_HTTP_LISTEN`         | `127.0.0.1:8723`     |
| HTTP bearer token | (env only)                | `DERIBIT_HTTP_BEARER_TOKEN`   | (none, no auth)      |
| Log format        | `--log-format=text\|json` | `DERIBIT_LOG_FORMAT`          | `text` / `json`      |
| Env file path     | `--env-file PATH`         | (n/a)                         | `./.env` if present  |

CLI flags exist for everything **except** `client_secret` and the HTTP bearer
token — secrets must flow via env or `.env`. Passing a secret on the command
line would put it in the parent process's `argv`, visible to other users on
the same host (ADR-0004).

### Project structure

```
src/
  main.rs            — binary entry point: arg parsing, transport
  lib.rs             — re-exports for integration tests
  config.rs          — CLI args + env / .env resolution (dotenvy)
  context.rs         — AdapterContext: shared upstream clients
  error.rs           — AdapterError + From impls for upstream errors
  server.rs          — rmcp ServerHandler: initialize, list, capabilities
  http_transport.rs  — axum router + Streamable HTTP service + bearer
  observability.rs   — tracing setup, secret redaction
  tools/{public,account,trading}.rs
  resources/{static_,live}.rs
tests/               — integration tests (stdio, http, schema, …)
.github/workflows/   — ci.yml, coverage.yml, release.yml
```

### Companion crates

- [`deribit-base`](https://crates.io/crates/deribit-base) — shared types,
  error catalogue, primitives.
- [`deribit-http`](https://crates.io/crates/deribit-http) — HTTP REST client.
- [`deribit-websocket`](https://crates.io/crates/deribit-websocket) —
  WebSocket JSON-RPC client.
- [`deribit-fix`](https://crates.io/crates/deribit-fix) — FIX 4.4 client.

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
