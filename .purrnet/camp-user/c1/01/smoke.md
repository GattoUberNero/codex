# Gate 5 Smoke: Hook Actions (`after_agent`) for Codex-Nero

## Purpose

Manual smoke for PoC hook actions:
- `visible_note`
- `auto_user_reply`
- both in one hook response
- invalid JSON / legacy stdout compatibility

This uses the existing `notify = [...]` hook path (legacy notify argv) so it exercises the real `after_agent` hook pipeline in Codex-Nero.

## Files

- Hook harness: `.purrnet/camp-user/c1/01/smoke_notify.sh`
- Optional payload log: `/tmp/codex-nero-hook-smoke.log` (default)

## One-time setup

```bash
chmod +x /workspace/purrnet/apps/codex-nero/.purrnet/camp-user/c1/01/smoke_notify.sh
```

## Config (Codex / CodexN)

Add to `~/.codex/config.toml` (temporary smoke setting):

```toml
notify = ["/workspace/purrnet/apps/codex-nero/.purrnet/camp-user/c1/01/smoke_notify.sh"]
```

Notes:
- `codexn` uses the same `~/.codex` config as upstream `codex` on purpose.
- Codex appends one extra JSON arg automatically.

## Smoke Modes

Set mode per terminal before launching `codexn`:

```bash
export NERO_HOOK_SMOKE_MODE=visible   # visible | auto | both | garbage | legacy
export NERO_HOOK_SMOKE_LOG=/tmp/codex-nero-hook-smoke.log
```

## Scenarios

### 1. `visible_note` only

```bash
export NERO_HOOK_SMOKE_MODE=visible
codexn --dev
```

In Codex:
- send a simple prompt (e.g. "say hi")
- wait for turn completion

Expected:
- visible warning entry with prefix `[nero-hook]`
- text contains `Smoke: visible_note OK`
- `/tmp/codex-nero-hook-smoke.log` contains raw payload JSON appended by Codex

### 2. `auto_user_reply` only

```bash
export NERO_HOOK_SMOKE_MODE=auto
codexn --dev
```

In Codex:
- send a prompt that naturally produces a completed turn

Expected:
- after turn completion, synthetic next user input may be enqueued (best-effort)
- if another manual turn races first, synthetic input may be skipped (by design for safety)
- logs should mention synthetic submission / skip path (`hook-auto-*`)

### 3. Both actions in one response (ordering UX)

```bash
export NERO_HOOK_SMOKE_MODE=both
codexn --dev
```

Expected:
- visible note appears before any synthetic continuation behavior
- note text: `Smoke: visible_note before auto_user_reply`

### 4. Garbage JSON (parser robustness)

```bash
export NERO_HOOK_SMOKE_MODE=garbage
codexn --dev
```

Expected:
- no process crash
- hook failure should be handled by current `FailedContinue` path
- turn completion should continue (subject to current failure semantics)

### 5. Legacy plain stdout compat

```bash
export NERO_HOOK_SMOKE_MODE=legacy
codexn --dev
```

Expected:
- plain stdout text is ignored for actions
- no crash
- no synthetic action

## Quick Cleanup

Remove temporary hook config after smoke:

1. delete/comment `notify = [...]` from `~/.codex/config.toml`
2. `unset NERO_HOOK_SMOKE_MODE`

## Notes for UX Evaluation

- Current `visible_note` renders via `WarningEvent` with `[nero-hook]` prefix (MVP path).
- Current `auto_user_reply` is intentionally safety-biased:
  - best-effort,
  - may skip on race with manual input,
  - guarded by chain-depth and synthetic submission marker.

