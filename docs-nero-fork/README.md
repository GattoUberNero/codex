# Codex Nero Fork Architecture

This directory documents the current architecture of the `codex-nero` ecosystem after the
upstream upgrade lane to `rust-v0.118.0`.

Scope:

- `codex-nero`: the Rust fork that owns execution, hooks, TUI, app-server, and selected
  fork-specific protocol/runtime extensions.
- `codex-nero-sdk`: the Python runtime/policy layer that adapts hook payloads, composes
  organizer behavior, and exposes runtime bridge commands.
- `nerobar-ui`: the operator control plane that wraps Codex app-server APIs and the SDK
  runtime bridge into session, lifecycle, config, and account workflows.

The documents are intentionally split by viewpoint:

- `functional-architecture.md`
  - what the system does
  - which capabilities exist
  - which component owns which behavior
  - which end-to-end operator/runtime flows matter
- `structural-architecture.md`
  - how the system is physically organized
  - repo/module topology
  - runtime processes
  - control/data channels
  - state/config/log storage boundaries
- `upstream-integration-opportunities.md`
  - which surfaces are already native upstream Codex
  - which surfaces remain fork-specific
  - what can realistically migrate toward official hooks/plugins/app-server APIs

Reading order:

1. Start with `functional-architecture.md` if the question is "what capability lives where?"
2. Read `structural-architecture.md` if the question is "which repo/module/process implements it?"
3. Read `upstream-integration-opportunities.md` if the question is "what should stay custom and what should move toward upstream-native extension points?"

Working assumptions captured here:

- preserve operator-facing behavior when possible,
- prefer upstream-native hooks/plugins/app-server contracts over fork-only patches,
- keep `codex-nero-sdk` as the fast-changing policy/orchestration layer,
- keep `nerobar-ui` as the operator plane instead of duplicating policy logic in Node,
- treat the current legacy `notify -> actions[]` path as a compatibility layer, not the final design target.
