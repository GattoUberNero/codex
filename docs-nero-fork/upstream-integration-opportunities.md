# Upstream Integration Opportunities

## 1. Why This Matters

The main architectural question is not "how do we preserve every fork patch forever?"

It is:

- which behaviors should remain custom because they are truly product-specific,
- and which behaviors should move toward official upstream hooks/plugins/app-server surfaces to reduce upgrade cost.

This document maps that boundary.

## 2. Native Upstream Surfaces Already Available

These surfaces now exist in upstream Codex and should be treated as preferred extension points.

### 2.1 Native hook lifecycle

Available native events:

- `SessionStart`
- `Stop`
- `UserPromptSubmit`
- `PreToolUse`
- `PostToolUse`

Why this matters:

- many runtime interventions no longer need to piggyback only on the old legacy notify path,
- hook runs have typed input/output contracts,
- hook run summaries can travel through app-server/TUI.

### 2.2 App-server as normal execution surface

Upstream TUI now lives on app-server by default.

Why this matters:

- runtime/session supervision should prefer explicit app-server contracts over TUI-only side flows,
- operator tooling such as `nerobar-ui` can lean more on stable RPC/event surfaces.

### 2.3 Plugins and apps

Plugin management is now first-class:

- list
- read
- install
- uninstall
- product-scoped capability injection

Why this matters:

- some future extension logic belongs in plugins/apps rather than in permanent fork patches.

## 3. Surfaces That Should Probably Stay Custom

These behaviors look intentionally product-specific and can stay outside pure upstream Codex.

### 3.1 Organizer policy and campaign-aware runtime composition

Examples:

- custom `NERO-SYSTEM` content,
- campaign-aware reminders,
- custom auto policy scoring and gates,
- domain-specific operator messaging.

Reason:

- this is product policy, not general Codex runtime behavior.

Best home:

- `codex-nero-sdk`

### 3.2 Operator lifecycle overlays

Examples:

- registered session overlay,
- orphaned/forgotten local artifacts,
- operator cleanup and session-pane lifecycle states.

Reason:

- these are control-plane/operator concerns, not core Codex execution semantics.

Best home:

- `nerobar-ui`

### 3.3 Selected-session rollout trim workflow UX

Reason:

- the low-level safe trim execution belongs in `codex-nero`,
- but the operator workflow, job monitoring, reopen verification, and backup lifecycle UX remain product-specific.

Best split:

- execution in app-server/Rust,
- workflow in `nerobar-ui`.

## 4. Surfaces That Should Move Toward Native Upstream Mechanisms

## 4.1 Hook-visible status delivery

Today:

- custom `nero_hook_msg` action is the main visible status bridge.

Opportunity:

- represent more hook results through native hook run summaries and hook notifications,
- keep `nero_hook_msg` only for behavior that truly needs dual delivery semantics.

Expected benefit:

- less custom parsing/execution glue,
- cleaner TUI/app-server visibility,
- easier upgrade compatibility.

## 4.2 Runtime control authority

Today:

- session auto control uses Python bridge commands and TUI/runtime-specific overlays.

Opportunity:

- move runtime read/update toward explicit app-server session/runtime RPCs.

Expected benefit:

- one authority path,
- cleaner UI/backend integration,
- less TUI-local state coupling.

## 4.3 Extension packaging

Today:

- many useful behaviors are expressed as fork/runtime patches.

Opportunity:

- decide whether a new capability should be:
  - plugin/app,
  - hook policy,
  - or fork core change.

Expected benefit:

- smaller fork delta,
- better upgrade survivability.

## 5. Surfaces That Need Transitional Compatibility

Some parts cannot jump straight to a pure native design.

### 5.1 Legacy `notify -> actions[]`

Keep for now because:

- the SDK organizer already depends on it,
- it provides a stable Python process boundary,
- it still carries important operator policy.

But treat it as:

- compatibility transport,
- not the final destination architecture.

### 5.2 TUI-visible Nero runtime affordances

Keep for now because:

- operator usability matters,
- the fork already carries session/runtime cues in TUI state.

But gradually reduce:

- direct TUI-side ownership of runtime authority,
- stale custom state that can instead come from app-server session contracts.

## 6. Recommended Migration Sequence

### Step 1: preserve behavior while reducing coupling

- keep current SDK organizer outputs,
- expose more structured hook/session/runtime state through app-server,
- shrink custom ad hoc side channels.

### Step 2: move supervision to native channels

- prefer app-server session/runtime APIs,
- prefer hook notifications/run summaries,
- keep Node as orchestrator only.

### Step 3: productize extension behavior where possible

- evaluate plugin/app packaging for capabilities that should survive upstream upgrades with less merge pain.

### Step 4: minimize fork-only execution patches

- keep fork patches for:
  - execution safety,
  - runtime semantics,
  - selected-session maintenance,
  - truly custom operator/runtime affordances.

## 6.1 Current packaging pilot outcome

The first low-risk packaging pilot is a repo-local plugin:

- `nero-context-pack`

Why this pilot exists:

- it packages read-only Nero context guidance as plugin-bundled skills,
- it exercises the upstream-native plugin/marketplace shape without moving runtime authority or operator lifecycle into plugin form,
- it lowers future merge pain for guidance/context behavior that does not need to stay in fork-only execution paths.

Why this is intentionally limited:

- it is a discovery/read-oriented pilot, not an install/auth-heavy rollout,
- it does not attempt to move multi-account runtime switching, `session-auto` policy authority, rollout trim, or session lifecycle overlays into plugin form,
- those surfaces remain custom until there is a clearly safer native fit.

## 7. Decision Rules for Future Changes

When adding a new behavior, ask in this order:

1. Can this be done as a native plugin/app?
2. Can this be done as native hook processing without new custom action semantics?
3. Can this be done as an app-server/session/runtime contract?
4. Only if all three answers are no: should this become a new fork-only patch?

That rule is the main architectural guardrail for keeping future upgrades tractable.
