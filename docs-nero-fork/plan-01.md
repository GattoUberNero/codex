	## Plan (tylko Hook Msg + Hook Auto, bez dotykania reszty)

	### 1. Command channel tylko przez STOP

	- Zostawić jedyną ścieżkę komendy do agenta: STOP -> continuation_fragments ->
		HookPrompt.
	- Wyciąć agent-facing wstrzyknięcia z after_agent dla:
		NeroHookMsg(show.agent), ContextNote, DualNote(agent_message).

	### 2. After-agent tylko raport/status dla usera

	- after_agent ma publikować status i audyt runtime (msg/auto/contract), bez
		komendy do agenta.
	- DualNote zostaje tylko częścią user/TUI (bez części agentowej).

	### 3. Subagent: świadoma kontrola, ale tylko dla Nero msg/auto

	- Nie wyłączać globalnie lifecycle hooków.
	- Zablokować tylko akcje Nero msg/auto dla subagentów.
	- Zero zmian w innych feature’ach forka: multi-account, dynamic switch, fallback
		modeli, itd.

	### 4. Legacy cleanup tylko w tym obszarze

	- Usunąć stare gałęzie kompatybilności/fallbacków dotyczące tylko msg/auto.
	- Nie ruszać legacy poza hook msg/auto.
	- Nie robić „zmiany nazw” jako celu; tylko usunięcie starych ścieżek.

	### 5. Testy
	- Test: STOP nadal steruje kontynuacją i komendą.
	- Test: subagent blokuje tylko Nero msg/auto, nie cały system hooków.
	- Test regresji: brak zmian w multi-account/dynamic switch/fallback modeli.
- ----

  - HIGH: po cleanupie został złamany testowy kontrakt w samym codex.rs:14685. Kod
    produkcyjny ma już stop_delivery_contract_status(...) i nowy literal failed-stop-
    checkpoint-missing, ale test nadal woła stare
    runtime_delivery_contract_status(...) i oczekuje starego statusu. To oznacza
    niespójność po rename i ryzyko, że ścieżka --lib dla codex-core jest teraz
    faktycznie popsuta. Odniesienia: codex.rs:643, codex.rs:652, codex.rs:14685,
    codex.rs:14693.   
 
   ---   
 
     # Nero Naming Policy For Fork-Owned Capability Surfaces

  ## Summary

  Wprowadzamy spójną zasadę: każdy fork-owned symbol Rust, który stanowi granicę capability albo integracji runtime/reporting, ma być oznaczony Nero / nero_. Nie robimy
  overkillu na każdym lokalnym obliczeniu, ale usuwamy niejednoznaczność na wszystkich nazwach, które czyta drugi człowiek lub inny moduł. Zmiany dotyczą tylko symboli
  Rust; wire strings i natywne upstream API zostają bez zmian.

  ## Naming Rules

  - Prefiks Nero / nero_ dostają fork-owned:
      - publiczne typy i enumy capability
      - parsery i envelope’y hook actions
      - runtime/reporting entrypoints
      - audit/status/render functions specyficzne dla Nero
  - Upstream-native symbole zachowują swoje nazwy:
      - EventMsg::Warning
      - WarningEvent
      - HookCompletedEvent
      - inne niezmienione upstream API
  - Drobne lokalne helpery czysto techniczne nie muszą mieć nero_*, jeśli nie są capability-specific.
  - Dla enumów własność może być oznaczona na poziomie typu, nie wariantu.
  - Nie wprowadzamy aliasów kompatybilności ani „deprecated old names”. Rename ma być prosty i pełny.
  ## Implementation Changes

  - Uporządkować hook action surface w codex-rs/hooks/src/response.rs:
      - HookAction -> NeroHookAction
      - ParsedHookActions -> NeroParsedHookActions
      - HookActionEnvelope -> NeroHookActionEnvelope
      - parse_hook_action_envelope -> parse_nero_hook_action_envelope
      - parse_hook_actions_from_stdout -> parse_nero_hook_actions_from_stdout
  - Uporządkować fork-owned runtime/reporting names w codex-rs/core/src/codex.rs tam, gdzie dziś są capability-specific, ale brzmią generycznie.
      - Przykład: after_agent_runtime_hook_completed_event powinien mieć nazwę z nero_, bo dotyczy wyłącznie Nero after-agent reporting.
      - Przykład: helpery kontraktu delivery/stop mają mieć jawny nero_ w nazwie, jeśli nie są upstream-neutral.
  - Zostawić bez zmian nazwy już dobre:
      - nero_hook_tui_warning_message
      - nero_hook_tui_delivery
      - append_nero_hook_delivery_audit
      - try_new_nero_hook_warning_event
  - Nie zmieniać JSON action type strings:
      - nero_hook_msg
      - visible_note
      - auto_user_reply
        To jest świadoma decyzja stabilności kontraktu wire.

  ## Test Plan

  - Zaktualizować testy kompilacyjne i jednostkowe po rename symboli Rust.
  - Utrzymać bez zmian semantykę parsera i runtime:
      - known actions nadal działają
      - unknown actions nadal są ignorowane/liczone
      - mixed payload nadal zachowuje valid actions
  - Dodać jedną kontrolę repo-style dla tego lane:
      - grep/review check, że w fork-owned hook msg/auto capability surfaces nie zostały już gołe generyczne nazwy typu HookAction / ParsedHookActions bez Nero.

  ## Assumptions And Defaults

  - Zakres polityki: capability surfaces, nie każdy lokalny detal implementacyjny.
  - Zakres wdrożenia pierwszej rundy: hook msg / hook auto / parser-runtime-reporting lane.
  - Rust symbol rename: tak.
  - Wire/protocol rename: nie.
  - Brak fallbacków, aliasów i warstw przejściowych.



 1. HIGH, schema C, confirmed 2/2: CODEXN_AUTH_ROTATE_CMD omija hardened spawn path i idzie przez surowe tokio::process::Command. Przez to nie dostaje standardowych
     zabezpieczeń jak env_clear(), kill_on_drop(true) i parent-death handling. To jest najsilniej potwierdzony problem z całej rundy. Zobacz client.rs:1757 i spawn.rs:50.
  2. MEDIUM, schema D, confirmed 2/2: model fallback miesza dwa porządki tożsamości modelu. Config/cooldown używają znormalizowanej identity, ale selection/sticky path dalej
     porównuje surowe stringi modelu. Alias typu custom/gpt-5.3-codex vs gpt-5.3-codex może więc zgubić sticky state albo ustawić zły punkt startu drabinki fallbacku. Zobacz
     config/mod.rs:1194, state/session.rs:55, codex.rs:1731, codex.rs:3504.
  3. MEDIUM, schema B, single-review finding: F1-F5 może się zakleszczyć po stale-thread / disconnect. clear_active_thread() nie czyści do końca widget-side identity, więc
     hotkey może polecieć w martwy thread id, wejść w early return i nie zawołać finish_nero_auto_hotkey_action(...), zostawiając nero_auto_hotkey_inflight podniesione.
     Zobacz app.rs:4043, app.rs:3166, chatwidget.rs:5149, chatwidget.rs:10613.
  4. MEDIUM, schema A, single-review finding: VisibleNote idzie do usera jako WarningEvent, ale nie uczestniczy w HookCompleted, więc “warning/reporting lane” jest
     rozdzielony na dwa niesymetryczne kanały. Hook oparty tylko o visible_note nie zostawia completion recordu. Zobacz codex.rs:656, codex.rs:8564, codex.rs:8799.
  5. MEDIUM, schema C, single-review findings, nie w pełni zbieżne: