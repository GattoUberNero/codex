# Subagent Fork Context Budgeting

Status: draft for architecture review

Scope: `codex-rs` fork layer only

## Purpose

This document specifies a fork-local capability for making subagent context inheritance materially cheaper, more predictable, and less contaminating without rewriting native upstream thread lineage or the generic thread-fork mechanism.

The target outcome is not semantic summarization. The target outcome is controlled reuse of parent context through bounded rollout suffix selection, backed by runtime budget checks and explicit telemetry.

## Problem Statement

Current subagent context inheritance behavior is effectively binary:

- `false`: child starts without forked parent history.
- `true`: child is spawned with `InitialHistory::Forked(Vec<RolloutItem>)` built from the parent rollout.

This has three major costs:

1. Storage cost
   Child rollout history can immediately inherit a large parent rollout footprint.
2. Prompt/token cost
   Child startup can inherit much more background context than is needed for the delegated task.
3. Context contamination
   Even when the child keeps its own active role/config, inherited parent history can still carry stale prompts, hook payloads, special instructions, and general conversational drag that are semantically unhelpful or actively harmful for specialized subagents.

The current system is too coarse:

- it is often useful to give a subagent some parent background,
- but full-history fork is too expensive and too blunt,
- and pure `on/off` does not provide enough control.

## Current Runtime Facts

These points describe the current code behavior and are the basis for the proposal.

### `fork_context` is real and native to multi-agent spawn

Tool surface:

- `fork_context` exists on the `spawn_agent` tool schema in `codex-rs/tools/src/agent_tool.rs`.

Spawn wiring:

- `fork_context=true` is mapped to `fork_parent_spawn_call_id` in:
  - `codex-rs/core/src/tools/handlers/multi_agents/spawn.rs`
  - `codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs`

Fork execution:

- `spawn_agent_with_metadata(...)` loads the parent rollout, appends a synthetic output item for the parent spawn call, and creates `InitialHistory::Forked(forked_rollout_items)` in:
  - `codex-rs/core/src/agent/control.rs`

### The current public contract says `fork_context=true` means exact context

The current tool schema description says:

- “When true, fork the current thread history into the new agent before sending the initial prompt. This must be used when you want the new agent to have exactly the same context as you.”

Implication:

- v1 must not silently redefine existing `fork_context=true` to mean “maybe trimmed.”
- any bounded mode must be introduced as an explicit fork-local policy or a future explicit tool/runtime mode, not as a hidden reinterpretation of the current public meaning.

### Child config is still built separately from the forked history

Subagent spawn config is built before the child thread starts:

- `build_agent_spawn_config(...)` in `codex-rs/core/src/tools/handlers/multi_agents_common.rs`

That path:

- starts from current turn/runtime config,
- strips Nero main-agent and auto developer instructions for subagents,
- sets `config.base_instructions = Some(parent_current_base_instructions)` before role application,
- then applies the selected role through `apply_role_to_config(...)`.

This means current child sessions are not simply “parent prompt copied wholesale.”

### Formal instruction precedence is not the same as inherited context

Base instructions resolve in this priority order:

1. `config.base_instructions`
2. `conversation_history.get_base_instructions()` from `SessionMeta`
3. model default instructions

That resolution happens in `codex-rs/core/src/codex.rs`.

Implication:

- base-instructions precedence is still `config` first and forked `SessionMeta` second,
- but for role-backed subagents that rebuild config through `apply_role_to_config(...)`, that earlier runtime `config.base_instructions` assignment is not an unconditional invariant,
- so forked `SessionMeta` can still become the effective fallback source after role application if the rebuilt config no longer carries the direct override.

Developer instructions are also explicitly rebuilt for subagents:

- `strip_codexn_fork_subagent_developer_instructions(...)`
- `refresh_codexn_fork_developer_instructions(...)`

These live in `codex-rs/core/src/config/mod.rs`.

Current v1 evidence for this area is intentionally narrower:

- the subagent spawn-config build step strips Nero main-agent / auto developer overlays,
- the config machinery clearly rebuilds subagent-specific developer instructions before spawn,
- and live bounded/exact spawn tests prove the fork payload mode/reporting on the real spawn path,
  without yet serving as a dedicated end-to-end proof of child role/base/developer instruction
  precedence for every role variant.

A dedicated end-to-end assertion for role/base/developer precedence remains useful follow-up work.
For v1, the hard acceptance gate is that budgeting must not change the shared spawn-config path,
and the live spawn path must prove the actual inheritance mode/reporting behavior.

### The real risk is inherited background history

Even with proper child role/config, `InitialHistory::Forked(...)` still reconstructs parent rollout history into the child startup state.

That is where the actual problem sits:

- unnecessary prompt baggage,
- heavier startup,
- higher chance of semantic bleed from irrelevant parent history.

## Capability Definition

Capability name:

- Subagent Fork Context Budgeting

Capability role:

- Internal service-enabling capability for `codex-rs` multi-agent spawn.

User-facing effect:

- requested exact `fork_context=true` stays on the exact-inheritance lane unless runtime safety
  must suppress it fully to `off` (for example invalid parent spawn pairing),
- a new bounded inheritance mode can be introduced for cases where exact fork is not desired,
- bounded inheritance is measurable, reported, and intentionally distinct from exact fork.

Primary objective:

- preserve the usefulness of parent grounding,
- while preventing obviously wasteful or destabilizing full-history forks.

Non-objectives for v1:

- no semantic summarization,
- no custom mini-compaction engine,
- no upstream-wide rewrite of thread lineage/fork internals,
- no per-role policy matrix yet,
- no new prompt reasoning layer that “explains” the trim.

## Design Principles

1. Fail cheap before failing smart.
   Prefer simple structural trimming to expensive semantic processing.
2. Preserve required structural items.
   Never strip blindly if the result becomes invalid or loses required startup metadata.
3. Treat count-based heuristics as prefilters, not ground truth.
4. Use runtime-visible budgets, not guessed global constants.
5. Emit explicit telemetry whenever context is requested, trimmed, or suppressed.
6. Keep current `fork_context` semantics recognizable to the caller.
   The caller asked for context inheritance, so the system should not silently degrade to “no context” unless that is explicitly reported.
7. Do not silently reinterpret a public exactness flag.
   If the runtime wants bounded inheritance, it must use an explicitly different mode from `fork_context=true`.

## Proposed v1 Model

### High-level approach

When a subagent is spawned with bounded inheritance enabled:

1. Collect the parent rollout items.
2. Preserve replay-safe ordering and pairing invariants.
3. Use a fast count-based heuristic to choose an initial candidate history size.
4. Compute approximate token usage for the candidate fork payload.
5. If it is within the allowed child startup budget, keep it.
6. If it exceeds the allowed budget, trim by dropping oldest replay-safe user-turn segments and recompute until the candidate falls within the allowed range.
7. Emit telemetry describing what was requested and what actually shipped.

### Replay-safe preservation rule

The budgeting mechanism must never construct an arbitrary `required_items + newest(payload)` list.

Current replay semantics are richer than a flat append-only stream:

- `SessionMeta` is read during fork startup for `forked_from_id`, base-instructions fallback, and dynamic tools,
- `Compacted` can carry replacement history,
- `ThreadRolledBack` mutates effective user-turn boundaries,
- `TurnStarted` / `TurnComplete` / `TurnAborted` delimit replay segments,
- `TurnContext` hydrates previous-turn state and reference context,
- the synthetic spawn `FunctionCallOutput` is only valid when the matching parent `FunctionCall` is still present and ordered correctly.

Therefore v1 must operate on replay-safe suffixes or replay-safe turn truncation boundaries, not raw rollout-item splitting.

### Candidate construction rule

For v1, candidate construction should be defined in terms of replay-safe history trimming:

- start from the exact full fork candidate,
- keep `SessionMeta` pinned,
- trim only along rollback-aware, replay-compatible turn boundaries,
- preserve any required synthetic spawn-call pairing,
- never emit a candidate that violates existing replay or pairing invariants.

This still keeps the “newest context is favored” spirit, but it respects the actual semantics of the rollout machinery.

Important caveat:

- the current simple rollout truncation helper only indexes real user messages,
- but replay semantics also treat assistant inter-agent instruction messages as turn boundaries,
- so v1 must not blindly reuse that helper as the final definition of bounded-fork trimming for multi-agent histories.

Instead, v1 should either:

- introduce a dedicated replay-compatible bounded-fork boundary helper, or
- extend existing truncation logic until it matches the replay-compatible turn-boundary rules used by history reconstruction.

## Budget Model

### Runtime budget source

The budgeting system should use the child’s practical context budget, not the model’s theoretical maximum context window.

Target source:

- ideally the real pre-compaction usable token budget for the child session,
- but v1 should assume that exact value is not currently available at the `AgentControl` fork-construction seam.

The budget input to this mechanism should be represented conceptually as:

- `usable_context_budget_tokens`

For implementation planning, v1 should use a seam-available proxy, for example:

- child `ModelInfo::auto_compact_token_limit()` when it is available from resolved child model info,
- minus a mandatory startup reserve for safe headroom and non-fork overhead.

This is a better v1 proxy than raw `context_window * effective_context_window_percent`, because turn execution already checks practical compaction pressure against `auto_compact_token_limit()`.

Important:

- `auto_compact_token_limit()` is a ceiling for the full child session/turn usage, not for fork payload alone,
- so bounded fork acceptance must subtract non-fork startup overhead before deciding that a fork candidate is safe.

Conceptually:

- `usable_context_budget_tokens = auto_compact_token_limit - startup_overhead_reserve_tokens`

Where `startup_overhead_reserve_tokens` is mandatory in v1 and covers at least:

- the initial delegated operation,
- instruction/tool overhead,
- and a safety margin against immediate follow-up compaction pressure.

Exact “usable budget before compaction” can remain a later refinement once it is cleanly exposed at this seam.

### Reference normalization

The count-based heuristic model discussed during design remains a useful future optimization, but it
is not a required part of v1.

Current v1 behavior is intentionally simpler:

- materialize the exact parent fork candidate,
- validate replay-safe truncation points,
- estimate tokens for the current candidate,
- and only then trim oldest replay-safe user-turn segments until the candidate fits or must be suppressed.

So for v1:

- reference-budget normalization is optional future work,
- not a required runtime behavior,
- and operator expectations should be set by replay-safe token validation, not by `obj_max_*`
  style pre-sizing.

## Statistics and Calibration

### Why statistics are needed

Payload rollout items are not size-uniform.

So item count alone cannot be trusted as the final gate.

But item count is still useful as a cheap first-pass predictor if it is backed by observed density.

### Proposed calibration inputs

For v1, calibration can be simple and local:

- derive a rolling estimate from sampled parent rollouts,
- store it locally per workspace or equivalent runtime scope,
- fall back to a conservative built-in density if no local samples exist yet.

Potential future storage:

- local config/state under the active `cwd`,
- later widened to broader cache scopes if needed.

### Calibration outputs

The minimal useful calibration state is:

- `reference_budget_tokens`
- `obj_max_density`
- `sample_count`
- `last_updated_at`

Optional future fields:

- `median_payload_tokens_per_item`
- `p90_payload_tokens_per_item`
- model/provider split if runtime evidence shows large enough divergence

## Proposed Algorithm

### Inputs

- parent rollout items
- child resolved spawn config
- child model-info-derived budget proxy
- local density calibration
- caller request: bounded inheritance mode

### Output

- a replay-safe bounded `InitialHistory::Forked(trimmed_rollout_items)`, or explicit suppression if no viable replay-safe candidate remains

### Steps

1. Materialize and flush the live parent rollout, then read parent rollout items.
2. Build the exact full-fork candidate the current implementation would produce.
3. Identify replay-safe user-turn truncation boundaries using existing or equivalent rollback-aware helpers.
4. Estimate token usage of the exact full-fork candidate.
5. Compute candidate fill versus `usable_context_budget_tokens`, where that usable budget already subtracts mandatory non-fork startup reserve from the child compaction ceiling.
6. If within threshold:
   - accept candidate
7. Else:
   - repeatedly drop oldest replay-safe user-turn segments,
   - recompute token estimate,
   - stop when candidate falls within threshold
8. If no replay-safe candidate remains:
   - suppress bounded inheritance for this spawn
9. Emit telemetry:
   - requested fork,
   - effective inheritance mode,
   - replay-safe turns requested vs shipped,
   - estimated shipped tokens when inheritance actually ships fork payload,
   - whether trimming occurred.

## Policy Choice for Empty Payload

This needs to be explicit.

Recommended v1 policy:

- if no replay-safe bounded candidate remains,
- suppress bounded inheritance for that spawn,
- and report that suppression explicitly.

Do not silently pretend the child received meaningful parent context when it did not.

## Telemetry and Operator Visibility

This capability is not safe if it is invisible.

### Required telemetry

For each forked subagent spawn, emit at least:

- whether exact fork or bounded inheritance was requested,
- whether bounded inheritance was applied,
- parent replay-safe turn count,
- shipped replay-safe turn count,
- estimated shipped tokens when effective mode actually ships fork payload,
- child budget proxy,
- whether trimming occurred

### Suggested operator-facing statuses

- `fork_mode=exact`
- `fork_mode=bounded-full`
- `fork_mode=bounded-trimmed`
- `fork_mode=bounded-suppressed`

The parent agent should not be left to assume that “full context was sent” if the runtime used bounded mode and trimmed or suppressed it.

### Delivery surface for v1 telemetry

The current fork-local implementation now exposes inheritance reporting on active spawn surfaces:

- `spawn_agent` tool output includes:
  - `context_inheritance_requested`
  - `context_inheritance_effective`
  - `context_inheritance_telemetry`
- persisted `CollabAgentToolCall` history items in app-server protocol also include the same fields

This means v1 no longer needs a separate diagnostic-only lane just to make bounded-mode decisions visible on the main spawn path.

However, begin/end event persistence is still asymmetric, so the spec should treat the durable reporting surface as:

1. tool output first
2. persisted thread history items second
3. ephemeral begin/end notifications only as supplemental UI signals

Persistence caveat:

- `CollabAgentSpawnBegin` is not persisted,
- `CollabAgentSpawnEnd` is persisted only in extended mode.

So if fork-mode reporting must survive resume/replay, v1 should prefer a persisted lane such as:

- `spawn_end`,
- tool output,
- or another explicitly persisted event/warning surface,

instead of treating begin/end expansion as interchangeable.

## Configuration Surface

V1 should remain deliberately small.

Suggested internal config concepts:

- `enabled`
- `reference_budget_tokens`
- `start_ratio`
- `max_allowed_context_fill_pp`
- `overflow_tolerance_pp`
- `local_density_sampling_enabled`
- `telemetry_mode`
- `mode`

This surface is intentionally about budgeting, not agent policy authoring.

`mode` should separate exact and bounded inheritance explicitly. Example conceptual values:

- `exact`
- `bounded`
- `off`

Per-role enable/disable rules can come later.

## Compatibility and Safety

### Why this is safer than a full rewrite

This approach:

- does not rewrite upstream thread lineage,
- does not replace the native fork mechanism,
- does not require semantic prompt synthesis,
- only constrains the size of the fork payload before thread startup.

That keeps the blast radius smaller than replacing subagent fork behavior wholesale.

### Known limitations of v1

- suffix trimming is still blind,
- newest context is not always the best context,
- approximate token estimation can be off,
- local density calibration may be rough at first,
- some subagent types may still be better off with no inherited context at all

These are acceptable trade-offs for v1 because the goal is controlled cost reduction, not perfect context selection.

## Capability UX

### User-visible experience

From the user/operator point of view:

- exact fork remains a supported option,
- bounded inheritance becomes a distinct option or policy,
- and the runtime becomes explicit about whether the child got:
  - exact inherited context,
  - bounded full context,
  - bounded trimmed context,
  - or suppressed bounded inheritance

### Main-agent experience

The main agent should be able to reason with accurate reporting:

- “I asked for inherited context”
- “the runtime used bounded mode and trimmed it to fit”
- “the runtime suppressed bounded inheritance because no valid replay-safe candidate remained”

This is essential for good orchestration.

## Implementation Boundaries

Recommended implementation boundary for v1:

- integrate close to the fork payload construction path in `AgentControl`,
- keep the budgeting logic in a dedicated module rather than bloating existing multi-agent files,
- keep token-estimation helpers separate from rollout-boundary selection helpers.

Suggested ownership split:

- selection logic: new focused module
- telemetry emission: existing event/reporting lane
- config: narrow extension, not a broad policy system

## Verification Plan

### Unit tests

1. Replay-safe trimming:
   - trimming preserves replay compatibility and spawn-call pairing invariants
2. Count heuristic:
   - `obj_max_child` scaling behaves deterministically
3. Threshold behavior:
   - no trim when candidate is in budget
   - trim when candidate exceeds threshold
4. Empty payload behavior:
   - bounded mode suppresses cleanly when no replay-safe candidate remains

### Integration tests

1. Forked spawn with large parent rollout:
   - child receives replay-safe trimmed history, not the exact full fork
2. Child role preservation:
   - role/base/developer instruction behavior remains unchanged
3. Telemetry:
   - emitted status matches actual selected fork mode
4. Storage sanity:
   - child rollout size is materially reduced in bounded mode versus full fork

### Review criteria

The capability is acceptable when:

- it does not change the shared child role/config construction path,
- it materially reduces child fork payload size in large-parent scenarios,
- it reports the effective result honestly,
- it does not require upstream lineage surgery.

Coverage note for v1:

- selector/unit coverage is required for rollback-safe trimming logic,
- spawn-surface coverage is required for exact-mode telemetry and bounded suppression/full/trimmed
  outcomes,
- dedicated end-to-end role/base/developer precedence assertions are valuable follow-up coverage,
  but are not part of the narrow v1 acceptance gate for budgeting itself.

## Delivery Plan

### Phase 1: Measurement and seam confirmation

- confirm the best seam-available child budget proxy
- identify replay-safe truncation boundaries and pairing invariants
- identify the narrowest safe injection point before `InitialHistory::Forked(...)`

### Phase 2: Internal spec lock

- lock symbolic config names and threshold semantics
- lock telemetry vocabulary
- confirm `percentage points` semantics for `overflow_tolerance_pp`
- lock exact vs bounded mode separation so current `fork_context` contract is not silently changed

### Phase 3: Implementation v1

- implement bounded fork payload builder
- wire it into subagent spawn only behind an explicit bounded mode selection
- keep `fork_context=true` plus default mode on the existing exact-fork path
- add telemetry/reporting

### Phase 4: Verification

- targeted tests
- storage sanity check
- multi-agent regression review

### Phase 5: Optional future policy layer

Later, but not in v1:

- per-role fork policy
- role-level `no_context` opt-out
- alternative modes like `recent_only` or `summary_only`

## Recommendation

Proceed with this capability.

Reason:

- it addresses a real and high-cost weakness in the current binary fork behavior,
- it preserves the useful part of `fork_context`,
- it limits implementation risk,
- and it creates a path toward future finer-grained policy without forcing a full rewrite now.
