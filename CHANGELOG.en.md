# Changelog (English)

This file records user-facing changes to `wfusion` / `wfl` / `wfgen` / `wfadm`.
Internal implementation details, dependency alignment, and test counts are not covered here.

## [0.7.0]

The `.wfg` scenario DSL injection syntax is rewritten (**breaking** — existing scenarios must be rewritten), generation-time hard assertions are added, and the engine is aligned to wp-reactor 2.1.0.

### Injection syntax (breaking)

Counts are explicit now: both the entity count and the per-entity count are written in the case (total events = background quota + injected). Old spellings fail at load time with the target spelling in the message.

| Old | New |
|---|---|
| `hit<20%>` | `hit<sip: 500>` + `use(...) x 12` |
| `traffic { ... }` | `background { ... }` |
| `injection { ... }` | `inject { ... }` |
| `use(...) with(N)` | `use(...) x N` |
| `<field> seq { ... }` | removed -- the entity field goes in the case header (inferred from the rule when omitted) |
| `not(...) within(...)` | `without(preds) [within D]` -- no count, no step slot |
| `expect { ... }` | removed -- expectations are carried by the mode `hit` / `near_miss` / `miss` |

`without(...)` declares "this entity's window must contain no matching event"; `use from "file"` now works (resolved relative to the `.wfg`, supporting a top-level object / array / NDJSON).

### Added

- **Generation-time hard assertions INJ1 / INJ2**: every `hit` entity must alert and every `near_miss` / `miss` entity must not; failures name the entity (instead of a single percentage line from `verify`).
- `on each` rules can be injection targets.
- **Background entity distribution `entity <window>.<field> zipf(pool=N, exponent=S, fresh=R)`**: background traffic used to draw every field fresh per event (so no entity ever repeated and hot entities were impossible); fields can now be drawn from an entity pool with Zipf weights plus a fresh-entity ratio. The value space is partitioned away from injected entities and its budget is checked at load time (`VN31`).
- **Cross-stream injection `join <window> as <key> { ... }`**: builds the paired events a rule's `join` target window needs (the join key is written from the left entity's key value, the timestamp is taken from the left event — both derived, not hand-written), so join-family rules can now be fed assertable data from `.wfg`. v1 supports only the default inner form and single-key rules; anything else reports `VN30`.
- Cross-stream injection now also covers the **snapshot form** (`join ... snapshot on ...`, no `within`): the right event is placed 1ms earlier, covering point-lookup enrichment rules such as q3/q20; "join-then-key" (key taken from the join side, e.g. q6) is recorded as a known gap.
- Cross-stream injection now supports **join-then-key** (`match<seller:...>` where `seller` lives on the join target window, e.g. nexmark q6): the connective key is shared across both sides, the join-side key is written onto the right row with a value band disjoint from background noise, and the entity stays the driver-side field.
- Fixed: `gen`'s oracle evaluation did not load window schemas, so join-family rules could never produce expectations (INJ1 always failed and `.except.jsonl` stayed empty); it now evaluates with schemas, making join rules usable.
- **New `replay <window> { use from "file" }` pass-through channel**: feed an existing dataset as-is (no count, no entity math, no entity assertions); timestamps are rebased onto the scenario start from the file's earliest record. Empty files, mixed time-field usage and spans beyond `#[duration]` fail at load/validation time.
- Injected events are now spread **evenly** across the scenario `#[duration]` (previously each cluster got a random start, so entities could overlap).
- object / array fields from `use({...})` / `use from` are parsed as structured values by the engine (previously strings, so rules reading nested fields never matched).
- New guide `docs/useage/scenarios.md` (including the `VN` / `SC` / `SV` / `INJ` validation-code families); the `wfadm init` templates and example scenarios are migrated to the new syntax.

New validation codes (reported at load time by `lint` / `gen`):

| Code | Trigger |
|---|---|
| VN22 | an explicit entity field is not in the stream schema |
| VN23 | an explicit entity field disagrees with the rule-inferred one |
| VN24 | the number of `use` groups exceeds the rule's event steps |
| VN25 | `spread` / `without ... within` exceeds `#[duration]` |
| VN27 | the scenario's total entity-id count reaches the 2^24 limit (`miss` counts one id per event) |
| VN28 | a background rate uses `wave` / `burst` / `timeline`
| VN29 | a scenario annotation key is not in the allow-list (`#[...]` takes only `duration`, `<...>` only `seed`), or its value type is invalid | (syntax is defined but the time-varying semantics are not implemented — it used to be generated silently at the constant `base=` rate) |
| VN30 | a `join` block matches no join clause of the rule / its form is unsupported / the rule's `within` does not contain the left event time |
| VN31 | an `entity` distribution's window / field / type / parameters are invalid, or its value-space budget collides with injected entities |

### Fixed

- Scenario annotations accept only `duration` and `seed`: other keys (including the `tick` / `rows` / `emit` mentioned in early docs) used to be silently ignored, and a wrong key name or value type quietly fell back to the defaults; now reported by the new code **VN29**.
- Background rates written as `wave(...)` / `burst(...)` / `timeline { ... }` used to be accepted by `lint` but were generated at the constant `base=` rate (so `burst(peak=300/s, ...)` silently produced flat traffic); now rejected at load time by the new code **VN28**, with the target spelling in the message.
- `array/<base>` fields (e.g. `array/digit`) no longer degrade and lose their array structure and values.
- Structured fields are no longer dropped on the assertion side, where expected files disagreed with the actual output.
- Close timing: tail instances of `close` rules could previously disagree with the engine's `close:flush` timestamp (same count and entities, time only); now aligned with the engine.

### Engine (aligned with wp-reactor 2.1.0)

- **Online behavioral baseline detection**: real-time `judge` (against the last K closed windows, `|z| > 3`) and periodic `detect` (against a same-phase historical profile, for magnitude drift); new rule-side built-ins `baseline_dev` / `sumsq` / `phase_bucket`.
- External feed refresh is more consistent (configuration and usage unchanged); a malformed feed SQL variable config now fails **at startup** instead of at the first refresh; fixed an occasional join degradation when refresh races with dynamic join configuration.

## [0.6.3]

### Fixed

- **`first(field)` drifts past the field-history cap (issue #100)**: beyond 1024 events in a window the earliest sample was dropped, so aggregate keys / `alert_id` built from `first(field)` produced multiple unique keys; it is now pinned to the earliest event of the instance (sample bound 1024 -> 1025; `collect_*` / `min` / `max` / `sum` / `avg(alias.field)` include it).
- **Threshold expressions must be compile-time constants (issue #101)**: field references, non-constant `let` and function calls (`first` / `collect_*` / `now*` / `baseline`, ...) compiled but never fired at runtime with no error; now rejected at compile time.

### Language

- A rule-level constant `let` (e.g. `let THRESHOLD = 3`) can be used as a threshold; non-constant `let` is still rejected, rule-level `let` is unavailable inside a pipeline stage, and duplicate names are rejected.
- Over-complex input is now a compile-time error instead of a compiler stack-overflow abort: expression nesting groups <= 5, operator chains within one group <= 16, rule-level `let` reference chains <= 5; `let` forward references report a declaration-order error.

### Engine

- Aligned to wp-reactor v2.0.24.

## [0.6.2]

### Fixed

- **L3 series functions produce empty output when only written inside a rule-level `let` (issue #99)**: `first` / `last` / `collect_*` used solely in a `let` (referenced indirectly by `yield`) yielded empty values (`alert_id` empty in entity output); fixed — now identical to writing them directly in `yield`.

### Engine

- Aligned to wp-reactor v2.0.23: includes the async-persist flush race fix and `@first_match_time` semantics docs/coverage (public API and rule semantics unchanged).

## [0.6.1]

### Fixed

- **Kafka NDJSON numeric timestamps no longer become null (issue #95)**: NDJSON→Arrow decoding now accepts JSON numeric epoch timestamps (s / ms / us / ns normalized by digit width), numeric strings, and `%Y-%m-%d %H:%M:%S` — Kafka and file inputs now agree on time fields; boolean text (`1/0`, case, whitespace) aligned too (wp-connector-utils 0.2.1).

### Changed

- **Housekeeping**: removed leftover moju modeling annotations and their dependency from the `wfusion` CLI (public behavior unchanged); routine dependency-tree refresh (arrow 59.3 etc.).

## [0.6.0]

### Changed

- **alpha → beta channel promotion**: content matches alpha v0.5.10 — `events` conditions can reuse rule-level string-literal `let` regexes (issue #90), aligned with wp-reactor v2.0.21 and wp-connectors v0.20.0; the version line moves to 0.6.x.

## [0.5.10]

### Language (aligned with wp-reactor 2.0.21)

- **`events` conditions can reuse rule-level string-literal `let` regexes (issue #90)**: one regex referenced by name in `regex_match` across multiple fields (e.g. URI / headers / body); the `let` may sit before or after `events`. Semantics match inline; invalid/undefined references fail static check.

### Engine

- Aligned to wp-reactor v2.0.21 (the rest is internal refactor; public API and rule semantics unchanged); `wp-connectors` aligned to v0.20.0.

## [0.5.9]

### Fixed

- **TCP connection fix**: `wp-core-connectors` 0.8.3 → 0.8.4 — fixes the TCP connection regression (socket2 rolled back 0.6.x → 0.5.10). Rule language and public API unchanged.

## [0.5.8]

### Engine (aligned with wp-reactor 2.0.19)

- Aligned to wp-reactor v2.0.19: memory-eviction WARN anti-flood (issue #86) — the first eviction logs a detailed report followed by rolling/end-of-window summaries instead of per-eviction spam; reject/jitter counters are kept for metric alerts. The rest of v2.0.19 is internal engineering refactors (public API and rule semantics unchanged).

## [0.5.7]

### Engine (aligned with wp-reactor 2.0.18)

- Aligned to wp-reactor v2.0.18 (2.0.16–2.0.18 are internal refactors; public API and rule semantics unchanged).

### wfl

- **L1 structured receipts**: `wfl test --format json` (verify isomorphic) — assertion-level pass/fail receipts.
- **L2 adversarial generation**: `wfl test --gen-negatives` — appends violating rows for bind guards (`field == literal` / `!= literal`), asserting hits stay unchanged.
- **L3 intent compilation**: `wfl intent` — `.wfi` positive samples (`hits >= 1`, missed detection) / negative samples (`hits == 0`, false positive) compiled into receipts.
- **`wfl limits-est`**: recommends `max_memory` / `max_instances` = measured peak × headroom (default 2) from `metrics.ndjson`.

### wfgen

- **L4 performance gate**: `wfgen perf-diag --gate` — fails on regression.

### Examples / docs

- Added `hello_detection` integration example and the memory-limits guide; `ssh_brute_force` replay gains the `_stream` field; README positioning and license wording updated.

## [0.5.6]

### Language (WFL, aligned with wp-reactor 2.0.15)

- **match grouping keys support derived expressions (issue #80)**: `coalesce` / `concat` / `case` / literal results usable as window keys (`match<k:10m>`), mixed with field keys; empty/fallback-less results behave like a missing key (event not grouped).
  - Derived keys support `rule_shards`; window stats/query and `now*` functions rejected by the checker.
  - Example: `examples/rules/match_expr_key_demo`; `match_let_demo` shows `let`-derived keys (issue #83).

### wfl

- **`wfl verify` EOF close fix (issue #23)**: for spans shorter than the window, EOF closes remaining `and close` instances uniformly — verify hits match the oracle.

### Engine (aligned with wp-reactor 2.0.15)

- Aligned to wp-reactor v2.0.15 (expression-derived match keys, fanout expression sharding, async spill watchdog).

## [0.5.5]

### Language (WFL)

- **`let` derived fields (issue #79)**: complex logic shared by multiple output fields defined once and referenced by bare name (e.g. `dedup_key` → `alert_id`); referenceable from `entity` / `yield` / `where` / `score`. Supported on match / close / on-each / deferred paths; stats rules not yet supported (checker errors).
- **`case` value-dispatch expression (issue #79)**: `case x { "crit" | "alert" => "CRITICAL", _ => x }` replaces nested if/else for enum normalization (multi-pattern `|` / default `_` / short-circuit). Keyword finalized as `case`, leaving `match` to the rule-level CEP clause.
- Example: `examples/rules/match_let_demo` (let chain + case normalization).

### Engine (aligned with wp-reactor 2.0.12)

- Aligned to `wp-reactor` v2.0.12: `rule_parallelism` → `rule_shards` (stateless each rules shard whole batches — output-chain parallelism); `parse_parallelism` / `parse_buffer_bytes` deprecated (ignored by the engine).

### wfgen

- **`verify-nexmark --detail-diff`**: oracle field-level detail diff — each alert's yield field values compared row-by-row against engine output, upgrading verification from count-level to field-level.

## [0.5.4]

### Engine (aligned with wp-reactor 2.0.10)

- Aligned to `wp-reactor` v2.0.10, centered on multi-key join indexing and multi-rule correctness fixes:
  - **Multi-key join index**: when multiple rules join the same window on different key fields (e.g. q8 by seller / q20 by id), a later registrant previously fell back to a full-window scan O(window)×pending and froze mixed runs; each key field now gets its own index, restoring `mix` multi-rule runs.
  - **Multi-rule correctness**: fixes located via q8/q11/q6/q7 cross-checks (join index field validation / shard conflict detection / stats key injection / close_all watermark alignment granularity).
  - **Snapshot join**: first-batch index race + gate performance regression fix (q20 regression 10–12%→3.7%).
  - Shutdown tail-batch loss + q13 mid-pipeline consume race fixes.

### License

- Licensed under **Elastic License 2.0 (ELv2)**: free for internal use (including deployment / modification / embedding in your own product); a commercial license is required to offer it as a hosted service.

## [0.5.3]

### Language (WFL)

- **Top-level lists + `use` imports (issue #73)**: define once with `name = ("a", "b", ...)`, reference from multiple rules via `expr in <name>` / `expr not in <name>`; `use "lists.wfl"` imports all top-level lists from the target file (no visibility control — all visible).
  - Compiled to literal lists + **InList type checking** (element/left-value type comparison, unified for literal and named lists; mixed/incompatible types error).
  - Error surface: unknown name / missing use target / cyclic reference / duplicate name → error (same path for lint and compile).
  - `wfl lint`/`test`/`replay`/`explain` all route through `load_wfl_with_imports` (use resolution); `wfl fmt` (tree-sitter) support for list declarations pending tree-sitter-wfl sync.

### Engine (aligned with wp-reactor 2.0.9)

- Aligned to `wp-reactor` v2.0.9 (internal engine fixes).

### wfgen

- **Stats rules wired into oracle cross-check**: q15–q19 verify consistent; oracle feeds rows by bound window + enqueues intermediate events.

## [0.5.2]

### Engine (aligned with wp-reactor 2.0.8)

- Aligned to `wp-reactor` v2.0.8; `mimalloc` periodic collection (`WF_COLLECT_MS`, default 5s) significantly reduces q18 RSS.

## [0.5.1]

### wfusion

- **mimalloc memory accounting**: process memory from `mi_process_info` reported under `metrics alloc.*` for observable true memory usage.

### wfgen

- **Configurable send buffer**: `WFGEN_SEND_BUF` (default 1MB); perf-diag send 8KB→1MB (injection 620MB/s→6.9GB/s).
- **Streaming perf-diag send**: eliminates reading the whole file into memory (drops the 30M 6.4GB×2 peak).

## [0.5.0]

### wfgen — NEXMark data generation and oracle cross-check

- **Data generation aligned with Flink official**: `gen-nexmark` distribution parameters corrected item-by-item (string fields / extra padding / fixed 100µs event rate / nextExtra range / cold 90% / horizon millisecond rounding); `bid.url` matches the official `getBaseUrl` (3-segment directory, supporting q22); `bid` gains a `channel_id` field (q21 alignment).
- **`gen-nexmark --check` self-check**: value ranges / timestamps / stream counts + md5 fingerprint + stream-order self-check; `--check` / `verify-nexmark` emit a Flink NEXMark conformance statement.
- **`verify-nexmark` oracle cross-check**: new Rust NEXMark ground-truth simulator, cross-checking against the real WFL rule engine; adds deferred join / cross-stream time ordering within frames / join window state (q21 green) / intermediate output fed downstream + union-find grouping (q13 dual-rule chain); `known-diff` mechanism (q12/q17); parallel by auction (100M 5min→44s).
- **`diff` command**: layered file comparison (L1 hash equality / L2 Myers diff volume / L3 `--detail` localization).
- **Terminal progress bars**: `gen-nexmark` / `verify-nexmark` (stderr, TTY only); non-TTY falls back to a completion summary.
- **`send-arrow` injection control**: `--rate-bytes` rate limiting (default 0 = unlimited); 1MiB large-buffer TCP copy (replacing 8KiB).

### Performance diagnostics (perf-diag)

- **Sentinel tuple system**: `wfgen`/`wfusion` support `--perf-diag`; `send-arrow`/`stream` `--sentinel <n>` appends a `__wf_sentinel` completion frame (per-connection sentinel) for precise EPS/CPU measurement.

### wfadm

- `wfadm init` auto-generates `business.d/sentinel.toml` (perf-diag sentinel sink group); docker defaults add the sentinel sink template.

## [0.3.1]

### Engine (aligned with wp-reactor 1.0.2)

- Aligned to `wp-reactor` v1.0.2; the preread budget (`parse_buffer_bytes`) now accounts in content bytes:
  - **Accounting fix**: the budget no longer charges decoded Arrow allocation size (IPC decode structurally over-counts real memory ~10× and starved the pipeline slots); it now charges content bytes (≈ wire), matching the window accounting.
  - **Default 256MB → 128MB**: avoids the 12–14GB RSS plateau at 256MB while slightly improving throughput (q1 100M: 6.13M EPS / RSS 5.88GB); raise explicitly for more throughput (512MB–2GB sweet spot; 4GB over-buffers and regresses).

### wfgen

- **`shard-frames` shard files + multi-connection replay**: `shard-frames` splits one frame file into N key-sharded files (`--shards` / `--shard-keys`, the same key always lands in the same file); `send-arrow --shard-files` raw-copies one shard file per TCP connection with zero decode, keeping key closure so stateful rules stay correct — the right way to scale supply across connections (C-UCP).
- **Fix dropping the final partial frame**: rows left over at the end of a stream (less than a full frame) were previously tagged `tail`, and the engine dropped the whole frame when routing by tag — 100M lost 1.4M rows and the bench hung until timeout. Those rows now keep the original stream tag, so no data is lost.

## [0.3.0 Unreleased]

### Engine (aligned with wp-reactor 1.0.0)

- Aligned to `wp-reactor` v1.0.0, gaining sharded rule aggregation and shared resource limits:
  - **`conv` rules can aggregate across shards**: fixed-window `conv` (sort/top/dedup/where) rules are now shardable; each shard's close output is merged across shards via a watermark barrier and `apply_conv` runs on the merged batch (global top-N / sort), then a shared rate limit applies before emitting. EOS/drained flush is the correct exit for complete data; cancel drops unsealed (partial) buckets.
  - **Cross-shard shared rate limit / budget**: `max_throttle` / `max_instances` / `max_memory_bytes` are enforced collectively across shards via shared `SharedLimits` atomics (shared sliding-window throttle, exact CAS instance reservation, rule-wide FailRule latch) instead of per-shard limits; the `rule_instances` metric sums across shards.
  - Semantics fixes: `max_instances` is now exact under sharding (the old read-then-act could overshoot by ≤ shard_count-1); conv-stage throttle overflow dispatches per `on_exceed` (FailRule latches correctly); `shards=1` behavior is unchanged.

### Language (WFL)

- `on event<accu>` within-window accumulation: after the threshold is met, the window's count and evidence keep accumulating without reset, and each subsequent qualifying event re-fires with the running cumulative values and full evidence, until the window expires (aligned with `wp-reactor` v0.4.0).

### Examples

- The `ssh_brute_force` example now uses `on event<accu>`: after brute force is detected (count >= 10), every subsequent failed login re-fires with the running cumulative count and the accumulating evidence.

### wfgen

- `--no-wfl` skips the entire WFL pipeline (no rule load/compile, no injection) and generates pure baseline random events.
- `--no-oracle` still compiles WFL (keeping injection `use()` fixed values) and only skips oracle/expected output (no `.except.*` sidecars).
- `yield preset` declared in a rule-directory `_global.wfl` is auto-merged, so `yield <target> : <preset>` reuses common output fields.

### wfusion

- `[metrics] console_output = false` disables the periodic console stats log.

## [0.1.43]

- Version bump; aligned to `wp-reactor` v0.1.42 (includes its internal engine fixes).

## [0.1.42]

- Version bump (alpha); aligned to `wp-reactor` internal fixes; no new user-facing features.

## [0.1.41]

### Language (WFL)

- `on event seq { ... }` ordered sequences and `on event any { ... }` unordered co-occurrence: attack-chain detection with per-step `within` gaps, `not has ... within` negation steps, and `consec` strict adjacency (`skip = to_next` deferred to L3).

### wfusion

- Rule instances auto-expire by their window TTL when input is idle, instead of lingering.

## [0.1.40]

### wfusion

- `[logging] level` is the single source of truth for the log level and is no longer overridden by `RUST_LOG`; use `[logging].modules` for per-module overrides.

## [0.1.39]

### wfgen

- `--out` is now optional and decoupled from `--send` (four combinations; a clear usage error when neither is given).
- `--no-oracle` renamed to `--no-wfl` (skips the whole WFL pipeline); `--no-oracle` retained as a backward-compatible alias.

## [0.1.38]

### Language (WFL)

- Project `_global.wfl` rule prelude: declare shared `yield preset`, reuse via `yield <target> : <preset>`.
- String helpers: `sha1_n(text, n)`, `join(...)`, `join_by(sep, ...)`.
- DEBUG funnel logs and state-machine progress diagnostics for rules (locate which step a rule is stuck on).

### wfusion

- Stream windows accept `object` / `array` / `array/T` input fields; `merge()` for shallow object enrichment.

## [0.1.35-alpha] — 2026-07-22

### Language (WFL)

- `object { ... }` / `array [ ... ]` structured literals and `merge()`; structured input fields on stream windows.

### wfadm

- `wfadm self update` (incl. `self check`), installing the warp-fusion suite.

## [0.1.34-alpha] — 2026-07-20

### wfadm

- `self update` installs the full warp-fusion binaries; manifest channel selection fix.

## [0.1.32-alpha] — 2026-07-20

### wfadm

- `self update` picks the remote manifest URL by channel (`alpha` / `beta`), avoiding reading another branch's manifest.

## [0.1.31-alpha] — 2026-07-19

### wfadm

- `self update` aligned with `wpadm`: new `--channel` / `--updates-base-url` / `--updates-root` / `--json` / `--yes` / `--dry-run` / `--force`; update source uses the manifest canonical target triple, fixing macOS arm64 short-name 404s.

## [0.1.30-alpha] — 2026-07-19

### wfusion

- Built-in `__window_miss` diagnostic window: unknown stream schema / missing stream-tag field observable via the monitor sink.

## [0.1.29-alpha] — 2026-07-14

### wfusion

- Sink groups support `wf_meta_disable` with wildmatch patterns (`__wfu_*`, etc.) to suppress metadata fields.
- Admin API reload semantics: requires-restart changes return `restart_required` instead of a 409 failure.

## [0.1.28] — 2026-07-13

### Language (WFL)

- Helpers: `now()` / `now_s()` / `now_ms()` / `now_us()` / `now_ns()`, `is_blank()` / `null_if_blank()` / `default_if_blank()`, `md5()` / `sha1()` / `sha256()` / `hex()` / `stable_id()`.
- Source-aware WFL parse/compile diagnostics (file, line/column, source snippet on error).

### wfusion

- `.wfs` uses `window.stream_tag` as the distribution key (replacing the old `stream`); upstream carrier unified to `wp_oml_name`.

## [0.1.24] — 2026-07-09

### wfusion — Admin API publish protocol aligned with wparse (Break)

- `POST /admin/v1/reloads/model` params now `wait` / `update` / `version` / `group` / `timeout_ms` / `reason`; old `full` / `update_remote` semantics removed.
- Non-loopback `admin_api.bind` must enable TLS; `admin_api.auth.mode` accepts only `bearer_token`.

## [0.1.23] — 2026-07-08

### wfusion / wfadm

- daemon supports `update=true` remote update (git fetch + version resolution + managed-dir sync) followed by hot reload, with automatic rollback on failure.
- `wfadm conf update` delegates to the remote-update API.

## [0.1.22] — 2026-07-07

### wfusion / wfgen

- Fixed: non-loopback admin API can start with TLS disabled.
- `wfgen --version`.

## [0.1.21] — 2026-07-06

### wfusion — path base change (Break)

- Relative paths are now resolved against the **working directory**, not the config file's directory. Relative paths in `wfusion.toml` (`schemas` / `rules` / `sinks` / `sources_dir`, etc.) must drop one `..` level; `business.d/*.toml` `base` paths likewise. An explicit `--work-dir` takes priority.

## [0.1.17] — 2026-07-01

### wfusion

- Online hot reload via the admin API: `POST /admin/v1/reloads/model` (L1 rule swap / L2 add window / L3 partial rebuild / L4 restart).
- **Break**: `wfusion config` subcommand removed (moved to `wfadm config`).

### wfadm

- `conf update` (remote rule-source sync), `init --repo` (project bootstrap from a remote template).

## [0.1.16] — 2026-06-28

### wfusion

- **Break**: `wfusion run` split into `wfusion daemon` / `wfusion batch`; `mode` is set by the CLI, not the config.
- **Break**: `wfusion rule` removed (duplicated `wfl`).
- Admin API HTTP server (`GET /admin/v1/runtime/status`).

### wfadm

- `check` (deep WFL/WFS/WFG validation), `conf diff`, `engine status` / `engine reload`, `self-update`.

## [0.1.11] — 2026-06-21

### wfgen

- Send data via `wp-core-connectors` TcpArrowSink (RFC6587 framing, matching wfusion `tcp_src`).
