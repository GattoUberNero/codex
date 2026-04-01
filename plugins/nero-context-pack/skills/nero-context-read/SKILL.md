---
name: nero-context-read
description: Read-only helper for understanding active campaign, runtime, and ownership boundaries in the codex-nero ecosystem. Use when you need to orient work before changing code, especially to classify whether a concern belongs in codex-nero, codex-nero-sdk, or nerobar-ui.
---

# Nero Context Read

## When to use

- You need a concise read of the active campaign and phase before implementation.
- You need to classify whether a behavior belongs in:
  - `codex-nero`
  - `codex-nero-sdk`
  - `nerobar-ui`
- You need a read-only explanation of runtime/session context before deciding where to work.

## What to do

1. Read the active campaign and current phase from the campaign store for the current repo root.
2. Read the relevant runtime/session surface only from authoritative sources already present in the repo or app-server contracts.
3. Classify the concern into one of these buckets:
   - `codex-nero`: runtime semantics, app-server contracts, hook-visible native delivery, execution-safe maintenance
   - `codex-nero-sdk`: organizer policy, campaign-aware reminder/scoring composition, hidden single-writer/runtime composition that still intentionally remains custom
   - `nerobar-ui`: operator overlays, multi-account operations, session lifecycle UX, supervision presentation
4. Return:
   - active campaign + phase
   - authoritative sources used
   - ownership classification
   - any boundary warning if the request would create dual authority or reintroduce stale bridge semantics

## Boundaries

- Read-only skill: do not mutate campaign, runtime, or operator state just because this skill is active.
- Do not guess missing authority from fallback files or stale UI hints when a native app-server source exists.
- Do not route multi-account runtime switching into plugin/app logic.
- Do not treat rollout trim, orphan lifecycle overlays, or session-auto policy decisions as plugin-native responsibilities.

## Expected result

The user gets a short, reliable orientation for where a Nero concern belongs and what authoritative context should drive the next implementation step.
