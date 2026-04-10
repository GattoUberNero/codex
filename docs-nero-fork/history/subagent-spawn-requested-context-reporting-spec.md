# Subagent Spawn Requested Context Reporting

Status: draft for architecture review

Scope:

- `codex-rs/protocol`
- `codex-rs/core` spawn handlers
- `codex-rs/app-server-protocol` thread-history mapping
- `codex-rs/app-server` bespoke event mapping
- `codex-rs/tui` collaboration transcript rendering

## Purpose

This document specifies a narrow fork-local capability for making subagent spawn intent visible
before the runtime resolves the final inheritance outcome.

The goal is simple:

- when the main agent asks to spawn a subagent,
- the system should explicitly show whether that spawn was requested with parent-context
  inheritance,
- and it should continue to show the final effective runtime outcome separately.

This capability is intentionally not a new telemetry lane. It is an extension of the already
existing collaboration-spawn event flow.

## Problem Statement

Today the collaboration spawn UI is strongest at the end of the spawn lifecycle:

- `CollabAgentSpawnEndEvent` already reports the effective inheritance mode,
- it already carries inheritance telemetry,
- and the TUI already renders that result.

But the system is weaker at the start of the lifecycle:

- `CollabAgentSpawnBeginEvent` currently reports prompt/model/reasoning only,
- so the user cannot see the main agent's explicit request about context inheritance until after
  the runtime has already resolved the spawn,
- and the intent/effect split is therefore less visible than it should be.

That makes it harder to reason about:

1. whether the main agent intended to fork parent context at all,
2. whether the runtime changed that intent,
3. whether a final bounded/suppressed result came from the runtime or from the main agent never
   asking for context in the first place.

## Current Runtime Facts

### Spawn already has a natural two-phase event model

The existing spawn flow already emits:

- `CollabAgentSpawnBeginEvent`
- `CollabAgentSpawnEndEvent`

These live in:

- `codex-rs/protocol/src/protocol.rs`

And the core spawn handlers already use them in:

- `codex-rs/core/src/tools/handlers/multi_agents/spawn.rs`
- `codex-rs/core/src/tools/handlers/multi_agents_v2/spawn.rs`

This is important because it means the desired capability already has a natural seam:

- begin event = requested intent,
- end event = effective runtime outcome.

### Requested inheritance is already known at spawn-begin time

Before `AgentControl::spawn_agent_with_metadata(...)` is called, the spawn handlers already
resolve the requested inheritance mode:

- `off`
- `exact`
- `bounded`

That requested mode is already available in the spawn handlers before the begin event is sent.

So this capability does not require speculative inference, replay inspection, or post-hoc
guessing. The requested mode already exists as real runtime data at the correct seam.

### Effective inheritance belongs to spawn-end, not spawn-begin

The actual effective result can only be known after runtime validation/budgeting:

- `exact`
- `bounded_full`
- `bounded_trimmed`
- `bounded_suppressed`
- `off`

And the runtime telemetry for that effective result already belongs to:

- `CollabAgentSpawnEndEvent`

This must not be moved earlier or blurred into the begin event.

## Capability Definition

Capability name:

- Subagent Spawn Requested Context Reporting

Capability role:

- Internal service-enabling capability for `codex-rs` collaboration spawn observability.

User-facing effect:

- the user sees the main agent's context-inheritance request at spawn-begin time,
- the user later sees the effective runtime result at spawn-end time,
- and the UI makes the difference between intent and effect explicit.

Primary objective:

- expose main-agent intent without inventing a new logging subsystem.

Non-objectives:

- no new independent telemetry channel,
- no new audit log file,
- no reinterpretation of runtime inheritance semantics,
- no duplication of spawn-end telemetry on the begin event,
- no additional reporting for unrelated collaboration events in this capability.

## Design Principles

1. Reuse the existing collaboration spawn lane.
   Do not create a parallel reporting system when a correct event family already exists.
2. Keep intent and effect separate.
   Begin event reports what was requested; end event reports what actually happened.
3. Do not over-report early.
   Spawn-begin must not pretend to know budgets, trimming, suppression, or shipped token counts.
4. Prefer small protocol growth over TUI-only inference.
   The begin event should carry the requested mode explicitly, not rely on local reconstruction.
5. Stay narrow.
   This capability is about spawn observability, not broader review/agent-debug infrastructure.

## Proposed v1 Design

### Wire-level change

Add one optional field to `CollabAgentSpawnBeginEvent`:

- `context_inheritance_requested: Option<SpawnContextInheritanceMode>`

Expected values:

- `off`
- `exact`
- `bounded`

Rules:

- if the spawn path resolved a requested inheritance mode, the begin event should carry it,
- if a legacy or non-standard path has no resolved value, `None` remains allowed for compatibility,
- begin event must not carry effective inheritance mode,
- begin event must not carry inheritance telemetry.

### Core emission behavior

At spawn begin:

- emit the main agent's resolved requested inheritance mode as-is,
- before runtime budgeting or validation changes anything.

At spawn end:

- continue emitting the effective inheritance outcome and telemetry exactly as today.

This preserves a clean mental model:

- begin answers: "what did the main agent ask for?"
- end answers: "what did the runtime actually do?"

### TUI rendering behavior

The collaboration transcript should render:

- on spawn begin:
  - a concise line such as `requested context inheritance: bounded`
- on spawn end:
  - the existing effective line such as `context inheritance: bounded -> bounded_trimmed`
  - plus existing telemetry lines

Important:

- in the live event flow, begin intent and end result should not be merged into one synthetic
  status,
- because users need to see both the initial intent and the final effective result while the spawn
  is happening.
- begin-phase UX must not imply that the new child thread identity is already known; that identity
  belongs to spawn-end once `new_thread_id` exists.

Important runtime caveat:

- app-server history reconstruction currently upserts collaboration spawn items by `call_id`,
- so a persisted/replayed history entry can collapse begin-state into end-state,
- therefore v1 should guarantee distinct begin/end visibility in the live spawn flow,
- but v1 must not claim that snapshot/replay automatically preserves two distinct historical rows
  unless the underlying history representation changes.

### Why this is the least-complex path

This design reuses:

- existing protocol event family,
- existing TUI collaboration transcript surfaces,
- existing effective telemetry model,
- existing spawn-handler runtime data.

It avoids:

- new warning/event categories,
- new app-server-specific reporting families,
- duplicated telemetry structures,
- heuristics in the TUI to infer requested mode from the end event.

## UX Shape

### Begin-phase UX

At the moment the spawn is initiated, the collaboration transcript should reveal:

- the requested spawn parameters already known at begin time,
- with which model/reasoning,
- and whether parent context was requested.

This is an intent-level message.

Examples:

- `requested context inheritance: off`
- `requested context inheritance: exact`
- `requested context inheritance: bounded`

The begin-phase message is about requested parameters only. It is not a claim that the child agent
already has a resolved thread identity or final runtime status.

For v1 this guarantee is specifically about the live event flow:

- direct core event delivery,
- and app-server `ItemStartedNotification` / in-progress item delivery.

It is not yet a guarantee that replayed historical transcript reconstruction will keep a separate
begin-phase row after the spawn has completed.

### End-phase UX

When the runtime finishes the spawn resolution, the transcript should continue to render the
effective mode and telemetry.

Examples:

- `context inheritance: exact -> exact`
- `context inheritance: bounded -> bounded_trimmed`
- `context inheritance: bounded -> bounded_suppressed`

And then the existing telemetry lines:

- parent replay-safe turn count
- shipped replay-safe turn count
- estimated shipped tokens
- usable context budget
- suppression reason

This creates an explicit `requested -> effective` story without inventing extra categories.

## Compatibility

### Backward compatibility

This should be a compatible protocol extension:

- old consumers can ignore the new begin-event field,
- current end-event shape does not need semantic change,
- current runtime budgeting logic remains untouched.

### Compatibility with existing bounded-fork work

This capability complements the existing bounded-fork telemetry work:

- it does not replace the spawn-end report,
- it does not alter `SpawnContextInheritanceTelemetry`,
- it does not alter the acceptance criteria for bounded-fork budgeting,
- it only exposes the missing intent-side half of the story.

## Implementation Boundaries

Recommended implementation surface:

1. Protocol
   - extend `CollabAgentSpawnBeginEvent`
2. Core spawn handlers
   - fill the requested inheritance field in:
     - `multi_agents/spawn.rs`
     - `multi_agents_v2/spawn.rs`
3. App-server protocol/history bridge
   - propagate `context_inheritance_requested` into `ThreadItem::CollabAgentToolCall` for
     `InProgress` spawn items
4. App-server bespoke event mapping
   - propagate the same begin-field into `ItemStartedNotification`
5. TUI
   - extend `SpawnRequestSummary` so the pending begin-state can retain requested inheritance mode
   - render a one-line requested-mode detail from the live begin/pending summary
   - keep spawn-end rendering as the source of truth for effective mode and telemetry

Implementation note:

The cleanest shape is likely:

- put the requested mode directly on `CollabAgentSpawnBeginEvent`,
- extend `SpawnRequestSummary` to retain it between begin and end if needed,
- propagate it through app-server thread-history / started-notification seams instead of forcing
  TUI inference,
- treat live begin visibility and persisted replay visibility as separate concerns,
- keep `spawn_end(...)` unchanged except for any optional formatting improvements.

This minimizes moving parts.

## Verification Plan

### Unit / protocol coverage

1. Protocol schema/export reflects the new begin-event field.
2. Begin-event serialization includes `context_inheritance_requested` when present.
3. Legacy `None` begin-event behavior remains accepted.

### Core behavior coverage

1. `multi_agents/spawn.rs`
   - begin event emits the resolved requested mode
2. `multi_agents_v2/spawn.rs`
   - begin event emits the resolved requested mode
3. Requested mode in begin event matches the same requested mode later used to derive the
   effective report in spawn-end.

### TUI coverage

1. Spawn-begin transcript renders requested context inheritance.
2. Spawn-end transcript still renders effective context inheritance and telemetry.
3. Snapshot coverage shows the full `requested -> effective` story for at least:
   - `off`
   - `exact`
   - `bounded -> bounded_full`
   - `bounded -> bounded_trimmed`
   - `bounded -> bounded_suppressed`

### App-server bridge coverage

1. `thread_history.rs` begin handling preserves `context_inheritance_requested` for in-progress
   spawn items instead of hardcoding `None`.
2. `bespoke_event_handling.rs` begin notification preserves the same field for
   `ItemStartedNotification`.
3. App-server-backed live begin delivery shows the requested mode before spawn-end arrives.
4. Replay/history tests explicitly document the v1 limitation: completed spawn reconstruction may
   collapse begin into end when both share the same `call_id`.

## Acceptance Criteria

This capability is acceptable when:

- the begin event carries the main agent's requested inheritance mode,
- the app-server-backed live thread-history/event bridge preserves that begin value instead of
  dropping it to `None`,
- the end event continues to carry the effective runtime outcome,
- the TUI clearly shows both intent and effect in the live spawn flow,
- no new parallel telemetry subsystem is introduced,
- the change stays localized to the existing collaboration spawn lane,
- the spec does not claim replay/history preserves two distinct begin/end rows unless the
  underlying upsert-by-`call_id` behavior is explicitly changed.

## Explicit Non-Goals for v1

This capability does not attempt to answer:

- how much context should be inherited,
- whether bounded mode should be default,
- whether different agent roles should get different inheritance policies,
- whether spawn begin should also show budget proxies,
- whether other collaboration actions should get similar begin/end intent reporting.

Those can be designed later. This v1 is intentionally narrow and cheap.
