# Moju AI Context

This file is context for `.moju/ai/ai-task.md`. Follow the selected AI task, not a generic fix task.

## Project
/Users/zuowenjian/devspace/rust/wfusion/warp-fusion/moju/draft

## Active View
Structures

## Active Domain
Replay

## Selected Element
Struct `Replay.ConsumerRoute`

## Model Summary
57 structs, 5 flows, 12 modules, 5 verify cases

## Diagnostics
- none

## Related Files
- /Users/zuowenjian/devspace/rust/wfusion/warp-fusion/moju/draft/domain/replay/domain.mju

## Source Snippets
### /Users/zuowenjian/devspace/rust/wfusion/warp-fusion/moju/draft/domain/replay/domain.mju

```mju
// ---------------------------------------------------------------------------
// replay domain — WFL rule replay and verification
// Crate: wfl
// ---------------------------------------------------------------------------

// -- Commands ----------------------------------------------------------------

command ReplayRequest {
  wfl_file: String
  input_file: String
  schema_dir: String
}

command ReplayVerifyRequest {
  file: String
}

// -- Actors ------------------------------------------------------------------

actor Engineer {
  can ReplayRequest
  can ReplayVerifyRequest
}

// -- Replay types ------------------------------------------------------------

struct ResolvedPaths {
  file: PathBuf
  input: PathBuf
  expected: PathBuf
  meta: PathBuf?
}
struct ReplayResult {
  alerts: List<OutputRecord>
  event_count: Int
  match_count: Int
  error_count: Int
}
struct NullWindowLookup {
  // unit struct
}
struct ReplayEngine {
  machine: CepStateMachine
  executor: RuleExecutor
  conv_plan: String?
}
struct ReplayExecOptions {
  scan_expired_each_event: Bool
  eof_action: ReplayEofAction
}
struct ConsumerRoute {
  engine_idx: Int
  bind_alias: String
}

// -- Replay states -----------------------------------------------------------

state ReplayEofAction {
  CloseAllEos, SweepTimeoutAtLastWatermark
}
```

## Working Rules
- Use `.moju/ai/ai-task.md` as the task source.
- Keep changes focused on relevant `.mju`, `layout.json`, or necessary documentation files.
- Do not introduce duplicate definitions.
- Run the relevant `moju verify .` / `moju readiness .`, or the project's existing validation command.
