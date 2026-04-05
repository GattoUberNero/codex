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