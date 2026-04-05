# Functional Architecture

## 1. Purpose

`codex-nero` is not only a forked Codex CLI binary. It is a three-repo operating system for:

- controlled execution of Codex threads and tools,
- policy-aware hook composition,
- per-session runtime control,
- operator lifecycle management,
- and selected-session maintenance workflows such as rollout trim.

The system is composed of:

- `codex-nero`
- `codex-nero-sdk`
- `nerobar-ui`

The functional split is:

- `codex-nero` executes,
- `codex-nero-sdk` decides and composes,
- `nerobar-ui` observes, controls, and supervises.

## 2. Capability Map

| Capability | Outcome | Primary owner | Current implementation method |
| --- | --- | --- | --- |
| Thread and tool execution | Run turns, tools, sessions, resume, fork, subagents | `codex-nero` | Rust core/TUI/app-server runtime |
| Hook interception | Observe session, prompt, tool, and stop lifecycle | `codex-nero` | native upstream hook engine and core hook runtime |
| Native hook execution and outcomes | Match native hook handlers, execute them, and enforce block/stop/context semantics | `codex-nero` | upstream hook engine, event handlers, dispatcher, and hook run summaries |
| Nero overlay hook action composition | Decide which Nero-specific visible/runtime actions should happen through the legacy notify compatibility path | `codex-nero-sdk` | Python router + organizer runtime returning `actions[]` |
| Hook-visible messaging | Show hook status to operator and/or inject hook guidance to agent | `codex-nero` + `codex-nero-sdk` | `nero_hook_msg` action parsed in Rust, composed in Python |
| Session auto runtime policy | Control auto mode, round budget, score thresholds, and auto reply gating | `codex-nero-sdk` + `codex-nero` | runtime bridge commands + core runtime state |
| Session runtime supervision | Inspect and mutate current per-session auto settings | `nerobar-ui` | backend spawns SDK bridge commands, frontend exposes controls |
| Session lifecycle classification | Separate registered, live, stale, orphaned, and forgotten local artifacts | `nerobar-ui` | app-server inventory + local overlays + registry-backed classification |
| Selected-session rollout trim | Analyze, trim, backup, restore, delete backup for one important session | `codex-nero` + `nerobar-ui` | app-server `thread/rollout/*` APIs plus UI/background job orchestration |
| Hook/operator observability | Show hook runs, delivery behavior, runtime status, and debug info | `codex-nero` + `nerobar-ui` | hook notifications, TUI render, backend read models, UI views |
| Config and account control | Manage config overlays and Codex auth slots/accounts | `nerobar-ui` | Node backend + frontend operator views + launcher utilities |
| Extension surface growth | Add capabilities without deep fork edits when possible | upstream Codex first, then fork | native hooks, plugins, apps, app-server RPCs, then custom overlay only if needed |

## 3. Functional Roles by System

## 3.1 `codex-nero`

Functional role:

- the execution engine,
- the source of truth for live thread/tool lifecycle,
- the place where hook events happen,
- the place where hook actions are finally applied,
- the place where selected-session rollout trim is enforced safely.

It owns:

- thread and turn execution,
- tool invocation,
- hook runtime invocation,
- TUI rendering of hook and session state,
- app-server request handling,
- fork-specific runtime/session fields exposed over protocol,
- rollout analyze/trim/backup/restore/delete execution.

It does not own:

- high-level organizer policy,
- multi-repo campaign/business reminder composition,
- Node-side lifecycle classification overlays,
- operator dashboard UX.

## 3.2 `codex-nero-sdk`

Functional role:

- the policy and orchestration adapter layer that is intentionally kept outside Rust rebuild cycles.

It owns:

- parsing legacy notify payloads,
- normalizing hook input,
- composing `nero_hook_msg` and `auto_user_reply`,
- resolving runtime policy from overlays and environment,
- campaign-aware message construction,
- runtime bridge commands such as `read-session-auto` and `apply-session-auto`.

It does not own:

- final hook execution semantics,
- TUI rendering,
- app-server authority over threads,
- Node operator read-model decisions.

## 3.3 `nerobar-ui`

Functional role:

- the operator control plane over Codex runtime, hooks, sessions, config, and lifecycle.

It owns:

- account and slot operations,
- config editing overlays,
- session pane and lifecycle workflows,
- runtime bridge orchestration into SDK helper modules,
- background job wrappers around heavy app-server operations,
- local operator overlays such as session registry and forgotten/orphaned artifacts.

It does not own:

- reimplementing hook policy decisions,
- reimplementing rollout trim logic,
- mutating Codex internals directly when a real app-server/SDK interface exists.

## 4. End-to-End Functional Flows

## 4.1 Turn Completion -> Hook Composition -> Visible Delivery

Goal:

- turn runtime/session context into visible hook guidance and optional auto continuation.

Flow:

1. `codex-nero` executes a turn.
2. Hook runtime emits hook events and legacy notify compatibility when configured.
3. `codex-nero-sdk` router receives the notify payload.
4. Organizer runtime resolves config, runtime policy, campaign/session context, and desired outputs.
5. Router writes canonical `actions[]` JSON to stdout.
6. `codex-nero` parses and applies actions.
7. TUI and/or agent context receives the resulting message.

Important behavior:

- `nero_hook_msg` is the main delivery contract for visible/operator and agent-visible runtime messages.
- `auto_user_reply` is the main continuation contract for synthetic follow-up input.
- delivery can be suppressed or gated for subagents and policy reasons.
- native upstream hook outcomes such as block/stop/context decisions are already composed inside the Rust hook engine and are not owned by the SDK organizer.

## 4.2 Session Auto Runtime Control

Goal:

- let the operator inspect and change auto runtime state without directly mutating Rust internals from the UI.

Flow:

1. Operator opens session controls in `nerobar-ui`.
2. Backend spawns SDK runtime-control helper.
3. SDK resolves effective runtime state from overlay config and runtime sources.
4. Backend returns normalized status to the frontend.
5. Operator changes runtime state.
6. Backend calls the SDK apply command.
7. SDK performs controlled update.
8. `codex-nero` receives the resulting runtime settings through its existing runtime bridge flow.

Important behavior:

- Node does not duplicate runtime policy logic.
- Python remains the policy/config translation layer.
- Rust remains the session runtime executor.

## 4.3 Session Lifecycle Management

Goal:

- distinguish truly available sessions from historical local artifacts and let the operator clean them up safely.

Flow:

1. `nerobar-ui` backend reads app-server thread inventory.
2. Backend merges that with operator registry and local overlays.
3. Sessions are classified into buckets such as registered, available/live, and orphaned.
4. UI shows the buckets in the session rail.
5. Operator may register, unregister, or forget local orphaned entries.

Important behavior:

- missing rollout or stale historical metadata does not automatically mean a live session.
- local lifecycle cleanup is intentionally local; it does not rewrite upstream Codex history just to hide a dead artifact from the UI.

## 4.4 Selected-Session Rollout Trim

Goal:

- safely reduce the size of one important persisted session without blindly editing raw JSONL in place.

Flow:

1. Operator selects one session.
2. UI fetches a quick summary first.
3. UI starts exact analyze as a background job for large rollouts.
4. `codex-nero` app-server computes:
   - eligibility,
   - protected head,
   - safe tail window,
   - trim fingerprint,
   - backup context.
5. Operator confirms trim.
6. UI starts trim as a background job.
7. `codex-nero` creates backup and atomically replaces the rollout with the trimmed file.
8. Operator reopens the session.
9. Operator can mark reopen verified, restore backup, or delete backup.

Important behavior:

- trim is for selected main sessions, not a bulk cleanup tool,
- heavy analysis and trim are job-based in `nerobar-ui` to avoid web timeout failure,
- safety lives in app-server and file-level validation, not only in UI gating.

## 4.5 Generic Upstream Plugins and Apps

Goal:

- use official extension surfaces where possible instead of adding more fork-only runtime patches.

Flow:

1. Codex plugin manager resolves installed and available plugins.
2. Plugin/app capabilities become available to the runtime and app-server.
3. TUI/app-server can list and manage plugins.
4. Fork architecture can decide whether a new behavior belongs in:
   - native plugin/app surface,
   - hook interception surface,
   - or only as a fork-specific runtime overlay.

Important behavior:

- plugins are not only UX features; they are a strategic escape hatch from deep forking.

## 5. Capability Boundaries and Ownership Rules

These rules keep the system coherent.

### Rule A: execution vs policy

- Rust executes.
- Python composes policy.
- Node supervises operators and lifecycle.

### Rule B: native first

When upstream already provides a native lifecycle surface, prefer:

- hooks,
- plugins,
- apps,
- app-server RPCs,

before adding another fork-only side contract.

### Rule C: one place for each type of truth

- live thread/runtime truth: `codex-nero`
- runtime policy composition: `codex-nero-sdk`
- operator overlays and UI workflow state: `nerobar-ui`

### Rule D: compatibility layers are not destination architecture

The legacy `notify -> actions[]` path still works and is useful, but it should be treated as a compatibility bridge while native hook/event/plugin/app-server surfaces grow stronger.

## 6. Upstream-Native vs Fork-Specific Functionality

Already upstream-native:

- hook lifecycle events:
  - `SessionStart`
  - `Stop`
  - `UserPromptSubmit`
  - `PreToolUse`
  - `PostToolUse`
- hook run summaries and notifications over app-server/TUI
- plugin/app loading and management
- app-server thread and config RPC model
- app-server TUI as the normal runtime path

Still fork-specific:

- `nero_hook_msg`
- `auto_user_reply` contract as currently used by the organizer
- runtime session-auto bridge semantics
- session lifecycle overlays such as orphaned/forgotten local artifacts
- selected-session rollout trim lifecycle and UI workflow
- operator-facing Codex/Nero runtime dashboards

## 7. Concrete Improvement Directions

The current functional architecture suggests these improvements:

1. Move more hook-visible status from custom side channels into native hook-run summaries and hook notifications.
2. Move runtime mutation/read authority toward explicit app-server session APIs instead of TUI-local or bridge-only semantics where possible.
3. Use plugin/app surfaces for productized extensions that should survive upstream upgrades more cleanly.
4. Keep `codex-nero-sdk` for fast-changing organizer policy, but shrink its transport-compat burden over time.
5. Keep `nerobar-ui` as operator plane, not as a place that reimplements execution or trim policy.
