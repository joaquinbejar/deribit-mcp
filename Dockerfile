# syntax=docker/dockerfile:1.7
#
# Multi-stage build for `deribit-mcp` (ADR-0011):
#
# - Stage 1 (`builder`) compiles the release binary against the
#   Rust toolchain image.
# - Stage 2 (`runtime`) ships only the binary on top of the
#   distroless `cc-debian12` base, runs as `nonroot:nonroot`, and
#   exposes the documented HTTP port.
#
# Credentials are never baked in. Configuration lives in env vars at
# runtime (see `--help` and `doc/DERIBIT-INTEGRATION.md`).

# ---------- builder ----------
FROM rust:1-slim AS builder
WORKDIR /src

# OpenSSL/CA bits the upstream `deribit-http` and `deribit-websocket`
# crates need at link time; pruned afterwards in the same layer.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy in the manifests first so a code-only change reuses the
# cached dependency build layer.
COPY Cargo.toml Cargo.lock ./
COPY rust-toolchain.toml ./
COPY clippy.toml ./
COPY rustfmt.toml ./
COPY src ./src

RUN cargo build --release --locked --bin deribit-mcp

# ---------- runtime ----------
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

LABEL org.opencontainers.image.title="deribit-mcp" \
      org.opencontainers.image.description="MCP server for the Deribit derivatives platform" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.source="https://github.com/joaquinbejar/deribit-mcp"

COPY --from=builder /src/target/release/deribit-mcp /usr/local/bin/deribit-mcp

USER nonroot:nonroot
EXPOSE 8723

ENTRYPOINT ["/usr/local/bin/deribit-mcp"]
# Default to the HTTP/SSE transport on `0.0.0.0:8723` so a
# `docker run -p 127.0.0.1:8723:8723` works out of the box. Testnet
# is the default endpoint per ADR-0009.
CMD ["--transport=http", "--listen=0.0.0.0:8723"]
