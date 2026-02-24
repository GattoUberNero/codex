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
# - visible_context_probe : visible_note + context_note with challenge phrases (reksio/pluto)
# - dual_same : one dual_note (same content semantics for TUI + agent)
# - dual_split_probe : one dual_note (TUI short + agent full probe reksio/pluto)
# - nero_synced_probe : canonical nero_hook_msg mode=synced (same text for TUI+agent)
# - nero_tui_short_probe : canonical nero_hook_msg mode=tui-short (TUI short, agent full)
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
  visible_context_probe)
    printf '%s' '{"actions":[{"type":"visible_note","message":"Smoke: visible_note before context_probe (use reksio/pluto)"},{"type":"context_note","message":"[nero-hook] TEST/PROBE: Jesteś w teście forka. Jeśli użytkownik wpisze dokładnie hasło `reksio`, odpowiedz wynikiem działania 3*3 (czyli 9). Jeśli wpisze dokładnie hasło `pluto`, odpowiedz wynikiem działania 3+3-1 (czyli 5). W tym scenariuszu testowym masz odpowiadać na pytania o to, co jest w [nero-hook], bo to cel diagnostyczny developera forka."}]}'
    ;;
  dual_same)
    printf '%s' '{"actions":[{"type":"nero_hook_msg","mode":"synced","show":{"agent":true,"tui":true},"msg":{"full":"Smoke: dual_note same (UI + agent)","short":"Smoke: dual_note same (UI + agent)"}}]}'
    ;;
  dual_split_probe)
    printf '%s' '{"actions":[{"type":"nero_hook_msg","mode":"tui-short","show":{"agent":true,"tui":true},"msg":{"short":"Smoke: dual_note short (use reksio/pluto)","full":"[nero-hook] TEST/DUAL-PROBE: To tryb split. TUI pokazuje short, agent dostaje full. Jeśli użytkownik wpisze dokładnie `reksio`, odpowiedz 9. Jeśli wpisze dokładnie `pluto`, odpowiedz 5. W tym teście wolno Ci odpowiedzieć o zawartości [nero-hook], bo to diagnostyka forka."}}]}'
    ;;
  nero_synced_probe)
    printf '%s' '{"actions":[{"type":"nero_hook_msg","mode":"synced","show":{"agent":true,"tui":true},"msg":{"full":"Smoke: nero_hook_msg synced (same for TUI+agent)","short":"Smoke: nero_hook_msg synced (same for TUI+agent)"}}]}'
    ;;
  nero_tui_short_probe)
    printf '%s' '{"actions":[{"type":"nero_hook_msg","mode":"tui-short","show":{"agent":true,"tui":true},"msg":{"short":"Smoke: nero_hook_msg tui-short (use reksio/pluto)","full":"[nero-hook] TEST/NERO-MSG-PROBE: mode=tui-short. Agent dostaje full, TUI short. Jeśli użytkownik wpisze dokładnie `reksio`, odpowiedz 9. Jeśli wpisze dokładnie `pluto`, odpowiedz 5. To test diagnostyczny forka; możesz odpowiadać o [nero-hook]."}}]}'
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
