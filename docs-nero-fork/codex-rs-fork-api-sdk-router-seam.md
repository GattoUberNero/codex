# Codex-rs Fork API/SDK Router Seam

Purpose: strict technical supplement for the multi-agent delegation seam (`sdk/api -> router -> protocol -> tui`) in forked `codex-rs`.

Version stamp:

- upstream baseline: `0.118.0`
- fork doc version: `0.118.0-nero.v3`
- snapshot date: `2026-04-10`
- repo commit (HEAD at write time): `0ec73acb8`

## 1) Scope

This document is limited to:

- tool ingress/egress contracts used by `spawn_agent`, `send_message`, `assign_task`, `send_input`, `resume_agent`, `wait_agent`, `close_agent`, `list_agents`
- event + router projection into app-server notifications
- replay reconstruction into `ThreadItem::CollabAgentToolCall`
- TUI rendering semantics for live/replay parity
- test ownership map for this seam

Out of scope:

- general hook/runtime surfaces
- account/fallback surfaces
- non-collab app-server APIs

## 2) Contract Ownership

- `NERO-EXPOSED`: tool contract shape and explicit fork policy (including context inheritance ingress disable).
- `NATIVE carrier`: transport events/items (`Collab*` events, app-server notifications, `ThreadItem`).
- `NERO-INTERNAL`: policy and rendering behavior, including dedupe behavior and compatibility fallback logic.

## 3) Tool Contract (Ingress/Egress)

Primary module: `codex-rs/tools/src/agent_tool.rs`.

### `spawn_agent`

Ingress:

- v2 requires `task_name`.
- `message` or `items`.
- optional: `agent_type`, `model`, `reasoning_effort`, `delegation_report`.
- `delegation_report`, when present, has 9 required top-level fields:
  - `general_task_type`
  - `task_difficulty_1_10` (1..10)
  - `brief_completeness_1_10` (1..10)
  - `task_self_sufficiency_1_10` (1..10)
  - `expected_duration_minutes` (>0)
  - `why_this_agent`
  - `expected_output_shape`
  - `files_or_scope`
  - `risks_or_unknowns`
- optional nested extension: `orchestration_context`

Ingress policy:

- passing `fork_context` or `context_inheritance` is rejected (hard-disabled ingress).
- accepted calls without those fields return inheritance output as `off/off`.
- whether `delegation_report` is optional or required at ingress is controlled by
  `spawn_delegation_report_profile`; default `optional_only_ui` keeps it optional, richer profiles
  require it.
- current runtime validation is structural: non-empty required strings, bounded numeric scores,
  positive duration, and hard per-field validation for `orchestration_context` when present.

Egress:

- v2 response contains:
  - `task_name` (canonical)
  - `agent_id` (compat lane, may be `null`)
  - `nickname`
  - `delegation_report` (required output key; object or `null`)
  - `context_inheritance_requested`
  - `context_inheritance_effective`
  - `context_inheritance_telemetry`

Spawn child-context projection:

- the full report record is preserved on the protocol/UI path.
- child prompt enrichment is profile-dependent:
  - `optional_only_ui`: no child-context forward
  - `all_on`: filtered report projection
  - `all_on_with_orchestration`: filtered report projection + `orchestration_context`
  - `all_on_full`: filtered report projection + `orchestration_context` + `why_this_agent`
  - `orchestration_router_block`: router-generated block; requires `orchestration_context`
- `orchestration_router_block` is current-stage bridge behavior, not a stable public lane.

### Message tools

- `send_message` (v2): queue-only message to target.
- `assign_task` (v2): same shape, but wakes target immediately.
- `send_input` (legacy): agent-id path; supports `message` or `items`.
- all message tools return `submission_id`.

### Other tools

- `resume_agent`: input `id`, output `status`.
- `wait_agent`: output `message` + `pending[]` + `timed_out` + `wait_outcome`; targeted waits return after first observed completion and `pending[]` lists still-active targets observed at the end of that listen window. App-server replay preserves or derives `waitOutcome` for old wait events.
- `close_agent`: input `target` + optional `mode` (`safe_close` default, `force_cancel` explicit); output `previous_status`. `safe_close` rejects active targets and active descendants before closing the subtree.
- `list_agents`: output `agents[]` with `agent_name`, `agent_status`, `last_task_message`.

### Current schema/runtime mismatch

- v2 schema for `wait_agent` declares `targets`; current v2 runtime still accepts timeout-only call (mailbox-activity mode).

## 4) Protocol Event Contract

Primary module: `codex-rs/protocol/src/protocol.rs`.

### Spawn begin

- carries requested spawn intent (`model`, `reasoning_effort`, requested inheritance mode, optional delegation report).

### Spawn end

- carries requested + effective model/reasoning.
- custom deserialize preserves compatibility:
  - if `requested_*` absent, fallback to effective values from legacy payload.
  - if both present, `requested_*` is authoritative for requested lane.

## 5) App-server Router Mapping

Primary module: `codex-rs/app-server/src/bespoke_event_handling.rs`.

Mapping:

- `EventMsg::CollabAgentSpawnBegin` -> `item/started` with `ThreadItem::CollabAgentToolCall { status: InProgress }`.
- `EventMsg::CollabAgentSpawnEnd` -> `item/completed` with `ThreadItem::CollabAgentToolCall { status: Completed|Failed }`.
- interaction lanes normalize at `ThreadItem` level to `SendInput` tool identity.

## 6) App-server Protocol Item Shape

Primary module: `codex-rs/app-server-protocol/src/protocol/v2.rs`.

`ThreadItem::CollabAgentToolCall` carries:

- identity/lifecycle: `id`, `tool`, `status`, `senderThreadId`, `receiverThreadIds`, `prompt`
- requested lane: `requestedModel`, `requestedReasoningEffort`
- compat lane: `model`, `reasoningEffort`
- explicit effective lane: `effectiveModel`, `effectiveReasoningEffort`
- inheritance lane: `contextInheritanceRequested`, `contextInheritanceEffective`, `contextInheritanceTelemetry`
- delegation lane: `delegationReport`
- target states: `agentsStates`

Consumer guidance:

- treat `effectiveModel` / `effectiveReasoningEffort` as authoritative effective semantics.
- use `model` / `reasoningEffort` only as compatibility fallback for mixed-version data.

## 7) Replay Reconstruction Rules

Primary module: `codex-rs/app-server-protocol/src/protocol/thread_history.rs`.

- begin/end are merged by `id` into one final `ThreadItem::CollabAgentToolCall`.
- begin-only replay yields `InProgress` item.
- begin+end replay yields completed/failed merged item with requested/effective lanes and telemetry continuity.
- interaction lanes remain normalized to `SendInput`.
- replay visibility for collab events depends on thread persistence mode: deterministic collab replay in
  `thread/read` requires `persist_extended_history: true`.

## 8) TUI Rendering Contract

Primary modules: `codex-rs/tui/src/chatwidget.rs`, `codex-rs/tui/src/multi_agents.rs`.

- Spawn completed row shows effective model/effort.
- Requested model/effort line is shown when requested != effective.
- Begin row may be suppressed on replay when completed row is reconstructed.
- `delegation_report: None` renders explicit `Delegation: not provided`.
- In non-replay live flow, spawn-end may intentionally render `Delegation: not provided` after begin already displayed full delegation block (dedupe behavior).

## 9) Test Ownership Map

### Core policy + tool contract

- `codex-rs/core/src/tools/handlers/multi_agents_tests.rs`
- validates:
  - ingress rejection of `fork_context` / `context_inheritance`
  - v2 `task_name` requirement
  - delegation report roundtrip
  - delegation-report negative validation (score bounds, required non-empty strings, positive expected duration)
  - wait/resume/send/close/list behavior
  - mailbox wait semantics

### Protocol compatibility

- `codex-rs/protocol/src/protocol.rs` tests:
  - legacy spawn-end payload without `requested_*`
  - precedence when both requested and legacy effective fields exist

### Live mapping

- helper-level mapping coverage in `codex-rs/app-server/src/bespoke_event_handling.rs` tests
- integration notification coverage in `codex-rs/app-server/tests/suite/v2/turn_start.rs`
  - includes `turn_start_spawn_agent_thread_read_replay_matches_live_item_v2` as parity anchor between
    live spawn notifications and replayed `thread/read` item shape for the same turn when
    `persist_extended_history` is enabled for that thread

### Replay reconstruction

- `codex-rs/app-server-protocol/src/protocol/thread_history.rs` tests:
  - spawn begin reconstruction
  - begin+end merge
  - effective/requested continuity and telemetry

### TUI parity

- `codex-rs/tui/src/chatwidget/tests.rs`
- `codex-rs/tui/src/multi_agents.rs` snapshots
- validates requested/effective rendering, replay suppression, fallback semantics, delegation block rendering.

## 10) Verification Checklist

If you change this seam, confirm the affected layers below still match current behavior:

1. tool schema updated (`agent_tool.rs`) and docs updated (atlas + this supplement).
2. protocol event compatibility tests cover new/legacy decode.
3. app-server live notification tests validate affected item fields.
4. replay reconstruction tests validate begin/end merge and fields.
5. TUI tests/snapshots validate live+replay render output and compatibility fallback.
6. no hidden ingress path re-enables `fork_context` or `context_inheritance`.
