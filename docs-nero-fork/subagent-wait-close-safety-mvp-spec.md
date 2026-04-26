# Subagent Wait and Close Safety MVP

Status: implemented MVP, final-audited

Scope:

- `codex-rs/protocol`
- `codex-rs/tools`
- `codex-rs/core` multi-agent wait/close handlers
- `codex-rs/core` agent control lifecycle guardrails
- `codex-rs/tui` collaboration transcript rendering

## Cel

Ten MVP ogranicza przypadki, w ktorych glowny agent albo user interpretuje wynik `wait_agent`
jako dowod, ze subagent zakonczyl prace albo sie zawiesil, mimo ze w praktyce nadal wykonuje
zadanie.

To nie jest redesign schedulera agentow ani monitoring aktywnosci rolloutow. To jest bezpieczny,
minimalny patch semantyki i prezentacji:

- `wait_agent` nie komunikuje timeoutu w mylacy sposob,
- UI nie sugeruje zakonczenia oczekiwania jako zakonczenia pracy,
- `close_agent` nie zabija domyslnie agentow ani potomkow, ktorzy nadal pracuja.

## Problem

Stan sprzed MVP mial 3 poznawczo toksyczne cechy:

1. `wait_agent` zwraca surowy timeout bez rozroznienia miedzy:
   - "zobaczono finalizacje",
   - "okno nasluchu minelo, ale agent moze dalej pracowac".
2. TUI renderuje `Finished waiting`, nawet gdy nikt nie zakonczyl pracy.
3. `close_agent` nie mial guardu bezpieczenstwa i mogl zamknac agenta w stanie `running`.

W praktyce daje to dwa bledne odruchy:

- przedwczesne ponawianie `wait_agent`,
- zamykanie agenta, ktory nadal wykonuje prace.

## Non-Goals

W tym MVP nie robimy:

- pelnego monitoringu aktywnosci rolloutow,
- heurystyki "czy agent na pewno zyje",
- logow ostatnich akcji agenta w odpowiedzi `wait_agent`,
- nowego schedulera ani nowej lane telemetrycznej,
- zmiany globalnego `AgentStatus` na stan obserwacji `wait_agent`.

To jest etap 2 i powinien byc rozwazany oddzielnie.

## Zasada architektoniczna

Timeout `wait_agent` nie jest nowym lifecycle state agenta.

`AgentStatus` ma dalej opisywac stan bytu agenta:

- `pending_init`
- `running`
- `interrupted`
- `completed`
- `errored`
- `shutdown`
- `not_found`

Natomiast wynik pojedynczego wywolania `wait_agent` ma opisywac wynik obserwacji:

- czy zobaczono finalizacje,
- czy tylko minelo okno nasluchu,
- ktore targety pozostaja nadal aktywne w momencie zakonczenia okna obserwacji.

Nie wolno mieszac tych dwoch poziomow.

## Zmiany MVP

### 1. Wait outcome jako wynik obserwacji

Do wyniku `wait_agent` oraz do `CollabWaitingEndEvent` nalezy dodac jawne pole obserwacyjne:

`wait_outcome`

Proponowany enum wire-level:

- `completion_already_available`
- `completion_observed`
- `activity_observed`
- `listen_window_ended`

Semantyka:

- `completion_already_available`
  - co najmniej jeden target byl juz w stanie finalnym przed startem tego wywolania `wait_agent`;
  - to nie jest nowa finalizacja zaobserwowana podczas biezacego okna nasluchu;
- `completion_observed`
  - podczas tego wywolania `wait_agent` zaobserwowano co najmniej jeden status finalny;
- `activity_observed`
  - zaobserwowano aktywnosc mailbox/koordynacyjna, ale bez statusu finalnego;
  - uzywane tylko na kompatybilnej sciezce v2 bez jawnych `targets`;
- `listen_window_ended`
  - okno nasluchu zakonczylo sie bez zaobserwowania finalizacji;
  - nie jest to dowod bledu, zawieszenia ani bezczynnosci.

Dla jawnych wielu `targets`, `wait_agent` konczy obserwacje po pierwszym zaobserwowanym statusie finalnym. Nie oznacza to, ze wszystkie targety zakonczyly prace.

### 2. Zachowanie `timed_out`

Dla kompatybilnosci wstecznej `timed_out` moze pozostac.

Jednak:

- `timed_out = true` musi odpowiadac `wait_outcome = listen_window_ended`,
- `timed_out = false` musi odpowiadac `wait_outcome = completion_already_available`,
  `completion_observed` albo `activity_observed`.

Nowe pole `wait_outcome` staje sie kanoniczne dla UI i dla dalszych decyzji runtime.
`timed_out` pozostaje polem kompatybilnosci.

### 3. Zmiana semantyki komunikatu v2

Obecny komunikat:

- `Wait timed out.`

jest zbyt sugestywny i powinien zostac zastapiony komunikatem obserwacyjnym.

Proponowane komunikaty:

- dla `completion_already_available`:
  - gdy nie ma pozostalych targetow pending:
    - `Completion was already available before this wait call.`
  - gdy pozostaja pending targety:
    - `Completion already available; N target(s) remain pending.`
- dla `completion_observed`:
  - gdy nie ma pozostalych targetow pending:
    - `Observed completion.`
  - gdy pozostaja pending targety:
    - `Observed first completion; N target(s) remain pending.`
- dla `activity_observed`:
  - `Observed activity.`
- dla `listen_window_ended`:
  - `Listen window ended; no completion observed yet.`

Nie wolno uzywac komunikatow, ktore implikuja:

- awarie,
- zawieszenie,
- prawo do zamkniecia agenta.

### 4. Timeout multiplier

Obecny self-estimate podawany przez delegujacy model jest praktycznie zbyt krotki.

W MVP nalezy zastosowac centralny mnoznik:

- `effective_timeout_ms = requested_timeout_ms * 2.0`

Nastepnie wynik musi zostac nadal ograniczony przez istniejace:

- `MIN_WAIT_TIMEOUT_MS`
- `MAX_WAIT_TIMEOUT_MS`

Czyli:

- najpierw pobieramy `requested_timeout_ms`,
- potem mnozymy przez `2.0`,
- dopiero potem clamp do min/max.

Cel:

- mniej bezsensownego ponawiania `wait_agent`,
- mniej kosztownego ponownego czytania kontekstu,
- mniej falszywych interpretacji, ze agent "nie odpowiedzial, wiec pewnie zdechl".

Jesli praktyka pokaze, ze `2.0` nadal jest zbyt agresywne, follow-up moze podniesc wartosc do
`2.5`, ale nie jest to czesc tego MVP.

### 5. Guard bezpieczenstwa dla `close_agent`

To jest najwazniejszy fix bezpieczenstwa.

`close_agent` nie moze domyslnie zamykac agentow ani ich potomkow, ktorzy sa nadal aktywni.

Nalezy dodac tryb wywolania:

- `mode: safe_close | force_cancel`

Domyslny tryb:

- `safe_close`

Semantyka:

- `safe_close`
  - dozwolone tylko gdy wskazany agent i caly jego live subtree nie zawieraja aktywnej pracy
  - status wskazanego agenta musi byc finalny albo nieobecny:
    - `completed`
    - `errored`
    - `shutdown`
    - `not_found`
  - dla statusow:
    - `pending_init`
    - `running`
    - `interrupted`
    zwracany jest blad runtime, a agent nie jest zamykany;
- `force_cancel`
  - wykonuje obecne zachowanie: zamkniecie agenta i jego zywego poddrzewa.

To rozdziela:

- porzadkowe zamkniecie juz zakonczonej pracy,
- od jawnej decyzji o anulowaniu pracujacego agenta.

### 6. TUI wording i render

TUI nie moze uzywac stalego tytulu `Finished waiting`.

Nowe renderowanie:

- dla `completion_already_available`
  - gdy nie ma pending targetow:
    - tytul: `Completion already available`
  - gdy pozostaja pending targety:
    - tytul: `Completion already available; N target(s) still pending`
- dla `completion_observed`
  - gdy nie ma pending targetow:
    - tytul: `Observed completion`
  - gdy pozostaja pending targety:
    - tytul: `First completion observed; N target(s) still pending`
- dla `activity_observed`
  - tytul: `Observed activity`
- dla `listen_window_ended`
  - tytul: `Listen window ended`

Detale dla `listen_window_ended`:

- pierwsza linia:
  - `No completion observed yet`
- druga linia:
  - `Agents may still be running`

Jesli event zawiera finalne statusy, maja byc renderowane jak dzisiaj.
Jesli nie zawiera finalnych statusow, UI nie moze sugerowac, ze cokolwiek sie zakonczyl.

### 7. Event protocol

`CollabWaitingEndEvent` musi dostac:

- `wait_outcome`

TUI ma korzystac z tego pola jako z kanonicznego zrodla interpretacji.
Nie wolno inferowac outcome z pustego `statuses`.

## Backward Compatibility

- `wait_agent` zachowuje `timed_out`.
- `wait_agent` rozszerza output o `wait_outcome`.
- `wait_agent` rozszerza output o `pending[]` z nadal aktywnymi targetami obserwowanymi na koncu okna.
- `CollabWaitingEndEvent` rozszerza payload o `wait_outcome`.
- `close_agent` przy braku pola `mode` ma zachowywac sie jak `safe_close`.
- `force_cancel` jest nowa, jawna droga do starego destrukcyjnego zachowania.

Nie wolno:

- tworzyc nowego `AgentStatus` typu `timed_out_running`,
- mapowac timeoutu `wait_agent` na lifecycle status agenta,
- traktowac `listen_window_ended` jako bledu.

## Suggested Wire Shapes

### `wait_agent` v1

```json
{
  "status": {
    "agent_123": "completed"
  },
  "pending": [],
  "timed_out": false,
  "wait_outcome": "completion_observed"
}
```

```json
{
  "status": {},
  "pending": [
    {
      "id": "agent_456",
      "state": "running"
    }
  ],
  "timed_out": true,
  "wait_outcome": "listen_window_ended"
}
```

### `wait_agent` v2

```json
{
  "message": "Observed activity.",
  "pending": [],
  "timed_out": false,
  "wait_outcome": "activity_observed"
}
```

```json
{
  "message": "Listen window ended; no completion observed yet. Agents may still be running.",
  "pending": [
    {
      "id": "agent_456",
      "state": "running"
    }
  ],
  "timed_out": true,
  "wait_outcome": "listen_window_ended"
}
```

### `close_agent`

```json
{
  "target": "agent-id-or-task-name",
  "mode": "safe_close"
}
```

```json
{
  "target": "agent-id-or-task-name",
  "mode": "force_cancel"
}
```

## Error Semantics

Jesli `close_agent(mode = safe_close)` trafi na aktywnego agenta albo aktywnego potomka w
zamykanym subtree, blad powinien byc jawny i operacyjny. Przykladowy sens komunikatu:

`agent subtree still has active agent <id>; use force_cancel only when you intentionally want to terminate running work`

Komunikat ma:

- nie byc agresywny,
- nie byc wieloznaczny,
- jasno sugerowac, ze aktywny agent nie zostal zamkniety przez przypadek.

## Success Criteria

Ta specyfikacja jest spelniona, gdy:

- timeout `wait_agent` nie jest juz mylony z finalizacja pracy,
- UI nie komunikuje `Finished waiting`, gdy nic nie zostalo zakonczone,
- domyslne `close_agent` nie ubija pracujacych agentow ani pracujacych potomkow,
- anulowanie pracujacego agenta wymaga jawnej decyzji,
- a koszt poznawczy i koszt tokenowy ponawiania waitow spada bez wchodzenia w etap 2.

## Etap 2 Explicitly Deferred

Poza tym MVP pozostaja swiadomie odlozone:

- monitoring aktywnosci rolloutow,
- wglad w ostatnie akcje agenta,
- heurystyka "agent najprawdopodobniej nadal pracuje",
- adaptacyjny timeout oparty o typ pracy albo telemetryke.
