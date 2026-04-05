# Nero Fork Capabilities: Multiaccount i Model Fallback

Zweryfikowany opis capability oraz lista realnych problemow dla worktree:
`/home/dev/worktrees/codex-nero/upgrade-0.117`

Ten dokument celowo rozdziela:

1. co faktycznie zostalo zaimplementowane i gdzie to siedzi w kodzie,
2. jakie problemy po weryfikacji uznalem za realne.

Nie powiela odrzuconych lub przeszacowanych podejrzen z poprzedniego review.

---

## 1. Co tu zostalo dodane

Fork Nero dodaje dwie oddzielne capability:

1. **Model fallback**
   - automatyczne przejscie na kolejny model z drabinki po wybranych problemach technicznych modelu,
   - opcjonalny cooldown i sticky behavior.

2. **Multiaccount / auth rotation**
   - proba odzyskania dzialania po `usage_limit_reached` lub `insufficient_quota`,
   - najpierw przez `ExternalAuthRecovery`,
   - potem opcjonalnie przez zewnetrzne polecenie z `CODEXN_AUTH_ROTATE_CMD`.

Obie capability sa dodatkami forka. Nie wyglada to na upstreamowy mechanizm z `0.117`.

---

## 2. Model fallback

### Gdzie jest skonfigurowany

Konfiguracja jest czytana z nakladek Nero ladowanych z `CODEXN_CONFIG_NERO_*`:

- `codex-rs/core/src/config/mod.rs`
  - `read_codexn_fork_model_fallback()`
  - `resolve_codexn_fork_model_fallback_from_env()`
  - `CodexnForkModelFallbackConfig`
  - `CodexnForkModelFallbackStep`

Konfiguracja zawiera:

- `enabled`
- `cooldown_seconds`
- `max_wait_seconds`
- `sticky`
- `ladder[]`

Walidacja juz istnieje:

- fallback wlacza sie tylko przy jawnie ustawionym `enabled = true`,
- `ladder` nie moze byc puste,
- duplikaty modelu sa wykrywane przez `codexn_fork_model_fallback_identity()`.

### Gdzie jest podlaczony do runtime

Glowne punkty integracji sa w `codex-rs/core/src/codex.rs`:

- przy starcie sesji:
  - `resolve_codexn_fork_model_fallback_from_env()` jest wpinany do `SessionConfiguration`,
- przed utworzeniem `TurnContext`:
  - `apply_model_fallback_pre_turn()`,
- po bledzie tury:
  - `try_model_fallback_after_error()`,
- po sukcesie fallbackowego modelu:
  - `mark_model_fallback_success()`.

Stan runtime trzymany jest w:

- `codex-rs/core/src/state/session.rs`
  - `ModelFallbackRuntimeState`
  - `cooldown_by_model`
  - `sticky_step`
  - `last_requested_model`

### Jak to dziala

#### Przed tura

`apply_model_fallback_pre_turn()`:

- czyta runtime spod `self.state.lock()`,
- czy stale sticky dalej moze byc uzyte,
- jezeli zadany model jest aktualnie w cooldownie, szuka kolejnego dostepnego kroku z `ladder`,
- aktualizuje `SessionConfiguration.collaboration_mode`.

#### Po bledzie

`try_model_fallback_after_error()`:

- dziala tylko dla wybranych bledow:
  - `CodexErr::ServerOverloaded`,
  - kontrolowanych `503` rozpoznawanych przez tresc odpowiedzi,
- ustawia cooldown dla modelu, ktory zawiodl,
- szuka kolejnego modelu z drabinki,
- jezeli taki model istnieje:
  - buduje nowy `TurnContext` przez `with_model_and_reasoning()`,
  - loguje probe fallbacku do audytu,
  - restartuje klienta modelowego dla kolejnej proby,
- jezeli wszystkie modele sa chwilowo w cooldownie:
  - czeka do najblizszego wygasniecia cooldownu,
  - respektuje `max_wait_seconds`,
  - respektuje cancellation token.

#### Po sukcesie

`mark_model_fallback_success()`:

- czyści cooldown dla modelu, ktory zadzialal,
- zapisuje `sticky_step`, jesli `sticky = true`,
- inkrementuje telemetrie sukcesu.

### Gdzie sa testy

Nie ma duzego pokrycia samego runtime fallbacku, ale nie jest prawda, ze wszystko ma `0%`.

Istnieja przynajmniej testy jednostkowe dla:

- konserwatywnego triggera fallbacku na `503`,
- wyboru kolejnego modelu z uwzglednieniem cooldownu.

To siedzi w:

- `codex-rs/core/src/codex.rs`

---

## 3. Multiaccount / auth rotation

### Gdzie jest zaimplementowane

Glowne miejsce:

- `codex-rs/core/src/client.rs`

Najwazniejsze elementy:

- `try_recover_stream_usage_limit_or_quota()`
- `reset_usage_limit_recovery_budget()`
- `try_recover_usage_limit_or_quota()`
- `try_recover_with_auth_rotate_command()`

### Jak to dziala

Mechanizm uruchamia sie przy:

- `CodexErr::UsageLimitReached(_)`
- `CodexErr::QuotaExceeded`

Flow:

1. resetowany jest stan websocketowej sesji klienta,
2. kod probuje `ExternalAuthRecovery`,
3. gdy to sie nie powiedzie przejsciowo, moze sprobowac `CODEXN_AUTH_ROTATE_CMD`,
4. zewnetrzne polecenie dostaje:
   - `CODEXN_ROTATION_REASON=usage_limit_reached`
   - albo `CODEXN_ROTATION_REASON=quota_exceeded`,
5. po sukcesie wykonywane jest `auth_manager.reload()`,
6. request jest probowany ponownie.

Wazne:

- `RefreshTokenError::Permanent` nie przechodzi do rotate command,
- budzet `command_recovery_attempted` jest resetowany na poczatku requestu, a nie raz na proces.

### Gdzie jest podlaczone

To jest wpiete w obsluge requestow strumieniowych w `codex-rs/core/src/codex.rs`:

- przed rozpoczeciem request loop wykonywany jest `reset_usage_limit_recovery_budget()`,
- podczas streamowania bledy `usage_limit/quota` moga od razu wyzwolic recovery i retry.

### Gdzie sa testy

Ta capability ma realne testy integracyjne. Sa m.in. scenariusze:

- recovery przez `CODEXN_AUTH_ROTATE_CMD` dla `usage_limit_reached`,
- recovery przez `CODEXN_AUTH_ROTATE_CMD` dla `insufficient_quota`,
- brak retry po czesciowym outputcie,
- scenariusze websocketowe,
- scenariusze z remote pre-turn compaction.

Pliki:

- `codex-rs/core/tests/suite/client.rs`
- `codex-rs/core/tests/suite/client_websockets.rs`
- `codex-rs/core/tests/suite/compact_remote.rs`

---

## 4. Potwierdzone realne problemy

Poniezej sa tylko problemy, ktore po weryfikacji kodu uznalem za realne.

### 4.1. Zewnetrzne polecenie rotacji auth omija standardowy hardened spawn path

Lokalizacja:

- `codex-rs/core/src/client.rs`
- `codex-rs/core/src/spawn.rs`

Opis:

- `CODEXN_AUTH_ROTATE_CMD` jest wykonywane przez surowy `tokio::process::Command`,
- ta sciezka nie korzysta z `spawn_child_async()`,
- przez to nie dostaje standardowych zabezpieczen stosowanych w innych spawnach procesu:
  - `env_clear()`,
  - kontrolowanego env,
  - `kill_on_drop(true)`,
  - parent-death handling,
  - standardowego policy wrappera dla sieci/sandboxa.

Ocena:

- **realny problem hardeningu i bezpieczenstwa operacyjnego**,
- opis z poprzedniego review byl jednak zbyt mocny:
  - nie potwierdzilem zadnej sciezki z `kilo.json`,
  - zrodlem komendy jest env `CODEXN_AUTH_ROTATE_CMD`.

### 4.2. Brak limitu prob fallbacku na jedna ture moze zapetlic obieganie drabinki

Lokalizacja:

- `codex-rs/core/src/codex.rs`

Opis:

- fallback nie ma licznika maksymalnej liczby przelaczen modelu w jednej turze,
- `max_wait_seconds` ogranicza tylko czekanie, gdy **nie ma zadnego kandydata**,
- nie ogranicza liczby natychmiastowych przelaczen, gdy kandydat zawsze istnieje,
- przy konfiguracji typu `cooldown_seconds = 0` mozna wejsc w ciag:
  - model A fail,
  - przejscie na B,
  - B fail,
  - przejscie na C,
  - ...
  - i z powrotem do A.

Ocena:

- **realny bug sterowania retry/fallbackem**,
- warunkowy od konkretnej konfiguracji i zachowania providerow,
- ale rzeczywisty, a nie wydumany.

### 4.3. Normalizacja tozsamosci modelu nie jest stosowana konsekwentnie we wszystkich punktach flow

Lokalizacja:

- `codex-rs/core/src/config/mod.rs`
- `codex-rs/core/src/state/session.rs`
- `codex-rs/core/src/codex.rs`

Opis:

- runtime cooldownow i walidacja duplikatow uzywaja `codexn_fork_model_fallback_identity()`,
- ale czesc decyzji rotacyjnych i porownan dalej opiera sie na surowym `step.model` albo `requested_model`,
- to moze prowadzic do niespojnego zachowania dla:
  - aliasow,
  - roznic w case,
  - jednopoziomowych namespace slugow.

Ocena:

- **realny problem logiki**, ale o nizszej wadze niz dwa punkty wyzej.

### 4.4. Przepelnienie bardzo duzych wartosci czasu wylacza fallback dla danej tury

Lokalizacja:

- `codex-rs/core/src/codex.rs`

Opis:

- gdy `cooldown_seconds` albo `max_wait_seconds` nie miesci sie w runtime arithmetic,
  fallback dla tej tury jest wylaczany,
- kod emituje warning i zapis do audytu, ale nie probuje przyciac wartosci do bezpiecznego maksimum.

Ocena:

- **realny edge-case hardeningowy**,
- niska waga,
- bardziej odpornosc na skrajna konfiguracje niz bug produkcyjny o wysokim prawdopodobienstwie.

---

## 5. Krotki werdykt

Sama czesc opisujaca capability ma sens:

- model fallback jest realnie zaimplementowany i wpiety w runtime tury,
- multiaccount/auth rotation tez jest realnie zaimplementowane i ma kilka testow integracyjnych.

Po weryfikacji jako realne problemy zostawilbym glownie:

1. ominiecie hardened spawn path przez `CODEXN_AUTH_ROTATE_CMD`,
2. brak limitu prob fallbacku na jedna ture,
3. niespojna normalizacja tozsamosci modeli,
4. slaba odpornosc na skrajnie duze wartosci czasu.

Reszte podejrzen z poprzedniej listy traktowalbym jako odrzucone, niepotwierdzone albo przeszacowane.
