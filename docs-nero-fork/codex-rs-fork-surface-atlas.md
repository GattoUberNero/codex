# Codex-rs Fork Surface Atlas

Purpose: authoritative, current map of fork-owned `codex-rs` capability surfaces and the external contracts exposed from `codex-rs`.

Version stamp:

- upstream baseline: `0.118.0`
- fork doc version: `0.118.0-nero.v3`
- snapshot date: `2026-04-10`
- repo commit (HEAD at write time): `0ec73acb8`

Companion doc:

- `docs-nero-fork/codex-rs-fork-guide.md`
  - higher-level guide for capability intent, user experience, helper services, and extension boundaries
- `docs-nero-fork/codex-rs-fork-api-sdk-router-seam.md`
  - strict API/SDK seam supplement: request/response contracts, compatibility rules, and test ownership map

## Ownership Legend

- `NATIVE`: upstream Codex/runtime surface that Nero depends on but does not own.
- `NERO-INTERNAL`: fork-owned implementation/state internal to `codex-rs`.
- `NERO-EXPOSED`: fork-owned typed/stable contract exported from `codex-rs` (wire, RPC, protocol field, env/config interface).
- `NERO-DIAGNOSTIC`: fork-owned diagnostic/user-visible payload convention carried on native surfaces, but not versioned as typed public API.
- `EXTERNAL-CONSUMER`: system outside `codex-rs` consuming exposed contracts (for this map: `codex-nero-sdk`, `nerobar-ui`, operator command surface).

## Top-Level Capability Matrix

| Surface | Primary outcome | Ownership | Primary carriers | Primary modules |
| --- | --- | --- | --- | --- |
| Hook Msg | Runtime msg composition and delivery decisions for turn lifecycle | `NERO-EXPOSED` + `NERO-INTERNAL` on `NATIVE` hooks | `NeroHookAction::NeroHookMsg`, hook action envelope | `hooks/src/response.rs`, `core/src/codex.rs`, `hooks/src/user_notification.rs` |
| Hook Auto | Stop-centric auto continuation decision and enqueue contract | `NERO-EXPOSED` + `NERO-INTERNAL` on `NATIVE` STOP | `NeroHookAction::AutoUserReply`, stop checkpoint contract meta | `hooks/src/response.rs`, `core/src/codex.rs`, `tui/src/chatwidget.rs` |
| User-visible warning/reporting lane | User-visible runtime status and warning rendering | Carrier `NATIVE`, formatter/control `NERO-INTERNAL`, output `NERO-DIAGNOSTIC` | `EventMsg::Warning`, `HookCompletedEvent`, nero warning formatter/delivery | `core/src/codex.rs`, `tui/src/chatwidget.rs` |
| Session-auto authority / F1-F5 | Runtime auto parameter changes through authority path | `NERO-EXPOSED` + `NERO-INTERNAL` on native app-server RPC and session protocol surfaces | F1-F5 hotkeys, app-server `thread/sessionAuto/*`, protocol `nero_auto_runtime` | `tui/src/chatwidget.rs`, `tui/src/app.rs`, `app-server/src/codex_message_processor.rs`, `app-server/src/thread_session_auto.rs`, `app-server-protocol/src/protocol/v2.rs` |
| Pre-turn auto booster | Read session-auto state, resolve auto contract, and inject a pre-turn hook prompt when runtime policy allows it | `NERO-INTERNAL` on shared-seam read + `NATIVE` prompt injection carrier | `read-session-auto`, auto contract resolver, `HookPrompt` injection | `core/src/codex.rs`, `core/src/config/mod.rs` |
| Dynamic account switching / auth rotation | Recovery from quota/usage-limit through controlled rotate command path | `NERO-EXPOSED` + `NERO-INTERNAL` | `CODEXN_AUTH_ROTATE_CMD*`, `CODEXN_ROTATION_REASON` | `core/src/client.rs` |
| Model fallback | Controlled model ladder switching on eligible model failures | `NERO-EXPOSED` config + `NERO-INTERNAL` runtime state | `[nero.model_fallback]`, fallback runtime state/methods | `core/src/config/mod.rs`, `core/src/codex.rs`, `core/src/state/session.rs` |
| Multi-agent / collab delegation seam | Delegation, current MultiAgentV2 text-message relay, wait/close/list projection, and transcript projection across tool/router/protocol/TUI | `NERO-EXPOSED` + `NERO-INTERNAL` on native event carriers | `spawn_agent` (v2 requires `task_name`), `send_message`/`assign_task`, legacy `send_input` and `resume_agent`, `wait_agent`, `close_agent`, `list_agents`, `DelegationReport`, `CollabAgent*` events, `ThreadItem::CollabAgentToolCall` | `tools/src/agent_tool.rs`, `core/src/tools/handlers/multi_agents_v2/*`, `core/src/tools/handlers/multi_agents_common.rs`, `protocol/src/protocol.rs`, `app-server/src/bespoke_event_handling.rs`, `app-server-protocol/src/protocol/{v2,thread_history}.rs`, `tui/src/{multi_agents.rs,chatwidget.rs}` |
| Native hook dependency surfaces | Native lifecycle checkpoints and completion summary used by Nero capabilities | `NATIVE` | `HookEventName::{Stop, AfterAgent, AfterCompaction}`, `HookCompletedEvent` | `protocol/src/protocol.rs`, runtime hook dispatch in `core/src/codex.rs` |
| Runtime/config authority boundary | External control/config ingress and narrower shared-seam helper boundary | `NERO-EXPOSED` + `NERO-INTERNAL` | app-server RPC, protocol session fields, env/config overlays, bridge helper module | `app-server/src/thread_session_auto.rs`, `core/src/config/mod.rs`, `core/src/codex.rs`, `protocol/src/protocol.rs` |

## Detailed Surfaces

### 1) Hook Msg

- Purpose: carry structured runtime messaging decisions from hook action output into agent/user delivery flow.
- Active modules:
  - `codex-rs/hooks/src/response.rs`
  - `codex-rs/hooks/src/user_notification.rs`
  - `codex-rs/core/src/codex.rs`
- Active Rust symbols:
  - `NeroHookAction::NeroHookMsg`
  - `NeroHookMsgMode`, `NeroHookMsgShow`, `NeroHookMsgContent`, `NeroHookMsgFormat`, `NeroHookMsgStatus`
  - `parse_nero_hook_actions_from_stdout(...)`
- External inputs:
  - Hook action envelope `{"actions":[...]}`
  - Action wire `type: "nero_hook_msg"`
  - Mode values: `"synced"`, `"tui-short"` (`"tui_short"` alias accepted)
  - Format values: `"block"`, `"inline"`
- External outputs:
  - Runtime-applied msg action in after-agent processing for confirmed main-session delivery lanes
  - User-visible status blocks and/or summary entries through the reporting lane
- Runtime state ownership:
  - Turn-local delivery counters and status assembly in `core/src/codex.rs`
- Native dependencies:
  - Hook lifecycle events and native summary/event transport
- External consumers:
  - `codex-nero-sdk` (producer via hook output contract)
  - TUI/user surface (consumer of rendered result)
- Branding status:
  - Capability symbols are branded (`Nero*`, `nero_*`)
- Confirmed residues:
  - none for this surface in current active path
- Parsing caveats:
  - parser ignores unknown action types instead of failing the whole envelope
  - parser can recover a trailing canonical JSON envelope if hook stdout prefixed plaintext log lines
  - malformed known actions can still fail parse; the channel is stable but not strict “exact bytes” identity
- Delivery caveats:
  - subagent sessions suppress the active `msg/auto` lane
  - `visible_note` remains the only hook action that still emits directly for subagent-visible warning delivery

### 2) Hook Auto

- Purpose: stop-centric auto continuation control based on strict hook output and delivery contract.
- Active modules:
  - `codex-rs/hooks/src/response.rs`
  - `codex-rs/core/src/codex.rs`
  - `codex-rs/tui/src/chatwidget.rs`
- Active Rust symbols:
  - `NeroHookAction::AutoUserReply`
  - stop contract meta assembly in `core/src/codex.rs` (`stop_checkpoint_expected`, `stop_checkpoint_delivered`, `contract_satisfied`)
- Exact external inputs:
  - Hook action `type: "auto_user_reply"`
  - STOP hook lifecycle checkpoint
- Exact external outputs:
  - Auto user reply enqueue decision path
  - Diagnostic hook completion metadata consumed by TUI (`protocol.stop_checkpoint_expected`, `protocol.stop_checkpoint_delivered`)
- Runtime state ownership:
  - Session runtime config + turn-local follow-up counters in `core/src/codex.rs`
- Native dependencies:
  - `HookEventName::Stop`
  - `HookCompletedEvent`
- External consumers:
  - TUI status rendering
  - downstream runtime continuation behavior
- Branding status:
  - Branded action and stop-centric meta naming are active
- Confirmed residues:
  - none in active `msg/auto` action set
- Delivery caveats:
  - `auto_user_reply` is ignored for subagent session sources
  - duplicate `auto_user_reply` actions in the same turn are ignored after the first selected action

### 3) User-visible warning/reporting lane

- Purpose: render fork runtime status/messages to the user without changing native event carriers.
- Active modules:
  - `codex-rs/core/src/codex.rs`
  - `codex-rs/tui/src/chatwidget.rs`
- Active Rust symbols:
  - `nero_hook_tui_warning_message(...)`
  - `nero_hook_tui_delivery(...)`
  - `NeroHookAction::VisibleNote`
- Exact external inputs:
  - Hook action `type: "visible_note"`
  - Hook runtime status/meta produced in after-agent path for the summary/reporting branch
- Exact external outputs:
  - direct `EventMsg::Warning(WarningEvent { message })` for `visible_note`
  - `HookCompletedEvent` entries/meta shown in TUI for the after-agent reporting branch
- Spawn-time `delegation_report` payloads can appear as a structured spawn diagnostic block in the
  same collaboration transcript lane; they summarize the handoff without adding a new carrier. The
  full report record stays available on the protocol/UI path, while child prompt enrichment can use
  a filtered projection or a router-generated bridge block depending on the active spawn profile.
  In live non-replay flows, spawn-end may still render `Delegation: not provided` when delegation
  details were already rendered on spawn-begin (dedupe behavior), so this text is not always
  equivalent to caller omission.
- Runtime state ownership:
  - Core owns composition and emission; TUI owns projection/render state
- Native dependencies:
  - `EventMsg::Warning`
  - `HookCompletedEvent`
- External consumers:
  - User/TUI
- Branding status:
  - Nero formatter/delivery functions are branded; carriers remain native by design
- Confirmed residues:
  - none required for active behavior
- Flow caveat:
  - `visible_note` does not travel through after-agent status/meta assembly before warning emission
  - it emits warning + audit directly in the hook action application path
- Stability caveat:
  - warning text and hook summary `meta` are diagnostic payload conventions assembled in core
  - they are not schema-locked RPC/protocol contracts and should not be treated as versioned public API

### 4) Session-auto authority / F1-F5

- Purpose: deterministic runtime parameter updates through authority-backed session-auto controls.
- Active modules:
  - `codex-rs/tui/src/chatwidget.rs`
  - `codex-rs/tui/src/app.rs`
  - `codex-rs/app-server/src/codex_message_processor.rs`
  - `codex-rs/app-server/src/thread_session_auto.rs`
  - `codex-rs/app-server-protocol/src/protocol/v2.rs`
  - `codex-rs/protocol/src/protocol.rs`
- Active Rust symbols:
  - `detect_nero_auto_hotkey_action(...)`
  - `AppEvent::ApplyNeroAutoHotkey`
  - `handle_nero_auto_hotkey_event(...)`
  - `NeroThreadSessionAutoContext`
  - `NeroAutoBridgeReadRequest` (shared-seam read helper used by core booster path)
- External inputs:
  - F1-F5 key actions
  - RPC methods: `thread/sessionAuto/read`, `thread/sessionAuto/inputActivity`, `thread/sessionAuto/update`
  - update request contract:
    - required: `threadId`, `expectedVersion`
    - optional: `expectedSessionSource`
    - mutable runtime knobs:
      - nullable merge-patch fields: `enabled`, `autonomyLevel`, `autonomyStepPerRound`, `maxAutoRounds`, `doneStopScope`, `autoRounds`
      - plain bool field: `resetCounter`
    - omission keeps the current override, `null` clears the override
  - input-activity request contract:
    - `threadId`
    - `activity`, currently `DraftChanged`
    - active gate: loaded thread plus confirmed main session
  - read/update response contract carries `authority` and `state`
  - Protocol field: `nero_auto_runtime`
  - shared-seam bridge read module for core booster path: `nero_hook_runtime.session_auto_bridge`
- External outputs:
  - `ThreadSessionAutoReadResponse { thread_id, authority, state }`
  - `ThreadSessionAutoInputActivityResponse { thread_id, applied, authority, generation_epoch }`
  - `ThreadSessionAutoUpdateResponse { thread_id, authority, applied, conflict, message, error_code, reason_code, state }`
  - live read/update path currently returns `ThreadSessionAutoAuthorityMode::AppServerAuthority`
  - persisted session-auto state for later projection into runtime context on read/update paths
  - User-visible authority feedback in TUI
- Runtime state ownership:
  - Source of authority: app-server + protocol session state
  - Runtime application: persisted session-auto state later projected by TUI/core into runtime context
  - read path may also persist refreshed `main_session_confirmed` into the snapshot before returning
- Native dependencies:
  - Native app-server RPC transport
  - Native protocol event stream
- External consumers:
  - `nerobar-ui` backend
  - TUI runtime controls
- Branding status:
  - Authority/context/bridge symbols in this lane are branded
- Confirmed residues:
  - One retained compatibility alias inside bridge module resolution: retired module alias `nero_hook_runtime.state_runtime_control` (normalization only; active default remains `nero_hook_runtime.session_auto_bridge`)
  - Validation guard: app-server rejects empty `expectedVersion` on update requests
- Ownership caveats:
  - current `thread/sessionAuto/read|inputActivity|update` are handled locally inside app-server
  - the bridge/apply flow is not the primary active implementation for these RPC methods
  - shared-seam bridge read remains a narrower helper used by core auto-booster preparation

### 5) Dynamic account switching / auth rotation

- Purpose: recover from quota/usage-limit failures via explicit rotation command boundary.
- Active modules:
  - `codex-rs/core/src/client.rs`
- Active Rust symbols:
  - `try_recover_with_nero_auth_rotate_command(...)`
  - recovery flow around usage/quota handling in client request path
- Exact external inputs:
  - `CODEXN_AUTH_ROTATE_CMD`
  - `CODEXN_AUTH_ROTATE_CMD_TIMEOUT_MS`
  - recognized recovery reasons (`usage_limit_reached`, `quota_exceeded`)
- Exact external outputs:
  - child process invocation with `CODEXN_ROTATION_REASON`
  - auth reload + retry attempt if recovery succeeds
- Runtime state ownership:
  - model client session tracks recovery budget/attempt lifecycle
- Native dependencies:
  - upstream auth manager reload and API error classification
- External consumers:
  - operator-provided rotation command endpoint
- Branding status:
  - rotation entrypoint symbols are branded for fork-owned behavior
- Confirmed residues:
  - none in primary recovery path
- Policy caveats:
  - command fallback is one-shot per active recovery budget
  - a new request budget resets the command-attempt guard

### 6) Model fallback

- Purpose: move to ladder-defined backup models on eligible model failure conditions.
- Active modules:
  - `codex-rs/core/src/config/mod.rs`
  - `codex-rs/core/src/codex.rs`
  - `codex-rs/core/src/state/session.rs`
- Active Rust symbols:
  - `NeroModelFallbackConfig`, `NeroModelFallbackStep`, `NeroModelFallbackResolution`
  - `read_nero_model_fallback(...)`
  - `resolve_nero_model_fallback_from_env(...)`
  - `apply_model_fallback_pre_turn(...)`
  - `try_model_fallback_after_error(...)`
  - `mark_model_fallback_success(...)`
- Exact external inputs:
  - config namespace `[nero.model_fallback]`
  - keys: `enabled`, `cooldown_seconds`, `max_wait_seconds`, `sticky`, `ladder`
  - config overlay env entrypoints (Nero config overlay lane)
- Exact external outputs:
  - runtime model switch decisions prior to and after turn errors
  - fallback telemetry/audit entries in runtime path
- Runtime state ownership:
  - `NeroModelFallbackRuntimeState` in session state
- Native dependencies:
  - model turn execution and error handling loop
- External consumers:
  - runtime turn processing (internal consumer of config contract)
- Branding status:
  - capability and config/runtime symbols are branded
- Confirmed residues:
  - none required for active fallback behavior
- Policy caveats:
  - fallback is disabled for `SessionSource::SubAgent(_)`
  - this is a deliberate policy boundary, not an accidental omission

### 7) Native hook dependency surfaces

- Purpose: define the native lifecycle/checkpoint surfaces that Nero capabilities depend on.
- Active modules:
  - `codex-rs/protocol/src/protocol.rs`
  - `codex-rs/core/src/codex.rs`
- Active Rust symbols:
  - `HookEventName::{Stop, AfterAgent, AfterCompaction}`
  - `HookCompletedEvent`
- Exact external inputs:
  - Native lifecycle event dispatch from runtime hook engine
- Exact external outputs:
  - Standardized hook run summaries/events consumed by TUI and app-server pathways
- Runtime state ownership:
  - native runtime hook execution state
- Native dependencies:
  - this surface itself is native (dependency layer, not fork capability)
- External consumers:
  - Nero fork features (`hook msg`, `hook auto`, reporting lane)
- Branding status:
  - native names intentionally unchanged
- Confirmed residues:
  - none

### 8) Runtime/config authority boundary

- Purpose: govern where fork runtime behavior can be configured/updated from outside `codex-rs`.
- Active modules:
  - `codex-rs/core/src/config/mod.rs`
  - `codex-rs/core/src/codex.rs`
  - `codex-rs/core/src/nero_auto_runtime_state.rs`
  - `codex-rs/app-server/src/codex_message_processor.rs`
  - `codex-rs/app-server/src/thread_session_auto.rs`
  - `codex-rs/app-server-protocol/src/protocol/common.rs`
  - `codex-rs/app-server-protocol/src/protocol/v2.rs`
  - `codex-rs/protocol/src/protocol.rs`
- Active Rust symbols:
  - Nero config resolver/read functions for fallback and auto runtime
  - app-server RPC entrypoints for `thread/sessionAuto/read`, `thread/sessionAuto/inputActivity`, and `thread/sessionAuto/update`
  - `read_thread_session_auto`, `update_thread_session_auto`, and `thread_session_auto_input_activity`
- External inputs:
  - RPC: `thread/sessionAuto/read`, `thread/sessionAuto/inputActivity`, `thread/sessionAuto/update`
  - protocol session field: `nero_auto_runtime`
  - env/config ingress:
    - `CODEXN_ROOT` (bridge bootstrap/default cwd resolution)
    - `CODEXN_CONFIG_NERO_PATH`
    - `CODEXN_CONFIG_NERO_MSG_PATH`
    - `CODEXN_CONFIG_NERO_AUTO_PATH`
    - `CODEXN_CONFIG_NERO_DEV_PATH`
    - `CODEXN_CONFIG_NERO_MERGE_PATHS`
    - runtime default overrides:
      - `NERO_HOOK_AUTO_ENABLED`
      - `NERO_HOOK_AUTO_AUTONOMY_LEVEL`
      - `NERO_HOOK_AUTO_MAX_ROUNDS`
      - `NERO_HOOK_AUTO_STEP_PER_ROUND`
      - effective semantics:
        - `autonomyLevel` clamps to `1..=10`
        - `maxAutoRounds` floors at `0`
        - `autonomyStepPerRound` clamps to `0..=10` and is rounded to 3 decimal places
        - `effective.enabled=false` when the resolved session is subagent or `main_session_confirmed=false`, regardless of `NERO_HOOK_AUTO_ENABLED`
        - `thread/sessionAuto/read` syncs the `main_session_confirmed` marker when needed before returning state
        - `thread/sessionAuto/update` and `thread/sessionAuto/inputActivity` reject subagent sessions and unconfirmed main sessions
        - `enabled=true` can still be rejected on update when runtime-msg delivery is required but the thread has no name
        - coverage caveat: the missing-thread-name rejection is enforced in the handler today, but the focused `thread_session_auto.rs` tests do not directly cover that branch yet
    - bridge/runtime envs in thread-session-auto lane:
      - `NERO_RUNTIME_STATE_CONTROL_CWD`
      - `NERO_RUNTIME_STATE_CONTROL_MODULE`
      - `NERO_RUNTIME_CONTROL_TIMEOUT_MS`
      - `NERO_RUNTIME_PYTHON_BIN`
      - compat aliases `NEROBAR_NERO_RUNTIME_STATE_CONTROL_CWD`, `NEROBAR_NERO_RUNTIME_STATE_CONTROL_MODULE`, `NEROBAR_NERO_RUNTIME_CONTROL_TIMEOUT_MS`, `NEROBAR_NERO_RUNTIME_PYTHON_BIN`
- External outputs:
  - effective runtime settings applied to session config
  - app-server authority responses for active `thread/sessionAuto/*`
  - normalized bridge command requests for the narrower shared seam (`read-session-auto`)
- Runtime state ownership:
  - core session configuration + app-server authority mediation
- Native dependencies:
  - session lifecycle and protocol update stream
- External consumers:
  - app-server and TUI authority clients
  - optional shared-seam bridge module endpoint for booster/runtime read helpers
  - `nerobar-ui` authority clients
- Branding status:
  - majority of capability-level surfaces are branded
- Confirmed residues:
  - compatibility alias listed once in section 4

### 9) Multi-agent / collab delegation seam

- Purpose: expose a delegation seam that is readable for operators and deterministic across the tool surface, app-server projection, protocol snapshots, and TUI rendering.
- Active modules:
  - `codex-rs/tools/src/agent_tool.rs`
  - `codex-rs/core/src/tools/handlers/multi_agents_v2/*`
  - `codex-rs/core/src/tools/handlers/multi_agents_common.rs`
  - `codex-rs/protocol/src/protocol.rs`
  - `codex-rs/app-server/src/bespoke_event_handling.rs`
  - `codex-rs/app-server-protocol/src/protocol/v2.rs`
  - `codex-rs/app-server-protocol/src/protocol/thread_history.rs`
  - `codex-rs/tui/src/multi_agents.rs`
  - `codex-rs/tui/src/chatwidget.rs`
- Active Rust symbols:
  - tool/API family:
    - MultiAgentV2: `spawn_agent` (requires `task_name`), `send_message`, `assign_task`, `wait_agent`, `close_agent`, `list_agents`
    - legacy/non-v2: `send_input`, `resume_agent`
  - payloads: `DelegationReport`, `CollabAgentSpawnBeginEvent`, `CollabAgentSpawnEndEvent`
  - replay/live item: `ThreadItem::CollabAgentToolCall`
  - policy gate: `resolve_spawn_context_inheritance_mode(...)`
- Exact external inputs:
  - `spawn_agent` supports:
    - required `task_name`
    - `message` or `items`
    - optional `agent_type`, `model`, `reasoning_effort`
    - optional `delegation_report` with 9 required top-level fields when present:
      - `general_task_type`
      - `task_difficulty_1_10`
      - `brief_completeness_1_10`
      - `task_self_sufficiency_1_10`
      - `expected_duration_minutes`
      - `why_this_agent`
      - `expected_output_shape`
      - `files_or_scope`
      - `risks_or_unknowns`
    - optional nested `delegation_report.orchestration_context`
  - `send_message` and `assign_task` are the current MultiAgentV2 text-only message tools:
    - `target`
    - `items`
    - optional `interrupt`
    - `send_message` queues without starting a turn
    - `assign_task` wakes the target immediately
  - legacy `send_input` remains the v1 agent-id path:
    - `target`
    - `message` or `items`
    - optional `interrupt`
  - context inheritance input is hard-disabled:
    - requests that include legacy `fork_context` or `context_inheritance` are rejected with a caller-facing error
    - requests that omit these fields are accepted; tool output reports `off/off`
    - event/item carriers currently emit `context_inheritance_requested: null` and `context_inheritance_effective: off`
  - spawn delegation report handling is profile-driven:
    - `optional_only_ui`: report stays optional and render-only
    - `all_on`: report required at ingress, filtered child-context forward enabled
    - `all_on_with_orchestration`: same as `all_on`, plus `orchestration_context` forward
    - `all_on_full`: same as `all_on_with_orchestration`, plus `why_this_agent` forward
    - `orchestration_router_block`: report required, `orchestration_context` required, child prompt gets router-generated block instead of direct JSON forward
  - current validation is structural, not a semantic spawn-readiness gate:
    - base report fields must satisfy non-empty / bounded-number / positive-duration rules
    - `orchestration_context`, when present, must satisfy its per-field string/pattern rules
- Exact external outputs:
  - `spawn_agent` output echoes:
    - `task_name` as the current canonical return field
    - `agent_id` only as legacy compatibility in the v2 schema
    - `nickname`
    - `delegation_report` (required output key; value is full report object or `null`)
    - `context_inheritance_requested`
    - `context_inheritance_effective`
    - `context_inheritance_telemetry`
  - protocol events carry requested/effective model+reasoning seam:
    - begin: requested model/effort intent
    - end: requested + effective model/effort finalization
  - app-server v2 item shape includes both compatibility and explicit effective fields:
    - `requestedModel`, `requestedReasoningEffort`
    - `model`, `reasoningEffort` (compat lane)
    - `effectiveModel`, `effectiveReasoningEffort` (explicit lane)
    - context inheritance fields + telemetry + delegation report
  - child prompt enrichment is separate from protocol/UI storage:
    - protocol/UI keep the full report record
    - prompt injection uses either a filtered projection or a router-generated block, depending on profile
- Router seam (live vs replay):
  - Live path:
    - core emits `EventMsg::CollabAgent*`
    - app-server maps to `ItemStarted`/`ItemCompleted` with `ThreadItem::CollabAgentToolCall`
    - TUI consumes live notifications and renders spawn/wait/send/close/resume rows
  - Replay path:
    - rollout events are reconstructed in `thread_history.rs`
    - begin/end are merged/upserted into a single `ThreadItem::CollabAgentToolCall` state
    - TUI consumes replayed items with equivalent field semantics
  - Replay caveat:
    - interaction tools normalize to `ThreadItem::CollabAgentToolCall { tool: SendInput, ... }` at the `ThreadItem` layer (legacy `send_input` already uses that lane; `send_message`/`assign_task` collapse into it)
    - this normalization happens in both live app-server notifications and snapshot/replay projection, so tool-name identity is not preserved
- Runtime state ownership:
  - core owns policy validation and spawn execution semantics
  - app-server owns event-to-notification projection
  - app-server-protocol owns replay reconstruction and wire shape
  - TUI owns operator-facing rendering and formatting decisions
- Branding status:
  - carriers are native (`Collab*`, `ThreadItem`), but the stricter delegation semantics and context-inheritance hard-disable policy are fork-owned behavior
- Confirmed residues:
  - output schema still exposes context inheritance fields for compatibility, even while input control is disabled by policy
  - v2 item keeps both `model` / `reasoningEffort` (compat lane) and `effectiveModel` / `effectiveReasoningEffort` (explicit lane) to avoid ambiguity for mixed-version readers
- Display caveats:
  - spawn prompt/meta now render full text in `tui/src/multi_agents.rs` (no prompt-preview ellipsis)
  - final `Completed/Error` agent status summaries remain preview-truncated by design

## External Contract Index

### Hook action wire contracts

- Action types:
  - `nero_hook_msg`
  - `visible_note`
  - `auto_user_reply`
- Envelope:
  - `{"actions":[...]}`
- Nero hook msg schema highlights:
  - `mode`, `show`, `freq`, `format`, `status`, `msg`

### Native event carriers used by Nero contracts

- `EventMsg::Warning` carrying `WarningEvent`
- `HookCompletedEvent` carrying hook summary/meta/entries

### Hook summary/meta contracts used by stop-centric lane

These are diagnostic keys carried in hook summary meta, not typed public RPC schema:

- `protocol.stop_checkpoint_expected`
- `protocol.stop_checkpoint_delivered`
- `protocol.contract_satisfied`
- follow-up status counters in runtime meta

### App-server RPC contracts

- `thread/sessionAuto/read`
- `thread/sessionAuto/inputActivity`
- `thread/sessionAuto/update`
- authority enum type on the wire: `ThreadSessionAutoAuthorityMode::{BridgeProxy, AppServerAuthority}`
- live read/update path currently returns: `AppServerAuthority`
- read response wire shape: `{ threadId, authority, state }`
- input-activity response wire shape: `{ threadId, applied, authority, generationEpoch }`
- update request:
  - required `threadId`, `expectedVersion`
  - optional `expectedSessionSource`
  - nullable merge-patch fields: `enabled`, `autonomyLevel`, `autonomyStepPerRound`, `maxAutoRounds`, `doneStopScope`, `autoRounds`
  - plain bool field: `resetCounter`
  - omission keeps current override, `null` clears override
- update response wire shape: `{ threadId, authority, applied, conflict, message?, errorCode?, reasonCode?, state? }`

### Protocol session contract

- `nero_auto_runtime` (session configured/update surfaces)

### Auth rotation contracts

- `CODEXN_AUTH_ROTATE_CMD`
- `CODEXN_AUTH_ROTATE_CMD_TIMEOUT_MS`
- `CODEXN_ROTATION_REASON`

### Model fallback contracts

- `[nero.model_fallback]` with ladder/cooldown/sticky controls
- Nero config overlay env ingress keys for config resolution

### Multi-agent delegation contracts

- Tool ingress / egress:
  - `spawn_agent` v1 returns `agent_id`; v2 requires `task_name` and returns `task_name`, with `agent_id: null` kept for compatibility
  - related tools: `send_input`, `send_message`, `assign_task`, `resume_agent`, `wait_agent`, `close_agent`, `list_agents`
  - `send_input` is the legacy agent-id path and accepts `message` or `items`
  - `send_message` and `assign_task` share the same text-only input shape in MultiAgentV2; `assign_task` triggers a turn, `send_message` only queues
  - `close_agent` and `send_message` / `assign_task` resolve agent ids or canonical `task_name` values in MultiAgentV2
  - `close_agent` defaults to `mode: safe_close`; it rejects active targets or active descendants and requires explicit `mode: force_cancel` to terminate running subtree work
  - optional `delegation_report` input has 9 required top-level fields when present, with 1-10 bounds on the three score fields and `expected_duration_minutes > 0`
  - optional nested `orchestration_context` is validated only when present, with hard per-field
    string/pattern rules
  - whether the whole report is optional or required at spawn ingress is controlled by `spawn_delegation_report_profile`
  - spawn output still includes `delegation_report`, with `null` when not provided
  - `fork_context` / `context_inheritance` are rejected at handler level
  - child prompt enrichment is profile-dependent:
    - default `optional_only_ui` keeps the report out of child prompt context
    - richer profiles forward a filtered projection
    - `orchestration_router_block` uses a current-stage bridge helper to generate the injected block and requires `orchestration_context`
  - request/response shape highlights:
    - `send_input` / `send_message` / `assign_task`: return `submission_id`
    - `close_agent`: `target` + optional `mode` (`safe_close` default, `force_cancel` explicit) -> `previous_status`
    - `resume_agent`: `id` -> `status`
    - `wait_agent` v2: optional `targets` (+ optional `timeout_ms`) -> `message` + `pending[]` + `timed_out` + `wait_outcome`; targeted waits return after first observed completion, while timeout-only calls use mailbox-activity mode
    - `list_agents`: optional `path_prefix` -> `agents[]` (`agent_name`, `agent_status`, `last_task_message`)
- Event and item contracts:
  - protocol events: `CollabAgentSpawnBeginEvent`, `CollabAgentSpawnEndEvent`, `CollabAgentInteractionBeginEvent`, `CollabAgentInteractionEndEvent`, `CollabWaitingBeginEvent`, `CollabWaitingEndEvent`, `CollabCloseBeginEvent`, `CollabCloseEndEvent`, `CollabResumeBeginEvent`, `CollabResumeEndEvent`
  - app-server v2 item: `ThreadItem::CollabAgentToolCall` carrying:
    - `id`
    - `tool`, `status`, `senderThreadId`, `receiverThreadIds`, `prompt` (rendered preview text, not original input payload)
    - `requestedModel` / `requestedReasoningEffort`
    - `model` / `reasoningEffort` as compatibility mirror
    - `effectiveModel` / `effectiveReasoningEffort`
    - `contextInheritanceRequested`, `contextInheritanceEffective`, `contextInheritanceTelemetry`
    - `delegationReport`
    - `agentsStates`
- Compatibility policy:
  - keep the compat pair (`model`, `reasoningEffort`) for older item data; prefer `effectiveModel` / `effectiveReasoningEffort` for post-resolution semantics and fall back to the compat pair when reading mixed-version snapshots
  - keep context inheritance output fields for historical/replay continuity, despite disabled ingress knobs

### Bridge command/module contracts

- configured module name for the shared-seam runtime bridge contract: `nero_hook_runtime.session_auto_bridge`
- actively used command in current `codex-rs`: `read-session-auto`, which short-circuits to in-process shared-seam logic before any external module invocation
- historical/compatibility naming may reference apply-style flows, but current active `thread/sessionAuto/*` RPC does not depend on a bridge `apply-session-auto` path

## Flowcharts

### A) Stop-centric Hook Msg / Hook Auto / User Report

```mermaid
%%{ init: { 'theme': 'dark' } }%%
flowchart TD
    A[STOP parser classifies output] --> B{continue_processing == false?}
    B -- yes --> C[Set should_stop and break before AfterAgent]
    B -- no --> D{blocked with valid continuation reason?}
    D -- yes --> E[build_hook_prompt_message]
    E --> F{Prompt built?}
    F -- no --> G[Fail-closed abort with EventMsg::Error]
    F -- yes --> H[Persist HookPrompt]
    H --> I[STOP debug reporting from final HookPrompt]
    I --> J[STOP checkpoint delivered]
    D -- no --> K[No HookPrompt injection on this STOP path]
    J --> L[Turn loop continues immediately]
    K --> M[Continue to AfterAgent in current pass]
    L --> N[Later pass reaches AfterAgent]
    M --> O[AfterAgent consumes pending auto_user_reply]
    N --> O
    O --> S[Nero warning formatter and HookCompleted meta]
    O --> P{STOP delivery contract satisfied?}
    P -- yes --> Q[Queue follow-up]
    P -- no --> R[Block follow-up and emit warning]
```

### B) Session-auto Authority / F1-F5

```mermaid
%%{ init: { 'theme': 'dark' } }%%
flowchart TD
    A[F1-F5 key] --> B[detect_nero_auto_hotkey_action]
    B --> C[AppEvent::ApplyNeroAutoHotkey]
    C --> D[handle_nero_auto_hotkey_event]
    D --> E[RPC thread/sessionAuto/read]
    E --> F[App-server local read resolves context and state]
    F --> G[ReadResponse authority=AppServerAuthority]
    G --> H[Build requested runtime update]
    H --> I[RPC thread/sessionAuto/update]
    I --> J{Confirmed main session and CAS valid?}
    J -- no --> K[Conflict or rejected update]
    J -- yes --> L[App-server local update and persisted state]
    L --> M[UpdateResponse authority=AppServerAuthority]
    M --> N[TUI derives runtime context from returned state]
```

### C) Dynamic Account Switching / Auth Rotation

```mermaid
%%{ init: { 'theme': 'dark' } }%%
flowchart TD
    A[Model request fails] --> B{usage_limit or quota class?}
    B -- no --> C[Normal error path]
    B -- yes --> D[External auth recovery path]
    D --> E{Recovered?}
    E -- yes --> F[Reload auth and retry]
    E -- no --> G{Permanent external failure?}
    G -- yes --> H[Return refresh/auth failure]
    G -- no --> I[try_recover_with_nero_auth_rotate_command]
    I --> J[Run CODEXN_AUTH_ROTATE_CMD with CODEXN_ROTATION_REASON]
    J --> K{Rotate success?}
    K -- yes --> F
    K -- no --> L[Fail request]
```

### D) Model Fallback

```mermaid
%%{ init: { 'theme': 'dark' } }%%
flowchart TD
    A[Turn requested with model] --> B{Subagent session source?}
    B -- yes --> C[Fallback disabled]
    B -- no --> D[apply_model_fallback_pre_turn]
    D --> E{Fallback switch needed now?}
    E -- yes --> F[Use next ladder step]
    E -- no --> G[Use requested model]
    F --> H[Run turn]
    G --> H
    H --> I{Eligible model failure?}
    I -- no --> J[Keep current state]
    I -- yes --> K[try_model_fallback_after_error]
    K --> L[Set cooldown and choose next step]
    L --> M{Step available within policy?}
    M -- yes --> N[Retry on fallback model]
    M -- no --> O[Return failure]
    N --> P[mark_model_fallback_success on success]
```

## Out-of-Scope Appendix

This atlas maps `codex-rs` only. The systems below are external and included only through boundary contracts:

- `codex-nero-sdk` (`EXTERNAL-CONSUMER`):
  - is a major consumer/producer around hook action contracts in the Nero ecosystem
  - may provide bridge module endpoint `nero_hook_runtime.session_auto_bridge` for shared-seam reads
- `nerobar-ui` (`EXTERNAL-CONSUMER`):
  - consumes app-server RPC and protocol surfaces exposed by `codex-rs`
  - does not redefine `codex-rs` internal capability ownership

## Multi-agent Coverage Map

Goal: map current coverage for policy, wire contracts, replay, and UI projection. This is a coverage
map, not a semantic spawn-readiness gate.

- Core handler policy and validation:
  - `codex-rs/core/src/tools/handlers/multi_agents_tests.rs`
  - key coverage present:
    - context-inheritance ingress rejection (`fork_context`, `context_inheritance`)
    - delegation report roundtrip
    - task_name validation errors
    - delegation-report negative validation assertions for:
      - score bounds (`1..=10`)
      - non-empty required strings
      - `expected_duration_minutes > 0`
- Protocol compatibility:
  - `codex-rs/protocol/src/protocol.rs` deserialization tests for `CollabAgentSpawnEndEvent` only, covering legacy payloads that omit `requested_*` fields and precedence when both requested and legacy shapes are present
- App-server live mapping:
  - `codex-rs/app-server/src/bespoke_event_handling.rs` helper-level tests for `collab_spawn_begin_item` / `collab_spawn_end_item` mapping of requested/effective model fields and context-inheritance telemetry
  - end-to-end app-server integration coverage in `codex-rs/app-server/tests/suite/v2/turn_start.rs` validates live `item/started` + `item/completed` spawn item fields
- Replay reconstruction:
  - `codex-rs/app-server-protocol/src/protocol/thread_history.rs` tests:
    - `reconstructs_collab_spawn_begin_item_with_requested_context_inheritance`
    - `reconstructs_collab_spawn_begin_and_end_as_single_completed_item`
    - `reconstructs_collab_spawn_end_item_with_model_metadata`
  - these cover begin-item reconstruction, begin+end merging, and requested/effective model-reasoning plus context-inheritance telemetry continuity
- TUI rendering:
  - `codex-rs/tui/src/multi_agents.rs` snapshot tests for collab transcript rows
  - `codex-rs/tui/src/chatwidget/tests.rs` coverage for:
    - live/replay spawn rendering
    - requested vs effective model display
    - duplicate begin-row suppression after completed spawn replay
    - legacy fallback when `effective_*` fields are missing
    - delegation report persistence and context-inheritance lines
  - `completed_spawn_item_falls_back_to_legacy_model_fields_when_effective_missing` is the explicit fallback caveat for completed spawn rows

Current replay/bridge limitation to note:

- there is now an app-server E2E parity anchor (`turn_start_spawn_agent_thread_read_replay_matches_live_item_v2`)
  proving live spawn item parity with replayed `thread/read` for the same turn when the thread is started with
  `persist_extended_history: true`; with default limited persistence, collab replay parity is not guaranteed.
- full-chain parity still remains split across app-server + app-server-protocol + TUI fixtures (not a single cross-crate fixture).
- `orchestration_router_block` remains current-stage bridge behavior rather than a final native campaign lane.
