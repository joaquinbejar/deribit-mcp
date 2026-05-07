# justfile for deribit-mcp — mirrors Makefile targets so `just <target>` works.

set shell := ["bash", "-cu"]

default: check

build:
    cargo build

release:
    cargo build --release

test:
    LOGLEVEL=WARN cargo test --lib --bins --all-features

integration-tests:
    LOGLEVEL=WARN cargo test --tests --all-features

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

lint:
    cargo clippy --all-targets --all-features -- -D warnings

doc:
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features

deny:
    @command -v cargo-deny >/dev/null 2>&1 || cargo install cargo-deny --locked
    cargo deny check

# Pre-push gate: format + lint + test + doc.
# `deny` runs only when a `deny.toml` exists.
check: fmt-check lint test doc
    @if [ -f deny.toml ]; then just deny; fi

run:
    cargo run -- --transport=stdio --testnet

run-http:
    cargo run -- --transport=http --listen=127.0.0.1:8723 --testnet

clean:
    cargo clean
