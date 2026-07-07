# syntax=docker/dockerfile:1.7
#
# Multi-stage build for `deribit-mcp` (ADR-0011):
#
# - Stage 1 (`builder`) compiles the release binary against an
#   explicitly pinned Rust toolchain image so production builds are
#   reproducible regardless of when they run.
# - Stage 2 (`runtime`) ships only the binary on top of the
#   distroless `cc-debian12:nonroot` base, runs as `nonroot`, and
#   exposes the documented HTTP port.
#
# Credentials are never baked in. Configuration lives in env vars at
# runtime (see `--help` and `doc/DERIBIT-INTEGRATION.md`).

# ---------- builder ----------
# Pin to MSRV from `rust-toolchain.toml`. Bumping the toolchain is a
# deliberate change and lands together with the lockfile / CI bump.
FROM rust:1.96.1-slim-bookworm AS builder
WORKDIR /src

# Build deps:
#
# - `cmake` + `perl` + the C toolchain for `aws-lc-sys` (pulled by
#   `rustls` / `tokio-tungstenite` via `deribit-websocket`'s
#   `rustls-aws-lc` feature).
# - `libssl-dev` for `openssl-sys`, pulled transitively by
#   `deribit-fix` 0.3 → `reqwest` 0.13 with default `native-tls`
#   features. v0.5 builds did not need it; v0.6 added it via the
#   FIX dep graph. The runtime stage (`gcr.io/distroless/cc`)
#   already ships `libssl3` so dynamic linking still resolves at
#   runtime.
# - `ca-certificates` for crates-io fetches inside the build.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        cmake \
        libssl-dev \
        perl \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Copy in the manifests so the dependency build can be cached
# independently of `src/`. Combined with the BuildKit cache mounts
# below, a code-only change reuses the cached registry + target,
# rather than re-compiling the whole graph.
COPY Cargo.toml Cargo.lock ./
COPY rust-toolchain.toml clippy.toml rustfmt.toml ./
COPY src ./src

# `--mount=type=cache` re-uses the cargo registry, git checkouts, and
# the `target/` directory across builds. Outputs live in the cache
# mount, so we copy the binary out before the layer is closed.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked --bin deribit-mcp \
    && cp /src/target/release/deribit-mcp /usr/local/bin/deribit-mcp

# ---------- runtime ----------
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

LABEL org.opencontainers.image.title="deribit-mcp" \
      org.opencontainers.image.description="MCP server for the Deribit derivatives platform" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.source="https://github.com/joaquinbejar/deribit-mcp"

COPY --from=builder /usr/local/bin/deribit-mcp /usr/local/bin/deribit-mcp

USER nonroot:nonroot
EXPOSE 8723

ENTRYPOINT ["/usr/local/bin/deribit-mcp"]
# Default to the HTTP/SSE transport on `0.0.0.0:8723` so a
# `docker run -p 127.0.0.1:8723:8723` works out of the box. Testnet
# is the default endpoint per ADR-0009.
CMD ["--transport=http", "--listen=0.0.0.0:8723"]
