# Changelog (English)

## [Unreleased] — 2026-06-22

### Dependencies — Centralized & Upgraded

- **arrow** 54 → 59 (IPC encoding compatibility)
- **wp-arrow** 0.1 → 0.2 (arrow 59 support)
- **wp-core-connectors** 0.5.5 → 0.5.6
- **toml** 0.9 → 1.0
- **wf-connector-api** 0.1 → 0.2
- **sha2** 0.10 → 0.11
- **rand** pinned to `=0.9.0` (prevents 0.10 upgrade breaking `random_range` API)

### Workspace — Dependency Centralization

All crate-level dependency versions moved to `[workspace.dependencies]`:

| Dependency | Crates |
|-----------|--------|
| `serde_json`, `chrono`, `clap`, `tokio`, `rand` | wfgen, wfl, wfusion |
| `wp-arrow`, `wp-connector-api`, `tracing` | wfgen, wfusion |

This ensures a single source of truth for version management and prevents
drift between crates.

### wfgen — Deterministic Scenario Timestamps

- Default scenario start time changed from `Utc::now()` to fixed
  `"2026-01-01T00:00:00Z"`. Fixes non-deterministic test failures
  (`test_fault_deterministic`) and ensures reproducible data generation.

### wfgen — Chunked TCP Send in Stream Mode

- Stream command splits generated events into 1000-row chunks before
  sending via `TcpArrowSink`. Prevents wfusion's TCP source (64KB
  batch cap) from choking on single giant frames.

### Tests — e2e Tests Self-Contained

- Copied schemas, rules, sinks, and connectors from `wp-reactor/examples/`
  into `crates/wfgen/examples/`. e2e tests no longer require `wp-reactor`
  to be checked out alongside `warp-fusion`. CI can now build and test
  with only the `warp-fusion` repository.
- Updated all `.wfg` scenario files to use local relative paths
  (`../schemas/`, `../rules/`).

### Docs — AI Agent Skills Guide

- Added `skills/test-pipeline-guide.md`: an AI-agent-oriented
  troubleshooting guide covering the wf-rules test pipeline
  (wfgen → wfusion → alerts). Documents common failure modes,
  diagnostic techniques, and quick verification commands.

---

## [0.1.11] — 2026-06-21

### wfgen — Use wp-core-connectors TcpArrowSink for TCP Send

- **Dependencies**: Added `wp-core-connectors`, `wp-connector-api`, `tokio`
- **Refactor**: `tcp_send.rs` rewritten from raw `TcpStream` + manual Arrow IPC
  encoding → `TcpArrowSink::connect()` + `encode_batch_payload_with_tag()` +
  `send_payload()`
  - Arrow IPC encoding via `encode_ipc_frame` (compatible with `wp_arrow::ipc::encode_ipc`)
  - Framing: RFC6587 octet-counted (`<len> <payload>`), matching wfusion `tcp_src` `framing = "len"`
  - Transport: `NetWriter` with backpressure
- **Async**: `cmd_stream`, `cmd_send`, `cmd_bench`, `cmd_gen` all converted to `async fn`
- **Dependency**: `wp-core-connectors` 0.5.2 → 0.5.5 (exposes `encode_batch_payload_with_tag` as public API)
