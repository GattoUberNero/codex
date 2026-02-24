# User Tests (UX + E2E Campaign) — Hook Actions PoC `after_agent`

## Cel kampanii

Zweryfikować **realność założeń UX i workflow** dla PoC hooków:
- `visible_note` (widoczny wpis kontekstowy)
- `auto_user_reply` (best-effort synthetic user continuation)
- kombinacja obu akcji
- zachowanie przy błędnym output hooka / legacy stdout

Kampania łączy:
- **AUTO e2e** (powtarzalne scenariusze regresyjne)
- **MANUAL UX** (ocena realnego zachowania z perspektywy użytkownika)

Testy są nastawione na **zachowanie z perspektywy użytkownika**, nie tylko poprawność techniczną.

## Wymagania wstępne

1. `codexn` działa (`codexn --version`)
2. Hook harness gotowy:
   - `apps/codex-nero/.purrnet/camp-user/c1/01/smoke_notify.sh`
3. Tymczasowo w `~/.codex/config.toml`:

```toml
notify = ["/workspace/purrnet/apps/codex-nero/.purrnet/camp-user/c1/01/smoke_notify.sh"]
```

4. (Opcjonalnie) log payloadów:

```bash
export NERO_HOOK_SMOKE_LOG=/tmp/codex-nero-hook-smoke.log
```

## Skala oceny UX (prosta)

Po każdym scenariuszu oceń:
- `A` — działa dobrze, UX czytelny, zachowanie przewidywalne
- `B` — działa, ale drobne tarcie UX / wording / timing
- `C` — działa technicznie, ale UX mylący lub irytujący
- `D` — problem funkcjonalny / zachowanie nieakceptowalne

## Kampania A: AUTO E2E (regresja / realizm przepływu)

Cel:
- szybko potwierdzić, że podstawowe flow nadal działa po zmianach w hookach
- złapać regresje zanim zaczniemy oceniać UX ręcznie

Prereq:
- zbudowana binarka `codex-linux-sandbox` (dla lokalnych e2e bez soft-skip)

Uruchomienie (targetowane):

```bash
cd /workspace/purrnet/apps/codex-nero/codex-rs
cargo test -p codex-core --test all hook_actions_notify::after_agent_ -- --nocapture
```

Debug handoff logs (opcjonalnie, opt-in):

```bash
RUST_LOG=codex_core=debug,codex_hooks=debug \
cargo test -p codex-core --test all hook_actions_notify::after_agent_ -- --nocapture
```

To pokazuje m.in.:
- inicjalizację hook registry
- dispatch hooków `after_agent`
- spawn `legacy_notify`
- parse `actions[]`
- wykonanie `visible_note`
- defer/queue `auto_user_reply`

Scenariusze AUTO (mapa):
- `visible_note` -> warning + completion
- `legacy stdout` -> brak akcji + normalny flow
- `garbage JSON` -> brak crasha + normalny flow
- `both` -> note + follow-up turn (best-effort path)

Pass criteria (AUTO):
- test target przechodzi
- brak crasha harnessu
- brak regresji w kolejności akcji (note przed auto)

## Kampania B: MANUAL UX (ocena doświadczenia użytkownika)

### U1. Visible Note Only (baseline UX)

Setup:
```bash
export NERO_HOOK_SMOKE_MODE=visible
codexn --dev
```

Kroki:
1. Wyślij prosty prompt, np. `Say hi`.
2. Poczekaj na zakończenie tury.

Expected:
- pojawia się widoczny wpis `[nero-hook] ...`
- wpis jest czytelny i nie wygląda jak przypadkowy warning systemowy
- tura kończy się normalnie
- brak dziwnych side-efektów (brak dodatkowych turnów)

Zanotuj:
- czy prefix `[nero-hook]` jest OK UX-owo?
- czy warning style powinien być inny typ eventu w przyszłości?

### U2. Legacy Plain Stdout (compat UX)

Setup:
```bash
export NERO_HOOK_SMOKE_MODE=legacy
codexn --dev
```

Kroki:
1. Wyślij prosty prompt.
2. Poczekaj na zakończenie tury.

Expected:
- normalny flow bez crasha
- brak widocznego hook action (bo plain stdout nie jest akcją)
- brak synthetic user continuation

Zanotuj:
- czy brak widocznej informacji jest OK dla compat path?

### U3. Garbage JSON (resilience UX)

Setup:
```bash
export NERO_HOOK_SMOKE_MODE=garbage
codexn --dev
```

Kroki:
1. Wyślij prosty prompt.
2. Obserwuj zakończenie tury i ewentualne warningi/errors.

Expected:
- brak crasha sesji
- turn flow dalej działa
- zachowanie jest przewidywalne (hook failure continue / komunikat)

Zanotuj:
- czy komunikat o błędzie hooka (jeśli jest) jest zrozumiały?
- czy powinien być bardziej “operacyjny” niż user-facing?

### U4. Both Actions (`visible_note` + `auto_user_reply`)

Setup:
```bash
export NERO_HOOK_SMOKE_MODE=both
codexn --dev
```

Kroki:
1. Wyślij prompt, który kończy turę bez blokad.
2. Obserwuj:
   - wpis `[nero-hook]`
   - ewentualny follow-up synthetic turn
3. Nic nie klikaj/nie odpowiadaj przez chwilę (sprawdzenie auto flow).

Expected:
- `visible_note` pojawia się przed auto-kontynuacją
- synthetic turn może ruszyć automatycznie (best-effort)
- brak pętli
- brak “dziwnego mieszania” z ręcznym inputem, jeśli nic nie wpisujesz

Zanotuj:
- czy treść auto-reply promptu jest dobra?
- czy to pomaga workflow, czy spamuje?

### U5. Race Test (manual user vs auto_user_reply)

Setup:
```bash
export NERO_HOOK_SMOKE_MODE=both
codexn --dev
```

Kroki:
1. Wyślij prompt.
2. Gdy tura się kończy, szybko ręcznie wpisz kolejną wiadomość (celowo ścigając auto-reply).

Expected:
- brak crasha / brak pętli
- jeśli synthetic zostanie pominięty z powodu race, to jest OK (safety-first)
- manualny input powinien wygrać/przejść normalnie

Zanotuj:
- czy to zachowanie jest akceptowalne produktowo?
- czy potrzebujemy wyraźniejszego komunikatu “auto skipped due to manual input”?

### U6. Multi-turn Stability (3-5 tur)

Setup:
```bash
export NERO_HOOK_SMOKE_MODE=visible
codexn --dev
```

Kroki:
1. Zrób 3-5 zwykłych tur.
2. Obserwuj, czy hook działa stabilnie w każdej.

Expected:
- brak degradacji UX
- brak opóźnień / brak zanikających wpisów hooka
- brak narastających efektów ubocznych

Zanotuj:
- czy warning styl zaczyna przeszkadzać po kilku turach?

## Arkusz wyników (AUTO + MANUAL)

AUTO E2E:
- A1 `visible_note`: `PASS/FAIL/SKIP` + notatki
- A2 `legacy stdout`: `PASS/FAIL/SKIP` + notatki
- A3 `garbage JSON`: `PASS/FAIL/SKIP` + notatki
- A4 `both`: `PASS/FAIL/SKIP` + notatki

MANUAL UX:

- U1 Visible Note: `A/B/C/D` + notatki
- U2 Legacy Stdout: `A/B/C/D` + notatki
- U3 Garbage JSON: `A/B/C/D` + notatki
- U4 Both Actions: `A/B/C/D` + notatki
- U5 Race Manual vs Auto: `A/B/C/D` + notatki
- U6 Multi-turn Stability: `A/B/C/D` + notatki

## Decyzje po kampanii (do podjęcia)

1. Czy `visible_note` zostaje jako `WarningEvent`, czy robimy dedykowany event/UI style?
2. Czy `auto_user_reply` domyślnie ON, czy feature-flag OFF?
3. Czy treść auto promptu ma być:
   - stała,
   - konfigurowalna,
   - generowana przez plugin/server?
4. Jak komunikować skip auto-reply przy race z manualnym inputem?
5. Jak ścisła ma być kompatybilność `legacy_notify` stdout?

## Szybki workflow wykonania (rekomendowany)

1. Uruchom Kampanię A (AUTO E2E)
2. Jeśli AUTO zielone -> przejdź Kampanię B (MANUAL UX)
3. Wypełnij arkusz wyników
4. Podejmij decyzje z sekcji końcowej (policy + UX)
