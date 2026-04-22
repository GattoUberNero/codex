# Model Catalog Runtime Overlay Spec

## Problem

The fork currently ships its base model catalog from `codex-rs/core/models.json`, but that file is compiled into the binary through `include_str!`.

This creates two practical problems:

1. adding or overriding model definitions requires rebuilding the binary,
2. upstream can publish new model metadata independently of code changes, but the fork cannot selectively ingest those catalog changes without rebuilding a new runtime.

At the same time, the current TUI model picker is already driven dynamically from `model/list`; the blocker is the catalog source, not the picker itself.
This is not a picker-only change: the active catalog also defines runtime model metadata, instructions, and capability flags through full `ModelInfo`.

## Current state

- `model_catalog_json` already exists in config.
- Its semantics are **full replacement**:
  - when set, it replaces the bundled catalog for the current process,
  - it disables the normal bundled-base + refresh mutation behavior in `ModelsManager`.
- `ModelsManager` currently rebuilds its mutable catalog by reloading bundled `models.json` and applying cache/remote updates on top.

## Goal

Add a **runtime overlay catalog** that lets the fork:

- keep bundled `models.json` as fallback base,
- optionally load a local JSON file at startup,
- merge that file on top of the bundled catalog,
- keep the existing cache/network refresh behavior,
- surface new/overridden models through existing `model/list` and TUI flows,
- preserve the full `ModelInfo` contract for runtime instructions and capability metadata.

Terminology note:

- `model_catalog_json` remains an authoritative replacement source,
- `model_catalog_overlay_json` is a startup seed layered onto the bundled catalog,
- overlay is not a permanent winner over later cache/remote data for the same `slug`.

## Non-goals

- No hot reload in MVP.
- No per-thread live reapplication.
- No change to `model_catalog_json` semantics.
- No redesign of TUI model grouping beyond existing dynamic picker behavior.
- No attempt to externalize every model-related instruction surface in this pass.

## Proposed config contract

Add a new startup-only config field:

- `model_catalog_overlay_json: Option<AbsolutePathBuf>`

Placement:

- `ConfigToml`
- `ConfigProfile`
- resolved `Config`

Resolved `Config` shape:

- keep `model_catalog: Option<ModelsResponse>` as full replacement catalog,
- add `model_catalog_overlay: Option<ModelsResponse>` for overlay content.

Critical invariant:

- overlay content must never be mapped into `model_catalog`,
- `model_catalog` remains the existing full-replacement path only,
- otherwise `ModelsManager` enters `CatalogMode::Custom` and disables refresh, which would violate the overlay goal.

## Validation rules

`model_catalog_overlay_json`:

- must point to valid JSON deserializable as `ModelsResponse`,
- must contain at least one model,
- is startup-only, same operational class as `model_catalog_json`.

Loader requirement:

- the JSON loader must parameterize its field name in validation errors,
- overlay failures must mention `model_catalog_overlay_json`,
- replacement failures must keep mentioning `model_catalog_json`.

Conflict rule:

- `model_catalog_json` and `model_catalog_overlay_json` are mutually exclusive,
- config load fails if both are set.
- this validation must run on the effective resolved values after profile + global selection, not only on raw per-layer fields.

Reason:

- this keeps the model source contract explicit,
- avoids ambiguous precedence,
- avoids silently composing two separate authoritative local sources.

## Merge semantics

### Authoritative replacement path (no refresh)

If `model_catalog_json` is set:

- current behavior remains unchanged,
- bundled catalog is not used as base,
- overlay config must not be accepted.

### Bundled base + local overlay path (refreshable)

If `model_catalog_json` is not set:

- load bundled `models.json`,
- if `model_catalog_overlay_json` is present, merge overlay models onto bundled models by `slug`,
- if slug exists in bundled catalog, replace that model entry,
- if slug does not exist, append it,
- resulting merged catalog becomes the manager base catalog for this process.

Precedence in this path:

- bundled catalog,
- then local overlay seed,
- then cache/remote refresh updates.

Product interpretation:

- overlay is a startup-local way to seed or patch the base catalog,
- remote/cache data may still replace the same `slug` later when refresh happens,
- this is intentional and must be documented as part of the contract.

### Cache/remote refresh path

Current refresh behavior should continue, but it must use the already-built base catalog instead of reloading bundled `models.json` directly.

Required change:

- `ModelsManager` stores a `base_catalog: Vec<ModelInfo>`,
- `remote_models` is initialized from:
  - full replacement catalog, or
  - bundled + overlay merged base catalog,
- `apply_remote_models()` merges fetched/cache models onto `base_catalog`, not onto a fresh reload of bundled `models.json`.
- `ThreadManager` must pass both full replacement and overlay data into the manager constructor or equivalent builder API.

Reason:

- otherwise overlay models disappear after the first cache or remote refresh.
- the manager must also preserve existing compatibility semantics:
  - `CatalogMode::Custom` continues to disable refresh,
  - non-custom mode continues to allow cache/network mutation on top of its base catalog.

## Runtime metadata implications

Overlay content is not limited to picker visibility.

Because the runtime consumes full `ModelInfo`, overlay data must be treated as authoritative for:

- `base_instructions`,
- `model_messages`,
- capability flags,
- reasoning metadata,
- visibility and picker behavior metadata.

MVP must therefore validate at least one runtime path where an overlaid model is selected and its effective metadata is used beyond `model/list`.

## Picker / TUI implications

Expected behavior:

- no new TUI picker source is needed,
- `model/list` continues to be the single runtime source for the picker,
- newly overlaid models appear automatically if they survive visibility/auth filtering.

Known exception:

- auto-model quick grouping still has explicit logic for `codex-auto-*`,
- this is acceptable for MVP because it does not block normal model availability in the full picker.
- some TUI startup and warning flows still contain slug-specific logic,
- this MVP does not remove those hardcoded branches; it only ensures the full picker and startup list source remain driven by `model/list`.

## Testing scope

### Config tests

Add tests for:

- `model_catalog_overlay_json` loads from path,
- empty overlay fails,
- `model_catalog_json` + `model_catalog_overlay_json` together fail with explicit error,
- conflict is rejected both for direct config and for profile-vs-global combinations.

### ModelsManager tests

Add tests for:

- overlay overrides a bundled model by slug,
- overlay appends a new model absent from bundled catalog,
- cache/remote refresh preserves overlay-only entries when unrelated remote updates arrive,
- cache/remote refresh can still override an overlaid slug if remote data for that slug arrives,
- full replacement mode still blocks refresh exactly as it does today.

### Integration expectation

No dedicated TUI behavior change is required if `model/list` already reflects the merged catalog, but at least one app-server or manager integration test should prove that an overlay-added visible model can be listed.

### Offline helper and harness expectation

The implementation must cover offline and test-only helpers that currently assume either:

- bundled catalog only, or
- `config.model_catalog` only.

Add at least one regression test proving overlay metadata is visible through offline helper paths used by tests and non-refresh startup flows.

### Runtime metadata expectation

Add at least one regression test proving that an overlaid model definition affects real runtime metadata selection, not only picker visibility.

## Documentation updates

Update:

- config schema,
- config comments,
- any user-facing docs that currently describe `model_catalog_json` without mentioning the new overlay path.

## MVP success criteria

The change is complete when:

1. a user can point config at `model_catalog_overlay_json`,
2. the runtime starts from bundled catalog + overlay without replacing the bundled base,
3. refreshed catalogs do not erase overlay content,
4. `model/list` returns overlay-added or overlay-overridden models,
5. at least one runtime path uses overlaid metadata correctly after model selection,
6. `model_catalog_json` keeps its existing full-replacement semantics.
