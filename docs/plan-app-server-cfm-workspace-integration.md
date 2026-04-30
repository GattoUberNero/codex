# Plan: app-server / CFM workspace integration

## Cel

Zaprojektowac kontrolowany update, ktory pozwoli CFM i NeroBar korzystac z natywnych kanalow `codex-rs/app-server` zamiast budowac rownolegle czytniki, parsery i stany. Efekt ma byc baza pod samodzielna aplikacje operatorska: terminal egzekutora, panel kampanii, czytnik plikow/dokumentow, plan/status/todo i artefakty w jednym miejscu.

## Decyzje architektoniczne

- `codex-rs/app-server` jest podstawowym kanalem semantycznym dla aplikacji operatorskich.
- `tmux` pozostaje powierzchnia terminalowa i przezywalnym procesem, ale nie jest zrodlem prawdy dla planow, plikow ani statusu.
- CFM pozostaje zrodlem prawdy procesu kampanii: fazy, gate'y, runtime, artefakty i aktywna kampania.
- Hooki Nero przekazuja kontekst operacyjny do egzekutora, ale aplikacja nie powinna parsowac tekstu hooka jako kontraktu.
- Nie budujemy nowych czytnikow plikow, jezeli istnieje natywny app-server endpoint.
- Nie robimy pelnego update'u upstream. Kazdy import z nowszego Codexa musi byc waskim backportem z osobnym testem.

## Stan lokalny potwierdzony

- Lokalny app-server ma v2 RPC dla plikow: `fs/readFile`, `fs/readDirectory`, `fs/watch`, `fs/unwatch`.
- Lokalny app-server emituje plan: `turn/plan/updated` oraz eksperymentalne `item/plan/delta`.
- Lokalny thread history zna itemy `imageView` i `imageGeneration`.
- Lokalny fork ma Nero `multiFileReaderCall`; to jest zdolnosc forkowa, nie nalezy jej traktowac jako upstreamowego standardu.
- Lokalny app-server nie ma obecnie potwierdzonego `thread/turns/list` ani `thread/inject_items`.
- `persistExtendedHistory` istnieje jako rozszerzenie dla bogatszego `thread/read`, ale nie zastepuje endpointu listujacego tury.

## Granice zmiany

W zakresie:

- uporzadkowanie kontraktu komunikacji CFM/NeroBar z `codex-rs/app-server`,
- plan waskiego backportu brakujacych endpointow, jezeli sa potrzebne,
- plan adaptera klienta po stronie CFM/NeroBar,
- plan UI workspace readera opartego o app-server,
- plan integracji plan/todo/artefaktow jako odczyt semantyczny.

Poza zakresem:

- globalny rebase do upstream `0.125+`,
- migracja calego app-server bez selekcji,
- zastapienie CFM Campaign state przez Codex plan mode,
- nowe wlasne czytniki plikow omijajace app-server,
- zmiany w `codex-core`, jezeli da sie ich uniknac.

## Kontrolowane kroki wykonania

### Krok 0: Porownac upstream przed backportem

Zrodla:

- aktualny lokalny fork: branch roboczy `integration/codex-0.118-nero-12`,
- upstream tag albo branch, z ktorego pochodzi interesujaca funkcja,
- lokalne Nero-only zmiany w app-server.

Praca:

1. Dla kazdego kandydata sprawdzic upstream diff tylko dla app-server/protocol.
2. Odrzucic import, jezeli wymaga globalnego rebase albo przenosi niepowiazane zmiany.
3. Zapisac minimalna liste plikow i typow do backportu.
4. Sprawdzic, czy lokalny fork nie ma juz rownowaznej zdolnosci pod inna nazwa.

Gate:

- decyzja `backport / nie backport` jest zapisana przed edycja kodu.
- zakres backportu jest mniejszy niz pelny app-server merge.

### Krok 1: Zamrozic lokalny kontrakt app-server

Pliki do sprawdzenia:

- `codex-rs/app-server-protocol/src/protocol/common.rs`
- `codex-rs/app-server-protocol/src/protocol/v2.rs`
- `codex-rs/app-server-protocol/src/protocol/thread_history.rs`
- `codex-rs/app-server/src/codex_message_processor.rs`
- `codex-rs/app-server/src/bespoke_event_handling.rs`
- `codex-rs/app-server/README.md`

Praca:

1. Spisac aktywne requesty, notificationy i `ThreadItem` potrzebne dla CFM/NeroBar.
2. Oznaczyc, ktore sa upstreamowe, a ktore sa Nero-only.
3. Zapisac wynik jako tabela kontraktu w dokumencie implementacyjnym albo w README app-server.

Gate:

- wiadomo, czy dana funkcja ma isc przez istniejacy endpoint, czy wymaga backportu.
- brak zmian runtime.

### Krok 2: Backport tylko brakujacego odczytu historii tur

Kandydat:

- `thread/turns/list`

Powod:

- CFM/NeroBar potrzebuje stabilnego, stronicowanego odczytu historii operacyjnej bez skanowania calego rollouta i bez parsowania terminala.

Pliki potencjalne:

- `codex-rs/app-server-protocol/src/protocol/common.rs`
- `codex-rs/app-server-protocol/src/protocol/v2.rs`
- `codex-rs/app-server/src/codex_message_processor.rs`
- `codex-rs/app-server/tests/suite/v2/thread_turns_list.rs`
- `codex-rs/app-server/tests/suite/v2/mod.rs`
- schema fixtures pod `codex-rs/app-server-protocol/schema/`
- `codex-rs/app-server/README.md`

Zasady:

- Endpoint tylko v2.
- Request z `threadId`, opcjonalnym `cursor`, opcjonalnym `limit`.
- Response z `data` i `nextCursor`.
- Dane musza pochodzic z istniejacej historii thread/app-server, nie z ad hoc parsowania JSONL w UI.

Weryfikacja:

- `just write-app-server-schema`
- `cargo test -p codex-app-server-protocol`
- `cargo test -p codex-app-server thread_turns_list`

### Krok 3: Zbudowac adapter app-server po stronie UI

Docelowy modul:

- NeroBar/CFM: jeden klient `CodexAppServerClient` zamiast rozproszonych fetchy.

Obowiazki adaptera:

- laczenie z app-server,
- request/response correlation,
- subskrypcja notificationow,
- normalizacja bledow,
- retry tylko dla polaczenia, nie dla mutacji,
- jawna informacja, ktory endpoint jest experimental.

Gate:

- UI potrafi pokazac `thread/read` i `fs/readFile` przez jeden adapter.
- Brak duplikacji klientow app-server w roznych panelach.

### Krok 4: Workspace reader bez wlasnego czytnika

Zrodla:

- `fs/readDirectory`
- `fs/readFile`
- `fs/watch`

Zachowanie:

- panel repo tree,
- file preview,
- markdown/text/code rendering,
- binary detection przez metadata albo safe decode,
- file watch tylko dla aktualnie otwartego pliku lub katalogu.

Zakazy:

- nie uzywac Nero `multiFileReaderCall` jako backendu UI readera,
- nie skanowac calego repo bez limitow,
- nie czytac poza project root bez jawnej walidacji.

Gate:

- otwarcie dokumentu z repo nie wymaga VS Code.
- czytnik dziala w selected CWD egzekutora.

### Krok 5: Plan i status jako strumien semantyczny

Zrodla:

- `turn/plan/updated`
- `item/plan/delta`
- `thread/read`
- potencjalnie `thread/turns/list` po backporcie.

Zachowanie:

- UI pokazuje ostatni plan modelu jako stan operacyjny, nie jako zrodlo prawdy kampanii.
- CFM moze mapowac plan modelu do sugestii/operator notes.
- Kampania nadal zamyka gate'y przez CFM Campaign MCP i canonical docs.

Gate:

- plan modelu jest widoczny obok phase workspace.
- brak automatycznego nadpisywania gate'ow kampanii przez plan modelu.

### Krok 6: Artefakty i obrazy

Zrodla:

- `imageView`
- `imageGeneration`
- `fs/readFile`

Zachowanie:

- panel artefaktow pokazuje sciezke, typ, preview i link do pliku.
- obrazy renderowane sa z istniejacych `ThreadItem`, a nie przez nowy wlasny event.
- zapisane `savedPath` jest preferowane nad surowym wynikiem, jezeli istnieje.

Gate:

- wygenerowany obraz albo otwarty obraz widac w CFM/NeroBar bez przechodzenia do terminala.

### Krok 7: Rejestr egzekutorow i kontekst CFM

Zrodla prawdy:

- globalny rejestr egzekutorow: stabilne przypisanie thread/session do repo i roli,
- CFM campaign runtime: aktywna kampania, aktywna faza, aktualny gate,
- hook Nero: dostarczenie kontekstu do egzekutora.

Zachowanie:

- UI moze pokazac liste egzekutorow dla repo.
- Jeden egzekutor moze miec role opisowa widoczna w CFM.
- Aktywna kampania moze byc wyprowadzana z CFM, ale rejestr moze trzymac jawny override, jezeli proces wymaga stalego przypisania.
- Konflikt przypisan musi byc widoczny jako stan `ambiguous`, nie jako cichy fallback.

Gate:

- hook wie, ktore repo i kampanie ma czytac.
- CFM UI widzi, ktory egzekutor obsluguje dana kampanie.

## Kolejnosc commitow implementacyjnych

1. Spec i kontrakt: dokumentacja bez runtime changes.
2. App-server inventory/test baseline.
3. Waski backport endpointu historii tur, jezeli potwierdzony jako potrzebny.
4. Adapter UI dla app-server.
5. Workspace reader read-only.
6. Plan/status panel.
7. Artefact/image panel.
8. CFM executor registry coordination.

Kazdy commit ma byc czysty tematycznie i poprzedzony `git status --short`.

## Ryzyka

- Pelny import app-server z upstream moze nadpisac Nero-only endpoints.
- `multiFileReaderCall` moze mylic jako rzekomo upstreamowy standard; w UI traktowac go jako historie narzedzia, nie jako fundament readera.
- Plan modelu moze wygladac jak proces kampanii; UI musi jasno rozdzielac model plan od CFM Campaign state.
- `thread/read` bez bogatej historii moze nie wystarczyc dla starszych sesji.
- Mutacje plikow przez UI wymagaja osobnego modelu uprawnien; pierwsza wersja workspace readera ma byc read-only.

## Kryteria akceptacji

- Spec jest zapisana i zreviewowana przed implementacja.
- Kazdy nowy endpoint app-server ma test protokolu, test app-server i aktualne schema fixtures.
- UI nie parsuje terminala jako zrodla semantycznego.
- UI nie buduje wlasnego file readera, dopoki app-server ma odpowiedni endpoint.
- CFM campaign pozostaje canonical process state.
- Tmux jest attach/runtime surface, nie state bus.

## Lokalny audyt specyfikacji

- Pokrycie wymagan: PASS. Spec obejmuje app-server, CFM, NeroBar, tmux, pliki, plan, obrazy, rejestr egzekutorow i upstream diff przed backportem.
- Granice: PASS. Dokument jawnie zabrania pelnego upstream update'u i rownoleglych readerow.
- Kontrolowalnosc: PASS. Kroki sa odseparowane na male commity z gate'ami.
- Ryzyko hardcodow: PASS. Spec wymaga czytania z runtime/project values i jawnego stanu konfliktu.
- Brak runtime changes: PASS. Ten dokument nie uruchamia zadnej implementacji.
