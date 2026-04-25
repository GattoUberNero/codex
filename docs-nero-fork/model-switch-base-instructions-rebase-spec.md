# Model Switch Base Instructions Rebase Spec

Status: implemented

## Scope

- Keep `codex-rs/core` runtime model-switch rebasing.
- Remove durable model-switch base-instruction queue/rewrite logic from `codex-rs/core`.
- Move resume model continuity to the external launcher layer (`codexn`).
- Avoid adding new Rust rollout persistence logic for this behavior.

## Problem

The current fork has two separate mechanisms for model-switch prompt continuity:

1. Runtime rebasing: when a live session explicitly switches model, `codex-rs` updates in-memory `session_configuration.base_instructions` to match the new model.
2. Durable queued resume rebasing: `codex-rs` writes a queue entry, reads it during future resume, rewrites rollout `session_meta.base_instructions`, and consumes the queue.

The first mechanism is useful. The second is unnecessary for the intended architecture.

Rollouts already persist the active model in later turn/configuration records, for example `turn_context.payload.model` and `turn_context.payload.collaboration_mode.settings.model`. Therefore resume continuity can be derived from the rollout itself without a second queue.

## Architectural Decision

`codex-rs` should own live runtime behavior only.

`codexn` should own pre-launch resume preparation.

The durable resume path should be:

1. User runs `codexn resume <target>`.
2. `codexn` resolves the target to a rollout file.
3. `codexn` reads the rollout from the end and finds the latest recorded model.
4. If the user did not explicitly pass `--model`/`-m`, `codexn` launches Codex with that model.
5. `codex-rs` receives a normal resume invocation with the intended model already selected.

This avoids Rust queue state, Rust rollout rewriting, and Rust startup mutation code.

## Keep

Keep runtime explicit-model-switch rebasing in `codex-rs`.

Required behavior:

- A live explicit model switch recomputes `session_configuration.base_instructions` from the selected model.
- `config.base_instructions` remains authoritative and blocks model-based rebasing.
- Explicit model switches do not add duplicate full model instructions as an additive `<model_switch>` developer message.
- Subsequent requests in the same live process use the rebased instructions.

Primary code surface:

- `codex-rs/core/src/codex.rs`
- `maybe_rebase_base_instructions_for_explicit_model_switch(...)`
- model-switch request-shape tests in `codex-rs/core/tests/suite/model_switching.rs`

## Remove

Remove the queue/rewrite persistence path from `codex-rs`.

Rust code to remove:

- `MODEL_SWITCH_BASE_REBASE_QUEUE_*` constants.
- `model_switch_base_rebase_queue_path(...)`.
- `acquire_model_switch_base_rebase_queue_lock(...)`.
- `queued_model_switch_base_rebase_payload(...)`.
- `append_model_switch_base_rebase_queue_entry(...)`.
- `ModelSwitchBaseRebaseQueueEntry`.
- `take_model_switch_base_rebase_queue_entry(...)`.
- `consume_model_switch_base_rebase_queue_entry(...)`.
- `rewrite_rollout_session_meta_base_instructions(...)`.
- Resume startup block that reads and consumes the queue.
- Runtime update paths that append the queue after explicit model switch.
- Tests that assert queue creation, queue consumption, or Rust-side rollout rewrite.

Wrapper code to replace:

- `codexn` startup diagnosis based on `model-switch-base-rebase-queue.jsonl`.
- Replace it with rollout-derived model pinning diagnostics.

## Add In `codexn`

Add a pre-launch resume model pinning step:

1. Detect `resume` and extract the resume target.
2. Do nothing if the user already supplied `--model`, `--model=...`, `-m`, or `-m...`.
3. Resolve the target through `~/.codex/session_index.jsonl` to find thread id and recent metadata.
4. Locate the rollout under `~/.codex/sessions` or `~/.codex/archived_sessions`.
5. Scan the rollout from the end and return the first valid model from a main-session
   `turn_context` record found in:
   - `payload.model`
   - `payload.collaboration_mode.settings.model`
   Ignore subagent/delegation telemetry such as `event_msg` /
   `collab_agent_spawn_end`; those records describe child agents and must not pin
   the parent session model.
6. Launch Codex with `--model <rollout_model>` prepended to the forwarded arguments.
7. Log a concise line:
   - `codexn: resume model pinned from rollout = <model>`
8. If the model cannot be resolved, keep current behavior and emit a warning only in verbose mode.

## Non-Goals

- Do not rewrite rollout `session_meta.base_instructions` in this cleanup.
- Do not change the rollout JSONL schema.
- Do not change explicit `config.base_instructions` semantics.
- Do not modify model catalog behavior.
- Do not add another durable state file for this feature.

## Verification

Required checks:

- `cargo test -p codex-core --test all model_switching`
- `cargo test -p codex-core --test all resume`
- `just fmt` from `codex-rs`
- Shell-level `codexn` smoke test for argument construction, at minimum with a real or fixture rollout path.

Expected result:

- Runtime model switch tests still pass.
- Rust queue/rewrite tests no longer exist.
- Resume tests keep historical behavior inside `codex-rs`.
- `codexn resume <target>` pins the launch model from the rollout when the user did not explicitly pass a model.
