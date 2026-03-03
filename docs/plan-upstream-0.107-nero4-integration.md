# Plan Integracji v2: upstream `rust-v0.107.0` -> fork `0.107.nero4`

## Cel

- bezpiecznie podniesc fork z linii `0.106` do bazy `rust-v0.107.0`,
- zachowac custom semantyke NERO,
- zamknac znane ryzyka multi-agent scope,
- przygotowac wydanie forka opisane biznesowo jako `0.107.nero4`.

## Stan wejsciowy

- branch fork: `main-nero` @ `2919e255f` (snapshot lokalnych zmian),
- target upstream release: `rust-v0.107.0` (peeled commit `19f8797c0`, data 2026-03-02),
- divergence: `main-nero...rust-v0.107.0` = `30` commitow fork-only i `51` target-only.

## Krytyczne ograniczenia release

Aktualny workflow release (`.github/workflows/rust-release.yml`) akceptuje tylko tagi:
- `rust-vX.Y.Z`
- `rust-vX.Y.Z-alpha.N`
- `rust-vX.Y.Z-beta.N`

i wymusza `tag_ver == cargo_ver`.

Wniosek:
- `rust-v0.107.0-nero4` i `0.107.0-nero.4` nie przejdzie bez modyfikacji workflow.

Decyzja bezpieczna na teraz:
- Cargo version: `0.107.0-nero.4` (SemVer poprawne dla Cargo),
- tag wewnetrzny forka: `nero-v0.107.0-nero.4` (nie odpala upstream workflow `rust-release`),
- nazwa wydania biznesowa: `0.107.nero4`.

Opcja alternatywna (jesli chcemy `rust-v...`):
- najpierw patch workflow regex i walidacji pod `-nero.N`,
- dopiero potem tag `rust-v0.107.0-nero.4`.

## Faza A: Baseline i safety

1. `git fetch --all --tags --prune`
2. `git switch main-nero && git pull --ff-only`
3. Jednoznaczny punkt bazowy: commit opisujacy `0.106.nero3`.
4. Backup:
- lokalny tag z timestampem: `nero/pre-0.107-integration-YYYYmmdd-HHMMSS`
- opcjonalny backup push (branch/tag) do `origin`.
5. `git config rerere.enabled true`

Gate A:
- clean working tree,
- baseline `0.106.nero3` istnieje jako commit,
- backup referencja istnieje.

## Faza B: Audyty przed integracja

### B1. Audyt merge-risk

- `git cherry -v rust-v0.107.0 main-nero`
- `git range-diff rust-v0.106.0...main-nero rust-v0.106.0...rust-v0.107.0`
- klasyfikacja commitow:
  - strict NERO custom,
  - merge/noise/integracyjne,
  - potencjalnie zduplikowane semantycznie.

### B2. Audyt scope/ownership dla multi-agent

Sprawdzic, czy operacje destrukcyjne i sterujace sa scope-checked:
- `close_agent`
- `send_input`
- `resume_agent`
- `wait`

Minimalny standard:
- brak mozliwosci oddzialywania na obce live-thready spoza sesji.

Gate B:
- lista commitow do replay zamrozona,
- lista wymaganych patchy hardeningowych zamrozona.

## Faza C: Integracja na branchu roboczym

1. `git switch -c integration/0.107-nero4 rust-v0.107.0^{}`
2. Replay commitow fork-only w batchach tematycznych (nie wszystko naraz):
- batch 1: plumbing/test harness,
- batch 2: hook contract/runtime,
- batch 3: TUI i UX,
- batch 4: final cleanup.
3. Duze commity snapshotowe rozbijac tematycznie przy replay.

Gate C:
- po kazdym batchu zielony build i testy obszaru,
- brak unresolved konfliktow,
- commit history czytelna tematycznie.

## Faza D: Testy i walidacja

Po kazdym batchu:
- `cargo check --workspace`
- targetowane testy:
  - `cargo test -p codex-core --tests`
  - `cargo test -p codex-hooks --tests`
  - `cargo test -p codex-tui --tests`

Przed finalem:
- `cargo clippy --all-features --tests -- -D warnings`
- `cargo nextest run --all-features --no-fail-fast`

Dodatkowo dla hook e2e:
- upewnic sie, ze wymagane binarki/sandbox sa dostepne,
- potwierdzic brak cichego skip krytycznych testow.

Gate D:
- wynik lokalny zielony,
- PR CI green (`rust-ci` + wymagane checki) przed tagiem.

## Faza E: Review i release

1. Niezalezne code review calego diffu integracyjnego.
2. Poprawki po review.
3. Merge do `main-nero` po akceptacji i CI.
4. Dopiero po merge:
- ustawienie version na `0.107.0-nero.4` (jesli nie ustawione wczesniej),
- tag forka: `nero-v0.107.0-nero.4`,
- push branch + tylko ten konkretny tag (bez `--tags`).

Gate E:
- SHA zmergowany i zatwierdzony,
- testy i review zamkniete,
- tag i wersja spójne.

## Rollback

- przed tagiem: usuwamy wyłącznie branch `integration/0.107-nero4`, `main-nero` nienaruszony,
- po tagu/release:
  - usuniecie remote taga,
  - oznaczenie/anulowanie release artifactow,
  - ewentualny revert merge commit,
  - komunikat operacyjny o cofnieciu.

## Definition of Done

- kod bazuje na `rust-v0.107.0`,
- NERO behavior zachowany i pokryty testami,
- hardening scope dla multi-agent potwierdzony,
- release forka gotowy jako `0.107.nero4` (technicznie: `0.107.0-nero.4` + `nero-v0.107.0-nero.4`).
