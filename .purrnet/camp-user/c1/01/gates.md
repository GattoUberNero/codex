# Gates: Codex-Nero Hook PoC (Plan c1/01)

## Gate 0: Scope Freeze (must pass before coding)

- [x] PoC scope ograniczony do `after_agent`
- [x] Tylko 2 akcje: `visible_note`, `auto_user_reply`
- [x] Brak pełnej orkiestracji MCP w tym etapie
- [x] Brak nowego eventu protokołu (na start używamy `WarningEvent`)

## Gate 1: Hook Response Contract

- [x] Istnieje jawny typ odpowiedzi hooka (MVP: opcjonalne `actions[]`)
- [x] Parser stdout hooka obsługuje:
  - [x] poprawny JSON
  - [x] pusty stdout
  - [x] niepoprawny JSON (bez crasha)
  - [x] nieznane typy akcji (ignore + log)
- [x] Testy jednostkowe parsera przechodzą

## Gate 2: Visible Note (punkt 1)

- [x] `after_agent` potrafi wykonać akcję `visible_note`
- [x] Akcja emituje widoczny event (`WarningEvent`)
- [x] Wpis jest widoczny w TUI/exec
- [x] Wpis jest persistowany w rollout przez standardowy pipeline eventów
- [x] Prefix/oznaczenie (`[nero-hook]` lub równoważne) odróżnia wpis hooka od zwykłych warningów

## Gate 3: Auto User Reply (punkt 2)

- [x] `after_agent` potrafi wykonać akcję `auto_user_reply`
- [x] Auto-reply jest enqueue jako `Op::UserInput` (synthetic user)
- [ ] Startuje kolejny turn bez ręcznego inputu (best-effort; przy wyścigu z manualnym inputem może być celowo pominięty)
- [ ] Brak crasha / deadlocka przy auto-reply
- [x] Log/telemetria pozwala stwierdzić, że turn był synthetic (min. debug log)

## Gate 4: Safety / Determinism

- [x] Jest zabezpieczenie przed pętlą auto-reply (min. jedno z poniższych):
  - [x] chain depth limit
  - [x] skip marker dla synthetic turnu
  - [ ] feature flag default OFF
- [ ] Hook failure w trybie continue nie psuje zakończenia turnu
- [ ] Hook failure w trybie abort nadal działa zgodnie z obecnym kontraktem
- [x] Kolejność wykonania akcji jest deterministyczna (`visible_note` przed `auto_user_reply`) w obecnym `after_agent` flow (auto-reply defer po batchu hooków)
- [x] `nero_hook_msg.freq` (optional, default `0`) wspiera throttling reminderów/statusów z jawnym komunikatem TUI przy skipie (countdown)
- [x] `nero_hook_msg` obsługuje opcjonalne `format` + `status` (wstecznie kompatybilnie)
- [x] Domyślny render TUI `nero_hook_msg` używa structured block (`content` + opcjonalny `status`)
- [x] Skip przez `freq` emituje countdown jako semantykę statusu TUI (nie tylko raw warning string)
- [x] Reset throttlingu `nero_hook_msg.freq` po compaction (po udanym zapisie `RolloutItem::Compacted`)

## Gate 5: Manual Smoke (MVP)

Harness / checklist prepared:
- `./.purrnet/camp-user/c1/01/smoke_notify.sh`
- `./.purrnet/camp-user/c1/01/smoke.md`
- `./.purrnet/camp-user/c1/01/user-tests.md` (AUTO e2e + MANUAL UX campaign sheet)

Automated e2e baseline prepared:
- `codex-rs/core/tests/suite/hook_actions_notify.rs` (basic user flows via `notify = [...]`)
- Note: tests soft-skip when `codex-linux-sandbox` binary is not built in local test env.

AUTO campaign execution status:
- [x] e2e run executed locally without soft-skip (`codex-linux-sandbox` present)
- [x] e2e results reviewed and mapped to manual UX campaign follow-up

Latest AUTO e2e result snapshot (local):
- `hook_actions_notify::after_agent_legacy_plain_stdout_keeps_normal_flow` ✅
- `hook_actions_notify::after_agent_garbage_json_does_not_crash_turn_flow` ✅
- `hook_actions_notify::after_agent_visible_note_emits_warning_and_turn_completes` ✅
- `hook_actions_notify::after_agent_both_actions_can_trigger_follow_up_turn_without_manual_input` ✅
- `hook_actions_notify::after_agent_auto_user_reply_only_can_trigger_follow_up_turn_without_manual_input` ✅
- `hook_actions_notify::after_agent_nero_hook_msg_block_status_emits_structured_warning_and_turn_completes` ✅ (canonical `nero_hook_msg` + `format/status`)
- Follow-up narrowing: `codex-hooks::user_notification::notify_hook_parses_actions_from_stdout` ✅

Notes from debug run:
- e2e false negatives were caused by mounting SSE fixtures on a different mock server than `TestCodexHarness` (test harness bug, not core hook bug).
- Added opt-in `debug` logs across hook handoff points (`codex_hooks` + `codex_core`) for future diagnosis via `RUST_LOG`.
- `codexn --hook-debug --check` autodetects real TUI log path (e.g. `~/.codex/log/codex-tui.log`) and prints usable `tail -f ...` command.

- [x] Hook testowy zwraca tylko `visible_note` -> wpis widoczny (AUTO e2e)
- [x] Hook testowy zwraca tylko `auto_user_reply` -> kolejny turn rusza (AUTO e2e)
- [x] Hook testowy zwraca obie akcje -> najpierw wpis, potem auto-turn (AUTO e2e `both`)
- [x] Hook testowy zwraca śmieci JSON -> brak crasha, normalny flow (AUTO e2e)

## Gate 6: Merge-Safety / Maintainability

- [ ] Diff ograniczony głównie do:
  - [ ] `hooks/`
  - [ ] `core/src/codex.rs`
- [ ] Brak szerokich zmian w `protocol` (poza ewentualnymi minimalnymi typami pomocniczymi, jeśli konieczne)
- [x] Brak szerokich zmian w `protocol` (poza ewentualnymi minimalnymi typami pomocniczymi, jeśli konieczne)
- [x] Kod ma testy dla nowego kontraktu hook response
- [ ] Brak wpływu na zwykłe sesje, gdy hooki są wyłączone / nie zwracają akcji

## Exit Criteria (Done)

Task `c1/01` uznajemy za domknięty, gdy:

- [ ] Gates 0-6 przechodzą
- [x] Jest krótki przykład hook response JSON w kodzie/testach lub docs
- [x] Smoke harness ma tryb `nero_hook_msg` z `format/status` (structured block)
- [ ] Jest jasne miejsce rozszerzenia na `after_tool_use` w kolejnym kroku
