# User Tests (UX + E2E Campaign) — Hook Actions PoC `after_agent`

## Cel kampanii

Zweryfikować **realność założeń UX i workflow** dla PoC hooków:

*   `visible_note` (widoczny wpis kontekstowy)
*   `auto_user_reply` (best-effort synthetic user continuation)
*   kombinacja obu akcji
*   zachowanie przy błędnym output hooka / legacy stdout

Kampania łączy:

*   **AUTO e2e** (powtarzalne scenariusze regresyjne)
*   **MANUAL UX** (ocena realnego zachowania z perspektywy użytkownika)

Testy są nastawione na **zachowanie z perspektywy użytkownika**, nie tylko poprawność techniczną.

## Wymagania wstępne

1.  `codexn` działa (`codexn --version`)
2.  Hook harness gotowy:
    *   `apps/codex-nero/.purrnet/camp-user/c1/01/smoke_notify.sh`
3.  Tymczasowo w `~/.codex/config.toml`:

```
notify = ["/workspace/purrnet/apps/codex-nero/.purrnet/camp-user/c1/01/smoke_notify.sh"]
```

1.  (Opcjonalnie) log payloadów:

```
export NERO_HOOK_SMOKE_LOG=/tmp/codex-nero-hook-smoke.log
```

## Skala oceny UX (prosta)

Po każdym scenariuszu oceń:

*   `A` — działa dobrze, UX czytelny, zachowanie przewidywalne
*   `B` — działa, ale drobne tarcie UX / wording / timing
*   `C` — działa technicznie, ale UX mylący lub irytujący
*   `D` — problem funkcjonalny / zachowanie nieakceptowalne

## Kampania A: AUTO E2E (regresja / realizm przepływu)

Cel:

*   szybko potwierdzić, że podstawowe flow nadal działa po zmianach w hookach
*   złapać regresje zanim zaczniemy oceniać UX ręcznie

Prereq:

*   zbudowana binarka `codex-linux-sandbox` (dla lokalnych e2e bez soft-skip)

Uruchomienie (targetowane):

```
cd /workspace/purrnet/apps/codex-nero/codex-rs
cargo test -p codex-core --test all hook_actions_notify::after_agent_ -- --nocapture
```

rezultaty:
```bash
dev@DESKTOP-GT96PHP:/workspace/purrnet/apps/codex-nero/codex-rs$ cargo test -p codex-core --test all hook_actions_notify::after_agent_ -- --nocapture
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.69s
     Running tests/all.rs (target/debug/deps/all-87db31d84430b84f)

running 5 tests
test suite::hook_actions_notify::after_agent_visible_note_emits_warning_and_turn_completes ... ok
test suite::hook_actions_notify::after_agent_garbage_json_does_not_crash_turn_flow ... ok
test suite::hook_actions_notify::after_agent_both_actions_can_trigger_follow_up_turn_without_manual_input ... ok
test suite::hook_actions_notify::after_agent_auto_user_reply_only_can_trigger_follow_up_turn_without_manual_input ... ok
test suite::hook_actions_notify::after_agent_legacy_plain_stdout_keeps_normal_flow ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 759 filtered out; finished in 0.80s

dev@DESKTOP-GT96PHP:/workspace/purrnet/apps/codex-nero/codex-rs$

```


Debug handoff logs (opcjonalnie, opt-in):





```
RUST_LOG=codex_core=debug,codex_hooks=debug \
cargo test -p codex-core --test all hook_actions_notify::after_agent_ -- --nocapture
```

rezultaty:

```bash
dev@DESKTOP-GT96PHP:/workspace/purrnet/apps/codex-nero/codex-rs$ RUST_LOG=codex_core=debug,codex_hooks=debug \
cargo test -p codex-core --test all hook_actions_notify::after_agent_ -- --nocapture
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.65s
     Running tests/all.rs (target/debug/deps/all-87db31d84430b84f)

running 5 tests
2026-02-24T11:46:43.855693Z DEBUG codex_core::config_loader::layer_io: /etc/codex/managed_config.toml not found
2026-02-24T11:46:43.855851Z DEBUG codex_core::config_loader::layer_io: /etc/codex/managed_config.toml not found
2026-02-24T11:46:43.860542Z DEBUG codex_core::config_loader::layer_io: /etc/codex/managed_config.toml not found
2026-02-24T11:46:43.861754Z DEBUG codex_core::config_loader::layer_io: /etc/codex/managed_config.toml not found
2026-02-24T11:46:43.893293Z DEBUG codex_core::config_loader::layer_io: /etc/codex/managed_config.toml not found
2026-02-24T11:46:43.906584Z DEBUG codex_core::exec_policy: loaded rules from 0 files
2026-02-24T11:46:43.906712Z  INFO codex_core::models_manager::manager: models cache: evaluating cache eligibility client_version="0.0.0"
2026-02-24T11:46:43.906738Z  INFO codex_core::models_manager::cache: models cache: attempting load_fresh cache_path=/tmp/.tmpLzEmKW/models_cache.json expected_version="0.0.0"
2026-02-24T11:46:43.906997Z  INFO codex_core::models_manager::manager: models cache: no usable cache entry
2026-02-24T11:46:43.907620Z  INFO codex_core::models_manager::manager: models cache: evaluating cache eligibility client_version="0.0.0"
2026-02-24T11:46:43.907710Z  INFO codex_core::models_manager::cache: models cache: attempting load_fresh cache_path=/tmp/.tmpLzEmKW/models_cache.json expected_version="0.0.0"
2026-02-24T11:46:43.907930Z  INFO codex_core::models_manager::manager: models cache: no usable cache entry
2026-02-24T11:46:43.908862Z DEBUG session_init: codex_core::codex: Configuring session: model=gpt-5.2-codex; provider=ModelProviderInfo { name: "OpenAI", base_url: Some("http://127.0.0.1:34343/v1"), env_key: None, env_key_instructions: None, experimental_bearer_token: None, wire_api: Responses, query_params: None, http_headers: Some({"version": "0.0.0"}), env_http_headers: Some({"OpenAI-Project": "OPENAI_PROJECT", "OpenAI-Organization": "OPENAI_ORGANIZATION"}), request_max_retries: None, stream_max_retries: None, stream_idle_timeout_ms: None, requires_openai_auth: true, supports_websockets: true }
2026-02-24T11:46:43.909476Z DEBUG codex_core::exec_policy: loaded rules from 0 files
2026-02-24T11:46:43.909570Z  INFO codex_core::models_manager::manager: models cache: evaluating cache eligibility client_version="0.0.0"
2026-02-24T11:46:43.909625Z  INFO codex_core::models_manager::cache: models cache: attempting load_fresh cache_path=/tmp/.tmpreRurP/models_cache.json expected_version="0.0.0"
2026-02-24T11:46:43.909752Z  INFO codex_core::models_manager::manager: models cache: no usable cache entry
2026-02-24T11:46:43.910148Z  INFO codex_core::models_manager::manager: models cache: evaluating cache eligibility client_version="0.0.0"
2026-02-24T11:46:43.910217Z  INFO codex_core::models_manager::cache: models cache: attempting load_fresh cache_path=/tmp/.tmpreRurP/models_cache.json expected_version="0.0.0"
2026-02-24T11:46:43.910478Z  INFO codex_core::models_manager::manager: models cache: no usable cache entry
2026-02-24T11:46:43.911157Z DEBUG session_init: codex_core::codex: Configuring session: model=gpt-5.2-codex; provider=ModelProviderInfo { name: "OpenAI", base_url: Some("http://127.0.0.1:32769/v1"), env_key: None, env_key_instructions: None, experimental_bearer_token: None, wire_api: Responses, query_params: None, http_headers: Some({"version": "0.0.0"}), env_http_headers: Some({"OpenAI-Organization": "OPENAI_ORGANIZATION", "OpenAI-Project": "OPENAI_PROJECT"}), request_max_retries: None, stream_max_retries: None, stream_idle_timeout_ms: None, requires_openai_auth: true, supports_websockets: true }
2026-02-24T11:46:43.937714Z DEBUG codex_core::exec_policy: loaded rules from 0 files
2026-02-24T11:46:43.938169Z  INFO codex_core::models_manager::manager: models cache: evaluating cache eligibility client_version="0.0.0"
2026-02-24T11:46:43.938273Z  INFO codex_core::models_manager::cache: models cache: attempting load_fresh cache_path=/tmp/.tmpU0bfcn/models_cache.json expected_version="0.0.0"
2026-02-24T11:46:43.938878Z  INFO codex_core::models_manager::manager: models cache: no usable cache entry
2026-02-24T11:46:43.939574Z  INFO codex_core::models_manager::manager: models cache: evaluating cache eligibility client_version="0.0.0"
2026-02-24T11:46:43.939941Z  INFO codex_core::models_manager::cache: models cache: attempting load_fresh cache_path=/tmp/.tmpU0bfcn/models_cache.json expected_version="0.0.0"
2026-02-24T11:46:43.940425Z  INFO codex_core::models_manager::manager: models cache: no usable cache entry
2026-02-24T11:46:43.941498Z DEBUG session_init: codex_core::codex: Configuring session: model=gpt-5.2-codex; provider=ModelProviderInfo { name: "OpenAI", base_url: Some("http://127.0.0.1:34141/v1"), env_key: None, env_key_instructions: None, experimental_bearer_token: None, wire_api: Responses, query_params: None, http_headers: Some({"version": "0.0.0"}), env_http_headers: Some({"OpenAI-Organization": "OPENAI_ORGANIZATION", "OpenAI-Project": "OPENAI_PROJECT"}), request_max_retries: None, stream_max_retries: None, stream_idle_timeout_ms: None, requires_openai_auth: true, supports_websockets: true }
2026-02-24T11:46:43.944865Z DEBUG codex_core::exec_policy: loaded rules from 0 files
2026-02-24T11:46:43.945179Z  INFO codex_core::models_manager::manager: models cache: evaluating cache eligibility client_version="0.0.0"
2026-02-24T11:46:43.945282Z  INFO codex_core::models_manager::cache: models cache: attempting load_fresh cache_path=/tmp/.tmp3pulpP/models_cache.json expected_version="0.0.0"
2026-02-24T11:46:43.945904Z  INFO codex_core::models_manager::manager: models cache: no usable cache entry
2026-02-24T11:46:43.946465Z  INFO codex_core::models_manager::manager: models cache: evaluating cache eligibility client_version="0.0.0"
2026-02-24T11:46:43.946560Z  INFO codex_core::models_manager::cache: models cache: attempting load_fresh cache_path=/tmp/.tmp3pulpP/models_cache.json expected_version="0.0.0"
2026-02-24T11:46:43.947111Z  INFO codex_core::models_manager::manager: models cache: no usable cache entry
2026-02-24T11:46:43.948279Z DEBUG session_init: codex_core::codex: Configuring session: model=gpt-5.2-codex; provider=ModelProviderInfo { name: "OpenAI", base_url: Some("http://127.0.0.1:43623/v1"), env_key: None, env_key_instructions: None, experimental_bearer_token: None, wire_api: Responses, query_params: None, http_headers: Some({"version": "0.0.0"}), env_http_headers: Some({"OpenAI-Project": "OPENAI_PROJECT", "OpenAI-Organization": "OPENAI_ORGANIZATION"}), request_max_retries: None, stream_max_retries: None, stream_idle_timeout_ms: None, requires_openai_auth: true, supports_websockets: true }
2026-02-24T11:46:43.957160Z DEBUG codex_core::exec_policy: loaded rules from 0 files
2026-02-24T11:46:43.957283Z  INFO codex_core::models_manager::manager: models cache: evaluating cache eligibility client_version="0.0.0"
2026-02-24T11:46:43.957329Z  INFO codex_core::models_manager::cache: models cache: attempting load_fresh cache_path=/tmp/.tmp0R6md6/models_cache.json expected_version="0.0.0"
2026-02-24T11:46:43.957614Z  INFO codex_core::models_manager::manager: models cache: no usable cache entry
2026-02-24T11:46:43.958117Z  INFO codex_core::models_manager::manager: models cache: evaluating cache eligibility client_version="0.0.0"
2026-02-24T11:46:43.958216Z  INFO codex_core::models_manager::cache: models cache: attempting load_fresh cache_path=/tmp/.tmp0R6md6/models_cache.json expected_version="0.0.0"
2026-02-24T11:46:43.958543Z  INFO codex_core::models_manager::manager: models cache: no usable cache entry
2026-02-24T11:46:43.959288Z DEBUG session_init: codex_core::codex: Configuring session: model=gpt-5.2-codex; provider=ModelProviderInfo { name: "OpenAI", base_url: Some("http://127.0.0.1:39653/v1"), env_key: None, env_key_instructions: None, experimental_bearer_token: None, wire_api: Responses, query_params: None, http_headers: Some({"version": "0.0.0"}), env_http_headers: Some({"OpenAI-Organization": "OPENAI_ORGANIZATION", "OpenAI-Project": "OPENAI_PROJECT"}), request_max_retries: None, stream_max_retries: None, stream_idle_timeout_ms: None, requires_openai_auth: true, supports_websockets: true }
2026-02-24T11:46:43.989432Z DEBUG session_init: codex_hooks::registry: initialized hook registry legacy_notify_enabled=true after_agent_hook_count=1
2026-02-24T11:46:44.002875Z  INFO codex_core::rollout::metadata: state db backfill scanned=0, upserted=0, failed=0
2026-02-24T11:46:44.006450Z DEBUG session_loop{thread_id=019c8f79-1845-7ee1-b338-ab1b1f2b11d1}: codex_core::codex: Submission sub=Submission { id: "019c8f79-18a6-76a3-97e5-5d1561f014b9", op: UserTurn { items: [Text { text: "hello", text_elements: [] }], cwd: "/tmp/.tmprpMcqp", approval_policy: Never, sandbox_policy: DangerFullAccess, model: "gpt-5.2-codex", effort: None, summary: Auto, final_output_json_schema: None, collaboration_mode: None, personality: None } }
2026-02-24T11:46:44.015336Z DEBUG session_init: codex_hooks::registry: initialized hook registry legacy_notify_enabled=true after_agent_hook_count=1
2026-02-24T11:46:44.028685Z  INFO codex_core::rollout::metadata: state db backfill scanned=0, upserted=0, failed=0
2026-02-24T11:46:44.031421Z DEBUG session_loop{thread_id=019c8f79-1847-7040-9109-775d39db7a38}: codex_core::codex: Submission sub=Submission { id: "019c8f79-18bf-7fd0-aef0-0294cb2e71e8", op: UserTurn { items: [Text { text: "hello legacy", text_elements: [] }], cwd: "/tmp/.tmppU0SIM", approval_policy: Never, sandbox_policy: DangerFullAccess, model: "gpt-5.2-codex", effort: None, summary: Auto, final_output_json_schema: None, collaboration_mode: None, personality: None } }
2026-02-24T11:46:44.037377Z  INFO session_init:shell_snapshot{thread_id=019c8f79-1845-7ee1-b338-ab1b1f2b11d1}: codex_core::shell_snapshot: Shell snapshot successfully created: /tmp/.tmpLzEmKW/shell_snapshots/019c8f79-1845-7ee1-b338-ab1b1f2b11d1.sh
2026-02-24T11:46:44.042755Z DEBUG session_init: codex_hooks::registry: initialized hook registry legacy_notify_enabled=true after_agent_hook_count=1
2026-02-24T11:46:44.058071Z DEBUG session_init: codex_hooks::registry: initialized hook registry legacy_notify_enabled=true after_agent_hook_count=1
2026-02-24T11:46:44.070487Z  INFO codex_core::rollout::metadata: state db backfill scanned=0, upserted=0, failed=0
2026-02-24T11:46:44.081643Z DEBUG session_init: codex_hooks::registry: initialized hook registry legacy_notify_enabled=true after_agent_hook_count=1
2026-02-24T11:46:44.082401Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::codex: Submission sub=Submission { id: "019c8f79-18f1-74f1-bfbf-f63c33336d22", op: UserTurn { items: [Text { text: "hello both", text_elements: [] }], cwd: "/tmp/.tmpiSoXeN", approval_policy: Never, sandbox_policy: DangerFullAccess, model: "gpt-5.2-codex", effort: None, summary: Auto, final_output_json_schema: None, collaboration_mode: None, personality: None } }
2026-02-24T11:46:44.091534Z  INFO codex_core::rollout::metadata: state db backfill scanned=0, upserted=0, failed=0
2026-02-24T11:46:44.093706Z  INFO session_init:shell_snapshot{thread_id=019c8f79-1847-7040-9109-775d39db7a38}: codex_core::shell_snapshot: Shell snapshot successfully created: /tmp/.tmpreRurP/shell_snapshots/019c8f79-1847-7040-9109-775d39db7a38.sh
2026-02-24T11:46:44.097850Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_core::codex: Submission sub=Submission { id: "019c8f79-1900-71e0-b3f4-664d2c9f5336", op: UserTurn { items: [Text { text: "hello auto", text_elements: [] }], cwd: "/tmp/.tmp2NvUwt", approval_policy: Never, sandbox_policy: DangerFullAccess, model: "gpt-5.2-codex", effort: None, summary: Auto, final_output_json_schema: None, collaboration_mode: None, personality: None } }
2026-02-24T11:46:44.102805Z  INFO codex_core::rollout::metadata: state db backfill scanned=0, upserted=0, failed=0
2026-02-24T11:46:44.103327Z DEBUG session_loop{thread_id=019c8f79-1877-79f3-aa37-e511e2f4f52b}: codex_core::codex: Submission sub=Submission { id: "019c8f79-1907-72c3-a243-38e448162ffe", op: UserTurn { items: [Text { text: "hello garbage", text_elements: [] }], cwd: "/tmp/.tmpJzATui", approval_policy: Never, sandbox_policy: DangerFullAccess, model: "gpt-5.2-codex", effort: None, summary: Auto, final_output_json_schema: None, collaboration_mode: None, personality: None } }
2026-02-24T11:46:44.122155Z  INFO session_init:shell_snapshot{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::shell_snapshot: Shell snapshot successfully created: /tmp/.tmpU0bfcn/shell_snapshots/019c8f79-1865-7a82-a75b-cbd2db37000f.sh
2026-02-24T11:46:44.128316Z  INFO session_init:shell_snapshot{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_core::shell_snapshot: Shell snapshot successfully created: /tmp/.tmp3pulpP/shell_snapshots/019c8f79-186c-7793-9879-214a2bf1c063.sh
2026-02-24T11:46:44.138566Z  INFO session_init:shell_snapshot{thread_id=019c8f79-1877-79f3-aa37-e511e2f4f52b}: codex_core::shell_snapshot: Shell snapshot successfully created: /tmp/.tmp0R6md6/shell_snapshots/019c8f79-1877-79f3-aa37-e511e2f4f52b.sh
2026-02-24T11:46:44.217442Z DEBUG session_loop{thread_id=019c8f79-1845-7ee1-b338-ab1b1f2b11d1}: codex_core::stream_events_utils: Output item item=Message { id: Some("m1"), role: "assistant", content: [OutputText { text: "Done" }], end_turn: None, phase: None }
2026-02-24T11:46:44.218426Z DEBUG session_loop{thread_id=019c8f79-1845-7ee1-b338-ab1b1f2b11d1}: codex_hooks::registry: dispatching hooks hook_count=1 event_type="after_agent"
2026-02-24T11:46:44.218514Z DEBUG session_loop{thread_id=019c8f79-1845-7ee1-b338-ab1b1f2b11d1}: codex_hooks::registry: executing hook hook_name=legacy_notify
2026-02-24T11:46:44.218587Z DEBUG session_loop{thread_id=019c8f79-1845-7ee1-b338-ab1b1f2b11d1}: codex_hooks::user_notification: spawning legacy notify hook process hook_name="legacy_notify" argv0="/tmp/.tmpujokxQ/notify.sh" argv_len=1
2026-02-24T11:46:44.223757Z DEBUG session_loop{thread_id=019c8f79-1845-7ee1-b338-ab1b1f2b11d1}: codex_hooks::user_notification: parsed hook actions from legacy notify stdout hook_name="legacy_notify" parsed_actions=1 ignored_unknown_actions=0 stdout_len=61
2026-02-24T11:46:44.223828Z DEBUG session_loop{thread_id=019c8f79-1845-7ee1-b338-ab1b1f2b11d1}: codex_hooks::user_notification: legacy notify hook completed successfully hook_name="legacy_notify" actions_count=1
2026-02-24T11:46:44.223886Z DEBUG session_loop{thread_id=019c8f79-1845-7ee1-b338-ab1b1f2b11d1}: codex_hooks::registry: hook execution finished hook_name=legacy_notify actions_count=1 abort=false result="success"
2026-02-24T11:46:44.223922Z DEBUG session_loop{thread_id=019c8f79-1845-7ee1-b338-ab1b1f2b11d1}: codex_core::codex: after_agent hooks dispatched turn_id=019c8f79-18a6-76a3-97e5-5d1561f014b9 hook_outcomes=1
2026-02-24T11:46:44.223944Z DEBUG session_loop{thread_id=019c8f79-1845-7ee1-b338-ab1b1f2b11d1}: codex_core::codex: processing after_agent hook actions turn_id=019c8f79-18a6-76a3-97e5-5d1561f014b9 hook_name=legacy_notify actions=1
2026-02-24T11:46:44.224053Z DEBUG session_loop{thread_id=019c8f79-1845-7ee1-b338-ab1b1f2b11d1}: codex_core::codex: emitted visible_note warning event turn_id=019c8f79-18a6-76a3-97e5-5d1561f014b9 hook_name=legacy_notify
2026-02-24T11:46:44.237335Z  WARN codex_core::file_watcher: failed to unwatch /tmp/.tmpLzEmKW/skills: No watch was found. about ["/tmp/.tmpLzEmKW/skills"]
2026-02-24T11:46:44.237440Z  WARN codex_core::file_watcher: failed to unwatch /tmp/.tmpLzEmKW/skills/.system: No watch was found. about ["/tmp/.tmpLzEmKW/skills/.system"]
2026-02-24T11:46:44.238190Z  WARN session_loop{thread_id=019c8f79-1845-7ee1-b338-ab1b1f2b11d1}: codex_core::shell_snapshot: Failed to delete shell snapshot at "/tmp/.tmpLzEmKW/shell_snapshots/019c8f79-1845-7ee1-b338-ab1b1f2b11d1.sh": Os { code: 2, kind: NotFound, message: "No such file or directory" }
test suite::hook_actions_notify::after_agent_visible_note_emits_warning_and_turn_completes ... ok
2026-02-24T11:46:44.261238Z DEBUG session_loop{thread_id=019c8f79-1847-7040-9109-775d39db7a38}: codex_core::stream_events_utils: Output item item=Message { id: Some("m1"), role: "assistant", content: [OutputText { text: "Done" }], end_turn: None, phase: None }
2026-02-24T11:46:44.262173Z DEBUG session_loop{thread_id=019c8f79-1847-7040-9109-775d39db7a38}: codex_hooks::registry: dispatching hooks hook_count=1 event_type="after_agent"
2026-02-24T11:46:44.262268Z DEBUG session_loop{thread_id=019c8f79-1847-7040-9109-775d39db7a38}: codex_hooks::registry: executing hook hook_name=legacy_notify
2026-02-24T11:46:44.262348Z DEBUG session_loop{thread_id=019c8f79-1847-7040-9109-775d39db7a38}: codex_hooks::user_notification: spawning legacy notify hook process hook_name="legacy_notify" argv0="/tmp/.tmpjLz6WY/notify.sh" argv_len=1
2026-02-24T11:46:44.265098Z DEBUG session_loop{thread_id=019c8f79-1847-7040-9109-775d39db7a38}: codex_hooks::user_notification: legacy notify stdout ignored (compat plain text) hook_name="legacy_notify" stdout_len=18
2026-02-24T11:46:44.265162Z DEBUG session_loop{thread_id=019c8f79-1847-7040-9109-775d39db7a38}: codex_hooks::user_notification: legacy notify hook completed successfully hook_name="legacy_notify" actions_count=0
2026-02-24T11:46:44.265190Z DEBUG session_loop{thread_id=019c8f79-1847-7040-9109-775d39db7a38}: codex_hooks::registry: hook execution finished hook_name=legacy_notify actions_count=0 abort=false result="success"
2026-02-24T11:46:44.265221Z DEBUG session_loop{thread_id=019c8f79-1847-7040-9109-775d39db7a38}: codex_core::codex: after_agent hooks dispatched turn_id=019c8f79-18bf-7fd0-aef0-0294cb2e71e8 hook_outcomes=1
2026-02-24T11:46:44.265242Z DEBUG session_loop{thread_id=019c8f79-1847-7040-9109-775d39db7a38}: codex_core::codex: processing after_agent hook actions turn_id=019c8f79-18bf-7fd0-aef0-0294cb2e71e8 hook_name=legacy_notify actions=0
2026-02-24T11:46:44.281389Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::stream_events_utils: Output item item=Message { id: Some("m1"), role: "assistant", content: [OutputText { text: "Done" }], end_turn: None, phase: None }
2026-02-24T11:46:44.282390Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_hooks::registry: dispatching hooks hook_count=1 event_type="after_agent"
2026-02-24T11:46:44.282489Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_hooks::registry: executing hook hook_name=legacy_notify
2026-02-24T11:46:44.282579Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_hooks::user_notification: spawning legacy notify hook process hook_name="legacy_notify" argv0="/tmp/.tmpWDzh89/notify.sh" argv_len=1
2026-02-24T11:46:44.285957Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_hooks::user_notification: parsed hook actions from legacy notify stdout hook_name="legacy_notify" parsed_actions=2 ignored_unknown_actions=0 stdout_len=106
2026-02-24T11:46:44.286056Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_hooks::user_notification: legacy notify hook completed successfully hook_name="legacy_notify" actions_count=2
2026-02-24T11:46:44.286103Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_hooks::registry: hook execution finished hook_name=legacy_notify actions_count=2 abort=false result="success"
2026-02-24T11:46:44.286134Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::codex: after_agent hooks dispatched turn_id=019c8f79-18f1-74f1-bfbf-f63c33336d22 hook_outcomes=1
2026-02-24T11:46:44.286170Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::codex: processing after_agent hook actions turn_id=019c8f79-18f1-74f1-bfbf-f63c33336d22 hook_name=legacy_notify actions=2
2026-02-24T11:46:44.286293Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::codex: emitted visible_note warning event turn_id=019c8f79-18f1-74f1-bfbf-f63c33336d22 hook_name=legacy_notify
2026-02-24T11:46:44.286325Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::codex: queued deferred auto_user_reply from hook turn_id=019c8f79-18f1-74f1-bfbf-f63c33336d22 hook_name=legacy_notify
2026-02-24T11:46:44.297449Z  INFO codex_core::codex: queued synthetic user reply from hook action turn_id=019c8f79-18f1-74f1-bfbf-f63c33336d22 hook_name=legacy_notify chain_depth=1 submission_id=hook-auto-1
2026-02-24T11:46:44.297615Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::codex: Submission sub=Submission { id: "hook-auto-1", op: UserInput { items: [Text { text: "continue", text_elements: [] }], final_output_json_schema: None } }
2026-02-24T11:46:44.306503Z DEBUG session_loop{thread_id=019c8f79-1877-79f3-aa37-e511e2f4f52b}: codex_core::stream_events_utils: Output item item=Message { id: Some("m1"), role: "assistant", content: [OutputText { text: "Done" }], end_turn: None, phase: None }
2026-02-24T11:46:44.306494Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_core::stream_events_utils: Output item item=Message { id: Some("m1"), role: "assistant", content: [OutputText { text: "Done" }], end_turn: None, phase: None }
2026-02-24T11:46:44.307558Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_hooks::registry: dispatching hooks hook_count=1 event_type="after_agent"
2026-02-24T11:46:44.307558Z DEBUG session_loop{thread_id=019c8f79-1877-79f3-aa37-e511e2f4f52b}: codex_hooks::registry: dispatching hooks hook_count=1 event_type="after_agent"
2026-02-24T11:46:44.307639Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_hooks::registry: executing hook hook_name=legacy_notify
2026-02-24T11:46:44.307654Z DEBUG session_loop{thread_id=019c8f79-1877-79f3-aa37-e511e2f4f52b}: codex_hooks::registry: executing hook hook_name=legacy_notify
2026-02-24T11:46:44.307700Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_hooks::user_notification: spawning legacy notify hook process hook_name="legacy_notify" argv0="/tmp/.tmpx10ekk/notify.sh" argv_len=1
2026-02-24T11:46:44.307718Z DEBUG session_loop{thread_id=019c8f79-1877-79f3-aa37-e511e2f4f52b}: codex_hooks::user_notification: spawning legacy notify hook process hook_name="legacy_notify" argv0="/tmp/.tmpM94qBt/notify.sh" argv_len=1
2026-02-24T11:46:44.311252Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_hooks::user_notification: parsed hook actions from legacy notify stdout hook_name="legacy_notify" parsed_actions=1 ignored_unknown_actions=0 stdout_len=61
2026-02-24T11:46:44.311344Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_hooks::user_notification: legacy notify hook completed successfully hook_name="legacy_notify" actions_count=1
2026-02-24T11:46:44.311392Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_hooks::registry: hook execution finished hook_name=legacy_notify actions_count=1 abort=false result="success"
2026-02-24T11:46:44.311434Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_core::codex: after_agent hooks dispatched turn_id=019c8f79-1900-71e0-b3f4-664d2c9f5336 hook_outcomes=1
2026-02-24T11:46:44.311459Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_core::codex: processing after_agent hook actions turn_id=019c8f79-1900-71e0-b3f4-664d2c9f5336 hook_name=legacy_notify actions=1
2026-02-24T11:46:44.311483Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_core::codex: queued deferred auto_user_reply from hook turn_id=019c8f79-1900-71e0-b3f4-664d2c9f5336 hook_name=legacy_notify
2026-02-24T11:46:44.311511Z DEBUG session_loop{thread_id=019c8f79-1877-79f3-aa37-e511e2f4f52b}: codex_hooks::user_notification: legacy notify stdout looked like JSON but actions parsing failed hook_name="legacy_notify" stdout_len=9 error=key must be a string at line 1 column 2
2026-02-24T11:46:44.311578Z DEBUG session_loop{thread_id=019c8f79-1877-79f3-aa37-e511e2f4f52b}: codex_hooks::registry: hook execution finished hook_name=legacy_notify actions_count=0 abort=false result="failed_continue"
2026-02-24T11:46:44.311615Z DEBUG session_loop{thread_id=019c8f79-1877-79f3-aa37-e511e2f4f52b}: codex_core::codex: after_agent hooks dispatched turn_id=019c8f79-1907-72c3-a243-38e448162ffe hook_outcomes=1
2026-02-24T11:46:44.311689Z  WARN session_loop{thread_id=019c8f79-1877-79f3-aa37-e511e2f4f52b}: codex_core::codex: after_agent hook failed; continuing turn_id=019c8f79-1907-72c3-a243-38e448162ffe hook_name=legacy_notify error=key must be a string at line 1 column 2
2026-02-24T11:46:44.317511Z  INFO codex_core::skills::manager: skills cache cleared (1 entries)
2026-02-24T11:46:44.320765Z  WARN codex_core::file_watcher: failed to unwatch /tmp/.tmp0R6md6/skills: No watch was found. about ["/tmp/.tmp0R6md6/skills"]
2026-02-24T11:46:44.320927Z  WARN codex_core::file_watcher: failed to unwatch /tmp/.tmp0R6md6/skills/.system: No watch was found. about ["/tmp/.tmp0R6md6/skills/.system"]
2026-02-24T11:46:44.321941Z  WARN session_loop{thread_id=019c8f79-1877-79f3-aa37-e511e2f4f52b}: codex_core::shell_snapshot: Failed to delete shell snapshot at "/tmp/.tmp0R6md6/shell_snapshots/019c8f79-1877-79f3-aa37-e511e2f4f52b.sh": Os { code: 2, kind: NotFound, message: "No such file or directory" }
2026-02-24T11:46:44.322917Z  INFO codex_core::codex: queued synthetic user reply from hook action turn_id=019c8f79-1900-71e0-b3f4-664d2c9f5336 hook_name=legacy_notify chain_depth=1 submission_id=hook-auto-1
2026-02-24T11:46:44.323018Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_core::codex: Submission sub=Submission { id: "hook-auto-1", op: UserInput { items: [Text { text: "continue", text_elements: [] }], final_output_json_schema: None } }
test suite::hook_actions_notify::after_agent_garbage_json_does_not_crash_turn_flow ... ok
2026-02-24T11:46:44.397291Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::stream_events_utils: Output item item=Message { id: Some("m2"), role: "assistant", content: [OutputText { text: "Done 2" }], end_turn: None, phase: None }
2026-02-24T11:46:44.398293Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_hooks::registry: dispatching hooks hook_count=1 event_type="after_agent"
2026-02-24T11:46:44.398381Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_hooks::registry: executing hook hook_name=legacy_notify
2026-02-24T11:46:44.398481Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_hooks::user_notification: spawning legacy notify hook process hook_name="legacy_notify" argv0="/tmp/.tmpWDzh89/notify.sh" argv_len=1
2026-02-24T11:46:44.402008Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_hooks::user_notification: parsed hook actions from legacy notify stdout hook_name="legacy_notify" parsed_actions=2 ignored_unknown_actions=0 stdout_len=106
2026-02-24T11:46:44.402118Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_hooks::user_notification: legacy notify hook completed successfully hook_name="legacy_notify" actions_count=2
2026-02-24T11:46:44.402157Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_hooks::registry: hook execution finished hook_name=legacy_notify actions_count=2 abort=false result="success"
2026-02-24T11:46:44.402208Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::codex: after_agent hooks dispatched turn_id=hook-auto-1 hook_outcomes=1
2026-02-24T11:46:44.402245Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::codex: processing after_agent hook actions turn_id=hook-auto-1 hook_name=legacy_notify actions=2
2026-02-24T11:46:44.402401Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::codex: emitted visible_note warning event turn_id=hook-auto-1 hook_name=legacy_notify
2026-02-24T11:46:44.402451Z DEBUG session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::codex: queued deferred auto_user_reply from hook turn_id=hook-auto-1 hook_name=legacy_notify
2026-02-24T11:46:44.402523Z  INFO session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::codex: skipping synthetic user reply from hook action (auto-reply chain depth exhausted) turn_id=hook-auto-1 hook_name=legacy_notify max_chain_depth=1
2026-02-24T11:46:44.406668Z  INFO codex_core::skills::manager: skills cache cleared (1 entries)
2026-02-24T11:46:44.408533Z  WARN codex_core::file_watcher: failed to unwatch /tmp/.tmpU0bfcn/skills: No watch was found. about ["/tmp/.tmpU0bfcn/skills"]
2026-02-24T11:46:44.408762Z  WARN codex_core::file_watcher: failed to unwatch /tmp/.tmpU0bfcn/skills/.system: No watch was found. about ["/tmp/.tmpU0bfcn/skills/.system"]
2026-02-24T11:46:44.409966Z  WARN session_loop{thread_id=019c8f79-1865-7a82-a75b-cbd2db37000f}: codex_core::shell_snapshot: Failed to delete shell snapshot at "/tmp/.tmpU0bfcn/shell_snapshots/019c8f79-1865-7a82-a75b-cbd2db37000f.sh": Os { code: 2, kind: NotFound, message: "No such file or directory" }
2026-02-24T11:46:44.410752Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_core::stream_events_utils: Output item item=Message { id: Some("m2"), role: "assistant", content: [OutputText { text: "Done 2" }], end_turn: None, phase: None }
test suite::hook_actions_notify::after_agent_both_actions_can_trigger_follow_up_turn_without_manual_input ... ok
2026-02-24T11:46:44.412355Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_hooks::registry: dispatching hooks hook_count=1 event_type="after_agent"
2026-02-24T11:46:44.412459Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_hooks::registry: executing hook hook_name=legacy_notify
2026-02-24T11:46:44.412553Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_hooks::user_notification: spawning legacy notify hook process hook_name="legacy_notify" argv0="/tmp/.tmpx10ekk/notify.sh" argv_len=1
2026-02-24T11:46:44.415285Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_hooks::user_notification: parsed hook actions from legacy notify stdout hook_name="legacy_notify" parsed_actions=1 ignored_unknown_actions=0 stdout_len=61
2026-02-24T11:46:44.415378Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_hooks::user_notification: legacy notify hook completed successfully hook_name="legacy_notify" actions_count=1
2026-02-24T11:46:44.415440Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_hooks::registry: hook execution finished hook_name=legacy_notify actions_count=1 abort=false result="success"
2026-02-24T11:46:44.415502Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_core::codex: after_agent hooks dispatched turn_id=hook-auto-1 hook_outcomes=1
2026-02-24T11:46:44.415557Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_core::codex: processing after_agent hook actions turn_id=hook-auto-1 hook_name=legacy_notify actions=1
2026-02-24T11:46:44.415609Z DEBUG session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_core::codex: queued deferred auto_user_reply from hook turn_id=hook-auto-1 hook_name=legacy_notify
2026-02-24T11:46:44.415676Z  INFO session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_core::codex: skipping synthetic user reply from hook action (auto-reply chain depth exhausted) turn_id=hook-auto-1 hook_name=legacy_notify max_chain_depth=1
2026-02-24T11:46:44.420305Z  INFO codex_core::skills::manager: skills cache cleared (1 entries)
2026-02-24T11:46:44.422647Z  WARN codex_core::file_watcher: failed to unwatch /tmp/.tmp3pulpP/skills: No watch was found. about ["/tmp/.tmp3pulpP/skills"]
2026-02-24T11:46:44.422807Z  WARN codex_core::file_watcher: failed to unwatch /tmp/.tmp3pulpP/skills/.system: No watch was found. about ["/tmp/.tmp3pulpP/skills/.system"]
2026-02-24T11:46:44.423537Z  WARN session_loop{thread_id=019c8f79-186c-7793-9879-214a2bf1c063}: codex_core::shell_snapshot: Failed to delete shell snapshot at "/tmp/.tmp3pulpP/shell_snapshots/019c8f79-186c-7793-9879-214a2bf1c063.sh": Os { code: 2, kind: NotFound, message: "No such file or directory" }
test suite::hook_actions_notify::after_agent_auto_user_reply_only_can_trigger_follow_up_turn_without_manual_input ... ok
2026-02-24T11:46:44.472610Z  INFO codex_core::skills::manager: skills cache cleared (1 entries)
2026-02-24T11:46:44.474615Z  WARN codex_core::file_watcher: failed to unwatch /tmp/.tmpreRurP/skills: No watch was found. about ["/tmp/.tmpreRurP/skills"]
2026-02-24T11:46:44.474756Z  WARN codex_core::file_watcher: failed to unwatch /tmp/.tmpreRurP/skills/.system: No watch was found. about ["/tmp/.tmpreRurP/skills/.system"]
2026-02-24T11:46:44.475297Z  WARN session_loop{thread_id=019c8f79-1847-7040-9109-775d39db7a38}: codex_core::shell_snapshot: Failed to delete shell snapshot at "/tmp/.tmpreRurP/shell_snapshots/019c8f79-1847-7040-9109-775d39db7a38.sh": Os { code: 2, kind: NotFound, message: "No such file or directory" }
test suite::hook_actions_notify::after_agent_legacy_plain_stdout_keeps_normal_flow ... ok

test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 759 filtered out; finished in 0.81s

dev@DESKTOP-GT96PHP:/workspace/purrnet/apps/codex-nero/codex-rs$
```

To pokazuje m.in.:

*   inicjalizację hook registry
*   dispatch hooków `after_agent`
*   spawn `legacy_notify`
*   parse `actions[]`
*   wykonanie `visible_note`
*   defer/queue `auto_user_reply`

Scenariusze AUTO (mapa):

*   `visible_note` -> warning + completion
*   `legacy stdout` -> brak akcji + normalny flow
*   `garbage JSON` -> brak crasha + normalny flow
*   `both` -> note + follow-up turn (best-effort path)

Pass criteria (AUTO):

*   test target przechodzi
*   brak crasha harnessu
*   brak regresji w kolejności akcji (note przed auto)

## Kampania B: MANUAL UX (ocena doświadczenia użytkownika)

### U1. Visible Note Only (baseline UX)

Setup:

```
export NERO_HOOK_SMOKE_MODE=visible
codexn --dev
```

Kroki:

1.  Wyślij prosty prompt, np. `Say hi`.
2.  Poczekaj na zakończenie tury.

Expected:

*   pojawia się widoczny wpis `[nero-hook] ...`
*   wpis jest czytelny i nie wygląda jak przypadkowy warning systemowy
*   tura kończy się normalnie
*   brak dziwnych side-efektów (brak dodatkowych turnów)

Zanotuj:

*   czy prefix `[nero-hook]` jest OK UX-owo?
*   czy warning style powinien być inny typ eventu w przyszłości?

rezultaty
```bash
dev@DESKTOP-GT96PHP:/workspace/purrnet/apps/codex-nero/codex-rs$ export NERO_HOOK_SMOKE_MODE=visible
codexn --dev
╭────────────────────────────────────────────────────────╮
│ >_ OpenAI Codex (v0.104.0-nero.1)                      │
│                                                        │
│ model:     gpt-5.3-codex medium   /model to change     │
│ directory: /workspace/purrnet/apps/codex-nero/codex-rs │
╰────────────────────────────────────────────────────────╯

  Tip: New 2x rate limits until April 2nd.

• Model changed to gpt-5.3-codex low


› Say hi - this is just test


• Quick test noted. I’ll keep it simple and just reply with a friendly greeting.

• Hi!


› czy widzisz w swoim potoku danych/histori `[nero-hook]` ? bo robię fork który dodaje hook i widze
  że nie weszło... ale pytanei czy w UI (TUI) czy w dane


• Nie widzę u siebie żadnego [nero-hook] w historii ani w “potoku danych” poza tym, co trafia do
  rozmowy jako treść wiadomości.

  Krótko:

  - w tym czacie widzę tylko to, co UI/runtime przekazuje jako message context,
  - nie mam podglądu do wewnętrznych hooków/logów TUI, jeśli nie są wstrzyknięte do wiadomości,
  - jeśli hook działa tylko w UI (np. lokalny render/log), to mogę go nie zobaczyć w danych modelu.

  Jeśli chcesz, możemy to szybko rozróżnić:

  1. wstrzyknij testowo [nero-hook] do treści promptu/system message i zobaczę go od razu,
  2. jeśli pojawia się tylko w TUI/logach, to znaczy że hook jest na warstwie UI, nie w payloadzie do
     modelu.
```


### U2. Legacy Plain Stdout (compat UX)

Setup:

```
export NERO_HOOK_SMOKE_MODE=legacy
codexn --dev
```

Kroki:

1.  Wyślij prosty prompt.
2.  Poczekaj na zakończenie tury.

Expected:

*   normalny flow bez crasha
*   brak widocznego hook action (bo plain stdout nie jest akcją)
*   brak synthetic user continuation

Zanotuj:

*   czy brak widocznej informacji jest OK dla compat path?

rezultaty
```bash
dev@DESKTOP-GT96PHP:/workspace/purrnet/apps/codex-nero/codex-rs$ export NERO_HOOK_SMOKE_MODE=legacy
codexn --dev
╭────────────────────────────────────────────────────────╮
│ >_ OpenAI Codex (v0.104.0-nero.1)                      │
│                                                        │
│ model:     gpt-5.3-codex low   /model to change        │
│ directory: /workspace/purrnet/apps/codex-nero/codex-rs │
╰────────────────────────────────────────────────────────╯

  Tip: New 2x rate limits until April 2nd.


› Say hi - this is just test


• Hi!


› czy masz info w histori swoich danych o `[nero-hook]` -> to powinien być wynik hook a to jest fork
  który taki hook wprowadza


• Nie mam wglądu w „historię swoich danych” ani listę konkretnych commitów/forków, na których byłem
  trenowany, więc nie mogę potwierdzić, czy [nero-hook] było tam obecne.

  Ale rozumiem, o co chodzi:
  [nero-hook] brzmi jak własny marker/output z hooka dodanego w tym forku, więc jeśli widzisz taki
  prefix, to bardzo możliwe, że to właśnie rezultat tego mechanizmu.

  Jeśli chcesz, mogę od razu sprawdzić w tym repo:

  - gdzie jest definiowany nero-hook
  - kiedy i w jakim flow jest emitowany
  - jaki dokładnie format outputu powinien mieć


› że stdout... ale to powinno być chyba czytelne dla ciebie że to odczytujesz widzisz a ja nie w UI
  (TUI)


• Tak, dokładnie, to ma sens.

  Jeśli hook pisze na stdout, to bardzo możliwe że:

  - ja (agent) to widzę w strumieniu wykonania narzędzia/komendy,
  - a TUI tego nie pokazuje użytkownikowi (bo filtruje / renderuje tylko część outputu).

  Czyli [nero-hook] może być dla mnie „czytelne”, nawet jeśli u Ciebie w UI tego nie widać.

  To jest dość typowy efekt, gdy:

  - UI przechwytuje stdout/stderr i pokazuje tylko finalny wynik,
  - albo hook output jest traktowany jako techniczny/logowy i ukrywany.

  Jeśli chcesz, mogę sprawdzić w tym forku, czy [nero-hook] idzie na stdout vs stderr i gdzie TUI to
  gubi/ukrywa.


› ok... cyzli nie widzisz teraz tego... tylko o to pytam


• Tak, teraz tego nie widzę.

  W tej rozmowie nie mam automatycznie podglądu surowego stdout z hooków, jeśli nie został przekazany
  w treści wiadomości albo nie uruchomię komendy, która to wypisze.

```


### U3. Garbage JSON (resilience UX)

Setup:

```
export NERO_HOOK_SMOKE_MODE=garbage
codexn --dev
```

Kroki:

1.  Wyślij prosty prompt.
2.  Obserwuj zakończenie tury i ewentualne warningi/errors.

Expected:

*   brak crasha sesji
*   turn flow dalej działa
*   zachowanie jest przewidywalne (hook failure continue / komunikat)

Zanotuj:

*   czy komunikat o błędzie hooka (jeśli jest) jest zrozumiały?
*   czy powinien być bardziej “operacyjny” niż user-facing?

### U4. Both Actions (`visible_note` + `auto_user_reply`)

Setup:

```
export NERO_HOOK_SMOKE_MODE=both
codexn --dev
```

Kroki:

1.  Wyślij prompt, który kończy turę bez blokad.
2.  Obserwuj:
    *   wpis `[nero-hook]`
    *   ewentualny follow-up synthetic turn
3.  Nic nie klikaj/nie odpowiadaj przez chwilę (sprawdzenie auto flow).

Expected:

*   `visible_note` pojawia się przed auto-kontynuacją
*   synthetic turn może ruszyć automatycznie (best-effort)
*   brak pętli
*   brak “dziwnego mieszania” z ręcznym inputem, jeśli nic nie wpisujesz

Zanotuj:

*   czy treść auto-reply promptu jest dobra?
*   czy to pomaga workflow, czy spamuje?

### U5. Race Test (manual user vs auto\_user\_reply)

Setup:

```
export NERO_HOOK_SMOKE_MODE=both
codexn --dev
```

Kroki:

1.  Wyślij prompt.
2.  Gdy tura się kończy, szybko ręcznie wpisz kolejną wiadomość (celowo ścigając auto-reply).

Expected:

*   brak crasha / brak pętli
*   jeśli synthetic zostanie pominięty z powodu race, to jest OK (safety-first)
*   manualny input powinien wygrać/przejść normalnie

Zanotuj:

*   czy to zachowanie jest akceptowalne produktowo?
*   czy potrzebujemy wyraźniejszego komunikatu “auto skipped due to manual input”?

### U6. Multi-turn Stability (3-5 tur)

Setup:

```
export NERO_HOOK_SMOKE_MODE=visible
codexn --dev
```

Kroki:

1.  Zrób 3-5 zwykłych tur.
2.  Obserwuj, czy hook działa stabilnie w każdej.

Expected:

*   brak degradacji UX
*   brak opóźnień / brak zanikających wpisów hooka
*   brak narastających efektów ubocznych

Zanotuj:

*   czy warning styl zaczyna przeszkadzać po kilku turach?

## Arkusz wyników (AUTO + MANUAL)

AUTO E2E:

*   A1 `visible_note`: `PASS/FAIL/SKIP` + notatki
*   A2 `legacy stdout`: `PASS/FAIL/SKIP` + notatki
*   A3 `garbage JSON`: `PASS/FAIL/SKIP` + notatki
*   A4 `both`: `PASS/FAIL/SKIP` + notatki

MANUAL UX:

*   U1 Visible Note: `A/B/C/D` + notatki
*   U2 Legacy Stdout: `A/B/C/D` + notatki
*   U3 Garbage JSON: `A/B/C/D` + notatki
*   U4 Both Actions: `A/B/C/D` + notatki
*   U5 Race Manual vs Auto: `A/B/C/D` + notatki
*   U6 Multi-turn Stability: `A/B/C/D` + notatki

## Decyzje po kampanii (do podjęcia)

1.  Czy `visible_note` zostaje jako `WarningEvent`, czy robimy dedykowany event/UI style?
2.  Czy `auto_user_reply` domyślnie ON, czy feature-flag OFF?
3.  Czy treść auto promptu ma być:
    *   stała,
    *   konfigurowalna,
    *   generowana przez plugin/server?
4.  Jak komunikować skip auto-reply przy race z manualnym inputem?
5.  Jak ścisła ma być kompatybilność `legacy_notify` stdout?

## Szybki workflow wykonania (rekomendowany)

1.  Uruchom Kampanię A (AUTO E2E)
2.  Jeśli AUTO zielone -> przejdź Kampanię B (MANUAL UX)
3.  Wypełnij arkusz wyników
4.  Podejmij decyzje z sekcji końcowej (policy + UX)

---

## Scope Reset (Nero Hook Only) — 2026-02-24

Ustalenie po testach:

*   Na tym etapie kampanii testujemy wyłącznie `nero-hook` / `nero_hook_msg`.
*   `auto_user_reply` jest poza scope bieżącej oceny UX/produkcji (nie testujemy teraz policy auto-turnów).
*   Wszelkie wcześniejsze testy z “podwójnymi odpowiedziami” należy traktować jako poza bieżącym scope (mieszany probe).

### Scope aktywny (only `nero_hook_msg`)

*   `show.agent` / `show.tui`
*   `mode = synced | tui-short`
*   `format = block | inline`
*   `status`
*   `freq`
*   reset `freq` po compaction
*   kanał `notify -> router.py -> organizer -> actions[]`

### Scope out (na razie)

*   `auto_user_reply`
*   auto-turn policy / UX synthetic turnów
*   race manual-vs-auto

## Manualne wyniki (scope-valid, nero-hook only)

### S1. `nero_status_block_probe` — PASS

Setup:

```bash
NERO_HOOK_SMOKE_MODE=nero_status_block_probe codexn --hook-debug --dev
```

Wynik:

*   TUI pokazuje blok `nero-hook` ✅
*   `content` renderuje się poprawnie ✅
*   `status` renderuje się osobno i czytelnie ✅
*   status demo jest teraz jawny (`info: static status demo (format probe)`) ✅

### S2. `nero_tui_short_probe` — PASS

Setup:

```bash
NERO_HOOK_SMOKE_MODE=nero_tui_short_probe codexn --hook-debug --dev
```

Wynik:

*   TUI pokazuje krótką treść (`short`) ✅
*   Agent rozumie pełną treść (`full`) ✅
*   `reksio -> 9`, `pluto -> 5` potwierdzone ✅
*   brak chaosu z podwójnymi odpowiedziami (po rozdzieleniu probe) ✅

### S3. `nero_synced_probe` — PASS

Setup:

```bash
NERO_HOOK_SMOKE_MODE=nero_synced_probe codexn --hook-debug --dev
```

Wynik:

*   TUI pokazuje pełną treść (`full`) ✅
*   Agent dostaje tę samą pełną treść ✅
*   `filemon -> 23` potwierdzone ✅

### S4. `nero_tui_short_freq_probe` — PASS

Setup:

```bash
NERO_HOOK_SMOKE_MODE=nero_tui_short_freq_probe codexn --hook-debug --dev
```

Wynik:

*   Pierwsza tura: normalny komunikat `nero-hook` ✅
*   Kolejna szybka tura: throttling + countdown ✅
*   Po `Context compacted`: throttle reset działa ✅
*   Następna tura po compaction pokazuje znowu normalny komunikat ✅

### S5. Production Path (config-driven, bez smoke env) — PASS

Setup:

*   `notify = ["python3", "/workspace/purrnet/apps/codex-nero-sdk/router.py"]`
*   `[nero.hook.runtime.nero_hook_msg]` skonfigurowane w `~/.codex/config-nero.toml`
*   uruchomienie bez `NERO_HOOK_SMOKE_MODE`

Wynik:

*   `router.py --check` pokazuje `runtime cfg : nero_hook_msg=yes` ✅
*   `codexn --hook-debug --dev` renderuje blok `nero-hook` z configu runtime ✅
*   `freq=120` działa (throttling + countdown) ✅
*   reset po compaction działa w production path ✅

## Status (nero-hook)

`nero-hook` (`nero_hook_msg`) jest obecnie potwierdzony manualnie jako production-ready MVP:

*   kanał komunikacji działa (`Codex -> router.py -> organizer -> actions[]`)
*   konfiguracja runtime działa (`config-nero.toml`)
*   TUI UX (block + status) działa i jest czytelny
*   agent/TUI split (`show.*`, `mode`) działa
*   `freq` + reset po compaction działa
