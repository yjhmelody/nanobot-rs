# E2E Testing Plan

## Goal

Provide a **developer-side, one-command validation** that exercises real runtime wiring (CLI -> runtime -> provider -> tool execution -> session persistence), so teams can quickly detect integration breaks before real user traffic.

## Design Principles

- **Deterministic**: no external network dependency by default.
- **Real path**: run the actual `nanobot-rs` binary, not only unit-level mocks.
- **Fast feedback**: single command for local smoke-level confidence.
- **Layered rollout**: core offline E2E always runs, external protocol E2E optional.

## Coverage Matrix

### Core Offline E2E (`just e2e`)

- CLI command path:
  - `status`
  - `onboard --overwrite`
  - `agent -m ...`
- Runtime composition:
  - config loading from `~/.nanobot/config.json`
  - workspace bootstrapping
  - agent loop iteration
- Tool execution path:
  - model returns tool calls (`write_file`, `read_file`)
  - tool dispatch and argument decoding
  - tool result re-injected into model loop
- Persistence:
  - generated files exist and contents are correct
  - session `.jsonl` contains tool call records

### Optional Protocol E2E (`just e2e-codex`)

- Adds MCP integration check:
  - nanobot auto-spawns `codex mcp-server`
  - MCP connection established and tools registered

## What Is Still Not Covered

- Real external LLM providers (OpenAI/Anthropic/etc.) SLA/latency/error behavior
- Gateway mode multi-channel delivery (telegram/wechat/http channel adapters)
- ACP server interoperability (non-MCP protocol path)
- Long-running cron/heartbeat stability over hours/days

These should be added as separate profile suites (nightly/live environment), not mixed into the default offline E2E.

## Commands

```bash
just e2e
```

Optional Codex MCP profile:

```bash
just e2e-codex
```

## Local Harness Implementation (Rust Only)

- `tests/e2e_local.rs`
  - Starts a local mock OpenAI-compatible HTTP server in Rust
  - Executes the real `nanobot-rs` binary (`onboard`, `status`, `agent`) via `std::process::Command`
  - Uses isolated temp `HOME` and workspace
  - Verifies generated artifacts and session persistence
  - Includes an ignored `codex_mcp_connect_smoke` test for optional Codex MCP validation

## Failure Triage Hints

- `E2E_FAILURE` in agent output:
  - inspect test failure output (`--nocapture`) for the full CLI stdout/stderr
- missing session file:
  - inspect workspace path in generated config
- MCP connect failure in `e2e-codex`:
  - verify local `codex` binary and `codex mcp-server` availability
