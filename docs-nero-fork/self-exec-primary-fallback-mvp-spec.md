# Self-Exec Primary/Fallback Binary Resolution MVP

Status: proposed, review-adjusted

Scope:

- `codex-rs/arg0`
- fork-owned self-exec resolver crate
- `codex-rs/core` config and turn-context propagation
- `codex-rs/core` apply-patch runtime
- app-server config reload / thread override paths
- fork launcher / wrapper behavior (`codexnx` side)
- fork docs in `docs-nero-fork`

## Goal

Make child self-exec flows resilient during live development on a shared `target/debug` tree.

The immediate MVP target is `apply_patch`, because it is the most visible self-exec path and currently fails when an existing long-lived process continues running from an executable inode that Cargo has already replaced.

The solution must:

- keep normal dev-first execution from the debug binary,
- stop relying on `std::env::current_exe()` as the only self-exec source,
- allow a stable out-of-tree fallback binary for child self-execs,
- preserve current behavior for ordinary users who do not use the fork launcher,
- stay narrow enough that it can be adopted now without redesigning the whole launcher/runtime stack.

## Problem

Current non-Windows `apply_patch` self-exec resolution is:

1. prefer `config.codex_self_exe` when present,
2. otherwise use `std::env::current_exe()`,
3. build child exec as `codex --codex-run-as-apply-patch <patch>`.

In a live fork workflow this breaks in a common sequence:

1. process A starts from `target/debug/codex`,
2. process B rebuilds the same binary in place,
3. process A still runs, but its `current_exe()` now resolves to a deleted inode path,
4. `apply_patch` tries to launch that deleted path and fails before the patch engine even runs.

This is operationally toxic because one rebuild can temporarily break `apply_patch` across multiple running sessions on the same worktree.

## Root cause

The bug is not the patch engine itself.

The failure happens earlier, in self-exec program selection.

`current_exe()` is an unstable source in a dev workflow where the same binary path is rebuilt in place by Cargo. It reflects the currently running executable inode, not a stable launcher-managed path contract.

## Non-goals

This MVP does not do the following:

- no hot swap of the main running process,
- no automatic rebuild orchestration inside `codex-rs`,
- no binary compatibility handshake protocol between primary and fallback,
- no full migration of every self-exec path in one pass,
- no user-config-file support for these paths,
- no attempt to redesign Windows launch semantics in this MVP.

Windows already has a dedicated launch resolution path and should remain unchanged unless explicitly extended later.

## Architectural rule

Self-exec must resolve from a launcher/runtime contract, not from process identity alone.

Meaning:

- `current_exe()` may remain a compatibility fallback,
- but it must stop being the canonical source of truth for child self-exec in the fork development workflow.

The canonical contract becomes:

- primary executable path,
- fallback executable path,
- deterministic selection order,
- explicit diagnostics for every candidate,
- one shared payload propagated through runtime wiring.

## Proposed shared payload

Introduce one in-memory runtime payload instead of propagating separate loose fields:

```rust
pub struct SelfExecPaths {
    pub primary: Option<PathBuf>,
    pub fallback: Option<PathBuf>,
}
```

This payload should be the canonical transport shape in:

- `Arg0DispatchPaths`
- `ConfigOverrides`
- `Config`
- turn context / child-turn inheritance
- app-server reload and thread override flows

Important:

- this is runtime-only,
- it must not be exposed as user-editable TOML config,
- it is the same operational class as existing executable override paths.

### Migration note

Current code uses `codex_self_exe` as a loose field meaning “path to the current executable”.

For this MVP, that meaning is tightened:

- the existing concept becomes `self_exec_paths.primary`,
- a new `self_exec_paths.fallback` is added,
- loose propagation of `codex_self_exe` must stop growing further.

If temporary bridging is needed during implementation, it must be treated as migration scaffolding only, not as the final contract.

## Launcher/runtime environment contract

The fork launcher should provide two optional absolute paths:

- `CODEXNX_SELF_BIN_PRIMARY`
- `CODEXNX_SELF_BIN_FALLBACK`

Semantics:

- `CODEXNX_SELF_BIN_PRIMARY`
  - intended path to the preferred dev binary, normally the debug binary path in the active worktree,
  - this is a path contract, not the currently running inode identity;
- `CODEXNX_SELF_BIN_FALLBACK`
  - stable fallback binary path outside Cargo-managed rebuild churn.

### Environment source policy

`arg0` currently loads `~/.codex/.env` before constructing dispatch paths and filters only `CODEX_*` variables.

Therefore MVP explicitly allows `CODEXNX_*` to come from either:

- parent process environment,
- or `~/.codex/.env`.

The runtime does not distinguish launcher-exported vs dotenv-provided values.

Operational interpretation:

- launcher is the normal producer,
- dotenv is an allowed advanced override path.

Implementation requirement:

- read with `std::env::var_os`,
- treat missing env as `None`,
- require absolute filesystem paths,
- invalid env-derived candidates do not abort startup; they are ignored and recorded in diagnostics/warnings.

## Fallback location invariants

Acceptable fallback locations:

- a launcher-managed copy under the fork root but outside `target/`,
- a launcher-managed cache directory,
- a stable dev-tools bin directory.

Unacceptable fallback locations:

- the same normalized/canonicalized path as primary,
- a path inside the same mutable Cargo `target/...` tree as primary, including symlinks that resolve back into that tree,
- a relative path,
- a non-file path.

MVP enforcement:

- same-path fallback must be rejected after canonicalized-or-normalized comparison,
- relative fallback must be rejected,
- same-target-tree fallback must be rejected with a clear warning/diagnostic after canonicalized-or-normalized comparison,
- invalid fallback disables only the fallback candidate; it must not abort the whole session.

## Effective resolution order

For non-Windows self-exec consumers, the resolver must try candidates in this order:

1. configured primary path (`self_exec_paths.primary`),
2. configured fallback path (`self_exec_paths.fallback`),
3. `current_exe()` compatibility fallback.

Selection rule:

- choose the first candidate that is configured, absolute where required, exists, and is a regular file,
- if none qualify, fail with diagnostics that enumerate every candidate and its outcome.

Reasoning:

- primary gives correct dev-path semantics even if the current process itself is running from a deleted inode,
- fallback keeps child tools alive during rebuild churn,
- `current_exe()` remains only for compatibility in legacy/non-launcher contexts.

## Resolver behavior

Introduce one central resolver in a fork-owned crate.

Suggested crate:

- folder: `codex-rs/nero-self-exec`
- crate: `codex-nero-self-exec`

Suggested model:

```rust
pub enum SelfExecProgramSource {
    ConfiguredPrimary,
    ConfiguredFallback,
    CurrentExe,
}

pub enum SelfExecCandidateDiagnostic {
    NotConfigured {
        label: &'static str,
    },
    InvalidConfiguredPath {
        label: &'static str,
        path: PathBuf,
        reason: String,
    },
    PathStatus {
        label: &'static str,
        path: PathBuf,
        exists: bool,
        is_file: bool,
        is_dir: bool,
        metadata_error: Option<String>,
    },
    LookupError {
        label: &'static str,
        error: String,
    },
}

pub struct ResolvedSelfExec {
    pub path: PathBuf,
    pub source: SelfExecProgramSource,
    pub diagnostics: Vec<SelfExecCandidateDiagnostic>,
}
```

Suggested API:

```rust
pub fn resolve_self_exec(paths: &SelfExecPaths) -> Result<ResolvedSelfExec, SelfExecResolveError>
```

Important:

- diagnostics must represent not-configured, invalid configured, metadata-based failure, and `current_exe()` lookup failure distinctly,
- the resolver diagnostics augment runtime launch diagnostics; they do not replace existing launch-context reporting.

## Apply Patch MVP behavior

`apply_patch` is the first adopter.

Required change:

- replace direct `resolve_apply_patch_program(codex_self_exe)` with resolver input that includes:
  - `self_exec_paths.primary`,
  - `self_exec_paths.fallback`,
  - `current_exe()` compatibility fallback.

New non-Windows source enum values in apply-patch launch reporting:

- `configured_self_exec_primary`
- `configured_self_exec_fallback`
- `current_exe`

### Launch diagnostics rule

`ApplyPatchLaunchContext` must remain the outer diagnostic container.

Resolver diagnostics must be embedded into it, not substituted for it.

The runtime must continue to report:

- selected source,
- pre-sandbox program path,
- pre-sandbox cwd status,
- sandbox-transformed final program path,
- filesystem metadata for the executable path(s),
- launch/exec failure details.

This avoids regressing the current diagnostic richness.

### Exec-time fallback policy

Preflight resolution alone is not enough because races can still happen between metadata check and actual spawn.

MVP exec-time policy:

- if primary was selected,
- and fallback was validated as usable during resolver pass,
- and launch fails before patch execution begins with a launch/exec filesystem error,
- retry exactly once using fallback.

Retry is allowed only for launch-stage failures, not for patch-engine failures.

Examples that qualify:

- executable missing after preflight,
- executable not accessible / permission denied,
- direct spawn/open failure before patch runtime starts.

Examples that do not qualify:

- patch verification failure,
- non-zero exit from the patch engine,
- runtime logic errors after successful child launch.

No recursive retry loops are allowed.

## Shared wiring requirement

The spec must not assume a single linear wiring path.

MVP implementation must explicitly cover all current executable-path propagation sites, including:

- `arg0` dispatch path creation,
- `ConfigOverrides -> Config` build path,
- turn-context clone/inheritance,
- app-server config reload mutation,
- app-server thread override builder,
- agent/role reload paths that rebuild config overrides.

Recommended boundary:

- define one shared mapper from `Arg0DispatchPaths` to executable overrides,
- use it everywhere instead of ad-hoc field-by-field assignment.

This is required to prevent drift between CLI, app-server, mcp-server, and child-agent flows.

## Implementation plan

1. add fork-owned resolver crate `codex-nero-self-exec`,
2. add `SelfExecPaths` to `Arg0DispatchPaths`,
3. extend `arg0` to ingest `CODEXNX_SELF_BIN_PRIMARY` and `CODEXNX_SELF_BIN_FALLBACK`,
4. add one shared mapper from `Arg0DispatchPaths` to runtime override payload,
5. propagate `SelfExecPaths` through `ConfigOverrides`, `Config`, turn context, and child-turn inheritance,
6. patch app-server reload and thread override paths to use the shared payload,
7. patch agent/role reload override generation to preserve `SelfExecPaths`,
8. switch `apply_patch` runtime to the central resolver,
9. add one-shot exec-time fallback retry for launch-stage failures,
10. preserve and extend existing apply-patch launch diagnostics.

## Test plan

### Resolver unit tests

Add focused tests for:

- primary exists -> primary selected,
- primary missing + fallback exists -> fallback selected,
- primary and fallback missing + `current_exe()` exists -> `current_exe` selected,
- all missing -> failure contains diagnostics for every candidate,
- `current_exe()` lookup failure is preserved as a distinct diagnostic,
- relative fallback is rejected,
- duplicate primary/fallback path is rejected,
- same-target-tree fallback is rejected.

### Arg0 / env ingestion tests

Add tests for:

- `CODEXNX_SELF_BIN_PRIMARY` ingestion,
- `CODEXNX_SELF_BIN_FALLBACK` ingestion,
- absolute-path requirement,
- dotenv-fed `CODEXNX_*` values being accepted the same way as process env.

### Config / propagation tests

Add tests for:

- `ConfigOverrides -> Config` carries `SelfExecPaths`,
- turn-context clone preserves `SelfExecPaths`,
- child-turn inheritance preserves `SelfExecPaths`,
- agent role reload preserves `SelfExecPaths`,
- app-server reload path reattaches `SelfExecPaths`,
- app-server thread override builder includes `SelfExecPaths`.

### Apply-patch runtime tests

Extend existing apply-patch runtime tests with:

- prefers configured primary over fallback,
- uses fallback when primary path is missing,
- reports fallback source in launch context,
- preserves outer launch diagnostics while adding candidate diagnostics,
- retries once on launch-stage failure from primary when fallback is valid,
- does not retry on patch-engine verification/runtime failures.

### Integration test

Add at least one end-to-end integration test that starts from env-derived self-exec paths and verifies the selected source observed by `apply_patch`.

A missing primary path is sufficient to prove the new selection logic; a real deleted inode is not required for MVP.

## Success criteria

This MVP is successful when:

1. `apply_patch` no longer depends solely on `current_exe()` in fork launcher flows,
2. a rebuild of the shared debug binary no longer necessarily breaks child self-exec if fallback is present,
3. failure diagnostics clearly show primary/fallback/current-exe candidate states,
4. app-server / reload / child-turn paths preserve the same executable contract instead of drifting,
5. direct non-launcher runs remain backward compatible,
6. the new policy is isolated enough that other self-exec tools can adopt it later without copying logic.

## Phase 2 explicitly deferred

After MVP, possible follow-up work includes:

- migrate other self-exec consumers onto the same resolver,
- add launcher-managed fallback refresh/update workflow,
- add compatibility or build-fingerprint checks,
- add stale-fallback warnings,
- optionally expose richer runtime telemetry about which self-exec source was used.
