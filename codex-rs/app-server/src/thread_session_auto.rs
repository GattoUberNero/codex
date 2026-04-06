use chrono::Utc;
use codex_app_server_protocol::SessionSource;
use codex_app_server_protocol::ThreadSessionAutoAuthorityMode;
use codex_app_server_protocol::ThreadSessionAutoReadResponse;
use codex_app_server_protocol::ThreadSessionAutoState;
use codex_app_server_protocol::ThreadSessionAutoUpdateParams;
use codex_app_server_protocol::ThreadSessionAutoUpdateResponse;
pub(crate) use codex_core::nero_auto_runtime_state::NeroThreadSessionAutoContext;
use codex_core::nero_auto_runtime_state::acquire_state_lock;
use codex_core::nero_auto_runtime_state::build_session_auto_state;
use codex_core::nero_auto_runtime_state::is_subagent_session_source;
use codex_core::nero_auto_runtime_state::normalize_done_stop_scope;
use codex_core::nero_auto_runtime_state::normalized_policy_override_from_entry;
use codex_core::nero_auto_runtime_state::read_state_snapshot;
use codex_core::nero_auto_runtime_state::read_thread_state_entry;
use codex_core::nero_auto_runtime_state::resolve_nero_auto_config_path;
use codex_core::nero_auto_runtime_state::resolve_nero_auto_state_path;
use codex_core::nero_auto_runtime_state::runtime_delivery_requires_runtime_msg;
use codex_core::nero_auto_runtime_state::session_source_wire_value;
use codex_core::nero_auto_runtime_state::threads_map_mut;
use codex_core::nero_auto_runtime_state::write_state_snapshot_atomic;
use serde_json::Value as JsonValue;
use std::path::Path;

fn version_conflict_response(
    context: &NeroThreadSessionAutoContext,
    state: ThreadSessionAutoState,
    expected_version: &str,
) -> ThreadSessionAutoUpdateResponse {
    ThreadSessionAutoUpdateResponse {
        thread_id: context.thread_id.clone(),
        authority: ThreadSessionAutoAuthorityMode::BridgeProxy,
        applied: false,
        conflict: true,
        message: Some(format!(
            "session-auto version conflict: expected={expected_version}, current={}",
            state.version
        )),
        error_code: Some("version_conflict".to_string()),
        reason_code: None,
        state: Some(state),
    }
}

fn session_source_conflict_response(
    context: &NeroThreadSessionAutoContext,
    state: ThreadSessionAutoState,
    expected: &SessionSource,
) -> ThreadSessionAutoUpdateResponse {
    ThreadSessionAutoUpdateResponse {
        thread_id: context.thread_id.clone(),
        authority: ThreadSessionAutoAuthorityMode::BridgeProxy,
        applied: false,
        conflict: true,
        message: Some(format!(
            "session-auto sessionSource mismatch: expected={}, current={}",
            session_source_wire_value(expected),
            session_source_wire_value(&state.session_source),
        )),
        error_code: Some("session_source_mismatch".to_string()),
        reason_code: None,
        state: Some(state),
    }
}

fn invalid_input_response(
    context: &NeroThreadSessionAutoContext,
    state: ThreadSessionAutoState,
    message: String,
) -> ThreadSessionAutoUpdateResponse {
    ThreadSessionAutoUpdateResponse {
        thread_id: context.thread_id.clone(),
        authority: ThreadSessionAutoAuthorityMode::BridgeProxy,
        applied: false,
        conflict: false,
        message: Some(message),
        error_code: Some("invalid_input".to_string()),
        reason_code: None,
        state: Some(state),
    }
}

fn rejected_update_response(
    context: &NeroThreadSessionAutoContext,
    state: ThreadSessionAutoState,
    error_code: &str,
    reason_code: &str,
    message: &str,
) -> ThreadSessionAutoUpdateResponse {
    ThreadSessionAutoUpdateResponse {
        thread_id: context.thread_id.clone(),
        authority: ThreadSessionAutoAuthorityMode::BridgeProxy,
        applied: false,
        conflict: false,
        message: Some(message.to_string()),
        error_code: Some(error_code.to_string()),
        reason_code: Some(reason_code.to_string()),
        state: Some(state),
    }
}

pub(crate) async fn read_thread_session_auto(
    codex_home: &Path,
    context: &NeroThreadSessionAutoContext,
) -> Result<ThreadSessionAutoReadResponse, String> {
    let config_path = resolve_nero_auto_config_path(codex_home);
    let state_path = resolve_nero_auto_state_path(&config_path);
    let snapshot = read_state_snapshot(&state_path)?;
    Ok(ThreadSessionAutoReadResponse {
        thread_id: context.thread_id.clone(),
        authority: ThreadSessionAutoAuthorityMode::BridgeProxy,
        state: build_session_auto_state(context, &state_path, &config_path, &snapshot),
    })
}

pub(crate) async fn update_thread_session_auto(
    codex_home: &Path,
    context: &NeroThreadSessionAutoContext,
    params: &ThreadSessionAutoUpdateParams,
) -> Result<ThreadSessionAutoUpdateResponse, String> {
    if is_subagent_session_source(&context.session_source) {
        return Err("session-auto updates are unsupported for subagent sessions".to_string());
    }
    let config_path = resolve_nero_auto_config_path(codex_home);
    let state_path = resolve_nero_auto_state_path(&config_path);
    let _lock = acquire_state_lock(&state_path)?;
    let mut snapshot = read_state_snapshot(&state_path)?;
    let current_state = build_session_auto_state(context, &state_path, &config_path, &snapshot);

    if let Some(expected_session_source) = &params.expected_session_source
        && current_state.session_source != *expected_session_source
    {
        return Ok(session_source_conflict_response(
            context,
            current_state,
            expected_session_source,
        ));
    }
    if current_state.version != params.expected_version {
        return Ok(version_conflict_response(
            context,
            current_state,
            &params.expected_version,
        ));
    }
    if matches!(params.enabled, Some(Some(true)))
        && runtime_delivery_requires_runtime_msg(&config_path)
        && context.thread_name.as_deref().is_none_or(str::is_empty)
    {
        return Ok(rejected_update_response(
            context,
            current_state,
            "runtime_msg_unavailable_for_auto",
            "missing_thread_name",
            "Cannot enable session auto: runtime_msg delivery contract cannot be satisfied for this session.",
        ));
    }

    let thread_key = format!("id:{}", context.thread_id);
    let now_iso = Utc::now().to_rfc3339();

    let threads = threads_map_mut(&mut snapshot.root);
    let (mut entry, legacy_key) = read_thread_state_entry(threads, &thread_key);
    let has_enabled = params.enabled.is_some();

    if has_enabled {
        match params.enabled.flatten() {
            Some(value) => {
                entry.insert(
                    "session_auto_enabled_override".to_string(),
                    JsonValue::Bool(value),
                );
            }
            None => {
                entry.remove("session_auto_enabled_override");
            }
        }
    }

    let mut policy = normalized_policy_override_from_entry(&entry).unwrap_or_default();

    if params.autonomy_level.is_some() {
        match params.autonomy_level.flatten() {
            Some(value) => {
                policy.insert(
                    "autonomy_level".to_string(),
                    JsonValue::Number(value.clamp(1, 10).into()),
                );
            }
            None => {
                policy.remove("autonomy_level");
            }
        }
    }
    if params.autonomy_step_per_round.is_some() {
        match params.autonomy_step_per_round.flatten() {
            Some(value) => {
                let clamped = value.clamp(0.0, 10.0);
                if let Some(number) =
                    serde_json::Number::from_f64((clamped * 1000.0).round() / 1000.0)
                {
                    policy.insert(
                        "autonomy_step_per_round".to_string(),
                        JsonValue::Number(number),
                    );
                }
            }
            None => {
                policy.remove("autonomy_step_per_round");
            }
        }
    }
    if params.max_auto_rounds.is_some() {
        match params.max_auto_rounds.flatten() {
            Some(value) => {
                policy.insert(
                    "max_auto_rounds".to_string(),
                    JsonValue::Number(value.max(0).into()),
                );
            }
            None => {
                policy.remove("max_auto_rounds");
            }
        }
    }
    if params.done_stop_scope.is_some() {
        match params.done_stop_scope.clone().flatten() {
            Some(value) => {
                let normalized = normalize_done_stop_scope(Some(value.as_str()), None, "");
                if normalized.is_empty() {
                    let current_state =
                        build_session_auto_state(context, &state_path, &config_path, &snapshot);
                    return Ok(invalid_input_response(
                        context,
                        current_state,
                        "doneStopScope must be one of active_phase, campaign, disabled or null"
                            .to_string(),
                    ));
                }
                policy.insert("done_stop_scope".to_string(), JsonValue::String(normalized));
                policy.remove("stop_when_all_gates_done");
            }
            None => {
                policy.remove("done_stop_scope");
                policy.remove("stop_when_all_gates_done");
            }
        }
    }

    if policy.is_empty() {
        entry.remove("session_auto_policy_override");
    } else {
        entry.insert(
            "session_auto_policy_override".to_string(),
            JsonValue::Object(policy),
        );
    }

    if params.auto_rounds.is_some() {
        match params.auto_rounds.flatten() {
            Some(value) => {
                entry.insert(
                    "auto_rounds".to_string(),
                    JsonValue::Number(value.max(0).into()),
                );
            }
            None => {
                entry.remove("auto_rounds");
            }
        }
    } else if params.reset_counter {
        entry.insert("auto_rounds".to_string(), JsonValue::Number(0.into()));
    }

    entry.insert(
        "session_auto_override_updated_at".to_string(),
        JsonValue::String(now_iso),
    );

    threads.insert(thread_key.clone(), JsonValue::Object(entry));
    if let Some(legacy_key) = legacy_key
        && legacy_key != thread_key
    {
        threads.remove(&legacy_key);
    }

    write_state_snapshot_atomic(&state_path, &snapshot.root)?;
    let confirmed = read_state_snapshot(&state_path)?;
    let confirmed_state = build_session_auto_state(context, &state_path, &config_path, &confirmed);

    Ok(ThreadSessionAutoUpdateResponse {
        thread_id: context.thread_id.clone(),
        authority: ThreadSessionAutoAuthorityMode::BridgeProxy,
        applied: true,
        conflict: false,
        message: None,
        error_code: None,
        reason_code: None,
        state: Some(confirmed_state),
    })
}

#[cfg(test)]
mod tests {
    use super::NeroThreadSessionAutoContext;
    use super::read_thread_session_auto;
    use super::update_thread_session_auto;
    use codex_app_server_protocol::SessionSource;
    use codex_app_server_protocol::ThreadSessionAutoUpdateParams;
    use codex_core::nero_auto_runtime_state::CODEXN_CONFIG_NERO_DEV_PATH_ENV;
    use codex_core::nero_auto_runtime_state::CODEXN_CONFIG_NERO_MERGE_PATHS_ENV;
    use codex_core::nero_auto_runtime_state::CODEXN_CONFIG_NERO_MSG_PATH_ENV;
    use codex_core::nero_auto_runtime_state::CODEXN_CONFIG_NERO_PATH_ENV;
    use codex_core::nero_auto_runtime_state::NERO_AUTO_RUNTIME_CONFIG_ENV;
    use codex_core::nero_auto_runtime_state::NERO_HOOK_AUTO_AUTONOMY_LEVEL_ENV;
    use codex_core::nero_auto_runtime_state::NERO_HOOK_AUTO_ENABLED_ENV;
    use codex_core::nero_auto_runtime_state::NERO_HOOK_AUTO_MAX_ROUNDS_ENV;
    use codex_core::nero_auto_runtime_state::NERO_HOOK_AUTO_STEP_PER_ROUND_ENV;
    use pretty_assertions::assert_eq;
    use std::sync::OnceLock;
    use tempfile::tempdir;
    use tokio::sync::Mutex;

    fn runtime_auto_test_guard() -> &'static Mutex<()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD.get_or_init(|| Mutex::new(()))
    }

    fn sample_context() -> NeroThreadSessionAutoContext {
        NeroThreadSessionAutoContext {
            thread_id: "thread-1".to_string(),
            thread_name: Some("thread-1".to_string()),
            session_source: SessionSource::Cli,
            loaded: true,
        }
    }

    fn clear_runtime_auto_env_overrides() {
        // SAFETY: test-only env normalization for a deterministic runtime-default surface.
        unsafe {
            std::env::remove_var(NERO_HOOK_AUTO_ENABLED_ENV);
            std::env::remove_var(NERO_HOOK_AUTO_AUTONOMY_LEVEL_ENV);
            std::env::remove_var(NERO_HOOK_AUTO_MAX_ROUNDS_ENV);
            std::env::remove_var(NERO_HOOK_AUTO_STEP_PER_ROUND_ENV);
            std::env::remove_var(CODEXN_CONFIG_NERO_MERGE_PATHS_ENV);
        }
    }

    fn configure_isolated_runtime_config_env(
        config_path: &std::path::Path,
        base_path: &std::path::Path,
        msg_path: &std::path::Path,
        dev_path: &std::path::Path,
    ) {
        clear_runtime_auto_env_overrides();
        // SAFETY: test-only env normalization for deterministic config layering.
        unsafe {
            std::env::set_var(NERO_AUTO_RUNTIME_CONFIG_ENV, config_path);
            std::env::set_var(CODEXN_CONFIG_NERO_PATH_ENV, base_path);
            std::env::set_var(CODEXN_CONFIG_NERO_MSG_PATH_ENV, msg_path);
            std::env::set_var(CODEXN_CONFIG_NERO_DEV_PATH_ENV, dev_path);
        }
    }

    #[tokio::test]
    async fn roundtrip_update_preserves_cas_and_null_clear() {
        let _guard = runtime_auto_test_guard().lock().await;
        let temp = tempdir().expect("tempdir");
        let config_path = temp.path().join("config-nero-hook-auto.toml");
        let base_config_path = temp.path().join("config-nero.toml");
        let msg_config_path = temp.path().join("config-nero-hook-msg.toml");
        let dev_config_path = temp.path().join("config-nero-dev.toml");
        let state_path = temp.path().join("nero-hook-auto-state.json");
        std::fs::write(&base_config_path, "").expect("write base config");
        std::fs::write(&msg_config_path, "").expect("write msg config");
        std::fs::write(&dev_config_path, "").expect("write dev config");
        std::fs::write(
            &config_path,
            format!(
                r#"
[nero.hook.runtime.auto]
enabled = false
[nero.hook.runtime.auto.state]
path = "{}"
[nero.hook.runtime.auto.policy]
autonomy_level = 5
max_auto_rounds = 7
autonomy_step_per_round = 1.0
done_stop_scope = "active_phase"
"#,
                state_path.display()
            ),
        )
        .expect("write config");

        configure_isolated_runtime_config_env(
            &config_path,
            &base_config_path,
            &msg_config_path,
            &dev_config_path,
        );
        let context = sample_context();
        let initial = read_thread_session_auto(temp.path(), &context)
            .await
            .expect("read initial");

        let updated = update_thread_session_auto(
            temp.path(),
            &context,
            &ThreadSessionAutoUpdateParams {
                thread_id: context.thread_id.clone(),
                expected_version: initial.state.version.clone(),
                expected_session_source: None,
                enabled: Some(Some(true)),
                autonomy_level: Some(Some(9)),
                autonomy_step_per_round: Some(Some(1.5)),
                max_auto_rounds: Some(Some(12)),
                done_stop_scope: Some(Some("campaign".to_string())),
                auto_rounds: Some(Some(4)),
                reset_counter: false,
            },
        )
        .await
        .expect("update");
        assert!(updated.applied);
        let state = updated.state.expect("state");
        assert_eq!(state.applied.enabled, Some(true));
        assert_eq!(state.applied.autonomy_level, Some(9));
        assert_eq!(state.applied.max_auto_rounds, Some(12));
        assert_eq!(state.applied.done_stop_scope.as_deref(), Some("campaign"));
        assert_eq!(state.applied.auto_rounds, 4);

        let cleared = update_thread_session_auto(
            temp.path(),
            &context,
            &ThreadSessionAutoUpdateParams {
                thread_id: context.thread_id.clone(),
                expected_version: state.version.clone(),
                expected_session_source: None,
                enabled: Some(None),
                autonomy_level: Some(None),
                autonomy_step_per_round: Some(None),
                max_auto_rounds: Some(None),
                done_stop_scope: Some(None),
                auto_rounds: Some(None),
                reset_counter: false,
            },
        )
        .await
        .expect("clear");
        assert!(cleared.applied);
        let cleared_state = cleared.state.expect("cleared state");
        assert_eq!(cleared_state.applied.enabled, None);
        assert_eq!(cleared_state.applied.autonomy_level, None);
        assert_eq!(cleared_state.applied.max_auto_rounds, None);
        assert_eq!(cleared_state.applied.done_stop_scope, None);
    }

    #[tokio::test]
    async fn update_conflicts_on_version_mismatch() {
        let _guard = runtime_auto_test_guard().lock().await;
        let temp = tempdir().expect("tempdir");
        let config_path = temp.path().join("config-nero-hook-auto.toml");
        let base_config_path = temp.path().join("config-nero.toml");
        let msg_config_path = temp.path().join("config-nero-hook-msg.toml");
        let dev_config_path = temp.path().join("config-nero-dev.toml");
        let state_path = temp.path().join("nero-hook-auto-state.json");
        std::fs::write(&base_config_path, "").expect("write base config");
        std::fs::write(&msg_config_path, "").expect("write msg config");
        std::fs::write(&dev_config_path, "").expect("write dev config");
        std::fs::write(
            &config_path,
            format!(
                r#"
[nero.hook.runtime.auto]
enabled = false
[nero.hook.runtime.auto.state]
path = "{}"
"#,
                state_path.display()
            ),
        )
        .expect("write config");
        configure_isolated_runtime_config_env(
            &config_path,
            &base_config_path,
            &msg_config_path,
            &dev_config_path,
        );
        let context = sample_context();
        let initial = read_thread_session_auto(temp.path(), &context)
            .await
            .expect("read initial");
        let update = update_thread_session_auto(
            temp.path(),
            &context,
            &ThreadSessionAutoUpdateParams {
                thread_id: context.thread_id.clone(),
                expected_version: "sha256:stale".to_string(),
                expected_session_source: None,
                enabled: Some(Some(true)),
                autonomy_level: None,
                autonomy_step_per_round: None,
                max_auto_rounds: None,
                done_stop_scope: None,
                auto_rounds: None,
                reset_counter: false,
            },
        )
        .await
        .expect("update response");
        assert!(!update.applied);
        assert!(update.conflict);
        assert_eq!(update.error_code.as_deref(), Some("version_conflict"));
        assert_eq!(
            update.state.as_ref().map(|state| state.version.clone()),
            Some(initial.state.version)
        );
    }
}
