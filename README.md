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

## Quick start

The fastest path is Docker — clone the repo, drop credentials in
`.env`, run compose. The binary also installs with
`cargo install`.

### 1. Get credentials

Create a Deribit API key at <https://test.deribit.com> (testnet) or
<https://www.deribit.com> (mainnet). Account → API → Add new key.
You only need `client_id` and `client_secret`. Public Read tools
work without credentials; Account / Trading tools require them.

### 2. Configure

Copy `.env.example` to `.env` at the repository root and fill in:

```bash
DERIBIT_NETWORK=testnet                # or `mainnet`
DERIBIT_CLIENT_ID=your_client_id
DERIBIT_CLIENT_SECRET=your_client_secret
# Optional — gates the HTTP endpoint with a static bearer:
DERIBIT_HTTP_BEARER_TOKEN=
RUST_LOG=info,deribit_mcp=debug
```

`.env` is git-ignored (mode `600` recommended). Credentials never
appear on the command line — they live in the environment only
([ADR-0004](./doc/adr/0004-credentials-env-only.md)).

### 3. Deploy

#### Option A — Docker compose (recommended)

```bash
docker compose -f Docker/docker-compose.yml up -d
curl -sf http://127.0.0.1:8723/healthz && echo "ok"
```

The compose file builds the image locally on first run and binds
`127.0.0.1:8723` only — front it with a TLS-terminating reverse
proxy if you need to expose it. Defaults to testnet, HTTP transport,
no trading.

#### Option B — `docker run`

```bash
docker build -f Docker/Dockerfile -t deribit-mcp:local .

docker run -d --name deribit-mcp \
  --restart unless-stopped \
  --env-file .env \
  -p 127.0.0.1:8723:8723 \
  deribit-mcp:local \
  --transport=http --listen=0.0.0.0:8723
```

#### Option C — `cargo install`

```bash
cargo install --path . --locked
deribit-mcp --transport=stdio   # or --transport=http --listen=0.0.0.0:8723
```

The HTTP runtime image is distroless
([ADR-0011](./doc/adr/0011-container-friendly.md)) — no shell, no
package manager, runs as `nonroot`. Health probe is `GET /healthz`
(unauthenticated, returns `200 OK` while the server accepts
requests).

### 4. Enable trading (opt-in)

Trading tools (`place_order`, `edit_order`, `cancel_order`,
`cancel_all_by_*`) are absent from `tools/list` unless you launch
with `--allow-trading` AND credentials are configured. Add a
notional cap to keep blast-radius bounded:

```bash
deribit-mcp --transport=http --listen=0.0.0.0:8723 \
  --allow-trading --max-order-usd=100
```

Per [ADR-0010](./doc/adr/0010-trading-tools-opt-in.md) the cap
classifies inverse vs linear instruments and falls back to the
upstream mark price for market orders that don't carry a price.

## Use from Claude Desktop

Edit `~/Library/Application Support/Claude/claude_desktop_config.json`
(macOS) or `%APPDATA%\Claude\claude_desktop_config.json` (Windows).

### Option A — HTTP (uses the running container)

```json
{
  "mcpServers": {
    "deribit": {
      "command": "npx",
      "args": [
        "-y",
        "mcp-remote",
        "http://127.0.0.1:8723/mcp"
      ]
    }
  }
}
```

If you set `DERIBIT_HTTP_BEARER_TOKEN`, append
`"--header", "Authorization: Bearer YOUR_TOKEN"` to `args`.

### Option B — stdio (no Docker)

```json
{
  "mcpServers": {
    "deribit": {
      "command": "/Users/you/.cargo/bin/deribit-mcp",
      "args": ["--transport=stdio"],
      "env": {
        "DERIBIT_NETWORK": "testnet",
        "DERIBIT_CLIENT_ID": "…",
        "DERIBIT_CLIENT_SECRET": "…"
      }
    }
  }
}
```

Quit Claude Desktop completely (⌘Q on macOS — including the
menubar icon) and relaunch. The slash menu should now show
`deribit:get_ticker`, `deribit:get_currencies`, etc.

### Sample prompts (Spanish)

> Usando el servidor MCP **deribit**, dame la hora del servidor y
> luego el ticker actual de BTC-PERPETUAL. Resume en una línea:
> timestamp, mark price, last price, mejor bid y mejor ask.

> Invoca el prompt **funding_snapshot** del MCP deribit con
> `currency=BTC` y `lookback_hours=24`. Sigue lo que indica.

> Con **deribit**, dame `get_account_summary` de BTC en modo
> extendido. Quiero: equity, margen inicial, margen de
> mantenimiento, P&L, balance disponible.

## Capability surface (v1.0.0)

| Class | Count | Examples |
|---|---:|---|
| `Read` tools (no auth) | 14 | `get_ticker`, `get_currencies`, `get_order_book`, `get_book_summary_by_currency`, `get_funding_rate_history`, `get_historical_volatility`, `get_last_trades`, `get_tradingview_chart_data`, `get_index_price`, `get_server_time`, `get_status`, … |
| `Account` tools (creds) | 10 | `get_account_summary`, `get_positions`, `get_subaccounts`, `get_open_orders_by_currency`, `get_user_trades_by_currency`, `get_transaction_log`, `get_deposits`, `get_withdrawals`, … |
| `Trading` tools (`--allow-trading`) | 5 | `place_order`, `edit_order`, `cancel_order`, `cancel_all_by_currency`, `cancel_all_by_instrument` |
| Resources | 1 + 4 templates | `deribit://currencies`, `deribit://instruments/{currency}`, `deribit://book/{instrument}`, `deribit://ticker/{instrument}`, `deribit://trades/{instrument}` |
| Prompts | 3 | `daily_options_summary`, `funding_snapshot`, `position_review` |

The MCP wire shape is locked under SemVer from 1.0.0 — adding /
renaming / removing tool fields, resource URIs, error variants, or
prompt arguments requires a 2.0 major bump
([CHANGELOG](./CHANGELOG.md)).

## Configuration reference

| Setting | CLI | Env var | Default |
|---|---|---|---|
| Network | `--testnet` / `--mainnet` | `DERIBIT_NETWORK=testnet\|mainnet` or `DERIBIT_ENDPOINT=<url>` | testnet |
| Credentials | — | `DERIBIT_CLIENT_ID` / `DERIBIT_CLIENT_SECRET` | none |
| Trading opt-in | `--allow-trading` | `DERIBIT_ALLOW_TRADING=1` | off |
| Notional cap | `--max-order-usd=N` | `DERIBIT_MAX_ORDER_USD=N` | unlimited |
| MCP transport | `--transport=stdio\|http` | `DERIBIT_MCP_TRANSPORT` | stdio |
| HTTP listen | `--listen=ADDR:PORT` | `DERIBIT_HTTP_LISTEN` | `127.0.0.1:8723` |
| HTTP bearer | — | `DERIBIT_HTTP_BEARER_TOKEN` | none (anonymous) |
| Order transport | `--order-transport=http\|fix` | `DERIBIT_ORDER_TRANSPORT` | http |
| Log filter | — | `RUST_LOG` | `info` |
| Log format | `--log-format=text\|json` | `DERIBIT_LOG_FORMAT` | text on stdio, json on http |

CLI flags win over env vars; env vars win over `.env`; `.env` wins
over built-in defaults.

## Observability

Each MCP request emits two structured events on stderr (stdio) /
stdout (http) — entry and outcome — with target
`deribit_mcp::request` and `elapsed_ms`:

```bash
docker logs -f deribit-mcp 2>&1 | grep deribit_mcp::request
```

```
INFO deribit_mcp::request tools/call tool="get_ticker"
INFO deribit_mcp::request tools/call ok tool="get_ticker" elapsed_ms=75
WARN deribit_mcp::request tools/call error tool="missing" elapsed_ms=0 error="..."
```

Secrets (`client_secret`, `access_token`, `refresh_token`,
`http_bearer_token`) are redacted before they reach any tracing
sink.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `tools/call` returns `-32601 method not found` | Old image without the `ServerHandler::call_tool` override | Rebuild: `docker compose -f Docker/docker-compose.yml build --no-cache` |
| Every `/mcp` request returns `401` | `DERIBIT_HTTP_BEARER_TOKEN` set in env (even empty `=` lines) | Remove the line from `.env` or pass `Authorization: Bearer …` from the client |
| `Upstream { error decoding response body }` | Custom endpoint URL missing `/api/v2` suffix | Use `DERIBIT_NETWORK=testnet\|mainnet` instead of `DERIBIT_ENDPOINT`; the canonical paths are auto-applied |
| Account / Trading tools missing from `tools/list` | Credentials not loaded | Verify `docker inspect <id>` shows `DERIBIT_CLIENT_ID` and `DERIBIT_CLIENT_SECRET` |
| Trading tools missing despite credentials | `--allow-trading` not set | Add it to the `command:` block in compose |
| Claude Desktop says "connector not responding" | Cached old session in `mcp-remote` | Quit Claude Desktop with ⌘Q (also the menubar icon), relaunch |

Build problems? `cargo install` requires `cmake`, `perl`,
`pkg-config`, and `libssl-dev` (or `openssl-devel`) for the FIX
dependency chain. Docker handles this automatically.

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
