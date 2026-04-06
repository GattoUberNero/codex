use chrono::Utc;
use codex_app_server_protocol::SessionSource;
use codex_app_server_protocol::ThreadSessionAutoApplied;
use codex_app_server_protocol::ThreadSessionAutoAuthorityMode;
use codex_app_server_protocol::ThreadSessionAutoDefaults;
use codex_app_server_protocol::ThreadSessionAutoEffective;
use codex_app_server_protocol::ThreadSessionAutoReadResponse;
use codex_app_server_protocol::ThreadSessionAutoState;
use codex_app_server_protocol::ThreadSessionAutoUpdateParams;
use codex_app_server_protocol::ThreadSessionAutoUpdateResponse;
use codex_protocol::protocol::NeroAutoRuntimeConfig;
use serde_json::Map as JsonMap;
use serde_json::Value as JsonValue;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeSet;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use toml::Table as TomlTable;
use toml::Value as TomlValue;

const NERO_AUTO_RUNTIME_CONFIG_ENV: &str = "CODEXN_CONFIG_NERO_AUTO_PATH";
const CODEXN_CONFIG_NERO_PATH_ENV: &str = "CODEXN_CONFIG_NERO_PATH";
const CODEXN_CONFIG_NERO_MSG_PATH_ENV: &str = "CODEXN_CONFIG_NERO_MSG_PATH";
const CODEXN_CONFIG_NERO_DEV_PATH_ENV: &str = "CODEXN_CONFIG_NERO_DEV_PATH";
const CODEXN_CONFIG_NERO_MERGE_PATHS_ENV: &str = "CODEXN_CONFIG_NERO_MERGE_PATHS";
const NERO_HOOK_AUTO_ENABLED_ENV: &str = "NERO_HOOK_AUTO_ENABLED";
const NERO_HOOK_AUTO_AUTONOMY_LEVEL_ENV: &str = "NERO_HOOK_AUTO_AUTONOMY_LEVEL";
const NERO_HOOK_AUTO_MAX_ROUNDS_ENV: &str = "NERO_HOOK_AUTO_MAX_ROUNDS";
const NERO_HOOK_AUTO_STEP_PER_ROUND_ENV: &str = "NERO_HOOK_AUTO_STEP_PER_ROUND";

#[derive(Debug, Clone)]
pub(crate) struct NeroThreadSessionAutoContext {
    pub(crate) thread_id: String,
    pub(crate) thread_name: Option<String>,
    pub(crate) session_source: SessionSource,
    pub(crate) loaded: bool,
}

#[derive(Debug, Clone)]
struct NeroRuntimeDefaults {
    runtime: NeroAutoRuntimeConfig,
    autonomy_step_per_round: f64,
    done_stop_scope: String,
}

#[derive(Debug)]
struct NeroStateSnapshot {
    root: JsonMap<String, JsonValue>,
    version: String,
}

struct NeroStateLock {
    lock_file: File,
}

impl Drop for NeroStateLock {
    fn drop(&mut self) {
        let _ = self.lock_file.unlock();
    }
}

fn first_non_empty_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name).ok().and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    })
}

fn expand_user_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    if trimmed == "~"
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home);
    }
    PathBuf::from(trimmed)
}

fn bool_from_env(name: &str) -> Option<bool> {
    let raw = std::env::var(name).ok()?;
    match raw.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn i64_from_env(name: &str) -> Option<i64> {
    std::env::var(name).ok()?.trim().parse::<i64>().ok()
}

fn f64_from_env(name: &str) -> Option<f64> {
    let value = std::env::var(name).ok()?.trim().parse::<f64>().ok()?;
    if value.is_finite() { Some(value) } else { None }
}

fn bool_from_json(value: Option<&JsonValue>) -> Option<bool> {
    value.and_then(JsonValue::as_bool)
}

fn i64_from_json(value: Option<&JsonValue>) -> Option<i64> {
    value.and_then(|raw| if raw.is_boolean() { None } else { raw.as_i64() })
}

fn f64_from_json(value: Option<&JsonValue>) -> Option<f64> {
    value.and_then(|raw| if raw.is_boolean() { None } else { raw.as_f64() })
}

fn string_from_json(value: Option<&JsonValue>) -> Option<String> {
    value
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn clamp_runtime(runtime: NeroAutoRuntimeConfig) -> NeroAutoRuntimeConfig {
    NeroAutoRuntimeConfig {
        enabled: runtime.enabled,
        autonomy_level: runtime.autonomy_level.clamp(1, 10),
        max_auto_rounds: runtime.max_auto_rounds.max(0),
    }
}

fn normalize_done_stop_scope(
    done_stop_scope: Option<&str>,
    stop_when_all_gates_done: Option<bool>,
    default: &str,
) -> String {
    if let Some(scope) = done_stop_scope {
        let normalized = scope.trim().to_lowercase();
        if matches!(
            normalized.as_str(),
            "active_phase" | "campaign" | "disabled"
        ) {
            return normalized;
        }
    }
    if let Some(flag) = stop_when_all_gates_done {
        return if flag {
            "active_phase".to_string()
        } else {
            "disabled".to_string()
        };
    }
    let fallback = default.trim().to_lowercase();
    if matches!(fallback.as_str(), "active_phase" | "campaign" | "disabled") {
        fallback
    } else {
        "active_phase".to_string()
    }
}

fn is_subagent_session_source(session_source: &SessionSource) -> bool {
    matches!(session_source, SessionSource::SubAgent(_))
}

fn session_source_wire_value(session_source: &SessionSource) -> &str {
    match session_source {
        SessionSource::Cli => "cli",
        SessionSource::VsCode => "vscode",
        SessionSource::Exec => "exec",
        SessionSource::AppServer => "mcp",
        SessionSource::Custom(source) => source.as_str(),
        SessionSource::SubAgent(_) => "subAgent",
        SessionSource::Unknown => "unknown",
    }
}

fn resolve_nero_auto_config_path(codex_home: &Path) -> PathBuf {
    if let Some(path) = first_non_empty_env(&[NERO_AUTO_RUNTIME_CONFIG_ENV]) {
        return expand_user_path(&path);
    }
    codex_home.join("config-nero-hook-auto.toml")
}

fn load_toml_document(path: &Path) -> Option<TomlTable> {
    let content = std::fs::read_to_string(path).ok()?;
    let parsed = content.parse::<TomlValue>().ok()?;
    parsed.as_table().cloned()
}

fn merge_toml_tables(base: &mut TomlTable, overlay: TomlTable) {
    for (key, value) in overlay {
        if let Some(existing) = base.get_mut(&key)
            && let (Some(existing_table), Some(overlay_table)) =
                (existing.as_table_mut(), value.as_table())
        {
            merge_toml_tables(existing_table, overlay_table.clone());
            continue;
        }
        base.insert(key, value);
    }
}

fn default_base_config_path() -> PathBuf {
    first_non_empty_env(&[CODEXN_CONFIG_NERO_PATH_ENV])
        .map(|value| expand_user_path(&value))
        .unwrap_or_else(|| expand_user_path("~/.codex/config-nero.toml"))
}

fn extra_merge_config_paths() -> Vec<PathBuf> {
    let Some(raw) = first_non_empty_env(&[CODEXN_CONFIG_NERO_MERGE_PATHS_ENV]) else {
        return Vec::new();
    };
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(expand_user_path)
        .collect()
}

fn runtime_config_layers(auto_config_path: &Path) -> Vec<PathBuf> {
    let base_path = default_base_config_path();
    let msg_paths = if let Some(path) = first_non_empty_env(&[CODEXN_CONFIG_NERO_MSG_PATH_ENV]) {
        vec![expand_user_path(&path)]
    } else {
        vec![
            expand_user_path("~/.codex/config-nero-hook-msg.toml"),
            base_path.with_file_name("config-nero-hook-msg.toml"),
        ]
    };
    let dev_paths = if let Some(path) = first_non_empty_env(&[CODEXN_CONFIG_NERO_DEV_PATH_ENV]) {
        vec![expand_user_path(&path)]
    } else {
        vec![
            expand_user_path("~/.codex/config-nero-dev.toml"),
            base_path.with_file_name("config-nero-dev.toml"),
        ]
    };

    let mut dedupe = BTreeSet::new();
    let mut out = Vec::new();
    for path in std::iter::once(base_path)
        .chain(msg_paths)
        .chain(std::iter::once(auto_config_path.to_path_buf()))
        .chain(dev_paths)
        .chain(extra_merge_config_paths())
    {
        if dedupe.insert(path.clone()) {
            out.push(path);
        }
    }
    out
}

fn merged_runtime_document(auto_config_path: &Path) -> TomlTable {
    let mut merged = TomlTable::new();
    for layer in runtime_config_layers(auto_config_path) {
        if let Some(table) = load_toml_document(&layer) {
            merge_toml_tables(&mut merged, table);
        }
    }
    merged
}

fn runtime_table_from_merged_doc(merged_doc: &TomlTable) -> TomlTable {
    merged_doc
        .get("nero")
        .and_then(TomlValue::as_table)
        .and_then(|nero| nero.get("hook"))
        .and_then(TomlValue::as_table)
        .and_then(|hook| hook.get("runtime"))
        .and_then(TomlValue::as_table)
        .cloned()
        .unwrap_or_default()
}

fn resolve_nero_auto_state_path(auto_config_path: &Path) -> PathBuf {
    let merged = merged_runtime_document(auto_config_path);
    let path = merged
        .get("nero")
        .and_then(TomlValue::as_table)
        .and_then(|nero| nero.get("hook"))
        .and_then(TomlValue::as_table)
        .and_then(|hook| hook.get("runtime"))
        .and_then(TomlValue::as_table)
        .and_then(|runtime| runtime.get("auto"))
        .and_then(TomlValue::as_table)
        .and_then(|auto| auto.get("state"))
        .and_then(TomlValue::as_table)
        .and_then(|state| state.get("path"))
        .and_then(TomlValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(expand_user_path);
    path.unwrap_or_else(|| expand_user_path("~/.codex/log/nero-hook-auto-state.json"))
}

fn runtime_delivery_requires_runtime_msg(auto_config_path: &Path) -> bool {
    merged_runtime_document(auto_config_path)
        .get("nero")
        .and_then(TomlValue::as_table)
        .and_then(|nero| nero.get("hook"))
        .and_then(TomlValue::as_table)
        .and_then(|hook| hook.get("runtime"))
        .and_then(TomlValue::as_table)
        .and_then(|runtime| runtime.get("delivery"))
        .and_then(TomlValue::as_table)
        .and_then(|delivery| delivery.get("require_runtime_msg_for_auto"))
        .and_then(TomlValue::as_bool)
        .unwrap_or(true)
}

fn resolve_runtime_defaults(
    auto_config_path: &Path,
    session_source: &SessionSource,
) -> NeroRuntimeDefaults {
    let merged = merged_runtime_document(auto_config_path);
    let runtime = runtime_table_from_merged_doc(&merged);
    let auto = runtime
        .get("auto")
        .and_then(TomlValue::as_table)
        .cloned()
        .unwrap_or_default();
    let policy = auto
        .get("policy")
        .and_then(TomlValue::as_table)
        .cloned()
        .unwrap_or_default();

    let mut enabled = auto
        .get("enabled")
        .and_then(TomlValue::as_bool)
        .unwrap_or(false);
    let mut autonomy_level = policy
        .get("autonomy_level")
        .and_then(TomlValue::as_integer)
        .unwrap_or(5)
        .clamp(1, 10);
    let mut max_auto_rounds = policy
        .get("max_auto_rounds")
        .and_then(TomlValue::as_integer)
        .unwrap_or(7)
        .max(0);
    let mut autonomy_step_per_round = policy
        .get("autonomy_step_per_round")
        .and_then(TomlValue::as_float)
        .unwrap_or(1.0)
        .clamp(0.0, 10.0);

    if let Some(value) = bool_from_env(NERO_HOOK_AUTO_ENABLED_ENV) {
        enabled = value;
    }
    if let Some(value) = i64_from_env(NERO_HOOK_AUTO_AUTONOMY_LEVEL_ENV) {
        autonomy_level = value.clamp(1, 10);
    }
    if let Some(value) = i64_from_env(NERO_HOOK_AUTO_MAX_ROUNDS_ENV) {
        max_auto_rounds = value.max(0);
    }
    if let Some(value) = f64_from_env(NERO_HOOK_AUTO_STEP_PER_ROUND_ENV) {
        autonomy_step_per_round = value.clamp(0.0, 10.0);
    }

    let done_stop_scope = normalize_done_stop_scope(
        policy.get("done_stop_scope").and_then(TomlValue::as_str),
        policy
            .get("stop_when_all_gates_done")
            .and_then(TomlValue::as_bool),
        "active_phase",
    );

    let runtime = if is_subagent_session_source(session_source) {
        NeroAutoRuntimeConfig {
            enabled: false,
            autonomy_level,
            max_auto_rounds,
        }
    } else {
        NeroAutoRuntimeConfig {
            enabled,
            autonomy_level,
            max_auto_rounds,
        }
    };

    NeroRuntimeDefaults {
        runtime: clamp_runtime(runtime),
        autonomy_step_per_round: (autonomy_step_per_round * 1000.0).round() / 1000.0,
        done_stop_scope,
    }
}

fn compute_state_version(raw: &str) -> String {
    let digest = Sha256::digest(raw.as_bytes());
    format!("sha256:{digest:x}")
}

fn read_state_snapshot(path: &Path) -> Result<NeroStateSnapshot, String> {
    let raw = if path.is_file() {
        std::fs::read_to_string(path).map_err(|err| {
            format!(
                "failed to read session-auto state {}: {err}",
                path.display()
            )
        })?
    } else {
        String::new()
    };
    let mut root = if raw.trim().is_empty() {
        JsonMap::new()
    } else {
        match serde_json::from_str::<JsonValue>(&raw) {
            Ok(JsonValue::Object(obj)) => obj,
            Ok(_) => {
                return Err(format!(
                    "session-auto state {} is corrupted: root must be a JSON object",
                    path.display()
                ));
            }
            Err(err) => {
                return Err(format!(
                    "session-auto state {} is corrupted: {err}",
                    path.display()
                ));
            }
        }
    };
    if let Some(threads) = root.get("threads")
        && !threads.is_object()
    {
        return Err(format!(
            "session-auto state {} is corrupted: `threads` must be a JSON object",
            path.display()
        ));
    }
    if !matches!(root.get("threads"), Some(JsonValue::Object(_))) {
        root.insert("threads".to_string(), JsonValue::Object(JsonMap::new()));
    }
    Ok(NeroStateSnapshot {
        root,
        version: compute_state_version(&raw),
    })
}

fn write_state_snapshot_atomic(
    path: &Path,
    root: &JsonMap<String, JsonValue>,
) -> Result<(), String> {
    let serialized = serde_json::to_string(root)
        .map_err(|err| format!("failed to serialize session-auto state: {err}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create session-auto state dir {}: {err}",
                parent.display()
            )
        })?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid session-auto state path: {}", path.display()))?;
    let temp_path = path.with_file_name(format!(".{file_name}.tmp"));
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)
            .map_err(|err| format!("failed to open session-auto temp file: {err}"))?;
        file.write_all(serialized.as_bytes())
            .map_err(|err| format!("failed to write session-auto temp file: {err}"))?;
        file.sync_all()
            .map_err(|err| format!("failed to fsync session-auto temp file: {err}"))?;
    }
    std::fs::rename(&temp_path, path).map_err(|err| {
        format!(
            "failed to replace session-auto state {}: {err}",
            path.display()
        )
    })?;
    Ok(())
}

fn acquire_state_lock(path: &Path) -> Result<NeroStateLock, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid session-auto state path: {}", path.display()))?;
    let lock_path = path.with_file_name(format!(".{file_name}.lock"));
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create session-auto lock dir {}: {err}",
                parent.display()
            )
        })?;
    }
    let mut lock_file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|err| format!("failed to open session-auto lock file: {err}"))?;
    lock_file
        .try_lock()
        .map_err(|_| "state lock is unavailable".to_string())?;
    lock_file
        .set_len(0)
        .map_err(|err| format!("failed to reset session-auto lock file: {err}"))?;
    writeln!(lock_file, "{}", std::process::id())
        .map_err(|err| format!("failed to write session-auto lock owner: {err}"))?;
    Ok(NeroStateLock { lock_file })
}

fn threads_map_mut(root: &mut JsonMap<String, JsonValue>) -> &mut JsonMap<String, JsonValue> {
    let threads = root
        .entry("threads".to_string())
        .or_insert_with(|| JsonValue::Object(JsonMap::new()));
    if !threads.is_object() {
        *threads = JsonValue::Object(JsonMap::new());
    }
    match threads {
        JsonValue::Object(map) => map,
        _ => unreachable!("threads always object"),
    }
}

fn read_thread_state_entry(
    threads: &JsonMap<String, JsonValue>,
    thread_key: &str,
) -> (JsonMap<String, JsonValue>, Option<String>) {
    if let Some(JsonValue::Object(entry)) = threads.get(thread_key) {
        return (entry.clone(), None);
    }
    if let Some(legacy_key) = thread_key.strip_prefix("id:") {
        if legacy_key.starts_with("id:")
            || legacy_key.starts_with("name:")
            || legacy_key.starts_with("turn:")
        {
            return (JsonMap::new(), None);
        }
        if let Some(JsonValue::Object(entry)) = threads.get(legacy_key) {
            return (entry.clone(), Some(legacy_key.to_string()));
        }
    }
    (JsonMap::new(), None)
}

fn normalized_policy_override_from_entry(
    entry: &JsonMap<String, JsonValue>,
) -> Option<JsonMap<String, JsonValue>> {
    let policy = entry
        .get("session_auto_policy_override")
        .and_then(JsonValue::as_object)?;
    let mut out = JsonMap::new();
    if let Some(level) = i64_from_json(policy.get("autonomy_level"))
        && (1..=10).contains(&level)
    {
        out.insert(
            "autonomy_level".to_string(),
            JsonValue::Number(level.into()),
        );
    }
    if let Some(step) = f64_from_json(policy.get("autonomy_step_per_round"))
        && (0.0..=10.0).contains(&step)
        && let Some(number) = serde_json::Number::from_f64((step * 1000.0).round() / 1000.0)
    {
        out.insert(
            "autonomy_step_per_round".to_string(),
            JsonValue::Number(number),
        );
    }
    if let Some(max_rounds) = i64_from_json(policy.get("max_auto_rounds"))
        && (0..=1000).contains(&max_rounds)
    {
        out.insert(
            "max_auto_rounds".to_string(),
            JsonValue::Number(max_rounds.into()),
        );
    }
    let done_scope = normalize_done_stop_scope(
        string_from_json(policy.get("done_stop_scope")).as_deref(),
        bool_from_json(policy.get("stop_when_all_gates_done")),
        "",
    );
    if !done_scope.is_empty() {
        out.insert("done_stop_scope".to_string(), JsonValue::String(done_scope));
    }
    if out.is_empty() { None } else { Some(out) }
}

fn build_session_auto_state(
    context: &NeroThreadSessionAutoContext,
    state_path: &Path,
    config_path: &Path,
    snapshot: &NeroStateSnapshot,
) -> ThreadSessionAutoState {
    let defaults = resolve_runtime_defaults(config_path, &context.session_source);
    let thread_key = format!("id:{}", context.thread_id);
    let threads = snapshot
        .root
        .get("threads")
        .and_then(JsonValue::as_object)
        .cloned()
        .unwrap_or_default();
    let (entry, _) = read_thread_state_entry(&threads, &thread_key);

    let applied_enabled = bool_from_json(entry.get("session_auto_enabled_override"));
    let applied_policy = normalized_policy_override_from_entry(&entry);
    let auto_rounds = i64_from_json(entry.get("auto_rounds")).unwrap_or(0).max(0);
    let updated_at = string_from_json(entry.get("session_auto_override_updated_at"));

    let is_subagent = is_subagent_session_source(&context.session_source);
    let effective_enabled = if is_subagent {
        false
    } else if let Some(value) = applied_enabled {
        value
    } else {
        defaults.runtime.enabled
    };
    let effective_source = if is_subagent {
        "subagent-forced-off".to_string()
    } else if applied_enabled.is_some() || applied_policy.is_some() {
        "session-override".to_string()
    } else {
        "config-default".to_string()
    };

    let policy_level = applied_policy
        .as_ref()
        .and_then(|policy| i64_from_json(policy.get("autonomy_level")));
    let policy_step = applied_policy
        .as_ref()
        .and_then(|policy| f64_from_json(policy.get("autonomy_step_per_round")));
    let policy_rounds = applied_policy
        .as_ref()
        .and_then(|policy| i64_from_json(policy.get("max_auto_rounds")));
    let policy_done_scope = applied_policy
        .as_ref()
        .and_then(|policy| string_from_json(policy.get("done_stop_scope")));

    let effective_runtime = clamp_runtime(NeroAutoRuntimeConfig {
        enabled: effective_enabled,
        autonomy_level: policy_level.unwrap_or(defaults.runtime.autonomy_level),
        max_auto_rounds: policy_rounds.unwrap_or(defaults.runtime.max_auto_rounds),
    });

    ThreadSessionAutoState {
        thread_name: context.thread_name.clone(),
        session_source: context.session_source.clone(),
        loaded: context.loaded,
        config_path: config_path.to_path_buf(),
        state_path: state_path.to_path_buf(),
        version: snapshot.version.clone(),
        is_subagent,
        defaults: ThreadSessionAutoDefaults {
            runtime: defaults.runtime,
            autonomy_step_per_round: defaults.autonomy_step_per_round,
            done_stop_scope: defaults.done_stop_scope.clone(),
        },
        applied: ThreadSessionAutoApplied {
            enabled: applied_enabled,
            autonomy_level: policy_level,
            autonomy_step_per_round: policy_step,
            max_auto_rounds: policy_rounds,
            done_stop_scope: policy_done_scope.clone(),
            auto_rounds,
            updated_at,
        },
        effective: ThreadSessionAutoEffective {
            runtime: effective_runtime,
            autonomy_step_per_round: policy_step.unwrap_or(defaults.autonomy_step_per_round),
            done_stop_scope: policy_done_scope.unwrap_or(defaults.done_stop_scope),
            auto_rounds,
            source: effective_source,
        },
    }
}

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
    use super::CODEXN_CONFIG_NERO_DEV_PATH_ENV;
    use super::CODEXN_CONFIG_NERO_MERGE_PATHS_ENV;
    use super::CODEXN_CONFIG_NERO_MSG_PATH_ENV;
    use super::CODEXN_CONFIG_NERO_PATH_ENV;
    use super::NERO_AUTO_RUNTIME_CONFIG_ENV;
    use super::NERO_HOOK_AUTO_AUTONOMY_LEVEL_ENV;
    use super::NERO_HOOK_AUTO_ENABLED_ENV;
    use super::NERO_HOOK_AUTO_MAX_ROUNDS_ENV;
    use super::NERO_HOOK_AUTO_STEP_PER_ROUND_ENV;
    use super::NeroThreadSessionAutoContext;
    use super::read_thread_session_auto;
    use super::update_thread_session_auto;
    use codex_app_server_protocol::SessionSource;
    use codex_app_server_protocol::ThreadSessionAutoUpdateParams;
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
