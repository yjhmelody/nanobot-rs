set shell := ["bash", "-euo", "pipefail", "-c"]

default: list

# List available recipes
list:
  @just --list

# Format Rust and TOML files
fmt:
  cargo fmt --all
  taplo format

# Check formatting without modifying files
fmt-check:
  cargo fmt --all -- --check
  taplo format --check

# Lint TOML and Rust code
lint:
  taplo lint
  cargo clippy --all-targets --all-features

# Type-check all targets and features
check:
  cargo check --all-targets --all-features

# Run tests for all targets and features
test:
  cargo test --all-targets --all-features

# Run local offline end-to-end verification
e2e:
  cargo test --test e2e_local -- --nocapture

# Run local end-to-end verification including optional codex MCP connect
e2e-codex:
  cargo test --test e2e_local codex_mcp_connect_smoke -- --ignored --nocapture

# Local CI parity
ci: fmt-check lint test

# Build debug binary
build:
  cargo build

# Build release binary
build-release:
  cargo build --release

# Run with arbitrary arguments, e.g. `just run agent -m "hello"`
run *args:
  cargo run -- {{args}}

# Start agent mode, pass through extra args
agent *args:
  cargo run -- agent {{args}}

# Start gateway mode, pass through extra args
gateway *args:
  cargo run -- gateway {{args}}

# Run onboarding flow, pass through extra args
onboard *args:
  cargo run -- onboard {{args}}

# Clean build artifacts
clean:
  cargo clean
