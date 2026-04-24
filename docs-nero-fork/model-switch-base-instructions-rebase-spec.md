# Model Switch Base Instructions Rebase Spec

Status: implemented (MVP)

Scope:

- `codex-rs/core` session configuration and model switch handling
- `codex-rs/core` context-manager developer update logic
- `codex-rs/core` resume behavior tests
- `codex-rs/rollout` session metadata interpretation

## Problem

The current runtime mixes two different concepts:

1. session identity persisted in `session_configuration.base_instructions`,
2. model-switch prompt updates injected later as developer messages.

In practice this means a live session can carry:

- an old base prompt in `session_configuration.base_instructions`,
- and a later developer-layer message containing instructions for a newly selected model.

This has three bad consequences:

1. the effective instruction state becomes layered and hard to reason about,
2. model switching stops being a clean operational tool for changing approach,
3. rollout inspection becomes misleading because `session_meta.base_instructions` no longer reflects the effective model identity after a switch.

For this fork, model switching is not only a cosmetic picker action. It is a deliberate operational technique:

- switch from `gpt-5.4` to a codex-optimized model,
- switch back when a different planning or implementation style is needed,
- use a different model to attack a stalled problem from another angle.

That requires a clean switch of model identity, not additive prompt layering.

## Current state

At session creation, base instructions are resolved in this order:

1. `config.base_instructions`
2. `conversation_history.get_base_instructions()` from rollout/session metadata
3. `model_info.get_model_instructions(config.personality)`

This is implemented in `codex-rs/core/src/codex.rs`.

After the session is created:

- request payloads use `prompt.base_instructions.text`,
- `session_meta.base_instructions` persists the session-level value,
- model changes may add a developer update containing the new model instructions,
- but they do not rewrite the session-level base instructions.

Resume behavior intentionally preserves historical base instructions even if the selected model changes later.

## Goal

Make a live model switch behave as a real prompt rebase:

- when the user explicitly changes model in an active session,
- recompute `session_configuration.base_instructions` from the newly selected model,
- stop injecting model instructions as an additional developer update,
- keep `resume` behavior unchanged by default.

This gives the fork a clean operational contract:

- `resume` preserves historical session identity,
- explicit `model switch` changes the current working identity.

## Non-goals

- No redesign of rollout persistence format in this pass.
- No retroactive rewrite of existing rollout files.
- No attempt to infer whether an old resumed session "should" adopt new runtime prompts automatically.
- No change to explicit `config.base_instructions` override semantics.
- No change to personality template rendering beyond using the new model's already-supported `get_model_instructions(...)`.

## Architectural rule

Two events must have different semantics:

### 1. Resume / fork / historical reconstruction

This path restores an existing session identity.

It should continue to prefer persisted base instructions from session history unless the operator has explicitly supplied a stronger override such as `config.base_instructions`.

### 2. Explicit model switch in a live session

This path is an operational decision to change the active model's working posture.

It should update the session's base instructions to the newly selected model instructions and should not preserve the old model prompt as the active session base.

Those two concepts must not be conflated.

### Trigger definition

For this MVP, `explicit model switch` means:

- a user-originated session settings update that changes `collaboration_mode.settings.model`,
- originating from explicit operator intent such as `Op::UserTurn` or `Op::OverrideTurnContext`.

It does not include:

- automatic runtime fallback,
- server-side reroute,
- retry-driven model substitution,
- resume reconstruction,
- any internal safety path that changes model without explicit user intent.

## Proposed behavior

### A. Session creation and resume

Keep the current startup priority:

1. `config.base_instructions`
2. resumed/forked `session_meta.base_instructions`
3. current model instructions

This preserves historical continuity and keeps old sessions reproducible.

Important clarification:

- this MVP does not change the persisted startup contract for `resume`,
- if resume starts from a historical session whose startup base instructions differ from the currently selected model, startup still prefers historical `SessionMeta.base_instructions`,
- resume-with-different-model must not emit one-time full model guidance as a substitute for live-switch rebasing,
- after resume, rebasing may occur only after a subsequent explicit live model switch event.

### B. Explicit model switch in an active session

When the active model changes inside a running session:

- compute `new_model_instructions = next.model_info.get_model_instructions(next.personality)`,
- if `config.base_instructions` is set, preserve it and do not replace session base instructions,
- otherwise rewrite `state.session_configuration.base_instructions = new_model_instructions`,
- persist that rebased value in in-memory session state for all subsequent requests in the live process,
- this MVP does not add rollout serialization for post-start rebases.

### C. Disable model-switch developer instruction injection for explicit live switches

`build_model_instructions_update_item(...)` should no longer add a developer message containing the full instructions for the new model for explicit live model switches once session base rebasing is active.

Reason:

- after the session base is rebased, that developer message becomes duplicate prompt material,
- duplicate prompt material creates ambiguous or conflicting instruction weight,
- model switching should not produce a "base + corrective append" prompt shape.

Scope note:

- this rule applies to explicit live model switches only,
- resume-path one-time full model guidance is explicitly out of contract and must not be emitted as part of this MVP target behavior.

### D. Personality updates without model change

If the model slug stays the same and only personality changes:

- current personality-specific update behavior must remain unchanged,
- because this is not the same problem as cross-model switching.

This spec is about model-switch rebasing, not personality-only updates.

## Explicit invariants

After this change:

1. for newly switched live sessions under this MVP, only one authoritative full-model instruction set should remain active after an explicit switch,
2. switching models should replace the active model instruction set rather than append another one,
3. `resume` should still restore historical session instructions unless a stronger explicit override exists,
4. `config.base_instructions` should remain the highest-priority operator override and should disable model-based rebasing.

Legacy note:

- old histories may still contain previously persisted `<model_switch>` developer messages,
- this MVP does not retroactively delete or rewrite them.

## Runtime impact

### Expected benefits

- Cleaner and more predictable model switching.
- Better operational use of model switching as a deliberate problem-solving tool.
- Less prompt contamination from old model instructions.
- Easier reasoning about what instructions the model actually received.

### Expected tradeoff

Prompt-prefix stability will decrease when switching models.

This may reduce prompt-cache reuse and increase token cost after a switch.

That is acceptable for this fork because:

- model switching is an explicit operator action,
- the operator is intentionally asking for a new working posture,
- correctness and clarity of model identity are more important here than preserving a stale prompt prefix for cache efficiency.

This tradeoff is not incidental. It is part of the design choice:

- explicit live model switch is treated as a request for a clean working identity change,
- not as a cache-preserving prompt patch.

## Implementation outline

### 1. Rebase session base instructions on explicit model switch

Target area:

- `codex-rs/core/src/codex.rs`

Required change:

- identify the path where an explicit live model change updates session state,
- perform rebasing on persisted in-memory session state, not only on turn-local prompt construction,
- after resolving the new `ModelInfo`, update `state.session_configuration.base_instructions` to `get_model_instructions(current_personality)` unless `config.base_instructions` is active,
- ensure subsequent turns and request builders read the rebased value from session state.

### 2. Remove model-switch instructions developer update

Target area:

- `codex-rs/core/src/context_manager/updates.rs`

Required change:

- `build_model_instructions_update_item(...)` should stop emitting a developer message for explicit live model changes,
- or the caller should stop consuming it for explicit live model changes,
- whichever produces the smallest, clearest ownership boundary.

The preferred outcome is that no model-switch full-prompt update is appended once base rebasing exists.

### 3. Preserve personality-only developer update semantics

Target area:

- `codex-rs/core/src/context_manager/updates.rs`

Required change:

- keep personality-only update handling separate from cross-model handling,
- do not regress the case where model stays constant but personality changes.

### 4. Keep resume semantics unchanged

Target area:

- `codex-rs/core/src/codex.rs`
- existing resume tests

Required change:

- ensure startup priority still prefers resumed session base instructions over current model instructions,
- ensure explicit model switch after resume rebases only from that point onward,
- do not auto-rebase base instructions during resume reconstruction itself,
- do not emit one-time full model guidance on resume when model differs from the historical session.

## Testing scope

### A. Session startup tests

Keep or update tests proving:

- startup still uses `config.base_instructions` first,
- resumed session base instructions still win over current model instructions,
- a fresh new session still uses the selected model instructions when no stronger override exists.

### B. Model-switch runtime tests

Add or update tests proving:

1. model switch updates the session base instructions when no config override exists,
2. subsequent request payloads use the new model instructions as `instructions`,
3. no extra developer update containing full model instructions is emitted for the model switch,
4. switching back to the original model rebases again cleanly,
5. a normal live-session `A -> B -> A` switch updates instructions each time without additive `<model_switch>` payloads.

### C. Override tests

Add or update tests proving:

1. `config.base_instructions` prevents session-base rebasing on model switch,
2. in that case, model switch does not silently override the explicit operator prompt,
3. pre-first-turn `OverrideTurnContext` model change rebases base instructions before the first request when no explicit base override exists.

### D. Resume-followed-by-switch tests

Add a regression test proving:

1. resume restores historical base instructions,
2. a later explicit model switch rebases the session base instructions from that moment onward,
3. later request payloads use the rebased instructions,
4. resume-with-different-model does not emit one-time full model guidance and does not rebase until an explicit live switch occurs.

### E. Compaction and full-context tests

Add tests proving:

1. local compaction no longer relies on `<model_switch>` restoration after explicit live switch rebasing,
2. remote compaction no longer relies on `<model_switch>` restoration after explicit live switch rebasing,
3. full-context reinjection paths after model switch still use rebased `instructions` and do not reintroduce stale full model-switch payloads.

## Rollout expectations

This MVP does not rewrite older rollout lines.

However, after the change:

- new sessions still start with `session_meta.base_instructions`,
- later requests after explicit model switch should reflect the rebased in-memory session base,
- `SessionMeta.base_instructions` remains the startup snapshot and is not rewritten by later model switches in this MVP,
- rollout readers should no longer need to infer a second full model prompt from developer updates for explicit live switch paths created after this change.

The historical `session_meta` at line 1 of an old rollout will still reflect the startup state of that session. That remains true and acceptable.

## Compaction invariant

Current compaction behavior has explicit knowledge of `<model_switch>` prompt material.

After this change:

- compaction correctness for explicit live-switch sessions must rely on rebased `instructions` state,
- not on stripping and restoring `<model_switch>` developer payloads,
- tests and expectations for local and remote compaction must be updated accordingly.

## Migration risks

This change must account for already-existing histories and test assumptions.

Known migration risks:

- legacy histories may already contain persisted `<model_switch>` developer messages,
- prompt-caching tests may currently assert the old additive behavior,
- compaction tests may currently assert stripping/restoring `<model_switch>` messages,
- resume tests may currently rely on one-time model guidance when startup model differs from historical state.

Expected test and behavior review surface:

- `model_switching`
- `resume`
- `prompt_caching`
- `compact`
- `compact_remote`
- any snapshots or request-shape assertions tied to model-switch developer updates

## Documentation updates

Update at minimum:

- internal model-switch behavior notes if they exist,
- any user-facing explanation of what model switching means in the fork,
- tests or comments that currently encode the old additive behavior as intentional.

## Decision summary

This spec deliberately chooses:

- logical cleanliness of explicit model switching,
- over cache-preserving additive prompt updates.

And it deliberately keeps:

- historical continuity for resume,
- explicit operator override priority for `config.base_instructions`.

Within this MVP, that separation applies to live process behavior. Durable post-switch resume/fork persistence is explicitly deferred because it requires additional rollout/state design beyond this patch.
