# Plan: Codex-Nero Hook PoC (Visible Note + Auto User Reply)

## Cel

Zrobić minimalny, merge-safe PoC rozszerzenia hooków w `codex-rs`, który po `after_agent` potrafi:

1. dodać widoczny wpis do potoku/historii (widoczny dla usera i agenta),
2. opcjonalnie zakolejkować automatyczną odpowiedź jako `user` (synthetic user turn),

bez pełnej orkiestracji workflow/MCP na tym etapie.

## Kontekst (ustalone)

- `after_agent` hook już istnieje i jest odpalany w `core/src/codex.rs`.
- `after_tool_use` hook też istnieje (przyda się w kolejnym kroku pod MCP/workflow).
- Obecny hook system jest outbound-only (`payload -> hook`, brak akcji zwrotnej).
- `WarningEvent` jest renderowany w UI/TUI i nadaje się na szybki "widoczny wpis".
- `Op::UserInput` / `Op::UserTurn` pozwala na auto-kontynuację jako user.

## Zakres PoC (ten etap)

### In scope

- Rozszerzenie hooków o akcje zwrotne (response actions).
- Obsługa akcji po `after_agent`.
- Dwie akcje:
  - `visible_note`
  - `auto_user_reply`
- Minimalna konfiguracja feature flag / enable.
- Testy jednostkowe i smoke dla parsera + wykonania akcji.

### Out of scope (na razie)

- Pełna integracja z zewnętrznym serwerem workflow przez MCP.
- Orkiestracja `dev + review`.
- Zaawansowane policy engine / wieloetapowe gates.
- Nowy dedykowany event UI (na start używamy `WarningEvent`).
- `after_tool_use` akcje wykonawcze (tylko przygotowanie pod później).

## Docelowe zachowanie PoC

### 1) Visible note (widoczne dla wszystkich)

Po `after_agent` hook może zwrócić akcję z tekstem:

- Codex emituje `EventMsg::Warning(WarningEvent { message })`
- wpis jest widoczny w TUI/exec i zapisany w rollout

Przykład użycia:

- `[nero-hook] uncertainty=4/10, continue=true (threshold=6)`

### 2) Auto user reply (synthetic user message)

Po `after_agent` hook może zwrócić akcję z tekstem do wysłania jako user:

- Codex kolejkuje `Op::UserInput` z `UserInput::Text`
- kolejny turn rusza automatycznie (bez manualnego "tak, dalej")

Przykład treści (MVP):

- prośba o ocenę niepewności 1-10 i instrukcja kontynuacji przy `<= threshold`

## Architektura PoC (minimalna)

### A. Rozszerzenie hook response (hooks crate)

Dodać nowy typ odpowiedzi hooka (parsowany ze stdout JSON hook command), np.:

- `HookAction`
  - `VisibleNote { message }`
  - `AutoUserReply { message }`

Nowy typ wyniku wykonania hooka:

- status (`success` / `failed_continue` / `failed_abort`) jak dziś
- opcjonalne `actions[]`

Wymaganie MVP:

- brak stdout lub niepoprawny JSON => zachowanie jak dziś (hook success/failure bez akcji)

### B. Wykonanie akcji w core (after_agent)

W miejscu dispatchu `after_agent`:

- po zebraniu outcomes z hooków
- zebrać akcje i wykonać je w deterministycznej kolejności:
  1. `visible_note`
  2. `auto_user_reply`

### C. Enqueue auto user reply

Najprościej w PoC:

- użyć istniejącego `sess.submit(Op::UserInput { ... })`
- message jako zwykły tekst `UserInput::Text`

Założenie:

- auto-reply odpalamy dopiero po zakończeniu bieżącego turnu (`after_agent`, gdy `needs_follow_up == false`)

## Proponowane pliki do zmian

### 1. Hooks types / parsing

- `apps/codex-nero/codex-rs/hooks/src/types.rs`
- `apps/codex-nero/codex-rs/hooks/src/registry.rs`
- (opcjonalnie nowy plik) `apps/codex-nero/codex-rs/hooks/src/response.rs`

Zakres:

- typy `HookAction*`
- format odpowiedzi JSON hooka
- parser stdout -> `actions[]`
- testy serializacji/parsingu

### 2. Core execution after hook

- `apps/codex-nero/codex-rs/core/src/codex.rs`

Zakres:

- wykonanie `visible_note` jako `WarningEvent`
- enqueue `auto_user_reply` jako `Op::UserInput`
- guardi przed pętlą (patrz ryzyka)

### 3. (Opcjonalnie) Config wire-up dla enable/threshold (jeśli PoC ma być włączalny z config)

- minimalnie: `config-nero.toml` czytane później przez fork (niekoniecznie w tym kroku)
- na teraz można zacząć od hardcoded dev flag + test hook command

## Format hook response (MVP propozycja)

Hook command może wypisać JSON na stdout:

```json
{
  "actions": [
    { "type": "visible_note", "message": "[nero-hook] uncertainty=4/10, continue=true" },
    { "type": "auto_user_reply", "message": "Oceń poziom niepewności 1-10... Jeśli <= 6 i masz plan dalszych działań, kontynuuj samodzielnie." }
  ]
}
```

Zasady MVP:

- `actions` opcjonalne
- nieznane typy akcji: ignoruj + warning log
- limit rozmiaru tekstu (np. 2-4 KB) aby nie wpuszczać przesadnych payloadów

## Sequencing / kolejność wdrożenia

### Etap 1: parser i kontrakt (bez efektów w core)

1. Dodać typy `HookAction`
2. Dodać parser stdout JSON hooka
3. Testy parsera:
   - valid actions
   - empty stdout
   - invalid JSON
   - unknown action type

### Etap 2: visible_note (punkt 1)

1. W `after_agent` odczyt akcji z outcomes
2. Emit `WarningEvent`
3. Test/integration smoke: event pojawia się w streamie

### Etap 3: auto_user_reply (punkt 2)

1. Enqueue `Op::UserInput`
2. Guard przeciw pętli automatu (patrz niżej)
3. Smoke test: po hooku pojawia się kolejny user turn

## Ryzyka i zabezpieczenia (ważne)

### Ryzyko 1: Pętla auto-reply (agent -> hook -> auto-user -> agent -> hook -> ...)

Mitigacje MVP:

- domyślnie `auto_user_reply` disabled (feature flag)
- limit auto-chain depth per session/turn (np. max 1-2 z rzędu)
- marker w synthetic user message (np. prefix/tag) i skip hook trigger dla kolejnego kroku, jeśli potrzeba

### Ryzyko 2: Hook server zawiesza turn

Mitigacje:

- timeout hooka (jeśli nie ma już)
- fail-open (`FailedContinue`) jako default dla błędów integracyjnych

### Ryzyko 3: Widoczny wpis jako `WarningEvent` myli semantykę

Mitigacja:

- prefiks `[nero-hook]` w message
- później można dodać dedykowany event typu `NeroHookNote`

## Testy PoC (co ma udowodnić wejście)

### Test 1: visible_note

- Hook zwraca `visible_note`
- Codex emituje `EventMsg::Warning`
- Wpis ląduje w rollout / streamie

### Test 2: auto_user_reply

- Hook zwraca `auto_user_reply`
- Codex kolejkuje `Op::UserInput`
- Startuje kolejny turn bez manualnego inputu

### Test 3: safety / no-op

- Hook bez stdout JSON -> brak akcji, normalne zachowanie
- Invalid JSON -> brak crasha, log warning, normalne zachowanie

## Następny krok po PoC (nie w tym tasku)

## Upgrade backlog (nero_hook_msg ergonomia)

### Upgrade U1: Throttling / frequency (`freq`)

Cel:
- ograniczyć bombardowanie agenta i TUI tym samym reminderem/status message przy szybkich wymianach.

Kontrakt (canonical `nero_hook_msg`):
- opcjonalne pole `freq` (sekundy)
- `freq = 0` (domyślnie) => bez throttlingu, emituj zawsze
- `freq > 0` => nie emituj częściej niż co `freq` sekund dla tego samego komunikatu (per sesja)

Semantyka PoC upgrade:
- throttling kluczowany per sesja + treść/ustawienia wiadomości (`hook_name`, `mode`, `show.*`, `msg.full`, `msg.short`)
- gdy komunikat jest stłumiony przez `freq` i `show.tui=true`:
  - TUI dostaje jawny wpis `[nero-hook] throttled (...)` z countdownem (`next update in Ns`)
  - agentowa notka (`show.agent`) jest pomijana dla tego cyklu

Planowany etap następny (po praktyce):
- reset throttlingu po compaction (wysoka wartość praktyczna po zmianie układu kontekstu)

### Upgrade U2: TUI structured hook note (`format` + `status`)

Cel:
- dodać kanoniczny, wstecznie kompatybilny sposób przekazania metadata statusu do TUI
- renderować `nero_hook_msg` domyślnie jako ustrukturyzowany blok (content + status), zamiast surowego warning string

Kontrakt (canonical `nero_hook_msg`):
- opcjonalne `format` (`block|inline`, default `block`)
- opcjonalne `status` (min. `kind`, `text`) dla metadata TUI

Semantyka upgrade:
- stare hooki / stare akcje pozostają wspierane bez zmian
- `show.agent` nadal append-only na bazie `msg.full`
- countdown przy `freq` jest reprezentowany w TUI jako `status.kind=countdown` + `status.text`

- Rozszerzyć ten sam mechanizm na `after_tool_use`
- Dodać workflow server/MCP:
  - np. `basic_status(campaign_id)`
  - dynamiczne progi (`threshold`)
  - orkiestracja `dev/review`
- Dodać config `~/.codex/config-nero.toml` realnie czytany przez fork
