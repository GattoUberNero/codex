# Delegation Report for Spawned Subagents

Status: draft under review

Scope:

- `codex-rs/protocol`
- `codex-rs/core` spawn handlers
- `codex-rs/app-server-protocol` thread-history bridge
- `codex-rs/app-server` bespoke event mapping
- `codex-rs/tui` collaboration transcript rendering

## Cel

`delegation_report` to maly, strukturalny raport tworzony przez glowny agent przed spawnem
subagenta.

Raport odpowiada na dwa praktyczne pytania: jak delegator ocenia gotowosc briefu oraz co dokladnie
ma zwrocic subagent.

To nie jest nowy scheduler, nowa lane telemetryczna ani zamiennik dla istniejacych pol spawn
prompt/model/reasoning. To jest zwiezly handoff summary, ktory ma uczynic delegacje jawnym i
audytowalnym krokiem.

## Kontrakt

### Request

`delegation_report` jest obiektem dolaczanym do requestu spawnu.

```json
{
  "general_task_type": "code change",
  "task_difficulty_1_10": 6,
  "brief_completeness_1_10": 8,
  "task_self_sufficiency_1_10": 9,
  "expected_duration_minutes": 20,
  "why_this_agent": "The task is a bounded UI edit with local rendering knowledge.",
  "expected_output_shape": "One patch plus a short verification note.",
  "files_or_scope": "codex-rs/tui/src/multi_agents.rs",
  "risks_or_unknowns": "Snapshot text may need an update if the render string changes."
}
```

Bazowy kontrakt raportu ma 9 wymaganych pol top-level. Dodatkowo moze wystapic opcjonalne
`orchestration_context`; runtime akceptuje, waliduje, zachowuje i renderuje je zawsze, gdy jest
obecne. Profil spawnu steruje tylko tym, czy ten blok trafi do promptu dziecka oraz czy
`orchestration_router_block` uczyni go wymaganym.

### Output

Na sciezce protocol/UI zachowywany jest ten sam, znormalizowany rekord raportu, a TUI renderuje go
jako czesc istniejacego collab spawn bloku.

Do promptu dziecka nie musi trafic identyczny obiekt. Forward do subagenta jest profile-dependent:
moze byc wylaczony, moze uzyc przefiltrowanej projekcji raportu albo moze zostac zastapiony przez
router-generated bridge block.

Kolejnosc pol musi byc stabilna i zgodna z requestem:

1. `general_task_type`
2. `task_difficulty_1_10`
3. `brief_completeness_1_10`
4. `task_self_sufficiency_1_10`
5. `expected_duration_minutes`
6. `why_this_agent`
7. `expected_output_shape`
8. `files_or_scope`
9. `risks_or_unknowns`

Jesli aktywny profil forwarduje kontekst delegacji, raport musi byc dostepny przed startem pracy
subagenta, tak aby delegator mogl jeszcze poprawic brief, gdy koszt zmiany jest nadal niski.

## Walidacje

### Wymagany ksztalt runtime

- 9 bazowych pol jest wymaganych, gdy `delegation_report` jest obecny.
- `general_task_type`, `why_this_agent` i `expected_output_shape` nie moga byc pustymi stringami.
- `files_or_scope` i `risks_or_unknowns` nie moga byc pustymi stringami.
- Pola numeryczne musza byc integerami w zakresie `1..=10`.
- `expected_duration_minutes` musi byc dodatnim integerem.
- `orchestration_context`, jesli wystepuje, jest walidowany osobno jako opcjonalne rozszerzenie z
  twardymi regułami per field.
- Obecny runtime waliduje ksztalt i requiredness zgodnie z aktywnym profilem; nie wykonuje
  semantycznej oceny, czy zadanie jest rzeczywiscie gotowe do spawnu.

### Wskazowki gotowosci spawnu

Zadanie samo w sobie musi byc pelne, jasne i samowystarczalne.

To znaczy:

- subagent moze wystartowac bez rutynowych pytan doprecyzowujacych,
- wie, co ma zrobic i po czym poznac sukces,
- scope jest wystarczajaco waski, aby jeden agent mogl go wziac na wlasnosc,
- brief nie ukrywa kluczowych ograniczen w pobocznej historii rozmowy.

Niskie `brief_completeness_1_10` albo `task_self_sufficiency_1_10` powinny byc traktowane jako
sygnal, ze brief warto dosycic albo rozbic przed spawnowaniem. To jest guidance dla delegatora, a
nie osobny semantic gate wymuszany przez obecny runtime. Nie wolno maskowac slabego briefu ogolnym
raportem.

### Semantyka pol

- `general_task_type` powinno byc krotka etykieta, a nie proza.
- `task_difficulty_1_10` mierzy koszt wykonania i koordynacji, nie pilnosc.
- `brief_completeness_1_10` mierzy, ile potrzebnego kontekstu juz jest w briefie.
- `task_self_sufficiency_1_10` mierzy, czy subagent moze dojsc do konca bez dodatkowego
  doprecyzowania.
- `expected_duration_minutes` powinno odzwierciedlac realny wycinek pracy, a nie zyczeniowy plan.
- `why_this_agent` musi wyjasniac dopasowanie, a nie tylko powtarzac nazwe typu agenta.
- `expected_output_shape` powinno nazwac forme deliverable, na przyklad patch, checklist,
  findings table albo summary.
- `files_or_scope` powinno byc dosc precyzyjne, zeby ograniczyc read/write scope; wpisy moga
  byc plikami, katalogami, modulami albo jednym bounded scope phrase, ale nie moga byc ogolne.
- `risks_or_unknowns` powinno wymieniac konkretne niewiadome albo edge case'y; jesli nic nie
  wiadomo, wpisz `none known`.

## Backward Compatibility

- `delegation_report` jest dodatkiem.
- Starzy callerzy moga go pominac.
- Starzy consumerzy musza go ignorowac, jesli sie pojawi.
- Istniejace payloady spawn begin/end pozostaja wazne bez tego pola.
- Nie wolno inferowac ani syntetyzowac raportu z `prompt`, `model` lub `reasoning_effort`.
- Nie wolno zmieniac znaczenia `context_inheritance_requested` ani innych istniejacych pol
  spawnu.

Replay behavior:

- jesli raport zostal wyslany, replay powinien go zachowac;
- jesli nie zostal wyslany, replay nie moze go wymyslac.

## TUI Rendering Rules

- Renderuj raport wewnatrz istniejacego spawn bloku, nie jako osobna lane.
- Nie renderuj surowego JSON-a.
- Spawn title pozostaje pierwszoplanowy, a raport jest drugoplanowy.
- Najpierw pokazuje sie jednozdaniowy summary, potem linie detali.
- Zachowaj kolejnosc pol z kontraktu.
- Dlugie wartosci wrapuj istniejacymi helperami do text wrapping.
- Nie ucinaj `risks_or_unknowns`, jesli da sie je sensownie zwinac na kolejne linie.
- Uzywaj tej samej stonowanej/stylizowanej prezentacji metadanych, co obecny blok spawn metadata.
- Jesli raportu nie ma, nie renderuj nic dodatkowego.
- Jesli walidacja failuje, pokaz powod walidacji zamiast czesciowo zaufanego raportu.

Suggested summary line:

`Delegation: <general_task_type> | difficulty X/10 | brief Y/10 | self Z/10 | ~N min`

## Current Implementation Shape

1. Dodac `delegation_report` jako opcjonalne spawn metadata na istniejacej collab spawn begin path.
2. Przepchnac je przez obecny thread-history bridge i TUI spawn summary bez tworzenia nowej
   rodziny eventow.
3. Uzyc obecnego `spawn_begin` / `spawn_end` path i istniejacego layoutu detali.
4. Zostawic shape end-eventu bez zmian, chyba ze replay bedzie potrzebowal opaque echo do
   odtwarzania.
5. Nie dotykac pozostalych reporting lanes.

## Current Success Criteria

Ta specyfikacja jest spelniona, gdy:

- delegator moze opisac pelny handoff w jednym strukturalnym bloku,
- subagent widzi dokladnie, co ma zrobic, jesli aktywny profil forwarduje kontekst delegacji,
- UI moze pokazac raport bez wymyslania nowej machinerii prezentacyjnej,
- brak raportu nie zmienia istniejacego zachowania spawnu,
- a niskie score'y pozostaja czytelnym sygnalem dla delegatora, zamiast byc ukryte w historii
  rozmowy.
