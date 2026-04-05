# Codex-rs Fork Surface Atlas

Purpose: authoritative, current map of fork-owned `codex-rs` capability surfaces and the external contracts exposed from `codex-rs`.

## Ownership Legend

- `NATIVE`: upstream Codex/runtime surface that Nero depends on but does not own.
- `NERO-INTERNAL`: fork-owned implementation/state internal to `codex-rs`.
- `NERO-EXPOSED`: fork-owned contract exported from `codex-rs` (wire, RPC, protocol field, event payload, env/config interface).
- `EXTERNAL-CONSUMER`: system outside `codex-rs` consuming exposed contracts (for this map: `codex-nero-sdk`, `nerobar-ui`, operator command surface).

## Top-Level Capability Matrix

| Surface | Primary outcome | Ownership | Primary carriers | Primary modules |
| --- | --- | --- | --- | --- |
| Hook Msg | Runtime msg composition and delivery decisions for turn lifecycle | `NERO-EXPOSED` + `NERO-INTERNAL` on `NATIVE` hooks | `NeroHookAction::NeroHookMsg`, hook action envelope | `hooks/src/response.rs`, `core/src/codex.rs`, `hooks/src/user_notification.rs` |
| Hook Auto | Stop-centric auto continuation decision and enqueue contract | `NERO-EXPOSED` + `NERO-INTERNAL` on `NATIVE` STOP | `NeroHookAction::AutoUserReply`, stop checkpoint contract meta | `hooks/src/response.rs`, `core/src/codex.rs`, `tui/src/chatwidget.rs` |
| User-visible warning/reporting lane | User-visible runtime status and warning rendering | Carrier `NATIVE`, formatter/control `NERO-INTERNAL`, output `NERO-EXPOSED` | `EventMsg::Warning`, `HookCompletedEvent`, nero warning formatter/delivery | `core/src/codex.rs`, `tui/src/chatwidget.rs` |
| Session-auto authority / F1-F5 | Runtime auto parameter changes through authority path | `NERO-EXPOSED` + `NERO-INTERNAL` on `NATIVE` protocol stream | F1-F5 hotkeys, app-server `thread/sessionAuto/*`, protocol `nero_auto_runtime` | `tui/src/chatwidget.rs`, `tui/src/app.rs`, `app-server/src/codex_message_processor.rs`, `app-server/src/thread_session_auto.rs`, `app-server-protocol/src/protocol/common.rs` |
| Dynamic account switching / auth rotation | Recovery from quota/usage-limit through controlled rotate command path | `NERO-EXPOSED` + `NERO-INTERNAL` | `CODEXN_AUTH_ROTATE_CMD*`, `CODEXN_ROTATION_REASON` | `core/src/client.rs` |
| Model fallback | Controlled model ladder switching on eligible model failures | `NERO-EXPOSED` config + `NERO-INTERNAL` runtime state | `[nero.model_fallback]`, fallback runtime state/methods | `core/src/config/mod.rs`, `core/src/codex.rs`, `core/src/state/session.rs` |
| Native hook dependency surfaces | Native lifecycle checkpoints and completion summary used by Nero capabilities | `NATIVE` | `HookEventName::{Stop, AfterAgent, AfterCompaction}`, `HookCompletedEvent` | `protocol/src/protocol.rs`, runtime hook dispatch in `core/src/codex.rs` |
| Runtime/config authority boundary | External control/config ingress and bridge authority layer | `NERO-EXPOSED` + `NERO-INTERNAL` | app-server RPC, protocol session fields, env/config overlays, bridge module | `app-server/src/thread_session_auto.rs`, `core/src/config/mod.rs`, `core/src/codex.rs`, `protocol/src/protocol.rs` |

## Detailed Surfaces

### 1) Hook Msg

- Purpose: carry structured runtime messaging decisions from hook action output into agent/user delivery flow.
- Active modules:
  - `codex-rs/hooks/src/response.rs`
  - `codex-rs/hooks/src/user_notification.rs`
  - `codex-rs/core/src/codex.rs`
- Active Rust symbols:
  - `NeroHookAction::NeroHookMsg`
  - `NeroHookMsgMode`, `NeroHookMsgShow`, `NeroHookMsgContent`, `NeroHookMsgFormat`, `NeroHookMsgStatus`
  - `parse_nero_hook_actions_from_stdout(...)`
- Exact external inputs:
  - Hook action envelope `{"actions":[...]}`
  - Action wire `type: "nero_hook_msg"`
  - Mode values: `"synced"`, `"tui-short"` (`"tui_short"` alias accepted)
  - Format values: `"block"`, `"inline"`
- Exact external outputs:
  - Runtime-applied msg action in after-agent processing
  - User-visible status blocks and/or summary entries through the reporting lane
- Runtime state ownership:
  - Turn-local delivery counters and status assembly in `core/src/codex.rs`
- Native dependencies:
  - Hook lifecycle events and native summary/event transport
- External consumers:
  - `codex-nero-sdk` (producer via hook output contract)
  - TUI/user surface (consumer of rendered result)
- Branding status:
  - Capability symbols are branded (`Nero*`, `nero_*`)
- Confirmed residues:
  - none for this surface in current active path

### 2) Hook Auto

- Purpose: stop-centric auto continuation control based on strict hook output and delivery contract.
- Active modules:
  - `codex-rs/hooks/src/response.rs`
  - `codex-rs/core/src/codex.rs`
  - `codex-rs/tui/src/chatwidget.rs`
- Active Rust symbols:
  - `NeroHookAction::AutoUserReply`
  - stop contract meta assembly in `core/src/codex.rs` (`stop_checkpoint_expected`, `stop_checkpoint_delivered`, `contract_satisfied`)
- Exact external inputs:
  - Hook action `type: "auto_user_reply"`
  - STOP hook lifecycle checkpoint
- Exact external outputs:
  - Auto user reply enqueue decision path
  - Hook completion metadata consumed by TUI (`protocol.stop_checkpoint_expected`, `protocol.stop_checkpoint_delivered`)
- Runtime state ownership:
  - Session runtime config + turn-local follow-up counters in `core/src/codex.rs`
- Native dependencies:
  - `HookEventName::Stop`
  - `HookCompletedEvent`
- External consumers:
  - TUI status rendering
  - downstream runtime continuation behavior
- Branding status:
  - Branded action and stop-centric meta naming are active
- Confirmed residues:
  - none in active `msg/auto` action set

### 3) User-visible warning/reporting lane

- Purpose: render fork runtime status/messages to the user without changing native event carriers.
- Active modules:
  - `codex-rs/core/src/codex.rs`
  - `codex-rs/tui/src/chatwidget.rs`
- Active Rust symbols:
  - `nero_hook_tui_warning_message(...)`
  - `nero_hook_tui_delivery(...)`
  - `NeroHookAction::VisibleNote`
- Exact external inputs:
  - Hook action `type: "visible_note"`
  - Hook runtime status/meta produced in after-agent path
- Exact external outputs:
  - `EventMsg::Warning(WarningEvent { message })`
  - `HookCompletedEvent` entries/meta shown in TUI
- Runtime state ownership:
  - Core owns composition and emission; TUI owns projection/render state
- Native dependencies:
  - `EventMsg::Warning`
  - `HookCompletedEvent`
- External consumers:
  - User/TUI
- Branding status:
  - Nero formatter/delivery functions are branded; carriers remain native by design
- Confirmed residues:
  - none required for active behavior

### 4) Session-auto authority / F1-F5

- Purpose: deterministic runtime parameter updates through authority-backed session-auto controls.
- Active modules:
  - `codex-rs/tui/src/chatwidget.rs`
  - `codex-rs/tui/src/app.rs`
  - `codex-rs/app-server/src/codex_message_processor.rs`
  - `codex-rs/app-server/src/thread_session_auto.rs`
  - `codex-rs/app-server-protocol/src/protocol/common.rs`
  - `codex-rs/protocol/src/protocol.rs`
- Active Rust symbols:
  - `detect_nero_auto_hotkey_action(...)`
  - `AppEvent::ApplyNeroAutoHotkey`
  - `handle_nero_auto_hotkey_event(...)`
  - `NeroThreadSessionAutoContext`
  - `NeroBridgeReadRequest`, `NeroBridgeApplyRequest`
- Exact external inputs:
  - F1-F5 key actions
  - RPC methods: `thread/sessionAuto/read`, `thread/sessionAuto/update`
  - update request contract:
    - required: `expectedVersion`
    - optional: `expectedSessionSource`
  - read/update response contract carries `authority` and `state`
  - Protocol field: `nero_auto_runtime`
  - Bridge module: `nero_hook_runtime.session_auto_bridge`
- Exact external outputs:
  - `ThreadSessionAutoReadResponse { authority, state }`
  - `ThreadSessionAutoUpdateResponse { authority, applied, conflict, message, error_code, reason_code, state }`
  - authority enum/value contract used by both responses: `ThreadSessionAutoAuthorityMode::BridgeProxy`
  - Updated runtime state in session snapshots/updates
  - User-visible authority feedback in TUI
- Runtime state ownership:
  - Source of authority: app-server + protocol session state
  - Runtime application: core session configuration
- Native dependencies:
  - Native app-server RPC transport
  - Native protocol event stream
- External consumers:
  - `nerobar-ui` backend
  - TUI runtime controls
- Branding status:
  - Authority/context/bridge symbols in this lane are branded
- Confirmed residues:
  - One retained compatibility alias inside bridge module resolution: retired module alias `nero_hook_runtime.state_runtime_control` (normalization only; active default remains `nero_hook_runtime.session_auto_bridge`)
  - Validation guard: app-server rejects empty `expectedVersion` on update requests

### 5) Dynamic account switching / auth rotation

- Purpose: recover from quota/usage-limit failures via explicit rotation command boundary.
- Active modules:
  - `codex-rs/core/src/client.rs`
- Active Rust symbols:
  - `try_recover_with_nero_auth_rotate_command(...)`
  - recovery flow around usage/quota handling in client request path
- Exact external inputs:
  - `CODEXN_AUTH_ROTATE_CMD`
  - `CODEXN_AUTH_ROTATE_CMD_TIMEOUT_MS`
  - recognized recovery reasons (`usage_limit_reached`, `quota_exceeded`)
- Exact external outputs:
  - child process invocation with `CODEXN_ROTATION_REASON`
  - auth reload + retry attempt if recovery succeeds
- Runtime state ownership:
  - model client session tracks recovery budget/attempt lifecycle
- Native dependencies:
  - upstream auth manager reload and API error classification
- External consumers:
  - operator-provided rotation command endpoint
- Branding status:
  - rotation entrypoint symbols are branded for fork-owned behavior
- Confirmed residues:
  - none in primary recovery path

### 6) Model fallback

- Purpose: move to ladder-defined backup models on eligible model failure conditions.
- Active modules:
  - `codex-rs/core/src/config/mod.rs`
  - `codex-rs/core/src/codex.rs`
  - `codex-rs/core/src/state/session.rs`
- Active Rust symbols:
  - `NeroModelFallbackConfig`, `NeroModelFallbackStep`, `NeroModelFallbackResolution`
  - `read_nero_model_fallback(...)`
  - `resolve_nero_model_fallback_from_env(...)`
  - `apply_model_fallback_pre_turn(...)`
  - `try_model_fallback_after_error(...)`
  - `mark_model_fallback_success(...)`
- Exact external inputs:
  - config namespace `[nero.model_fallback]`
  - keys: `enabled`, `cooldown_seconds`, `max_wait_seconds`, `sticky`, `ladder`
  - config overlay env entrypoints (Nero config overlay lane)
- Exact external outputs:
  - runtime model switch decisions prior to and after turn errors
  - fallback telemetry/audit entries in runtime path
- Runtime state ownership:
  - `NeroModelFallbackRuntimeState` in session state
- Native dependencies:
  - model turn execution and error handling loop
- External consumers:
  - runtime turn processing (internal consumer of config contract)
- Branding status:
  - capability and config/runtime symbols are branded
- Confirmed residues:
  - none required for active fallback behavior

### 7) Native hook dependency surfaces

- Purpose: define the native lifecycle/checkpoint surfaces that Nero capabilities depend on.
- Active modules:
  - `codex-rs/protocol/src/protocol.rs`
  - `codex-rs/core/src/codex.rs`
- Active Rust symbols:
  - `HookEventName::{Stop, AfterAgent, AfterCompaction}`
  - `HookCompletedEvent`
- Exact external inputs:
  - Native lifecycle event dispatch from runtime hook engine
- Exact external outputs:
  - Standardized hook run summaries/events consumed by TUI and app-server pathways
- Runtime state ownership:
  - native runtime hook execution state
- Native dependencies:
  - this surface itself is native (dependency layer, not fork capability)
- External consumers:
  - Nero fork features (`hook msg`, `hook auto`, reporting lane)
- Branding status:
  - native names intentionally unchanged
- Confirmed residues:
  - none

### 8) Runtime/config authority boundary

- Purpose: govern where fork runtime behavior can be configured/updated from outside `codex-rs`.
- Active modules:
  - `codex-rs/core/src/config/mod.rs`
  - `codex-rs/core/src/codex.rs`
  - `codex-rs/app-server/src/thread_session_auto.rs`
  - `codex-rs/protocol/src/protocol.rs`
- Active Rust symbols:
  - Nero config resolver/read functions for fallback and auto runtime
  - bridge resolution/settings helpers in app-server thread session auto lane
- Exact external inputs:
  - RPC: `thread/sessionAuto/read`, `thread/sessionAuto/update`
  - protocol session field: `nero_auto_runtime`
  - env/config ingress:
    - `CODEXN_ROOT` (bridge bootstrap/default cwd resolution)
    - `CODEXN_CONFIG_NERO_PATH`
    - `CODEXN_CONFIG_NERO_MSG_PATH`
    - `CODEXN_CONFIG_NERO_AUTO_PATH`
    - `CODEXN_CONFIG_NERO_DEV_PATH`
    - bridge/runtime envs in thread-session-auto lane (`NERO_RUNTIME_STATE_CONTROL_*`, compat `NEROBAR_NERO_RUNTIME_*`)
- Exact external outputs:
  - effective runtime settings applied to session config
  - normalized bridge command requests (`read-session-auto`, `apply-session-auto`)
- Runtime state ownership:
  - core session configuration + app-server authority mediation
- Native dependencies:
  - session lifecycle and protocol update stream
- External consumers:
  - `codex-nero-sdk` bridge module endpoint
  - `nerobar-ui` authority clients
- Branding status:
  - majority of capability-level surfaces are branded
- Confirmed residues:
  - compatibility alias listed once in section 4

## External Contract Index

### Hook action wire contracts

- Action types:
  - `nero_hook_msg`
  - `visible_note`
  - `auto_user_reply`
- Envelope:
  - `{"actions":[...]}`
- Nero hook msg schema highlights:
  - `mode`, `show`, `freq`, `format`, `status`, `msg`

### Native event carriers used by Nero contracts

- `EventMsg::Warning` carrying `WarningEvent`
- `HookCompletedEvent` carrying hook summary/meta/entries

### Hook summary/meta contracts used by stop-centric lane

- `protocol.stop_checkpoint_expected`
- `protocol.stop_checkpoint_delivered`
- `protocol.contract_satisfied`
- follow-up status counters in runtime meta

### App-server RPC contracts

- `thread/sessionAuto/read`
- `thread/sessionAuto/update`
- authority enum/value contract: `ThreadSessionAutoAuthorityMode::BridgeProxy`
- read response: `ThreadSessionAutoReadResponse { authority, state }`
- update request: required `expectedVersion`, optional `expectedSessionSource`
- update response: `ThreadSessionAutoUpdateResponse { authority, applied, conflict, message, error_code, reason_code, state }`

### Protocol session contract

- `nero_auto_runtime` (session configured/update surfaces)

### Auth rotation contracts

- `CODEXN_AUTH_ROTATE_CMD`
- `CODEXN_AUTH_ROTATE_CMD_TIMEOUT_MS`
- `CODEXN_ROTATION_REASON`

### Model fallback contracts

- `[nero.model_fallback]` with ladder/cooldown/sticky controls
- Nero config overlay env ingress keys for config resolution

### Bridge command/module contracts

- module: `nero_hook_runtime.session_auto_bridge`
- commands: `read-session-auto`, `apply-session-auto`

## Flowcharts

### A) Stop-centric Hook Msg / Hook Auto / User Report

```mermaid
%%{ init: { 'theme': 'dark' } }%%
flowchart TD
    A[User msg or queued auto msg] --> B[Native hook lifecycle enters STOP]
    B --> C[Nero command prompt + JSON request in STOP lane]
    C --> D[Assistant auto-report reply]
    D --> E{JSON valid and stop contract satisfied?}
    E -- yes --> F[Queue AutoUserReply]
    E -- no --> G[No auto enqueue]
    F --> H[AfterAgent runtime report]
    G --> H
    H --> I[Nero warning formatter/delivery]
    I --> J[Native EventMsg::Warning to TUI]
    H --> K[Native HookCompleted summary/meta]
```

### B) Session-auto Authority / F1-F5

```mermaid
%%{ init: { 'theme': 'dark' } }%%
flowchart TD
    A[F1-F5 key] --> B[detect_nero_auto_hotkey_action]
    B --> C[AppEvent::ApplyNeroAutoHotkey]
    C --> D[handle_nero_auto_hotkey_event]
    D --> E[RPC thread/sessionAuto/read]
    E --> F[ReadResponse returns authority and state]
    F --> G[Build requested runtime update]
    G --> H[RPC thread/sessionAuto/update]
    H --> I[UpdateResponse returns authority/applied/conflict/state]
    I --> J[TUI derives runtime context from returned state]
    J --> K[Session/protocol surfaces also carry nero_auto_runtime context]
```

### C) Dynamic Account Switching / Auth Rotation

```mermaid
%%{ init: { 'theme': 'dark' } }%%
flowchart TD
    A[Model request fails] --> B{usage_limit or quota class?}
    B -- no --> C[Normal error path]
    B -- yes --> D[External auth recovery path]
    D --> E{Recovered?}
    E -- yes --> F[Reload auth and retry]
    E -- no --> G[try_recover_with_nero_auth_rotate_command]
    G --> H[Run CODEXN_AUTH_ROTATE_CMD with CODEXN_ROTATION_REASON]
    H --> I{Rotate success?}
    I -- yes --> F
    I -- no --> J[Fail request]
```

### D) Model Fallback

```mermaid
%%{ init: { 'theme': 'dark' } }%%
flowchart TD
    A[Turn requested with model] --> B[apply_model_fallback_pre_turn]
    B --> C{Fallback switch needed now?}
    C -- yes --> D[Use next ladder step]
    C -- no --> E[Use requested model]
    D --> F[Run turn]
    E --> F
    F --> G{Eligible model failure?}
    G -- no --> H[Keep current state]
    G -- yes --> I[try_model_fallback_after_error]
    I --> J[Set cooldown and choose next step]
    J --> K{Step available within policy?}
    K -- yes --> L[Retry on fallback model]
    K -- no --> M[Return failure]
    L --> N[mark_model_fallback_success on success]
```

## Out-of-Scope Appendix

This atlas maps `codex-rs` only. The systems below are external and included only through boundary contracts:

- `codex-nero-sdk` (`EXTERNAL-CONSUMER`):
  - consumes/emits hook action contracts
  - provides bridge module endpoint `nero_hook_runtime.session_auto_bridge`
- `nerobar-ui` (`EXTERNAL-CONSUMER`):
  - consumes app-server RPC and protocol surfaces exposed by `codex-rs`
  - does not redefine `codex-rs` internal capability ownership
