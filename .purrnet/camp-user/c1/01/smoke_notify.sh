#!/usr/bin/env bash
set -euo pipefail

# Legacy notify hook smoke harness for Codex-Nero PoC.
# Codex appends a JSON payload as the last argv item.
#
# Modes (set via NERO_HOOK_SMOKE_MODE):
# - visible  : emits visible_note action
# - context  : emits context_note action (developer/model-visible note)
# - auto     : emits auto_user_reply action
# - both     : emits visible_note then auto_user_reply
# - visible_context : emits visible_note then context_note
# - garbage  : emits invalid JSON (for parser/fail-open smoke)
# - legacy   : emits plain stdout text (legacy compat path)
#
# Optional envs:
# - NERO_HOOK_SMOKE_LOG : path to append raw payload + metadata (default: /tmp/codex-nero-hook-smoke.log)

mode="${NERO_HOOK_SMOKE_MODE:-visible}"
payload="${*: -1}"
log_file="${NERO_HOOK_SMOKE_LOG:-/tmp/codex-nero-hook-smoke.log}"
ts="$(date -Is)"

mkdir -p "$(dirname "$log_file")"
{
  echo "[$ts] mode=$mode"
  echo "$payload"
  echo
} >> "$log_file"

case "$mode" in
  visible)
    printf '%s' '{"actions":[{"type":"visible_note","message":"Smoke: visible_note OK"}]}'
    ;;
  context)
    printf '%s' '{"actions":[{"type":"context_note","message":"Smoke: context_note OK (developer history)"}]}'
    ;;
  auto)
    printf '%s' '{"actions":[{"type":"auto_user_reply","message":"Oceń poziom niepewności 1-10. Jeśli <= 6 i masz plan dalszych kroków, kontynuuj samodzielnie."}]}'
    ;;
  both)
    printf '%s' '{"actions":[{"type":"visible_note","message":"Smoke: visible_note before auto_user_reply"},{"type":"auto_user_reply","message":"Kontynuuj kolejny krok z planu. Najpierw wskaż niepewność 1-10; jeśli <= 6, działaj dalej."}]}'
    ;;
  visible_context)
    printf '%s' '{"actions":[{"type":"visible_note","message":"Smoke: visible_note before context_note"},{"type":"context_note","message":"Smoke: context_note after visible_note"}]}'
    ;;
  garbage)
    printf '%s' '{not-json'
    ;;
  legacy)
    printf '%s' 'legacy-notifier-ok'
    ;;
  *)
    printf '%s\n' "unknown NERO_HOOK_SMOKE_MODE=$mode" >&2
    exit 2
    ;;
esac
