# Structural Architecture

## 1. Repository Topology

The `codex-nero` ecosystem currently spans three active repositories:

```text
codex-nero
  Rust workspace fork of Codex CLI/TUI/app-server/core/hooks/protocol

codex-nero-sdk
  Python hook router + organizer runtime + runtime-control helper layer

nerobar-ui
  Node backend + frontend operator control plane + launch wrappers
```

High-level dependency direction:

```text
nerobar-ui
  -> codex-nero app-server RPC
  -> codex-nero-sdk helper subprocesses

codex-nero-sdk
  <-> codex-nero legacy notify/action contract
  -> local config/state/log overlays

codex-nero
  -> upstream Codex crates and fork-specific patches
```

The intended primary operator-control rule is:

- UI orchestrates runtime and operator workflows,
- SDK adapts and composes policy,
- Rust runtime executes.

Important runtime caveat:

- the live hook integration between `codex-nero` and `codex-nero-sdk` is bidirectional at runtime.
- `codex-nero` invokes external notify handlers,
- `codex-nero-sdk` returns action payloads back into `codex-nero`,
- so the hook channel is a feedback loop even though operator authority should remain structurally layered.

## 2. Runtime Process Topology

The important live processes are:

```text
Codex TUI / codexn / codexnx
  -> embedded or standalone app-server client flow
  -> Codex core runtime
  -> hook runtime

Python hook router/runtime process
  -> invoked from hook compatibility path
  -> invoked from runtime bridge helper commands

NeroBar backend
  -> spawns codex app-server subprocesses
  -> spawns SDK helper subprocesses
  -> serves frontend APIs

NeroBar frontend
  -> operator UI for sessions, runtime, lifecycle, config, and accounts
```

The main control channels are:

- app-server JSON-RPC between `nerobar-ui` backend and `codex-nero`
- legacy hook `notify` input/output between `codex-nero` and `codex-nero-sdk`
- subprocess bridge commands between `nerobar-ui` backend and `codex-nero-sdk`

## 3. `codex-nero` Structural Map

## 3.1 Core execution and hook integration

Key areas:

- `codex-rs/core/src/codex.rs`
  - high-touch fork integration point
  - runtime session-auto behavior
  - hook action execution
  - delivery throttling and gating
- `codex-rs/core/src/hook_runtime.rs`
  - native hook integration seam around session/turn/tool lifecycle
- `codex-rs/core/src/config/mod.rs`
  - config overlay handling and Nero runtime config resolution

This is where the fork currently concentrates most cross-cutting behavior.

## 3.2 Native hook engine

Key areas:

- `codex-rs/hooks/src/engine/config.rs`
- `codex-rs/hooks/src/engine/discovery.rs`
- `codex-rs/hooks/src/engine/dispatcher.rs`
- `codex-rs/hooks/src/events/*.rs`
- `codex-rs/hooks/src/response.rs`
- `codex-rs/hooks/src/user_notification.rs`

Structural split:

- native hook engine:
  - event discovery
  - matcher/dispatcher
  - run summaries
  - lifecycle event contracts
- compatibility layer:
  - legacy notify support
  - tolerant action parsing
  - Nero-specific action variants

## 3.3 App-server and protocol

Key areas:

- `codex-rs/app-server-protocol/src/protocol/v2.rs`
- `codex-rs/app-server/src/codex_message_processor.rs`
- `codex-rs/app-server/src/bespoke_event_handling.rs`
- `codex-rs/app-server/src/thread_rollout_trim.rs`
- `codex-rs/app-server/src/in_process.rs`

Structural roles:

- protocol crate defines wire contract,
- message processor routes app-server methods,
- in-process host powers app-server TUI/default runtime,
- bespoke event mapping turns core events into app-server notifications,
- rollout trim module is the fork’s selected-session maintenance extension.

## 3.4 TUI/session state

Key areas:

- `codex-rs/tui/src/app.rs`
- `codex-rs/tui/src/chatwidget.rs`
- `codex-rs/tui/src/history_cell.rs`
- `codex-rs/tui/src/app_server_session.rs`

Structural roles:

- `app.rs`
  - main TUI orchestration
  - hotkeys
  - runtime bridge interaction
- `chatwidget.rs`
  - session state projection from app-server events into renderable UI state
- `history_cell.rs`
  - visible rendering of hook/runtime messages
- `app_server_session.rs`
  - session model ingestion from app-server session events

## 3.5 Plugin and app surfaces

Key areas:

- `codex-rs/core/src/plugins/manager.rs`
- `codex-rs/core/src/plugins/store.rs`
- `codex-rs/core/src/plugins/injection.rs`
- `codex-rs/core/src/plugins/render.rs`

Structural role:

- first-class extension and capability loading path that can absorb future behavior currently hard-coded in the fork.

## 4. `codex-nero-sdk` Structural Map

## 4.1 Router boundary

Key areas:

- `router.py`
- `nero_hook_router/cli.py`
- `nero_hook_router/adapter.py`
- `nero_hook_router/config.py`
- `nero_hook_router/compat_legacy_notify.py`

Structural role:

- stable local process boundary for Rust -> Python hook calls.

The router is intentionally thin:

- receive payload,
- normalize,
- load config,
- call runtime,
- validate/normalize actions,
- return JSON.

## 4.2 Organizer runtime

Key areas:

- `nero_hook_runtime/entrypoints.py`
- `nero_hook_runtime/session_auto_bridge.py`
- `nero_hook_runtime/state_runtime_control.py`

Structural role:

- high-level composition layer for:
  - campaign-aware reminders,
  - auto policy,
- runtime note text,
- runtime control reads and writes.

Current boundary note:

- `session_auto_bridge.py` is the active CLI entrypoint for session-auto reads and writes.
- `state_runtime_control.py` remains the hidden single-writer implementation surface behind that entrypoint.

This is the most replaceable and fastest-moving layer in the whole system.

## 4.3 SDK documentation and contracts

Key docs already present in the SDK repo:

- `doc/00-codex-nero-framwork.md`
- `doc/02-architecture-and-separation.md`
- `doc/03-hook-contracts.md`
- `doc/09-implementation-map.md`

These are important historical/operational references, but the present directory documents the architecture from the `codex-nero` lane perspective.

## 5. `nerobar-ui` Structural Map

## 5.1 Backend

Key area:

- `backend/src/server.js`

Structural role:

- operator API aggregator and orchestration boundary.

It owns:

- launching app-server subprocesses,
- calling app-server RPCs,
- launching SDK helper subprocesses,
- maintaining local overlays for session registry/orphaned/cleanup flows,
- job registries for long-running analyze/trim operations.

## 5.2 Frontend

Key areas:

- `frontend/src/components/CodexNeroSessionView.tsx`
- `frontend/src/components/codexNeroSessionModel.ts`
- `frontend/src/config/api.ts`

Structural role:

- the operator-facing representation of session lifecycle, runtime controls, rollout trim, and related safety workflows.

## 5.3 Launch wrappers

Key areas:

- `bin/codexnx`
- `scripts/start.sh`

Structural role:

- connect operator shell/startup behavior to the local fork/runtime.

This is also where the control plane intentionally prefers the local `codex-nero` binary over a generic globally installed Codex binary.

## 6. Control Channels

## 6.1 Hook channel

```text
codex-nero core
  -> legacy notify payload
  -> codex-nero-sdk router/runtime
  -> actions[] JSON
  -> codex-nero action execution and TUI delivery
```

This is the most fork-specific runtime channel.

It is structurally bidirectional:

- `codex-nero` emits/invokes,
- `codex-nero-sdk` composes and returns,
- `codex-nero` executes the returned actions.

## 6.2 App-server channel

```text
nerobar-ui backend
  -> initialize
  -> thread/config/plugin/session RPCs
  -> hook notifications / thread events / rollout job requests
  -> codex-nero app-server
```

This is the most upstream-compatible control channel.

## 6.3 Runtime bridge channel

```text
nerobar-ui backend
  -> spawn SDK helper command
  -> read/apply session-auto state
  -> normalized status or mutation result
```

This exists because runtime auto control still lives partly in fork-specific semantics.

## 7. State, Config, and Log Boundaries

## 7.1 Upstream Codex state

Examples:

- `~/.codex/config.toml`
- `~/.codex/sessions/...`
- `~/.codex/archived_sessions/...`
- upstream auth state and plugin/app state under Codex home

Owned by:

- Codex runtime and upstream extension systems.

## 7.2 Fork overlay config

Examples:

- `config-nero.toml`
- `config-nero-hook-msg.toml`
- `config-nero-hook-auto.toml`

Owned by:

- SDK runtime/config translation,
- read and partially surfaced by UI/operator flows.

## 7.3 Fork runtime logs and overlays

Examples:

- hook router logs
- hook delivery audits
- session registry
- forgotten/orphaned local lifecycle overlays
- rollout trim backup state

Owned by:

- SDK and `nerobar-ui` operational model,
- with app-server trim backups physically managed by `codex-nero`.

## 8. Structural Pressure Points

These are the places where upgrades and refactors are riskiest:

1. `codex-rs/core/src/codex.rs`
   - too many responsibilities converge here.
2. Legacy notify compatibility plus custom action parsing.
3. TUI-local runtime bridge behavior in `tui/src/app.rs`.
4. Cross-repo assumptions about config overlays and runtime authority.
5. Node job/status wrappers around heavy app-server operations.

## 9. Structural Direction

The structure suggests this target shape:

- keep `codex-nero` focused on runtime, protocol, hooks, plugins, and execution-safe file/session operations,
- keep `codex-nero-sdk` focused on fast-changing organizer policy,
- keep `nerobar-ui` focused on operator workflows and orchestration,
- reduce deep coupling through:
  - explicit app-server session/runtime contracts,
  - native hook run summaries and notifications,
  - plugin/app extension surfaces,
  - fewer fork-only side channels.
