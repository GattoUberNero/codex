use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::DefaultHasher;
use std::fmt::Debug;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicU64;
use std::time::Duration as StdDuration;
use std::time::Instant as StdInstant;

use crate::AuthManager;
use crate::CodexAuth;
use crate::SandboxState;
use crate::agent::AgentControl;
use crate::agent::AgentStatus;
use crate::agent::Mailbox;
use crate::agent::MailboxReceiver;
use crate::agent::agent_status_from_event;
use crate::apps::render_apps_section;
use crate::auth_env_telemetry::collect_auth_env_telemetry;
use crate::commit_attribution::commit_message_trailer_instruction;
use crate::compact;
use crate::compact::InitialContextInjection;
use crate::compact::run_inline_auto_compact_task;
use crate::compact::should_use_remote_compact_task;
use crate::compact_remote::run_inline_remote_auto_compact_task;
use crate::config::ManagedFeatures;
use crate::config::NeroModelFallbackConfig;
use crate::config::NeroModelFallbackStep;
use crate::connectors;
use crate::exec_policy::ExecPolicyManager;
#[cfg(test)]
use crate::models_manager::collaboration_mode_presets::CollaborationModesConfig;
use crate::models_manager::manager::ModelsManager;
use crate::models_manager::manager::RefreshStrategy;
use crate::nero_auto_runtime_state::NeroStopHookDebugReportingMode;
use crate::nero_auto_runtime_state::resolve_nero_auto_config_path;
use crate::nero_auto_runtime_state::resolve_stop_hook_debug_reporting_mode;
use crate::parse_command::parse_command;
use crate::parse_turn_item;
use crate::path_utils::normalize_for_native_workdir;
use crate::realtime_conversation::RealtimeConversationManager;
use crate::realtime_conversation::handle_audio as handle_realtime_conversation_audio;
use crate::realtime_conversation::handle_close as handle_realtime_conversation_close;
use crate::realtime_conversation::handle_start as handle_realtime_conversation_start;
use crate::realtime_conversation::handle_text as handle_realtime_conversation_text;
use crate::render_skills_section;
use crate::rollout::session_index;
use crate::skills_load_input_from_config;
use crate::stream_events_utils::HandleOutputCtx;
use crate::stream_events_utils::handle_non_tool_response_item;
use crate::stream_events_utils::handle_output_item_done;
use crate::stream_events_utils::last_assistant_message_from_item;
use crate::stream_events_utils::raw_assistant_output_text_from_item;
use crate::stream_events_utils::record_completed_response_item;
use crate::turn_metadata::TurnMetadataState;
use crate::util::error_or_panic;
use async_channel::Receiver;
use async_channel::Sender;
use chrono::Local;
use chrono::Utc;
use codex_analytics::AnalyticsEventsClient;
use codex_analytics::AppInvocation;
use codex_analytics::InvocationType;
use codex_analytics::build_track_events_context;
use codex_app_server_protocol::McpServerElicitationRequest;
use codex_app_server_protocol::McpServerElicitationRequestParams;
use codex_exec_server::Environment;
use codex_exec_server::EnvironmentManager;
use codex_features::FEATURES;
use codex_features::Feature;
use codex_features::unstable_features_warning_event;
use codex_hooks::HookEvent;
use codex_hooks::HookEventAfterAgent;
use codex_hooks::HookPayload;
use codex_hooks::HookResult;
use codex_hooks::Hooks;
use codex_hooks::HooksConfig;
use codex_hooks::NeroHookAction;
use codex_hooks::NeroHookMsgFormat;
use codex_network_proxy::NetworkProxy;
use codex_network_proxy::NetworkProxyAuditMetadata;
use codex_network_proxy::normalize_host;
use codex_otel::current_span_trace_id;
use codex_otel::current_span_w3c_trace_context;
use codex_otel::set_parent_from_w3c_trace_context;
use codex_protocol::ThreadId;
use codex_protocol::approvals::ElicitationRequestEvent;
use codex_protocol::approvals::ExecPolicyAmendment;
use codex_protocol::approvals::NetworkPolicyAmendment;
use codex_protocol::approvals::NetworkPolicyRuleAction;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::config_types::ModeKind;
use codex_protocol::config_types::Settings;
use codex_protocol::config_types::WebSearchMode;
use codex_protocol::dynamic_tools::DynamicToolResponse;
use codex_protocol::dynamic_tools::DynamicToolSpec;
use codex_protocol::items::PlanItem;
use codex_protocol::items::TurnItem;
use codex_protocol::items::UserMessageItem;
use codex_protocol::items::build_hook_prompt_message;
use codex_protocol::items::parse_hook_prompt_message;
use codex_protocol::mcp::CallToolResult;
use codex_protocol::models::BaseInstructions;
use codex_protocol::models::PermissionProfile;
use codex_protocol::models::format_allow_prefixes;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::HasLegacyEvent;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::ItemStartedEvent;
use codex_protocol::protocol::RawResponseItemEvent;
use codex_protocol::protocol::ReviewRequest;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use codex_protocol::protocol::TurnAbortReason;
use codex_protocol::protocol::TurnContextItem;
use codex_protocol::protocol::TurnContextNetworkItem;
use codex_protocol::protocol::TurnStartedEvent;
use codex_protocol::protocol::W3cTraceContext;
use codex_protocol::request_permissions::PermissionGrantScope;
use codex_protocol::request_permissions::RequestPermissionProfile;
use codex_protocol::request_permissions::RequestPermissionsArgs;
use codex_protocol::request_permissions::RequestPermissionsEvent;
use codex_protocol::request_permissions::RequestPermissionsResponse;
use codex_protocol::request_user_input::RequestUserInputArgs;
use codex_protocol::request_user_input::RequestUserInputResponse;
use codex_rmcp_client::ElicitationResponse;
use codex_rmcp_client::OAuthCredentialsStoreMode;
use codex_terminal_detection::user_agent;
use codex_tools::filter_tool_suggest_discoverable_tools_for_client;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_stream_parser::AssistantTextChunk;
use codex_utils_stream_parser::AssistantTextStreamParser;
use codex_utils_stream_parser::ProposedPlanSegment;
use codex_utils_stream_parser::extract_proposed_plan_text;
use codex_utils_stream_parser::strip_citations;
use futures::future::BoxFuture;
use futures::future::Shared;
use futures::prelude::*;
use futures::stream::FuturesOrdered;
use rmcp::model::ListResourceTemplatesResult;
use rmcp::model::ListResourcesResult;
use rmcp::model::PaginatedRequestParams;
use rmcp::model::ReadResourceRequestParams;
use rmcp::model::ReadResourceResult;
use rmcp::model::RequestId;
use serde_json;
use serde_json::Value;
use serde_json::json;
use tokio::fs::OpenOptions as TokioOpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use toml::Value as TomlValue;
use tracing::Instrument;
use tracing::debug;
use tracing::debug_span;
use tracing::error;
use tracing::field;
use tracing::info;
use tracing::info_span;
use tracing::instrument;
use tracing::trace;
use tracing::trace_span;
use tracing::warn;
use uuid::Uuid;

use crate::ModelProviderInfo;
use crate::client::ModelClient;
use crate::client::ModelClientSession;
use crate::client_common::Prompt;
use crate::client_common::ResponseEvent;
use crate::codex_thread::ThreadConfigSnapshot;
use crate::compact::collect_user_messages;
use crate::config::Config;
use crate::config::Constrained;
use crate::config::ConstraintResult;
use crate::config::GhostSnapshotConfig;
use crate::config::StartedNetworkProxy;
use crate::config::resolve_web_search_mode_for_turn;
use crate::config::types::McpServerConfig;
use crate::config::types::ShellEnvironmentPolicy;
use crate::context_manager::ContextManager;
use crate::context_manager::TotalTokenUsageBreakdown;
use crate::environment_context::EnvironmentContext;
use crate::error::CodexErr;
use crate::error::Result as CodexResult;
#[cfg(test)]
use crate::exec::StreamOutput;
use codex_config::CONFIG_TOML_FILE;
use codex_protocol::protocol::NeroAutoRuntimeConfig;

mod rollout_reconstruction;
#[cfg(test)]
mod rollout_reconstruction_tests;

#[derive(Debug, PartialEq)]
pub enum SteerInputError {
    NoActiveTurn(Vec<UserInput>),
    ExpectedTurnMismatch { expected: String, actual: String },
    ActiveTurnNotSteerable { turn_kind: NonSteerableTurnKind },
    EmptyInput,
}

impl SteerInputError {
    fn to_error_event(&self) -> ErrorEvent {
        match self {
            Self::NoActiveTurn(_) => ErrorEvent {
                message: "no active turn to steer".to_string(),
                codex_error_info: Some(CodexErrorInfo::BadRequest),
            },
            Self::ExpectedTurnMismatch { expected, actual } => ErrorEvent {
                message: format!("expected active turn id `{expected}` but found `{actual}`"),
                codex_error_info: Some(CodexErrorInfo::BadRequest),
            },
            Self::ActiveTurnNotSteerable { turn_kind } => {
                let turn_kind_label = match turn_kind {
                    NonSteerableTurnKind::Review => "review",
                    NonSteerableTurnKind::Compact => "compact",
                };
                ErrorEvent {
                    message: format!("cannot steer a {turn_kind_label} turn"),
                    codex_error_info: Some(CodexErrorInfo::ActiveTurnNotSteerable {
                        turn_kind: *turn_kind,
                    }),
                }
            }
            Self::EmptyInput => ErrorEvent {
                message: "input must not be empty".to_string(),
                codex_error_info: Some(CodexErrorInfo::BadRequest),
            },
        }
    }
}

/// Notes from the previous real user turn.
///
/// Conceptually this is the same role that `previous_model` used to fill, but
/// it can carry other prior-turn settings that matter when constructing
/// sensible state-change diffs or full-context reinjection, such as model
/// switches or detecting a prior `realtime_active -> false` transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PreviousTurnSettings {
    pub(crate) model: String,
    pub(crate) realtime_active: Option<bool>,
}

use crate::SkillError;
use crate::SkillInjections;
use crate::SkillLoadOutcome;
use crate::SkillMetadata;
use crate::SkillsManager;
use crate::build_skill_injections;
use crate::collect_env_var_dependencies;
use crate::collect_explicit_skill_mentions;
use crate::exec_policy::ExecPolicyUpdateError;
use crate::feedback_tags;
use crate::guardian::GuardianReviewSessionManager;
use crate::hook_runtime::PendingInputHookDisposition;
use crate::hook_runtime::inspect_pending_input;
use crate::hook_runtime::record_additional_contexts;
use crate::hook_runtime::record_pending_input;
use crate::hook_runtime::run_pending_session_start_hooks;
use crate::hook_runtime::run_user_prompt_submit_hooks;
use crate::injection::ToolMentionKind;
use crate::injection::app_id_from_path;
use crate::injection::tool_kind_for_path;
use crate::instructions::UserInstructions;
use crate::mcp::CODEX_APPS_MCP_SERVER_NAME;
use crate::mcp::McpManager;
use crate::mcp::auth::compute_auth_statuses;
use crate::mcp::maybe_prompt_and_install_mcp_dependencies;
use crate::mcp::with_codex_apps_mcp;
use crate::mcp_connection_manager::McpConnectionManager;
use crate::mcp_connection_manager::codex_apps_tools_cache_key;
use crate::mcp_connection_manager::filter_non_codex_apps_mcp_tools_only;
use crate::memories;
use crate::mentions::build_connector_slug_counts;
use crate::mentions::build_skill_name_counts;
use crate::mentions::collect_explicit_app_ids;
use crate::mentions::collect_explicit_plugin_mentions;
use crate::mentions::collect_tool_mentions_from_messages;
use crate::network_policy_decision::execpolicy_network_rule_amendment;
use crate::plugins::PluginsManager;
use crate::plugins::build_plugin_injections;
use crate::plugins::render_plugins_section;
use crate::project_doc::get_user_instructions;
use crate::protocol::AgentMessageContentDeltaEvent;
use crate::protocol::AgentReasoningSectionBreakEvent;
use crate::protocol::ApplyPatchApprovalRequestEvent;
use crate::protocol::AskForApproval;
use crate::protocol::BackgroundEventEvent;
use crate::protocol::CompactedItem;
use crate::protocol::DeprecationNoticeEvent;
use crate::protocol::ErrorEvent;
use crate::protocol::Event;
use crate::protocol::EventMsg;
use crate::protocol::ExecApprovalRequestEvent;
use crate::protocol::McpServerRefreshConfig;
use crate::protocol::ModelRerouteEvent;
use crate::protocol::ModelRerouteReason;
use crate::protocol::NetworkApprovalContext;
use crate::protocol::Op;
use crate::protocol::PlanDeltaEvent;
use crate::protocol::RateLimitSnapshot;
use crate::protocol::ReasoningContentDeltaEvent;
use crate::protocol::ReasoningRawContentDeltaEvent;
use crate::protocol::RequestUserInputEvent;
use crate::protocol::ReviewDecision;
use crate::protocol::SandboxPolicy;
use crate::protocol::SessionConfiguredEvent;
use crate::protocol::SessionNetworkProxyRuntime;
use crate::protocol::SkillDependencies as ProtocolSkillDependencies;
use crate::protocol::SkillErrorInfo;
use crate::protocol::SkillInterface as ProtocolSkillInterface;
use crate::protocol::SkillMetadata as ProtocolSkillMetadata;
use crate::protocol::SkillToolDependency as ProtocolSkillToolDependency;
use crate::protocol::StreamErrorEvent;
use crate::protocol::Submission;
use crate::protocol::TokenCountEvent;
use crate::protocol::TokenUsage;
use crate::protocol::TokenUsageInfo;
use crate::protocol::TurnDiffEvent;
use crate::protocol::WarningEvent;
use crate::resolve_skill_dependencies_for_turn;
use crate::rollout::RolloutRecorder;
use crate::rollout::RolloutRecorderParams;
use crate::rollout::map_session_init_error;
use crate::rollout::metadata;
use crate::rollout::policy::EventPersistenceMode;
use crate::session_startup_prewarm::SessionStartupPrewarmHandle;
use crate::shell;
use crate::shell_snapshot::ShellSnapshot;
use crate::skills_watcher::SkillsWatcher;
use crate::skills_watcher::SkillsWatcherEvent;
use crate::state::ActiveTurn;
use crate::state::SessionServices;
use crate::state::SessionState;
use crate::state_db;
use crate::tasks::GhostSnapshotTask;
use crate::tasks::ReviewTask;
use crate::tasks::SessionTask;
use crate::tasks::SessionTaskContext;
use crate::tools::ToolRouter;
use crate::tools::context::SharedTurnDiffTracker;
use crate::tools::js_repl::JsReplHandle;
use crate::tools::js_repl::resolve_compatible_node;
use crate::tools::network_approval::NetworkApprovalService;
use crate::tools::network_approval::build_blocked_request_observer;
use crate::tools::network_approval::build_network_policy_decider;
use crate::tools::parallel::ToolCallRuntime;
use crate::tools::router::ToolRouterParams;
use crate::tools::sandboxing::ApprovalStore;
use crate::tools::spec::ToolsConfig;
use crate::tools::spec::ToolsConfigParams;
use crate::turn_diff_tracker::TurnDiffTracker;
use crate::turn_timing::TurnTimingState;
use crate::turn_timing::record_turn_ttfm_metric;
use crate::turn_timing::record_turn_ttft_metric;
use crate::unified_exec::UnifiedExecProcessManager;
use crate::util::backoff;
use crate::windows_sandbox::WindowsSandboxLevelExt;
use codex_async_utils::OrCancelExt;
use codex_git_utils::get_git_repo_root;
use codex_hooks::NeroHookMsgMode;
use codex_otel::SessionTelemetry;
use codex_otel::TelemetryAuthMode;
use codex_otel::metrics::names::THREAD_STARTED_METRIC;
use codex_protocol::config_types::CollaborationMode;
use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary as ReasoningSummaryConfig;
use codex_protocol::config_types::ServiceTier;
use codex_protocol::config_types::WindowsSandboxLevel;
use codex_protocol::models::ContentItem;
use codex_protocol::models::DeveloperInstructions;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ReasoningEffort as ReasoningEffortConfig;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::InitialHistory;
use codex_protocol::protocol::NonSteerableTurnKind;
use codex_protocol::user_input::UserInput;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_readiness::Readiness;
use codex_utils_readiness::ReadinessFlag;

/// The high-level interface to the Codex system.
/// It operates as a queue pair where you send submissions and receive events.
pub struct Codex {
    pub(crate) tx_sub: Sender<Submission>,
    pub(crate) rx_event: Receiver<Event>,
    // Last known status of the agent.
    pub(crate) agent_status: watch::Receiver<AgentStatus>,
    pub(crate) session: Arc<Session>,
    // Shared future for the background submission loop completion so multiple
    // callers can wait for shutdown.
    pub(crate) session_loop_termination: SessionLoopTermination,
}

pub(crate) type SessionLoopTermination = Shared<BoxFuture<'static, ()>>;

/// Wrapper returned by [`Codex::spawn`] containing the spawned [`Codex`],
/// the submission id for the initial `ConfigureSession` request and the
/// unique session id.
pub struct CodexSpawnOk {
    pub codex: Codex,
    pub thread_id: ThreadId,
    #[deprecated(note = "use thread_id")]
    pub conversation_id: ThreadId,
}

pub(crate) struct CodexSpawnArgs {
    pub(crate) config: Config,
    pub(crate) auth_manager: Arc<AuthManager>,
    pub(crate) models_manager: Arc<ModelsManager>,
    pub(crate) environment_manager: Arc<EnvironmentManager>,
    pub(crate) skills_manager: Arc<SkillsManager>,
    pub(crate) plugins_manager: Arc<PluginsManager>,
    pub(crate) mcp_manager: Arc<McpManager>,
    pub(crate) skills_watcher: Arc<SkillsWatcher>,
    pub(crate) conversation_history: InitialHistory,
    pub(crate) session_source: SessionSource,
    pub(crate) agent_control: AgentControl,
    pub(crate) dynamic_tools: Vec<DynamicToolSpec>,
    pub(crate) persist_extended_history: bool,
    pub(crate) metrics_service_name: Option<String>,
    pub(crate) inherited_shell_snapshot: Option<Arc<ShellSnapshot>>,
    pub(crate) inherited_exec_policy: Option<Arc<ExecPolicyManager>>,
    pub(crate) user_shell_override: Option<shell::Shell>,
    pub(crate) parent_trace: Option<W3cTraceContext>,
}

pub(crate) const INITIAL_SUBMIT_ID: &str = "";
pub(crate) const SUBMISSION_CHANNEL_CAPACITY: usize = 512;
const HOOK_AUTO_REPLY_SUBMISSION_PREFIX: &str = "hook-auto-";
// Allow a short autonomous streak before requiring explicit user re-entry.
// This matches the default nero auto policy (`max_rounds = 7`) plus initial turn.
const HOOK_AUTO_REPLY_MAX_CHAIN_DEPTH: u32 = 8;
/// Give TUI "Tab queued" user input a short head start after turn completion.
///
/// This keeps `tab-first` behavior deterministic enough in interactive mode:
/// user-queued follow-ups are preferred, while synthetic auto-replies are
/// still available as fallback when the user has not queued anything.
const HOOK_AUTO_REPLY_TAB_PRIORITY_GRACE_MS: u64 = 300;
const HOOK_AUTO_REPLY_WAIT_FOR_TERMINAL_TIMEOUT_MS: u64 = 30_000;
const HOOK_AUTO_REPLY_WAIT_POLL_INTERVAL_MS: u64 = 10;
const NERO_HOOK_DELIVERY_LOG_FILENAME: &str = "nero-hook-delivery.jsonl";
const CYBER_VERIFY_URL: &str = "https://chatgpt.com/cyber";
const CYBER_SAFETY_URL: &str = "https://developers.openai.com/codex/concepts/cyber-safety";
const DIRECT_APP_TOOL_EXPOSURE_THRESHOLD: usize = 100;

fn parse_hook_auto_reply_epoch(submission_id: &str) -> Option<u64> {
    let suffix = submission_id.strip_prefix(HOOK_AUTO_REPLY_SUBMISSION_PREFIX)?;
    let (epoch, _) = suffix.split_once('-')?;
    epoch.parse::<u64>().ok()
}

fn nero_hook_msg_throttle_key(
    hook_name: &str,
    mode: &NeroHookMsgMode,
    format: &NeroHookMsgFormat,
    show_agent: bool,
    show_tui: bool,
    full: &str,
    short: &str,
    status_kind: Option<&str>,
    status_text: Option<&str>,
) -> String {
    let mut hasher = DefaultHasher::new();
    hook_name.hash(&mut hasher);
    mode.hash(&mut hasher);
    format.hash(&mut hasher);
    show_agent.hash(&mut hasher);
    show_tui.hash(&mut hasher);
    full.hash(&mut hasher);
    short.hash(&mut hasher);
    status_kind.hash(&mut hasher);
    status_text.hash(&mut hasher);
    hasher.finish().to_string()
}

fn nero_hook_msg_remaining_secs_ceil(remaining: StdDuration) -> u64 {
    if remaining.is_zero() {
        return 1;
    }
    let millis = remaining.as_millis();
    let secs = millis.div_ceil(1000);
    let secs = u64::try_from(secs).unwrap_or(u64::MAX);
    secs.max(1)
}

fn nero_hook_prefixed_message(message: String) -> String {
    if message.starts_with("[nero-hook]") {
        message
    } else {
        format!("[nero-hook] {message}")
    }
}

fn nero_hook_tui_warning_message(
    content: &str,
    format: NeroHookMsgFormat,
    status: Option<(&str, &str)>,
) -> String {
    match format {
        NeroHookMsgFormat::Inline => {
            let mut line = content.to_string();
            if let Some((kind, text)) = status {
                line.push_str(" [");
                line.push_str(kind);
                line.push_str(": ");
                line.push_str(text);
                line.push(']');
            }
            nero_hook_prefixed_message(line)
        }
        NeroHookMsgFormat::Block => {
            let mut out = String::from("[nero-hook]\n------------\ncontent = ");
            out.push_str(content);
            if let Some((kind, text)) = status {
                out.push_str("\n------------\nstatus = ");
                out.push_str(kind);
                out.push_str(": ");
                out.push_str(text);
            }
            out
        }
    }
}

fn nero_hook_mode_label(mode: &NeroHookMsgMode) -> &'static str {
    match mode {
        NeroHookMsgMode::Synced => "synced",
        NeroHookMsgMode::TuiShort => "tui-short",
    }
}

fn nero_hook_format_label(format: &NeroHookMsgFormat) -> &'static str {
    match format {
        NeroHookMsgFormat::Block => "block",
        NeroHookMsgFormat::Inline => "inline",
    }
}

fn normalized_nero_hook_status_kind(
    status: Option<&codex_hooks::NeroHookMsgStatus>,
) -> Option<String> {
    status.map(|item| item.kind.trim().to_ascii_lowercase())
}

struct NeroHookTuiDelivery {
    warning: Option<String>,
    hook_summary: Option<codex_protocol::protocol::HookOutputEntry>,
}

fn nero_hook_summary_entry_kind(
    status_kind_normalized: Option<&str>,
) -> codex_protocol::protocol::HookOutputEntryKind {
    match status_kind_normalized {
        Some("warning") => codex_protocol::protocol::HookOutputEntryKind::Warning,
        Some("stop") => codex_protocol::protocol::HookOutputEntryKind::Stop,
        Some("feedback") => codex_protocol::protocol::HookOutputEntryKind::Feedback,
        Some("error") => codex_protocol::protocol::HookOutputEntryKind::Error,
        Some("state") | Some("auto") | None | Some(_) => {
            codex_protocol::protocol::HookOutputEntryKind::Context
        }
    }
}

fn nero_hook_summary_entry_text(
    tui_body: &str,
    status_entry: &codex_hooks::NeroHookMsgStatus,
) -> String {
    let kind = status_entry.kind.trim();
    let text = status_entry.text.trim();

    if !kind.is_empty() && !text.is_empty() {
        format!("{tui_body} [{kind}: {text}]")
    } else if !kind.is_empty() {
        format!("{tui_body} [{kind}]")
    } else if !text.is_empty() {
        format!("{tui_body} [{text}]")
    } else {
        tui_body.to_string()
    }
}

fn nero_hook_tui_delivery(
    tui_body: &str,
    format: NeroHookMsgFormat,
    status: Option<&codex_hooks::NeroHookMsgStatus>,
    status_kind_normalized: Option<&str>,
) -> NeroHookTuiDelivery {
    if let Some(status_entry) = status {
        let hook_summary = codex_protocol::protocol::HookOutputEntry {
            kind: nero_hook_summary_entry_kind(status_kind_normalized),
            text: nero_hook_summary_entry_text(tui_body, status_entry),
        };
        let (warning, hook_summary) = match format {
            // In block mode emit the warning payload and avoid duplicate status
            // rendering through the hook summary lane.
            NeroHookMsgFormat::Block => (
                Some(nero_hook_tui_warning_message(
                    tui_body,
                    format,
                    status.map(|item| (item.kind.as_str(), item.text.as_str())),
                )),
                None,
            ),
            NeroHookMsgFormat::Inline => (None, Some(hook_summary)),
        };
        return NeroHookTuiDelivery {
            warning,
            hook_summary,
        };
    }

    NeroHookTuiDelivery {
        warning: Some(nero_hook_tui_warning_message(
            tui_body,
            format,
            status.map(|item| (item.kind.as_str(), item.text.as_str())),
        )),
        hook_summary: None,
    }
}

fn stop_hook_debug_warning_message(
    mode: NeroStopHookDebugReportingMode,
    hook_prompt_message: &ResponseItem,
) -> Option<String> {
    let ResponseItem::Message { id, content, .. } = hook_prompt_message else {
        return None;
    };
    let fragments = parse_hook_prompt_message(id.as_ref(), content)?.fragments;
    if fragments.is_empty() {
        return None;
    }

    match mode {
        NeroStopHookDebugReportingMode::Off => None,
        NeroStopHookDebugReportingMode::Summary => {
            let count = fragments.len();
            let hook_run_ids = fragments
                .iter()
                .map(|fragment| fragment.hook_run_id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Some(format!(
                "[nero-hook][stop-debug] STOP_HOOK injected {count} hook prompt fragment(s) into agent history (hook_run_ids: {hook_run_ids})"
            ))
        }
        NeroStopHookDebugReportingMode::Full => {
            let mut message = format!(
                "[nero-hook][stop-debug]\n------------\nfragments = {}",
                fragments.len()
            );
            for (index, fragment) in fragments.iter().enumerate() {
                let fragment_index = index + 1;
                message.push_str("\n------------\n");
                message.push_str(&format!(
                    "fragment[{fragment_index}].hook_run_id = {}",
                    fragment.hook_run_id
                ));
                message.push('\n');
                message.push_str(&format!(
                    "fragment[{fragment_index}].text = {}",
                    fragment.text
                ));
            }
            Some(message)
        }
    }
}

fn runtime_delivery_contract_satisfied(
    stop_checkpoint_required: bool,
    stop_checkpoint_delivered: bool,
) -> bool {
    !stop_checkpoint_required || stop_checkpoint_delivered
}

fn stop_delivery_contract_status(
    contract_satisfied: bool,
    auto_user_replies_blocked: usize,
) -> &'static str {
    if contract_satisfied {
        "ok"
    } else if auto_user_replies_blocked > 0 {
        "fail-closed-blocked"
    } else {
        "failed-stop-checkpoint-missing"
    }
}

fn after_agent_runtime_hook_completed_event(
    turn_id: &str,
    hook_name: &str,
    status: codex_protocol::protocol::HookRunStatus,
    status_message: Option<String>,
    meta: Option<Value>,
    entries: Vec<codex_protocol::protocol::HookOutputEntry>,
) -> Option<crate::protocol::HookCompletedEvent> {
    if entries.is_empty() && meta.is_none() {
        return None;
    }

    let started_at = chrono::Utc::now().timestamp();
    Some(crate::protocol::HookCompletedEvent {
        turn_id: Some(turn_id.to_string()),
        run: codex_protocol::protocol::HookRunSummary {
            id: format!("after-agent:{hook_name}:{turn_id}"),
            event_name: codex_protocol::protocol::HookEventName::AfterAgent,
            handler_type: codex_protocol::protocol::HookHandlerType::Agent,
            execution_mode: codex_protocol::protocol::HookExecutionMode::Sync,
            scope: codex_protocol::protocol::HookScope::Turn,
            source_path: PathBuf::from(format!("hook://after_agent/{hook_name}")),
            display_order: 0,
            status,
            status_message,
            started_at,
            completed_at: Some(started_at),
            duration_ms: Some(0),
            meta,
            entries,
        },
    })
}

const NERO_HOOK_STATUS_META_MAX_STRING_CHARS: usize = 512;

fn truncate_audit_meta_string(input: &str) -> String {
    let mut chars = input.chars();
    let truncated: String = chars
        .by_ref()
        .take(NERO_HOOK_STATUS_META_MAX_STRING_CHARS)
        .collect();
    if chars.next().is_some() {
        return format!("{truncated}...[truncated]");
    }
    truncated
}

fn sanitize_auto_decision_meta_for_audit(value: &Value) -> Option<Value> {
    let Value::Object(obj) = value else {
        return None;
    };

    let mut out = serde_json::Map::new();

    for key in [
        "decision",
        "reason_code",
        "score_explanation",
        "assistant_stop_mode",
        "backend_error_code",
        "campaign_id",
        "campaign_status",
        "turn_id",
        "stop_cause",
        "stop_flag",
    ] {
        if let Some(raw) = obj.get(key).and_then(|v| v.as_str()) {
            out.insert(
                key.to_string(),
                Value::String(truncate_audit_meta_string(raw)),
            );
        }
    }

    for key in [
        "score",
        "effective_threshold",
        "margin",
        "auto_rounds_before",
        "auto_rounds_after",
        "max_auto_rounds",
        "autonomy_level",
        "autonomy_step_per_round",
    ] {
        if let Some(raw) = obj.get(key)
            && raw.is_number()
        {
            out.insert(key.to_string(), raw.clone());
        }
    }

    if let Some(flags) = obj.get("flags").and_then(|v| v.as_object()) {
        let mut out_flags = serde_json::Map::new();
        for key in [
            "emergency_flag",
            "gates_done_observed",
            "gates_done_backend",
            "user_collaboration_required",
            "assistant_stop",
            "backend_unavailable",
        ] {
            if let Some(raw) = flags.get(key).and_then(serde_json::Value::as_bool) {
                out_flags.insert(key.to_string(), Value::Bool(raw));
            }
        }
        if !out_flags.is_empty() {
            out.insert("flags".to_string(), Value::Object(out_flags));
        }
    }

    if let Some(policy) = obj
        .get("session_auto_policy_override")
        .and_then(|v| v.as_object())
    {
        let mut out_policy = serde_json::Map::new();
        for key in [
            "autonomy_level",
            "autonomy_step_per_round",
            "max_auto_rounds",
        ] {
            if let Some(raw) = policy.get(key)
                && raw.is_number()
            {
                out_policy.insert(key.to_string(), raw.clone());
            }
        }
        if !out_policy.is_empty() {
            out.insert(
                "session_auto_policy_override".to_string(),
                Value::Object(out_policy),
            );
        }
    }

    if out.is_empty() {
        return None;
    }
    Some(Value::Object(out))
}

fn sanitize_auto_stage_meta_for_audit(value: &Value) -> Option<Value> {
    let Value::Object(obj) = value else {
        return None;
    };

    let mut out = serde_json::Map::new();
    if let Some(raw) = obj.get("stage").and_then(|value| value.as_str()) {
        out.insert(
            "stage".to_string(),
            Value::String(truncate_audit_meta_string(raw)),
        );
    }
    if out.is_empty() {
        return None;
    }
    Some(Value::Object(out))
}

fn sanitize_nero_hook_status_meta_for_audit(meta: Option<Value>) -> Option<Value> {
    let Value::Object(meta_obj) = meta? else {
        return None;
    };
    let mut out = serde_json::Map::new();
    if let Some(auto_stage) = meta_obj.get("auto_stage")
        && let Some(sanitized) = sanitize_auto_stage_meta_for_audit(auto_stage)
    {
        out.insert("auto_stage".to_string(), sanitized);
    }
    if let Some(auto_decision) = meta_obj.get("auto_decision")
        && let Some(sanitized) = sanitize_auto_decision_meta_for_audit(auto_decision)
    {
        out.insert("auto_decision".to_string(), sanitized);
    }
    if out.is_empty() {
        return None;
    }
    Some(Value::Object(out))
}

fn merge_after_agent_runtime_status_meta(accumulated: &mut Option<Value>, incoming: Option<Value>) {
    let Some(Value::Object(incoming_obj)) = incoming else {
        return;
    };

    match accumulated {
        Some(Value::Object(existing_obj)) => {
            existing_obj.extend(incoming_obj);
        }
        _ => {
            *accumulated = Some(Value::Object(incoming_obj));
        }
    }
}

fn after_agent_runtime_hook_summary_meta(
    hook_name: &str,
    status_kind_normalized: Option<String>,
    status_meta: Option<Value>,
    protocol_status: &str,
    stop_checkpoint_required: bool,
    stop_checkpoint_delivered: bool,
    contract_satisfied: bool,
    nero_hook_msg_total: usize,
    nero_hook_msg_throttled: usize,
    follow_up_queued_count: usize,
    follow_up_blocked_count: usize,
    follow_up_expected_wait_seconds: Option<u64>,
    follow_up_generation_epoch: Option<u64>,
) -> Value {
    let follow_up_status = if follow_up_blocked_count > 0 {
        "blocked-delivery-contract"
    } else if follow_up_queued_count > 0 {
        "queued"
    } else {
        "none"
    };
    let mut follow_up = serde_json::Map::new();
    follow_up.insert(
        "status".to_string(),
        Value::String(follow_up_status.to_string()),
    );
    follow_up.insert("queued_count".to_string(), json!(follow_up_queued_count));
    follow_up.insert("blocked_count".to_string(), json!(follow_up_blocked_count));
    if let Some(expected_wait_seconds) = follow_up_expected_wait_seconds {
        follow_up.insert(
            "expected_wait_seconds".to_string(),
            json!(expected_wait_seconds),
        );
    }
    if let Some(generation_epoch) = follow_up_generation_epoch {
        follow_up.insert("generation_epoch".to_string(), json!(generation_epoch));
    }

    json!({
        "domain": "nero_runtime",
        "hook_name": hook_name,
        "status": {
            "kind_normalized": status_kind_normalized,
            "meta": status_meta,
        },
        "protocol": {
            "status": protocol_status,
            "stop_checkpoint_expected": stop_checkpoint_required,
            "stop_checkpoint_delivered": stop_checkpoint_delivered,
            "contract_satisfied": contract_satisfied,
            "nero_hook_msg_total": nero_hook_msg_total,
            "nero_hook_msg_throttled": nero_hook_msg_throttled,
            "auto_user_replies_blocked": follow_up_blocked_count,
        },
        "follow_up": follow_up,
    })
}

fn align_auto_decision_meta_with_delivery_contract(
    status_meta: &mut Option<Value>,
    contract_satisfied: bool,
    auto_user_replies_blocked: usize,
) {
    if contract_satisfied || auto_user_replies_blocked == 0 {
        return;
    }

    let mut status_meta_obj = match status_meta.take() {
        Some(Value::Object(obj)) => obj,
        _ => serde_json::Map::new(),
    };

    let mut auto_decision = status_meta_obj
        .get("auto_decision")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    auto_decision.insert(
        "decision".to_string(),
        Value::String("blocked-delivery-contract".to_string()),
    );
    auto_decision.insert(
        "reason_code".to_string(),
        Value::String("delivery-contract-blocked".to_string()),
    );
    auto_decision.insert(
        "score_explanation".to_string(),
        Value::String("STOP checkpoint was not delivered in this turn.".to_string()),
    );

    status_meta_obj.insert("auto_decision".to_string(), Value::Object(auto_decision));
    *status_meta = Some(Value::Object(status_meta_obj));
}

async fn append_nero_hook_delivery_audit(
    log_path: &Path,
    conversation_id: &ThreadId,
    turn_context: &TurnContext,
    hook_name: &str,
    action_type: &str,
    status: &str,
    delivered_agent: bool,
    delivered_tui: bool,
    details: Value,
) {
    let log_path = log_path.to_path_buf();
    let thread_id = conversation_id.to_string();
    let turn_id = turn_context.sub_id.clone();
    let session_source = turn_context.session_source.to_string();
    let hook_name = hook_name.to_string();
    let action_type = action_type.to_string();
    let status = status.to_string();
    let record = json!({
        "ts": Utc::now().to_rfc3339(),
        "thread_id": thread_id,
        "turn_id": turn_id,
        "session_source": session_source,
        "hook_name": hook_name,
        "action_type": action_type,
        "status": status,
        "delivered": {
            "agent": delivered_agent,
            "tui": delivered_tui,
        },
        "details": details,
    });
    let serialized = match serde_json::to_string(&record) {
        Ok(value) => value,
        Err(err) => {
            debug!(
                error = %err,
                hook_name,
                action_type,
                "failed to serialize nero hook delivery audit entry"
            );
            return;
        }
    };
    if let Some(parent) = log_path.parent()
        && let Err(err) = tokio::fs::create_dir_all(parent).await
    {
        debug!(
            error = %err,
            path = %log_path.display(),
            "failed to create nero hook delivery audit directory"
        );
        return;
    }
    let mut file = match TokioOpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
    {
        Ok(file) => file,
        Err(err) => {
            debug!(
                error = %err,
                path = %log_path.display(),
                "failed to open nero hook delivery audit file"
            );
            return;
        }
    };
    let mut line = serialized;
    line.push('\n');
    if let Err(err) = file.write_all(line.as_bytes()).await {
        debug!(
            error = %err,
            path = %log_path.display(),
            "failed to append nero hook delivery audit entry"
        );
    }
}

async fn append_nero_model_fallback_audit(
    log_path: Option<&Path>,
    conversation_id: &ThreadId,
    turn_context: &TurnContext,
    status: &str,
    details: Value,
) {
    let Some(log_path) = log_path else {
        return;
    };
    append_nero_hook_delivery_audit(
        log_path,
        conversation_id,
        turn_context,
        "model-fallback",
        "model_fallback",
        status,
        /*delivered_agent*/ false,
        /*delivered_tui*/ false,
        details,
    )
    .await;
}

impl Codex {
    /// Spawn a new [`Codex`] and initialize the session.
    pub(crate) async fn spawn(args: CodexSpawnArgs) -> CodexResult<CodexSpawnOk> {
        let parent_trace = match args.parent_trace {
            Some(trace) => {
                if codex_otel::context_from_w3c_trace_context(&trace).is_some() {
                    Some(trace)
                } else {
                    warn!("ignoring invalid thread spawn trace carrier");
                    None
                }
            }
            None => None,
        };
        let thread_spawn_span = info_span!("thread_spawn", otel.name = "thread_spawn");
        if let Some(trace) = parent_trace.as_ref() {
            let _ = set_parent_from_w3c_trace_context(&thread_spawn_span, trace);
        }
        Self::spawn_internal(CodexSpawnArgs {
            parent_trace,
            ..args
        })
        .instrument(thread_spawn_span)
        .await
    }

    async fn spawn_internal(args: CodexSpawnArgs) -> CodexResult<CodexSpawnOk> {
        let CodexSpawnArgs {
            mut config,
            auth_manager,
            models_manager,
            environment_manager,
            skills_manager,
            plugins_manager,
            mcp_manager,
            skills_watcher,
            conversation_history,
            session_source,
            agent_control,
            dynamic_tools,
            persist_extended_history,
            metrics_service_name,
            inherited_shell_snapshot,
            user_shell_override,
            inherited_exec_policy,
            parent_trace: _,
        } = args;
        let (tx_sub, rx_sub) = async_channel::bounded(SUBMISSION_CHANNEL_CAPACITY);
        let (tx_event, rx_event) = async_channel::unbounded();

        let plugin_outcome = plugins_manager.plugins_for_config(&config);
        let effective_skill_roots = plugin_outcome.effective_skill_roots();
        let skills_input = skills_load_input_from_config(&config, effective_skill_roots);
        let loaded_skills = skills_manager
            .skills_for_config(&skills_input)
            .filter_for_session_source(&session_source);

        for err in &loaded_skills.errors {
            error!(
                "failed to load skill {}: {}",
                err.path.display(),
                err.message
            );
        }

        if let SessionSource::SubAgent(SubAgentSource::ThreadSpawn { depth, .. }) = session_source
            && depth >= config.agent_max_depth
        {
            let _ = config.features.disable(Feature::SpawnCsv);
            let _ = config.features.disable(Feature::Collab);
        }

        if config.features.enabled(Feature::JsRepl)
            && let Err(err) = resolve_compatible_node(config.js_repl_node_path.as_deref()).await
        {
            let _ = config.features.disable(Feature::JsRepl);
            let _ = config.features.disable(Feature::JsReplToolsOnly);
            let message = if config.features.enabled(Feature::JsRepl) {
                format!(
                    "`js_repl` remains enabled because enterprise requirements pin it on, but the configured Node runtime is unavailable or incompatible. {err}"
                )
            } else {
                format!(
                    "Disabled `js_repl` for this session because the configured Node runtime is unavailable or incompatible. {err}"
                )
            };
            warn!("{message}");
            config.startup_warnings.push(message);
        }
        if config.features.enabled(Feature::CodeMode)
            && let Err(err) = resolve_compatible_node(config.js_repl_node_path.as_deref()).await
        {
            let message = format!(
                "Disabled `exec` for this session because the configured Node runtime is unavailable or incompatible. {err}"
            );
            warn!("{message}");
            let _ = config.features.disable(Feature::CodeMode);
            config.startup_warnings.push(message);
        }

        let user_instructions = get_user_instructions(&config).await;

        let exec_policy = if crate::guardian::is_guardian_reviewer_source(&session_source) {
            // Guardian review should rely on the built-in shell safety checks,
            // not on caller-provided exec-policy rules that could shape the
            // reviewer or silently auto-approve commands.
            Arc::new(ExecPolicyManager::default())
        } else if let Some(exec_policy) = &inherited_exec_policy {
            Arc::clone(exec_policy)
        } else {
            Arc::new(
                ExecPolicyManager::load(&config.config_layer_stack)
                    .await
                    .map_err(|err| CodexErr::Fatal(format!("failed to load rules: {err}")))?,
            )
        };

        let model_fallback_resolution =
            crate::config::resolve_nero_model_fallback_from_env(&session_source)
                .unwrap_or_else(|err| {
                    warn!(
                        error = %err,
                        "failed to resolve nero model fallback from env; disabling feature"
                    );
                    crate::config::NeroModelFallbackResolution {
                        config: None,
                        warning: Some(
                            "Failed to load [nero.model_fallback] from CODEXN_CONFIG_NERO_* overlays; fallback is disabled for this session.".to_string(),
                        ),
                    }
                });
        if let Some(message) = model_fallback_resolution.warning.as_ref() {
            warn!("{message}");
            config.startup_warnings.push(message.clone());
        }

        let config = Arc::new(config);
        let refresh_strategy = match session_source {
            SessionSource::SubAgent(_) => crate::models_manager::manager::RefreshStrategy::Offline,
            _ => crate::models_manager::manager::RefreshStrategy::OnlineIfUncached,
        };
        if config.model.is_none()
            || !matches!(
                refresh_strategy,
                crate::models_manager::manager::RefreshStrategy::Offline
            )
        {
            let _ = models_manager.list_models(refresh_strategy).await;
        }
        let model = models_manager
            .get_default_model(&config.model, refresh_strategy)
            .await;

        // Resolve base instructions for the session. Priority order:
        // 1. config.base_instructions override
        // 2. conversation history => session_meta.base_instructions
        // 3. base_instructions for current model
        let model_info = models_manager.get_model_info(model.as_str(), &config).await;
        let base_instructions = config
            .base_instructions
            .clone()
            .or_else(|| conversation_history.get_base_instructions().map(|s| s.text))
            .unwrap_or_else(|| model_info.get_model_instructions(config.personality));

        // Respect thread-start tools. When missing (resumed/forked threads), read from the db
        // first, then fall back to rollout-file tools.
        let persisted_tools = if dynamic_tools.is_empty() {
            let thread_id = match &conversation_history {
                InitialHistory::Resumed(resumed) => Some(resumed.conversation_id),
                InitialHistory::Forked(_) => conversation_history.forked_from_id(),
                InitialHistory::New => None,
            };
            match thread_id {
                Some(thread_id) => {
                    let state_db_ctx = state_db::get_state_db(&config).await;
                    state_db::get_dynamic_tools(state_db_ctx.as_deref(), thread_id, "codex_spawn")
                        .await
                }
                None => None,
            }
        } else {
            None
        };
        let dynamic_tools = if dynamic_tools.is_empty() {
            persisted_tools
                .or_else(|| conversation_history.get_dynamic_tools())
                .unwrap_or_default()
        } else {
            dynamic_tools
        };

        // TODO (aibrahim): Consolidate config.model and config.model_reasoning_effort into config.collaboration_mode
        // to avoid extracting these fields separately and constructing CollaborationMode here.
        let collaboration_mode = CollaborationMode {
            mode: ModeKind::Default,
            settings: Settings {
                model: model.clone(),
                reasoning_effort: config.model_reasoning_effort,
                developer_instructions: None,
            },
        };
        let nero_auto_runtime =
            crate::config::resolve_codexn_fork_nero_auto_runtime_from_env(&session_source)
                .unwrap_or_else(|err| {
                    warn!(
                        error = %err,
                        "failed to resolve codexn fork nero auto runtime from env; using defaults"
                    );
                    effective_nero_auto_runtime(NeroAutoRuntimeConfig::default(), &session_source)
                });
        let session_configuration = SessionConfiguration {
            provider: config.model_provider.clone(),
            collaboration_mode,
            model_reasoning_summary: config.model_reasoning_summary,
            service_tier: config.service_tier,
            user_instructions,
            personality: config.personality,
            base_instructions,
            compact_prompt: config.compact_prompt.clone(),
            approval_policy: config.permissions.approval_policy.clone(),
            approvals_reviewer: config.approvals_reviewer,
            sandbox_policy: config.permissions.sandbox_policy.clone(),
            file_system_sandbox_policy: config.permissions.file_system_sandbox_policy.clone(),
            network_sandbox_policy: config.permissions.network_sandbox_policy,
            windows_sandbox_level: WindowsSandboxLevel::from_config(&config),
            cwd: config.cwd.clone(),
            codex_home: config.codex_home.clone(),
            thread_name: None,
            original_config_do_not_use: Arc::clone(&config),
            metrics_service_name,
            app_server_client_name: None,
            session_source,
            nero_auto_runtime,
            nero_model_fallback: model_fallback_resolution.config,
            dynamic_tools,
            persist_extended_history,
            inherited_shell_snapshot,
            user_shell_override,
        };

        // Generate a unique ID for the lifetime of this Codex session.
        let session_source_clone = session_configuration.session_source.clone();
        let (agent_status_tx, agent_status_rx) = watch::channel(AgentStatus::PendingInit);

        let session = Session::new(
            session_configuration,
            config.clone(),
            auth_manager.clone(),
            models_manager.clone(),
            exec_policy,
            tx_sub.clone(),
            tx_event.clone(),
            agent_status_tx.clone(),
            conversation_history,
            session_source_clone,
            environment_manager,
            skills_manager,
            plugins_manager,
            mcp_manager.clone(),
            skills_watcher,
            agent_control,
        )
        .await
        .map_err(|e| {
            error!("Failed to create session: {e:#}");
            map_session_init_error(&e, &config.codex_home)
        })?;
        let thread_id = session.conversation_id;

        // This task will run until Op::Shutdown is received.
        let session_for_loop = Arc::clone(&session);
        let session_loop_handle = tokio::spawn(async move {
            submission_loop(session_for_loop, config, rx_sub)
                .instrument(info_span!("session_loop", thread_id = %thread_id))
                .await;
        });
        let codex = Codex {
            tx_sub,
            rx_event,
            agent_status: agent_status_rx,
            session,
            session_loop_termination: session_loop_termination_from_handle(session_loop_handle),
        };

        #[allow(deprecated)]
        Ok(CodexSpawnOk {
            codex,
            thread_id,
            conversation_id: thread_id,
        })
    }

    /// Submit the `op` wrapped in a `Submission` with a unique ID.
    pub async fn submit(&self, op: Op) -> CodexResult<String> {
        self.submit_with_trace(op, /*trace*/ None).await
    }

    pub async fn submit_with_trace(
        &self,
        op: Op,
        trace: Option<W3cTraceContext>,
    ) -> CodexResult<String> {
        let id = Uuid::now_v7().to_string();
        let sub = Submission {
            id: id.clone(),
            op,
            trace,
        };
        self.submit_with_id(sub).await?;
        Ok(id)
    }

    /// Use sparingly: prefer `submit()` so Codex is responsible for generating
    /// unique IDs for each submission.
    pub async fn submit_with_id(&self, mut sub: Submission) -> CodexResult<()> {
        if sub.trace.is_none() {
            sub.trace = current_span_w3c_trace_context();
        }
        self.tx_sub
            .send(sub)
            .await
            .map_err(|_| CodexErr::InternalAgentDied)?;
        Ok(())
    }

    pub async fn shutdown_and_wait(&self) -> CodexResult<()> {
        let session_loop_termination = self.session_loop_termination.clone();
        match self.submit(Op::Shutdown).await {
            Ok(_) => {}
            Err(CodexErr::InternalAgentDied) => {}
            Err(err) => return Err(err),
        }
        session_loop_termination.await;
        Ok(())
    }

    pub async fn next_event(&self) -> CodexResult<Event> {
        let event = self
            .rx_event
            .recv()
            .await
            .map_err(|_| CodexErr::InternalAgentDied)?;
        Ok(event)
    }

    pub async fn steer_input(
        &self,
        input: Vec<UserInput>,
        expected_turn_id: Option<&str>,
    ) -> Result<String, SteerInputError> {
        self.session.steer_input(input, expected_turn_id).await
    }

    pub(crate) async fn set_app_server_client_name(
        &self,
        app_server_client_name: Option<String>,
    ) -> ConstraintResult<()> {
        self.session
            .update_settings(SessionSettingsUpdate {
                app_server_client_name,
                ..Default::default()
            })
            .await
    }

    pub(crate) async fn set_nero_auto_runtime(
        &self,
        nero_auto_runtime: NeroAutoRuntimeConfig,
    ) -> ConstraintResult<ThreadConfigSnapshot> {
        self.session
            .update_settings(SessionSettingsUpdate {
                nero_auto_runtime: Some(nero_auto_runtime),
                ..Default::default()
            })
            .await?;
        Ok(self.thread_config_snapshot().await)
    }

    pub(crate) async fn note_user_input_activity(&self) -> u64 {
        self.session.note_user_input_activity().await
    }

    pub(crate) async fn agent_status(&self) -> AgentStatus {
        self.agent_status.borrow().clone()
    }

    pub(crate) async fn thread_config_snapshot(&self) -> ThreadConfigSnapshot {
        let state = self.session.state.lock().await;
        state.session_configuration.thread_config_snapshot()
    }

    pub(crate) fn state_db(&self) -> Option<state_db::StateDbHandle> {
        self.session.state_db()
    }

    pub(crate) fn enabled(&self, feature: Feature) -> bool {
        self.session.enabled(feature)
    }
}

#[cfg(test)]
pub(crate) fn completed_session_loop_termination() -> SessionLoopTermination {
    futures::future::ready(()).boxed().shared()
}

pub(crate) fn session_loop_termination_from_handle(
    handle: JoinHandle<()>,
) -> SessionLoopTermination {
    async move {
        let _ = handle.await;
    }
    .boxed()
    .shared()
}

/// Context for an initialized model agent
///
/// A session has at most 1 running task at a time, and can be interrupted by user input.
pub(crate) struct Session {
    pub(crate) conversation_id: ThreadId,
    tx_sub: Sender<Submission>,
    tx_event: Sender<Event>,
    agent_status: watch::Sender<AgentStatus>,
    out_of_band_elicitation_paused: watch::Sender<bool>,
    state: Mutex<SessionState>,
    /// The set of enabled features should be invariant for the lifetime of the
    /// session.
    features: ManagedFeatures,
    pending_mcp_server_refresh_config: Mutex<Option<McpServerRefreshConfig>>,
    pub(crate) conversation: Arc<RealtimeConversationManager>,
    pub(crate) active_turn: Mutex<Option<ActiveTurn>>,
    mailbox: Mailbox,
    mailbox_rx: Mutex<MailboxReceiver>,
    idle_pending_input: Mutex<Vec<ResponseInputItem>>, // TODO (jif) merge with mailbox!
    pub(crate) guardian_review_session: GuardianReviewSessionManager,
    pub(crate) services: SessionServices,
    js_repl: Arc<JsReplHandle>,
    hook_seen_terminal_turn_ids: Mutex<HashSet<String>>,
    hook_auto_reply_internal_submission_ids: StdMutex<HashSet<String>>,
    hook_nero_msg_throttle: StdMutex<HashMap<String, StdInstant>>,
    nero_auto_bridge_warning_emitted: StdMutex<bool>,
    hook_auto_reply_guard_state: StdMutex<HookAutoReplyGuardState>,
    next_internal_sub_id: AtomicU64,
}

#[derive(Clone, Debug)]
pub(crate) struct TurnSkillsContext {
    pub(crate) outcome: Arc<SkillLoadOutcome>,
    pub(crate) implicit_invocation_seen_skills: Arc<Mutex<HashSet<String>>>,
}

impl TurnSkillsContext {
    pub(crate) fn new(outcome: Arc<SkillLoadOutcome>) -> Self {
        Self {
            outcome,
            implicit_invocation_seen_skills: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

#[derive(Debug, Default)]
struct HookAutoReplyGuardState {
    epoch: u64,
    chain_depth: u32,
}

/// The context needed for a single turn of the thread.
#[derive(Debug)]
pub(crate) struct TurnContext {
    pub(crate) sub_id: String,
    pub(crate) trace_id: Option<String>,
    pub(crate) realtime_active: bool,
    pub(crate) config: Arc<Config>,
    pub(crate) auth_manager: Option<Arc<AuthManager>>,
    pub(crate) model_info: ModelInfo,
    pub(crate) session_telemetry: SessionTelemetry,
    pub(crate) provider: ModelProviderInfo,
    pub(crate) reasoning_effort: Option<ReasoningEffortConfig>,
    pub(crate) reasoning_summary: ReasoningSummaryConfig,
    pub(crate) session_source: SessionSource,
    pub(crate) environment: Arc<Environment>,
    /// The session's absolute working directory. All relative paths provided
    /// by the model as well as sandbox policies are resolved against this path
    /// instead of `std::env::current_dir()`.
    pub(crate) cwd: AbsolutePathBuf,
    pub(crate) current_date: Option<String>,
    pub(crate) timezone: Option<String>,
    pub(crate) app_server_client_name: Option<String>,
    pub(crate) developer_instructions: Option<String>,
    pub(crate) compact_prompt: Option<String>,
    pub(crate) user_instructions: Option<String>,
    pub(crate) collaboration_mode: CollaborationMode,
    pub(crate) model_fallback: Option<NeroModelFallbackConfig>,
    pub(crate) personality: Option<Personality>,
    pub(crate) approval_policy: Constrained<AskForApproval>,
    pub(crate) sandbox_policy: Constrained<SandboxPolicy>,
    pub(crate) file_system_sandbox_policy: FileSystemSandboxPolicy,
    pub(crate) network_sandbox_policy: NetworkSandboxPolicy,
    pub(crate) network: Option<NetworkProxy>,
    pub(crate) windows_sandbox_level: WindowsSandboxLevel,
    pub(crate) shell_environment_policy: ShellEnvironmentPolicy,
    pub(crate) tools_config: ToolsConfig,
    pub(crate) features: ManagedFeatures,
    pub(crate) ghost_snapshot: GhostSnapshotConfig,
    pub(crate) final_output_json_schema: Option<Value>,
    pub(crate) codex_self_exe: Option<PathBuf>,
    pub(crate) codex_linux_sandbox_exe: Option<PathBuf>,
    pub(crate) tool_call_gate: Arc<ReadinessFlag>,
    pub(crate) truncation_policy: TruncationPolicy,
    pub(crate) js_repl: Arc<JsReplHandle>,
    pub(crate) dynamic_tools: Vec<DynamicToolSpec>,
    pub(crate) turn_metadata_state: Arc<TurnMetadataState>,
    pub(crate) turn_skills: TurnSkillsContext,
    pub(crate) turn_timing_state: Arc<TurnTimingState>,
}
impl TurnContext {
    pub(crate) fn model_context_window(&self) -> Option<i64> {
        let effective_context_window_percent = self.model_info.effective_context_window_percent;
        self.model_info.context_window.map(|context_window| {
            context_window.saturating_mul(effective_context_window_percent) / 100
        })
    }

    pub(crate) fn apps_enabled(&self) -> bool {
        self.features
            .apps_enabled_cached(self.auth_manager.as_deref())
    }

    pub(crate) async fn with_model(&self, model: String, models_manager: &ModelsManager) -> Self {
        self.with_model_and_reasoning(
            model,
            /*requested_reasoning_effort*/ None,
            models_manager,
        )
        .await
    }

    pub(crate) async fn with_model_and_reasoning(
        &self,
        model: String,
        requested_reasoning_effort: Option<ReasoningEffortConfig>,
        models_manager: &ModelsManager,
    ) -> Self {
        let mut config = (*self.config).clone();
        config.model = Some(model.clone());
        let model_info = models_manager.get_model_info(model.as_str(), &config).await;
        let truncation_policy = model_info.truncation_policy.into();
        let supported_reasoning_levels = model_info
            .supported_reasoning_levels
            .iter()
            .map(|preset| preset.effort)
            .collect::<Vec<_>>();
        let preferred_reasoning_effort = requested_reasoning_effort.or(self.reasoning_effort);
        let reasoning_effort = if let Some(preferred_reasoning_effort) = preferred_reasoning_effort
        {
            if supported_reasoning_levels.contains(&preferred_reasoning_effort) {
                Some(preferred_reasoning_effort)
            } else {
                supported_reasoning_levels
                    .get(supported_reasoning_levels.len().saturating_sub(1) / 2)
                    .copied()
                    .or(model_info.default_reasoning_level)
            }
        } else {
            supported_reasoning_levels
                .get(supported_reasoning_levels.len().saturating_sub(1) / 2)
                .copied()
                .or(model_info.default_reasoning_level)
        };
        config.model_reasoning_effort = reasoning_effort;

        let collaboration_mode = self.collaboration_mode.with_updates(
            Some(model.clone()),
            Some(reasoning_effort),
            /*developer_instructions*/ None,
        );
        let features = self.features.clone();
        let tools_config = ToolsConfig::new(&ToolsConfigParams {
            model_info: &model_info,
            available_models: &models_manager
                .list_models(RefreshStrategy::OnlineIfUncached)
                .await,
            features: &features,
            web_search_mode: self.tools_config.web_search_mode,
            session_source: self.session_source.clone(),
            sandbox_policy: self.sandbox_policy.get(),
            windows_sandbox_level: self.windows_sandbox_level,
        })
        .with_unified_exec_shell_mode(self.tools_config.unified_exec_shell_mode.clone())
        .with_web_search_config(self.tools_config.web_search_config.clone())
        .with_allow_login_shell(self.tools_config.allow_login_shell)
        .with_agent_roles(config.agent_roles.clone())
        .with_spawn_delegation_report_required(
            config.spawn_delegation_report_profile.required_in_spawn(),
        );

        Self {
            sub_id: self.sub_id.clone(),
            trace_id: self.trace_id.clone(),
            realtime_active: self.realtime_active,
            config: Arc::new(config),
            auth_manager: self.auth_manager.clone(),
            model_info: model_info.clone(),
            session_telemetry: self
                .session_telemetry
                .clone()
                .with_model(model.as_str(), model_info.slug.as_str()),
            provider: self.provider.clone(),
            reasoning_effort,
            reasoning_summary: self.reasoning_summary,
            session_source: self.session_source.clone(),
            environment: Arc::clone(&self.environment),
            cwd: self.cwd.clone(),
            current_date: self.current_date.clone(),
            timezone: self.timezone.clone(),
            app_server_client_name: self.app_server_client_name.clone(),
            developer_instructions: self.developer_instructions.clone(),
            compact_prompt: self.compact_prompt.clone(),
            user_instructions: self.user_instructions.clone(),
            collaboration_mode,
            model_fallback: self.model_fallback.clone(),
            personality: self.personality,
            approval_policy: self.approval_policy.clone(),
            sandbox_policy: self.sandbox_policy.clone(),
            file_system_sandbox_policy: self.file_system_sandbox_policy.clone(),
            network_sandbox_policy: self.network_sandbox_policy,
            network: self.network.clone(),
            windows_sandbox_level: self.windows_sandbox_level,
            shell_environment_policy: self.shell_environment_policy.clone(),
            tools_config,
            features,
            ghost_snapshot: self.ghost_snapshot.clone(),
            final_output_json_schema: self.final_output_json_schema.clone(),
            codex_self_exe: self.codex_self_exe.clone(),
            codex_linux_sandbox_exe: self.codex_linux_sandbox_exe.clone(),
            tool_call_gate: Arc::new(ReadinessFlag::new()),
            truncation_policy,
            js_repl: Arc::clone(&self.js_repl),
            dynamic_tools: self.dynamic_tools.clone(),
            turn_metadata_state: self.turn_metadata_state.clone(),
            turn_skills: self.turn_skills.clone(),
            turn_timing_state: Arc::clone(&self.turn_timing_state),
        }
    }

    pub(crate) fn resolve_path(&self, path: Option<String>) -> PathBuf {
        path.as_ref()
            .map(PathBuf::from)
            .map_or_else(|| self.cwd.to_path_buf(), |p| self.cwd.as_path().join(p))
    }

    pub(crate) fn compact_prompt(&self) -> &str {
        self.compact_prompt
            .as_deref()
            .unwrap_or(compact::SUMMARIZATION_PROMPT)
    }

    pub(crate) fn to_turn_context_item(&self) -> TurnContextItem {
        TurnContextItem {
            turn_id: Some(self.sub_id.clone()),
            trace_id: self.trace_id.clone(),
            cwd: self.cwd.to_path_buf(),
            current_date: self.current_date.clone(),
            timezone: self.timezone.clone(),
            approval_policy: self.approval_policy.value(),
            sandbox_policy: self.sandbox_policy.get().clone(),
            network: self.turn_context_network_item(),
            model: self.model_info.slug.clone(),
            personality: self.personality,
            collaboration_mode: Some(self.collaboration_mode.clone()),
            realtime_active: Some(self.realtime_active),
            effort: self.reasoning_effort,
            summary: self.reasoning_summary,
            user_instructions: self.user_instructions.clone(),
            developer_instructions: self.developer_instructions.clone(),
            final_output_json_schema: self.final_output_json_schema.clone(),
            truncation_policy: Some(self.truncation_policy),
        }
    }

    fn turn_context_network_item(&self) -> Option<TurnContextNetworkItem> {
        let network = self
            .config
            .config_layer_stack
            .requirements()
            .network
            .as_ref()?;
        Some(TurnContextNetworkItem {
            allowed_domains: network
                .domains
                .as_ref()
                .and_then(codex_config::NetworkDomainPermissionsToml::allowed_domains)
                .unwrap_or_default(),
            denied_domains: network
                .domains
                .as_ref()
                .and_then(codex_config::NetworkDomainPermissionsToml::denied_domains)
                .unwrap_or_default(),
        })
    }
}

fn local_time_context() -> (String, String) {
    match iana_time_zone::get_timezone() {
        Ok(timezone) => (Local::now().format("%Y-%m-%d").to_string(), timezone),
        Err(_) => (
            Utc::now().format("%Y-%m-%d").to_string(),
            "Etc/UTC".to_string(),
        ),
    }
}

fn clamp_nero_auto_runtime(runtime: NeroAutoRuntimeConfig) -> NeroAutoRuntimeConfig {
    NeroAutoRuntimeConfig {
        enabled: runtime.enabled,
        autonomy_level: runtime.autonomy_level.clamp(1, 10),
        max_auto_rounds: runtime.max_auto_rounds.max(0),
    }
}

fn session_source_blocks_nero_msg_auto_lane(session_source: &SessionSource) -> bool {
    matches!(session_source, SessionSource::SubAgent(_))
}

fn effective_nero_auto_runtime(
    runtime: NeroAutoRuntimeConfig,
    session_source: &SessionSource,
) -> NeroAutoRuntimeConfig {
    let mut runtime = clamp_nero_auto_runtime(runtime);
    if session_source_blocks_nero_msg_auto_lane(session_source) {
        runtime.enabled = false;
    }
    runtime
}

fn model_fallback_rotated_indices(
    ladder_len: usize,
    start_model: Option<&str>,
    ladder: &[NeroModelFallbackStep],
) -> Vec<usize> {
    if ladder_len == 0 {
        return Vec::new();
    }
    let Some(start_model) = start_model else {
        return (0..ladder_len).collect();
    };
    let Some(start_index) = ladder.iter().position(|step| step.model == start_model) else {
        return (0..ladder_len).collect();
    };
    let mut indices = Vec::with_capacity(ladder_len);
    for offset in 0..ladder_len {
        indices.push((start_index + offset) % ladder_len);
    }
    indices
}

fn next_available_model_fallback_step(
    ladder: &[NeroModelFallbackStep],
    start_after_model: Option<&str>,
    now: StdInstant,
    cooldown_remaining: impl Fn(&str, StdInstant) -> Option<StdDuration>,
) -> Option<NeroModelFallbackStep> {
    let indices = model_fallback_rotated_indices(ladder.len(), start_after_model, ladder);
    let skip_current_model = start_after_model.map(str::to_string);
    for index in indices {
        let step = ladder.get(index)?;
        if skip_current_model.as_deref() == Some(step.model.as_str()) {
            continue;
        }
        if cooldown_remaining(step.model.as_str(), now).is_none() {
            return Some(step.clone());
        }
    }
    None
}

fn is_controlled_model_fallback_service_unavailable(
    err: &crate::error::UnexpectedResponseError,
) -> bool {
    if err.status.as_u16() != 503 {
        return false;
    }
    let body = err.body.to_ascii_lowercase();
    [
        "server_is_overloaded",
        "slow_down",
        "model currently not available",
        "currently not available",
        "high demand",
        "temporarily unavailable",
    ]
    .iter()
    .any(|needle| body.contains(needle))
}

fn should_trigger_model_fallback(err: &CodexErr) -> bool {
    match err {
        CodexErr::ServerOverloaded => true,
        CodexErr::UnexpectedStatus(err) => is_controlled_model_fallback_service_unavailable(err),
        _ => false,
    }
}

fn model_fallback_reasoning_label(effort: Option<ReasoningEffortConfig>) -> Option<String> {
    effort.map(|value| format!("{value:?}").to_ascii_lowercase())
}

enum ModelFallbackAfterErrorOutcome {
    Noop,
    Switched(Arc<TurnContext>, NeroModelFallbackStep),
    Exhausted,
    DisabledOverflow,
}

#[derive(Clone)]
pub(crate) struct SessionConfiguration {
    /// Provider identifier ("openai", "openrouter", ...).
    provider: ModelProviderInfo,

    collaboration_mode: CollaborationMode,
    model_reasoning_summary: Option<ReasoningSummaryConfig>,
    service_tier: Option<ServiceTier>,

    /// Model instructions that are appended to the base instructions.
    user_instructions: Option<String>,

    /// Personality preference for the model.
    personality: Option<Personality>,

    /// Base instructions for the session.
    base_instructions: String,

    /// Compact prompt override.
    compact_prompt: Option<String>,

    /// When to escalate for approval for execution
    approval_policy: Constrained<AskForApproval>,
    approvals_reviewer: ApprovalsReviewer,
    /// How to sandbox commands executed in the system
    sandbox_policy: Constrained<SandboxPolicy>,
    file_system_sandbox_policy: FileSystemSandboxPolicy,
    network_sandbox_policy: NetworkSandboxPolicy,
    windows_sandbox_level: WindowsSandboxLevel,

    /// Absolute working directory that should be treated as the *root* of the
    /// session. All relative paths supplied by the model as well as the
    /// execution sandbox are resolved against this directory **instead** of
    /// the process-wide current working directory.
    cwd: AbsolutePathBuf,
    /// Directory containing all Codex state for this session.
    codex_home: PathBuf,
    /// Optional user-facing name for the thread, updated during the session.
    thread_name: Option<String>,

    // TODO(pakrym): Remove config from here
    original_config_do_not_use: Arc<Config>,
    /// Optional service name tag for session metrics.
    metrics_service_name: Option<String>,
    app_server_client_name: Option<String>,
    /// Source of the session (cli, vscode, exec, mcp, ...)
    session_source: SessionSource,
    nero_auto_runtime: NeroAutoRuntimeConfig,
    nero_model_fallback: Option<NeroModelFallbackConfig>,
    dynamic_tools: Vec<DynamicToolSpec>,
    persist_extended_history: bool,
    inherited_shell_snapshot: Option<Arc<ShellSnapshot>>,
    user_shell_override: Option<shell::Shell>,
}

impl SessionConfiguration {
    pub(crate) fn codex_home(&self) -> &PathBuf {
        &self.codex_home
    }

    fn thread_config_snapshot(&self) -> ThreadConfigSnapshot {
        ThreadConfigSnapshot {
            model: self.collaboration_mode.model().to_string(),
            model_provider_id: self.original_config_do_not_use.model_provider_id.clone(),
            service_tier: self.service_tier,
            approval_policy: self.approval_policy.value(),
            approvals_reviewer: self.approvals_reviewer,
            sandbox_policy: self.sandbox_policy.get().clone(),
            cwd: self.cwd.to_path_buf(),
            ephemeral: self.original_config_do_not_use.ephemeral,
            reasoning_effort: self.collaboration_mode.reasoning_effort(),
            personality: self.personality,
            session_source: self.session_source.clone(),
            nero_auto_runtime: self.nero_auto_runtime,
        }
    }

    pub(crate) fn apply(&self, updates: &SessionSettingsUpdate) -> ConstraintResult<Self> {
        let mut next_configuration = self.clone();
        let file_system_policy_matches_legacy = self.file_system_sandbox_policy
            == FileSystemSandboxPolicy::from_legacy_sandbox_policy(
                self.sandbox_policy.get(),
                &self.cwd,
            );
        if let Some(collaboration_mode) = updates.collaboration_mode.clone() {
            next_configuration.collaboration_mode = collaboration_mode;
        }
        if let Some(summary) = updates.reasoning_summary {
            next_configuration.model_reasoning_summary = Some(summary);
        }
        if let Some(service_tier) = updates.service_tier {
            next_configuration.service_tier = service_tier;
        }
        if let Some(personality) = updates.personality {
            next_configuration.personality = Some(personality);
        }
        if let Some(approval_policy) = updates.approval_policy {
            next_configuration.approval_policy.set(approval_policy)?;
        }
        if let Some(approvals_reviewer) = updates.approvals_reviewer {
            next_configuration.approvals_reviewer = approvals_reviewer;
        }
        let mut sandbox_policy_changed = false;
        if let Some(sandbox_policy) = updates.sandbox_policy.clone() {
            next_configuration.sandbox_policy.set(sandbox_policy)?;
            next_configuration.network_sandbox_policy =
                NetworkSandboxPolicy::from(next_configuration.sandbox_policy.get());
            sandbox_policy_changed = true;
        }
        if let Some(windows_sandbox_level) = updates.windows_sandbox_level {
            next_configuration.windows_sandbox_level = windows_sandbox_level;
        }

        let absolute_cwd = updates
            .cwd
            .as_ref()
            .map(|cwd| {
                AbsolutePathBuf::relative_to_current_dir(normalize_for_native_workdir(
                    cwd.as_path(),
                ))
                .unwrap_or_else(|e| {
                    warn!("failed to normalize update cwd: {cwd:?}: {e}");
                    self.cwd.clone()
                })
            })
            .unwrap_or_else(|| self.cwd.clone());

        let cwd_changed = absolute_cwd.as_path() != self.cwd.as_path();
        next_configuration.cwd = absolute_cwd;
        if sandbox_policy_changed || (cwd_changed && file_system_policy_matches_legacy) {
            // Preserve richer split policies across cwd-only updates; only
            // rederive when the session is already using the legacy bridge.
            next_configuration.file_system_sandbox_policy =
                FileSystemSandboxPolicy::from_legacy_sandbox_policy(
                    next_configuration.sandbox_policy.get(),
                    &next_configuration.cwd,
                );
        }
        if let Some(app_server_client_name) = updates.app_server_client_name.clone() {
            next_configuration.app_server_client_name = Some(app_server_client_name);
        }
        if let Some(nero_auto_runtime) = updates.nero_auto_runtime {
            next_configuration.nero_auto_runtime =
                effective_nero_auto_runtime(nero_auto_runtime, &next_configuration.session_source);
        }
        Ok(next_configuration)
    }
}

#[derive(Default, Clone)]
pub(crate) struct SessionSettingsUpdate {
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) approval_policy: Option<AskForApproval>,
    pub(crate) approvals_reviewer: Option<ApprovalsReviewer>,
    pub(crate) sandbox_policy: Option<SandboxPolicy>,
    pub(crate) windows_sandbox_level: Option<WindowsSandboxLevel>,
    pub(crate) collaboration_mode: Option<CollaborationMode>,
    pub(crate) reasoning_summary: Option<ReasoningSummaryConfig>,
    pub(crate) service_tier: Option<Option<ServiceTier>>,
    pub(crate) final_output_json_schema: Option<Option<Value>>,
    pub(crate) personality: Option<Personality>,
    pub(crate) app_server_client_name: Option<String>,
    pub(crate) nero_auto_runtime: Option<NeroAutoRuntimeConfig>,
}

impl Session {
    /// Builds the `x-codex-beta-features` header value for this session.
    ///
    /// `ModelClient` is session-scoped and intentionally does not depend on the full `Config`, so
    /// we precompute the comma-separated list of enabled experimental feature keys at session
    /// creation time and thread it into the client.
    fn build_model_client_beta_features_header(config: &Config) -> Option<String> {
        let beta_features_header = FEATURES
            .iter()
            .filter_map(|spec| {
                if spec.stage.experimental_menu_description().is_some()
                    && config.features.enabled(spec.id)
                {
                    Some(spec.key)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(",");

        if beta_features_header.is_empty() {
            None
        } else {
            Some(beta_features_header)
        }
    }

    async fn start_managed_network_proxy(
        spec: &crate::config::NetworkProxySpec,
        exec_policy: &codex_execpolicy::Policy,
        sandbox_policy: &SandboxPolicy,
        network_policy_decider: Option<Arc<dyn codex_network_proxy::NetworkPolicyDecider>>,
        blocked_request_observer: Option<Arc<dyn codex_network_proxy::BlockedRequestObserver>>,
        managed_network_requirements_enabled: bool,
        audit_metadata: NetworkProxyAuditMetadata,
    ) -> anyhow::Result<(StartedNetworkProxy, SessionNetworkProxyRuntime)> {
        let spec = spec
            .with_exec_policy_network_rules(exec_policy)
            .map_err(|err| {
                tracing::warn!(
                    "failed to apply execpolicy network rules to managed proxy; continuing with configured network policy: {err}"
                );
                err
            })
            .unwrap_or_else(|_| spec.clone());
        let network_proxy = spec
            .start_proxy(
                sandbox_policy,
                network_policy_decider,
                blocked_request_observer,
                managed_network_requirements_enabled,
                audit_metadata,
            )
            .await
            .map_err(|err| anyhow::anyhow!("failed to start managed network proxy: {err}"))?;
        let session_network_proxy = {
            let proxy = network_proxy.proxy();
            SessionNetworkProxyRuntime {
                http_addr: proxy.http_addr().to_string(),
                socks_addr: proxy.socks_addr().to_string(),
            }
        };
        Ok((network_proxy, session_network_proxy))
    }

    /// Don't expand the number of mutated arguments on config. We are in the process of getting rid of it.
    pub(crate) fn build_per_turn_config(session_configuration: &SessionConfiguration) -> Config {
        // todo(aibrahim): store this state somewhere else so we don't need to mut config
        let config = session_configuration.original_config_do_not_use.clone();
        let mut per_turn_config = (*config).clone();
        per_turn_config.cwd = session_configuration.cwd.clone();
        per_turn_config.model_reasoning_effort =
            session_configuration.collaboration_mode.reasoning_effort();
        per_turn_config.model_reasoning_summary = session_configuration.model_reasoning_summary;
        per_turn_config.service_tier = session_configuration.service_tier;
        per_turn_config.personality = session_configuration.personality;
        per_turn_config.approvals_reviewer = session_configuration.approvals_reviewer;
        let resolved_web_search_mode = resolve_web_search_mode_for_turn(
            &per_turn_config.web_search_mode,
            session_configuration.sandbox_policy.get(),
        );
        if let Err(err) = per_turn_config
            .web_search_mode
            .set(resolved_web_search_mode)
        {
            let fallback_value = per_turn_config.web_search_mode.value();
            tracing::warn!(
                error = %err,
                ?resolved_web_search_mode,
                ?fallback_value,
                "resolved web_search_mode is disallowed by requirements; keeping constrained value"
            );
        }
        per_turn_config.features = config.features.clone();
        if let Err(err) = crate::config::refresh_codexn_fork_developer_instructions_with_runtime(
            &mut per_turn_config,
            session_configuration.nero_auto_runtime,
            &session_configuration.session_source,
        ) {
            tracing::warn!(
                error = %err,
                "failed to refresh codexn fork developer instructions for turn"
            );
        }
        per_turn_config
    }

    pub(crate) async fn codex_home(&self) -> PathBuf {
        let state = self.state.lock().await;
        state.session_configuration.codex_home().clone()
    }

    pub(crate) fn subscribe_out_of_band_elicitation_pause_state(&self) -> watch::Receiver<bool> {
        self.out_of_band_elicitation_paused.subscribe()
    }

    pub(crate) fn set_out_of_band_elicitation_pause_state(&self, paused: bool) {
        self.out_of_band_elicitation_paused.send_replace(paused);
    }

    fn start_skills_watcher_listener(self: &Arc<Self>) {
        let mut rx = self.services.skills_watcher.subscribe();
        let weak_sess = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(SkillsWatcherEvent::SkillsChanged { .. }) => {
                        let Some(sess) = weak_sess.upgrade() else {
                            break;
                        };
                        let event = Event {
                            id: sess.next_internal_sub_id(),
                            msg: EventMsg::SkillsUpdateAvailable,
                        };
                        sess.send_event_raw(event).await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn make_turn_context(
        conversation_id: ThreadId,
        auth_manager: Option<Arc<AuthManager>>,
        session_telemetry: &SessionTelemetry,
        provider: ModelProviderInfo,
        session_configuration: &SessionConfiguration,
        user_shell: &shell::Shell,
        shell_zsh_path: Option<&PathBuf>,
        main_execve_wrapper_exe: Option<&PathBuf>,
        per_turn_config: Config,
        model_info: ModelInfo,
        models_manager: &ModelsManager,
        network: Option<NetworkProxy>,
        environment: Arc<Environment>,
        sub_id: String,
        js_repl: Arc<JsReplHandle>,
        skills_outcome: Arc<SkillLoadOutcome>,
    ) -> TurnContext {
        let reasoning_effort = session_configuration.collaboration_mode.reasoning_effort();
        let reasoning_summary = session_configuration
            .model_reasoning_summary
            .unwrap_or(model_info.default_reasoning_summary);
        let session_telemetry = session_telemetry.clone().with_model(
            session_configuration.collaboration_mode.model(),
            model_info.slug.as_str(),
        );
        let session_source = session_configuration.session_source.clone();
        let auth_manager_for_context = auth_manager;
        let provider_for_context = provider;
        let session_telemetry_for_context = session_telemetry;
        let tools_config = ToolsConfig::new(&ToolsConfigParams {
            model_info: &model_info,
            available_models: &models_manager.try_list_models().unwrap_or_default(),
            features: &per_turn_config.features,
            web_search_mode: Some(per_turn_config.web_search_mode.value()),
            session_source: session_source.clone(),
            sandbox_policy: session_configuration.sandbox_policy.get(),
            windows_sandbox_level: session_configuration.windows_sandbox_level,
        })
        .with_unified_exec_shell_mode_for_session(
            user_shell,
            shell_zsh_path,
            main_execve_wrapper_exe,
        )
        .with_web_search_config(per_turn_config.web_search_config.clone())
        .with_allow_login_shell(per_turn_config.permissions.allow_login_shell)
        .with_agent_roles(per_turn_config.agent_roles.clone())
        .with_spawn_delegation_report_required(
            per_turn_config
                .spawn_delegation_report_profile
                .required_in_spawn(),
        );

        let cwd = session_configuration.cwd.clone();

        let per_turn_config = Arc::new(per_turn_config);
        let turn_metadata_state = Arc::new(TurnMetadataState::new(
            conversation_id.to_string(),
            sub_id.clone(),
            cwd.to_path_buf(),
            session_configuration.sandbox_policy.get(),
            session_configuration.windows_sandbox_level,
        ));
        let (current_date, timezone) = local_time_context();
        TurnContext {
            sub_id,
            trace_id: current_span_trace_id(),
            realtime_active: false,
            config: per_turn_config.clone(),
            auth_manager: auth_manager_for_context,
            model_info: model_info.clone(),
            session_telemetry: session_telemetry_for_context,
            provider: provider_for_context,
            reasoning_effort,
            reasoning_summary,
            session_source,
            environment,
            cwd,
            current_date: Some(current_date),
            timezone: Some(timezone),
            app_server_client_name: session_configuration.app_server_client_name.clone(),
            developer_instructions: per_turn_config.developer_instructions.clone(),
            compact_prompt: session_configuration.compact_prompt.clone(),
            user_instructions: session_configuration.user_instructions.clone(),
            collaboration_mode: session_configuration.collaboration_mode.clone(),
            model_fallback: session_configuration.nero_model_fallback.clone(),
            personality: session_configuration.personality,
            approval_policy: session_configuration.approval_policy.clone(),
            sandbox_policy: session_configuration.sandbox_policy.clone(),
            file_system_sandbox_policy: session_configuration.file_system_sandbox_policy.clone(),
            network_sandbox_policy: session_configuration.network_sandbox_policy,
            network,
            windows_sandbox_level: session_configuration.windows_sandbox_level,
            shell_environment_policy: per_turn_config.permissions.shell_environment_policy.clone(),
            tools_config,
            features: per_turn_config.features.clone(),
            ghost_snapshot: per_turn_config.ghost_snapshot.clone(),
            final_output_json_schema: None,
            codex_self_exe: per_turn_config.codex_self_exe.clone(),
            codex_linux_sandbox_exe: per_turn_config.codex_linux_sandbox_exe.clone(),
            tool_call_gate: Arc::new(ReadinessFlag::new()),
            truncation_policy: model_info.truncation_policy.into(),
            js_repl,
            dynamic_tools: session_configuration.dynamic_tools.clone(),
            turn_metadata_state,
            turn_skills: TurnSkillsContext::new(skills_outcome),
            turn_timing_state: Arc::new(TurnTimingState::default()),
        }
    }

    #[instrument(name = "session_init", level = "info", skip_all)]
    #[allow(clippy::too_many_arguments)]
    async fn new(
        mut session_configuration: SessionConfiguration,
        config: Arc<Config>,
        auth_manager: Arc<AuthManager>,
        models_manager: Arc<ModelsManager>,
        exec_policy: Arc<ExecPolicyManager>,
        tx_sub: Sender<Submission>,
        tx_event: Sender<Event>,
        agent_status: watch::Sender<AgentStatus>,
        initial_history: InitialHistory,
        session_source: SessionSource,
        environment_manager: Arc<EnvironmentManager>,
        skills_manager: Arc<SkillsManager>,
        plugins_manager: Arc<PluginsManager>,
        mcp_manager: Arc<McpManager>,
        skills_watcher: Arc<SkillsWatcher>,
        agent_control: AgentControl,
    ) -> anyhow::Result<Arc<Self>> {
        debug!(
            "Configuring session: model={}; provider={:?}",
            session_configuration.collaboration_mode.model(),
            session_configuration.provider
        );
        let forked_from_id = initial_history.forked_from_id();

        let (conversation_id, rollout_params) = match &initial_history {
            InitialHistory::New | InitialHistory::Forked(_) => {
                let conversation_id = ThreadId::default();
                (
                    conversation_id,
                    RolloutRecorderParams::new(
                        conversation_id,
                        forked_from_id,
                        session_source,
                        BaseInstructions {
                            text: session_configuration.base_instructions.clone(),
                        },
                        session_configuration.dynamic_tools.clone(),
                        if session_configuration.persist_extended_history {
                            EventPersistenceMode::Extended
                        } else {
                            EventPersistenceMode::Limited
                        },
                    ),
                )
            }
            InitialHistory::Resumed(resumed_history) => (
                resumed_history.conversation_id,
                RolloutRecorderParams::resume(
                    resumed_history.rollout_path.clone(),
                    if session_configuration.persist_extended_history {
                        EventPersistenceMode::Extended
                    } else {
                        EventPersistenceMode::Limited
                    },
                ),
            ),
        };
        let state_builder = match &initial_history {
            InitialHistory::Resumed(resumed) => metadata::builder_from_items(
                resumed.history.as_slice(),
                resumed.rollout_path.as_path(),
            ),
            InitialHistory::New | InitialHistory::Forked(_) => None,
        };

        // Kick off independent async setup tasks in parallel to reduce startup latency.
        //
        // - initialize RolloutRecorder with new or resumed session info
        // - perform default shell discovery
        // - load history metadata (skipped for subagents)
        let rollout_fut = async {
            if config.ephemeral {
                Ok::<_, anyhow::Error>((None, None))
            } else {
                let state_db_ctx = state_db::init(&config).await;
                let rollout_recorder = RolloutRecorder::new(
                    &config,
                    rollout_params,
                    state_db_ctx.clone(),
                    state_builder.clone(),
                )
                .await?;
                Ok((Some(rollout_recorder), state_db_ctx))
            }
        }
        .instrument(info_span!(
            "session_init.rollout",
            otel.name = "session_init.rollout",
            session_init.ephemeral = config.ephemeral,
        ));

        let is_subagent = matches!(
            session_configuration.session_source,
            SessionSource::SubAgent(_)
        );
        let history_meta_fut = async {
            if is_subagent {
                (0, 0)
            } else {
                crate::message_history::history_metadata(&config).await
            }
        }
        .instrument(info_span!(
            "session_init.history_metadata",
            otel.name = "session_init.history_metadata",
            session_init.is_subagent = is_subagent,
        ));
        let auth_manager_clone = Arc::clone(&auth_manager);
        let config_for_mcp = Arc::clone(&config);
        let mcp_manager_for_mcp = Arc::clone(&mcp_manager);
        let auth_and_mcp_fut = async move {
            let auth = auth_manager_clone.auth().await;
            let mcp_servers = mcp_manager_for_mcp.effective_servers(&config_for_mcp, auth.as_ref());
            let auth_statuses = compute_auth_statuses(
                mcp_servers.iter(),
                config_for_mcp.mcp_oauth_credentials_store_mode,
            )
            .await;
            (auth, mcp_servers, auth_statuses)
        }
        .instrument(info_span!(
            "session_init.auth_mcp",
            otel.name = "session_init.auth_mcp",
        ));

        // Join all independent futures.
        let (
            rollout_recorder_and_state_db,
            (history_log_id, history_entry_count),
            (auth, mcp_servers, auth_statuses),
        ) = tokio::join!(rollout_fut, history_meta_fut, auth_and_mcp_fut);

        let (rollout_recorder, state_db_ctx) = rollout_recorder_and_state_db.map_err(|e| {
            error!("failed to initialize rollout recorder: {e:#}");
            e
        })?;
        let rollout_path = rollout_recorder
            .as_ref()
            .map(|rec| rec.rollout_path().to_path_buf());

        let mut post_session_configured_events = Vec::<Event>::new();

        for usage in config.features.legacy_feature_usages() {
            post_session_configured_events.push(Event {
                id: INITIAL_SUBMIT_ID.to_owned(),
                msg: EventMsg::DeprecationNotice(DeprecationNoticeEvent {
                    summary: usage.summary.clone(),
                    details: usage.details.clone(),
                }),
            });
        }
        if crate::config::uses_deprecated_instructions_file(&config.config_layer_stack) {
            post_session_configured_events.push(Event {
                id: INITIAL_SUBMIT_ID.to_owned(),
                msg: EventMsg::DeprecationNotice(DeprecationNoticeEvent {
                    summary: "`experimental_instructions_file` is deprecated and ignored. Use `model_instructions_file` instead."
                        .to_string(),
                    details: Some(
                        "Move the setting to `model_instructions_file` in config.toml (or under a profile) to load instructions from a file."
                            .to_string(),
                    ),
                }),
            });
        }
        for message in &config.startup_warnings {
            post_session_configured_events.push(Event {
                id: "".to_owned(),
                msg: EventMsg::Warning(WarningEvent {
                    message: message.clone(),
                }),
            });
        }
        let config_path = config.codex_home.join(CONFIG_TOML_FILE);
        if let Some(event) = unstable_features_warning_event(
            config
                .config_layer_stack
                .effective_config()
                .get("features")
                .and_then(TomlValue::as_table),
            config.suppress_unstable_features_warning,
            &config.features,
            &config_path.display().to_string(),
        ) {
            post_session_configured_events.push(event);
        }
        if config.permissions.approval_policy.value() == AskForApproval::OnFailure {
            post_session_configured_events.push(Event {
                id: "".to_owned(),
                msg: EventMsg::Warning(WarningEvent {
                    message: "`on-failure` approval policy is deprecated and will be removed in a future release. Use `on-request` for interactive approvals or `never` for non-interactive runs.".to_string(),
                }),
            });
        }

        let auth = auth.as_ref();
        let auth_mode = auth.map(CodexAuth::auth_mode).map(TelemetryAuthMode::from);
        let account_id = auth.and_then(CodexAuth::get_account_id);
        let account_email = auth.and_then(CodexAuth::get_account_email);
        let originator = crate::default_client::originator().value;
        let terminal_type = user_agent();
        let session_model = session_configuration.collaboration_mode.model().to_string();
        let auth_env_telemetry = collect_auth_env_telemetry(
            &session_configuration.provider,
            auth_manager.codex_api_key_env_enabled(),
        );
        let mut session_telemetry = SessionTelemetry::new(
            conversation_id,
            session_model.as_str(),
            session_model.as_str(),
            account_id.clone(),
            account_email.clone(),
            auth_mode,
            originator.clone(),
            config.otel.log_user_prompt,
            terminal_type.clone(),
            session_configuration.session_source.clone(),
        )
        .with_auth_env(auth_env_telemetry.to_otel_metadata());
        if let Some(service_name) = session_configuration.metrics_service_name.as_deref() {
            session_telemetry = session_telemetry.with_metrics_service_name(service_name);
        }
        let network_proxy_audit_metadata = NetworkProxyAuditMetadata {
            conversation_id: Some(conversation_id.to_string()),
            app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            user_account_id: account_id,
            auth_mode: auth_mode.map(|mode| mode.to_string()),
            originator: Some(originator),
            user_email: account_email,
            terminal_type: Some(terminal_type),
            model: Some(session_model.clone()),
            slug: Some(session_model),
        };
        config.features.emit_metrics(&session_telemetry);
        session_telemetry.counter(
            THREAD_STARTED_METRIC,
            /*inc*/ 1,
            &[(
                "is_git",
                if get_git_repo_root(&session_configuration.cwd).is_some() {
                    "true"
                } else {
                    "false"
                },
            )],
        );

        session_telemetry.conversation_starts(
            config.model_provider.name.as_str(),
            session_configuration.collaboration_mode.reasoning_effort(),
            config
                .model_reasoning_summary
                .unwrap_or(ReasoningSummaryConfig::Auto),
            config.model_context_window,
            config.model_auto_compact_token_limit,
            config.permissions.approval_policy.value(),
            config.permissions.sandbox_policy.get().clone(),
            mcp_servers.keys().map(String::as_str).collect(),
            config.active_profile.clone(),
        );

        let use_zsh_fork_shell = config.features.enabled(Feature::ShellZshFork);
        let mut default_shell = if let Some(user_shell_override) =
            session_configuration.user_shell_override.clone()
        {
            user_shell_override
        } else if use_zsh_fork_shell {
            let zsh_path = config.zsh_path.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "zsh fork feature enabled, but `zsh_path` is not configured; set `zsh_path` in config.toml"
                )
            })?;
            let zsh_path = zsh_path.to_path_buf();
            shell::get_shell(shell::ShellType::Zsh, Some(&zsh_path)).ok_or_else(|| {
                anyhow::anyhow!(
                    "zsh fork feature enabled, but zsh_path `{}` is not usable; set `zsh_path` to a valid zsh executable",
                    zsh_path.display()
                )
            })?
        } else {
            shell::default_user_shell()
        };
        // Create the mutable state for the Session.
        let shell_snapshot_tx = if config.features.enabled(Feature::ShellSnapshot) {
            if let Some(snapshot) = session_configuration.inherited_shell_snapshot.clone() {
                let (tx, rx) = watch::channel(Some(snapshot));
                default_shell.shell_snapshot = rx;
                tx
            } else {
                ShellSnapshot::start_snapshotting(
                    config.codex_home.clone(),
                    conversation_id,
                    session_configuration.cwd.to_path_buf(),
                    &mut default_shell,
                    session_telemetry.clone(),
                )
            }
        } else {
            let (tx, rx) = watch::channel(None);
            default_shell.shell_snapshot = rx;
            tx
        };
        let thread_name =
            match session_index::find_thread_name_by_id(&config.codex_home, &conversation_id)
                .instrument(info_span!(
                    "session_init.thread_name_lookup",
                    otel.name = "session_init.thread_name_lookup",
                ))
                .await
            {
                Ok(name) => name,
                Err(err) => {
                    warn!("Failed to read session index for thread name: {err}");
                    None
                }
            };
        session_configuration.thread_name = thread_name.clone();
        let state = SessionState::new(session_configuration.clone());
        let managed_network_requirements_enabled = config.managed_network_requirements_enabled();
        let network_approval = Arc::new(NetworkApprovalService::default());
        // The managed proxy can call back into core for allowlist-miss decisions.
        let network_policy_decider_session = if managed_network_requirements_enabled {
            config
                .permissions
                .network
                .as_ref()
                .map(|_| Arc::new(RwLock::new(std::sync::Weak::<Session>::new())))
        } else {
            None
        };
        let blocked_request_observer = if managed_network_requirements_enabled {
            config
                .permissions
                .network
                .as_ref()
                .map(|_| build_blocked_request_observer(Arc::clone(&network_approval)))
        } else {
            None
        };
        let network_policy_decider =
            network_policy_decider_session
                .as_ref()
                .map(|network_policy_decider_session| {
                    build_network_policy_decider(
                        Arc::clone(&network_approval),
                        Arc::clone(network_policy_decider_session),
                    )
                });
        let (network_proxy, session_network_proxy) =
            if let Some(spec) = config.permissions.network.as_ref() {
                let current_exec_policy = exec_policy.current();
                let (network_proxy, session_network_proxy) = Self::start_managed_network_proxy(
                    spec,
                    current_exec_policy.as_ref(),
                    config.permissions.sandbox_policy.get(),
                    network_policy_decider.as_ref().map(Arc::clone),
                    blocked_request_observer.as_ref().map(Arc::clone),
                    managed_network_requirements_enabled,
                    network_proxy_audit_metadata,
                )
                .instrument(info_span!(
                    "session_init.network_proxy",
                    otel.name = "session_init.network_proxy",
                    session_init.managed_network_requirements_enabled =
                        managed_network_requirements_enabled,
                ))
                .await?;
                (Some(network_proxy), Some(session_network_proxy))
            } else {
                (None, None)
            };

        let mut hook_shell_argv =
            default_shell.derive_exec_args("", /*use_login_shell*/ false);
        let hook_shell_program = hook_shell_argv.remove(0);
        let _ = hook_shell_argv.pop();
        let hooks = Hooks::new(HooksConfig {
            legacy_notify_argv: config.notify.clone(),
            feature_enabled: config.features.enabled(Feature::CodexHooks),
            config_layer_stack: Some(config.config_layer_stack.clone()),
            shell_program: Some(hook_shell_program),
            shell_args: hook_shell_argv,
        });
        for warning in hooks.startup_warnings() {
            post_session_configured_events.push(Event {
                id: INITIAL_SUBMIT_ID.to_owned(),
                msg: EventMsg::Warning(WarningEvent {
                    message: warning.clone(),
                }),
            });
        }

        let services = SessionServices {
            // Initialize the MCP connection manager with an uninitialized
            // instance. It will be replaced with one created via
            // McpConnectionManager::new() once all its constructor args are
            // available. This also ensures `SessionConfigured` is emitted
            // before any MCP-related events. It is reasonable to consider
            // changing this to use Option or OnceCell, though the current
            // setup is straightforward enough and performs well.
            mcp_connection_manager: Arc::new(RwLock::new(McpConnectionManager::new_uninitialized(
                &config.permissions.approval_policy,
            ))),
            mcp_startup_cancellation_token: Mutex::new(CancellationToken::new()),
            unified_exec_manager: UnifiedExecProcessManager::new(
                config.background_terminal_max_timeout,
            ),
            shell_zsh_path: config.zsh_path.clone(),
            main_execve_wrapper_exe: config.main_execve_wrapper_exe.clone(),
            analytics_events_client: AnalyticsEventsClient::new(
                Arc::clone(&auth_manager),
                config.chatgpt_base_url.trim_end_matches('/').to_string(),
                config.analytics_enabled,
            ),
            hooks,
            rollout: Mutex::new(rollout_recorder),
            user_shell: Arc::new(default_shell),
            shell_snapshot_tx,
            show_raw_agent_reasoning: config.show_raw_agent_reasoning,
            exec_policy,
            auth_manager: Arc::clone(&auth_manager),
            session_telemetry,
            models_manager: Arc::clone(&models_manager),
            tool_approvals: Mutex::new(ApprovalStore::default()),
            skills_manager,
            plugins_manager: Arc::clone(&plugins_manager),
            mcp_manager: Arc::clone(&mcp_manager),
            skills_watcher,
            agent_control,
            network_proxy,
            network_approval: Arc::clone(&network_approval),
            state_db: state_db_ctx.clone(),
            model_client: ModelClient::new(
                Some(Arc::clone(&auth_manager)),
                conversation_id,
                session_configuration.provider.clone(),
                session_configuration.session_source.clone(),
                config.model_verbosity,
                config.features.enabled(Feature::EnableRequestCompression),
                config.features.enabled(Feature::RuntimeMetrics),
                Self::build_model_client_beta_features_header(config.as_ref()),
            ),
            code_mode_service: crate::tools::code_mode::CodeModeService::new(
                config.js_repl_node_path.clone(),
            ),
            environment: environment_manager.current().await?,
        };
        let js_repl = Arc::new(JsReplHandle::with_node_path(
            config.js_repl_node_path.clone(),
            config.js_repl_node_module_dirs.clone(),
        ));
        let (out_of_band_elicitation_paused, _out_of_band_elicitation_paused_rx) =
            watch::channel(false);

        let (mailbox, mailbox_rx) = Mailbox::new();
        let sess = Arc::new(Session {
            conversation_id,
            tx_sub,
            tx_event: tx_event.clone(),
            agent_status,
            out_of_band_elicitation_paused,
            state: Mutex::new(state),
            features: config.features.clone(),
            pending_mcp_server_refresh_config: Mutex::new(None),
            conversation: Arc::new(RealtimeConversationManager::new()),
            active_turn: Mutex::new(None),
            mailbox,
            mailbox_rx: Mutex::new(mailbox_rx),
            idle_pending_input: Mutex::new(Vec::new()),
            guardian_review_session: GuardianReviewSessionManager::default(),
            services,
            js_repl,
            hook_seen_terminal_turn_ids: Mutex::new(HashSet::new()),
            hook_auto_reply_internal_submission_ids: StdMutex::new(HashSet::new()),
            hook_nero_msg_throttle: StdMutex::new(HashMap::new()),
            nero_auto_bridge_warning_emitted: StdMutex::new(false),
            hook_auto_reply_guard_state: StdMutex::new(HookAutoReplyGuardState::default()),
            next_internal_sub_id: AtomicU64::new(0),
        });
        if let Some(network_policy_decider_session) = network_policy_decider_session {
            let mut guard = network_policy_decider_session.write().await;
            *guard = Arc::downgrade(&sess);
        }
        // Dispatch the SessionConfiguredEvent first and then report any errors.
        // If resuming, include converted initial messages in the payload so UIs can render them immediately.
        let initial_messages = initial_history.get_event_msgs();
        let events = std::iter::once(Event {
            id: INITIAL_SUBMIT_ID.to_owned(),
            msg: EventMsg::SessionConfigured(SessionConfiguredEvent {
                session_id: conversation_id,
                forked_from_id,
                thread_name: session_configuration.thread_name.clone(),
                model: session_configuration.collaboration_mode.model().to_string(),
                model_provider_id: config.model_provider_id.clone(),
                service_tier: session_configuration.service_tier,
                approval_policy: session_configuration.approval_policy.value(),
                approvals_reviewer: session_configuration.approvals_reviewer,
                sandbox_policy: session_configuration.sandbox_policy.get().clone(),
                cwd: session_configuration.cwd.to_path_buf(),
                reasoning_effort: session_configuration.collaboration_mode.reasoning_effort(),
                session_source: session_configuration.session_source.clone(),
                nero_auto_runtime: session_configuration.nero_auto_runtime,
                history_log_id,
                history_entry_count,
                initial_messages,
                network_proxy: session_network_proxy,
                rollout_path,
            }),
        })
        .chain(post_session_configured_events.into_iter());
        for event in events {
            sess.send_event_raw(event).await;
        }

        // Start the watcher after SessionConfigured so it cannot emit earlier events.
        sess.start_skills_watcher_listener();
        // Construct sandbox_state before MCP startup so it can be sent to each
        // MCP server immediately after it becomes ready (avoiding blocking).
        let sandbox_state = SandboxState {
            sandbox_policy: session_configuration.sandbox_policy.get().clone(),
            codex_linux_sandbox_exe: config.codex_linux_sandbox_exe.clone(),
            sandbox_cwd: session_configuration.cwd.to_path_buf(),
            use_legacy_landlock: config.features.use_legacy_landlock(),
        };
        let mut required_mcp_servers: Vec<String> = mcp_servers
            .iter()
            .filter(|(_, server)| server.enabled && server.required)
            .map(|(name, _)| name.clone())
            .collect();
        required_mcp_servers.sort();
        let enabled_mcp_server_count = mcp_servers.values().filter(|server| server.enabled).count();
        let required_mcp_server_count = required_mcp_servers.len();
        let tool_plugin_provenance = mcp_manager.tool_plugin_provenance(config.as_ref());
        {
            let mut cancel_guard = sess.services.mcp_startup_cancellation_token.lock().await;
            cancel_guard.cancel();
            *cancel_guard = CancellationToken::new();
        }
        let (mcp_connection_manager, cancel_token) = McpConnectionManager::new(
            &mcp_servers,
            config.mcp_oauth_credentials_store_mode,
            auth_statuses.clone(),
            &session_configuration.approval_policy,
            tx_event.clone(),
            sandbox_state,
            config.codex_home.clone(),
            codex_apps_tools_cache_key(auth),
            tool_plugin_provenance,
        )
        .instrument(info_span!(
            "session_init.mcp_manager_init",
            otel.name = "session_init.mcp_manager_init",
            session_init.enabled_mcp_server_count = enabled_mcp_server_count,
            session_init.required_mcp_server_count = required_mcp_server_count,
        ))
        .await;
        {
            let mut manager_guard = sess.services.mcp_connection_manager.write().await;
            *manager_guard = mcp_connection_manager;
        }
        {
            let mut cancel_guard = sess.services.mcp_startup_cancellation_token.lock().await;
            if cancel_guard.is_cancelled() {
                cancel_token.cancel();
            }
            *cancel_guard = cancel_token;
        }
        if !required_mcp_servers.is_empty() {
            let failures = sess
                .services
                .mcp_connection_manager
                .read()
                .await
                .required_startup_failures(&required_mcp_servers)
                .instrument(info_span!(
                    "session_init.required_mcp_wait",
                    otel.name = "session_init.required_mcp_wait",
                    session_init.required_mcp_server_count = required_mcp_server_count,
                ))
                .await;
            if !failures.is_empty() {
                let details = failures
                    .iter()
                    .map(|failure| format!("{}: {}", failure.server, failure.error))
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(anyhow::anyhow!(
                    "required MCP servers failed to initialize: {details}"
                ));
            }
        }
        sess.schedule_startup_prewarm(session_configuration.base_instructions.clone())
            .await;
        let session_start_source = match &initial_history {
            InitialHistory::Resumed(_) => codex_hooks::SessionStartSource::Resume,
            InitialHistory::New | InitialHistory::Forked(_) => {
                codex_hooks::SessionStartSource::Startup
            }
        };

        // record_initial_history can emit events. We record only after the SessionConfiguredEvent is emitted.
        sess.record_initial_history(initial_history).await;
        {
            let mut state = sess.state.lock().await;
            state.set_pending_session_start_source(Some(session_start_source));
        }

        memories::start_memories_startup_task(
            &sess,
            Arc::clone(&config),
            &session_configuration.session_source,
        );

        Ok(sess)
    }

    pub(crate) fn get_tx_event(&self) -> Sender<Event> {
        self.tx_event.clone()
    }

    pub(crate) fn state_db(&self) -> Option<state_db::StateDbHandle> {
        self.services.state_db.clone()
    }

    /// Ensure rollout file writes are durably flushed.
    pub(crate) async fn flush_rollout(&self) {
        let recorder = {
            let guard = self.services.rollout.lock().await;
            guard.clone()
        };
        if let Some(rec) = recorder
            && let Err(e) = rec.flush().await
        {
            warn!("failed to flush rollout recorder: {e}");
        }
    }

    pub(crate) async fn ensure_rollout_materialized(&self) {
        let recorder = {
            let guard = self.services.rollout.lock().await;
            guard.clone()
        };
        if let Some(rec) = recorder
            && let Err(e) = rec.persist().await
        {
            warn!("failed to materialize rollout recorder: {e}");
        }
    }

    fn next_internal_sub_id(&self) -> String {
        let id = self
            .next_internal_sub_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("auto-compact-{id}")
    }

    fn next_internal_sub_id_with_prefix(&self, prefix: &str) -> String {
        let id = self
            .next_internal_sub_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("{prefix}{id}")
    }

    async fn begin_new_user_submission_generation(&self) -> u64 {
        let next_epoch = {
            let mut guard = match self.hook_auto_reply_guard_state.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.epoch = guard.epoch.saturating_add(1);
            guard.chain_depth = 0;
            guard.epoch
        };
        let cleared_markers = {
            let mut seen = self.hook_seen_terminal_turn_ids.lock().await;
            let len = seen.len();
            seen.clear();
            len
        };
        if cleared_markers > 0 {
            debug!(
                generation_epoch = next_epoch,
                cleared_markers, "cleared hook terminal markers for new user submission generation"
            );
        }
        next_epoch
    }

    pub(crate) async fn note_user_input_activity(&self) -> u64 {
        self.begin_new_user_submission_generation().await
    }

    fn current_hook_auto_reply_epoch(&self) -> u64 {
        let guard = match self.hook_auto_reply_guard_state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.epoch
    }

    fn reset_nero_hook_msg_throttle(&self) {
        let mut guard = match self.hook_nero_msg_throttle.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let cleared = guard.len();
        guard.clear();
        debug!(cleared, "reset nero_hook_msg throttle cache");
    }

    async fn maybe_emit_nero_auto_session_auto_read_warning(
        &self,
        turn_context: &TurnContext,
        detail: &str,
    ) {
        let should_emit = {
            let mut guard = match self.nero_auto_bridge_warning_emitted.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if *guard {
                false
            } else {
                *guard = true;
                true
            }
        };
        if !should_emit {
            return;
        }
        let message = nero_hook_tui_warning_message(
            &format!(
                "NERO auto booster session-auto runtime read failed for this session: {detail}"
            ),
            NeroHookMsgFormat::Block,
            Some(("warning", "session-auto-read")),
        );
        self.send_event(turn_context, EventMsg::Warning(WarningEvent { message }))
            .await;
    }

    fn nero_hook_msg_throttle_remaining(
        &self,
        hook_name: &str,
        mode: &NeroHookMsgMode,
        format: &NeroHookMsgFormat,
        show_agent: bool,
        show_tui: bool,
        full: &str,
        short: &str,
        status: Option<&codex_hooks::NeroHookMsgStatus>,
        freq_seconds: u64,
    ) -> Option<StdDuration> {
        if freq_seconds == 0 {
            return None;
        }
        let now = StdInstant::now();
        let key = nero_hook_msg_throttle_key(
            hook_name,
            mode,
            format,
            show_agent,
            show_tui,
            full,
            short,
            status.map(|s| s.kind.as_str()),
            status.map(|s| s.text.as_str()),
        );
        let mut guard = match self.hook_nero_msg_throttle.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let window = StdDuration::from_secs(freq_seconds);
        if let Some(last_seen) = guard.get(&key) {
            let elapsed = now.saturating_duration_since(*last_seen);
            if elapsed < window {
                return Some(window - elapsed);
            }
        }
        guard.insert(key, now);
        None
    }

    pub(crate) async fn mark_turn_terminal_event_emitted(&self, turn_id: &str) {
        self.hook_seen_terminal_turn_ids
            .lock()
            .await
            .insert(turn_id.to_string());
    }

    async fn clear_turn_terminal_marker(&self, turn_id: &str) {
        self.hook_seen_terminal_turn_ids
            .lock()
            .await
            .remove(turn_id);
    }

    fn try_reserve_hook_auto_reply_chain_slot(&self) -> Option<(u32, u64)> {
        let mut guard = match self.hook_auto_reply_guard_state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.chain_depth >= HOOK_AUTO_REPLY_MAX_CHAIN_DEPTH {
            return None;
        }
        guard.chain_depth += 1;
        Some((guard.chain_depth, guard.epoch))
    }

    fn release_hook_auto_reply_chain_slot_for_epoch(&self, epoch: u64) {
        let mut guard = match self.hook_auto_reply_guard_state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.epoch != epoch {
            debug!(
                guard_epoch = guard.epoch,
                release_epoch = epoch,
                "skipping hook auto-reply slot release for stale epoch"
            );
            return;
        }
        guard.chain_depth = guard.chain_depth.saturating_sub(1);
    }

    fn normalize_hook_auto_reply_wait_seconds(expected_wait_seconds: Option<u64>) -> Option<u64> {
        let max_wait_seconds = HOOK_AUTO_REPLY_WAIT_FOR_TERMINAL_TIMEOUT_MS.div_ceil(1000);
        expected_wait_seconds
            .filter(|wait_seconds| *wait_seconds > 0)
            .map(|wait_seconds| wait_seconds.min(max_wait_seconds))
            .filter(|wait_seconds| *wait_seconds > 0)
    }

    fn register_internal_hook_auto_submission_id(&self, submission_id: String) {
        let mut ids = match self.hook_auto_reply_internal_submission_ids.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        ids.insert(submission_id);
    }

    fn take_internal_hook_auto_submission_id(&self, submission_id: &str) -> bool {
        let mut ids = match self.hook_auto_reply_internal_submission_ids.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        ids.remove(submission_id)
    }

    fn spawn_deferred_auto_user_reply(
        self: &Arc<Self>,
        source_turn_id: String,
        hook_name: String,
        text: String,
        expected_wait_seconds: Option<u64>,
        session_source: SessionSource,
    ) {
        if session_source_blocks_nero_msg_auto_lane(&session_source) {
            debug!(
                turn_id = %source_turn_id,
                hook_name = %hook_name,
                session_source = %session_source,
                "skipping synthetic user reply from hook for subagent session"
            );
            return;
        }

        let Some((chain_depth, reservation_epoch)) = self.try_reserve_hook_auto_reply_chain_slot()
        else {
            info!(
                turn_id = %source_turn_id,
                hook_name = %hook_name,
                max_chain_depth = HOOK_AUTO_REPLY_MAX_CHAIN_DEPTH,
                "skipping synthetic user reply from hook action (auto-reply chain depth exhausted)"
            );
            return;
        };

        let sess = Arc::clone(self);
        tokio::spawn(async move {
            let wait_started = StdInstant::now();
            loop {
                if sess.current_hook_auto_reply_epoch() != reservation_epoch {
                    sess.release_hook_auto_reply_chain_slot_for_epoch(reservation_epoch);
                    info!(
                        turn_id = %source_turn_id,
                        hook_name = %hook_name,
                        reservation_epoch,
                        current_epoch = sess.current_hook_auto_reply_epoch(),
                        "dropping deferred synthetic user reply while waiting because a newer generation is active"
                    );
                    sess.clear_turn_terminal_marker(&source_turn_id).await;
                    return;
                }
                let source_turn_terminal_event_emitted = {
                    let guard = sess.hook_seen_terminal_turn_ids.lock().await;
                    guard.contains(source_turn_id.as_str())
                };
                if source_turn_terminal_event_emitted {
                    break;
                }
                if wait_started.elapsed()
                    >= StdDuration::from_millis(HOOK_AUTO_REPLY_WAIT_FOR_TERMINAL_TIMEOUT_MS)
                {
                    sess.release_hook_auto_reply_chain_slot_for_epoch(reservation_epoch);
                    sess.clear_turn_terminal_marker(&source_turn_id).await;
                    warn!(
                        turn_id = %source_turn_id,
                        hook_name = %hook_name,
                        reservation_epoch,
                        timeout_ms = HOOK_AUTO_REPLY_WAIT_FOR_TERMINAL_TIMEOUT_MS,
                        "timed out waiting for source turn terminal marker; dropping synthetic user reply"
                    );
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(
                    HOOK_AUTO_REPLY_WAIT_POLL_INTERVAL_MS,
                ))
                .await;
            }
            let apply_tab_priority_grace = {
                let state = sess.state.lock().await;
                matches!(
                    state.session_configuration.session_source,
                    SessionSource::Cli | SessionSource::VSCode
                )
            };
            if apply_tab_priority_grace && HOOK_AUTO_REPLY_TAB_PRIORITY_GRACE_MS > 0 {
                debug!(
                    turn_id = %source_turn_id,
                    hook_name = %hook_name,
                    grace_ms = HOOK_AUTO_REPLY_TAB_PRIORITY_GRACE_MS,
                    "delaying synthetic user reply to prioritize queued user input"
                );
                tokio::time::sleep(std::time::Duration::from_millis(
                    HOOK_AUTO_REPLY_TAB_PRIORITY_GRACE_MS,
                ))
                .await;
            }
            if let Some(wait_seconds) =
                Self::normalize_hook_auto_reply_wait_seconds(expected_wait_seconds)
            {
                let scheduled_wait = StdDuration::from_secs(wait_seconds);
                let delay_started = StdInstant::now();
                info!(
                    turn_id = %source_turn_id,
                    hook_name = %hook_name,
                    reservation_epoch,
                    requested_wait_seconds = expected_wait_seconds,
                    wait_seconds,
                    "delaying synthetic user reply by requested expected_wait_seconds"
                );
                while delay_started.elapsed() < scheduled_wait {
                    if sess.current_hook_auto_reply_epoch() != reservation_epoch {
                        sess.release_hook_auto_reply_chain_slot_for_epoch(reservation_epoch);
                        info!(
                            turn_id = %source_turn_id,
                            hook_name = %hook_name,
                            reservation_epoch,
                            current_epoch = sess.current_hook_auto_reply_epoch(),
                            wait_seconds,
                            "dropping delayed synthetic user reply because a newer user submission generation is active"
                        );
                        sess.clear_turn_terminal_marker(&source_turn_id).await;
                        return;
                    }
                    let remaining = scheduled_wait.saturating_sub(delay_started.elapsed());
                    if remaining.is_zero() {
                        break;
                    }
                    let sleep_for = remaining.min(StdDuration::from_millis(
                        HOOK_AUTO_REPLY_WAIT_POLL_INTERVAL_MS,
                    ));
                    tokio::time::sleep(sleep_for).await;
                }
            }
            if sess.current_hook_auto_reply_epoch() != reservation_epoch {
                sess.release_hook_auto_reply_chain_slot_for_epoch(reservation_epoch);
                info!(
                    turn_id = %source_turn_id,
                    hook_name = %hook_name,
                    reservation_epoch,
                    current_epoch = sess.current_hook_auto_reply_epoch(),
                    "dropping stale synthetic user reply because a newer user submission generation is active"
                );
                sess.clear_turn_terminal_marker(&source_turn_id).await;
                return;
            }
            sess.clear_turn_terminal_marker(&source_turn_id).await;

            let hook_auto_submission_prefix =
                format!("{HOOK_AUTO_REPLY_SUBMISSION_PREFIX}{reservation_epoch}-");
            let submission_id = sess.next_internal_sub_id_with_prefix(&hook_auto_submission_prefix);
            sess.register_internal_hook_auto_submission_id(submission_id.clone());
            let send_result = sess.tx_sub.send(Submission {
                id: submission_id.clone(),
                op: Op::UserInput {
                    items: vec![UserInput::Text {
                        text,
                        text_elements: Vec::new(),
                    }],
                    final_output_json_schema: None,
                },
                trace: None,
            });
            match send_result.await {
                Ok(()) => {
                    info!(
                            turn_id = %source_turn_id,
                        hook_name = %hook_name,
                        chain_depth,
                        reservation_epoch,
                        expected_wait_seconds,
                        %submission_id,
                        "queued synthetic user reply from hook action"
                    );
                }
                Err(err) => {
                    let _ = sess.take_internal_hook_auto_submission_id(&submission_id);
                    sess.release_hook_auto_reply_chain_slot_for_epoch(reservation_epoch);
                    warn!(
                        turn_id = %source_turn_id,
                        hook_name = %hook_name,
                        error = %err,
                        "failed to queue synthetic user reply from hook action"
                    );
                }
            }
        });
    }

    pub(crate) async fn route_realtime_text_input(self: &Arc<Self>, text: String) {
        handlers::user_input_or_turn(
            self,
            self.next_internal_sub_id(),
            Op::UserInput {
                items: vec![UserInput::Text {
                    text,
                    text_elements: Vec::new(),
                }],
                final_output_json_schema: None,
            },
        )
        .await;
    }

    pub(crate) async fn get_total_token_usage(&self) -> i64 {
        let state = self.state.lock().await;
        state.get_total_token_usage(state.server_reasoning_included())
    }

    pub(crate) async fn get_total_token_usage_breakdown(&self) -> TotalTokenUsageBreakdown {
        let state = self.state.lock().await;
        state.history.get_total_token_usage_breakdown()
    }

    pub(crate) async fn total_token_usage(&self) -> Option<TokenUsage> {
        let state = self.state.lock().await;
        state.token_info().map(|info| info.total_token_usage)
    }

    pub(crate) async fn get_estimated_token_count(
        &self,
        turn_context: &TurnContext,
    ) -> Option<i64> {
        let state = self.state.lock().await;
        state.history.estimate_token_count(turn_context)
    }

    pub(crate) async fn get_base_instructions(&self) -> BaseInstructions {
        let state = self.state.lock().await;
        BaseInstructions {
            text: state.session_configuration.base_instructions.clone(),
        }
    }

    // Merges connector IDs into the session-level explicit connector selection.
    pub(crate) async fn merge_connector_selection(
        &self,
        connector_ids: HashSet<String>,
    ) -> HashSet<String> {
        let mut state = self.state.lock().await;
        state.merge_connector_selection(connector_ids)
    }

    // Returns the connector IDs currently selected for this session.
    pub(crate) async fn get_connector_selection(&self) -> HashSet<String> {
        let state = self.state.lock().await;
        state.get_connector_selection()
    }

    // Clears connector IDs that were accumulated for explicit selection.
    pub(crate) async fn clear_connector_selection(&self) {
        let mut state = self.state.lock().await;
        state.clear_connector_selection();
    }

    async fn record_initial_history(&self, conversation_history: InitialHistory) {
        let turn_context = self.new_default_turn().await;
        let is_subagent = {
            let state = self.state.lock().await;
            matches!(
                state.session_configuration.session_source,
                SessionSource::SubAgent(_)
            )
        };
        match conversation_history {
            InitialHistory::New => {
                // Defer initial context insertion until the first real turn starts so
                // turn/start overrides can be merged before we write model-visible context.
                self.set_previous_turn_settings(/*previous_turn_settings*/ None)
                    .await;
            }
            InitialHistory::Resumed(resumed_history) => {
                let rollout_items = resumed_history.history;
                let previous_turn_settings = self
                    .apply_rollout_reconstruction(&turn_context, &rollout_items)
                    .await;

                // If resuming, warn when the last recorded model differs from the current one.
                let curr: &str = turn_context.model_info.slug.as_str();
                if let Some(prev) = previous_turn_settings
                    .as_ref()
                    .map(|settings| settings.model.as_str())
                    .filter(|model| *model != curr)
                {
                    warn!("resuming session with different model: previous={prev}, current={curr}");
                    self.send_event(
                        &turn_context,
                        EventMsg::Warning(WarningEvent {
                            message: format!(
                                "This session was recorded with model `{prev}` but is resuming with `{curr}`. \
                         Consider switching back to `{prev}` as it may affect Codex performance."
                            ),
                        }),
                    )
                    .await;
                }

                // Seed usage info from the recorded rollout so UIs can show token counts
                // immediately on resume/fork.
                if let Some(info) = Self::last_token_info_from_rollout(&rollout_items) {
                    let mut state = self.state.lock().await;
                    state.set_token_info(Some(info));
                }

                // Defer seeding the session's initial context until the first turn starts so
                // turn/start overrides can be merged before we write to the rollout.
                if !is_subagent {
                    self.flush_rollout().await;
                }
            }
            InitialHistory::Forked(rollout_items) => {
                self.apply_rollout_reconstruction(&turn_context, &rollout_items)
                    .await;

                // Seed usage info from the recorded rollout so UIs can show token counts
                // immediately on resume/fork.
                if let Some(info) = Self::last_token_info_from_rollout(&rollout_items) {
                    let mut state = self.state.lock().await;
                    state.set_token_info(Some(info));
                }

                // If persisting, persist all rollout items as-is (recorder filters)
                if !rollout_items.is_empty() {
                    self.persist_rollout_items(&rollout_items).await;
                }

                // Forked threads should remain file-backed immediately after startup.
                self.ensure_rollout_materialized().await;

                // Flush after seeding history and any persisted rollout copy.
                if !is_subagent {
                    self.flush_rollout().await;
                }
            }
        }
    }

    async fn apply_rollout_reconstruction(
        &self,
        turn_context: &TurnContext,
        rollout_items: &[RolloutItem],
    ) -> Option<PreviousTurnSettings> {
        let reconstructed_rollout = self
            .reconstruct_history_from_rollout(turn_context, rollout_items)
            .await;
        let previous_turn_settings = reconstructed_rollout.previous_turn_settings.clone();
        self.replace_history(
            reconstructed_rollout.history,
            reconstructed_rollout.reference_context_item,
        )
        .await;
        self.set_previous_turn_settings(previous_turn_settings.clone())
            .await;
        previous_turn_settings
    }

    fn last_token_info_from_rollout(rollout_items: &[RolloutItem]) -> Option<TokenUsageInfo> {
        rollout_items.iter().rev().find_map(|item| match item {
            RolloutItem::EventMsg(EventMsg::TokenCount(ev)) => ev.info.clone(),
            _ => None,
        })
    }

    async fn previous_turn_settings(&self) -> Option<PreviousTurnSettings> {
        let state = self.state.lock().await;
        state.previous_turn_settings()
    }

    pub(crate) async fn set_previous_turn_settings(
        &self,
        previous_turn_settings: Option<PreviousTurnSettings>,
    ) {
        let mut state = self.state.lock().await;
        state.set_previous_turn_settings(previous_turn_settings);
    }

    fn maybe_refresh_shell_snapshot_for_cwd(
        &self,
        previous_cwd: &Path,
        next_cwd: &Path,
        codex_home: &Path,
        session_source: &SessionSource,
    ) {
        if previous_cwd == next_cwd {
            return;
        }

        if !self.features.enabled(Feature::ShellSnapshot) {
            return;
        }

        if matches!(
            session_source,
            SessionSource::SubAgent(SubAgentSource::ThreadSpawn { .. })
        ) {
            return;
        }

        ShellSnapshot::refresh_snapshot(
            codex_home.to_path_buf(),
            self.conversation_id,
            next_cwd.to_path_buf(),
            self.services.user_shell.as_ref().clone(),
            self.services.shell_snapshot_tx.clone(),
            self.services.session_telemetry.clone(),
        );
    }

    pub(crate) async fn update_settings(
        &self,
        updates: SessionSettingsUpdate,
    ) -> ConstraintResult<()> {
        let mut state = self.state.lock().await;

        match state.session_configuration.apply(&updates) {
            Ok(updated) => {
                let previous_cwd = state.session_configuration.cwd.clone();
                let next_cwd = updated.cwd.clone();
                let codex_home = updated.codex_home.clone();
                let session_source = updated.session_source.clone();
                state.session_configuration = updated;
                drop(state);

                self.maybe_refresh_shell_snapshot_for_cwd(
                    &previous_cwd,
                    &next_cwd,
                    &codex_home,
                    &session_source,
                );

                Ok(())
            }
            Err(err) => {
                warn!("rejected session settings update: {err}");
                Err(err)
            }
        }
    }

    pub(crate) async fn new_turn_with_sub_id(
        &self,
        sub_id: String,
        updates: SessionSettingsUpdate,
    ) -> ConstraintResult<Arc<TurnContext>> {
        let (
            session_configuration,
            sandbox_policy_changed,
            previous_cwd,
            codex_home,
            session_source,
        ) = {
            let mut state = self.state.lock().await;
            match state.session_configuration.clone().apply(&updates) {
                Ok(next) => {
                    let previous_cwd = state.session_configuration.cwd.clone();
                    let sandbox_policy_changed =
                        state.session_configuration.sandbox_policy != next.sandbox_policy;
                    let codex_home = next.codex_home.clone();
                    let session_source = next.session_source.clone();
                    state.session_configuration = next.clone();
                    (
                        next,
                        sandbox_policy_changed,
                        previous_cwd,
                        codex_home,
                        session_source,
                    )
                }
                Err(err) => {
                    drop(state);
                    self.send_event_raw(Event {
                        id: sub_id.clone(),
                        msg: EventMsg::Error(ErrorEvent {
                            message: err.to_string(),
                            codex_error_info: Some(CodexErrorInfo::BadRequest),
                        }),
                    })
                    .await;
                    return Err(err);
                }
            }
        };

        self.maybe_refresh_shell_snapshot_for_cwd(
            &previous_cwd,
            &session_configuration.cwd,
            &codex_home,
            &session_source,
        );

        Ok(self
            .new_turn_from_configuration(
                sub_id,
                session_configuration,
                updates.final_output_json_schema,
                sandbox_policy_changed,
            )
            .await)
    }

    async fn apply_model_fallback_pre_turn(
        &self,
        mut session_configuration: SessionConfiguration,
    ) -> SessionConfiguration {
        let Some(model_fallback) = session_configuration.nero_model_fallback.clone() else {
            return session_configuration;
        };

        let requested_model = session_configuration.collaboration_mode.model().to_string();
        let requested_effort = session_configuration.collaboration_mode.reasoning_effort();
        let now = StdInstant::now();
        let selected_step = {
            let mut state = self.state.lock().await;
            let runtime = &mut state.model_fallback_runtime;
            runtime.prune_expired(now);

            if runtime.last_requested_model.as_deref() != Some(requested_model.as_str()) {
                runtime.sticky_step = None;
            }
            runtime.last_requested_model = Some(requested_model.clone());

            let sticky_candidate = if model_fallback.sticky {
                runtime.sticky_step.clone()
            } else {
                None
            };
            if let Some(sticky_step) = sticky_candidate {
                if runtime
                    .cooldown_remaining(sticky_step.model.as_str(), now)
                    .is_none()
                {
                    Some(sticky_step)
                } else {
                    next_available_model_fallback_step(
                        &model_fallback.ladder,
                        Some(sticky_step.model.as_str()),
                        now,
                        |model, now| runtime.cooldown_remaining(model, now),
                    )
                }
            } else if runtime
                .cooldown_remaining(requested_model.as_str(), now)
                .is_some()
            {
                next_available_model_fallback_step(
                    &model_fallback.ladder,
                    Some(requested_model.as_str()),
                    now,
                    |model, now| runtime.cooldown_remaining(model, now),
                )
            } else {
                None
            }
        };

        if let Some(step) = selected_step
            && (step.model != requested_model || Some(step.reasoning_effort) != requested_effort)
        {
            warn!(
                from_model = %requested_model,
                to_model = %step.model,
                "applying pre-turn model fallback selection"
            );
            session_configuration.collaboration_mode =
                session_configuration.collaboration_mode.with_updates(
                    Some(step.model),
                    Some(Some(step.reasoning_effort)),
                    /*developer_instructions*/ None,
                );
        }

        session_configuration
    }

    async fn try_model_fallback_after_error(
        &self,
        turn_context: &Arc<TurnContext>,
        err: &CodexErr,
        wait_deadline: &mut Option<StdInstant>,
        cancellation_token: &CancellationToken,
        delivery_log_path: Option<&Path>,
    ) -> CodexResult<ModelFallbackAfterErrorOutcome> {
        let Some(model_fallback) = turn_context.model_fallback.clone() else {
            return Ok(ModelFallbackAfterErrorOutcome::Noop);
        };
        if !should_trigger_model_fallback(err) {
            return Ok(ModelFallbackAfterErrorOutcome::Noop);
        }

        let current_model = turn_context.model_info.slug.clone();
        loop {
            let now = StdInstant::now();
            let Some(cooldown_until) =
                now.checked_add(StdDuration::from_secs(model_fallback.cooldown_seconds))
            else {
                let warning = format!(
                    "Model fallback disabled for this turn: cooldown_seconds={} overflows runtime duration arithmetic.",
                    model_fallback.cooldown_seconds
                );
                warn!("{warning}");
                self.send_event(
                    turn_context,
                    EventMsg::Warning(WarningEvent { message: warning }),
                )
                .await;
                append_nero_model_fallback_audit(
                    delivery_log_path,
                    &self.conversation_id,
                    turn_context,
                    "disabled-overflow",
                    json!({
                        "reason": "cooldown_overflow",
                        "cooldown_seconds": model_fallback.cooldown_seconds,
                        "from_model": current_model,
                    }),
                )
                .await;
                return Ok(ModelFallbackAfterErrorOutcome::DisabledOverflow);
            };
            let (candidate, wait_for) = {
                let mut state = self.state.lock().await;
                let runtime = &mut state.model_fallback_runtime;
                runtime.prune_expired(now);
                runtime.set_cooldown_for(current_model.clone(), cooldown_until);

                let candidate = next_available_model_fallback_step(
                    &model_fallback.ladder,
                    Some(current_model.as_str()),
                    now,
                    |model, now| runtime.cooldown_remaining(model, now),
                );
                let wait_for = if candidate.is_none() {
                    model_fallback
                        .ladder
                        .iter()
                        .filter(|step| step.model != current_model)
                        .filter_map(|step| runtime.cooldown_remaining(step.model.as_str(), now))
                        .min()
                } else {
                    None
                };
                (candidate, wait_for)
            };

            if let Some(step) = candidate {
                self.services.session_telemetry.counter(
                    "codex.nero.model_fallback.attempt",
                    /*inc*/ 1,
                    &[],
                );
                let warning = format!(
                    "Model fallback activated: `{}` -> `{}` ({err}).",
                    current_model, step.model
                );
                self.send_event(
                    turn_context,
                    EventMsg::Warning(WarningEvent { message: warning }),
                )
                .await;
                let next_context = Arc::new(
                    turn_context
                        .with_model_and_reasoning(
                            step.model.clone(),
                            Some(step.reasoning_effort),
                            &self.services.models_manager,
                        )
                        .await,
                );
                append_nero_model_fallback_audit(
                    delivery_log_path,
                    &self.conversation_id,
                    turn_context,
                    "attempt",
                    json!({
                        "from_model": current_model.clone(),
                        "to_model": step.model.clone(),
                        "to_reasoning_effort": model_fallback_reasoning_label(next_context.reasoning_effort),
                        "requested_reasoning_effort": model_fallback_reasoning_label(Some(step.reasoning_effort)),
                        "trigger_error": err.to_string(),
                        "cooldown_seconds": model_fallback.cooldown_seconds,
                        "max_wait_seconds": model_fallback.max_wait_seconds,
                    }),
                )
                .await;
                return Ok(ModelFallbackAfterErrorOutcome::Switched(next_context, step));
            }

            if model_fallback.max_wait_seconds == 0 {
                return Ok(ModelFallbackAfterErrorOutcome::Exhausted);
            }

            let Some(wait_for) = wait_for else {
                return Ok(ModelFallbackAfterErrorOutcome::Exhausted);
            };
            let max_wait = StdDuration::from_secs(model_fallback.max_wait_seconds);
            let deadline = if let Some(existing_deadline) = wait_deadline.as_ref() {
                *existing_deadline
            } else {
                let Some(new_deadline) = now.checked_add(max_wait) else {
                    let warning = format!(
                        "Model fallback disabled for this turn: max_wait_seconds={} overflows runtime duration arithmetic.",
                        model_fallback.max_wait_seconds
                    );
                    warn!("{warning}");
                    self.send_event(
                        turn_context,
                        EventMsg::Warning(WarningEvent { message: warning }),
                    )
                    .await;
                    append_nero_model_fallback_audit(
                        delivery_log_path,
                        &self.conversation_id,
                        turn_context,
                        "disabled-overflow",
                        json!({
                            "reason": "max_wait_overflow",
                            "max_wait_seconds": model_fallback.max_wait_seconds,
                            "from_model": current_model,
                        }),
                    )
                    .await;
                    return Ok(ModelFallbackAfterErrorOutcome::DisabledOverflow);
                };
                *wait_deadline = Some(new_deadline);
                new_deadline
            };
            if now >= deadline {
                return Ok(ModelFallbackAfterErrorOutcome::Exhausted);
            }
            let remaining_budget = deadline.saturating_duration_since(now);
            let sleep_for = wait_for.min(remaining_budget);
            if sleep_for.is_zero() {
                return Ok(ModelFallbackAfterErrorOutcome::Exhausted);
            }
            let warning = format!(
                "All fallback models are cooling down. Waiting {sleep_for:?} before retry."
            );
            self.send_event(
                turn_context,
                EventMsg::Warning(WarningEvent { message: warning }),
            )
            .await;
            tokio::select! {
                _ = tokio::time::sleep(sleep_for) => {},
                _ = cancellation_token.cancelled() => return Err(CodexErr::TurnAborted),
            }
        }
    }

    async fn mark_model_fallback_success(
        &self,
        model_fallback: &NeroModelFallbackConfig,
        step: &NeroModelFallbackStep,
    ) {
        let mut state = self.state.lock().await;
        let runtime = &mut state.model_fallback_runtime;
        runtime.clear_cooldown_for(step.model.as_str());
        if model_fallback.sticky {
            runtime.sticky_step = Some(step.clone());
        } else {
            runtime.sticky_step = None;
        }
        self.services.session_telemetry.counter(
            "codex.nero.model_fallback.success",
            /*inc*/ 1,
            &[],
        );
    }

    async fn new_turn_from_configuration(
        &self,
        sub_id: String,
        session_configuration: SessionConfiguration,
        final_output_json_schema: Option<Option<Value>>,
        sandbox_policy_changed: bool,
    ) -> Arc<TurnContext> {
        let session_configuration = self
            .apply_model_fallback_pre_turn(session_configuration)
            .await;
        let per_turn_config = Self::build_per_turn_config(&session_configuration);
        self.services
            .mcp_connection_manager
            .read()
            .await
            .set_approval_policy(&session_configuration.approval_policy);

        if sandbox_policy_changed {
            let sandbox_state = SandboxState {
                sandbox_policy: per_turn_config.permissions.sandbox_policy.get().clone(),
                codex_linux_sandbox_exe: per_turn_config.codex_linux_sandbox_exe.clone(),
                sandbox_cwd: per_turn_config.cwd.to_path_buf(),
                use_legacy_landlock: per_turn_config.features.use_legacy_landlock(),
            };
            if let Err(e) = self
                .services
                .mcp_connection_manager
                .read()
                .await
                .notify_sandbox_state_change(&sandbox_state)
                .await
            {
                warn!("Failed to notify sandbox state change to MCP servers: {e:#}");
            }
        }

        let model_info = self
            .services
            .models_manager
            .get_model_info(
                session_configuration.collaboration_mode.model(),
                &per_turn_config,
            )
            .await;
        let plugin_outcome = self
            .services
            .plugins_manager
            .plugins_for_config(&per_turn_config);
        let effective_skill_roots = plugin_outcome.effective_skill_roots();
        let skills_input = skills_load_input_from_config(&per_turn_config, effective_skill_roots);
        let skills_outcome = Arc::new(
            self.services
                .skills_manager
                .skills_for_config(&skills_input),
        );
        let mut turn_context: TurnContext = Self::make_turn_context(
            self.conversation_id,
            Some(Arc::clone(&self.services.auth_manager)),
            &self.services.session_telemetry,
            session_configuration.provider.clone(),
            &session_configuration,
            self.services.user_shell.as_ref(),
            self.services.shell_zsh_path.as_ref(),
            self.services.main_execve_wrapper_exe.as_ref(),
            per_turn_config,
            model_info,
            &self.services.models_manager,
            self.services
                .network_proxy
                .as_ref()
                .map(StartedNetworkProxy::proxy),
            Arc::clone(&self.services.environment),
            sub_id,
            Arc::clone(&self.js_repl),
            skills_outcome,
        );
        turn_context.realtime_active = self.conversation.running_state().await.is_some();

        if let Some(final_schema) = final_output_json_schema {
            turn_context.final_output_json_schema = final_schema;
        }
        let turn_context = Arc::new(turn_context);
        turn_context.turn_metadata_state.spawn_git_enrichment_task();
        turn_context
    }

    pub(crate) async fn maybe_emit_unknown_model_warning_for_turn(&self, tc: &TurnContext) {
        if tc.model_info.used_fallback_model_metadata {
            self.send_event(
                tc,
                EventMsg::Warning(WarningEvent {
                    message: format!(
                        "Model metadata for `{}` not found. Defaulting to fallback metadata; this can degrade performance and cause issues.",
                        tc.model_info.slug
                    ),
                }),
            )
            .await;
        }
    }

    pub(crate) async fn new_default_turn(&self) -> Arc<TurnContext> {
        self.new_default_turn_with_sub_id(self.next_internal_sub_id())
            .await
    }

    pub(crate) async fn set_session_startup_prewarm(
        &self,
        startup_prewarm: SessionStartupPrewarmHandle,
    ) {
        let mut state = self.state.lock().await;
        state.set_session_startup_prewarm(startup_prewarm);
    }

    pub(crate) async fn take_session_startup_prewarm(&self) -> Option<SessionStartupPrewarmHandle> {
        let mut state = self.state.lock().await;
        state.take_session_startup_prewarm()
    }

    pub(crate) async fn get_config(&self) -> std::sync::Arc<Config> {
        let state = self.state.lock().await;
        state
            .session_configuration
            .original_config_do_not_use
            .clone()
    }

    pub(crate) async fn provider(&self) -> ModelProviderInfo {
        let state = self.state.lock().await;
        state.session_configuration.provider.clone()
    }

    pub(crate) async fn reload_user_config_layer(&self) {
        let config_toml_path = {
            let state = self.state.lock().await;
            state
                .session_configuration
                .codex_home
                .join(CONFIG_TOML_FILE)
        };

        let user_config = match std::fs::read_to_string(&config_toml_path) {
            Ok(contents) => match toml::from_str::<toml::Value>(&contents) {
                Ok(config) => config,
                Err(err) => {
                    warn!("failed to parse user config while reloading layer: {err}");
                    return;
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                toml::Value::Table(Default::default())
            }
            Err(err) => {
                warn!("failed to read user config while reloading layer: {err}");
                return;
            }
        };

        let config_toml_path = match AbsolutePathBuf::try_from(config_toml_path) {
            Ok(path) => path,
            Err(err) => {
                warn!("failed to resolve user config path while reloading layer: {err}");
                return;
            }
        };

        let mut state = self.state.lock().await;
        let mut config = (*state.session_configuration.original_config_do_not_use).clone();
        config.config_layer_stack = config
            .config_layer_stack
            .with_user_config(&config_toml_path, user_config);
        state.session_configuration.original_config_do_not_use = Arc::new(config);
        self.services.skills_manager.clear_cache();
        self.services.plugins_manager.clear_cache();
    }

    pub(crate) async fn new_default_turn_with_sub_id(&self, sub_id: String) -> Arc<TurnContext> {
        let session_configuration = {
            let state = self.state.lock().await;
            state.session_configuration.clone()
        };
        self.new_turn_from_configuration(
            sub_id,
            session_configuration,
            /*final_output_json_schema*/ None,
            /*sandbox_policy_changed*/ false,
        )
        .await
    }

    async fn build_settings_update_items(
        &self,
        reference_context_item: Option<&TurnContextItem>,
        current_context: &TurnContext,
    ) -> Vec<ResponseItem> {
        // TODO: Make context updates a pure diff of persisted previous/current TurnContextItem
        // state so replay/backtracking is deterministic. Runtime inputs that affect model-visible
        // context (shell, exec policy, feature gates, previous-turn bridge) should be persisted
        // state or explicit non-state replay events.
        let previous_turn_settings = {
            let state = self.state.lock().await;
            state.previous_turn_settings()
        };
        let shell = self.user_shell();
        let exec_policy = self.services.exec_policy.current();
        let mut items = crate::context_manager::updates::build_settings_update_items(
            reference_context_item,
            previous_turn_settings.as_ref(),
            current_context,
            shell.as_ref(),
            exec_policy.as_ref(),
            self.features.enabled(Feature::Personality),
        );

        let subagents = self
            .services
            .agent_control
            .format_environment_context_subagents(self.conversation_id)
            .await;
        if !subagents.trim().is_empty() {
            if let Some(guardrail) = crate::context_manager::updates::build_developer_update_item(
                vec![
                    "Sub-agent coordination guardrail: active sub-agents are present in this session. \
                     Do not finalize as done without explicit reconciliation via `wait`, `send_input`, \
                     and/or `close_agent`. A wait timeout means pending, not completed. \
                     Be patient with long-running delegated operations, especially audits. \
                     Stop waiting only when there is clear evidence waiting longer is not useful \
                     (explicit failure/shutdown or repeated timeout with no progress signal). \
                     Never reproduce a delegated audit yourself; re-issue or redirect the audit task \
                     to a subagent and report that handoff/status."
                        .to_string(),
                ],
            ) {
                items.push(guardrail);
            }
            items.push(ResponseItem::from(
                crate::environment_context::EnvironmentContext::new(
                    /*cwd*/ None,
                    shell.as_ref().clone(),
                    /*current_date*/ None,
                    /*timezone*/ None,
                    /*network*/ None,
                    Some(subagents),
                ),
            ));
        }

        items
    }

    /// Persist the event to rollout and send it to clients.
    pub(crate) async fn send_event(&self, turn_context: &TurnContext, msg: EventMsg) {
        let legacy_source = msg.clone();
        let event = Event {
            id: turn_context.sub_id.clone(),
            msg,
        };
        self.send_event_raw(event).await;
        self.maybe_mirror_event_text_to_realtime(&legacy_source)
            .await;
        self.maybe_clear_realtime_handoff_for_event(&legacy_source)
            .await;

        let show_raw_agent_reasoning = self.show_raw_agent_reasoning();
        for legacy in legacy_source.as_legacy_events(show_raw_agent_reasoning) {
            let legacy_event = Event {
                id: turn_context.sub_id.clone(),
                msg: legacy,
            };
            self.send_event_raw(legacy_event).await;
        }
    }

    async fn maybe_mirror_event_text_to_realtime(&self, msg: &EventMsg) {
        let Some(text) = realtime_text_for_event(msg) else {
            return;
        };
        if self.conversation.running_state().await.is_none()
            || self.conversation.active_handoff_id().await.is_none()
        {
            return;
        }
        if let Err(err) = self.conversation.handoff_out(text).await {
            debug!("failed to mirror event text to realtime conversation: {err}");
        }
    }

    async fn maybe_clear_realtime_handoff_for_event(&self, msg: &EventMsg) {
        if !matches!(msg, EventMsg::TurnComplete(_)) {
            return;
        }
        if let Err(err) = self.conversation.handoff_complete().await {
            debug!("failed to finalize realtime handoff output: {err}");
        }
        self.conversation.clear_active_handoff().await;
    }

    pub(crate) async fn send_event_raw(&self, event: Event) {
        // Persist the event into rollout (recorder filters as needed)
        let rollout_items = vec![RolloutItem::EventMsg(event.msg.clone())];
        self.persist_rollout_items(&rollout_items).await;
        self.deliver_event_raw(event).await;
    }

    async fn deliver_event_raw(&self, event: Event) {
        // Record the last known agent status.
        if let Some(status) = agent_status_from_event(&event.msg) {
            self.agent_status.send_replace(status);
        }
        if let Err(e) = self.tx_event.send(event).await {
            debug!("dropping event because channel is closed: {e}");
        }
    }

    pub(crate) async fn emit_turn_item_started(&self, turn_context: &TurnContext, item: &TurnItem) {
        self.send_event(
            turn_context,
            EventMsg::ItemStarted(ItemStartedEvent {
                thread_id: self.conversation_id,
                turn_id: turn_context.sub_id.clone(),
                item: item.clone(),
            }),
        )
        .await;
    }

    pub(crate) async fn emit_turn_item_completed(
        &self,
        turn_context: &TurnContext,
        item: TurnItem,
    ) {
        let is_context_compaction = matches!(item, TurnItem::ContextCompaction(_));
        record_turn_ttfm_metric(turn_context, &item).await;
        self.send_event(
            turn_context,
            EventMsg::ItemCompleted(ItemCompletedEvent {
                thread_id: self.conversation_id,
                turn_id: turn_context.sub_id.clone(),
                item,
            }),
        )
        .await;
        if is_context_compaction {
            self.reset_nero_hook_msg_throttle();
        }
    }

    /// Adds an execpolicy amendment to both the in-memory and on-disk policies so future
    /// commands can use the newly approved prefix.
    pub(crate) async fn persist_execpolicy_amendment(
        &self,
        amendment: &ExecPolicyAmendment,
    ) -> Result<(), ExecPolicyUpdateError> {
        let codex_home = self
            .state
            .lock()
            .await
            .session_configuration
            .codex_home()
            .clone();

        self.services
            .exec_policy
            .append_amendment_and_update(&codex_home, amendment)
            .await?;

        Ok(())
    }

    pub(crate) async fn turn_context_for_sub_id(&self, sub_id: &str) -> Option<Arc<TurnContext>> {
        let active = self.active_turn.lock().await;
        active
            .as_ref()
            .and_then(|turn| turn.tasks.get(sub_id))
            .map(|task| Arc::clone(&task.turn_context))
    }

    async fn active_turn_context_and_cancellation_token(
        &self,
    ) -> Option<(Arc<TurnContext>, CancellationToken)> {
        let active = self.active_turn.lock().await;
        let (_, task) = active.as_ref()?.tasks.first()?;
        Some((
            Arc::clone(&task.turn_context),
            task.cancellation_token.child_token(),
        ))
    }

    pub(crate) async fn record_execpolicy_amendment_message(
        &self,
        sub_id: &str,
        amendment: &ExecPolicyAmendment,
    ) {
        let Some(prefixes) = format_allow_prefixes(vec![amendment.command.clone()]) else {
            warn!("execpolicy amendment for {sub_id} had no command prefix");
            return;
        };
        let text = format!("Approved command prefix saved:\n{prefixes}");
        let message: ResponseItem = DeveloperInstructions::new(text.clone()).into();

        if let Some(turn_context) = self.turn_context_for_sub_id(sub_id).await {
            self.record_conversation_items(&turn_context, std::slice::from_ref(&message))
                .await;
            return;
        }

        if self
            .inject_response_items(vec![ResponseInputItem::Message {
                role: "developer".to_string(),
                content: vec![ContentItem::InputText { text }],
            }])
            .await
            .is_err()
        {
            warn!("no active turn found to record execpolicy amendment message for {sub_id}");
        }
    }

    pub(crate) async fn persist_network_policy_amendment(
        &self,
        amendment: &NetworkPolicyAmendment,
        network_approval_context: &NetworkApprovalContext,
    ) -> anyhow::Result<()> {
        let host =
            Self::validated_network_policy_amendment_host(amendment, network_approval_context)?;
        let codex_home = self
            .state
            .lock()
            .await
            .session_configuration
            .codex_home()
            .clone();
        let execpolicy_amendment =
            execpolicy_network_rule_amendment(amendment, network_approval_context, &host);

        if let Some(started_network_proxy) = self.services.network_proxy.as_ref() {
            let proxy = started_network_proxy.proxy();
            match amendment.action {
                NetworkPolicyRuleAction::Allow => proxy
                    .add_allowed_domain(&host)
                    .await
                    .map_err(|err| anyhow::anyhow!("failed to update runtime allowlist: {err}"))?,
                NetworkPolicyRuleAction::Deny => proxy
                    .add_denied_domain(&host)
                    .await
                    .map_err(|err| anyhow::anyhow!("failed to update runtime denylist: {err}"))?,
            }
        }

        self.services
            .exec_policy
            .append_network_rule_and_update(
                &codex_home,
                &host,
                execpolicy_amendment.protocol,
                execpolicy_amendment.decision,
                Some(execpolicy_amendment.justification),
            )
            .await
            .map_err(|err| {
                anyhow::anyhow!("failed to persist network policy amendment to execpolicy: {err}")
            })?;

        Ok(())
    }

    fn validated_network_policy_amendment_host(
        amendment: &NetworkPolicyAmendment,
        network_approval_context: &NetworkApprovalContext,
    ) -> anyhow::Result<String> {
        let approved_host = normalize_host(&network_approval_context.host);
        let amendment_host = normalize_host(&amendment.host);
        if amendment_host != approved_host {
            return Err(anyhow::anyhow!(
                "network policy amendment host '{}' does not match approved host '{}'",
                amendment.host,
                network_approval_context.host
            ));
        }
        Ok(approved_host)
    }

    pub(crate) async fn record_network_policy_amendment_message(
        &self,
        sub_id: &str,
        amendment: &NetworkPolicyAmendment,
    ) {
        let (action, list_name) = match amendment.action {
            NetworkPolicyRuleAction::Allow => ("Allowed", "allowlist"),
            NetworkPolicyRuleAction::Deny => ("Denied", "denylist"),
        };
        let text = format!(
            "{action} network rule saved in execpolicy ({list_name}): {}",
            amendment.host
        );
        let message: ResponseItem = DeveloperInstructions::new(text.clone()).into();

        if let Some(turn_context) = self.turn_context_for_sub_id(sub_id).await {
            self.record_conversation_items(&turn_context, std::slice::from_ref(&message))
                .await;
            return;
        }

        if self
            .inject_response_items(vec![ResponseInputItem::Message {
                role: "developer".to_string(),
                content: vec![ContentItem::InputText { text }],
            }])
            .await
            .is_err()
        {
            warn!("no active turn found to record network policy amendment message for {sub_id}");
        }
    }

    /// Emit an exec approval request event and await the user's decision.
    ///
    /// The request is keyed by `call_id` + `approval_id` so matching responses
    /// are delivered to the correct in-flight turn. If the pending approval is
    /// cleared before a response arrives, treat it as an abort so interrupted
    /// turns do not continue on a synthetic denial.
    ///
    /// Note that if `available_decisions` is `None`, then the other fields will
    /// be used to derive the available decisions via
    /// [ExecApprovalRequestEvent::default_available_decisions].
    #[allow(clippy::too_many_arguments)]
    pub async fn request_command_approval(
        &self,
        turn_context: &TurnContext,
        call_id: String,
        approval_id: Option<String>,
        command: Vec<String>,
        cwd: PathBuf,
        reason: Option<String>,
        network_approval_context: Option<NetworkApprovalContext>,
        proposed_execpolicy_amendment: Option<ExecPolicyAmendment>,
        additional_permissions: Option<PermissionProfile>,
        available_decisions: Option<Vec<ReviewDecision>>,
    ) -> ReviewDecision {
        //  command-level approvals use `call_id`.
        // `approval_id` is only present for subcommand callbacks (execve intercept)
        let effective_approval_id = approval_id.clone().unwrap_or_else(|| call_id.clone());
        // Add the tx_approve callback to the map before sending the request.
        let (tx_approve, rx_approve) = oneshot::channel();
        let prev_entry = {
            let mut active = self.active_turn.lock().await;
            match active.as_mut() {
                Some(at) => {
                    let mut ts = at.turn_state.lock().await;
                    ts.insert_pending_approval(effective_approval_id.clone(), tx_approve)
                }
                None => None,
            }
        };
        if prev_entry.is_some() {
            warn!("Overwriting existing pending approval for call_id: {effective_approval_id}");
        }

        let parsed_cmd = parse_command(&command);
        let proposed_network_policy_amendments = network_approval_context.as_ref().map(|context| {
            vec![
                NetworkPolicyAmendment {
                    host: context.host.clone(),
                    action: NetworkPolicyRuleAction::Allow,
                },
                NetworkPolicyAmendment {
                    host: context.host.clone(),
                    action: NetworkPolicyRuleAction::Deny,
                },
            ]
        });
        let available_decisions = available_decisions.unwrap_or_else(|| {
            ExecApprovalRequestEvent::default_available_decisions(
                network_approval_context.as_ref(),
                proposed_execpolicy_amendment.as_ref(),
                proposed_network_policy_amendments.as_deref(),
                additional_permissions.as_ref(),
            )
        });
        let event = EventMsg::ExecApprovalRequest(ExecApprovalRequestEvent {
            call_id,
            approval_id,
            turn_id: turn_context.sub_id.clone(),
            command,
            cwd,
            reason,
            network_approval_context,
            proposed_execpolicy_amendment,
            proposed_network_policy_amendments,
            additional_permissions,
            available_decisions: Some(available_decisions),
            parsed_cmd,
        });
        self.send_event(turn_context, event).await;
        rx_approve.await.unwrap_or(ReviewDecision::Abort)
    }

    pub async fn request_patch_approval(
        &self,
        turn_context: &TurnContext,
        call_id: String,
        changes: HashMap<PathBuf, FileChange>,
        reason: Option<String>,
        grant_root: Option<PathBuf>,
    ) -> oneshot::Receiver<ReviewDecision> {
        // Add the tx_approve callback to the map before sending the request.
        let (tx_approve, rx_approve) = oneshot::channel();
        let approval_id = call_id.clone();
        let prev_entry = {
            let mut active = self.active_turn.lock().await;
            match active.as_mut() {
                Some(at) => {
                    let mut ts = at.turn_state.lock().await;
                    ts.insert_pending_approval(approval_id.clone(), tx_approve)
                }
                None => None,
            }
        };
        if prev_entry.is_some() {
            warn!("Overwriting existing pending approval for call_id: {approval_id}");
        }

        let event = EventMsg::ApplyPatchApprovalRequest(ApplyPatchApprovalRequestEvent {
            call_id,
            turn_id: turn_context.sub_id.clone(),
            changes,
            reason,
            grant_root,
        });
        self.send_event(turn_context, event).await;
        rx_approve
    }

    pub async fn request_permissions(
        &self,
        turn_context: &TurnContext,
        call_id: String,
        args: RequestPermissionsArgs,
    ) -> Option<RequestPermissionsResponse> {
        match turn_context.approval_policy.value() {
            AskForApproval::Never => {
                return Some(RequestPermissionsResponse {
                    permissions: RequestPermissionProfile::default(),
                    scope: PermissionGrantScope::Turn,
                });
            }
            AskForApproval::Granular(granular_config)
                if !granular_config.allows_request_permissions() =>
            {
                return Some(RequestPermissionsResponse {
                    permissions: RequestPermissionProfile::default(),
                    scope: PermissionGrantScope::Turn,
                });
            }
            AskForApproval::OnFailure
            | AskForApproval::OnRequest
            | AskForApproval::UnlessTrusted
            | AskForApproval::Granular(_) => {}
        }

        let (tx_response, rx_response) = oneshot::channel();
        let prev_entry = {
            let mut active = self.active_turn.lock().await;
            match active.as_mut() {
                Some(at) => {
                    let mut ts = at.turn_state.lock().await;
                    ts.insert_pending_request_permissions(call_id.clone(), tx_response)
                }
                None => None,
            }
        };
        if prev_entry.is_some() {
            warn!("Overwriting existing pending request_permissions for call_id: {call_id}");
        }

        // TODO(ccunningham): Support auto-review for request_permissions /
        // with_additional_permissions. V0 still routes this surface through
        // the existing manual RequestPermissions event flow.
        let event = EventMsg::RequestPermissions(RequestPermissionsEvent {
            call_id,
            turn_id: turn_context.sub_id.clone(),
            reason: args.reason,
            permissions: args.permissions,
        });
        self.send_event(turn_context, event).await;
        rx_response.await.ok()
    }

    pub async fn request_user_input(
        &self,
        turn_context: &TurnContext,
        call_id: String,
        args: RequestUserInputArgs,
    ) -> Option<RequestUserInputResponse> {
        let sub_id = turn_context.sub_id.clone();
        let (tx_response, rx_response) = oneshot::channel();
        let event_id = sub_id.clone();
        let prev_entry = {
            let mut active = self.active_turn.lock().await;
            match active.as_mut() {
                Some(at) => {
                    let mut ts = at.turn_state.lock().await;
                    ts.insert_pending_user_input(sub_id, tx_response)
                }
                None => None,
            }
        };
        if prev_entry.is_some() {
            warn!("Overwriting existing pending user input for sub_id: {event_id}");
        }

        let event = EventMsg::RequestUserInput(RequestUserInputEvent {
            call_id,
            turn_id: turn_context.sub_id.clone(),
            questions: args.questions,
        });
        self.send_event(turn_context, event).await;
        rx_response.await.ok()
    }

    pub async fn request_mcp_server_elicitation(
        &self,
        turn_context: &TurnContext,
        request_id: RequestId,
        params: McpServerElicitationRequestParams,
    ) -> Option<ElicitationResponse> {
        let server_name = params.server_name.clone();
        let request = match params.request {
            McpServerElicitationRequest::Form {
                meta,
                message,
                requested_schema,
            } => {
                let requested_schema = match serde_json::to_value(requested_schema) {
                    Ok(requested_schema) => requested_schema,
                    Err(err) => {
                        warn!(
                            "failed to serialize MCP elicitation schema for server_name: {server_name}, request_id: {request_id}: {err:#}"
                        );
                        return None;
                    }
                };
                codex_protocol::approvals::ElicitationRequest::Form {
                    meta,
                    message,
                    requested_schema,
                }
            }
            McpServerElicitationRequest::Url {
                meta,
                message,
                url,
                elicitation_id,
            } => codex_protocol::approvals::ElicitationRequest::Url {
                meta,
                message,
                url,
                elicitation_id,
            },
        };

        let (tx_response, rx_response) = oneshot::channel();
        let prev_entry = {
            let mut active = self.active_turn.lock().await;
            match active.as_mut() {
                Some(at) => {
                    let mut ts = at.turn_state.lock().await;
                    ts.insert_pending_elicitation(
                        server_name.clone(),
                        request_id.clone(),
                        tx_response,
                    )
                }
                None => None,
            }
        };
        if prev_entry.is_some() {
            warn!(
                "Overwriting existing pending elicitation for server_name: {server_name}, request_id: {request_id}"
            );
        }
        let id = match request_id {
            rmcp::model::NumberOrString::String(value) => {
                codex_protocol::mcp::RequestId::String(value.to_string())
            }
            rmcp::model::NumberOrString::Number(value) => {
                codex_protocol::mcp::RequestId::Integer(value)
            }
        };
        let event = EventMsg::ElicitationRequest(ElicitationRequestEvent {
            turn_id: params.turn_id,
            server_name,
            id,
            request,
        });
        self.send_event(turn_context, event).await;
        rx_response.await.ok()
    }

    pub async fn notify_user_input_response(
        &self,
        sub_id: &str,
        response: RequestUserInputResponse,
    ) {
        let entry = {
            let mut active = self.active_turn.lock().await;
            match active.as_mut() {
                Some(at) => {
                    let mut ts = at.turn_state.lock().await;
                    ts.remove_pending_user_input(sub_id)
                }
                None => None,
            }
        };
        match entry {
            Some(tx_response) => {
                tx_response.send(response).ok();
            }
            None => {
                warn!("No pending user input found for sub_id: {sub_id}");
            }
        }
    }

    pub async fn notify_request_permissions_response(
        &self,
        call_id: &str,
        response: RequestPermissionsResponse,
    ) {
        let mut granted_for_session = None;
        let entry = {
            let mut active = self.active_turn.lock().await;
            match active.as_mut() {
                Some(at) => {
                    let mut ts = at.turn_state.lock().await;
                    let entry = ts.remove_pending_request_permissions(call_id);
                    if entry.is_some() && !response.permissions.is_empty() {
                        match response.scope {
                            PermissionGrantScope::Turn => {
                                ts.record_granted_permissions(response.permissions.clone().into());
                            }
                            PermissionGrantScope::Session => {
                                granted_for_session = Some(response.permissions.clone());
                            }
                        }
                    }
                    entry
                }
                None => None,
            }
        };
        if let Some(permissions) = granted_for_session {
            let mut state = self.state.lock().await;
            state.record_granted_permissions(permissions.into());
        }
        match entry {
            Some(tx_response) => {
                tx_response.send(response).ok();
            }
            None => {
                warn!("No pending request_permissions found for call_id: {call_id}");
            }
        }
    }

    pub(crate) async fn granted_turn_permissions(&self) -> Option<PermissionProfile> {
        let active = self.active_turn.lock().await;
        let active = active.as_ref()?;
        let ts = active.turn_state.lock().await;
        ts.granted_permissions()
    }

    pub(crate) async fn granted_session_permissions(&self) -> Option<PermissionProfile> {
        let state = self.state.lock().await;
        state.granted_permissions()
    }

    pub async fn notify_dynamic_tool_response(&self, call_id: &str, response: DynamicToolResponse) {
        let entry = {
            let mut active = self.active_turn.lock().await;
            match active.as_mut() {
                Some(at) => {
                    let mut ts = at.turn_state.lock().await;
                    ts.remove_pending_dynamic_tool(call_id)
                }
                None => None,
            }
        };
        match entry {
            Some(tx_response) => {
                tx_response.send(response).ok();
            }
            None => {
                warn!("No pending dynamic tool call found for call_id: {call_id}");
            }
        }
    }

    pub async fn notify_approval(&self, approval_id: &str, decision: ReviewDecision) {
        let entry = {
            let mut active = self.active_turn.lock().await;
            match active.as_mut() {
                Some(at) => {
                    let mut ts = at.turn_state.lock().await;
                    ts.remove_pending_approval(approval_id)
                }
                None => None,
            }
        };
        match entry {
            Some(tx_approve) => {
                tx_approve.send(decision).ok();
            }
            None => {
                warn!("No pending approval found for call_id: {approval_id}");
            }
        }
    }

    pub async fn resolve_elicitation(
        &self,
        server_name: String,
        id: RequestId,
        response: ElicitationResponse,
    ) -> anyhow::Result<()> {
        let entry = {
            let mut active = self.active_turn.lock().await;
            match active.as_mut() {
                Some(at) => {
                    let mut ts = at.turn_state.lock().await;
                    ts.remove_pending_elicitation(&server_name, &id)
                }
                None => None,
            }
        };
        if let Some(tx_response) = entry {
            tx_response
                .send(response)
                .map_err(|e| anyhow::anyhow!("failed to send elicitation response: {e:?}"))?;
            return Ok(());
        }

        self.services
            .mcp_connection_manager
            .read()
            .await
            .resolve_elicitation(server_name, id, response)
            .await
    }

    /// Records input items: always append to conversation history and
    /// persist these response items to rollout.
    pub(crate) async fn record_conversation_items(
        &self,
        turn_context: &TurnContext,
        items: &[ResponseItem],
    ) {
        self.record_into_history(items, turn_context).await;
        self.persist_rollout_response_items(items).await;
        self.send_raw_response_items(turn_context, items).await;
    }

    /// Append ResponseItems to the in-memory conversation history only.
    pub(crate) async fn record_into_history(
        &self,
        items: &[ResponseItem],
        turn_context: &TurnContext,
    ) {
        let mut state = self.state.lock().await;
        state.record_items(items.iter(), turn_context.truncation_policy);
    }

    pub(crate) async fn record_model_warning(&self, message: impl Into<String>, ctx: &TurnContext) {
        self.services
            .session_telemetry
            .counter("codex.model_warning", /*inc*/ 1, &[]);
        let item = ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: format!("Warning: {}", message.into()),
            }],
            end_turn: None,
            phase: None,
        };

        self.record_conversation_items(ctx, &[item]).await;
    }

    async fn maybe_warn_on_server_model_mismatch(
        self: &Arc<Self>,
        turn_context: &Arc<TurnContext>,
        server_model: String,
    ) -> bool {
        let requested_model = turn_context.model_info.slug.clone();
        let server_model_normalized = server_model.to_ascii_lowercase();
        let requested_model_normalized = requested_model.to_ascii_lowercase();
        if server_model_normalized == requested_model_normalized {
            info!("server reported model {server_model} (matches requested model)");
            return false;
        }

        warn!("server reported model {server_model} while requested model was {requested_model}");

        let warning_message = format!(
            "Your account was flagged for potentially high-risk cyber activity and this request was routed to gpt-5.2 as a fallback. To regain access to gpt-5.3-codex, apply for trusted access: {CYBER_VERIFY_URL} or learn more: {CYBER_SAFETY_URL}"
        );

        self.send_event(
            turn_context,
            EventMsg::ModelReroute(ModelRerouteEvent {
                from_model: requested_model.clone(),
                to_model: server_model.clone(),
                reason: ModelRerouteReason::HighRiskCyberActivity,
            }),
        )
        .await;

        self.send_event(
            turn_context,
            EventMsg::Warning(WarningEvent {
                message: warning_message.clone(),
            }),
        )
        .await;
        self.record_model_warning(warning_message, turn_context)
            .await;
        true
    }

    pub(crate) async fn replace_history(
        &self,
        items: Vec<ResponseItem>,
        reference_context_item: Option<TurnContextItem>,
    ) {
        let mut state = self.state.lock().await;
        state.replace_history(items, reference_context_item);
    }

    pub(crate) async fn replace_compacted_history(
        &self,
        items: Vec<ResponseItem>,
        reference_context_item: Option<TurnContextItem>,
        compacted_item: CompactedItem,
    ) {
        self.replace_history(items, reference_context_item.clone())
            .await;

        self.persist_rollout_items(&[RolloutItem::Compacted(compacted_item)])
            .await;
        if let Some(turn_context_item) = reference_context_item {
            self.persist_rollout_items(&[RolloutItem::TurnContext(turn_context_item)])
                .await;
        }
    }

    async fn persist_rollout_response_items(&self, items: &[ResponseItem]) {
        let rollout_items: Vec<RolloutItem> = items
            .iter()
            .cloned()
            .map(RolloutItem::ResponseItem)
            .collect();
        self.persist_rollout_items(&rollout_items).await;
    }

    pub fn enabled(&self, feature: Feature) -> bool {
        self.features.enabled(feature)
    }

    pub(crate) fn features(&self) -> ManagedFeatures {
        self.features.clone()
    }

    pub(crate) async fn collaboration_mode(&self) -> CollaborationMode {
        let state = self.state.lock().await;
        state.session_configuration.collaboration_mode.clone()
    }

    async fn send_raw_response_items(&self, turn_context: &TurnContext, items: &[ResponseItem]) {
        for item in items {
            self.send_event(
                turn_context,
                EventMsg::RawResponseItem(RawResponseItemEvent { item: item.clone() }),
            )
            .await;
        }
    }

    pub(crate) async fn build_initial_context(
        &self,
        turn_context: &TurnContext,
    ) -> Vec<ResponseItem> {
        let mut developer_sections = Vec::<String>::with_capacity(8);
        let mut contextual_user_sections = Vec::<String>::with_capacity(2);
        let shell = self.user_shell();
        let (
            reference_context_item,
            previous_turn_settings,
            collaboration_mode,
            base_instructions,
            session_source,
        ) = {
            let state = self.state.lock().await;
            (
                state.reference_context_item(),
                state.previous_turn_settings(),
                state.session_configuration.collaboration_mode.clone(),
                state.session_configuration.base_instructions.clone(),
                state.session_configuration.session_source.clone(),
            )
        };
        if let Some(model_switch_message) =
            crate::context_manager::updates::build_model_instructions_update_item(
                previous_turn_settings.as_ref(),
                turn_context,
            )
        {
            developer_sections.push(model_switch_message.into_text());
        }
        developer_sections.push(
            DeveloperInstructions::from_policy(
                turn_context.sandbox_policy.get(),
                turn_context.approval_policy.value(),
                turn_context.config.approvals_reviewer,
                self.services.exec_policy.current().as_ref(),
                &turn_context.cwd,
                turn_context
                    .features
                    .enabled(Feature::ExecPermissionApprovals),
                turn_context
                    .features
                    .enabled(Feature::RequestPermissionsTool),
            )
            .into_text(),
        );
        let separate_guardian_developer_message =
            crate::guardian::is_guardian_reviewer_source(&session_source);
        // Keep the guardian policy prompt out of the aggregated developer bundle so it
        // stays isolated as its own top-level developer message for guardian subagents.
        if !separate_guardian_developer_message
            && let Some(developer_instructions) = turn_context.developer_instructions.as_deref()
        {
            developer_sections.push(developer_instructions.to_string());
        }
        // Add developer instructions for memories.
        if turn_context.features.enabled(Feature::MemoryTool)
            && turn_context.config.memories.use_memories
            && let Some(memory_prompt) =
                build_memory_tool_developer_instructions(&turn_context.config.codex_home).await
        {
            developer_sections.push(memory_prompt);
        }
        // Add developer instructions from collaboration_mode if they exist and are non-empty
        if let Some(collab_instructions) =
            DeveloperInstructions::from_collaboration_mode(&collaboration_mode)
        {
            developer_sections.push(collab_instructions.into_text());
        }
        if let Some(realtime_update) = crate::context_manager::updates::build_initial_realtime_item(
            reference_context_item.as_ref(),
            previous_turn_settings.as_ref(),
            turn_context,
        ) {
            developer_sections.push(realtime_update.into_text());
        }
        if self.features.enabled(Feature::Personality)
            && let Some(personality) = turn_context.personality
        {
            let model_info = turn_context.model_info.clone();
            let has_baked_personality = model_info.supports_personality()
                && base_instructions == model_info.get_model_instructions(Some(personality));
            if !has_baked_personality
                && let Some(personality_message) =
                    crate::context_manager::updates::personality_message_for(
                        &model_info,
                        personality,
                    )
            {
                developer_sections.push(
                    DeveloperInstructions::personality_spec_message(personality_message)
                        .into_text(),
                );
            }
        }
        if turn_context.apps_enabled() {
            let mcp_connection_manager = self.services.mcp_connection_manager.read().await;
            let accessible_and_enabled_connectors =
                connectors::list_accessible_and_enabled_connectors_from_manager(
                    &mcp_connection_manager,
                    &turn_context.config,
                )
                .await;
            if let Some(apps_section) = render_apps_section(&accessible_and_enabled_connectors) {
                developer_sections.push(apps_section);
            }
        }
        let implicit_skills = turn_context
            .turn_skills
            .outcome
            .allowed_skills_for_implicit_invocation();
        if let Some(skills_section) = render_skills_section(&implicit_skills) {
            developer_sections.push(skills_section);
        }
        let loaded_plugins = self
            .services
            .plugins_manager
            .plugins_for_config(&turn_context.config);
        if let Some(plugin_section) = render_plugins_section(loaded_plugins.capability_summaries())
        {
            developer_sections.push(plugin_section);
        }
        if turn_context.features.enabled(Feature::CodexGitCommit)
            && let Some(commit_message_instruction) = commit_message_trailer_instruction(
                turn_context.config.commit_attribution.as_deref(),
            )
        {
            developer_sections.push(commit_message_instruction);
        }
        if let Some(user_instructions) = turn_context.user_instructions.as_deref() {
            contextual_user_sections.push(
                UserInstructions {
                    text: user_instructions.to_string(),
                    directory: turn_context.cwd.to_string_lossy().into_owned(),
                }
                .serialize_to_text(),
            );
        }
        let subagents = self
            .services
            .agent_control
            .format_environment_context_subagents(self.conversation_id)
            .await;
        if !subagents.trim().is_empty() {
            developer_sections.push(format!(
                "Sub-agent coordination guardrail:\n\
                 Active sub-agents in this session:\n{subagents}\n\
                 Before ending this turn, explicitly reconcile their state via `wait`, `send_input`, \
                 and/or `close_agent`.\n\
                 A timed-out wait is not completion; treat it as pending work and either wait longer \
                 or report unresolved agents explicitly.\n\
                 Be patient with long-running delegated operations, especially audits.\n\
                 Stop waiting only when there is clear evidence waiting longer is not useful \
                 (explicit failure/shutdown or repeated timeout with no progress signal).\n\
                 Never reproduce a delegated audit yourself; re-issue or redirect the audit task \
                 to a subagent and report that handoff/status."
            ));
        }
        contextual_user_sections.push(
            EnvironmentContext::from_turn_context(turn_context, shell.as_ref())
                .with_subagents(subagents)
                .serialize_to_xml(),
        );

        let mut items = Vec::with_capacity(3);
        if let Some(developer_message) =
            crate::context_manager::updates::build_developer_update_item(developer_sections)
        {
            items.push(developer_message);
        }
        if let Some(contextual_user_message) =
            crate::context_manager::updates::build_contextual_user_message(contextual_user_sections)
        {
            items.push(contextual_user_message);
        }
        // Emit the guardian policy prompt as a separate developer item so the guardian
        // subagent sees a distinct, easy-to-audit instruction block.
        if separate_guardian_developer_message
            && let Some(developer_instructions) = turn_context.developer_instructions.as_deref()
            && let Some(guardian_developer_message) =
                crate::context_manager::updates::build_developer_update_item(vec![
                    developer_instructions.to_string(),
                ])
        {
            items.push(guardian_developer_message);
        }
        items
    }

    pub(crate) async fn persist_rollout_items(&self, items: &[RolloutItem]) {
        let contains_compaction = items
            .iter()
            .any(|item| matches!(item, RolloutItem::Compacted(_)));
        let recorder = {
            let guard = self.services.rollout.lock().await;
            guard.clone()
        };
        if let Some(rec) = recorder
            && let Err(e) = rec.record_items(items).await
        {
            error!("failed to record rollout items: {e:#}");
        }
        if contains_compaction {
            self.reset_nero_hook_msg_throttle();
        }
    }

    pub(crate) async fn clone_history(&self) -> ContextManager {
        let state = self.state.lock().await;
        state.clone_history()
    }

    pub(crate) async fn reference_context_item(&self) -> Option<TurnContextItem> {
        let state = self.state.lock().await;
        state.reference_context_item()
    }

    /// Persist the latest turn context snapshot for the first real user turn and for
    /// steady-state turns that emit model-visible context updates.
    ///
    /// When the reference snapshot is missing, this injects full initial context. Otherwise, it
    /// emits only settings diff items.
    ///
    /// If full context is injected and a model switch occurred, this prepends the
    /// `<model_switch>` developer message so model-specific instructions are not lost.
    ///
    /// This is the normal runtime path that establishes a new `reference_context_item`.
    /// Mid-turn compaction is the other path that can re-establish that baseline when it
    /// reinjects full initial context into replacement history. Other non-regular tasks
    /// intentionally do not update the baseline.
    pub(crate) async fn record_context_updates_and_set_reference_context_item(
        &self,
        turn_context: &TurnContext,
    ) {
        let reference_context_item = {
            let state = self.state.lock().await;
            state.reference_context_item()
        };
        let should_inject_full_context = reference_context_item.is_none();
        let context_items = if should_inject_full_context {
            self.build_initial_context(turn_context).await
        } else {
            // Steady-state path: append only context diffs to minimize token overhead.
            self.build_settings_update_items(reference_context_item.as_ref(), turn_context)
                .await
        };
        let turn_context_item = turn_context.to_turn_context_item();
        if !context_items.is_empty() {
            self.record_conversation_items(turn_context, &context_items)
                .await;
        }
        // Persist one `TurnContextItem` per real user turn so resume/lazy replay can recover the
        // latest durable baseline even when this turn emitted no model-visible context diffs.
        self.persist_rollout_items(&[RolloutItem::TurnContext(turn_context_item.clone())])
            .await;

        // Advance the in-memory diff baseline even when this turn emitted no model-visible
        // context items. This keeps later runtime diffing aligned with the current turn state.
        let mut state = self.state.lock().await;
        state.set_reference_context_item(Some(turn_context_item));
    }

    pub(crate) async fn update_token_usage_info(
        &self,
        turn_context: &TurnContext,
        token_usage: Option<&TokenUsage>,
    ) {
        if let Some(token_usage) = token_usage {
            let mut state = self.state.lock().await;
            state.update_token_info_from_usage(token_usage, turn_context.model_context_window());
        }
        self.send_token_count_event(turn_context).await;
    }

    pub(crate) async fn recompute_token_usage(&self, turn_context: &TurnContext) {
        let history = self.clone_history().await;
        let base_instructions = self.get_base_instructions().await;
        let Some(estimated_total_tokens) =
            history.estimate_token_count_with_base_instructions(&base_instructions)
        else {
            return;
        };
        {
            let mut state = self.state.lock().await;
            let mut info = state.token_info().unwrap_or(TokenUsageInfo {
                total_token_usage: TokenUsage::default(),
                last_token_usage: TokenUsage::default(),
                model_context_window: None,
            });

            info.last_token_usage = TokenUsage {
                input_tokens: 0,
                cached_input_tokens: 0,
                output_tokens: 0,
                reasoning_output_tokens: 0,
                total_tokens: estimated_total_tokens.max(0),
            };

            if let Some(model_context_window) = turn_context.model_context_window() {
                info.model_context_window = Some(model_context_window);
            }

            state.set_token_info(Some(info));
        }
        self.send_token_count_event(turn_context).await;
    }

    pub(crate) async fn update_rate_limits(
        &self,
        turn_context: &TurnContext,
        new_rate_limits: RateLimitSnapshot,
    ) {
        {
            let mut state = self.state.lock().await;
            state.set_rate_limits(new_rate_limits);
        }
        self.send_token_count_event(turn_context).await;
    }

    pub(crate) async fn mcp_dependency_prompted(&self) -> HashSet<String> {
        let state = self.state.lock().await;
        state.mcp_dependency_prompted()
    }

    pub(crate) async fn record_mcp_dependency_prompted<I>(&self, names: I)
    where
        I: IntoIterator<Item = String>,
    {
        let mut state = self.state.lock().await;
        state.record_mcp_dependency_prompted(names);
    }

    pub async fn dependency_env(&self) -> HashMap<String, String> {
        let state = self.state.lock().await;
        state.dependency_env()
    }

    pub async fn set_dependency_env(&self, values: HashMap<String, String>) {
        let mut state = self.state.lock().await;
        state.set_dependency_env(values);
    }

    pub(crate) async fn set_server_reasoning_included(&self, included: bool) {
        let mut state = self.state.lock().await;
        state.set_server_reasoning_included(included);
    }

    async fn send_token_count_event(&self, turn_context: &TurnContext) {
        let (info, rate_limits) = {
            let state = self.state.lock().await;
            state.token_info_and_rate_limits()
        };
        let event = EventMsg::TokenCount(TokenCountEvent { info, rate_limits });
        self.send_event(turn_context, event).await;
    }

    pub(crate) async fn set_total_tokens_full(&self, turn_context: &TurnContext) {
        if let Some(context_window) = turn_context.model_context_window() {
            let mut state = self.state.lock().await;
            state.set_token_usage_full(context_window);
        }
        self.send_token_count_event(turn_context).await;
    }

    pub(crate) async fn record_response_item_and_emit_turn_item(
        &self,
        turn_context: &TurnContext,
        response_item: ResponseItem,
    ) {
        // Add to conversation history and persist response item to rollout.
        self.record_conversation_items(turn_context, std::slice::from_ref(&response_item))
            .await;

        // Derive a turn item and emit lifecycle events if applicable.
        if let Some(item) = parse_turn_item(&response_item) {
            self.emit_turn_item_started(turn_context, &item).await;
            self.emit_turn_item_completed(turn_context, item).await;
        }
    }

    pub(crate) async fn record_user_prompt_and_emit_turn_item(
        &self,
        turn_context: &TurnContext,
        input: &[UserInput],
        response_item: ResponseItem,
    ) {
        // Persist the user message to history, but emit the turn item from `UserInput` so
        // UI-only `text_elements` are preserved. `ResponseItem::Message` does not carry
        // those spans, and `record_response_item_and_emit_turn_item` would drop them.
        self.record_conversation_items(turn_context, std::slice::from_ref(&response_item))
            .await;
        let turn_item = TurnItem::UserMessage(UserMessageItem::new(input));
        self.emit_turn_item_started(turn_context, &turn_item).await;
        self.emit_turn_item_completed(turn_context, turn_item).await;
        self.ensure_rollout_materialized().await;
    }

    pub(crate) async fn notify_background_event(
        &self,
        turn_context: &TurnContext,
        message: impl Into<String>,
    ) {
        let event = EventMsg::BackgroundEvent(BackgroundEventEvent {
            message: message.into(),
        });
        self.send_event(turn_context, event).await;
    }

    pub(crate) async fn notify_stream_error(
        &self,
        turn_context: &TurnContext,
        message: impl Into<String>,
        codex_error: CodexErr,
    ) {
        let additional_details = codex_error.to_string();
        let codex_error_info = CodexErrorInfo::ResponseStreamDisconnected {
            http_status_code: codex_error.http_status_code_value(),
        };
        let event = EventMsg::StreamError(StreamErrorEvent {
            message: message.into(),
            codex_error_info: Some(codex_error_info),
            additional_details: Some(additional_details),
        });
        self.send_event(turn_context, event).await;
    }

    async fn maybe_start_ghost_snapshot(
        self: &Arc<Self>,
        turn_context: Arc<TurnContext>,
        cancellation_token: CancellationToken,
    ) {
        if !self.enabled(Feature::GhostCommit) {
            return;
        }
        let token = match turn_context.tool_call_gate.subscribe().await {
            Ok(token) => token,
            Err(err) => {
                warn!("failed to subscribe to ghost snapshot readiness: {err}");
                return;
            }
        };

        info!("spawning ghost snapshot task");
        let task = GhostSnapshotTask::new(token);
        Arc::new(task)
            .run(
                Arc::new(SessionTaskContext::new(self.clone())),
                turn_context.clone(),
                Vec::new(),
                cancellation_token,
            )
            .await;
    }

    /// Inject additional user input into the currently active turn.
    ///
    /// Returns the active turn id when accepted.
    pub async fn steer_input(
        &self,
        input: Vec<UserInput>,
        expected_turn_id: Option<&str>,
    ) -> Result<String, SteerInputError> {
        if input.is_empty() {
            return Err(SteerInputError::EmptyInput);
        }

        let mut active = self.active_turn.lock().await;
        let Some(active_turn) = active.as_mut() else {
            return Err(SteerInputError::NoActiveTurn(input));
        };

        let Some((active_turn_id, _)) = active_turn.tasks.first() else {
            return Err(SteerInputError::NoActiveTurn(input));
        };

        if let Some(expected_turn_id) = expected_turn_id
            && expected_turn_id != active_turn_id
        {
            return Err(SteerInputError::ExpectedTurnMismatch {
                expected: expected_turn_id.to_string(),
                actual: active_turn_id.clone(),
            });
        }

        match active_turn.tasks.first().map(|(_, task)| task.kind) {
            Some(crate::state::TaskKind::Regular) => {}
            Some(crate::state::TaskKind::Review) => {
                return Err(SteerInputError::ActiveTurnNotSteerable {
                    turn_kind: NonSteerableTurnKind::Review,
                });
            }
            Some(crate::state::TaskKind::Compact) => {
                return Err(SteerInputError::ActiveTurnNotSteerable {
                    turn_kind: NonSteerableTurnKind::Compact,
                });
            }
            None => return Err(SteerInputError::NoActiveTurn(input)),
        }

        let mut turn_state = active_turn.turn_state.lock().await;
        turn_state.push_pending_input(input.into());
        Ok(active_turn_id.clone())
    }

    /// Returns the input if there was no task running to inject into.
    pub async fn inject_response_items(
        &self,
        input: Vec<ResponseInputItem>,
    ) -> Result<(), Vec<ResponseInputItem>> {
        let mut active = self.active_turn.lock().await;
        match active.as_mut() {
            Some(at) => {
                let mut ts = at.turn_state.lock().await;
                for item in input {
                    ts.push_pending_input(item);
                }
                Ok(())
            }
            None => Err(input),
        }
    }

    pub(crate) fn subscribe_mailbox_seq(&self) -> watch::Receiver<u64> {
        self.mailbox.subscribe()
    }

    pub(crate) fn enqueue_mailbox_communication(&self, communication: InterAgentCommunication) {
        self.mailbox.send(communication);
    }

    pub(crate) async fn has_trigger_turn_mailbox_items(&self) -> bool {
        self.mailbox_rx.lock().await.has_pending_trigger_turn()
    }

    pub async fn prepend_pending_input(&self, input: Vec<ResponseInputItem>) -> Result<(), ()> {
        let mut active = self.active_turn.lock().await;
        match active.as_mut() {
            Some(at) => {
                let mut ts = at.turn_state.lock().await;
                ts.prepend_pending_input(input);
                Ok(())
            }
            None => Err(()),
        }
    }

    pub async fn get_pending_input(&self) -> Vec<ResponseInputItem> {
        let pending_input = {
            let mut active = self.active_turn.lock().await;
            match active.as_mut() {
                Some(at) => {
                    let mut ts = at.turn_state.lock().await;
                    ts.take_pending_input()
                }
                None => Vec::new(),
            }
        };
        let mailbox_items = {
            let mut mailbox_rx = self.mailbox_rx.lock().await;
            mailbox_rx
                .drain()
                .into_iter()
                .map(|mail| mail.to_response_input_item())
                .collect::<Vec<_>>()
        };
        if pending_input.is_empty() {
            mailbox_items
        } else if mailbox_items.is_empty() {
            pending_input
        } else {
            let mut pending_input = pending_input;
            pending_input.extend(mailbox_items);
            pending_input
        }
    }

    /// Queue response items to be injected into the next active turn created for this session.
    #[cfg(test)]
    pub(crate) async fn queue_response_items_for_next_turn(&self, items: Vec<ResponseInputItem>) {
        if items.is_empty() {
            return;
        }

        let mut idle_pending_input = self.idle_pending_input.lock().await;
        idle_pending_input.extend(items);
    }

    pub(crate) async fn take_queued_response_items_for_next_turn(&self) -> Vec<ResponseInputItem> {
        std::mem::take(&mut *self.idle_pending_input.lock().await)
    }

    pub(crate) async fn has_queued_response_items_for_next_turn(&self) -> bool {
        !self.idle_pending_input.lock().await.is_empty()
    }

    pub async fn has_pending_input(&self) -> bool {
        if self.mailbox_rx.lock().await.has_pending() {
            return true;
        }
        let active = self.active_turn.lock().await;
        match active.as_ref() {
            Some(at) => {
                let ts = at.turn_state.lock().await;
                ts.has_pending_input()
            }
            None => false,
        }
    }

    pub async fn list_resources(
        &self,
        server: &str,
        params: Option<PaginatedRequestParams>,
    ) -> anyhow::Result<ListResourcesResult> {
        self.services
            .mcp_connection_manager
            .read()
            .await
            .list_resources(server, params)
            .await
    }

    pub async fn list_resource_templates(
        &self,
        server: &str,
        params: Option<PaginatedRequestParams>,
    ) -> anyhow::Result<ListResourceTemplatesResult> {
        self.services
            .mcp_connection_manager
            .read()
            .await
            .list_resource_templates(server, params)
            .await
    }

    pub async fn read_resource(
        &self,
        server: &str,
        params: ReadResourceRequestParams,
    ) -> anyhow::Result<ReadResourceResult> {
        self.services
            .mcp_connection_manager
            .read()
            .await
            .read_resource(server, params)
            .await
    }

    pub async fn call_tool(
        &self,
        server: &str,
        tool: &str,
        arguments: Option<serde_json::Value>,
        meta: Option<serde_json::Value>,
    ) -> anyhow::Result<CallToolResult> {
        self.services
            .mcp_connection_manager
            .read()
            .await
            .call_tool(server, tool, arguments, meta)
            .await
    }

    pub(crate) async fn parse_mcp_tool_name(
        &self,
        name: &str,
        namespace: &Option<String>,
    ) -> Option<(String, String)> {
        let tool_name = if let Some(namespace) = namespace {
            if name.starts_with(namespace.as_str()) {
                name
            } else {
                &format!("{namespace}{name}")
            }
        } else {
            name
        };
        self.services
            .mcp_connection_manager
            .read()
            .await
            .parse_tool_name(tool_name)
            .await
    }

    pub async fn interrupt_task(self: &Arc<Self>) {
        info!("interrupt received: abort current task, if any");
        let has_active_turn = { self.active_turn.lock().await.is_some() };
        if has_active_turn {
            self.abort_all_tasks(TurnAbortReason::Interrupted).await;
        } else {
            self.cancel_mcp_startup().await;
        }
    }

    pub(crate) fn hooks(&self) -> &Hooks {
        &self.services.hooks
    }

    pub(crate) fn user_shell(&self) -> Arc<shell::Shell> {
        Arc::clone(&self.services.user_shell)
    }

    pub(crate) async fn current_rollout_path(&self) -> Option<PathBuf> {
        let recorder = {
            let guard = self.services.rollout.lock().await;
            guard.clone()
        };
        recorder.map(|recorder| recorder.rollout_path().to_path_buf())
    }

    pub(crate) async fn hook_transcript_path(&self) -> Option<PathBuf> {
        self.ensure_rollout_materialized().await;
        self.current_rollout_path().await
    }

    pub(crate) async fn take_pending_session_start_source(
        &self,
    ) -> Option<codex_hooks::SessionStartSource> {
        let mut state = self.state.lock().await;
        state.take_pending_session_start_source()
    }

    async fn refresh_mcp_servers_inner(
        &self,
        turn_context: &TurnContext,
        mcp_servers: HashMap<String, McpServerConfig>,
        store_mode: OAuthCredentialsStoreMode,
    ) {
        let auth = self.services.auth_manager.auth().await;
        let config = self.get_config().await;
        let tool_plugin_provenance = self
            .services
            .mcp_manager
            .tool_plugin_provenance(config.as_ref());
        let mcp_servers = with_codex_apps_mcp(
            mcp_servers,
            self.features.apps_enabled_for_auth(auth.as_ref()),
            auth.as_ref(),
            config.as_ref(),
        );
        let auth_statuses = compute_auth_statuses(mcp_servers.iter(), store_mode).await;
        let sandbox_state = SandboxState {
            sandbox_policy: turn_context.sandbox_policy.get().clone(),
            codex_linux_sandbox_exe: turn_context.codex_linux_sandbox_exe.clone(),
            sandbox_cwd: turn_context.cwd.to_path_buf(),
            use_legacy_landlock: turn_context.features.use_legacy_landlock(),
        };
        {
            let mut guard = self.services.mcp_startup_cancellation_token.lock().await;
            guard.cancel();
            *guard = CancellationToken::new();
        }
        let (refreshed_manager, cancel_token) = McpConnectionManager::new(
            &mcp_servers,
            store_mode,
            auth_statuses,
            &turn_context.config.permissions.approval_policy,
            self.get_tx_event(),
            sandbox_state,
            config.codex_home.clone(),
            codex_apps_tools_cache_key(auth.as_ref()),
            tool_plugin_provenance,
        )
        .await;
        {
            let mut guard = self.services.mcp_startup_cancellation_token.lock().await;
            if guard.is_cancelled() {
                cancel_token.cancel();
            }
            *guard = cancel_token;
        }

        let mut manager = self.services.mcp_connection_manager.write().await;
        *manager = refreshed_manager;
    }

    async fn refresh_mcp_servers_if_requested(&self, turn_context: &TurnContext) {
        let refresh_config = { self.pending_mcp_server_refresh_config.lock().await.take() };
        let Some(refresh_config) = refresh_config else {
            return;
        };

        let McpServerRefreshConfig {
            mcp_servers,
            mcp_oauth_credentials_store_mode,
        } = refresh_config;

        let mcp_servers =
            match serde_json::from_value::<HashMap<String, McpServerConfig>>(mcp_servers) {
                Ok(servers) => servers,
                Err(err) => {
                    warn!("failed to parse MCP server refresh config: {err}");
                    return;
                }
            };
        let store_mode = match serde_json::from_value::<OAuthCredentialsStoreMode>(
            mcp_oauth_credentials_store_mode,
        ) {
            Ok(mode) => mode,
            Err(err) => {
                warn!("failed to parse MCP OAuth refresh config: {err}");
                return;
            }
        };

        self.refresh_mcp_servers_inner(turn_context, mcp_servers, store_mode)
            .await;
    }

    pub(crate) async fn refresh_mcp_servers_now(
        &self,
        turn_context: &TurnContext,
        mcp_servers: HashMap<String, McpServerConfig>,
        store_mode: OAuthCredentialsStoreMode,
    ) {
        self.refresh_mcp_servers_inner(turn_context, mcp_servers, store_mode)
            .await;
    }

    #[cfg(test)]
    async fn mcp_startup_cancellation_token(&self) -> CancellationToken {
        self.services
            .mcp_startup_cancellation_token
            .lock()
            .await
            .clone()
    }

    fn show_raw_agent_reasoning(&self) -> bool {
        self.services.show_raw_agent_reasoning
    }

    async fn cancel_mcp_startup(&self) {
        self.services
            .mcp_startup_cancellation_token
            .lock()
            .await
            .cancel();
    }
}

async fn submission_loop(sess: Arc<Session>, config: Arc<Config>, rx_sub: Receiver<Submission>) {
    // To break out of this loop, send Op::Shutdown.
    while let Ok(sub) = rx_sub.recv().await {
        debug!(?sub, "Submission");
        let dispatch_span = submission_dispatch_span(&sub);
        let should_exit = async {
            match sub.op.clone() {
                Op::Interrupt => {
                    handlers::interrupt(&sess).await;
                    false
                }
                Op::CleanBackgroundTerminals => {
                    handlers::clean_background_terminals(&sess).await;
                    false
                }
                Op::RealtimeConversationStart(params) => {
                    if let Err(err) =
                        handle_realtime_conversation_start(&sess, sub.id.clone(), params).await
                    {
                        sess.send_event_raw(Event {
                            id: sub.id.clone(),
                            msg: EventMsg::Error(ErrorEvent {
                                message: err.to_string(),
                                codex_error_info: Some(CodexErrorInfo::Other),
                            }),
                        })
                        .await;
                    }
                    false
                }
                Op::RealtimeConversationAudio(params) => {
                    handle_realtime_conversation_audio(&sess, sub.id.clone(), params).await;
                    false
                }
                Op::RealtimeConversationText(params) => {
                    handle_realtime_conversation_text(&sess, sub.id.clone(), params).await;
                    false
                }
                Op::RealtimeConversationClose => {
                    handle_realtime_conversation_close(&sess, sub.id.clone()).await;
                    false
                }
                Op::OverrideTurnContext {
                    cwd,
                    approval_policy,
                    approvals_reviewer,
                    sandbox_policy,
                    windows_sandbox_level,
                    model,
                    effort,
                    summary,
                    service_tier,
                    collaboration_mode,
                    personality,
                    nero_auto_runtime,
                } => {
                    let collaboration_mode = if let Some(collab_mode) = collaboration_mode {
                        collab_mode
                    } else {
                        let state = sess.state.lock().await;
                        state.session_configuration.collaboration_mode.with_updates(
                            model.clone(),
                            effort,
                            /*developer_instructions*/ None,
                        )
                    };
                    handlers::override_turn_context(
                        &sess,
                        sub.id.clone(),
                        SessionSettingsUpdate {
                            cwd,
                            approval_policy,
                            approvals_reviewer,
                            sandbox_policy,
                            windows_sandbox_level,
                            collaboration_mode: Some(collaboration_mode),
                            reasoning_summary: summary,
                            service_tier,
                            personality,
                            nero_auto_runtime,
                            ..Default::default()
                        },
                    )
                    .await;
                    false
                }
                Op::UserInput { .. } | Op::UserTurn { .. } => {
                    let is_hook_auto_reply = sub.id.starts_with(HOOK_AUTO_REPLY_SUBMISSION_PREFIX)
                        && sess.take_internal_hook_auto_submission_id(&sub.id);
                    let mut should_skip = false;
                    if !is_hook_auto_reply {
                        let generation_epoch = sess.begin_new_user_submission_generation().await;
                        debug!(
                            submission_id = %sub.id,
                            generation_epoch,
                            "started new user submission generation"
                        );
                    } else {
                        let hook_epoch = parse_hook_auto_reply_epoch(&sub.id);
                        if hook_epoch.is_none() {
                            warn!(
                                submission_id = %sub.id,
                                "dropping hook synthetic user reply with invalid epoch format"
                            );
                            should_skip = true;
                        }
                        if !should_skip && let Some(epoch) = hook_epoch {
                            let current_epoch = sess.current_hook_auto_reply_epoch();
                            if epoch != current_epoch {
                                info!(
                                    submission_id = %sub.id,
                                    hook_epoch = epoch,
                                    current_epoch,
                                    "skipping stale hook synthetic user reply from older generation"
                                );
                                should_skip = true;
                            }
                        }
                        if !should_skip && sess.active_turn.lock().await.is_some() {
                            if let Some(epoch) = hook_epoch {
                                sess.release_hook_auto_reply_chain_slot_for_epoch(epoch);
                            } else {
                                warn!(
                                    submission_id = %sub.id,
                                    "skipping hook synthetic user reply with unparseable epoch"
                                );
                            }
                            info!(
                                submission_id = %sub.id,
                                "skipping hook synthetic user reply because another turn is already active"
                            );
                            should_skip = true;
                        }
                    }
                    if should_skip {
                        false
                    } else {
                    handlers::user_input_or_turn(&sess, sub.id.clone(), sub.op).await;
                        false
                    }
                }
                Op::InterAgentCommunication { communication } => {
                    handlers::inter_agent_communication(&sess, sub.id.clone(), communication).await;
                    false
                }
                Op::ExecApproval {
                    id: approval_id,
                    turn_id,
                    decision,
                } => {
                    handlers::exec_approval(&sess, approval_id, turn_id, decision).await;
                    false
                }
                Op::PatchApproval { id, decision } => {
                    handlers::patch_approval(&sess, id, decision).await;
                    false
                }
                Op::UserInputAnswer { id, response } => {
                    handlers::request_user_input_response(&sess, id, response).await;
                    false
                }
                Op::RequestPermissionsResponse { id, response } => {
                    handlers::request_permissions_response(&sess, id, response).await;
                    false
                }
                Op::DynamicToolResponse { id, response } => {
                    handlers::dynamic_tool_response(&sess, id, response).await;
                    false
                }
                Op::AddToHistory { text } => {
                    handlers::add_to_history(&sess, &config, text).await;
                    false
                }
                Op::GetHistoryEntryRequest { offset, log_id } => {
                    handlers::get_history_entry_request(
                        &sess,
                        &config,
                        sub.id.clone(),
                        offset,
                        log_id,
                    )
                    .await;
                    false
                }
                Op::ListMcpTools => {
                    handlers::list_mcp_tools(&sess, &config, sub.id.clone()).await;
                    false
                }
                Op::RefreshMcpServers { config } => {
                    handlers::refresh_mcp_servers(&sess, config).await;
                    false
                }
                Op::ReloadUserConfig => {
                    handlers::reload_user_config(&sess).await;
                    false
                }
                Op::ListSkills { cwds, force_reload } => {
                    handlers::list_skills(&sess, sub.id.clone(), cwds, force_reload).await;
                    false
                }
                Op::Undo => {
                    handlers::undo(&sess, sub.id.clone()).await;
                    false
                }
                Op::Compact => {
                    handlers::compact(&sess, sub.id.clone()).await;
                    false
                }
                Op::DropMemories => {
                    handlers::drop_memories(&sess, &config, sub.id.clone()).await;
                    false
                }
                Op::UpdateMemories => {
                    handlers::update_memories(&sess, &config, sub.id.clone()).await;
                    false
                }
                Op::ThreadRollback { num_turns } => {
                    handlers::thread_rollback(&sess, sub.id.clone(), num_turns).await;
                    false
                }
                Op::SetThreadName { name } => {
                    handlers::set_thread_name(&sess, sub.id.clone(), name).await;
                    false
                }
                Op::RunUserShellCommand { command } => {
                    handlers::run_user_shell_command(&sess, sub.id.clone(), command).await;
                    false
                }
                Op::ResolveElicitation {
                    server_name,
                    request_id,
                    decision,
                    content,
                    meta,
                } => {
                    handlers::resolve_elicitation(
                        &sess,
                        server_name,
                        request_id,
                        decision,
                        content,
                        meta,
                    )
                    .await;
                    false
                }
                Op::Shutdown => handlers::shutdown(&sess, sub.id.clone()).await,
                Op::Review { review_request } => {
                    handlers::review(&sess, &config, sub.id.clone(), review_request).await;
                    false
                }
                _ => false, // Ignore unknown ops; enum is non_exhaustive to allow extensions.
            }
        }
        .instrument(dispatch_span)
        .await;
        if should_exit {
            break;
        }
    }
    // Also drain cached guardian state if the submission loop exits because
    // the channel closed without receiving an explicit shutdown op.
    sess.guardian_review_session.shutdown().await;
    debug!("Agent loop exited");
}

fn submission_dispatch_span(sub: &Submission) -> tracing::Span {
    let op_name = sub.op.kind();
    let span_name = format!("op.dispatch.{op_name}");
    let dispatch_span = match &sub.op {
        Op::RealtimeConversationAudio(_) => {
            debug_span!(
                "submission_dispatch",
                otel.name = span_name.as_str(),
                submission.id = sub.id.as_str(),
                codex.op = op_name
            )
        }
        _ => info_span!(
            "submission_dispatch",
            otel.name = span_name.as_str(),
            submission.id = sub.id.as_str(),
            codex.op = op_name
        ),
    };
    if let Some(trace) = sub.trace.as_ref()
        && !set_parent_from_w3c_trace_context(&dispatch_span, trace)
    {
        warn!(
            submission.id = sub.id.as_str(),
            "ignoring invalid submission trace carrier"
        );
    }
    dispatch_span
}

/// Operation handlers
mod handlers {
    use crate::codex::Session;
    use crate::codex::SessionSettingsUpdate;
    use crate::codex::SteerInputError;
    use crate::codex::TurnContext;
    use crate::protocol::SessionSource;

    use crate::SkillError;
    use crate::codex::spawn_review_thread;
    use crate::config::Config;
    use crate::config_loader::CloudRequirementsLoader;
    use crate::config_loader::LoaderOverrides;
    use crate::config_loader::load_config_layers_state;
    use codex_features::Feature;
    use codex_utils_absolute_path::AbsolutePathBuf;

    use crate::mcp::auth::compute_auth_statuses;
    use crate::mcp::collect_mcp_snapshot_from_manager;
    use crate::review_prompts::resolve_review_request;
    use crate::rollout::RolloutRecorder;
    use crate::rollout::session_index;
    use crate::tasks::CompactTask;
    use crate::tasks::UndoTask;
    use crate::tasks::UserShellCommandMode;
    use crate::tasks::UserShellCommandTask;
    use crate::tasks::execute_user_shell_command;
    use codex_protocol::protocol::CodexErrorInfo;
    use codex_protocol::protocol::ErrorEvent;
    use codex_protocol::protocol::Event;
    use codex_protocol::protocol::EventMsg;
    use codex_protocol::protocol::InterAgentCommunication;
    use codex_protocol::protocol::ListSkillsResponseEvent;
    use codex_protocol::protocol::McpServerRefreshConfig;
    use codex_protocol::protocol::Op;
    use codex_protocol::protocol::ReviewDecision;
    use codex_protocol::protocol::ReviewRequest;
    use codex_protocol::protocol::RolloutItem;
    use codex_protocol::protocol::SkillsListEntry;
    use codex_protocol::protocol::ThreadNameUpdatedEvent;
    use codex_protocol::protocol::ThreadRolledBackEvent;
    use codex_protocol::protocol::TurnAbortReason;
    use codex_protocol::protocol::WarningEvent;
    use codex_protocol::request_permissions::RequestPermissionsResponse;
    use codex_protocol::request_user_input::RequestUserInputResponse;

    use crate::context_manager::is_user_turn_boundary;
    use codex_app_server_protocol::SessionSource as AppServerSessionSource;
    use codex_protocol::config_types::CollaborationMode;
    use codex_protocol::config_types::ModeKind;
    use codex_protocol::config_types::Settings;
    use codex_protocol::dynamic_tools::DynamicToolResponse;
    use codex_protocol::items::HookPromptFragment;
    use codex_protocol::items::build_hook_prompt_message;
    use codex_protocol::mcp::RequestId as ProtocolRequestId;
    use codex_protocol::user_input::UserInput;
    use codex_rmcp_client::ElicitationAction;
    use codex_rmcp_client::ElicitationResponse;
    use serde::Deserialize;
    use serde::Serialize;
    use serde_json::Value;
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;
    use tracing::info;
    use tracing::warn;

    use crate::nero_auto_runtime_state::NeroThreadSessionAutoContext;
    use crate::nero_auto_runtime_state::build_session_auto_state;
    use crate::nero_auto_runtime_state::read_state_snapshot;
    use crate::nero_auto_runtime_state::resolve_nero_auto_state_path;

    const NERO_RUNTIME_STATE_CONTROL_CWD_ENV: &str = "NERO_RUNTIME_STATE_CONTROL_CWD";
    const NERO_RUNTIME_STATE_CONTROL_CWD_ENV_COMPAT: &str =
        "NEROBAR_NERO_RUNTIME_STATE_CONTROL_CWD";
    const NERO_RUNTIME_STATE_CONTROL_MODULE_ENV: &str = "NERO_RUNTIME_STATE_CONTROL_MODULE";
    const NERO_RUNTIME_STATE_CONTROL_MODULE_ENV_COMPAT: &str =
        "NEROBAR_NERO_RUNTIME_STATE_CONTROL_MODULE";
    const NERO_RUNTIME_STATE_CONTROL_TIMEOUT_ENV: &str = "NERO_RUNTIME_CONTROL_TIMEOUT_MS";
    const NERO_RUNTIME_STATE_CONTROL_TIMEOUT_ENV_COMPAT: &str =
        "NEROBAR_NERO_RUNTIME_CONTROL_TIMEOUT_MS";
    const NERO_RUNTIME_STATE_CONTROL_PYTHON_ENV: &str = "NERO_RUNTIME_PYTHON_BIN";
    const NERO_RUNTIME_STATE_CONTROL_PYTHON_ENV_COMPAT: &str = "NEROBAR_NERO_RUNTIME_PYTHON_BIN";
    const CODEXN_ROOT_ENV: &str = "CODEXN_ROOT";
    const NERO_AUTO_RUNTIME_CONFIG_ENV: &str = "CODEXN_CONFIG_NERO_AUTO_PATH";
    const NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE: &str = "nero_hook_runtime.session_auto_bridge";
    const NERO_RUNTIME_STATE_CONTROL_RETIRED_MODULE: &str =
        "nero_hook_runtime.state_runtime_control";
    const NERO_RUNTIME_STATE_CONTROL_DEFAULT_TIMEOUT_MS: u64 = 2_500;
    const NERO_AUTO_TURN_BOOST_TAG_PREFIX: &str = "[nero-hook-auto-boost";

    #[derive(Debug, Clone)]
    struct NeroAutoRuntimeBridgeSettings {
        cwd: PathBuf,
        module: String,
        python_bin: String,
        timeout: Duration,
    }

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct NeroAutoBridgeReadRequest {
        thread_id: String,
        session_source: String,
        config_path: String,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct NeroAutoBridgeEffective {
        enabled: bool,
        source: String,
        #[serde(default)]
        auto_rounds: usize,
    }

    #[derive(Debug, Clone, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct NeroAutoBridgeReadResponse {
        ok: bool,
        error: Option<String>,
        message: Option<String>,
        thread_id: Option<String>,
        session_source: Option<String>,
        is_subagent: bool,
        effective: NeroAutoBridgeEffective,
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

    fn normalize_runtime_state_control_module(module: String) -> String {
        if module == NERO_RUNTIME_STATE_CONTROL_RETIRED_MODULE {
            return NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string();
        }
        module
    }

    fn resolve_runtime_bridge_module(raw_module: Option<String>) -> (String, bool) {
        match raw_module {
            Some(module) => (normalize_runtime_state_control_module(module), true),
            None => (NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string(), false),
        }
    }

    fn resolve_runtime_bridge_module_from_env() -> (String, bool) {
        let raw_module = first_non_empty_env(&[
            NERO_RUNTIME_STATE_CONTROL_MODULE_ENV,
            NERO_RUNTIME_STATE_CONTROL_MODULE_ENV_COMPAT,
        ]);
        resolve_runtime_bridge_module(raw_module)
    }

    fn runtime_bridge_config_path_from_env(
        codex_home: &Path,
        configured_path: Option<&std::ffi::OsStr>,
    ) -> PathBuf {
        if let Some(path) = configured_path
            && !path.is_empty()
        {
            return PathBuf::from(path);
        }
        codex_home.join("config-nero-hook-auto.toml")
    }

    fn runtime_bridge_config_path(codex_home: &Path) -> PathBuf {
        runtime_bridge_config_path_from_env(
            codex_home,
            std::env::var_os(NERO_AUTO_RUNTIME_CONFIG_ENV).as_deref(),
        )
    }

    fn resolve_runtime_bridge_default_cwd_with_inputs(
        current_dir: &Path,
        codexn_root: Option<&str>,
    ) -> Result<PathBuf, String> {
        let mut candidates = Vec::with_capacity(8);
        let mut push_unique = |candidate: PathBuf| {
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        };
        if let Some(root) = codexn_root.map(str::trim).filter(|value| !value.is_empty()) {
            let root_path = PathBuf::from(root);
            push_unique(root_path.join("apps/codex-nero-sdk"));
            if let Some(parent) = root_path.parent() {
                push_unique(parent.join("codex-nero-sdk"));
            }
        }
        push_unique(current_dir.to_path_buf());
        push_unique(current_dir.join("apps/codex-nero-sdk"));
        if let Some(parent) = current_dir.parent() {
            push_unique(parent.join("apps/codex-nero-sdk"));
            push_unique(parent.join("codex-nero-sdk"));
        }
        if let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.join("nero_hook_runtime").is_dir())
        {
            return Ok(candidate.clone());
        }
        let searched = candidates
            .iter()
            .map(|candidate| candidate.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "runtime bridge bootstrap failed: no nero_hook_runtime package found under [{searched}]; set {NERO_RUNTIME_STATE_CONTROL_CWD_ENV} or {CODEXN_ROOT_ENV}"
        ))
    }

    fn resolve_runtime_bridge_default_cwd() -> Result<PathBuf, String> {
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        resolve_runtime_bridge_default_cwd_with_inputs(
            &current_dir,
            first_non_empty_env(&[CODEXN_ROOT_ENV]).as_deref(),
        )
    }

    fn resolve_nero_auto_runtime_bridge_settings() -> Result<NeroAutoRuntimeBridgeSettings, String>
    {
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let (module, module_overridden) = resolve_runtime_bridge_module_from_env();
        let configured_cwd = first_non_empty_env(&[
            NERO_RUNTIME_STATE_CONTROL_CWD_ENV,
            NERO_RUNTIME_STATE_CONTROL_CWD_ENV_COMPAT,
        ])
        .map(PathBuf::from);
        let cwd = match configured_cwd {
            Some(path) => path,
            None => {
                if module_overridden {
                    current_dir
                } else {
                    resolve_runtime_bridge_default_cwd()?
                }
            }
        };
        let python_bin = first_non_empty_env(&[
            NERO_RUNTIME_STATE_CONTROL_PYTHON_ENV,
            NERO_RUNTIME_STATE_CONTROL_PYTHON_ENV_COMPAT,
        ])
        .unwrap_or_else(|| "python3".to_string());
        let timeout_ms = first_non_empty_env(&[
            NERO_RUNTIME_STATE_CONTROL_TIMEOUT_ENV,
            NERO_RUNTIME_STATE_CONTROL_TIMEOUT_ENV_COMPAT,
        ])
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(NERO_RUNTIME_STATE_CONTROL_DEFAULT_TIMEOUT_MS);
        Ok(NeroAutoRuntimeBridgeSettings {
            cwd,
            module,
            python_bin,
            timeout: Duration::from_millis(timeout_ms),
        })
    }

    #[cfg(test)]
    mod bridge_settings_tests {
        use super::NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE;
        use super::NERO_RUNTIME_STATE_CONTROL_RETIRED_MODULE;
        use super::normalize_runtime_state_control_module;
        use super::resolve_runtime_bridge_default_cwd_with_inputs;
        use super::resolve_runtime_bridge_module;
        use pretty_assertions::assert_eq;
        use tempfile::tempdir;

        #[test]
        fn normalize_runtime_state_control_module_maps_retired_cli_module_to_current_bridge() {
            assert_eq!(
                normalize_runtime_state_control_module(
                    NERO_RUNTIME_STATE_CONTROL_RETIRED_MODULE.to_string(),
                ),
                NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string()
            );
        }

        #[test]
        fn normalize_runtime_state_control_module_keeps_current_bridge_module() {
            assert_eq!(
                normalize_runtime_state_control_module(
                    NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string(),
                ),
                NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string()
            );
        }

        #[test]
        fn resolve_runtime_bridge_default_cwd_uses_codexn_root_sdk_when_available() {
            let root = tempdir().expect("tempdir root");
            let sdk_dir = root.path().join("apps/codex-nero-sdk/nero_hook_runtime");
            std::fs::create_dir_all(&sdk_dir).expect("create sdk package");
            let cwd = tempdir().expect("tempdir cwd");
            assert_eq!(
                resolve_runtime_bridge_default_cwd_with_inputs(
                    cwd.path(),
                    Some(root.path().to_string_lossy().as_ref()),
                ),
                Ok(root.path().join("apps/codex-nero-sdk"))
            );
        }

        #[test]
        fn resolve_runtime_bridge_default_cwd_uses_codexn_root_parent_sdk_when_available() {
            let workspace = tempdir().expect("tempdir workspace");
            let app_root = workspace.path().join("apps/codex-nero");
            std::fs::create_dir_all(&app_root).expect("create app root");
            let sdk_dir = workspace.path().join("codex-nero-sdk/nero_hook_runtime");
            std::fs::create_dir_all(&sdk_dir).expect("create sibling sdk package");
            let cwd = workspace.path().join("cwd");
            std::fs::create_dir_all(&cwd).expect("create cwd");
            let expected_sdk_root = workspace.path().join("codex-nero-sdk");
            assert_eq!(
                resolve_runtime_bridge_default_cwd_with_inputs(
                    &cwd,
                    Some(app_root.to_string_lossy().as_ref()),
                ),
                Ok(expected_sdk_root)
            );
        }

        #[test]
        fn resolve_runtime_bridge_default_cwd_fails_when_no_candidate_contains_package() {
            let workspace = tempdir().expect("tempdir workspace");
            let cwd = workspace.path().join("sandbox/cwd");
            std::fs::create_dir_all(&cwd).expect("create cwd");
            let err = resolve_runtime_bridge_default_cwd_with_inputs(&cwd, None)
                .expect_err("expected bootstrap failure");
            assert!(err.contains("runtime bridge bootstrap failed"));
            assert!(err.contains("set NERO_RUNTIME_STATE_CONTROL_CWD or CODEXN_ROOT"));
        }

        #[test]
        fn resolve_runtime_bridge_module_uses_default_when_unset() {
            assert_eq!(
                resolve_runtime_bridge_module(None),
                (NERO_RUNTIME_STATE_CONTROL_DEFAULT_MODULE.to_string(), false)
            );
        }
    }

    fn app_server_session_source_from_wire(value: &str) -> AppServerSessionSource {
        match value {
            "cli" => AppServerSessionSource::Cli,
            "vscode" => AppServerSessionSource::VsCode,
            "exec" => AppServerSessionSource::Exec,
            "mcp" => AppServerSessionSource::AppServer,
            "subAgent" => AppServerSessionSource::SubAgent(
                codex_protocol::protocol::SubAgentSource::Other("runtime-bridge".to_string()),
            ),
            "unknown" => AppServerSessionSource::Unknown,
            other => AppServerSessionSource::Custom(other.to_string()),
        }
    }

    fn read_nero_auto_runtime_state_via_shared_seam(
        request: &NeroAutoBridgeReadRequest,
    ) -> Result<NeroAutoBridgeReadResponse, String> {
        let config_path = PathBuf::from(&request.config_path);
        let state_path = resolve_nero_auto_state_path(&config_path);
        let snapshot = read_state_snapshot(&state_path)?;
        let context = NeroThreadSessionAutoContext {
            thread_id: request.thread_id.clone(),
            thread_name: None,
            session_source: app_server_session_source_from_wire(&request.session_source),
            loaded: true,
            main_session_confirmed: true,
        };
        let state = build_session_auto_state(&context, &state_path, &config_path, &snapshot);
        let auto_rounds = usize::try_from(state.effective.auto_rounds).map_err(|err| {
            format!(
                "invalid session-auto effective.auto_rounds for thread {}: {err}",
                request.thread_id
            )
        })?;
        Ok(NeroAutoBridgeReadResponse {
            ok: true,
            error: None,
            message: None,
            thread_id: Some(request.thread_id.clone()),
            session_source: Some(request.session_source.clone()),
            is_subagent: state.is_subagent,
            effective: NeroAutoBridgeEffective {
                enabled: state.effective.runtime.enabled,
                source: state.effective.source,
                auto_rounds,
            },
        })
    }

    async fn run_nero_auto_runtime_bridge(
        command_name: &str,
        payload: &impl Serialize,
    ) -> Result<NeroAutoBridgeReadResponse, String> {
        if command_name == "read-session-auto" {
            let request = serde_json::to_value(payload)
                .map_err(|err| format!("serialize runtime state payload: {err}"))?;
            let request = serde_json::from_value::<NeroAutoBridgeReadRequest>(request)
                .map_err(|err| format!("deserialize runtime state payload: {err}"))?;
            return read_nero_auto_runtime_state_via_shared_seam(&request);
        }

        let settings = resolve_nero_auto_runtime_bridge_settings()?;
        let bridge_input = serde_json::to_vec(payload)
            .map_err(|err| format!("serialize bridge payload: {err}"))?;
        let mut command = Command::new(&settings.python_bin);
        command
            .current_dir(&settings.cwd)
            .kill_on_drop(true)
            .arg("-m")
            .arg(&settings.module)
            .arg(command_name)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|err| format!("spawn nero-auto runtime bridge: {err}"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "runtime bridge stdin unavailable".to_string())?;
        stdin
            .write_all(&bridge_input)
            .await
            .map_err(|err| format!("write runtime bridge stdin: {err}"))?;
        drop(stdin);

        let output = tokio::time::timeout(settings.timeout, child.wait_with_output())
            .await
            .map_err(|_| {
                format!(
                    "runtime bridge timed out after {}ms",
                    settings.timeout.as_millis()
                )
            })?
            .map_err(|err| format!("wait runtime bridge output: {err}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !output.status.success() {
            let detail = match (stderr.is_empty(), stdout.is_empty()) {
                (true, true) => format!("exit status {}", output.status),
                (false, true) => format!("exit status {}; stderr={stderr}", output.status),
                (true, false) => format!("exit status {}; stdout={stdout}", output.status),
                (false, false) => format!(
                    "exit status {}; stderr={stderr}; stdout={stdout}",
                    output.status
                ),
            };
            return Err(format!("runtime bridge command failed: {detail}"));
        }
        if stdout.is_empty() {
            let detail = if stderr.is_empty() {
                format!("exit status {}", output.status)
            } else {
                stderr
            };
            return Err(format!("runtime bridge returned empty stdout: {detail}"));
        }
        serde_json::from_str::<NeroAutoBridgeReadResponse>(&stdout).map_err(|err| {
            if stderr.is_empty() {
                format!("parse runtime bridge payload failed: {err}; stdout={stdout}")
            } else {
                format!(
                    "parse runtime bridge payload failed: {err}; stderr={stderr}; stdout={stdout}"
                )
            }
        })
    }

    fn developer_instructions_have_auto_path(value: Option<&str>) -> bool {
        let text = value.unwrap_or_default();
        text.contains("NERO_AUTO_V1")
            || text.contains("nero_auto_v1")
            || text.contains("NERO-HOOK-AUTO")
            || text.contains("scoring_system")
    }

    fn build_nero_auto_turn_booster(turn_id: &str, source: &str, auto_contract: &str) -> String {
        format!(
            "{NERO_AUTO_TURN_BOOST_TAG_PREFIX} stage=activation_boost turn_id={turn_id} source={source}]\n\
NERO-HOOK-AUTO bootstrap: auto runtime is enabled for this turn.\n\
This instruction is valid only for turn_id={turn_id}. Ignore it for all other turns.\n\
Use the exact auto protocol contract below in your final assistant response:\n\n\
{auto_contract}"
        )
    }

    fn should_inject_nero_auto_turn_booster(response: &NeroAutoBridgeReadResponse) -> bool {
        if !response.ok {
            return false;
        }
        if response.is_subagent || response.effective.source == "subagent-forced-off" {
            return false;
        }
        if !response.effective.enabled {
            return false;
        }
        let source = response.effective.source.trim();
        if source.is_empty() || source == "unknown" {
            return false;
        }
        if source != "session-override" {
            return false;
        }
        response.effective.auto_rounds == 0
    }

    fn validate_nero_auto_bridge_read_identity(
        response: &NeroAutoBridgeReadResponse,
        request: &NeroAutoBridgeReadRequest,
    ) -> Result<(), String> {
        let thread_id = response
            .thread_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "runtime bridge threadId echo is missing".to_string())?;
        if thread_id != request.thread_id {
            return Err(format!(
                "runtime bridge threadId mismatch: expected={}, got={thread_id}",
                request.thread_id
            ));
        }
        let session_source = response
            .session_source
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "runtime bridge sessionSource echo is missing".to_string())?;
        if session_source != request.session_source {
            return Err(format!(
                "runtime bridge sessionSource mismatch: expected={}, got={session_source}",
                request.session_source
            ));
        }
        Ok(())
    }

    fn session_source_supports_nero_auto_turn_booster(session_source: &SessionSource) -> bool {
        match session_source {
            SessionSource::SubAgent(_) | SessionSource::Unknown => false,
            SessionSource::Cli
            | SessionSource::VSCode
            | SessionSource::Exec
            | SessionSource::Mcp
            | SessionSource::Custom(_) => true,
        }
    }

    async fn maybe_prepare_nero_auto_turn_booster(
        sess: &Arc<Session>,
        turn_context: &Arc<TurnContext>,
        submission_id: &str,
    ) -> Option<String> {
        if submission_id.starts_with(super::HOOK_AUTO_REPLY_SUBMISSION_PREFIX) {
            return None;
        }
        if matches!(turn_context.session_source, SessionSource::SubAgent(_)) {
            return None;
        }
        // Booster is intentionally disabled for unknown/subagent sources and
        // allowed for first-party interactive sources that participate in the
        // shared runtime bridge contract.
        if !session_source_supports_nero_auto_turn_booster(&turn_context.session_source) {
            return None;
        }
        if developer_instructions_have_auto_path(turn_context.developer_instructions.as_deref()) {
            return None;
        }

        let request = NeroAutoBridgeReadRequest {
            thread_id: sess.conversation_id.to_string(),
            session_source: turn_context.session_source.to_string(),
            config_path: runtime_bridge_config_path(&sess.codex_home().await)
                .to_string_lossy()
                .to_string(),
        };
        let response = match run_nero_auto_runtime_bridge("read-session-auto", &request).await {
            Ok(response) => response,
            Err(err) => {
                warn!("failed to read session-auto runtime state for auto-turn booster: {err}");
                sess.maybe_emit_nero_auto_session_auto_read_warning(turn_context, &err)
                    .await;
                return None;
            }
        };
        if !response.ok {
            let detail = response
                .message
                .clone()
                .or(response.error)
                .unwrap_or_else(|| "session-auto runtime read failed".to_string());
            warn!("session-auto runtime read reported failure for auto-turn booster: {detail}");
            sess.maybe_emit_nero_auto_session_auto_read_warning(turn_context, &detail)
                .await;
            return None;
        }
        if let Err(err) = validate_nero_auto_bridge_read_identity(&response, &request) {
            warn!("{err}");
            sess.maybe_emit_nero_auto_session_auto_read_warning(turn_context, &err)
                .await;
            return None;
        }
        {
            let mut guard = match sess.nero_auto_bridge_warning_emitted.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = false;
        }
        if !should_inject_nero_auto_turn_booster(&response) {
            return None;
        }
        let auto_contract =
            match crate::config::resolve_codexn_fork_auto_developer_instructions_for_turn(
                &turn_context.session_source,
            ) {
                Ok(Some(value)) => value,
                Ok(None) => {
                    let detail = "session-auto auto protocol contract is unavailable for this turn";
                    warn!("{detail}");
                    sess.maybe_emit_nero_auto_session_auto_read_warning(turn_context, detail)
                        .await;
                    return None;
                }
                Err(err) => {
                    let detail =
                        format!("resolve session-auto auto protocol contract failed: {err}");
                    warn!("{detail}");
                    sess.maybe_emit_nero_auto_session_auto_read_warning(turn_context, &detail)
                        .await;
                    return None;
                }
            };
        let source = response.effective.source.trim();
        Some(build_nero_auto_turn_booster(
            &turn_context.sub_id,
            source,
            auto_contract.as_str(),
        ))
    }

    pub async fn interrupt(sess: &Arc<Session>) {
        sess.interrupt_task().await;
    }

    pub async fn clean_background_terminals(sess: &Arc<Session>) {
        sess.close_unified_exec_processes().await;
    }

    pub async fn override_turn_context(
        sess: &Session,
        sub_id: String,
        updates: SessionSettingsUpdate,
    ) {
        if let Err(err) = sess.update_settings(updates).await {
            sess.send_event_raw(Event {
                id: sub_id,
                msg: EventMsg::Error(ErrorEvent {
                    message: err.to_string(),
                    codex_error_info: Some(CodexErrorInfo::BadRequest),
                }),
            })
            .await;
        }
    }

    pub async fn user_input_or_turn(sess: &Arc<Session>, sub_id: String, op: Op) {
        let (items, updates) = match op {
            Op::UserTurn {
                cwd,
                approval_policy,
                approvals_reviewer,
                sandbox_policy,
                model,
                effort,
                summary,
                service_tier,
                final_output_json_schema,
                items,
                collaboration_mode,
                personality,
            } => {
                let collaboration_mode = collaboration_mode.or_else(|| {
                    Some(CollaborationMode {
                        mode: ModeKind::Default,
                        settings: Settings {
                            model: model.clone(),
                            reasoning_effort: effort,
                            developer_instructions: None,
                        },
                    })
                });
                (
                    items,
                    SessionSettingsUpdate {
                        cwd: Some(cwd),
                        approval_policy: Some(approval_policy),
                        approvals_reviewer,
                        sandbox_policy: Some(sandbox_policy),
                        windows_sandbox_level: None,
                        collaboration_mode,
                        reasoning_summary: summary,
                        service_tier,
                        final_output_json_schema: Some(final_output_json_schema),
                        personality,
                        app_server_client_name: None,
                        nero_auto_runtime: None,
                    },
                )
            }
            Op::UserInput {
                items,
                final_output_json_schema,
            } => (
                items,
                SessionSettingsUpdate {
                    final_output_json_schema: Some(final_output_json_schema),
                    ..Default::default()
                },
            ),
            _ => unreachable!(),
        };

        let submission_id = sub_id.clone();
        let Ok(current_context) = sess.new_turn_with_sub_id(sub_id.clone(), updates).await else {
            // new_turn_with_sub_id already emits the error event.
            return;
        };
        sess.maybe_emit_unknown_model_warning_for_turn(current_context.as_ref())
            .await;
        let nero_auto_turn_booster =
            maybe_prepare_nero_auto_turn_booster(sess, &current_context, &submission_id).await;
        match sess
            .steer_input(items.clone(), /*expected_turn_id*/ None)
            .await
        {
            Ok(_) => {
                if let Some(booster) = nero_auto_turn_booster.as_ref() {
                    let hook_run_id = format!("nero-auto-boost:{}", current_context.sub_id);
                    if let Some(hook_prompt_message) =
                        build_hook_prompt_message(&[HookPromptFragment::from_single_hook(
                            booster.clone(),
                            hook_run_id,
                        )])
                    {
                        sess.record_conversation_items(
                            &current_context,
                            std::slice::from_ref(&hook_prompt_message),
                        )
                        .await;
                    } else {
                        warn!(
                            turn_id = %current_context.sub_id,
                            "failed to build hook prompt for nero auto turn booster"
                        );
                    }
                }
                current_context.session_telemetry.user_prompt(&items);
            }
            Err(SteerInputError::NoActiveTurn(items)) => {
                if let Some(booster) = nero_auto_turn_booster.as_ref() {
                    let hook_run_id = format!("nero-auto-boost:{}", current_context.sub_id);
                    if let Some(hook_prompt_message) =
                        build_hook_prompt_message(&[HookPromptFragment::from_single_hook(
                            booster.clone(),
                            hook_run_id,
                        )])
                    {
                        sess.record_conversation_items(
                            &current_context,
                            std::slice::from_ref(&hook_prompt_message),
                        )
                        .await;
                    } else {
                        warn!(
                            turn_id = %current_context.sub_id,
                            "failed to build hook prompt for nero auto turn booster"
                        );
                    }
                }
                current_context.session_telemetry.user_prompt(&items);
                sess.refresh_mcp_servers_if_requested(&current_context)
                    .await;
                sess.spawn_task(
                    Arc::clone(&current_context),
                    items,
                    crate::tasks::RegularTask::new(),
                )
                .await;
            }
            Err(err) => {
                sess.send_event_raw(Event {
                    id: sub_id,
                    msg: EventMsg::Error(err.to_error_event()),
                })
                .await;
            }
        }
    }

    /// Records an inter-agent assistant envelope and, when requested, wakes the recipient by
    /// starting a regular turn if the session is currently idle.
    pub async fn inter_agent_communication(
        sess: &Arc<Session>,
        sub_id: String,
        communication: InterAgentCommunication,
    ) {
        let trigger_turn = communication.trigger_turn;
        sess.enqueue_mailbox_communication(communication);
        if trigger_turn {
            sess.ensure_task_for_pending_inputs_with_sub_id(sub_id)
                .await;
        }
    }

    pub async fn run_user_shell_command(sess: &Arc<Session>, sub_id: String, command: String) {
        if let Some((turn_context, cancellation_token)) =
            sess.active_turn_context_and_cancellation_token().await
        {
            let session = Arc::clone(sess);
            tokio::spawn(async move {
                execute_user_shell_command(
                    session,
                    turn_context,
                    command,
                    cancellation_token,
                    UserShellCommandMode::ActiveTurnAuxiliary,
                )
                .await;
            });
            return;
        }

        let turn_context = sess.new_default_turn_with_sub_id(sub_id).await;
        sess.spawn_task(
            Arc::clone(&turn_context),
            Vec::new(),
            UserShellCommandTask::new(command),
        )
        .await;
    }

    pub async fn resolve_elicitation(
        sess: &Arc<Session>,
        server_name: String,
        request_id: ProtocolRequestId,
        decision: codex_protocol::approvals::ElicitationAction,
        content: Option<Value>,
        meta: Option<Value>,
    ) {
        let action = match decision {
            codex_protocol::approvals::ElicitationAction::Accept => ElicitationAction::Accept,
            codex_protocol::approvals::ElicitationAction::Decline => ElicitationAction::Decline,
            codex_protocol::approvals::ElicitationAction::Cancel => ElicitationAction::Cancel,
        };
        let content = match action {
            // Preserve the legacy fallback for clients that only send an action.
            ElicitationAction::Accept => Some(content.unwrap_or_else(|| serde_json::json!({}))),
            ElicitationAction::Decline | ElicitationAction::Cancel => None,
        };
        let response = ElicitationResponse {
            action,
            content,
            meta,
        };
        let request_id = match request_id {
            ProtocolRequestId::String(value) => {
                rmcp::model::NumberOrString::String(std::sync::Arc::from(value))
            }
            ProtocolRequestId::Integer(value) => rmcp::model::NumberOrString::Number(value),
        };
        if let Err(err) = sess
            .resolve_elicitation(server_name, request_id, response)
            .await
        {
            warn!(
                error = %err,
                "failed to resolve elicitation request in session"
            );
        }
    }

    /// Propagate a user's exec approval decision to the session.
    /// Also optionally applies an execpolicy amendment.
    pub async fn exec_approval(
        sess: &Arc<Session>,
        approval_id: String,
        turn_id: Option<String>,
        decision: ReviewDecision,
    ) {
        let event_turn_id = turn_id.unwrap_or_else(|| approval_id.clone());
        if let ReviewDecision::ApprovedExecpolicyAmendment {
            proposed_execpolicy_amendment,
        } = &decision
        {
            match sess
                .persist_execpolicy_amendment(proposed_execpolicy_amendment)
                .await
            {
                Ok(()) => {
                    sess.record_execpolicy_amendment_message(
                        &event_turn_id,
                        proposed_execpolicy_amendment,
                    )
                    .await;
                }
                Err(err) => {
                    let message = format!("Failed to apply execpolicy amendment: {err}");
                    tracing::warn!("{message}");
                    let warning = EventMsg::Warning(WarningEvent { message });
                    sess.send_event_raw(Event {
                        id: event_turn_id.clone(),
                        msg: warning,
                    })
                    .await;
                }
            }
        }
        match decision {
            ReviewDecision::Abort => {
                sess.interrupt_task().await;
            }
            other => sess.notify_approval(&approval_id, other).await,
        }
    }

    pub async fn patch_approval(sess: &Arc<Session>, id: String, decision: ReviewDecision) {
        match decision {
            ReviewDecision::Abort => {
                sess.interrupt_task().await;
            }
            other => sess.notify_approval(&id, other).await,
        }
    }

    pub async fn request_user_input_response(
        sess: &Arc<Session>,
        id: String,
        response: RequestUserInputResponse,
    ) {
        sess.notify_user_input_response(&id, response).await;
    }

    pub async fn request_permissions_response(
        sess: &Arc<Session>,
        id: String,
        response: RequestPermissionsResponse,
    ) {
        sess.notify_request_permissions_response(&id, response)
            .await;
    }

    pub async fn dynamic_tool_response(
        sess: &Arc<Session>,
        id: String,
        response: DynamicToolResponse,
    ) {
        sess.notify_dynamic_tool_response(&id, response).await;
    }

    pub async fn add_to_history(sess: &Arc<Session>, config: &Arc<Config>, text: String) {
        let id = sess.conversation_id;
        let config = Arc::clone(config);
        tokio::spawn(async move {
            if let Err(e) = crate::message_history::append_entry(&text, &id, &config).await {
                warn!("failed to append to message history: {e}");
            }
        });
    }

    pub async fn get_history_entry_request(
        sess: &Arc<Session>,
        config: &Arc<Config>,
        sub_id: String,
        offset: usize,
        log_id: u64,
    ) {
        let config = Arc::clone(config);
        let sess_clone = Arc::clone(sess);

        tokio::spawn(async move {
            // Run lookup in blocking thread because it does file IO + locking.
            let entry_opt = tokio::task::spawn_blocking(move || {
                crate::message_history::lookup(log_id, offset, &config)
            })
            .await
            .unwrap_or(None);

            let event = Event {
                id: sub_id,
                msg: EventMsg::GetHistoryEntryResponse(
                    crate::protocol::GetHistoryEntryResponseEvent {
                        offset,
                        log_id,
                        entry: entry_opt.map(|e| codex_protocol::message_history::HistoryEntry {
                            conversation_id: e.session_id,
                            ts: e.ts,
                            text: e.text,
                        }),
                    },
                ),
            };

            sess_clone.send_event_raw(event).await;
        });
    }

    pub async fn refresh_mcp_servers(sess: &Arc<Session>, refresh_config: McpServerRefreshConfig) {
        let mut guard = sess.pending_mcp_server_refresh_config.lock().await;
        *guard = Some(refresh_config);
    }

    pub async fn reload_user_config(sess: &Arc<Session>) {
        sess.reload_user_config_layer().await;
    }

    pub async fn list_mcp_tools(sess: &Session, config: &Arc<Config>, sub_id: String) {
        let mcp_connection_manager = sess.services.mcp_connection_manager.read().await;
        let auth = sess.services.auth_manager.auth().await;
        let mcp_servers = sess
            .services
            .mcp_manager
            .effective_servers(config, auth.as_ref());
        let snapshot = collect_mcp_snapshot_from_manager(
            &mcp_connection_manager,
            compute_auth_statuses(mcp_servers.iter(), config.mcp_oauth_credentials_store_mode)
                .await,
        )
        .await;
        let event = Event {
            id: sub_id,
            msg: EventMsg::McpListToolsResponse(snapshot),
        };
        sess.send_event_raw(event).await;
    }

    pub async fn list_skills(
        sess: &Session,
        sub_id: String,
        cwds: Vec<PathBuf>,
        force_reload: bool,
    ) {
        let cwds = if cwds.is_empty() {
            let state = sess.state.lock().await;
            vec![state.session_configuration.cwd.to_path_buf()]
        } else {
            cwds
        };

        let skills_manager = &sess.services.skills_manager;
        let plugins_manager = &sess.services.plugins_manager;
        let config = sess.get_config().await;
        let codex_home = sess.codex_home().await;
        let mut skills = Vec::new();
        let empty_cli_overrides: &[(String, toml::Value)] = &[];
        for cwd in cwds {
            let cwd_abs = match AbsolutePathBuf::try_from(cwd.as_path()) {
                Ok(path) => path,
                Err(err) => {
                    let message = err.to_string();
                    let cwd_for_entry = cwd.clone();
                    skills.push(SkillsListEntry {
                        cwd: cwd_for_entry.clone(),
                        skills: Vec::new(),
                        errors: super::errors_to_info(&[SkillError {
                            path: cwd_for_entry,
                            message,
                        }]),
                    });
                    continue;
                }
            };
            let config_layer_stack = match load_config_layers_state(
                &codex_home,
                Some(cwd_abs),
                empty_cli_overrides,
                LoaderOverrides::default(),
                CloudRequirementsLoader::default(),
            )
            .await
            {
                Ok(config_layer_stack) => config_layer_stack,
                Err(err) => {
                    let message = err.to_string();
                    let cwd_for_entry = cwd.clone();
                    skills.push(SkillsListEntry {
                        cwd: cwd_for_entry.clone(),
                        skills: Vec::new(),
                        errors: super::errors_to_info(&[SkillError {
                            path: cwd_for_entry,
                            message,
                        }]),
                    });
                    continue;
                }
            };
            let effective_skill_roots = plugins_manager.effective_skill_roots_for_layer_stack(
                &config_layer_stack,
                config.features.enabled(Feature::Plugins),
            );
            let skills_input = crate::SkillsLoadInput::new(
                cwd.clone(),
                effective_skill_roots,
                config_layer_stack,
                config.bundled_skills_enabled(),
            );
            let outcome = skills_manager
                .skills_for_cwd(&skills_input, force_reload)
                .await;
            let errors = super::errors_to_info(&outcome.errors);
            let skills_metadata = super::skills_to_info(&outcome.skills, &outcome.disabled_paths);
            skills.push(SkillsListEntry {
                cwd,
                skills: skills_metadata,
                errors,
            });
        }

        let event = Event {
            id: sub_id,
            msg: EventMsg::ListSkillsResponse(ListSkillsResponseEvent { skills }),
        };
        sess.send_event_raw(event).await;
    }

    pub async fn undo(sess: &Arc<Session>, sub_id: String) {
        let turn_context = sess.new_default_turn_with_sub_id(sub_id).await;
        sess.spawn_task(turn_context, Vec::new(), UndoTask::new())
            .await;
    }

    pub async fn compact(sess: &Arc<Session>, sub_id: String) {
        let turn_context = sess.new_default_turn_with_sub_id(sub_id).await;

        sess.spawn_task(
            Arc::clone(&turn_context),
            vec![UserInput::Text {
                text: turn_context.compact_prompt().to_string(),
                // Compaction prompt is synthesized; no UI element ranges to preserve.
                text_elements: Vec::new(),
            }],
            CompactTask,
        )
        .await;
    }

    pub async fn drop_memories(sess: &Arc<Session>, config: &Arc<Config>, sub_id: String) {
        let mut errors = Vec::new();

        if let Some(state_db) = sess.services.state_db.as_deref() {
            if let Err(err) = state_db.clear_memory_data().await {
                errors.push(format!("failed clearing memory rows from state db: {err}"));
            }
        } else {
            errors.push("state db unavailable; memory rows were not cleared".to_string());
        }

        let memory_root = crate::memories::memory_root(&config.codex_home);
        if let Err(err) = crate::memories::clear_memory_root_contents(&memory_root).await {
            errors.push(format!(
                "failed clearing memory directory {}: {err}",
                memory_root.display()
            ));
        }

        if errors.is_empty() {
            sess.send_event_raw(Event {
                id: sub_id,
                msg: EventMsg::Warning(WarningEvent {
                    message: format!(
                        "Dropped memories at {} and cleared memory rows from state db.",
                        memory_root.display()
                    ),
                }),
            })
            .await;
            return;
        }

        sess.send_event_raw(Event {
            id: sub_id,
            msg: EventMsg::Error(ErrorEvent {
                message: format!("Memory drop completed with errors: {}", errors.join("; ")),
                codex_error_info: Some(CodexErrorInfo::Other),
            }),
        })
        .await;
    }

    pub async fn update_memories(sess: &Arc<Session>, config: &Arc<Config>, sub_id: String) {
        let session_source = {
            let state = sess.state.lock().await;
            state.session_configuration.session_source.clone()
        };

        crate::memories::start_memories_startup_task(sess, Arc::clone(config), &session_source);

        sess.send_event_raw(Event {
            id: sub_id.clone(),
            msg: EventMsg::Warning(WarningEvent {
                message: "Memory update triggered.".to_string(),
            }),
        })
        .await;
    }

    pub async fn thread_rollback(sess: &Arc<Session>, sub_id: String, num_turns: u32) {
        if num_turns == 0 {
            sess.send_event_raw(Event {
                id: sub_id,
                msg: EventMsg::Error(ErrorEvent {
                    message: "num_turns must be >= 1".to_string(),
                    codex_error_info: Some(CodexErrorInfo::ThreadRollbackFailed),
                }),
            })
            .await;
            return;
        }

        let has_active_turn = { sess.active_turn.lock().await.is_some() };
        if has_active_turn {
            sess.send_event_raw(Event {
                id: sub_id,
                msg: EventMsg::Error(ErrorEvent {
                    message: "Cannot rollback while a turn is in progress.".to_string(),
                    codex_error_info: Some(CodexErrorInfo::ThreadRollbackFailed),
                }),
            })
            .await;
            return;
        }

        let turn_context = sess.new_default_turn_with_sub_id(sub_id).await;
        let rollout_path = {
            let recorder = {
                let guard = sess.services.rollout.lock().await;
                guard.clone()
            };
            let Some(recorder) = recorder else {
                sess.send_event_raw(Event {
                    id: turn_context.sub_id.clone(),
                    msg: EventMsg::Error(ErrorEvent {
                        message: "thread rollback requires a persisted rollout path".to_string(),
                        codex_error_info: Some(CodexErrorInfo::ThreadRollbackFailed),
                    }),
                })
                .await;
                return;
            };
            recorder.rollout_path().to_path_buf()
        };
        if let Some(recorder) = {
            let guard = sess.services.rollout.lock().await;
            guard.clone()
        } && let Err(err) = recorder.flush().await
        {
            sess.send_event_raw(Event {
                id: turn_context.sub_id.clone(),
                msg: EventMsg::Error(ErrorEvent {
                    message: format!(
                        "failed to flush rollout `{}` for rollback replay: {err}",
                        rollout_path.display()
                    ),
                    codex_error_info: Some(CodexErrorInfo::ThreadRollbackFailed),
                }),
            })
            .await;
            return;
        }

        let initial_history =
            match RolloutRecorder::get_rollout_history(rollout_path.as_path()).await {
                Ok(history) => history,
                Err(err) => {
                    sess.send_event_raw(Event {
                        id: turn_context.sub_id.clone(),
                        msg: EventMsg::Error(ErrorEvent {
                            message: format!(
                                "failed to load rollout `{}` for rollback replay: {err}",
                                rollout_path.display()
                            ),
                            codex_error_info: Some(CodexErrorInfo::ThreadRollbackFailed),
                        }),
                    })
                    .await;
                    return;
                }
            };

        let rollback_event = ThreadRolledBackEvent { num_turns };
        let rollback_msg = EventMsg::ThreadRolledBack(rollback_event.clone());
        let replay_items = initial_history
            .get_rollout_items()
            .into_iter()
            .chain(std::iter::once(RolloutItem::EventMsg(rollback_msg.clone())))
            .collect::<Vec<_>>();
        sess.persist_rollout_items(&[RolloutItem::EventMsg(rollback_msg.clone())])
            .await;
        sess.flush_rollout().await;
        sess.apply_rollout_reconstruction(turn_context.as_ref(), replay_items.as_slice())
            .await;
        sess.recompute_token_usage(turn_context.as_ref()).await;

        sess.deliver_event_raw(Event {
            id: turn_context.sub_id.clone(),
            msg: rollback_msg,
        })
        .await;
    }

    /// Persists the thread name in the session index, updates in-memory state, and emits
    /// a `ThreadNameUpdated` event on success.
    ///
    /// This appends the name to `CODEX_HOME/sessions_index.jsonl` via `session_index::append_thread_name` for the
    /// current `thread_id`, then updates `SessionConfiguration::thread_name`.
    ///
    /// Returns an error event if the name is empty or session persistence is disabled.
    pub async fn set_thread_name(sess: &Arc<Session>, sub_id: String, name: String) {
        let Some(name) = crate::util::normalize_thread_name(&name) else {
            let event = Event {
                id: sub_id,
                msg: EventMsg::Error(ErrorEvent {
                    message: "Thread name cannot be empty.".to_string(),
                    codex_error_info: Some(CodexErrorInfo::BadRequest),
                }),
            };
            sess.send_event_raw(event).await;
            return;
        };

        let persistence_enabled = {
            let rollout = sess.services.rollout.lock().await;
            rollout.is_some()
        };
        if !persistence_enabled {
            let event = Event {
                id: sub_id,
                msg: EventMsg::Error(ErrorEvent {
                    message: "Session persistence is disabled; cannot rename thread.".to_string(),
                    codex_error_info: Some(CodexErrorInfo::Other),
                }),
            };
            sess.send_event_raw(event).await;
            return;
        };

        let codex_home = sess.codex_home().await;
        if let Err(e) =
            session_index::append_thread_name(&codex_home, sess.conversation_id, &name).await
        {
            let event = Event {
                id: sub_id,
                msg: EventMsg::Error(ErrorEvent {
                    message: format!("Failed to set thread name: {e}"),
                    codex_error_info: Some(CodexErrorInfo::Other),
                }),
            };
            sess.send_event_raw(event).await;
            return;
        }

        {
            let mut state = sess.state.lock().await;
            state.session_configuration.thread_name = Some(name.clone());
        }

        sess.send_event_raw(Event {
            id: sub_id,
            msg: EventMsg::ThreadNameUpdated(ThreadNameUpdatedEvent {
                thread_id: sess.conversation_id,
                thread_name: Some(name),
            }),
        })
        .await;
    }

    pub async fn shutdown(sess: &Arc<Session>, sub_id: String) -> bool {
        sess.abort_all_tasks(TurnAbortReason::Interrupted).await;
        let _ = sess.conversation.shutdown().await;
        sess.services
            .unified_exec_manager
            .terminate_all_processes()
            .await;
        sess.guardian_review_session.shutdown().await;
        info!("Shutting down Codex instance");
        let history = sess.clone_history().await;
        let turn_count = history
            .raw_items()
            .iter()
            .filter(|item| is_user_turn_boundary(item))
            .count();
        sess.services.session_telemetry.counter(
            "codex.conversation.turn.count",
            i64::try_from(turn_count).unwrap_or(0),
            &[],
        );

        // Gracefully flush and shutdown rollout recorder on session end so tests
        // that inspect the rollout file do not race with the background writer.
        let recorder_opt = {
            let mut guard = sess.services.rollout.lock().await;
            guard.take()
        };
        if let Some(rec) = recorder_opt
            && let Err(e) = rec.shutdown().await
        {
            warn!("failed to shutdown rollout recorder: {e}");
            let event = Event {
                id: sub_id.clone(),
                msg: EventMsg::Error(ErrorEvent {
                    message: "Failed to shutdown rollout recorder".to_string(),
                    codex_error_info: Some(CodexErrorInfo::Other),
                }),
            };
            sess.send_event_raw(event).await;
        }

        let event = Event {
            id: sub_id,
            msg: EventMsg::ShutdownComplete,
        };
        sess.send_event_raw(event).await;
        true
    }

    pub async fn review(
        sess: &Arc<Session>,
        config: &Arc<Config>,
        sub_id: String,
        review_request: ReviewRequest,
    ) {
        let turn_context = sess.new_default_turn_with_sub_id(sub_id.clone()).await;
        sess.maybe_emit_unknown_model_warning_for_turn(turn_context.as_ref())
            .await;
        sess.refresh_mcp_servers_if_requested(&turn_context).await;
        match resolve_review_request(review_request, turn_context.cwd.as_path()) {
            Ok(resolved) => {
                spawn_review_thread(
                    Arc::clone(sess),
                    Arc::clone(config),
                    turn_context.clone(),
                    sub_id,
                    resolved,
                )
                .await;
            }
            Err(err) => {
                let event = Event {
                    id: sub_id,
                    msg: EventMsg::Error(ErrorEvent {
                        message: err.to_string(),
                        codex_error_info: Some(CodexErrorInfo::Other),
                    }),
                };
                sess.send_event(&turn_context, event.msg).await;
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn sample_bridge_response(
            enabled: bool,
            source: &str,
            is_subagent: bool,
            auto_rounds: usize,
        ) -> NeroAutoBridgeReadResponse {
            NeroAutoBridgeReadResponse {
                ok: true,
                error: None,
                message: None,
                thread_id: Some("thread-123".to_string()),
                session_source: Some("mcp".to_string()),
                is_subagent,
                effective: NeroAutoBridgeEffective {
                    enabled,
                    source: source.to_string(),
                    auto_rounds,
                },
            }
        }

        #[test]
        fn developer_instructions_auto_marker_detection_is_stable() {
            assert!(developer_instructions_have_auto_path(Some(
                "... NERO_AUTO_V1 ..."
            )));
            assert!(developer_instructions_have_auto_path(Some(
                "... scoring_system ..."
            )));
            assert!(!developer_instructions_have_auto_path(Some(
                "plain instructions"
            )));
            assert!(!developer_instructions_have_auto_path(None));
        }

        #[test]
        fn turn_booster_tag_contains_machine_parseable_marker() {
            let text = build_nero_auto_turn_booster(
                "turn-123",
                "session-override",
                "## NERO-SYSTEM v1\n\n```json\n{}\n```",
            );
            assert!(text.contains("[nero-hook-auto-boost"));
            assert!(text.contains("stage=activation_boost"));
            assert!(text.contains("turn_id=turn-123"));
            assert!(text.contains("source=session-override"));
            assert!(text.contains("valid only for turn_id=turn-123"));
            assert!(text.contains("## NERO-SYSTEM v1"));
        }

        #[test]
        fn bridge_response_shapes_for_booster_preconditions_are_explicit() {
            let enabled = sample_bridge_response(true, "session-override", false, 0);
            assert!(enabled.effective.enabled);
            assert_eq!(enabled.effective.source, "session-override");
            assert_eq!(enabled.effective.auto_rounds, 0);
            assert!(!enabled.is_subagent);

            let subagent = sample_bridge_response(true, "subagent-forced-off", true, 0);
            assert!(subagent.is_subagent);
            assert_eq!(subagent.effective.source, "subagent-forced-off");
        }

        #[test]
        fn booster_injection_requires_session_override_boundary() {
            let boundary = sample_bridge_response(true, "session-override", false, 0);
            assert!(should_inject_nero_auto_turn_booster(&boundary));

            let later_round = sample_bridge_response(true, "session-override", false, 2);
            assert!(!should_inject_nero_auto_turn_booster(&later_round));

            let default_source = sample_bridge_response(true, "config-default", false, 0);
            assert!(!should_inject_nero_auto_turn_booster(&default_source));
        }

        #[test]
        fn booster_path_supports_all_non_subagent_sources() {
            assert!(session_source_supports_nero_auto_turn_booster(
                &SessionSource::Mcp
            ));
            assert!(session_source_supports_nero_auto_turn_booster(
                &SessionSource::Cli,
            ));
            assert!(session_source_supports_nero_auto_turn_booster(
                &SessionSource::VSCode,
            ));
            assert!(session_source_supports_nero_auto_turn_booster(
                &SessionSource::Exec,
            ));
            assert!(session_source_supports_nero_auto_turn_booster(
                &SessionSource::Custom("other".to_string()),
            ));
            assert!(!session_source_supports_nero_auto_turn_booster(
                &SessionSource::SubAgent(crate::protocol::SubAgentSource::Review),
            ));
            assert!(!session_source_supports_nero_auto_turn_booster(
                &SessionSource::Unknown
            ));
        }

        #[test]
        fn bridge_identity_validation_rejects_missing_echo_fields() {
            let response = NeroAutoBridgeReadResponse {
                ok: true,
                error: None,
                message: None,
                thread_id: None,
                session_source: Some("mcp".to_string()),
                is_subagent: false,
                effective: NeroAutoBridgeEffective {
                    enabled: true,
                    source: "session-override".to_string(),
                    auto_rounds: 0,
                },
            };
            let request = NeroAutoBridgeReadRequest {
                thread_id: "thread-123".to_string(),
                session_source: "mcp".to_string(),
                config_path: "/tmp/config-nero-hook-auto.toml".to_string(),
            };
            let result = validate_nero_auto_bridge_read_identity(&response, &request);
            assert_eq!(
                result,
                Err("runtime bridge threadId echo is missing".to_string())
            );
        }

        #[test]
        fn bridge_identity_validation_rejects_session_source_mismatch() {
            let response = NeroAutoBridgeReadResponse {
                ok: true,
                error: None,
                message: None,
                thread_id: Some("thread-123".to_string()),
                session_source: Some("cli".to_string()),
                is_subagent: false,
                effective: NeroAutoBridgeEffective {
                    enabled: true,
                    source: "session-override".to_string(),
                    auto_rounds: 0,
                },
            };
            let request = NeroAutoBridgeReadRequest {
                thread_id: "thread-123".to_string(),
                session_source: "mcp".to_string(),
                config_path: "/tmp/config-nero-hook-auto.toml".to_string(),
            };
            let result = validate_nero_auto_bridge_read_identity(&response, &request);
            assert_eq!(
                result,
                Err("runtime bridge sessionSource mismatch: expected=mcp, got=cli".to_string())
            );
        }

        #[test]
        fn runtime_bridge_config_path_uses_explicit_override_when_present() {
            assert_eq!(
                runtime_bridge_config_path_from_env(
                    std::path::Path::new("/tmp/codex-home"),
                    Some(std::ffi::OsStr::new("/tmp/explicit-nero-auto.toml"))
                ),
                std::path::Path::new("/tmp/explicit-nero-auto.toml")
            );
        }

        #[test]
        fn runtime_bridge_config_path_falls_back_for_empty_override() {
            assert_eq!(
                runtime_bridge_config_path_from_env(
                    std::path::Path::new("/tmp/codex-home"),
                    Some(std::ffi::OsStr::new(""))
                ),
                std::path::Path::new("/tmp/codex-home/config-nero-hook-auto.toml")
            );
        }
    }
}

/// Spawn a review thread using the given prompt.
async fn spawn_review_thread(
    sess: Arc<Session>,
    config: Arc<Config>,
    parent_turn_context: Arc<TurnContext>,
    sub_id: String,
    resolved: crate::review_prompts::ResolvedReviewRequest,
) {
    let model = config
        .review_model
        .clone()
        .unwrap_or_else(|| parent_turn_context.model_info.slug.clone());
    let review_model_info = sess
        .services
        .models_manager
        .get_model_info(&model, &config)
        .await;
    // For reviews, disable web_search and view_image regardless of global settings.
    let mut review_features = sess.features.clone();
    let _ = review_features.disable(Feature::WebSearchRequest);
    let _ = review_features.disable(Feature::WebSearchCached);
    let review_web_search_mode = WebSearchMode::Disabled;
    let tools_config = ToolsConfig::new(&ToolsConfigParams {
        model_info: &review_model_info,
        available_models: &sess
            .services
            .models_manager
            .list_models(RefreshStrategy::OnlineIfUncached)
            .await,
        features: &review_features,
        web_search_mode: Some(review_web_search_mode),
        session_source: parent_turn_context.session_source.clone(),
        sandbox_policy: parent_turn_context.sandbox_policy.get(),
        windows_sandbox_level: parent_turn_context.windows_sandbox_level,
    })
    .with_unified_exec_shell_mode_for_session(
        sess.services.user_shell.as_ref(),
        sess.services.shell_zsh_path.as_ref(),
        sess.services.main_execve_wrapper_exe.as_ref(),
    )
    .with_web_search_config(/*web_search_config*/ None)
    .with_allow_login_shell(config.permissions.allow_login_shell)
    .with_agent_roles(config.agent_roles.clone())
    .with_spawn_delegation_report_required(
        config.spawn_delegation_report_profile.required_in_spawn(),
    );

    let review_prompt = resolved.prompt.clone();
    let provider = parent_turn_context.provider.clone();
    let auth_manager = parent_turn_context.auth_manager.clone();
    let model_info = review_model_info.clone();

    // Build per‑turn client with the requested model/family.
    let mut per_turn_config = (*config).clone();
    per_turn_config.model = Some(model.clone());
    per_turn_config.features = review_features.clone();
    if let Err(err) = per_turn_config.web_search_mode.set(review_web_search_mode) {
        let fallback_value = per_turn_config.web_search_mode.value();
        tracing::warn!(
            error = %err,
            ?review_web_search_mode,
            ?fallback_value,
            "review web_search_mode is disallowed by requirements; keeping constrained value"
        );
    }

    let session_telemetry = parent_turn_context
        .session_telemetry
        .clone()
        .with_model(model.as_str(), review_model_info.slug.as_str());
    let auth_manager_for_context = auth_manager.clone();
    let provider_for_context = provider.clone();
    let session_telemetry_for_context = session_telemetry.clone();
    let reasoning_effort = per_turn_config.model_reasoning_effort;
    let reasoning_summary = per_turn_config
        .model_reasoning_summary
        .unwrap_or(model_info.default_reasoning_summary);
    let session_source = parent_turn_context.session_source.clone();

    let per_turn_config = Arc::new(per_turn_config);
    let review_turn_id = sub_id.to_string();
    let turn_metadata_state = Arc::new(TurnMetadataState::new(
        sess.conversation_id.to_string(),
        review_turn_id.clone(),
        parent_turn_context.cwd.to_path_buf(),
        parent_turn_context.sandbox_policy.get(),
        parent_turn_context.windows_sandbox_level,
    ));

    let review_turn_context = TurnContext {
        sub_id: review_turn_id,
        trace_id: current_span_trace_id(),
        realtime_active: parent_turn_context.realtime_active,
        config: per_turn_config,
        auth_manager: auth_manager_for_context,
        model_info: model_info.clone(),
        session_telemetry: session_telemetry_for_context,
        provider: provider_for_context,
        reasoning_effort,
        reasoning_summary,
        session_source,
        environment: Arc::clone(&parent_turn_context.environment),
        tools_config,
        features: parent_turn_context.features.clone(),
        ghost_snapshot: parent_turn_context.ghost_snapshot.clone(),
        current_date: parent_turn_context.current_date.clone(),
        timezone: parent_turn_context.timezone.clone(),
        app_server_client_name: parent_turn_context.app_server_client_name.clone(),
        developer_instructions: None,
        user_instructions: None,
        compact_prompt: parent_turn_context.compact_prompt.clone(),
        collaboration_mode: parent_turn_context.collaboration_mode.clone(),
        model_fallback: parent_turn_context.model_fallback.clone(),
        personality: parent_turn_context.personality,
        approval_policy: parent_turn_context.approval_policy.clone(),
        sandbox_policy: parent_turn_context.sandbox_policy.clone(),
        file_system_sandbox_policy: parent_turn_context.file_system_sandbox_policy.clone(),
        network_sandbox_policy: parent_turn_context.network_sandbox_policy,
        network: parent_turn_context.network.clone(),
        windows_sandbox_level: parent_turn_context.windows_sandbox_level,
        shell_environment_policy: parent_turn_context.shell_environment_policy.clone(),
        cwd: parent_turn_context.cwd.clone(),
        final_output_json_schema: None,
        codex_self_exe: parent_turn_context.codex_self_exe.clone(),
        codex_linux_sandbox_exe: parent_turn_context.codex_linux_sandbox_exe.clone(),
        tool_call_gate: Arc::new(ReadinessFlag::new()),
        js_repl: Arc::clone(&sess.js_repl),
        dynamic_tools: parent_turn_context.dynamic_tools.clone(),
        truncation_policy: model_info.truncation_policy.into(),
        turn_metadata_state,
        turn_skills: TurnSkillsContext::new(parent_turn_context.turn_skills.outcome.clone()),
        turn_timing_state: Arc::new(TurnTimingState::default()),
    };

    // Seed the child task with the review prompt as the initial user message.
    let input: Vec<UserInput> = vec![UserInput::Text {
        text: review_prompt,
        // Review prompt is synthesized; no UI element ranges to preserve.
        text_elements: Vec::new(),
    }];
    let tc = Arc::new(review_turn_context);
    tc.turn_metadata_state.spawn_git_enrichment_task();
    // TODO(ccunningham): Review turns currently rely on `spawn_task` for TurnComplete but do not
    // emit a parent TurnStarted. Consider giving review a full parent turn lifecycle
    // (TurnStarted + TurnComplete) for consistency with other standalone tasks.
    sess.spawn_task(tc.clone(), input, ReviewTask::new()).await;

    // Announce entering review mode so UIs can switch modes.
    let review_request = ReviewRequest {
        target: resolved.target,
        user_facing_hint: Some(resolved.user_facing_hint),
    };
    sess.send_event(&tc, EventMsg::EnteredReviewMode(review_request))
        .await;
}

fn skills_to_info(
    skills: &[SkillMetadata],
    disabled_paths: &HashSet<PathBuf>,
) -> Vec<ProtocolSkillMetadata> {
    skills
        .iter()
        .map(|skill| ProtocolSkillMetadata {
            name: skill.name.clone(),
            description: skill.description.clone(),
            short_description: skill.short_description.clone(),
            interface: skill
                .interface
                .clone()
                .map(|interface| ProtocolSkillInterface {
                    display_name: interface.display_name,
                    short_description: interface.short_description,
                    icon_small: interface.icon_small,
                    icon_large: interface.icon_large,
                    brand_color: interface.brand_color,
                    default_prompt: interface.default_prompt,
                }),
            dependencies: skill.dependencies.clone().map(|dependencies| {
                ProtocolSkillDependencies {
                    tools: dependencies
                        .tools
                        .into_iter()
                        .map(|tool| ProtocolSkillToolDependency {
                            r#type: tool.r#type,
                            value: tool.value,
                            description: tool.description,
                            transport: tool.transport,
                            command: tool.command,
                            url: tool.url,
                        })
                        .collect(),
                }
            }),
            path: skill.path_to_skills_md.clone(),
            scope: skill.scope,
            enabled: !disabled_paths.contains(&skill.path_to_skills_md),
        })
        .collect()
}

fn errors_to_info(errors: &[SkillError]) -> Vec<SkillErrorInfo> {
    errors
        .iter()
        .map(|err| SkillErrorInfo {
            path: err.path.clone(),
            message: err.message.clone(),
        })
        .collect()
}

/// Takes a user message as input and runs a loop where, at each sampling request, the model
/// replies with either:
///
/// - requested function calls
/// - an assistant message
///
/// While it is possible for the model to return multiple of these items in a
/// single sampling request, in practice, we generally one item per sampling request:
///
/// - If the model requests a function call, we execute it and send the output
///   back to the model in the next sampling request.
/// - If the model sends only an assistant message, we record it in the
///   conversation history and consider the turn complete.
///
pub(crate) async fn run_turn(
    sess: Arc<Session>,
    mut turn_context: Arc<TurnContext>,
    input: Vec<UserInput>,
    prewarmed_client_session: Option<ModelClientSession>,
    cancellation_token: CancellationToken,
) -> Option<String> {
    if input.is_empty() && !sess.has_pending_input().await {
        return None;
    }

    let event = EventMsg::TurnStarted(TurnStartedEvent {
        turn_id: turn_context.sub_id.clone(),
        model_context_window: turn_context.model_context_window(),
        collaboration_mode_kind: turn_context.collaboration_mode.mode,
    });
    sess.send_event(&turn_context, event).await;
    // TODO(ccunningham): Pre-turn compaction runs before context updates and the
    // new user message are recorded. Estimate pending incoming items (context
    // diffs/full reinjection + user input) and trigger compaction preemptively
    // when they would push the thread over the compaction threshold.
    if run_pre_sampling_compact(&sess, &turn_context)
        .await
        .is_err()
    {
        error!("Failed to run pre-sampling compact");
        return None;
    }

    let turn_skills_outcome = turn_context.turn_skills.outcome.clone();

    sess.record_context_updates_and_set_reference_context_item(turn_context.as_ref())
        .await;

    let loaded_plugins = sess
        .services
        .plugins_manager
        .plugins_for_config(&turn_context.config);
    // Structured plugin:// mentions are resolved from the current session's
    // enabled plugins, then converted into turn-scoped guidance below.
    let mentioned_plugins =
        collect_explicit_plugin_mentions(&input, loaded_plugins.capability_summaries());
    let mcp_tools = if turn_context.apps_enabled() || !mentioned_plugins.is_empty() {
        // Plugin mentions need raw MCP/app inventory even when app tools
        // are normally hidden so we can describe the plugin's currently
        // usable capabilities for this turn.
        match sess
            .services
            .mcp_connection_manager
            .read()
            .await
            .list_all_tools()
            .or_cancel(&cancellation_token)
            .await
        {
            Ok(mcp_tools) => mcp_tools,
            Err(_) if turn_context.apps_enabled() => return None,
            Err(_) => HashMap::new(),
        }
    } else {
        HashMap::new()
    };
    let available_connectors = if turn_context.apps_enabled() {
        let connectors = connectors::merge_plugin_apps_with_accessible(
            loaded_plugins.effective_apps(),
            connectors::accessible_connectors_from_mcp_tools(&mcp_tools),
        );
        connectors::with_app_enabled_state(connectors, &turn_context.config)
    } else {
        Vec::new()
    };
    let connector_slug_counts = build_connector_slug_counts(&available_connectors);
    let skill_name_counts_lower = build_skill_name_counts(
        &turn_skills_outcome.skills,
        &turn_skills_outcome.disabled_paths,
    )
    .1;
    let mentioned_skills = collect_explicit_skill_mentions(
        &input,
        &turn_skills_outcome.skills,
        &turn_skills_outcome.disabled_paths,
        &connector_slug_counts,
    );
    let config = turn_context.config.clone();
    if config
        .features
        .enabled(Feature::SkillEnvVarDependencyPrompt)
    {
        let env_var_dependencies = collect_env_var_dependencies(&mentioned_skills);
        resolve_skill_dependencies_for_turn(&sess, &turn_context, &env_var_dependencies).await;
    }

    maybe_prompt_and_install_mcp_dependencies(
        sess.as_ref(),
        turn_context.as_ref(),
        &cancellation_token,
        &mentioned_skills,
    )
    .await;

    let session_telemetry = turn_context.session_telemetry.clone();
    let thread_id = sess.conversation_id.to_string();
    let tracking = build_track_events_context(
        turn_context.model_info.slug.clone(),
        thread_id,
        turn_context.sub_id.clone(),
    );
    let SkillInjections {
        items: skill_items,
        warnings: skill_warnings,
    } = build_skill_injections(
        &mentioned_skills,
        Some(&session_telemetry),
        &sess.services.analytics_events_client,
        tracking.clone(),
    )
    .await;

    for message in skill_warnings {
        sess.send_event(&turn_context, EventMsg::Warning(WarningEvent { message }))
            .await;
    }

    let plugin_items =
        build_plugin_injections(&mentioned_plugins, &mcp_tools, &available_connectors);
    let mentioned_plugin_metadata = mentioned_plugins
        .iter()
        .filter_map(crate::plugins::PluginCapabilitySummary::telemetry_metadata)
        .collect::<Vec<_>>();

    let mut explicitly_enabled_connectors = collect_explicit_app_ids(&input);
    explicitly_enabled_connectors.extend(collect_explicit_app_ids_from_skill_items(
        &skill_items,
        &available_connectors,
        &skill_name_counts_lower,
    ));
    let connector_names_by_id = available_connectors
        .iter()
        .map(|connector| (connector.id.as_str(), connector.name.as_str()))
        .collect::<HashMap<&str, &str>>();
    let mentioned_app_invocations = explicitly_enabled_connectors
        .iter()
        .map(|connector_id| AppInvocation {
            connector_id: Some(connector_id.clone()),
            app_name: connector_names_by_id
                .get(connector_id.as_str())
                .map(|name| (*name).to_string()),
            invocation_type: Some(InvocationType::Explicit),
        })
        .collect::<Vec<_>>();

    if run_pending_session_start_hooks(&sess, &turn_context).await {
        return None;
    }
    let additional_contexts = if input.is_empty() {
        Vec::new()
    } else {
        let initial_input_for_turn: ResponseInputItem = ResponseInputItem::from(input.clone());
        let response_item: ResponseItem = initial_input_for_turn.clone().into();
        let user_prompt_submit_outcome = run_user_prompt_submit_hooks(
            &sess,
            &turn_context,
            UserMessageItem::new(&input).message(),
        )
        .await;
        if user_prompt_submit_outcome.should_stop {
            record_additional_contexts(
                &sess,
                &turn_context,
                user_prompt_submit_outcome.additional_contexts,
            )
            .await;
            return None;
        }
        sess.record_user_prompt_and_emit_turn_item(turn_context.as_ref(), &input, response_item)
            .await;
        user_prompt_submit_outcome.additional_contexts
    };
    sess.services
        .analytics_events_client
        .track_app_mentioned(tracking.clone(), mentioned_app_invocations);
    for plugin in mentioned_plugin_metadata {
        sess.services
            .analytics_events_client
            .track_plugin_used(tracking.clone(), plugin);
    }
    sess.merge_connector_selection(explicitly_enabled_connectors.clone())
        .await;
    record_additional_contexts(&sess, &turn_context, additional_contexts).await;
    if !input.is_empty() {
        // Track the previous-turn baseline from the regular user-turn path only so
        // standalone tasks (compact/shell/review/undo) cannot suppress future
        // model/realtime injections.
        sess.set_previous_turn_settings(Some(PreviousTurnSettings {
            model: turn_context.model_info.slug.clone(),
            realtime_active: Some(turn_context.realtime_active),
        }))
        .await;
    }

    if !skill_items.is_empty() {
        sess.record_conversation_items(&turn_context, &skill_items)
            .await;
    }
    if !plugin_items.is_empty() {
        sess.record_conversation_items(&turn_context, &plugin_items)
            .await;
    }

    sess.maybe_start_ghost_snapshot(Arc::clone(&turn_context), cancellation_token.child_token())
        .await;
    let mut last_agent_message: Option<String> = None;
    let mut stop_hook_active = false;
    let mut stop_runtime_command_delivered_for_turn = false;
    // Although from the perspective of codex.rs, TurnDiffTracker has the lifecycle of a Task which contains
    // many turns, from the perspective of the user, it is a single turn.
    let turn_diff_tracker = Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new()));
    let mut server_model_warning_emitted_for_turn = false;
    let mut fallback_wait_deadline: Option<StdInstant> = None;
    let mut pending_fallback_success_step: Option<(
        String,
        NeroModelFallbackStep,
        Option<ReasoningEffortConfig>,
        String,
    )> = None;
    let model_fallback_delivery_log_path = if turn_context.model_fallback.is_some() {
        Some(
            sess.codex_home()
                .await
                .join("log")
                .join(NERO_HOOK_DELIVERY_LOG_FILENAME),
        )
    } else {
        None
    };

    // `ModelClientSession` is turn-scoped and caches WebSocket + sticky routing state, so we reuse
    // one instance across retries within this turn.
    let mut client_session =
        prewarmed_client_session.unwrap_or_else(|| sess.services.model_client.new_session());

    loop {
        if run_pending_session_start_hooks(&sess, &turn_context).await {
            break;
        }

        // Note that pending_input would be something like a message the user
        // submitted through the UI while the model was running. Though the UI
        // may support this, the model might not.
        let pending_input = sess.get_pending_input().await;

        let mut blocked_pending_input = false;
        let mut blocked_pending_input_contexts = Vec::new();
        let mut requeued_pending_input = false;
        let mut accepted_pending_input = Vec::new();
        if !pending_input.is_empty() {
            let mut pending_input_iter = pending_input.into_iter();
            while let Some(pending_input_item) = pending_input_iter.next() {
                match inspect_pending_input(&sess, &turn_context, pending_input_item).await {
                    PendingInputHookDisposition::Accepted(pending_input) => {
                        accepted_pending_input.push(*pending_input);
                    }
                    PendingInputHookDisposition::Blocked {
                        additional_contexts,
                    } => {
                        let remaining_pending_input = pending_input_iter.collect::<Vec<_>>();
                        if !remaining_pending_input.is_empty() {
                            let _ = sess.prepend_pending_input(remaining_pending_input).await;
                            requeued_pending_input = true;
                        }
                        blocked_pending_input_contexts = additional_contexts;
                        blocked_pending_input = true;
                        break;
                    }
                }
            }
        }

        let has_accepted_pending_input = !accepted_pending_input.is_empty();
        for pending_input in accepted_pending_input {
            record_pending_input(&sess, &turn_context, pending_input).await;
        }
        record_additional_contexts(&sess, &turn_context, blocked_pending_input_contexts).await;

        if blocked_pending_input && !has_accepted_pending_input {
            if requeued_pending_input {
                continue;
            }
            break;
        }

        // Construct the input that we will send to the model.
        let sampling_request_input: Vec<ResponseItem> = {
            sess.clone_history()
                .await
                .for_prompt(&turn_context.model_info.input_modalities)
        };

        let sampling_request_input_messages = sampling_request_input
            .iter()
            .filter_map(|item| match parse_turn_item(item) {
                Some(TurnItem::UserMessage(user_message)) => Some(user_message),
                _ => None,
            })
            .map(|user_message| user_message.message())
            .collect::<Vec<String>>();
        let turn_metadata_header = turn_context.turn_metadata_state.current_header_value();
        let skills_outcome = Some(turn_context.turn_skills.outcome.as_ref());
        match run_sampling_request(
            Arc::clone(&sess),
            Arc::clone(&turn_context),
            Arc::clone(&turn_diff_tracker),
            &mut client_session,
            turn_metadata_header.as_deref(),
            sampling_request_input,
            &explicitly_enabled_connectors,
            skills_outcome,
            &mut server_model_warning_emitted_for_turn,
            cancellation_token.child_token(),
        )
        .await
        {
            Ok(sampling_request_output) => {
                if let Some((from_model, step, resolved_effort, trigger_error)) =
                    pending_fallback_success_step.take()
                    && let Some(model_fallback) = turn_context.model_fallback.as_ref()
                {
                    append_nero_model_fallback_audit(
                        model_fallback_delivery_log_path.as_deref(),
                        &sess.conversation_id,
                        &turn_context,
                        "success",
                        json!({
                            "from_model": from_model,
                            "to_model": step.model.clone(),
                            "to_reasoning_effort": model_fallback_reasoning_label(resolved_effort),
                            "requested_reasoning_effort": model_fallback_reasoning_label(Some(step.reasoning_effort)),
                            "trigger_error": trigger_error,
                            "sticky": model_fallback.sticky,
                        }),
                    )
                    .await;
                    sess.mark_model_fallback_success(model_fallback, &step)
                        .await;
                }
                let SamplingRequestResult {
                    needs_follow_up,
                    last_agent_message: sampling_request_last_agent_message,
                } = sampling_request_output;
                let auto_compact_limit = turn_context
                    .model_info
                    .auto_compact_token_limit()
                    .unwrap_or(i64::MAX);
                let total_usage_tokens = sess.get_total_token_usage().await;
                let token_limit_reached = total_usage_tokens >= auto_compact_limit;

                let estimated_token_count =
                    sess.get_estimated_token_count(turn_context.as_ref()).await;

                trace!(
                    turn_id = %turn_context.sub_id,
                    total_usage_tokens,
                    estimated_token_count = ?estimated_token_count,
                    auto_compact_limit,
                    token_limit_reached,
                    needs_follow_up,
                    "post sampling token usage"
                );

                // as long as compaction works well in getting us way below the token limit, we shouldn't worry about being in an infinite loop.
                if token_limit_reached && needs_follow_up {
                    if run_auto_compact(
                        &sess,
                        &turn_context,
                        InitialContextInjection::BeforeLastUserMessage,
                    )
                    .await
                    .is_err()
                    {
                        return None;
                    }
                    continue;
                }

                if !needs_follow_up {
                    last_agent_message = sampling_request_last_agent_message;
                    let codex_home = sess.codex_home().await;
                    let hook_delivery_log_path =
                        codex_home.join("log").join(NERO_HOOK_DELIVERY_LOG_FILENAME);
                    let stop_hook_debug_reporting = resolve_stop_hook_debug_reporting_mode(
                        &resolve_nero_auto_config_path(&codex_home),
                    );
                    let configured_after_agent_hooks = sess.hooks().after_agent_hook_count();
                    let (hook_thread_name, hook_nero_auto_runtime) = {
                        let state = sess.state.lock().await;
                        (
                            state.session_configuration.thread_name.clone(),
                            effective_nero_auto_runtime(
                                state.session_configuration.nero_auto_runtime,
                                &turn_context.session_source,
                            ),
                        )
                    };
                    let stop_hook_permission_mode = match turn_context.approval_policy.value() {
                        AskForApproval::Never => "bypassPermissions",
                        AskForApproval::UnlessTrusted
                        | AskForApproval::OnFailure
                        | AskForApproval::OnRequest
                        | AskForApproval::Granular(_) => "default",
                    }
                    .to_string();
                    let stop_request = codex_hooks::StopRequest {
                        session_id: sess.conversation_id,
                        turn_id: turn_context.sub_id.clone(),
                        cwd: turn_context.cwd.to_path_buf(),
                        transcript_path: sess.hook_transcript_path().await,
                        model: turn_context.model_info.slug.clone(),
                        permission_mode: stop_hook_permission_mode,
                        stop_hook_active,
                        last_assistant_message: last_agent_message.clone(),
                    };
                    for run in sess.hooks().preview_stop(&stop_request) {
                        sess.send_event(
                            &turn_context,
                            EventMsg::HookStarted(crate::protocol::HookStartedEvent {
                                turn_id: Some(turn_context.sub_id.clone()),
                                run,
                            }),
                        )
                        .await;
                    }
                    let stop_outcome = sess.hooks().run_stop(stop_request).await;
                    for completed in stop_outcome.hook_events {
                        sess.send_event(&turn_context, EventMsg::HookCompleted(completed))
                            .await;
                    }
                    if stop_outcome.should_block {
                        if let Some(hook_prompt_message) =
                            build_hook_prompt_message(&stop_outcome.continuation_fragments)
                        {
                            sess.record_conversation_items(
                                &turn_context,
                                std::slice::from_ref(&hook_prompt_message),
                            )
                            .await;
                            if let Some(message) = stop_hook_debug_warning_message(
                                stop_hook_debug_reporting,
                                &hook_prompt_message,
                            ) {
                                sess.send_event(
                                    &turn_context,
                                    EventMsg::Warning(WarningEvent { message }),
                                )
                                .await;
                            }
                            stop_runtime_command_delivered_for_turn = true;
                            stop_hook_active = true;
                            continue;
                        } else {
                            warn!(
                                turn_id = %turn_context.sub_id,
                                "stop hook requested continuation but no prompt could be built; aborting turn (fail-closed)"
                            );
                            sess.send_event(
                                &turn_context,
                                EventMsg::Error(ErrorEvent {
                                    message: "Stop hook requested continuation without a prompt; aborting turn.".to_string(),
                                    codex_error_info: None,
                                }),
                            )
                            .await;
                            return None;
                        }
                    }
                    if stop_outcome.should_stop {
                        break;
                    }
                    let hook_outcomes = sess
                        .hooks()
                        .dispatch(HookPayload {
                            session_id: sess.conversation_id,
                            cwd: turn_context.cwd.to_path_buf(),
                            client: turn_context.app_server_client_name.clone(),
                            session_source: Some(turn_context.session_source.to_string()),
                            session_agent_role: turn_context.session_source.get_agent_role(),
                            nero_auto_runtime: Some(hook_nero_auto_runtime),
                            triggered_at: chrono::Utc::now(),
                            hook_event: HookEvent::AfterAgent {
                                event: HookEventAfterAgent {
                                    thread_id: sess.conversation_id,
                                    thread_name: hook_thread_name,
                                    turn_id: turn_context.sub_id.clone(),
                                    input_messages: sampling_request_input_messages,
                                    last_assistant_message: last_agent_message.clone(),
                                },
                            },
                        })
                        .await;
                    debug!(
                        turn_id = %turn_context.sub_id,
                        hook_outcomes = hook_outcomes.len(),
                        "after_agent hooks dispatched"
                    );
                    let failed_hook_outcomes = hook_outcomes
                        .iter()
                        .filter(|outcome| !matches!(&outcome.result, HookResult::Success))
                        .count();
                    let all_after_agent_hooks_failed_without_actions = configured_after_agent_hooks
                        > 0
                        && !hook_outcomes.is_empty()
                        && hook_outcomes.iter().all(|outcome| {
                            !matches!(&outcome.result, HookResult::Success)
                                && outcome.actions.is_empty()
                        });
                    let all_after_agent_hooks_failed = configured_after_agent_hooks > 0
                        && failed_hook_outcomes == hook_outcomes.len();
                    let hook_dispatch_status = if configured_after_agent_hooks == 0 {
                        "no_hooks_configured"
                    } else if hook_outcomes.is_empty() {
                        "no_outcomes"
                    } else if all_after_agent_hooks_failed_without_actions {
                        "all_failed_no_actions"
                    } else if all_after_agent_hooks_failed {
                        "all_failed"
                    } else {
                        "executed"
                    };
                    append_nero_hook_delivery_audit(
                        &hook_delivery_log_path,
                        &sess.conversation_id,
                        turn_context.as_ref(),
                        "after_agent_hook_registry",
                        "hook_dispatch",
                        hook_dispatch_status,
                        /*delivered_agent*/ false,
                        /*delivered_tui*/ false,
                        json!({
                            "configured_after_agent_hooks": configured_after_agent_hooks,
                            "hook_outcomes": hook_outcomes.len(),
                            "failed_hook_outcomes": failed_hook_outcomes,
                            "session_source": turn_context.session_source.to_string(),
                            "hook_runtime_auto_enabled": hook_nero_auto_runtime.enabled,
                            "hook_runtime_auto_autonomy_level": hook_nero_auto_runtime.autonomy_level,
                            "hook_runtime_auto_max_rounds": hook_nero_auto_runtime.max_auto_rounds,
                        }),
                    )
                    .await;

                    if all_after_agent_hooks_failed_without_actions
                        && configured_after_agent_hooks > 0
                        && !matches!(turn_context.session_source, SessionSource::SubAgent(_))
                    {
                        let message = nero_hook_tui_warning_message(
                            "All configured after_agent hooks failed and returned no actions for this turn.",
                            NeroHookMsgFormat::Block,
                            Some(("error", "dispatch-all-failed")),
                        );
                        sess.send_event(&turn_context, EventMsg::Warning(WarningEvent { message }))
                            .await;
                    }

                    let mut abort_message = None;
                    let mut deferred_auto_user_replies: Vec<(String, String, Option<u64>)> =
                        Vec::new();
                    let mut auto_user_reply_selected_for_turn = false;
                    for hook_outcome in hook_outcomes {
                        let hook_name = hook_outcome.hook_name;
                        let result = hook_outcome.result;
                        let actions = hook_outcome.actions;
                        if matches!(&result, HookResult::Success | HookResult::FailedContinue(_)) {
                            debug!(
                                turn_id = %turn_context.sub_id,
                                hook_name = %hook_name,
                                actions = actions.len(),
                                "processing after_agent hook actions"
                            );
                            let mut hook_stop_checkpoint_expected = false;
                            let hook_stop_checkpoint_delivered =
                                stop_runtime_command_delivered_for_turn;
                            let mut hook_nero_msg_total = 0usize;
                            let mut hook_nero_msg_throttled = 0usize;
                            let mut hook_auto_user_replies_pending: Vec<(String, Option<u64>)> =
                                Vec::new();
                            let mut hook_auto_user_replies_blocked = 0usize;
                            let mut hook_auto_user_replies_queued = 0usize;
                            let mut hook_auto_user_reply_expected_wait_seconds = None::<u64>;
                            let mut hook_auto_user_reply_generation_epoch = None::<u64>;
                            let mut latest_runtime_status_kind_normalized = None::<String>;
                            let mut latest_runtime_status_meta = None::<Value>;
                            let mut after_agent_summary_entries =
                                Vec::<codex_protocol::protocol::HookOutputEntry>::new();
                            for action in actions {
                                match action {
                                    NeroHookAction::NeroHookMsg {
                                        mode,
                                        show,
                                        freq,
                                        format,
                                        status,
                                        msg,
                                    } => {
                                        if session_source_blocks_nero_msg_auto_lane(
                                            &turn_context.session_source,
                                        ) {
                                            debug!(
                                                turn_id = %turn_context.sub_id,
                                                hook_name = %hook_name,
                                                session_source = %turn_context.session_source,
                                                "ignored nero_hook_msg hook action for subagent session"
                                            );
                                            append_nero_hook_delivery_audit(
                                                &hook_delivery_log_path,
                                                &sess.conversation_id,
                                                turn_context.as_ref(),
                                                &hook_name,
                                                "nero_hook_msg",
                                                "ignored-subagent-session",
                                                /*delivered_agent*/ false,
                                                /*delivered_tui*/ false,
                                                json!({
                                                    "mode": nero_hook_mode_label(&mode),
                                                    "format": nero_hook_format_label(&format),
                                                    "freq_seconds": freq,
                                                    "show": {
                                                        "agent": show.agent,
                                                        "tui": show.tui,
                                                    },
                                                    "message_len": {
                                                        "full": msg.full.len(),
                                                        "short": msg.short.len(),
                                                    },
                                                    "status": {
                                                        "kind": status.as_ref().map(|item| item.kind.clone()),
                                                        "text": status.as_ref().map(|item| item.text.clone()),
                                                        "meta": sanitize_nero_hook_status_meta_for_audit(
                                                            status.as_ref().and_then(|item| item.meta.clone()),
                                                        ),
                                                    },
                                                }),
                                            )
                                            .await;
                                            continue;
                                        }
                                        hook_nero_msg_total = hook_nero_msg_total.saturating_add(1);
                                        let msg_full_len = msg.full.len();
                                        let msg_short_len = msg.short.len();
                                        let status_kind =
                                            status.as_ref().map(|item| item.kind.clone());
                                        let status_text =
                                            status.as_ref().map(|item| item.text.clone());
                                        let status_meta = sanitize_nero_hook_status_meta_for_audit(
                                            status.as_ref().and_then(|item| item.meta.clone()),
                                        );
                                        let status_kind_normalized =
                                            normalized_nero_hook_status_kind(status.as_ref());
                                        if let Some(remaining) = sess
                                            .nero_hook_msg_throttle_remaining(
                                                &hook_name,
                                                &mode,
                                                &format,
                                                show.agent,
                                                show.tui,
                                                &msg.full,
                                                &msg.short,
                                                status.as_ref(),
                                                freq,
                                            )
                                        {
                                            if show.tui {
                                                let remaining_secs =
                                                    nero_hook_msg_remaining_secs_ceil(remaining);
                                                let content = format!("throttled (freq={freq}s)");
                                                let countdown =
                                                    format!("next update in {remaining_secs}s");
                                                let message = nero_hook_tui_warning_message(
                                                    &content,
                                                    NeroHookMsgFormat::Block,
                                                    Some(("countdown", &countdown)),
                                                );
                                                sess.send_event(
                                                    &turn_context,
                                                    EventMsg::Warning(WarningEvent { message }),
                                                )
                                                .await;
                                            }
                                            debug!(
                                                turn_id = %turn_context.sub_id,
                                                hook_name = %hook_name,
                                                show_agent = show.agent,
                                                show_tui = show.tui,
                                                ?mode,
                                                freq,
                                                remaining_ms = remaining.as_millis(),
                                                "skipped nero_hook_msg due to freq throttle"
                                            );
                                            hook_nero_msg_throttled =
                                                hook_nero_msg_throttled.saturating_add(1);
                                            append_nero_hook_delivery_audit(
                                                &hook_delivery_log_path,
                                                &sess.conversation_id,
                                                turn_context.as_ref(),
                                                &hook_name,
                                                "nero_hook_msg",
                                                "throttled",
                                                /*delivered_agent*/ false,
                                                /*delivered_tui*/ show.tui,
                                                json!({
                                                    "mode": nero_hook_mode_label(&mode),
                                                    "format": nero_hook_format_label(&format),
                                                    "freq_seconds": freq,
                                                    "show": {
                                                        "agent": show.agent,
                                                        "tui": show.tui,
                                                    },
                                                    "remaining_ms": remaining.as_millis(),
                                                    "message_len": {
                                                        "full": msg_full_len,
                                                        "short": msg_short_len,
                                                    },
                                                    "status": {
                                                        "kind": status_kind,
                                                        "text": status_text,
                                                        "meta": status_meta,
                                                    },
                                                    "delivery_contract": {
                                                        "status_kind_normalized": status_kind_normalized,
                                                        "stop_runtime_command_delivered": stop_runtime_command_delivered_for_turn,
                                                    },
                                                }),
                                            )
                                            .await;
                                            continue;
                                        }
                                        latest_runtime_status_kind_normalized =
                                            status_kind_normalized.clone();
                                        merge_after_agent_runtime_status_meta(
                                            &mut latest_runtime_status_meta,
                                            status_meta.clone(),
                                        );
                                        let mut delivered_tui = false;
                                        let delivered_agent = false;
                                        if show.tui {
                                            let tui_body = match mode {
                                                NeroHookMsgMode::Synced => msg.full.clone(),
                                                NeroHookMsgMode::TuiShort => msg.short.clone(),
                                            };
                                            let delivery = nero_hook_tui_delivery(
                                                &tui_body,
                                                format,
                                                status.as_ref(),
                                                status_kind_normalized.as_deref(),
                                            );
                                            if let Some(message) = delivery.warning {
                                                sess.send_event(
                                                    &turn_context,
                                                    EventMsg::Warning(WarningEvent { message }),
                                                )
                                                .await;
                                            }
                                            if let Some(entry) = delivery.hook_summary {
                                                after_agent_summary_entries.push(entry);
                                            }
                                            delivered_tui = true;
                                        }
                                        if show.agent {
                                            debug!(
                                                turn_id = %turn_context.sub_id,
                                                hook_name = %hook_name,
                                                "ignored agent-facing nero_hook_msg injection in stop-centric flow"
                                            );
                                        }
                                        debug!(
                                            turn_id = %turn_context.sub_id,
                                            hook_name = %hook_name,
                                            show_agent = show.agent,
                                            show_tui = show.tui,
                                            ?mode,
                                            freq,
                                            "executed nero_hook_msg"
                                        );
                                        let nero_hook_msg_status = if show.agent {
                                            "executed-agent-ignored-stop-centric"
                                        } else {
                                            "executed"
                                        };
                                        append_nero_hook_delivery_audit(
                                            &hook_delivery_log_path,
                                            &sess.conversation_id,
                                            turn_context.as_ref(),
                                            &hook_name,
                                            "nero_hook_msg",
                                            nero_hook_msg_status,
                                            delivered_agent,
                                            delivered_tui,
                                            json!({
                                                "mode": nero_hook_mode_label(&mode),
                                                "format": nero_hook_format_label(&format),
                                                "freq_seconds": freq,
                                                "show": {
                                                    "agent": show.agent,
                                                    "tui": show.tui,
                                                },
                                                "message_len": {
                                                    "full": msg_full_len,
                                                    "short": msg_short_len,
                                                },
                                                "status": {
                                                    "kind": status_kind,
                                                    "text": status_text,
                                                    "meta": status_meta,
                                                },
                                                "delivery_contract": {
                                                    "status_kind_normalized": status_kind_normalized,
                                                    "stop_checkpoint_delivered_so_far": hook_stop_checkpoint_delivered,
                                                    "stop_runtime_command_delivered": stop_runtime_command_delivered_for_turn,
                                                },
                                            }),
                                        )
                                        .await;
                                    }
                                    NeroHookAction::VisibleNote { message } => {
                                        let message_len = message.len();
                                        let message = if message.starts_with("[nero-hook]") {
                                            message
                                        } else {
                                            format!("[nero-hook] {message}")
                                        };
                                        sess.send_event(
                                            &turn_context,
                                            EventMsg::Warning(WarningEvent { message }),
                                        )
                                        .await;
                                        debug!(
                                            turn_id = %turn_context.sub_id,
                                            hook_name = %hook_name,
                                            "emitted visible_note warning event"
                                        );
                                        append_nero_hook_delivery_audit(
                                            &hook_delivery_log_path,
                                            &sess.conversation_id,
                                            turn_context.as_ref(),
                                            &hook_name,
                                            "visible_note",
                                            "executed",
                                            /*delivered_agent*/ false,
                                            /*delivered_tui*/ true,
                                            json!({
                                                "message_len": message_len,
                                            }),
                                        )
                                        .await;
                                    }
                                    NeroHookAction::AutoUserReply {
                                        message,
                                        expected_wait_seconds,
                                    } => {
                                        if session_source_blocks_nero_msg_auto_lane(
                                            &turn_context.session_source,
                                        ) {
                                            debug!(
                                                turn_id = %turn_context.sub_id,
                                                hook_name = %hook_name,
                                                session_source = %turn_context.session_source,
                                                "ignored auto_user_reply hook action for subagent session"
                                            );
                                            append_nero_hook_delivery_audit(
                                                &hook_delivery_log_path,
                                                &sess.conversation_id,
                                                turn_context.as_ref(),
                                                &hook_name,
                                                "auto_user_reply",
                                                "ignored-subagent-session",
                                                /*delivered_agent*/ false,
                                                /*delivered_tui*/ false,
                                                json!({
                                                    "message_len": message.len(),
                                                    "expected_wait_seconds": expected_wait_seconds,
                                                    "auto_stage": {
                                                        "stage": "follow_up",
                                                    },
                                                }),
                                            )
                                            .await;
                                            continue;
                                        }
                                        if auto_user_reply_selected_for_turn {
                                            debug!(
                                                turn_id = %turn_context.sub_id,
                                                hook_name = %hook_name,
                                                "ignored duplicate auto_user_reply hook action for current turn"
                                            );
                                            append_nero_hook_delivery_audit(
                                                &hook_delivery_log_path,
                                                &sess.conversation_id,
                                                turn_context.as_ref(),
                                                &hook_name,
                                                "auto_user_reply",
                                                "ignored-duplicate-turn",
                                                /*delivered_agent*/ false,
                                                /*delivered_tui*/ false,
                                                json!({
                                                    "message_len": message.len(),
                                                    "expected_wait_seconds": expected_wait_seconds,
                                                    "auto_stage": {
                                                        "stage": "follow_up",
                                                    },
                                                }),
                                            )
                                            .await;
                                            continue;
                                        }
                                        auto_user_reply_selected_for_turn = true;
                                        hook_stop_checkpoint_expected = true;
                                        debug!(
                                            turn_id = %turn_context.sub_id,
                                            hook_name = %hook_name,
                                            "queued pending auto_user_reply from hook (awaiting delivery contract)"
                                        );
                                        append_nero_hook_delivery_audit(
                                            &hook_delivery_log_path,
                                            &sess.conversation_id,
                                            turn_context.as_ref(),
                                            &hook_name,
                                            "auto_user_reply",
                                            "pending-delivery-contract",
                                            /*delivered_agent*/ false,
                                            /*delivered_tui*/ false,
                                            json!({
                                                "message_len": message.len(),
                                                "expected_wait_seconds": expected_wait_seconds,
                                                "auto_stage": {
                                                    "stage": "follow_up",
                                                },
                                            }),
                                        )
                                        .await;
                                        hook_auto_user_replies_pending
                                            .push((message, expected_wait_seconds));
                                    }
                                }
                            }
                            let hook_delivery_contract_satisfied =
                                runtime_delivery_contract_satisfied(
                                    hook_stop_checkpoint_expected,
                                    hook_stop_checkpoint_delivered,
                                );
                            if !hook_auto_user_replies_pending.is_empty() {
                                if hook_delivery_contract_satisfied {
                                    hook_auto_user_reply_generation_epoch =
                                        Some(sess.current_hook_auto_reply_epoch());
                                    for (message, expected_wait_seconds) in
                                        hook_auto_user_replies_pending
                                    {
                                        if let Some(expected_wait_seconds) =
                                            Session::normalize_hook_auto_reply_wait_seconds(
                                                expected_wait_seconds,
                                            )
                                        {
                                            hook_auto_user_reply_expected_wait_seconds = Some(
                                                match hook_auto_user_reply_expected_wait_seconds {
                                                    Some(current) => {
                                                        current.min(expected_wait_seconds)
                                                    }
                                                    None => expected_wait_seconds,
                                                },
                                            );
                                        }
                                        hook_auto_user_replies_queued =
                                            hook_auto_user_replies_queued.saturating_add(1);
                                        append_nero_hook_delivery_audit(
                                            &hook_delivery_log_path,
                                            &sess.conversation_id,
                                            turn_context.as_ref(),
                                            &hook_name,
                                            "auto_user_reply",
                                            "queued",
                                            /*delivered_agent*/ false,
                                            /*delivered_tui*/ false,
                                            json!({
                                                "message_len": message.len(),
                                                "expected_wait_seconds": expected_wait_seconds,
                                                "auto_stage": {
                                                    "stage": "follow_up",
                                                },
                                                "delivery_contract": {
                                                    "stop_checkpoint_expected": hook_stop_checkpoint_expected,
                                                    "stop_checkpoint_delivered": hook_stop_checkpoint_delivered,
                                                    "contract_satisfied": hook_delivery_contract_satisfied,
                                                },
                                            }),
                                        )
                                        .await;
                                        deferred_auto_user_replies.push((
                                            hook_name.clone(),
                                            message,
                                            expected_wait_seconds,
                                        ));
                                    }
                                } else {
                                    hook_auto_user_replies_blocked =
                                        hook_auto_user_replies_pending.len();
                                    warn!(
                                        turn_id = %turn_context.sub_id,
                                        hook_name = %hook_name,
                                        pending_auto_user_replies = hook_auto_user_replies_pending.len(),
                                        stop_checkpoint_expected = hook_stop_checkpoint_expected,
                                        stop_checkpoint_delivered = hook_stop_checkpoint_delivered,
                                        "blocked auto_user_reply actions because STOP checkpoint delivery was not confirmed for current turn"
                                    );
                                    let contract_warning = nero_hook_tui_warning_message(
                                        "Auto delivery blocked: STOP checkpoint was not delivered in this turn.",
                                        NeroHookMsgFormat::Block,
                                        Some(("error", "delivery-contract")),
                                    );
                                    sess.send_event(
                                        &turn_context,
                                        EventMsg::Warning(WarningEvent {
                                            message: contract_warning,
                                        }),
                                    )
                                    .await;
                                    for (message, expected_wait_seconds) in
                                        hook_auto_user_replies_pending
                                    {
                                        append_nero_hook_delivery_audit(
                                            &hook_delivery_log_path,
                                            &sess.conversation_id,
                                            turn_context.as_ref(),
                                            &hook_name,
                                            "auto_user_reply",
                                            "blocked-delivery-contract",
                                            /*delivered_agent*/ false,
                                            /*delivered_tui*/ false,
                                            json!({
                                                "message_len": message.len(),
                                                "expected_wait_seconds": expected_wait_seconds,
                                                "auto_stage": {
                                                    "stage": "follow_up",
                                                },
                                                "delivery_contract": {
                                                    "stop_checkpoint_expected": hook_stop_checkpoint_expected,
                                                    "stop_checkpoint_delivered": hook_stop_checkpoint_delivered,
                                                    "contract_satisfied": hook_delivery_contract_satisfied,
                                                },
                                            }),
                                        )
                                        .await;
                                    }
                                }
                            }
                            align_auto_decision_meta_with_delivery_contract(
                                &mut latest_runtime_status_meta,
                                hook_delivery_contract_satisfied,
                                hook_auto_user_replies_blocked,
                            );
                            let contract_status = stop_delivery_contract_status(
                                hook_delivery_contract_satisfied,
                                hook_auto_user_replies_blocked,
                            );
                            append_nero_hook_delivery_audit(
                                &hook_delivery_log_path,
                                &sess.conversation_id,
                                turn_context.as_ref(),
                                &hook_name,
                                "delivery_contract",
                                contract_status,
                                /*delivered_agent*/ hook_stop_checkpoint_delivered,
                                /*delivered_tui*/ false,
                                json!({
                                    "auto_stage": {
                                        "stage": "protocol",
                                    },
                                    "stop_checkpoint_expected": hook_stop_checkpoint_expected,
                                    "stop_checkpoint_delivered": hook_stop_checkpoint_delivered,
                                    "contract_satisfied": hook_delivery_contract_satisfied,
                                    "nero_hook_msg_total": hook_nero_msg_total,
                                    "nero_hook_msg_throttled": hook_nero_msg_throttled,
                                    "auto_user_replies_blocked": hook_auto_user_replies_blocked,
                                }),
                            )
                            .await;
                            let has_runtime_signal = !after_agent_summary_entries.is_empty()
                                || latest_runtime_status_kind_normalized.is_some()
                                || latest_runtime_status_meta.is_some()
                                || hook_stop_checkpoint_expected
                                || hook_stop_checkpoint_delivered
                                || !hook_delivery_contract_satisfied
                                || hook_nero_msg_total > 0
                                || hook_nero_msg_throttled > 0
                                || hook_auto_user_replies_queued > 0
                                || hook_auto_user_replies_blocked > 0
                                || matches!(&result, HookResult::FailedContinue(_));
                            let runtime_summary_meta = has_runtime_signal.then(|| {
                                after_agent_runtime_hook_summary_meta(
                                    &hook_name,
                                    latest_runtime_status_kind_normalized,
                                    latest_runtime_status_meta,
                                    contract_status,
                                    hook_stop_checkpoint_expected,
                                    hook_stop_checkpoint_delivered,
                                    hook_delivery_contract_satisfied,
                                    hook_nero_msg_total,
                                    hook_nero_msg_throttled,
                                    hook_auto_user_replies_queued,
                                    hook_auto_user_replies_blocked,
                                    hook_auto_user_reply_expected_wait_seconds,
                                    hook_auto_user_reply_generation_epoch,
                                )
                            });
                            let (runtime_event_status, runtime_event_status_message) =
                                if matches!(&result, HookResult::FailedContinue(_)) {
                                    (
                                        codex_protocol::protocol::HookRunStatus::Failed,
                                        Some(
                                            "after_agent runtime status (hook failed_continue)"
                                                .to_string(),
                                        ),
                                    )
                                } else {
                                    (
                                        codex_protocol::protocol::HookRunStatus::Completed,
                                        Some("after_agent runtime status".to_string()),
                                    )
                                };
                            if let Some(event) = after_agent_runtime_hook_completed_event(
                                &turn_context.sub_id,
                                &hook_name,
                                runtime_event_status,
                                runtime_event_status_message,
                                runtime_summary_meta,
                                after_agent_summary_entries,
                            ) {
                                sess.send_event(&turn_context, EventMsg::HookCompleted(event))
                                    .await;
                            }
                        }
                        match result {
                            HookResult::Success => {}
                            HookResult::FailedContinue(error) => {
                                warn!(
                                    turn_id = %turn_context.sub_id,
                                    hook_name = %hook_name,
                                    error = %error,
                                    "after_agent hook failed; continuing"
                                );
                                append_nero_hook_delivery_audit(
                                    &hook_delivery_log_path,
                                    &sess.conversation_id,
                                    turn_context.as_ref(),
                                    &hook_name,
                                    "hook_execution",
                                    "failed-continue",
                                    /*delivered_agent*/ false,
                                    /*delivered_tui*/ false,
                                    json!({
                                        "error": error.to_string(),
                                    }),
                                )
                                .await;
                            }
                            HookResult::FailedAbort(error) => {
                                let message = format!(
                                    "after_agent hook '{hook_name}' failed and aborted turn completion: {error}"
                                );
                                warn!(
                                    turn_id = %turn_context.sub_id,
                                    hook_name = %hook_name,
                                    error = %error,
                                    "after_agent hook failed; aborting operation"
                                );
                                append_nero_hook_delivery_audit(
                                    &hook_delivery_log_path,
                                    &sess.conversation_id,
                                    turn_context.as_ref(),
                                    &hook_name,
                                    "hook_execution",
                                    "failed-abort",
                                    /*delivered_agent*/ false,
                                    /*delivered_tui*/ false,
                                    json!({
                                        "error": error.to_string(),
                                    }),
                                )
                                .await;
                                if abort_message.is_none() {
                                    abort_message = Some(message);
                                }
                            }
                        }
                    }
                    if let Some(message) = abort_message {
                        sess.send_event(
                            &turn_context,
                            EventMsg::Error(ErrorEvent {
                                message,
                                codex_error_info: None,
                            }),
                        )
                        .await;
                        return None;
                    }
                    for (hook_name, message, expected_wait_seconds) in deferred_auto_user_replies {
                        sess.spawn_deferred_auto_user_reply(
                            turn_context.sub_id.clone(),
                            hook_name,
                            message,
                            expected_wait_seconds,
                            turn_context.session_source.clone(),
                        );
                    }
                    break;
                }
                continue;
            }
            Err(CodexErr::TurnAborted) => {
                // Aborted turn is reported via a different event.
                break;
            }
            Err(CodexErr::InvalidImageRequest()) => {
                let mut state = sess.state.lock().await;
                error_or_panic(
                    "Invalid image detected; sanitizing tool output to prevent poisoning",
                );
                if state.history.replace_last_turn_images("Invalid image") {
                    continue;
                }
                let event = EventMsg::Error(ErrorEvent {
                    message: "Invalid image in your last message. Please remove it and try again."
                        .to_string(),
                    codex_error_info: Some(CodexErrorInfo::BadRequest),
                });
                sess.send_event(&turn_context, event).await;
                break;
            }
            Err(e) => {
                if should_trigger_model_fallback(&e) && turn_context.model_fallback.is_some() {
                    match sess
                        .try_model_fallback_after_error(
                            &turn_context,
                            &e,
                            &mut fallback_wait_deadline,
                            &cancellation_token,
                            model_fallback_delivery_log_path.as_deref(),
                        )
                        .await
                    {
                        Ok(ModelFallbackAfterErrorOutcome::Switched(next_turn_context, step)) => {
                            let from_model = turn_context.model_info.slug.clone();
                            let resolved_effort = next_turn_context.reasoning_effort;
                            if let Some((
                                previous_from_model,
                                previous_step,
                                previous_resolved_effort,
                                previous_trigger_error,
                            )) = pending_fallback_success_step.take()
                            {
                                let previous_to_model = previous_step.model;
                                let previous_requested_effort = previous_step.reasoning_effort;
                                append_nero_model_fallback_audit(
                                    model_fallback_delivery_log_path.as_deref(),
                                    &sess.conversation_id,
                                    &turn_context,
                                    "superseded",
                                    json!({
                                        "from_model": previous_from_model,
                                        "to_model": previous_to_model,
                                        "to_reasoning_effort": model_fallback_reasoning_label(previous_resolved_effort),
                                        "requested_reasoning_effort": model_fallback_reasoning_label(Some(previous_requested_effort)),
                                        "trigger_error": previous_trigger_error,
                                        "superseded_by_model": step.model.clone(),
                                        "superseded_by_reasoning_effort": model_fallback_reasoning_label(resolved_effort),
                                    }),
                                )
                                .await;
                            }
                            turn_context = next_turn_context;
                            pending_fallback_success_step =
                                Some((from_model, step, resolved_effort, e.to_string()));
                            client_session = sess.services.model_client.new_session();
                            continue;
                        }
                        Ok(ModelFallbackAfterErrorOutcome::Exhausted) => {
                            if let Some(model_fallback) = turn_context.model_fallback.as_ref() {
                                let message = format!(
                                    "Model fallback exhausted: all configured models are unavailable or cooling down (max_wait_seconds={}). Last provider error: {e}",
                                    model_fallback.max_wait_seconds
                                );
                                sess.services.session_telemetry.counter(
                                    "codex.nero.model_fallback.exhausted",
                                    /*inc*/ 1,
                                    &[],
                                );
                                sess.send_event(
                                    &turn_context,
                                    EventMsg::Error(ErrorEvent {
                                        message,
                                        codex_error_info: Some(CodexErrorInfo::ServerOverloaded),
                                    }),
                                )
                                .await;
                                append_nero_model_fallback_audit(
                                    model_fallback_delivery_log_path.as_deref(),
                                    &sess.conversation_id,
                                    &turn_context,
                                    "exhausted",
                                    json!({
                                        "from_model": turn_context.model_info.slug.clone(),
                                        "max_wait_seconds": model_fallback.max_wait_seconds,
                                        "last_provider_error": e.to_string(),
                                    }),
                                )
                                .await;
                                break;
                            }
                        }
                        Ok(ModelFallbackAfterErrorOutcome::DisabledOverflow)
                        | Ok(ModelFallbackAfterErrorOutcome::Noop) => {}
                        Err(CodexErr::TurnAborted) => {
                            break;
                        }
                        Err(err) => {
                            info!("Turn error: {err:#}");
                            let event =
                                EventMsg::Error(err.to_error_event(/*message_prefix*/ None));
                            sess.send_event(&turn_context, event).await;
                            break;
                        }
                    }
                }
                info!("Turn error: {e:#}");
                let event = EventMsg::Error(e.to_error_event(/*message_prefix*/ None));
                sess.send_event(&turn_context, event).await;
                // let the user continue the conversation
                break;
            }
        }
    }

    sess.set_previous_turn_settings(Some(PreviousTurnSettings {
        model: turn_context.model_info.slug.clone(),
        realtime_active: Some(turn_context.realtime_active),
    }))
    .await;

    last_agent_message
}

async fn run_pre_sampling_compact(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
) -> CodexResult<()> {
    let total_usage_tokens_before_compaction = sess.get_total_token_usage().await;
    maybe_run_previous_model_inline_compact(
        sess,
        turn_context,
        total_usage_tokens_before_compaction,
    )
    .await?;
    let total_usage_tokens = sess.get_total_token_usage().await;
    let auto_compact_limit = turn_context
        .model_info
        .auto_compact_token_limit()
        .unwrap_or(i64::MAX);
    // Compact if the total usage tokens are greater than the auto compact limit
    if total_usage_tokens >= auto_compact_limit {
        run_auto_compact(sess, turn_context, InitialContextInjection::DoNotInject).await?;
    }
    Ok(())
}

/// Runs pre-sampling compaction against the previous model when switching to a smaller
/// context-window model.
///
/// Returns `Ok(true)` when compaction ran successfully, `Ok(false)` when compaction was skipped
/// because the model/context-window preconditions were not met, and `Err(_)` only when compaction
/// was attempted and failed.
async fn maybe_run_previous_model_inline_compact(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    total_usage_tokens: i64,
) -> CodexResult<bool> {
    let Some(previous_turn_settings) = sess.previous_turn_settings().await else {
        return Ok(false);
    };
    let previous_model_turn_context = Arc::new(
        turn_context
            .with_model(previous_turn_settings.model, &sess.services.models_manager)
            .await,
    );

    let Some(old_context_window) = previous_model_turn_context.model_context_window() else {
        return Ok(false);
    };
    let Some(new_context_window) = turn_context.model_context_window() else {
        return Ok(false);
    };
    let new_auto_compact_limit = turn_context
        .model_info
        .auto_compact_token_limit()
        .unwrap_or(i64::MAX);
    let should_run = total_usage_tokens > new_auto_compact_limit
        && previous_model_turn_context.model_info.slug != turn_context.model_info.slug
        && old_context_window > new_context_window;
    if should_run {
        run_auto_compact(
            sess,
            &previous_model_turn_context,
            InitialContextInjection::DoNotInject,
        )
        .await?;
        return Ok(true);
    }
    Ok(false)
}

async fn run_auto_compact(
    sess: &Arc<Session>,
    turn_context: &Arc<TurnContext>,
    initial_context_injection: InitialContextInjection,
) -> CodexResult<()> {
    if should_use_remote_compact_task(&turn_context.provider) {
        run_inline_remote_auto_compact_task(
            Arc::clone(sess),
            Arc::clone(turn_context),
            initial_context_injection,
        )
        .await?;
    } else {
        run_inline_auto_compact_task(
            Arc::clone(sess),
            Arc::clone(turn_context),
            initial_context_injection,
        )
        .await?;
    }
    Ok(())
}

fn collect_explicit_app_ids_from_skill_items(
    skill_items: &[ResponseItem],
    connectors: &[connectors::AppInfo],
    skill_name_counts_lower: &HashMap<String, usize>,
) -> HashSet<String> {
    if skill_items.is_empty() || connectors.is_empty() {
        return HashSet::new();
    }

    let skill_messages = skill_items
        .iter()
        .filter_map(|item| match item {
            ResponseItem::Message { content, .. } => {
                content.iter().find_map(|content_item| match content_item {
                    ContentItem::InputText { text } => Some(text.clone()),
                    _ => None,
                })
            }
            _ => None,
        })
        .collect::<Vec<String>>();
    if skill_messages.is_empty() {
        return HashSet::new();
    }

    let mentions = collect_tool_mentions_from_messages(&skill_messages);
    let mention_names_lower = mentions
        .plain_names
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect::<HashSet<String>>();
    let mut connector_ids = mentions
        .paths
        .iter()
        .filter(|path| tool_kind_for_path(path) == ToolMentionKind::App)
        .filter_map(|path| app_id_from_path(path).map(str::to_string))
        .collect::<HashSet<String>>();

    let connector_slug_counts = build_connector_slug_counts(connectors);
    for connector in connectors {
        let slug = connectors::connector_mention_slug(connector);
        let connector_count = connector_slug_counts.get(&slug).copied().unwrap_or(0);
        let skill_count = skill_name_counts_lower.get(&slug).copied().unwrap_or(0);
        if connector_count == 1 && skill_count == 0 && mention_names_lower.contains(&slug) {
            connector_ids.insert(connector.id.clone());
        }
    }

    connector_ids
}

fn filter_connectors_for_input(
    connectors: &[connectors::AppInfo],
    input: &[ResponseItem],
    explicitly_enabled_connectors: &HashSet<String>,
    skill_name_counts_lower: &HashMap<String, usize>,
) -> Vec<connectors::AppInfo> {
    let connectors: Vec<connectors::AppInfo> = connectors
        .iter()
        .filter(|connector| connector.is_enabled)
        .cloned()
        .collect::<Vec<_>>();
    if connectors.is_empty() {
        return Vec::new();
    }

    let user_messages = collect_user_messages(input);
    if user_messages.is_empty() && explicitly_enabled_connectors.is_empty() {
        return Vec::new();
    }

    let mentions = collect_tool_mentions_from_messages(&user_messages);
    let mention_names_lower = mentions
        .plain_names
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect::<HashSet<String>>();

    let connector_slug_counts = build_connector_slug_counts(&connectors);
    let mut allowed_connector_ids = explicitly_enabled_connectors.clone();
    for path in mentions
        .paths
        .iter()
        .filter(|path| tool_kind_for_path(path) == ToolMentionKind::App)
    {
        if let Some(connector_id) = app_id_from_path(path) {
            allowed_connector_ids.insert(connector_id.to_string());
        }
    }

    connectors
        .into_iter()
        .filter(|connector| {
            connector_inserted_in_messages(
                connector,
                &mention_names_lower,
                &allowed_connector_ids,
                &connector_slug_counts,
                skill_name_counts_lower,
            )
        })
        .collect()
}

fn connector_inserted_in_messages(
    connector: &connectors::AppInfo,
    mention_names_lower: &HashSet<String>,
    allowed_connector_ids: &HashSet<String>,
    connector_slug_counts: &HashMap<String, usize>,
    skill_name_counts_lower: &HashMap<String, usize>,
) -> bool {
    if allowed_connector_ids.contains(&connector.id) {
        return true;
    }

    let mention_slug = connectors::connector_mention_slug(connector);
    let connector_count = connector_slug_counts
        .get(&mention_slug)
        .copied()
        .unwrap_or(0);
    let skill_count = skill_name_counts_lower
        .get(&mention_slug)
        .copied()
        .unwrap_or(0);
    connector_count == 1 && skill_count == 0 && mention_names_lower.contains(&mention_slug)
}

fn filter_codex_apps_mcp_tools(
    mcp_tools: &HashMap<String, crate::mcp_connection_manager::ToolInfo>,
    connectors: &[connectors::AppInfo],
    config: &Config,
) -> HashMap<String, crate::mcp_connection_manager::ToolInfo> {
    let allowed: HashSet<&str> = connectors
        .iter()
        .map(|connector| connector.id.as_str())
        .collect();

    mcp_tools
        .iter()
        .filter(|(_, tool)| {
            if tool.server_name != CODEX_APPS_MCP_SERVER_NAME {
                return false;
            }
            let Some(connector_id) = codex_apps_connector_id(tool) else {
                return false;
            };
            allowed.contains(connector_id) && connectors::codex_app_tool_is_enabled(config, tool)
        })
        .map(|(name, tool)| (name.clone(), tool.clone()))
        .collect()
}

fn codex_apps_connector_id(tool: &crate::mcp_connection_manager::ToolInfo) -> Option<&str> {
    tool.connector_id.as_deref()
}

pub(crate) fn build_prompt(
    input: Vec<ResponseItem>,
    router: &ToolRouter,
    turn_context: &TurnContext,
    base_instructions: BaseInstructions,
) -> Prompt {
    let deferred_dynamic_tools = turn_context
        .dynamic_tools
        .iter()
        .filter(|tool| tool.defer_loading)
        .map(|tool| tool.name.as_str())
        .collect::<HashSet<_>>();
    let tools = if deferred_dynamic_tools.is_empty() {
        router.model_visible_specs()
    } else {
        router
            .model_visible_specs()
            .into_iter()
            .filter(|spec| !deferred_dynamic_tools.contains(spec.name()))
            .collect()
    };

    Prompt {
        input,
        tools,
        parallel_tool_calls: turn_context.model_info.supports_parallel_tool_calls,
        base_instructions,
        personality: turn_context.personality,
        output_schema: turn_context.final_output_json_schema.clone(),
    }
}
#[allow(clippy::too_many_arguments)]
#[instrument(level = "trace",
    skip_all,
    fields(
        turn_id = %turn_context.sub_id,
        model = %turn_context.model_info.slug,
        cwd = %turn_context.cwd.display()
    )
)]
async fn run_sampling_request(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    turn_diff_tracker: SharedTurnDiffTracker,
    client_session: &mut ModelClientSession,
    turn_metadata_header: Option<&str>,
    input: Vec<ResponseItem>,
    explicitly_enabled_connectors: &HashSet<String>,
    skills_outcome: Option<&SkillLoadOutcome>,
    server_model_warning_emitted_for_turn: &mut bool,
    cancellation_token: CancellationToken,
) -> CodexResult<SamplingRequestResult> {
    let router = built_tools(
        sess.as_ref(),
        turn_context.as_ref(),
        &input,
        explicitly_enabled_connectors,
        skills_outcome,
        &cancellation_token,
    )
    .await?;

    let base_instructions = sess.get_base_instructions().await;

    let prompt = build_prompt(
        input,
        router.as_ref(),
        turn_context.as_ref(),
        base_instructions,
    );
    let tool_runtime = ToolCallRuntime::new(
        Arc::clone(&router),
        Arc::clone(&sess),
        Arc::clone(&turn_context),
        Arc::clone(&turn_diff_tracker),
    );
    let _code_mode_worker = sess
        .services
        .code_mode_service
        .start_turn_worker(
            &sess,
            &turn_context,
            Arc::clone(&router),
            Arc::clone(&turn_diff_tracker),
        )
        .await;
    let mut retries = 0;
    loop {
        let err = match try_run_sampling_request(
            tool_runtime.clone(),
            Arc::clone(&sess),
            Arc::clone(&turn_context),
            client_session,
            turn_metadata_header,
            Arc::clone(&turn_diff_tracker),
            server_model_warning_emitted_for_turn,
            &prompt,
            cancellation_token.child_token(),
        )
        .await
        {
            Ok(output) => {
                return Ok(output);
            }
            Err(CodexErr::ContextWindowExceeded) => {
                sess.set_total_tokens_full(&turn_context).await;
                return Err(CodexErr::ContextWindowExceeded);
            }
            Err(err) => err,
        };

        if let CodexErr::UsageLimitReached(e) = &err
            && let Some(rate_limits) = e.rate_limits.clone()
        {
            sess.update_rate_limits(&turn_context, *rate_limits).await;
        }

        if client_session
            .try_recover_stream_usage_limit_or_quota(&err)
            .await?
        {
            continue;
        }

        if !err.is_retryable() {
            return Err(err);
        }

        // Use the configured provider-specific stream retry budget.
        let max_retries = turn_context.provider.stream_max_retries();
        if retries >= max_retries
            && client_session.try_switch_fallback_transport(
                &turn_context.session_telemetry,
                &turn_context.model_info,
            )
        {
            sess.send_event(
                &turn_context,
                EventMsg::Warning(WarningEvent {
                    message: format!("Falling back from WebSockets to HTTPS transport. {err:#}"),
                }),
            )
            .await;
            retries = 0;
            continue;
        }
        if retries < max_retries {
            retries += 1;
            let delay = match &err {
                CodexErr::Stream(_, requested_delay) => {
                    requested_delay.unwrap_or_else(|| backoff(retries))
                }
                _ => backoff(retries),
            };
            warn!(
                "stream disconnected - retrying sampling request ({retries}/{max_retries} in {delay:?})...",
            );

            // In release builds, hide the first websocket retry notification to reduce noisy
            // transient reconnect messages. In debug builds, keep full visibility for diagnosis.
            let report_error = retries > 1
                || cfg!(debug_assertions)
                || !sess.services.model_client.responses_websocket_enabled();
            if report_error {
                // Surface retry information to any UI/front‑end so the
                // user understands what is happening instead of staring
                // at a seemingly frozen screen.
                sess.notify_stream_error(
                    &turn_context,
                    format!("Reconnecting... {retries}/{max_retries}"),
                    err,
                )
                .await;
            }
            tokio::time::sleep(delay).await;
        } else {
            return Err(err);
        }
    }
}

pub(crate) async fn built_tools(
    sess: &Session,
    turn_context: &TurnContext,
    input: &[ResponseItem],
    explicitly_enabled_connectors: &HashSet<String>,
    skills_outcome: Option<&SkillLoadOutcome>,
    cancellation_token: &CancellationToken,
) -> CodexResult<Arc<ToolRouter>> {
    let mcp_connection_manager = sess.services.mcp_connection_manager.read().await;
    let has_mcp_servers = mcp_connection_manager.has_servers();
    let mut mcp_tools = mcp_connection_manager
        .list_all_tools()
        .or_cancel(cancellation_token)
        .await?;
    drop(mcp_connection_manager);
    let loaded_plugins = sess
        .services
        .plugins_manager
        .plugins_for_config(&turn_context.config);

    let mut effective_explicitly_enabled_connectors = explicitly_enabled_connectors.clone();
    effective_explicitly_enabled_connectors.extend(sess.get_connector_selection().await);

    let apps_enabled = turn_context.apps_enabled();
    let accessible_connectors =
        apps_enabled.then(|| connectors::accessible_connectors_from_mcp_tools(&mcp_tools));
    let accessible_connectors_with_enabled_state =
        accessible_connectors.as_ref().map(|connectors| {
            connectors::with_app_enabled_state(connectors.clone(), &turn_context.config)
        });
    let connectors = if apps_enabled {
        let connectors = connectors::merge_plugin_apps_with_accessible(
            loaded_plugins.effective_apps(),
            accessible_connectors.clone().unwrap_or_default(),
        );
        Some(connectors::with_app_enabled_state(
            connectors,
            &turn_context.config,
        ))
    } else {
        None
    };
    let auth = sess.services.auth_manager.auth().await;
    let discoverable_tools = if apps_enabled && turn_context.tools_config.tool_suggest {
        if let Some(accessible_connectors) = accessible_connectors_with_enabled_state.as_ref() {
            match connectors::list_tool_suggest_discoverable_tools_with_auth(
                &turn_context.config,
                auth.as_ref(),
                accessible_connectors.as_slice(),
            )
            .await
            .map(|discoverable_tools| {
                filter_tool_suggest_discoverable_tools_for_client(
                    discoverable_tools,
                    turn_context.app_server_client_name.as_deref(),
                )
            }) {
                Ok(discoverable_tools) if discoverable_tools.is_empty() => None,
                Ok(discoverable_tools) => Some(discoverable_tools),
                Err(err) => {
                    warn!("failed to load discoverable tool suggestions: {err:#}");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let app_tools = connectors.as_ref().map(|connectors| {
        filter_codex_apps_mcp_tools(&mcp_tools, connectors, &turn_context.config)
    });

    if let Some(connectors) = connectors.as_ref() {
        let skill_name_counts_lower = skills_outcome.map_or_else(HashMap::new, |outcome| {
            build_skill_name_counts(&outcome.skills, &outcome.disabled_paths).1
        });

        let explicitly_enabled = filter_connectors_for_input(
            connectors,
            input,
            &effective_explicitly_enabled_connectors,
            &skill_name_counts_lower,
        );

        let mut selected_mcp_tools = filter_non_codex_apps_mcp_tools_only(&mcp_tools);
        selected_mcp_tools.extend(filter_codex_apps_mcp_tools(
            &mcp_tools,
            explicitly_enabled.as_ref(),
            &turn_context.config,
        ));

        mcp_tools = selected_mcp_tools;
    }

    // Expose app tools directly when tool_search is disabled, or when tool_search
    // is enabled but the accessible app tool set stays below the direct-exposure threshold.
    let expose_app_tools_directly = !turn_context.tools_config.search_tool
        || app_tools
            .as_ref()
            .is_some_and(|tools| tools.len() < DIRECT_APP_TOOL_EXPOSURE_THRESHOLD);
    if expose_app_tools_directly && let Some(app_tools) = app_tools.as_ref() {
        mcp_tools.extend(app_tools.clone());
    }
    let app_tools = if expose_app_tools_directly {
        None
    } else {
        app_tools
    };

    Ok(Arc::new(ToolRouter::from_config(
        &turn_context.tools_config,
        ToolRouterParams {
            mcp_tools: has_mcp_servers.then(|| {
                mcp_tools
                    .into_iter()
                    .map(|(name, tool)| (name, tool.tool))
                    .collect()
            }),
            app_tools,
            discoverable_tools,
            dynamic_tools: turn_context.dynamic_tools.as_slice(),
        },
    )))
}

#[derive(Debug)]
struct SamplingRequestResult {
    needs_follow_up: bool,
    last_agent_message: Option<String>,
}

/// Ephemeral per-response state for streaming a single proposed plan.
/// This is intentionally not persisted or stored in session/state since it
/// only exists while a response is actively streaming. The final plan text
/// is extracted from the completed assistant message.
/// Tracks a single proposed plan item across a streaming response.
struct ProposedPlanItemState {
    item_id: String,
    started: bool,
    completed: bool,
}

/// Aggregated state used only while streaming a plan-mode response.
/// Includes per-item parsers, deferred agent message bookkeeping, and the plan item lifecycle.
struct PlanModeStreamState {
    /// Agent message items started by the model but deferred until we see non-plan text.
    pending_agent_message_items: HashMap<String, TurnItem>,
    /// Agent message items whose start notification has been emitted.
    started_agent_message_items: HashSet<String>,
    /// Leading whitespace buffered until we see non-whitespace text for an item.
    leading_whitespace_by_item: HashMap<String, String>,
    /// Tracks plan item lifecycle while streaming plan output.
    plan_item_state: ProposedPlanItemState,
}

impl PlanModeStreamState {
    fn new(turn_id: &str) -> Self {
        Self {
            pending_agent_message_items: HashMap::new(),
            started_agent_message_items: HashSet::new(),
            leading_whitespace_by_item: HashMap::new(),
            plan_item_state: ProposedPlanItemState::new(turn_id),
        }
    }
}

#[derive(Debug, Default)]
struct AssistantMessageStreamParsers {
    plan_mode: bool,
    parsers_by_item: HashMap<String, AssistantTextStreamParser>,
}

type ParsedAssistantTextDelta = AssistantTextChunk;

impl AssistantMessageStreamParsers {
    fn new(plan_mode: bool) -> Self {
        Self {
            plan_mode,
            parsers_by_item: HashMap::new(),
        }
    }

    fn parser_mut(&mut self, item_id: &str) -> &mut AssistantTextStreamParser {
        let plan_mode = self.plan_mode;
        self.parsers_by_item
            .entry(item_id.to_string())
            .or_insert_with(|| AssistantTextStreamParser::new(plan_mode))
    }

    fn seed_item_text(&mut self, item_id: &str, text: &str) -> ParsedAssistantTextDelta {
        if text.is_empty() {
            return ParsedAssistantTextDelta::default();
        }
        self.parser_mut(item_id).push_str(text)
    }

    fn parse_delta(&mut self, item_id: &str, delta: &str) -> ParsedAssistantTextDelta {
        self.parser_mut(item_id).push_str(delta)
    }

    fn finish_item(&mut self, item_id: &str) -> ParsedAssistantTextDelta {
        let Some(mut parser) = self.parsers_by_item.remove(item_id) else {
            return ParsedAssistantTextDelta::default();
        };
        parser.finish()
    }

    fn drain_finished(&mut self) -> Vec<(String, ParsedAssistantTextDelta)> {
        let parsers_by_item = std::mem::take(&mut self.parsers_by_item);
        parsers_by_item
            .into_iter()
            .map(|(item_id, mut parser)| (item_id, parser.finish()))
            .collect()
    }
}

impl ProposedPlanItemState {
    fn new(turn_id: &str) -> Self {
        Self {
            item_id: format!("{turn_id}-plan"),
            started: false,
            completed: false,
        }
    }

    async fn start(&mut self, sess: &Session, turn_context: &TurnContext) {
        if self.started || self.completed {
            return;
        }
        self.started = true;
        let item = TurnItem::Plan(PlanItem {
            id: self.item_id.clone(),
            text: String::new(),
        });
        sess.emit_turn_item_started(turn_context, &item).await;
    }

    async fn push_delta(&mut self, sess: &Session, turn_context: &TurnContext, delta: &str) {
        if self.completed {
            return;
        }
        if delta.is_empty() {
            return;
        }
        let event = PlanDeltaEvent {
            thread_id: sess.conversation_id.to_string(),
            turn_id: turn_context.sub_id.clone(),
            item_id: self.item_id.clone(),
            delta: delta.to_string(),
        };
        sess.send_event(turn_context, EventMsg::PlanDelta(event))
            .await;
    }

    async fn complete_with_text(
        &mut self,
        sess: &Session,
        turn_context: &TurnContext,
        text: String,
    ) {
        if self.completed || !self.started {
            return;
        }
        self.completed = true;
        let item = TurnItem::Plan(PlanItem {
            id: self.item_id.clone(),
            text,
        });
        sess.emit_turn_item_completed(turn_context, item).await;
    }
}

/// In plan mode we defer agent message starts until the parser emits non-plan
/// text. The parser buffers each line until it can rule out a tag prefix, so
/// plan-only outputs never show up as empty assistant messages.
async fn maybe_emit_pending_agent_message_start(
    sess: &Session,
    turn_context: &TurnContext,
    state: &mut PlanModeStreamState,
    item_id: &str,
) {
    if state.started_agent_message_items.contains(item_id) {
        return;
    }
    if let Some(item) = state.pending_agent_message_items.remove(item_id) {
        sess.emit_turn_item_started(turn_context, &item).await;
        state
            .started_agent_message_items
            .insert(item_id.to_string());
    }
}

/// Agent messages are text-only today; concatenate all text entries.
fn agent_message_text(item: &codex_protocol::items::AgentMessageItem) -> String {
    item.content
        .iter()
        .map(|entry| match entry {
            codex_protocol::items::AgentMessageContent::Text { text } => text.as_str(),
        })
        .collect()
}

fn realtime_text_for_event(msg: &EventMsg) -> Option<String> {
    match msg {
        EventMsg::AgentMessage(event) => Some(event.message.clone()),
        EventMsg::ItemCompleted(event) => match &event.item {
            TurnItem::AgentMessage(item) => Some(agent_message_text(item)),
            _ => None,
        },
        EventMsg::Error(_)
        | EventMsg::Warning(_)
        | EventMsg::RealtimeConversationStarted(_)
        | EventMsg::RealtimeConversationRealtime(_)
        | EventMsg::RealtimeConversationClosed(_)
        | EventMsg::ModelReroute(_)
        | EventMsg::ContextCompacted(_)
        | EventMsg::ThreadRolledBack(_)
        | EventMsg::TurnStarted(_)
        | EventMsg::TurnComplete(_)
        | EventMsg::TokenCount(_)
        | EventMsg::UserMessage(_)
        | EventMsg::AgentMessageDelta(_)
        | EventMsg::AgentReasoning(_)
        | EventMsg::AgentReasoningDelta(_)
        | EventMsg::AgentReasoningRawContent(_)
        | EventMsg::AgentReasoningRawContentDelta(_)
        | EventMsg::AgentReasoningSectionBreak(_)
        | EventMsg::SessionConfigured(_)
        | EventMsg::ThreadNameUpdated(_)
        | EventMsg::McpStartupUpdate(_)
        | EventMsg::McpStartupComplete(_)
        | EventMsg::McpToolCallBegin(_)
        | EventMsg::McpToolCallEnd(_)
        | EventMsg::WebSearchBegin(_)
        | EventMsg::WebSearchEnd(_)
        | EventMsg::ExecCommandBegin(_)
        | EventMsg::ExecCommandOutputDelta(_)
        | EventMsg::TerminalInteraction(_)
        | EventMsg::ExecCommandEnd(_)
        | EventMsg::PatchApplyBegin(_)
        | EventMsg::PatchApplyEnd(_)
        | EventMsg::ViewImageToolCall(_)
        | EventMsg::ImageGenerationBegin(_)
        | EventMsg::ImageGenerationEnd(_)
        | EventMsg::ExecApprovalRequest(_)
        | EventMsg::RequestPermissions(_)
        | EventMsg::RequestUserInput(_)
        | EventMsg::DynamicToolCallRequest(_)
        | EventMsg::DynamicToolCallResponse(_)
        | EventMsg::GuardianAssessment(_)
        | EventMsg::ElicitationRequest(_)
        | EventMsg::ApplyPatchApprovalRequest(_)
        | EventMsg::DeprecationNotice(_)
        | EventMsg::BackgroundEvent(_)
        | EventMsg::UndoStarted(_)
        | EventMsg::UndoCompleted(_)
        | EventMsg::StreamError(_)
        | EventMsg::TurnDiff(_)
        | EventMsg::GetHistoryEntryResponse(_)
        | EventMsg::McpListToolsResponse(_)
        | EventMsg::ListSkillsResponse(_)
        | EventMsg::SkillsUpdateAvailable
        | EventMsg::PlanUpdate(_)
        | EventMsg::TurnAborted(_)
        | EventMsg::ShutdownComplete
        | EventMsg::EnteredReviewMode(_)
        | EventMsg::ExitedReviewMode(_)
        | EventMsg::RawResponseItem(_)
        | EventMsg::ItemStarted(_)
        | EventMsg::HookStarted(_)
        | EventMsg::HookCompleted(_)
        | EventMsg::AgentMessageContentDelta(_)
        | EventMsg::PlanDelta(_)
        | EventMsg::ReasoningContentDelta(_)
        | EventMsg::ReasoningRawContentDelta(_)
        | EventMsg::CollabAgentSpawnBegin(_)
        | EventMsg::CollabAgentSpawnEnd(_)
        | EventMsg::CollabAgentInteractionBegin(_)
        | EventMsg::CollabAgentInteractionEnd(_)
        | EventMsg::CollabWaitingBegin(_)
        | EventMsg::CollabWaitingEnd(_)
        | EventMsg::CollabCloseBegin(_)
        | EventMsg::CollabCloseEnd(_)
        | EventMsg::CollabResumeBegin(_)
        | EventMsg::CollabResumeEnd(_) => None,
    }
}

/// Split the stream into normal assistant text vs. proposed plan content.
/// Normal text becomes AgentMessage deltas; plan content becomes PlanDelta +
/// TurnItem::Plan.
async fn handle_plan_segments(
    sess: &Session,
    turn_context: &TurnContext,
    state: &mut PlanModeStreamState,
    item_id: &str,
    segments: Vec<ProposedPlanSegment>,
) {
    for segment in segments {
        match segment {
            ProposedPlanSegment::Normal(delta) => {
                if delta.is_empty() {
                    continue;
                }
                let has_non_whitespace = delta.chars().any(|ch| !ch.is_whitespace());
                if !has_non_whitespace && !state.started_agent_message_items.contains(item_id) {
                    let entry = state
                        .leading_whitespace_by_item
                        .entry(item_id.to_string())
                        .or_default();
                    entry.push_str(&delta);
                    continue;
                }
                let delta = if !state.started_agent_message_items.contains(item_id) {
                    if let Some(prefix) = state.leading_whitespace_by_item.remove(item_id) {
                        format!("{prefix}{delta}")
                    } else {
                        delta
                    }
                } else {
                    delta
                };
                maybe_emit_pending_agent_message_start(sess, turn_context, state, item_id).await;

                let event = AgentMessageContentDeltaEvent {
                    thread_id: sess.conversation_id.to_string(),
                    turn_id: turn_context.sub_id.clone(),
                    item_id: item_id.to_string(),
                    delta,
                };
                sess.send_event(turn_context, EventMsg::AgentMessageContentDelta(event))
                    .await;
            }
            ProposedPlanSegment::ProposedPlanStart => {
                if !state.plan_item_state.completed {
                    state.plan_item_state.start(sess, turn_context).await;
                }
            }
            ProposedPlanSegment::ProposedPlanDelta(delta) => {
                if !state.plan_item_state.completed {
                    if !state.plan_item_state.started {
                        state.plan_item_state.start(sess, turn_context).await;
                    }
                    state
                        .plan_item_state
                        .push_delta(sess, turn_context, &delta)
                        .await;
                }
            }
            ProposedPlanSegment::ProposedPlanEnd => {}
        }
    }
}

async fn emit_streamed_assistant_text_delta(
    sess: &Session,
    turn_context: &TurnContext,
    plan_mode_state: Option<&mut PlanModeStreamState>,
    item_id: &str,
    parsed: ParsedAssistantTextDelta,
) {
    if parsed.is_empty() {
        return;
    }
    if !parsed.citations.is_empty() {
        // Citation extraction is intentionally local for now; we strip citations from display text
        // but do not yet surface them in protocol events.
        let _citations = parsed.citations;
    }
    if let Some(state) = plan_mode_state {
        if !parsed.plan_segments.is_empty() {
            handle_plan_segments(sess, turn_context, state, item_id, parsed.plan_segments).await;
        }
        return;
    }
    if parsed.visible_text.is_empty() {
        return;
    }
    let event = AgentMessageContentDeltaEvent {
        thread_id: sess.conversation_id.to_string(),
        turn_id: turn_context.sub_id.clone(),
        item_id: item_id.to_string(),
        delta: parsed.visible_text,
    };
    sess.send_event(turn_context, EventMsg::AgentMessageContentDelta(event))
        .await;
}

/// Flush buffered assistant text parser state when an assistant message item ends.
async fn flush_assistant_text_segments_for_item(
    sess: &Session,
    turn_context: &TurnContext,
    plan_mode_state: Option<&mut PlanModeStreamState>,
    parsers: &mut AssistantMessageStreamParsers,
    item_id: &str,
) {
    let parsed = parsers.finish_item(item_id);
    emit_streamed_assistant_text_delta(sess, turn_context, plan_mode_state, item_id, parsed).await;
}

/// Flush any remaining buffered assistant text parser state at response completion.
async fn flush_assistant_text_segments_all(
    sess: &Session,
    turn_context: &TurnContext,
    mut plan_mode_state: Option<&mut PlanModeStreamState>,
    parsers: &mut AssistantMessageStreamParsers,
) {
    for (item_id, parsed) in parsers.drain_finished() {
        emit_streamed_assistant_text_delta(
            sess,
            turn_context,
            plan_mode_state.as_deref_mut(),
            &item_id,
            parsed,
        )
        .await;
    }
}

/// Emit completion for plan items by parsing the finalized assistant message.
async fn maybe_complete_plan_item_from_message(
    sess: &Session,
    turn_context: &TurnContext,
    state: &mut PlanModeStreamState,
    item: &ResponseItem,
) {
    if let ResponseItem::Message { role, content, .. } = item
        && role == "assistant"
    {
        let mut text = String::new();
        for entry in content {
            if let ContentItem::OutputText { text: chunk } = entry {
                text.push_str(chunk);
            }
        }
        if let Some(plan_text) = extract_proposed_plan_text(&text) {
            let (plan_text, _citations) = strip_citations(&plan_text);
            if !state.plan_item_state.started {
                state.plan_item_state.start(sess, turn_context).await;
            }
            state
                .plan_item_state
                .complete_with_text(sess, turn_context, plan_text)
                .await;
        }
    }
}

/// Emit a completed agent message in plan mode, respecting deferred starts.
async fn emit_agent_message_in_plan_mode(
    sess: &Session,
    turn_context: &TurnContext,
    agent_message: codex_protocol::items::AgentMessageItem,
    state: &mut PlanModeStreamState,
) {
    let agent_message_id = agent_message.id.clone();
    let text = agent_message_text(&agent_message);
    if text.trim().is_empty() {
        state.pending_agent_message_items.remove(&agent_message_id);
        state.started_agent_message_items.remove(&agent_message_id);
        return;
    }

    maybe_emit_pending_agent_message_start(sess, turn_context, state, &agent_message_id).await;

    if !state
        .started_agent_message_items
        .contains(&agent_message_id)
    {
        let start_item = state
            .pending_agent_message_items
            .remove(&agent_message_id)
            .unwrap_or_else(|| {
                TurnItem::AgentMessage(codex_protocol::items::AgentMessageItem {
                    id: agent_message_id.clone(),
                    content: Vec::new(),
                    phase: None,
                    memory_citation: None,
                })
            });
        sess.emit_turn_item_started(turn_context, &start_item).await;
        state
            .started_agent_message_items
            .insert(agent_message_id.clone());
    }

    sess.emit_turn_item_completed(turn_context, TurnItem::AgentMessage(agent_message))
        .await;
    state.started_agent_message_items.remove(&agent_message_id);
}

/// Emit completion for a plan-mode turn item, handling agent messages specially.
async fn emit_turn_item_in_plan_mode(
    sess: &Session,
    turn_context: &TurnContext,
    turn_item: TurnItem,
    previously_active_item: Option<&TurnItem>,
    state: &mut PlanModeStreamState,
) {
    match turn_item {
        TurnItem::AgentMessage(agent_message) => {
            emit_agent_message_in_plan_mode(sess, turn_context, agent_message, state).await;
        }
        _ => {
            if previously_active_item.is_none() {
                sess.emit_turn_item_started(turn_context, &turn_item).await;
            }
            sess.emit_turn_item_completed(turn_context, turn_item).await;
        }
    }
}

/// Handle a completed assistant response item in plan mode, returning true if handled.
async fn handle_assistant_item_done_in_plan_mode(
    sess: &Session,
    turn_context: &TurnContext,
    item: &ResponseItem,
    state: &mut PlanModeStreamState,
    previously_active_item: Option<&TurnItem>,
    last_agent_message: &mut Option<String>,
) -> bool {
    if let ResponseItem::Message { role, .. } = item
        && role == "assistant"
    {
        maybe_complete_plan_item_from_message(sess, turn_context, state, item).await;

        if let Some(turn_item) =
            handle_non_tool_response_item(sess, turn_context, item, /*plan_mode*/ true).await
        {
            emit_turn_item_in_plan_mode(
                sess,
                turn_context,
                turn_item,
                previously_active_item,
                state,
            )
            .await;
        }

        record_completed_response_item(sess, turn_context, item).await;
        if let Some(agent_message) = last_assistant_message_from_item(item, /*plan_mode*/ true) {
            *last_agent_message = Some(agent_message);
        }
        return true;
    }
    false
}

async fn drain_in_flight(
    in_flight: &mut FuturesOrdered<BoxFuture<'static, CodexResult<ResponseInputItem>>>,
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
) -> CodexResult<()> {
    while let Some(res) = in_flight.next().await {
        match res {
            Ok(response_input) => {
                sess.record_conversation_items(&turn_context, &[response_input.into()])
                    .await;
            }
            Err(err) => {
                error_or_panic(format!("in-flight tool future failed during drain: {err}"));
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[instrument(level = "trace",
    skip_all,
    fields(
        turn_id = %turn_context.sub_id,
        model = %turn_context.model_info.slug
    )
)]
async fn try_run_sampling_request(
    tool_runtime: ToolCallRuntime,
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    client_session: &mut ModelClientSession,
    turn_metadata_header: Option<&str>,
    turn_diff_tracker: SharedTurnDiffTracker,
    server_model_warning_emitted_for_turn: &mut bool,
    prompt: &Prompt,
    cancellation_token: CancellationToken,
) -> CodexResult<SamplingRequestResult> {
    feedback_tags!(
        model = turn_context.model_info.slug.clone(),
        approval_policy = turn_context.approval_policy.value(),
        sandbox_policy = turn_context.sandbox_policy.get(),
        effort = turn_context.reasoning_effort,
        auth_mode = sess.services.auth_manager.auth_mode(),
        features = sess.features.enabled_features(),
    );
    let plan_mode = turn_context.collaboration_mode.mode == ModeKind::Plan;
    let receiving_span = trace_span!("receiving_stream");
    // Keep one bounded rotate/retry budget for the whole turn request, not per stream attempt.
    client_session.reset_usage_limit_recovery_budget();
    'request: loop {
        let mut stream = client_session
            .stream(
                prompt,
                &turn_context.model_info,
                &turn_context.session_telemetry,
                turn_context.reasoning_effort,
                turn_context.reasoning_summary,
                turn_context.config.service_tier,
                turn_metadata_header,
            )
            .instrument(trace_span!("stream_request"))
            .or_cancel(&cancellation_token)
            .await??;

        let mut in_flight: FuturesOrdered<BoxFuture<'static, CodexResult<ResponseInputItem>>> =
            FuturesOrdered::new();
        let mut needs_follow_up = false;
        let mut last_agent_message: Option<String> = None;
        let mut active_item: Option<TurnItem> = None;
        let mut should_emit_turn_diff = false;
        let mut saw_response_output = false;
        let mut assistant_message_stream_parsers = AssistantMessageStreamParsers::new(plan_mode);
        let mut plan_mode_state = plan_mode.then(|| PlanModeStreamState::new(&turn_context.sub_id));
        let outcome: CodexResult<SamplingRequestResult> = loop {
            let handle_responses = trace_span!(
                parent: &receiving_span,
                "handle_responses",
                otel.name = field::Empty,
                tool_name = field::Empty,
                from = field::Empty,
            );
            let event = match stream
                .next()
                .instrument(trace_span!(parent: &handle_responses, "receiving"))
                .or_cancel(&cancellation_token)
                .await
            {
                Ok(event) => event,
                Err(codex_async_utils::CancelErr::Cancelled) => break Err(CodexErr::TurnAborted),
            };

            let event = match event {
                Some(res) => match res {
                    Ok(event) => event,
                    Err(err) => {
                        let can_retry_after_rotation =
                            !saw_response_output && active_item.is_none() && in_flight.is_empty();
                        if can_retry_after_rotation
                            && client_session
                                .try_recover_stream_usage_limit_or_quota(&err)
                                .await?
                        {
                            continue 'request;
                        }
                        break Err(err);
                    }
                },
                None => {
                    break Err(CodexErr::Stream(
                        "stream closed before response.completed".into(),
                        None,
                    ));
                }
            };

            sess.services
                .session_telemetry
                .record_responses(&handle_responses, &event);
            record_turn_ttft_metric(&turn_context, &event).await;

            match event {
                ResponseEvent::Created => {}
                ResponseEvent::OutputItemDone(item) => {
                    saw_response_output = true;
                    let previously_active_item = active_item.take();
                    if let Some(previous) = previously_active_item.as_ref()
                        && matches!(previous, TurnItem::AgentMessage(_))
                    {
                        let item_id = previous.id();
                        flush_assistant_text_segments_for_item(
                            &sess,
                            &turn_context,
                            plan_mode_state.as_mut(),
                            &mut assistant_message_stream_parsers,
                            &item_id,
                        )
                        .await;
                    }
                    if let Some(state) = plan_mode_state.as_mut()
                        && handle_assistant_item_done_in_plan_mode(
                            &sess,
                            &turn_context,
                            &item,
                            state,
                            previously_active_item.as_ref(),
                            &mut last_agent_message,
                        )
                        .await
                    {
                        continue;
                    }

                    let mut ctx = HandleOutputCtx {
                        sess: sess.clone(),
                        turn_context: turn_context.clone(),
                        tool_runtime: tool_runtime.clone(),
                        cancellation_token: cancellation_token.child_token(),
                    };

                    let output_result =
                        handle_output_item_done(&mut ctx, item, previously_active_item)
                            .instrument(handle_responses)
                            .await?;
                    if let Some(tool_future) = output_result.tool_future {
                        in_flight.push_back(tool_future);
                    }
                    if let Some(agent_message) = output_result.last_agent_message {
                        last_agent_message = Some(agent_message);
                    }
                    needs_follow_up |= output_result.needs_follow_up;
                }
                ResponseEvent::OutputItemAdded(item) => {
                    saw_response_output = true;
                    if let Some(turn_item) = handle_non_tool_response_item(
                        sess.as_ref(),
                        turn_context.as_ref(),
                        &item,
                        plan_mode,
                    )
                    .await
                    {
                        let mut turn_item = turn_item;
                        let mut seeded_parsed: Option<ParsedAssistantTextDelta> = None;
                        let mut seeded_item_id: Option<String> = None;
                        if matches!(turn_item, TurnItem::AgentMessage(_))
                            && let Some(raw_text) = raw_assistant_output_text_from_item(&item)
                        {
                            let item_id = turn_item.id();
                            let mut seeded = assistant_message_stream_parsers
                                .seed_item_text(&item_id, &raw_text);
                            if let TurnItem::AgentMessage(agent_message) = &mut turn_item {
                                agent_message.content =
                                    vec![codex_protocol::items::AgentMessageContent::Text {
                                        text: if plan_mode {
                                            String::new()
                                        } else {
                                            std::mem::take(&mut seeded.visible_text)
                                        },
                                    }];
                            }
                            seeded_parsed = plan_mode.then_some(seeded);
                            seeded_item_id = Some(item_id);
                        }
                        if let Some(state) = plan_mode_state.as_mut()
                            && matches!(turn_item, TurnItem::AgentMessage(_))
                        {
                            let item_id = turn_item.id();
                            state
                                .pending_agent_message_items
                                .insert(item_id, turn_item.clone());
                        } else {
                            sess.emit_turn_item_started(&turn_context, &turn_item).await;
                        }
                        if let (Some(state), Some(item_id), Some(parsed)) = (
                            plan_mode_state.as_mut(),
                            seeded_item_id.as_deref(),
                            seeded_parsed,
                        ) {
                            emit_streamed_assistant_text_delta(
                                &sess,
                                &turn_context,
                                Some(state),
                                item_id,
                                parsed,
                            )
                            .await;
                        }
                        active_item = Some(turn_item);
                    }
                }
                ResponseEvent::ServerModel(server_model) => {
                    if !*server_model_warning_emitted_for_turn
                        && sess
                            .maybe_warn_on_server_model_mismatch(&turn_context, server_model)
                            .await
                    {
                        *server_model_warning_emitted_for_turn = true;
                    }
                }
                ResponseEvent::ServerReasoningIncluded(included) => {
                    sess.set_server_reasoning_included(included).await;
                }
                ResponseEvent::RateLimits(snapshot) => {
                    // Update internal state with latest rate limits, but defer sending until
                    // token usage is available to avoid duplicate TokenCount events.
                    sess.update_rate_limits(&turn_context, snapshot).await;
                }
                ResponseEvent::ModelsEtag(etag) => {
                    // Update internal state with latest models etag
                    sess.services.models_manager.refresh_if_new_etag(etag).await;
                }
                ResponseEvent::Completed {
                    response_id: _,
                    token_usage,
                } => {
                    flush_assistant_text_segments_all(
                        &sess,
                        &turn_context,
                        plan_mode_state.as_mut(),
                        &mut assistant_message_stream_parsers,
                    )
                    .await;
                    sess.update_token_usage_info(&turn_context, token_usage.as_ref())
                        .await;
                    should_emit_turn_diff = true;

                    needs_follow_up |= sess.has_pending_input().await;

                    break Ok(SamplingRequestResult {
                        needs_follow_up,
                        last_agent_message,
                    });
                }
                ResponseEvent::OutputTextDelta(delta) => {
                    saw_response_output = true;
                    // In review child threads, suppress assistant text deltas; the
                    // UI will show a selection popup from the final ReviewOutput.
                    if let Some(active) = active_item.as_ref() {
                        let item_id = active.id();
                        if matches!(active, TurnItem::AgentMessage(_)) {
                            let parsed =
                                assistant_message_stream_parsers.parse_delta(&item_id, &delta);
                            emit_streamed_assistant_text_delta(
                                &sess,
                                &turn_context,
                                plan_mode_state.as_mut(),
                                &item_id,
                                parsed,
                            )
                            .await;
                        } else {
                            let event = AgentMessageContentDeltaEvent {
                                thread_id: sess.conversation_id.to_string(),
                                turn_id: turn_context.sub_id.clone(),
                                item_id,
                                delta,
                            };
                            sess.send_event(
                                &turn_context,
                                EventMsg::AgentMessageContentDelta(event),
                            )
                            .await;
                        }
                    } else {
                        error_or_panic("OutputTextDelta without active item".to_string());
                    }
                }
                ResponseEvent::ReasoningSummaryDelta {
                    delta,
                    summary_index,
                } => {
                    saw_response_output = true;
                    if let Some(active) = active_item.as_ref() {
                        let event = ReasoningContentDeltaEvent {
                            thread_id: sess.conversation_id.to_string(),
                            turn_id: turn_context.sub_id.clone(),
                            item_id: active.id(),
                            delta,
                            summary_index,
                        };
                        sess.send_event(&turn_context, EventMsg::ReasoningContentDelta(event))
                            .await;
                    } else {
                        error_or_panic("ReasoningSummaryDelta without active item".to_string());
                    }
                }
                ResponseEvent::ReasoningSummaryPartAdded { summary_index } => {
                    saw_response_output = true;
                    if let Some(active) = active_item.as_ref() {
                        let event =
                            EventMsg::AgentReasoningSectionBreak(AgentReasoningSectionBreakEvent {
                                item_id: active.id(),
                                summary_index,
                            });
                        sess.send_event(&turn_context, event).await;
                    } else {
                        error_or_panic("ReasoningSummaryPartAdded without active item".to_string());
                    }
                }
                ResponseEvent::ReasoningContentDelta {
                    delta,
                    content_index,
                } => {
                    saw_response_output = true;
                    if let Some(active) = active_item.as_ref() {
                        let event = ReasoningRawContentDeltaEvent {
                            thread_id: sess.conversation_id.to_string(),
                            turn_id: turn_context.sub_id.clone(),
                            item_id: active.id(),
                            delta,
                            content_index,
                        };
                        sess.send_event(&turn_context, EventMsg::ReasoningRawContentDelta(event))
                            .await;
                    } else {
                        error_or_panic("ReasoningRawContentDelta without active item".to_string());
                    }
                }
            }
        };

        flush_assistant_text_segments_all(
            &sess,
            &turn_context,
            plan_mode_state.as_mut(),
            &mut assistant_message_stream_parsers,
        )
        .await;

        drain_in_flight(&mut in_flight, sess.clone(), turn_context.clone()).await?;

        if cancellation_token.is_cancelled() {
            return Err(CodexErr::TurnAborted);
        }

        if should_emit_turn_diff {
            let unified_diff = {
                let mut tracker = turn_diff_tracker.lock().await;
                tracker.get_unified_diff()
            };
            if let Ok(Some(unified_diff)) = unified_diff {
                let msg = EventMsg::TurnDiff(TurnDiffEvent { unified_diff });
                sess.clone().send_event(&turn_context, msg).await;
            }
        }

        break outcome;
    }
}

pub(super) fn get_last_assistant_message_from_turn(responses: &[ResponseItem]) -> Option<String> {
    for item in responses.iter().rev() {
        if let Some(message) = last_assistant_message_from_item(item, /*plan_mode*/ false) {
            return Some(message);
        }
    }
    None
}

use crate::memories::prompts::build_memory_tool_developer_instructions;
#[cfg(test)]
pub(crate) use tests::make_session_and_context;
#[cfg(test)]
pub(crate) use tests::make_session_and_context_with_dynamic_tools_and_rx;
#[cfg(test)]
pub(crate) use tests::make_session_and_context_with_rx;
#[cfg(test)]
pub(crate) use tests::make_session_configuration_for_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::CodexAuth;
    use crate::config::ConfigBuilder;
    use crate::config::test_config;
    use crate::config_loader::ConfigLayerStack;
    use crate::config_loader::ConfigLayerStackOrdering;
    use crate::config_loader::NetworkConstraints;
    use crate::config_loader::RequirementSource;
    use crate::config_loader::Sourced;
    use crate::exec::ExecCapturePolicy;
    use crate::exec::ExecToolCallOutput;
    use crate::function_tool::FunctionCallError;
    use crate::mcp_connection_manager::ToolInfo;
    use crate::models_manager::model_info;
    use crate::shell::default_user_shell;
    use crate::tools::format_exec_output_str;
    use crate::tools::handlers::TOOL_SEARCH_TOOL_NAME;

    use codex_protocol::ThreadId;
    use codex_protocol::models::FunctionCallOutputBody;
    use codex_protocol::models::FunctionCallOutputPayload;
    use tracing::Span;

    use crate::protocol::CompactedItem;
    use crate::protocol::CreditsSnapshot;
    use crate::protocol::InitialHistory;
    use crate::protocol::NetworkApprovalProtocol;
    use crate::protocol::RateLimitSnapshot;
    use crate::protocol::RateLimitWindow;
    use crate::protocol::ResumedHistory;
    use crate::protocol::TokenCountEvent;
    use crate::protocol::TokenUsage;
    use crate::protocol::TokenUsageInfo;
    use crate::protocol::TurnCompleteEvent;
    use crate::protocol::UserMessageEvent;
    use crate::rollout::policy::EventPersistenceMode;
    use crate::rollout::recorder::RolloutRecorder;
    use crate::rollout::recorder::RolloutRecorderParams;
    use crate::state::TaskKind;
    use crate::tasks::SessionTask;
    use crate::tasks::SessionTaskContext;
    use crate::tools::ToolRouter;
    use crate::tools::context::ToolInvocation;
    use crate::tools::context::ToolPayload;
    use crate::tools::handlers::ShellHandler;
    use crate::tools::handlers::UnifiedExecHandler;
    use crate::tools::registry::ToolHandler;
    use crate::tools::router::ToolCallSource;
    use crate::turn_diff_tracker::TurnDiffTracker;
    use codex_app_server_protocol::AppInfo;
    use codex_features::Features;
    use codex_otel::TelemetryAuthMode;
    use codex_protocol::models::BaseInstructions;
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::ResponseInputItem;
    use codex_protocol::models::ResponseItem;
    use codex_protocol::openai_models::ModelsResponse;
    use codex_protocol::protocol::ConversationAudioParams;
    use codex_protocol::protocol::RealtimeAudioFrame;
    use codex_protocol::protocol::Submission;
    use codex_protocol::protocol::W3cTraceContext;
    use opentelemetry::trace::TraceContextExt;
    use opentelemetry::trace::TraceId;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use std::path::Path;
    use std::time::Duration;
    use tokio::time::sleep;
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_subscriber::prelude::*;

    use codex_protocol::mcp::CallToolResult as McpCallToolResult;
    use pretty_assertions::assert_eq;
    use rmcp::model::JsonObject;
    use rmcp::model::Tool;
    use serde::Deserialize;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::Once;
    use std::time::Duration as StdDuration;

    struct InstructionsTestCase {
        slug: &'static str,
        expects_apply_patch_instructions: bool,
    }

    fn user_message(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            end_turn: None,
            phase: None,
        }
    }

    fn skill_message(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            end_turn: None,
            phase: None,
        }
    }

    fn developer_input_texts(items: &[ResponseItem]) -> Vec<&str> {
        items
            .iter()
            .filter_map(|item| match item {
                ResponseItem::Message { role, content, .. } if role == "developer" => {
                    Some(content.as_slice())
                }
                _ => None,
            })
            .flat_map(|content| content.iter())
            .filter_map(|item| match item {
                ContentItem::InputText { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn make_connector(id: &str, name: &str) -> AppInfo {
        AppInfo {
            id: id.to_string(),
            name: name.to_string(),
            description: None,
            logo_url: None,
            logo_url_dark: None,
            distribution_channel: None,
            branding: None,
            app_metadata: None,
            labels: None,
            install_url: None,
            is_accessible: true,
            is_enabled: true,
            plugin_display_names: Vec::new(),
        }
    }

    #[test]
    fn assistant_message_stream_parsers_can_be_seeded_from_output_item_added_text() {
        let mut parsers = AssistantMessageStreamParsers::new(false);
        let item_id = "msg-1";

        let seeded = parsers.seed_item_text(item_id, "hello <oai-mem-citation>doc");
        let parsed = parsers.parse_delta(item_id, "1</oai-mem-citation> world");
        let tail = parsers.finish_item(item_id);

        assert_eq!(seeded.visible_text, "hello ");
        assert_eq!(seeded.citations, Vec::<String>::new());
        assert_eq!(parsed.visible_text, " world");
        assert_eq!(parsed.citations, vec!["doc1".to_string()]);
        assert_eq!(tail.visible_text, "");
        assert_eq!(tail.citations, Vec::<String>::new());
    }

    #[test]
    fn assistant_message_stream_parsers_seed_buffered_prefix_stays_out_of_finish_tail() {
        let mut parsers = AssistantMessageStreamParsers::new(false);
        let item_id = "msg-1";

        let seeded = parsers.seed_item_text(item_id, "hello <oai-mem-");
        let parsed = parsers.parse_delta(item_id, "citation>doc</oai-mem-citation> world");
        let tail = parsers.finish_item(item_id);

        assert_eq!(seeded.visible_text, "hello ");
        assert_eq!(seeded.citations, Vec::<String>::new());
        assert_eq!(parsed.visible_text, " world");
        assert_eq!(parsed.citations, vec!["doc".to_string()]);
        assert_eq!(tail.visible_text, "");
        assert_eq!(tail.citations, Vec::<String>::new());
    }

    #[test]
    fn assistant_message_stream_parsers_seed_plan_parser_across_added_and_delta_boundaries() {
        let mut parsers = AssistantMessageStreamParsers::new(true);
        let item_id = "msg-1";

        let seeded = parsers.seed_item_text(item_id, "Intro\n<proposed");
        let parsed = parsers.parse_delta(item_id, "_plan>\n- step\n</proposed_plan>\nOutro");
        let tail = parsers.finish_item(item_id);

        assert_eq!(seeded.visible_text, "Intro\n");
        assert_eq!(
            seeded.plan_segments,
            vec![ProposedPlanSegment::Normal("Intro\n".to_string())]
        );
        assert_eq!(parsed.visible_text, "Outro");
        assert_eq!(
            parsed.plan_segments,
            vec![
                ProposedPlanSegment::ProposedPlanStart,
                ProposedPlanSegment::ProposedPlanDelta("- step\n".to_string()),
                ProposedPlanSegment::ProposedPlanEnd,
                ProposedPlanSegment::Normal("Outro".to_string()),
            ]
        );
        assert_eq!(tail.visible_text, "");
        assert!(tail.plan_segments.is_empty());
    }

    fn make_mcp_tool(
        server_name: &str,
        tool_name: &str,
        connector_id: Option<&str>,
        connector_name: Option<&str>,
    ) -> ToolInfo {
        let tool_namespace = if server_name == CODEX_APPS_MCP_SERVER_NAME {
            connector_name
                .map(crate::connectors::sanitize_name)
                .map(|connector_name| format!("mcp__{server_name}__{connector_name}"))
                .unwrap_or_else(|| server_name.to_string())
        } else {
            server_name.to_string()
        };

        ToolInfo {
            server_name: server_name.to_string(),
            tool_name: tool_name.to_string(),
            tool_namespace,
            tool: Tool {
                name: tool_name.to_string().into(),
                title: None,
                description: Some(format!("Test tool: {tool_name}").into()),
                input_schema: Arc::new(JsonObject::default()),
                output_schema: None,
                annotations: None,
                execution: None,
                icons: None,
                meta: None,
            },
            connector_id: connector_id.map(str::to_string),
            connector_name: connector_name.map(str::to_string),
            plugin_display_names: Vec::new(),
            connector_description: None,
        }
    }

    fn function_call_rollout_item(name: &str, call_id: &str) -> RolloutItem {
        RolloutItem::ResponseItem(ResponseItem::FunctionCall {
            id: None,
            name: name.to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: call_id.to_string(),
        })
    }

    fn function_call_output_rollout_item(call_id: &str, output: &str) -> RolloutItem {
        RolloutItem::ResponseItem(ResponseItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output: FunctionCallOutputPayload::from_text(output.to_string()),
        })
    }

    #[derive(Deserialize)]
    struct SearchToolSelectionPayload {
        active_selected_tools: Vec<String>,
    }

    fn extract_mcp_tool_selection_from_rollout(
        rollout_items: &[RolloutItem],
    ) -> Option<Vec<String>> {
        let mut search_call_ids = HashSet::new();
        let mut latest_selected_tools = None;

        for item in rollout_items {
            match item {
                RolloutItem::ResponseItem(ResponseItem::FunctionCall { name, call_id, .. })
                    if name == TOOL_SEARCH_TOOL_NAME =>
                {
                    search_call_ids.insert(call_id.clone());
                }
                RolloutItem::ResponseItem(ResponseItem::FunctionCallOutput { call_id, output })
                    if search_call_ids.contains(call_id) =>
                {
                    if let Some(output_text) = output.body.to_text()
                        && let Ok(payload) =
                            serde_json::from_str::<SearchToolSelectionPayload>(&output_text)
                    {
                        latest_selected_tools = Some(payload.active_selected_tools);
                    }
                }
                _ => {}
            }
        }

        latest_selected_tools
    }

    fn filter_mcp_tools_by_name(
        mcp_tools: &HashMap<String, ToolInfo>,
        selected_tool_names: &[String],
    ) -> HashMap<String, ToolInfo> {
        selected_tool_names
            .iter()
            .filter_map(|tool_name| {
                mcp_tools
                    .get(tool_name)
                    .cloned()
                    .map(|tool| (tool_name.clone(), tool))
            })
            .collect()
    }

    fn filter_codex_apps_mcp_tools_only(
        mcp_tools: &HashMap<String, ToolInfo>,
        connectors: &[connectors::AppInfo],
    ) -> HashMap<String, ToolInfo> {
        let config = test_config();
        filter_codex_apps_mcp_tools(mcp_tools, connectors, &config)
    }

    #[test]
    fn validated_network_policy_amendment_host_allows_normalized_match() {
        let amendment = NetworkPolicyAmendment {
            host: "ExAmPlE.Com.:443".to_string(),
            action: NetworkPolicyRuleAction::Allow,
        };
        let context = NetworkApprovalContext {
            host: "example.com".to_string(),
            protocol: NetworkApprovalProtocol::Https,
        };

        let host = Session::validated_network_policy_amendment_host(&amendment, &context)
            .expect("normalized hosts should match");

        assert_eq!(host, "example.com");
    }

    #[test]
    fn validated_network_policy_amendment_host_rejects_mismatch() {
        let amendment = NetworkPolicyAmendment {
            host: "evil.example.com".to_string(),
            action: NetworkPolicyRuleAction::Deny,
        };
        let context = NetworkApprovalContext {
            host: "api.example.com".to_string(),
            protocol: NetworkApprovalProtocol::Https,
        };

        let err = Session::validated_network_policy_amendment_host(&amendment, &context)
            .expect_err("mismatched hosts should be rejected");

        let message = err.to_string();
        assert!(message.contains("does not match approved host"));
    }

    #[tokio::test]
    async fn get_base_instructions_no_user_content() {
        let prompt_with_apply_patch_instructions =
            include_str!("../prompt_with_apply_patch_instructions.md");
        let models_response: ModelsResponse =
            serde_json::from_str(include_str!("../models.json")).expect("valid models.json");
        let model_info_for_slug = |slug: &str, config: &Config| {
            let model = models_response
                .models
                .iter()
                .find(|candidate| candidate.slug == slug)
                .cloned()
                .unwrap_or_else(|| panic!("model slug {slug} is missing from models.json"));
            model_info::with_config_overrides(model, config)
        };
        let test_cases = vec![
            InstructionsTestCase {
                slug: "gpt-5",
                expects_apply_patch_instructions: false,
            },
            InstructionsTestCase {
                slug: "gpt-5.1",
                expects_apply_patch_instructions: false,
            },
            InstructionsTestCase {
                slug: "gpt-5.1-codex",
                expects_apply_patch_instructions: false,
            },
            InstructionsTestCase {
                slug: "gpt-5.1-codex-max",
                expects_apply_patch_instructions: false,
            },
        ];

        let (session, _turn_context) = make_session_and_context().await;
        let config = test_config();

        for test_case in test_cases {
            let model_info = model_info_for_slug(test_case.slug, &config);
            if test_case.expects_apply_patch_instructions {
                assert_eq!(
                    model_info.base_instructions.as_str(),
                    prompt_with_apply_patch_instructions
                );
            }

            {
                let mut state = session.state.lock().await;
                state.session_configuration.base_instructions =
                    model_info.base_instructions.clone();
            }

            let base_instructions = session.get_base_instructions().await;
            assert_eq!(base_instructions.text, model_info.base_instructions);
        }
    }

    #[tokio::test]
    async fn reload_user_config_layer_updates_effective_apps_config() {
        let (session, _turn_context) = make_session_and_context().await;
        let codex_home = session.codex_home().await;
        std::fs::create_dir_all(&codex_home).expect("create codex home");
        let config_toml_path = codex_home.join(CONFIG_TOML_FILE);
        std::fs::write(
            &config_toml_path,
            "[apps.calendar]\nenabled = false\ndestructive_enabled = false\n",
        )
        .expect("write user config");

        session.reload_user_config_layer().await;

        let config = session.get_config().await;
        let apps_toml = config
            .config_layer_stack
            .effective_config()
            .as_table()
            .and_then(|table| table.get("apps"))
            .cloned()
            .expect("apps table");
        let apps = crate::config::types::AppsConfigToml::deserialize(apps_toml)
            .expect("deserialize apps config");
        let app = apps
            .apps
            .get("calendar")
            .expect("calendar app config exists");

        assert!(!app.enabled);
        assert_eq!(app.destructive_enabled, Some(false));
    }

    #[test]
    fn filter_connectors_for_input_skips_duplicate_slug_mentions() {
        let connectors = vec![
            make_connector("one", "Foo Bar"),
            make_connector("two", "Foo-Bar"),
        ];
        let input = vec![user_message("use $foo-bar")];
        let explicitly_enabled_connectors = HashSet::new();
        let skill_name_counts_lower = HashMap::new();

        let selected = filter_connectors_for_input(
            &connectors,
            &input,
            &explicitly_enabled_connectors,
            &skill_name_counts_lower,
        );

        assert_eq!(selected, Vec::new());
    }

    #[test]
    fn filter_connectors_for_input_skips_when_skill_name_conflicts() {
        let connectors = vec![make_connector("one", "Todoist")];
        let input = vec![user_message("use $todoist")];
        let explicitly_enabled_connectors = HashSet::new();
        let skill_name_counts_lower = HashMap::from([("todoist".to_string(), 1)]);

        let selected = filter_connectors_for_input(
            &connectors,
            &input,
            &explicitly_enabled_connectors,
            &skill_name_counts_lower,
        );

        assert_eq!(selected, Vec::new());
    }

    #[test]
    fn filter_connectors_for_input_skips_disabled_connectors() {
        let mut connector = make_connector("calendar", "Calendar");
        connector.is_enabled = false;
        let input = vec![user_message("use $calendar")];
        let explicitly_enabled_connectors = HashSet::new();
        let selected = filter_connectors_for_input(
            &[connector],
            &input,
            &explicitly_enabled_connectors,
            &HashMap::new(),
        );

        assert_eq!(selected, Vec::new());
    }

    #[test]
    fn collect_explicit_app_ids_from_skill_items_includes_linked_mentions() {
        let connectors = vec![make_connector("calendar", "Calendar")];
        let skill_items = vec![skill_message(
            "<skill>\n<name>demo</name>\n<path>/tmp/skills/demo/SKILL.md</path>\nuse [$calendar](app://calendar)\n</skill>",
        )];

        let connector_ids =
            collect_explicit_app_ids_from_skill_items(&skill_items, &connectors, &HashMap::new());

        assert_eq!(connector_ids, HashSet::from(["calendar".to_string()]));
    }

    #[test]
    fn collect_explicit_app_ids_from_skill_items_resolves_unambiguous_plain_mentions() {
        let connectors = vec![make_connector("calendar", "Calendar")];
        let skill_items = vec![skill_message(
            "<skill>\n<name>demo</name>\n<path>/tmp/skills/demo/SKILL.md</path>\nuse $calendar\n</skill>",
        )];

        let connector_ids =
            collect_explicit_app_ids_from_skill_items(&skill_items, &connectors, &HashMap::new());

        assert_eq!(connector_ids, HashSet::from(["calendar".to_string()]));
    }

    #[test]
    fn collect_explicit_app_ids_from_skill_items_skips_plain_mentions_with_skill_conflicts() {
        let connectors = vec![make_connector("calendar", "Calendar")];
        let skill_items = vec![skill_message(
            "<skill>\n<name>demo</name>\n<path>/tmp/skills/demo/SKILL.md</path>\nuse $calendar\n</skill>",
        )];
        let skill_name_counts_lower = HashMap::from([("calendar".to_string(), 1)]);

        let connector_ids = collect_explicit_app_ids_from_skill_items(
            &skill_items,
            &connectors,
            &skill_name_counts_lower,
        );

        assert_eq!(connector_ids, HashSet::<String>::new());
    }

    #[test]
    fn non_app_mcp_tools_remain_visible_without_search_selection() {
        let mcp_tools = HashMap::from([
            (
                "mcp__codex_apps__calendar_create_event".to_string(),
                make_mcp_tool(
                    CODEX_APPS_MCP_SERVER_NAME,
                    "calendar_create_event",
                    Some("calendar"),
                    Some("Calendar"),
                ),
            ),
            (
                "mcp__rmcp__echo".to_string(),
                make_mcp_tool("rmcp", "echo", None, None),
            ),
        ]);

        let mut selected_mcp_tools = mcp_tools
            .iter()
            .filter(|(_, tool)| tool.server_name != CODEX_APPS_MCP_SERVER_NAME)
            .map(|(name, tool)| (name.clone(), tool.clone()))
            .collect::<HashMap<_, _>>();

        let connectors = connectors::accessible_connectors_from_mcp_tools(&mcp_tools);
        let explicitly_enabled_connectors = HashSet::new();
        let connectors = filter_connectors_for_input(
            &connectors,
            &[user_message("run echo")],
            &explicitly_enabled_connectors,
            &HashMap::new(),
        );
        let apps_mcp_tools = filter_codex_apps_mcp_tools_only(&mcp_tools, &connectors);
        selected_mcp_tools.extend(apps_mcp_tools);

        let mut tool_names: Vec<String> = selected_mcp_tools.into_keys().collect();
        tool_names.sort();
        assert_eq!(tool_names, vec!["mcp__rmcp__echo".to_string()]);
    }

    #[test]
    fn search_tool_selection_keeps_codex_apps_tools_without_mentions() {
        let selected_tool_names = vec![
            "mcp__codex_apps__calendar_create_event".to_string(),
            "mcp__rmcp__echo".to_string(),
        ];
        let mcp_tools = HashMap::from([
            (
                "mcp__codex_apps__calendar_create_event".to_string(),
                make_mcp_tool(
                    CODEX_APPS_MCP_SERVER_NAME,
                    "calendar_create_event",
                    Some("calendar"),
                    Some("Calendar"),
                ),
            ),
            (
                "mcp__rmcp__echo".to_string(),
                make_mcp_tool("rmcp", "echo", None, None),
            ),
        ]);

        let mut selected_mcp_tools = filter_mcp_tools_by_name(&mcp_tools, &selected_tool_names);
        let connectors = connectors::accessible_connectors_from_mcp_tools(&mcp_tools);
        let explicitly_enabled_connectors = HashSet::new();
        let connectors = filter_connectors_for_input(
            &connectors,
            &[user_message("run the selected tools")],
            &explicitly_enabled_connectors,
            &HashMap::new(),
        );
        let apps_mcp_tools = filter_codex_apps_mcp_tools_only(&mcp_tools, &connectors);
        selected_mcp_tools.extend(apps_mcp_tools);

        let mut tool_names: Vec<String> = selected_mcp_tools.into_keys().collect();
        tool_names.sort();
        assert_eq!(
            tool_names,
            vec![
                "mcp__codex_apps__calendar_create_event".to_string(),
                "mcp__rmcp__echo".to_string(),
            ]
        );
    }

    #[test]
    fn apps_mentions_add_codex_apps_tools_to_search_selected_set() {
        let selected_tool_names = vec!["mcp__rmcp__echo".to_string()];
        let mcp_tools = HashMap::from([
            (
                "mcp__codex_apps__calendar_create_event".to_string(),
                make_mcp_tool(
                    CODEX_APPS_MCP_SERVER_NAME,
                    "calendar_create_event",
                    Some("calendar"),
                    Some("Calendar"),
                ),
            ),
            (
                "mcp__rmcp__echo".to_string(),
                make_mcp_tool("rmcp", "echo", None, None),
            ),
        ]);

        let mut selected_mcp_tools = filter_mcp_tools_by_name(&mcp_tools, &selected_tool_names);
        let connectors = connectors::accessible_connectors_from_mcp_tools(&mcp_tools);
        let explicitly_enabled_connectors = HashSet::new();
        let connectors = filter_connectors_for_input(
            &connectors,
            &[user_message("use $calendar and then echo the response")],
            &explicitly_enabled_connectors,
            &HashMap::new(),
        );
        let apps_mcp_tools = filter_codex_apps_mcp_tools_only(&mcp_tools, &connectors);
        selected_mcp_tools.extend(apps_mcp_tools);

        let mut tool_names: Vec<String> = selected_mcp_tools.into_keys().collect();
        tool_names.sort();
        assert_eq!(
            tool_names,
            vec![
                "mcp__codex_apps__calendar_create_event".to_string(),
                "mcp__rmcp__echo".to_string(),
            ]
        );
    }

    #[test]
    fn extract_mcp_tool_selection_from_rollout_reads_search_tool_output() {
        let rollout_items = vec![
            function_call_rollout_item(TOOL_SEARCH_TOOL_NAME, "search-1"),
            function_call_output_rollout_item(
                "search-1",
                &json!({
                    "active_selected_tools": [
                        "mcp__codex_apps__calendar_create_event",
                        "mcp__codex_apps__calendar_list_events",
                    ],
                })
                .to_string(),
            ),
        ];

        let selected = extract_mcp_tool_selection_from_rollout(&rollout_items);
        assert_eq!(
            selected,
            Some(vec![
                "mcp__codex_apps__calendar_create_event".to_string(),
                "mcp__codex_apps__calendar_list_events".to_string(),
            ])
        );
    }

    #[test]
    fn extract_mcp_tool_selection_from_rollout_latest_valid_payload_wins() {
        let rollout_items = vec![
            function_call_rollout_item(TOOL_SEARCH_TOOL_NAME, "search-1"),
            function_call_output_rollout_item(
                "search-1",
                &json!({
                    "active_selected_tools": ["mcp__codex_apps__calendar_create_event"],
                })
                .to_string(),
            ),
            function_call_rollout_item(TOOL_SEARCH_TOOL_NAME, "search-2"),
            function_call_output_rollout_item(
                "search-2",
                &json!({
                    "active_selected_tools": ["mcp__codex_apps__calendar_delete_event"],
                })
                .to_string(),
            ),
        ];

        let selected = extract_mcp_tool_selection_from_rollout(&rollout_items);
        assert_eq!(
            selected,
            Some(vec!["mcp__codex_apps__calendar_delete_event".to_string(),])
        );
    }

    #[test]
    fn extract_mcp_tool_selection_from_rollout_ignores_non_search_and_malformed_payloads() {
        let rollout_items = vec![
            function_call_rollout_item("shell", "shell-1"),
            function_call_output_rollout_item(
                "shell-1",
                &json!({
                    "active_selected_tools": ["mcp__codex_apps__should_be_ignored"],
                })
                .to_string(),
            ),
            function_call_rollout_item(TOOL_SEARCH_TOOL_NAME, "search-1"),
            function_call_output_rollout_item("search-1", "{not-json"),
            function_call_output_rollout_item(
                "unknown-search-call",
                &json!({
                    "active_selected_tools": ["mcp__codex_apps__also_ignored"],
                })
                .to_string(),
            ),
            function_call_output_rollout_item(
                "search-1",
                &json!({
                    "active_selected_tools": ["mcp__codex_apps__calendar_list_events"],
                })
                .to_string(),
            ),
        ];

        let selected = extract_mcp_tool_selection_from_rollout(&rollout_items);
        assert_eq!(
            selected,
            Some(vec!["mcp__codex_apps__calendar_list_events".to_string(),])
        );
    }

    #[test]
    fn extract_mcp_tool_selection_from_rollout_returns_none_without_valid_search_output() {
        let rollout_items = vec![function_call_rollout_item(
            TOOL_SEARCH_TOOL_NAME,
            "search-1",
        )];
        let selected = extract_mcp_tool_selection_from_rollout(&rollout_items);
        assert_eq!(selected, None);
    }

    #[tokio::test]
    async fn reconstruct_history_matches_live_compactions() {
        let (session, turn_context) = make_session_and_context().await;
        let (rollout_items, expected) = sample_rollout(&session, &turn_context).await;

        let reconstruction_turn = session.new_default_turn().await;
        let reconstructed = session
            .reconstruct_history_from_rollout(reconstruction_turn.as_ref(), &rollout_items)
            .await;

        assert_eq!(expected, reconstructed.history);
    }

    #[tokio::test]
    async fn reconstruct_history_uses_replacement_history_verbatim() {
        let (session, turn_context) = make_session_and_context().await;
        let summary_item = ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "summary".to_string(),
            }],
            end_turn: None,
            phase: None,
        };
        let replacement_history = vec![
            summary_item.clone(),
            ResponseItem::Message {
                id: None,
                role: "developer".to_string(),
                content: vec![ContentItem::InputText {
                    text: "stale developer instructions".to_string(),
                }],
                end_turn: None,
                phase: None,
            },
        ];
        let rollout_items = vec![RolloutItem::Compacted(CompactedItem {
            message: String::new(),
            replacement_history: Some(replacement_history.clone()),
        })];

        let reconstructed = session
            .reconstruct_history_from_rollout(&turn_context, &rollout_items)
            .await;

        assert_eq!(reconstructed.history, replacement_history);
    }

    #[tokio::test]
    async fn record_initial_history_reconstructs_resumed_transcript() {
        let (session, turn_context) = make_session_and_context().await;
        let (rollout_items, expected) = sample_rollout(&session, &turn_context).await;

        session
            .record_initial_history(InitialHistory::Resumed(ResumedHistory {
                conversation_id: ThreadId::default(),
                history: rollout_items,
                rollout_path: PathBuf::from("/tmp/resume.jsonl"),
            }))
            .await;

        let history = session.state.lock().await.clone_history();
        assert_eq!(expected, history.raw_items());
    }

    #[tokio::test]
    async fn resumed_history_injects_initial_context_on_first_context_update_only() {
        let (session, turn_context) = make_session_and_context().await;
        let (rollout_items, mut expected) = sample_rollout(&session, &turn_context).await;

        session
            .record_initial_history(InitialHistory::Resumed(ResumedHistory {
                conversation_id: ThreadId::default(),
                history: rollout_items,
                rollout_path: PathBuf::from("/tmp/resume.jsonl"),
            }))
            .await;

        let history_before_seed = session.state.lock().await.clone_history();
        assert_eq!(expected, history_before_seed.raw_items());

        session
            .record_context_updates_and_set_reference_context_item(&turn_context)
            .await;
        expected.extend(session.build_initial_context(&turn_context).await);
        let history_after_seed = session.clone_history().await;
        assert_eq!(expected, history_after_seed.raw_items());

        session
            .record_context_updates_and_set_reference_context_item(&turn_context)
            .await;
        let history_after_second_seed = session.clone_history().await;
        assert_eq!(
            history_after_seed.raw_items(),
            history_after_second_seed.raw_items()
        );
    }

    #[tokio::test]
    async fn record_initial_history_seeds_token_info_from_rollout() {
        let (session, turn_context) = make_session_and_context().await;
        let (mut rollout_items, _expected) = sample_rollout(&session, &turn_context).await;

        let info1 = TokenUsageInfo {
            total_token_usage: TokenUsage {
                input_tokens: 10,
                cached_input_tokens: 0,
                output_tokens: 20,
                reasoning_output_tokens: 0,
                total_tokens: 30,
            },
            last_token_usage: TokenUsage {
                input_tokens: 3,
                cached_input_tokens: 0,
                output_tokens: 4,
                reasoning_output_tokens: 0,
                total_tokens: 7,
            },
            model_context_window: Some(1_000),
        };
        let info2 = TokenUsageInfo {
            total_token_usage: TokenUsage {
                input_tokens: 100,
                cached_input_tokens: 50,
                output_tokens: 200,
                reasoning_output_tokens: 25,
                total_tokens: 375,
            },
            last_token_usage: TokenUsage {
                input_tokens: 10,
                cached_input_tokens: 0,
                output_tokens: 20,
                reasoning_output_tokens: 5,
                total_tokens: 35,
            },
            model_context_window: Some(2_000),
        };

        rollout_items.push(RolloutItem::EventMsg(EventMsg::TokenCount(
            TokenCountEvent {
                info: Some(info1),
                rate_limits: None,
            },
        )));
        rollout_items.push(RolloutItem::EventMsg(EventMsg::TokenCount(
            TokenCountEvent {
                info: None,
                rate_limits: None,
            },
        )));
        rollout_items.push(RolloutItem::EventMsg(EventMsg::TokenCount(
            TokenCountEvent {
                info: Some(info2.clone()),
                rate_limits: None,
            },
        )));
        rollout_items.push(RolloutItem::EventMsg(EventMsg::TokenCount(
            TokenCountEvent {
                info: None,
                rate_limits: None,
            },
        )));

        session
            .record_initial_history(InitialHistory::Resumed(ResumedHistory {
                conversation_id: ThreadId::default(),
                history: rollout_items,
                rollout_path: PathBuf::from("/tmp/resume.jsonl"),
            }))
            .await;

        let actual = session.state.lock().await.token_info();
        assert_eq!(actual, Some(info2));
    }

    #[tokio::test]
    async fn recompute_token_usage_uses_session_base_instructions() {
        let (session, turn_context) = make_session_and_context().await;

        let override_instructions = "SESSION_OVERRIDE_INSTRUCTIONS_ONLY".repeat(120);
        {
            let mut state = session.state.lock().await;
            state.session_configuration.base_instructions = override_instructions.clone();
        }

        let item = user_message("hello");
        session
            .record_into_history(std::slice::from_ref(&item), &turn_context)
            .await;

        let history = session.clone_history().await;
        let session_base_instructions = BaseInstructions {
            text: override_instructions,
        };
        let expected_tokens = history
            .estimate_token_count_with_base_instructions(&session_base_instructions)
            .expect("estimate with session base instructions");
        let model_estimated_tokens = history
            .estimate_token_count(&turn_context)
            .expect("estimate with model instructions");
        assert_ne!(expected_tokens, model_estimated_tokens);

        session.recompute_token_usage(&turn_context).await;

        let actual_tokens = session
            .state
            .lock()
            .await
            .token_info()
            .expect("token info")
            .last_token_usage
            .total_tokens;
        assert_eq!(actual_tokens, expected_tokens.max(0));
    }

    #[tokio::test]
    async fn recompute_token_usage_updates_model_context_window() {
        let (session, mut turn_context) = make_session_and_context().await;

        {
            let mut state = session.state.lock().await;
            state.set_token_info(Some(TokenUsageInfo {
                total_token_usage: TokenUsage::default(),
                last_token_usage: TokenUsage::default(),
                model_context_window: Some(258_400),
            }));
        }

        turn_context.model_info.context_window = Some(128_000);
        turn_context.model_info.effective_context_window_percent = 100;

        session.recompute_token_usage(&turn_context).await;

        let actual = session.state.lock().await.token_info().expect("token info");
        assert_eq!(actual.model_context_window, Some(128_000));
    }

    #[tokio::test]
    async fn record_initial_history_reconstructs_forked_transcript() {
        let (session, turn_context) = make_session_and_context().await;
        let (rollout_items, expected) = sample_rollout(&session, &turn_context).await;

        session
            .record_initial_history(InitialHistory::Forked(rollout_items))
            .await;

        let history = session.state.lock().await.clone_history();
        assert_eq!(expected, history.raw_items());
    }

    #[tokio::test]
    async fn record_initial_history_forked_hydrates_previous_turn_settings() {
        let (session, turn_context) = make_session_and_context().await;
        let previous_model = "forked-rollout-model";
        let previous_context_item = TurnContextItem {
            turn_id: Some(turn_context.sub_id.clone()),
            trace_id: turn_context.trace_id.clone(),
            cwd: turn_context.cwd.clone().to_path_buf(),
            current_date: turn_context.current_date.clone(),
            timezone: turn_context.timezone.clone(),
            approval_policy: turn_context.approval_policy.value(),
            sandbox_policy: turn_context.sandbox_policy.get().clone(),
            network: None,
            model: previous_model.to_string(),
            personality: turn_context.personality,
            collaboration_mode: Some(turn_context.collaboration_mode.clone()),
            realtime_active: Some(turn_context.realtime_active),
            effort: turn_context.reasoning_effort,
            summary: turn_context.reasoning_summary,
            user_instructions: None,
            developer_instructions: None,
            final_output_json_schema: None,
            truncation_policy: Some(turn_context.truncation_policy),
        };
        let turn_id = previous_context_item
            .turn_id
            .clone()
            .expect("turn context should have turn_id");
        let rollout_items = vec![
            RolloutItem::EventMsg(EventMsg::TurnStarted(
                codex_protocol::protocol::TurnStartedEvent {
                    turn_id: turn_id.clone(),
                    model_context_window: Some(128_000),
                    collaboration_mode_kind: ModeKind::Default,
                },
            )),
            RolloutItem::EventMsg(EventMsg::UserMessage(
                codex_protocol::protocol::UserMessageEvent {
                    message: "forked seed".to_string(),
                    images: None,
                    local_images: Vec::new(),
                    text_elements: Vec::new(),
                },
            )),
            RolloutItem::TurnContext(previous_context_item),
            RolloutItem::EventMsg(EventMsg::TurnComplete(
                codex_protocol::protocol::TurnCompleteEvent {
                    turn_id,
                    last_agent_message: None,
                },
            )),
        ];

        session
            .record_initial_history(InitialHistory::Forked(rollout_items))
            .await;

        assert_eq!(
            session.previous_turn_settings().await,
            Some(PreviousTurnSettings {
                model: previous_model.to_string(),
                realtime_active: Some(turn_context.realtime_active),
            })
        );
    }

    #[tokio::test]
    async fn thread_rollback_fails_when_turn_in_progress() {
        let (sess, tc, rx) = make_session_and_context_with_rx().await;

        let initial_context = sess.build_initial_context(tc.as_ref()).await;
        sess.record_into_history(&initial_context, tc.as_ref())
            .await;

        *sess.active_turn.lock().await = Some(crate::state::ActiveTurn::default());
        handlers::thread_rollback(&sess, "sub-1".to_string(), 1).await;

        let error_event = wait_for_thread_rollback_failed(&rx).await;
        assert_eq!(
            error_event.codex_error_info,
            Some(CodexErrorInfo::ThreadRollbackFailed)
        );

        let history = sess.clone_history().await;
        assert_eq!(initial_context, history.raw_items());
    }

    #[tokio::test]
    async fn thread_rollback_fails_when_num_turns_is_zero() {
        let (sess, tc, rx) = make_session_and_context_with_rx().await;

        let initial_context = sess.build_initial_context(tc.as_ref()).await;
        sess.record_into_history(&initial_context, tc.as_ref())
            .await;

        handlers::thread_rollback(&sess, "sub-1".to_string(), 0).await;

        let error_event = wait_for_thread_rollback_failed(&rx).await;
        assert_eq!(error_event.message, "num_turns must be >= 1");
        assert_eq!(
            error_event.codex_error_info,
            Some(CodexErrorInfo::ThreadRollbackFailed)
        );

        let history = sess.clone_history().await;
        assert_eq!(initial_context, history.raw_items());
    }

    #[tokio::test]
    async fn set_rate_limits_retains_previous_credits() {
        let codex_home = tempfile::tempdir().expect("create temp dir");
        let config = build_test_config(codex_home.path()).await;
        let config = Arc::new(config);
        let model = ModelsManager::get_model_offline_for_tests(config.model.as_deref());
        let model_info =
            ModelsManager::construct_model_info_offline_for_tests(model.as_str(), &config);
        let reasoning_effort = config.model_reasoning_effort;
        let collaboration_mode = CollaborationMode {
            mode: ModeKind::Default,
            settings: Settings {
                model,
                reasoning_effort,
                developer_instructions: None,
            },
        };
        let session_configuration = SessionConfiguration {
            provider: config.model_provider.clone(),
            collaboration_mode,
            model_reasoning_summary: config.model_reasoning_summary,
            user_instructions: config.user_instructions.clone(),
            service_tier: None,
            personality: config.personality,
            base_instructions: config
                .base_instructions
                .clone()
                .unwrap_or_else(|| model_info.get_model_instructions(config.personality)),
            compact_prompt: config.compact_prompt.clone(),
            approval_policy: config.permissions.approval_policy.clone(),
            approvals_reviewer: config.approvals_reviewer,
            sandbox_policy: config.permissions.sandbox_policy.clone(),
            file_system_sandbox_policy: config.permissions.file_system_sandbox_policy.clone(),
            network_sandbox_policy: config.permissions.network_sandbox_policy,
            windows_sandbox_level: WindowsSandboxLevel::from_config(&config),
            cwd: config.cwd.clone(),
            codex_home: config.codex_home.clone(),
            thread_name: None,
            original_config_do_not_use: Arc::clone(&config),
            metrics_service_name: None,
            app_server_client_name: None,
            session_source: SessionSource::Exec,
            nero_auto_runtime: NeroAutoRuntimeConfig::default(),
            nero_model_fallback: None,
            dynamic_tools: Vec::new(),
            persist_extended_history: false,
            inherited_shell_snapshot: None,
            user_shell_override: None,
        };

        let mut state = SessionState::new(session_configuration);
        let initial = RateLimitSnapshot {
            limit_id: None,
            limit_name: None,
            primary: Some(RateLimitWindow {
                used_percent: 10.0,
                window_minutes: Some(15),
                resets_at: Some(1_700),
            }),
            secondary: None,
            credits: Some(CreditsSnapshot {
                has_credits: true,
                unlimited: false,
                balance: Some("10.00".to_string()),
            }),
            plan_type: Some(codex_protocol::account::PlanType::Plus),
        };
        state.set_rate_limits(initial.clone());

        let update = RateLimitSnapshot {
            limit_id: Some("codex_other".to_string()),
            limit_name: Some("codex_other".to_string()),
            primary: Some(RateLimitWindow {
                used_percent: 40.0,
                window_minutes: Some(30),
                resets_at: Some(1_800),
            }),
            secondary: Some(RateLimitWindow {
                used_percent: 5.0,
                window_minutes: Some(60),
                resets_at: Some(1_900),
            }),
            credits: None,
            plan_type: None,
        };
        state.set_rate_limits(update.clone());

        assert_eq!(
            state.latest_rate_limits,
            Some(RateLimitSnapshot {
                limit_id: Some("codex_other".to_string()),
                limit_name: Some("codex_other".to_string()),
                primary: update.primary.clone(),
                secondary: update.secondary,
                credits: initial.credits,
                plan_type: initial.plan_type,
            })
        );
    }

    #[tokio::test]
    async fn set_rate_limits_updates_plan_type_when_present() {
        let codex_home = tempfile::tempdir().expect("create temp dir");
        let config = build_test_config(codex_home.path()).await;
        let config = Arc::new(config);
        let model = ModelsManager::get_model_offline_for_tests(config.model.as_deref());
        let model_info =
            ModelsManager::construct_model_info_offline_for_tests(model.as_str(), &config);
        let reasoning_effort = config.model_reasoning_effort;
        let collaboration_mode = CollaborationMode {
            mode: ModeKind::Default,
            settings: Settings {
                model,
                reasoning_effort,
                developer_instructions: None,
            },
        };
        let session_configuration = SessionConfiguration {
            provider: config.model_provider.clone(),
            collaboration_mode,
            model_reasoning_summary: config.model_reasoning_summary,
            user_instructions: config.user_instructions.clone(),
            service_tier: None,
            personality: config.personality,
            base_instructions: config
                .base_instructions
                .clone()
                .unwrap_or_else(|| model_info.get_model_instructions(config.personality)),
            compact_prompt: config.compact_prompt.clone(),
            approval_policy: config.permissions.approval_policy.clone(),
            approvals_reviewer: config.approvals_reviewer,
            sandbox_policy: config.permissions.sandbox_policy.clone(),
            file_system_sandbox_policy: config.permissions.file_system_sandbox_policy.clone(),
            network_sandbox_policy: config.permissions.network_sandbox_policy,
            windows_sandbox_level: WindowsSandboxLevel::from_config(&config),
            cwd: config.cwd.clone(),
            codex_home: config.codex_home.clone(),
            thread_name: None,
            original_config_do_not_use: Arc::clone(&config),
            metrics_service_name: None,
            app_server_client_name: None,
            session_source: SessionSource::Exec,
            nero_auto_runtime: NeroAutoRuntimeConfig::default(),
            nero_model_fallback: None,
            dynamic_tools: Vec::new(),
            persist_extended_history: false,
            inherited_shell_snapshot: None,
            user_shell_override: None,
        };

        let mut state = SessionState::new(session_configuration);
        let initial = RateLimitSnapshot {
            limit_id: None,
            limit_name: None,
            primary: Some(RateLimitWindow {
                used_percent: 15.0,
                window_minutes: Some(20),
                resets_at: Some(1_600),
            }),
            secondary: Some(RateLimitWindow {
                used_percent: 5.0,
                window_minutes: Some(45),
                resets_at: Some(1_650),
            }),
            credits: Some(CreditsSnapshot {
                has_credits: true,
                unlimited: false,
                balance: Some("15.00".to_string()),
            }),
            plan_type: Some(codex_protocol::account::PlanType::Plus),
        };
        state.set_rate_limits(initial.clone());

        let update = RateLimitSnapshot {
            limit_id: None,
            limit_name: None,
            primary: Some(RateLimitWindow {
                used_percent: 35.0,
                window_minutes: Some(25),
                resets_at: Some(1_700),
            }),
            secondary: None,
            credits: None,
            plan_type: Some(codex_protocol::account::PlanType::Pro),
        };
        state.set_rate_limits(update.clone());

        assert_eq!(
            state.latest_rate_limits,
            Some(RateLimitSnapshot {
                limit_id: Some("codex".to_string()),
                limit_name: None,
                primary: update.primary,
                secondary: update.secondary,
                credits: initial.credits,
                plan_type: update.plan_type,
            })
        );
    }

    #[test]
    fn prefers_structured_content_when_present() {
        let ctr = McpCallToolResult {
            // Content present but should be ignored because structured_content is set.
            content: vec![text_block("ignored")],
            is_error: None,
            structured_content: Some(json!({
                "ok": true,
                "value": 42
            })),
            meta: None,
        };

        let got = ctr.into_function_call_output_payload();
        let expected = FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(
                serde_json::to_string(&json!({
                    "ok": true,
                    "value": 42
                }))
                .unwrap(),
            ),
            success: Some(true),
        };

        assert_eq!(expected, got);
    }

    #[tokio::test]
    async fn includes_timed_out_message() {
        let exec = ExecToolCallOutput {
            exit_code: 0,
            stdout: StreamOutput::new(String::new()),
            stderr: StreamOutput::new(String::new()),
            aggregated_output: StreamOutput::new("Command output".to_string()),
            duration: StdDuration::from_secs(1),
            timed_out: true,
        };
        let (_, turn_context) = make_session_and_context().await;

        let out = format_exec_output_str(&exec, turn_context.truncation_policy);

        assert_eq!(
            out,
            "command timed out after 1000 milliseconds\nCommand output"
        );
    }

    #[tokio::test]
    async fn turn_context_with_model_updates_model_fields() {
        let (session, mut turn_context) = make_session_and_context().await;
        turn_context.reasoning_effort = Some(ReasoningEffortConfig::Minimal);
        let updated = turn_context
            .with_model("gpt-5.1".to_string(), &session.services.models_manager)
            .await;
        let expected_model_info = session
            .services
            .models_manager
            .get_model_info("gpt-5.1", updated.config.as_ref())
            .await;

        assert_eq!(updated.config.model.as_deref(), Some("gpt-5.1"));
        assert_eq!(updated.collaboration_mode.model(), "gpt-5.1");
        assert_eq!(updated.model_info, expected_model_info);
        assert_eq!(
            updated.reasoning_effort,
            Some(ReasoningEffortConfig::Medium)
        );
        assert_eq!(
            updated.collaboration_mode.reasoning_effort(),
            Some(ReasoningEffortConfig::Medium)
        );
        assert_eq!(
            updated.config.model_reasoning_effort,
            Some(ReasoningEffortConfig::Medium)
        );
        assert_eq!(
            updated.truncation_policy,
            expected_model_info.truncation_policy.into()
        );
        assert!(!Arc::ptr_eq(
            &updated.tool_call_gate,
            &turn_context.tool_call_gate
        ));
    }

    #[test]
    fn falls_back_to_content_when_structured_is_null() {
        let ctr = McpCallToolResult {
            content: vec![text_block("hello"), text_block("world")],
            is_error: None,
            structured_content: Some(serde_json::Value::Null),
            meta: None,
        };

        let got = ctr.into_function_call_output_payload();
        let expected = FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(
                serde_json::to_string(&vec![text_block("hello"), text_block("world")]).unwrap(),
            ),
            success: Some(true),
        };

        assert_eq!(expected, got);
    }

    #[test]
    fn success_flag_reflects_is_error_true() {
        let ctr = McpCallToolResult {
            content: vec![text_block("unused")],
            is_error: Some(true),
            structured_content: Some(json!({ "message": "bad" })),
            meta: None,
        };

        let got = ctr.into_function_call_output_payload();
        let expected = FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(
                serde_json::to_string(&json!({ "message": "bad" })).unwrap(),
            ),
            success: Some(false),
        };

        assert_eq!(expected, got);
    }

    #[test]
    fn success_flag_true_with_no_error_and_content_used() {
        let ctr = McpCallToolResult {
            content: vec![text_block("alpha")],
            is_error: Some(false),
            structured_content: None,
            meta: None,
        };

        let got = ctr.into_function_call_output_payload();
        let expected = FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(
                serde_json::to_string(&vec![text_block("alpha")]).unwrap(),
            ),
            success: Some(true),
        };

        assert_eq!(expected, got);
    }

    async fn wait_for_thread_rolled_back(
        rx: &async_channel::Receiver<Event>,
    ) -> crate::protocol::ThreadRolledBackEvent {
        let deadline = StdDuration::from_secs(30);
        let start = std::time::Instant::now();
        loop {
            let remaining = deadline.saturating_sub(start.elapsed());
            let evt = tokio::time::timeout(remaining, rx.recv())
                .await
                .expect("timeout waiting for event")
                .expect("event");
            match evt.msg {
                EventMsg::ThreadRolledBack(payload) => return payload,
                _ => continue,
            }
        }
    }

    async fn wait_for_thread_rollback_failed(rx: &async_channel::Receiver<Event>) -> ErrorEvent {
        let deadline = StdDuration::from_secs(2);
        let start = std::time::Instant::now();
        loop {
            let remaining = deadline.saturating_sub(start.elapsed());
            let evt = tokio::time::timeout(remaining, rx.recv())
                .await
                .expect("timeout waiting for event")
                .expect("event");
            match evt.msg {
                EventMsg::Error(payload)
                    if payload.codex_error_info == Some(CodexErrorInfo::ThreadRollbackFailed) =>
                {
                    return payload;
                }
                _ => continue,
            }
        }
    }

    fn text_block(s: &str) -> serde_json::Value {
        json!({
            "type": "text",
            "text": s,
        })
    }

    fn init_test_tracing() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let provider = SdkTracerProvider::builder().build();
            let tracer = provider.tracer("codex-core-tests");
            let subscriber = tracing_subscriber::registry()
                .with(tracing_opentelemetry::layer().with_tracer(tracer));
            tracing::subscriber::set_global_default(subscriber)
                .expect("global tracing subscriber should only be installed once");
        });
    }

    async fn build_test_config(codex_home: &Path) -> Config {
        ConfigBuilder::default()
            .codex_home(codex_home.to_path_buf())
            .build()
            .await
            .expect("load default test config")
    }

    fn session_telemetry(
        conversation_id: ThreadId,
        config: &Config,
        model_info: &ModelInfo,
        session_source: SessionSource,
    ) -> SessionTelemetry {
        SessionTelemetry::new(
            conversation_id,
            ModelsManager::get_model_offline_for_tests(config.model.as_deref()).as_str(),
            model_info.slug.as_str(),
            None,
            Some("test@test.com".to_string()),
            Some(TelemetryAuthMode::Chatgpt),
            "test_originator".to_string(),
            false,
            "test".to_string(),
            session_source,
        )
    }

    pub(crate) async fn make_session_configuration_for_tests() -> SessionConfiguration {
        let codex_home = tempfile::tempdir().expect("create temp dir");
        let config = build_test_config(codex_home.path()).await;
        let config = Arc::new(config);
        let model = ModelsManager::get_model_offline_for_tests(config.model.as_deref());
        let model_info =
            ModelsManager::construct_model_info_offline_for_tests(model.as_str(), &config);
        let reasoning_effort = config.model_reasoning_effort;
        let collaboration_mode = CollaborationMode {
            mode: ModeKind::Default,
            settings: Settings {
                model,
                reasoning_effort,
                developer_instructions: None,
            },
        };

        SessionConfiguration {
            provider: config.model_provider.clone(),
            collaboration_mode,
            model_reasoning_summary: config.model_reasoning_summary,
            user_instructions: config.user_instructions.clone(),
            service_tier: None,
            personality: config.personality,
            base_instructions: config
                .base_instructions
                .clone()
                .unwrap_or_else(|| model_info.get_model_instructions(config.personality)),
            compact_prompt: config.compact_prompt.clone(),
            approval_policy: config.permissions.approval_policy.clone(),
            approvals_reviewer: config.approvals_reviewer,
            sandbox_policy: config.permissions.sandbox_policy.clone(),
            file_system_sandbox_policy: config.permissions.file_system_sandbox_policy.clone(),
            network_sandbox_policy: config.permissions.network_sandbox_policy,
            windows_sandbox_level: WindowsSandboxLevel::from_config(&config),
            cwd: config.cwd.clone(),
            codex_home: config.codex_home.clone(),
            thread_name: None,
            original_config_do_not_use: Arc::clone(&config),
            metrics_service_name: None,
            app_server_client_name: None,
            session_source: SessionSource::Exec,
            nero_auto_runtime: NeroAutoRuntimeConfig::default(),
            nero_model_fallback: None,
            dynamic_tools: Vec::new(),
            persist_extended_history: false,
            inherited_shell_snapshot: None,
            user_shell_override: None,
        }
    }

    #[test]
    fn effective_nero_auto_runtime_forces_subagent_sessions_off() {
        let runtime = effective_nero_auto_runtime(
            NeroAutoRuntimeConfig {
                enabled: true,
                autonomy_level: 9,
                max_auto_rounds: 2,
            },
            &SessionSource::SubAgent(SubAgentSource::Other("reviewer".to_string())),
        );

        assert_eq!(
            runtime,
            NeroAutoRuntimeConfig {
                enabled: false,
                autonomy_level: 9,
                max_auto_rounds: 2,
            }
        );
    }

    #[tokio::test]
    async fn session_configuration_apply_keeps_nero_auto_off_for_subagents() {
        let mut session_configuration = make_session_configuration_for_tests().await;
        session_configuration.session_source =
            SessionSource::SubAgent(SubAgentSource::Other("worker".to_string()));
        session_configuration.nero_auto_runtime = NeroAutoRuntimeConfig {
            enabled: false,
            autonomy_level: 5,
            max_auto_rounds: 7,
        };

        let updated = session_configuration
            .apply(&SessionSettingsUpdate {
                nero_auto_runtime: Some(NeroAutoRuntimeConfig {
                    enabled: true,
                    autonomy_level: 8,
                    max_auto_rounds: 3,
                }),
                ..Default::default()
            })
            .expect("subagent override should apply with forced auto-off");

        assert_eq!(
            updated.nero_auto_runtime,
            NeroAutoRuntimeConfig {
                enabled: false,
                autonomy_level: 8,
                max_auto_rounds: 3,
            }
        );
    }

    #[tokio::test]
    async fn session_new_fails_when_zsh_fork_enabled_without_zsh_path() {
        let codex_home = tempfile::tempdir().expect("create temp dir");
        let mut config = build_test_config(codex_home.path()).await;
        config
            .features
            .enable(Feature::ShellZshFork)
            .expect("test config should allow shell_zsh_fork");
        config.zsh_path = None;
        let config = Arc::new(config);

        let auth_manager =
            AuthManager::from_auth_for_testing(CodexAuth::from_api_key("Test API Key"));
        let models_manager = Arc::new(ModelsManager::new(
            config.codex_home.clone(),
            auth_manager.clone(),
            None,
            CollaborationModesConfig::default(),
        ));
        let model = ModelsManager::get_model_offline_for_tests(config.model.as_deref());
        let model_info =
            ModelsManager::construct_model_info_offline_for_tests(model.as_str(), &config);
        let collaboration_mode = CollaborationMode {
            mode: ModeKind::Default,
            settings: Settings {
                model,
                reasoning_effort: config.model_reasoning_effort,
                developer_instructions: None,
            },
        };
        let session_configuration = SessionConfiguration {
            provider: config.model_provider.clone(),
            collaboration_mode,
            model_reasoning_summary: config.model_reasoning_summary,
            user_instructions: config.user_instructions.clone(),
            service_tier: None,
            personality: config.personality,
            base_instructions: config
                .base_instructions
                .clone()
                .unwrap_or_else(|| model_info.get_model_instructions(config.personality)),
            compact_prompt: config.compact_prompt.clone(),
            approval_policy: config.permissions.approval_policy.clone(),
            approvals_reviewer: config.approvals_reviewer,
            sandbox_policy: config.permissions.sandbox_policy.clone(),
            file_system_sandbox_policy: config.permissions.file_system_sandbox_policy.clone(),
            network_sandbox_policy: config.permissions.network_sandbox_policy,
            windows_sandbox_level: WindowsSandboxLevel::from_config(&config),
            cwd: config.cwd.clone(),
            codex_home: config.codex_home.clone(),
            thread_name: None,
            original_config_do_not_use: Arc::clone(&config),
            metrics_service_name: None,
            app_server_client_name: None,
            session_source: SessionSource::Exec,
            nero_auto_runtime: NeroAutoRuntimeConfig::default(),
            nero_model_fallback: None,
            dynamic_tools: Vec::new(),
            persist_extended_history: false,
            inherited_shell_snapshot: None,
            user_shell_override: None,
        };

        let (tx_event, _rx_event) = async_channel::unbounded();
        let (tx_sub, _rx_sub) = async_channel::bounded::<Submission>(SUBMISSION_CHANNEL_CAPACITY);
        let (agent_status_tx, _agent_status_rx) = watch::channel(AgentStatus::PendingInit);
        let plugins_manager = Arc::new(PluginsManager::new(config.codex_home.clone()));
        let mcp_manager = Arc::new(McpManager::new(Arc::clone(&plugins_manager)));
        let skills_manager = Arc::new(SkillsManager::new(
            config.codex_home.clone(),
            config.bundled_skills_enabled(),
        ));
        let result = Session::new(
            session_configuration,
            Arc::clone(&config),
            auth_manager,
            models_manager,
            Arc::new(ExecPolicyManager::default()),
            tx_sub,
            tx_event,
            agent_status_tx,
            InitialHistory::New,
            SessionSource::Exec,
            Arc::new(codex_exec_server::EnvironmentManager::new(
                /*exec_server_url*/ None,
            )),
            skills_manager,
            plugins_manager,
            mcp_manager,
            Arc::new(SkillsWatcher::noop()),
            AgentControl::default(),
        )
        .await;

        let err = match result {
            Ok(_) => panic!("expected startup to fail"),
            Err(err) => err,
        };
        let msg = format!("{err:#}");
        assert!(msg.contains("zsh fork feature enabled, but `zsh_path` is not configured"));
    }

    // todo: use online model info
    pub(crate) async fn make_session_and_context() -> (Session, TurnContext) {
        let (tx_event, _rx_event) = async_channel::unbounded();
        let (tx_sub, _rx_sub) = async_channel::bounded::<Submission>(SUBMISSION_CHANNEL_CAPACITY);
        let codex_home = tempfile::tempdir().expect("create temp dir");
        let config = build_test_config(codex_home.path()).await;
        let config = Arc::new(config);
        let conversation_id = ThreadId::default();
        let auth_manager =
            AuthManager::from_auth_for_testing(CodexAuth::from_api_key("Test API Key"));
        let models_manager = Arc::new(ModelsManager::new(
            config.codex_home.clone(),
            auth_manager.clone(),
            None,
            CollaborationModesConfig::default(),
        ));
        let agent_control = AgentControl::default();
        let exec_policy = Arc::new(ExecPolicyManager::default());
        let (agent_status_tx, _agent_status_rx) = watch::channel(AgentStatus::PendingInit);
        let model = ModelsManager::get_model_offline_for_tests(config.model.as_deref());
        let model_info =
            ModelsManager::construct_model_info_offline_for_tests(model.as_str(), &config);
        let reasoning_effort = config.model_reasoning_effort;
        let collaboration_mode = CollaborationMode {
            mode: ModeKind::Default,
            settings: Settings {
                model,
                reasoning_effort,
                developer_instructions: None,
            },
        };
        let session_configuration = SessionConfiguration {
            provider: config.model_provider.clone(),
            collaboration_mode,
            model_reasoning_summary: config.model_reasoning_summary,
            user_instructions: config.user_instructions.clone(),
            service_tier: None,
            personality: config.personality,
            base_instructions: config
                .base_instructions
                .clone()
                .unwrap_or_else(|| model_info.get_model_instructions(config.personality)),
            compact_prompt: config.compact_prompt.clone(),
            approval_policy: config.permissions.approval_policy.clone(),
            approvals_reviewer: config.approvals_reviewer,
            sandbox_policy: config.permissions.sandbox_policy.clone(),
            file_system_sandbox_policy: config.permissions.file_system_sandbox_policy.clone(),
            network_sandbox_policy: config.permissions.network_sandbox_policy,
            windows_sandbox_level: WindowsSandboxLevel::from_config(&config),
            cwd: config.cwd.clone(),
            codex_home: config.codex_home.clone(),
            thread_name: None,
            original_config_do_not_use: Arc::clone(&config),
            metrics_service_name: None,
            app_server_client_name: None,
            session_source: SessionSource::Exec,
            nero_auto_runtime: NeroAutoRuntimeConfig::default(),
            nero_model_fallback: None,
            dynamic_tools: Vec::new(),
            persist_extended_history: false,
            inherited_shell_snapshot: None,
            user_shell_override: None,
        };
        let per_turn_config = Session::build_per_turn_config(&session_configuration);
        let model_info = ModelsManager::construct_model_info_offline_for_tests(
            session_configuration.collaboration_mode.model(),
            &per_turn_config,
        );
        let session_telemetry = session_telemetry(
            conversation_id,
            config.as_ref(),
            &model_info,
            session_configuration.session_source.clone(),
        );

        let state = SessionState::new(session_configuration.clone());
        let plugins_manager = Arc::new(PluginsManager::new(config.codex_home.clone()));
        let mcp_manager = Arc::new(McpManager::new(Arc::clone(&plugins_manager)));
        let skills_manager = Arc::new(SkillsManager::new(
            config.codex_home.clone(),
            config.bundled_skills_enabled(),
        ));
        let network_approval = Arc::new(NetworkApprovalService::default());
        let environment = Arc::new(
            codex_exec_server::Environment::create(/*exec_server_url*/ None)
                .await
                .expect("create environment"),
        );

        let user_shell = Arc::new(default_user_shell());
        let mut hook_shell_argv = user_shell.derive_exec_args("", false);
        let hook_shell_program = hook_shell_argv.remove(0);
        let _ = hook_shell_argv.pop();
        let skills_watcher = Arc::new(SkillsWatcher::noop());
        let services = SessionServices {
            mcp_connection_manager: Arc::new(RwLock::new(
                McpConnectionManager::new_mcp_connection_manager_for_tests(
                    &config.permissions.approval_policy,
                ),
            )),
            mcp_startup_cancellation_token: Mutex::new(CancellationToken::new()),
            unified_exec_manager: UnifiedExecProcessManager::new(
                config.background_terminal_max_timeout,
            ),
            shell_zsh_path: None,
            main_execve_wrapper_exe: config.main_execve_wrapper_exe.clone(),
            analytics_events_client: AnalyticsEventsClient::new(
                Arc::clone(&auth_manager),
                config.chatgpt_base_url.trim_end_matches('/').to_string(),
                config.analytics_enabled,
            ),
            hooks: Hooks::new(HooksConfig {
                legacy_notify_argv: config.notify.clone(),
                feature_enabled: config.features.enabled(Feature::CodexHooks),
                config_layer_stack: Some(config.config_layer_stack.clone()),
                shell_program: Some(hook_shell_program),
                shell_args: hook_shell_argv,
            }),
            rollout: Mutex::new(None),
            user_shell: Arc::clone(&user_shell),
            shell_snapshot_tx: watch::channel(None).0,
            show_raw_agent_reasoning: config.show_raw_agent_reasoning,
            exec_policy,
            auth_manager: auth_manager.clone(),
            session_telemetry: session_telemetry.clone(),
            models_manager: Arc::clone(&models_manager),
            tool_approvals: Mutex::new(ApprovalStore::default()),
            skills_manager,
            plugins_manager,
            mcp_manager,
            skills_watcher,
            agent_control,
            network_proxy: None,
            network_approval: Arc::clone(&network_approval),
            state_db: None,
            model_client: ModelClient::new(
                Some(auth_manager.clone()),
                conversation_id,
                session_configuration.provider.clone(),
                session_configuration.session_source.clone(),
                config.model_verbosity,
                config.features.enabled(Feature::EnableRequestCompression),
                config.features.enabled(Feature::RuntimeMetrics),
                Session::build_model_client_beta_features_header(config.as_ref()),
            ),
            code_mode_service: crate::tools::code_mode::CodeModeService::new(
                config.js_repl_node_path.clone(),
            ),
            environment: Arc::clone(&environment),
        };
        let js_repl = Arc::new(JsReplHandle::with_node_path(
            config.js_repl_node_path.clone(),
            config.js_repl_node_module_dirs.clone(),
        ));
        let (out_of_band_elicitation_paused, _out_of_band_elicitation_paused_rx) =
            watch::channel(false);
        let (mailbox, mailbox_rx) = Mailbox::new();

        let plugin_outcome = services
            .plugins_manager
            .plugins_for_config(&per_turn_config);
        let effective_skill_roots = plugin_outcome.effective_skill_roots();
        let skills_input =
            crate::skills_load_input_from_config(&per_turn_config, effective_skill_roots);
        let skills_outcome = Arc::new(services.skills_manager.skills_for_config(&skills_input));
        let turn_context = Session::make_turn_context(
            conversation_id,
            Some(Arc::clone(&auth_manager)),
            &session_telemetry,
            session_configuration.provider.clone(),
            &session_configuration,
            services.user_shell.as_ref(),
            services.shell_zsh_path.as_ref(),
            services.main_execve_wrapper_exe.as_ref(),
            per_turn_config,
            model_info,
            &models_manager,
            None,
            environment,
            "turn_id".to_string(),
            Arc::clone(&js_repl),
            skills_outcome,
        );

        let session = Session {
            conversation_id,
            tx_sub,
            tx_event,
            agent_status: agent_status_tx,
            state: Mutex::new(state),
            features: config.features.clone(),
            pending_mcp_server_refresh_config: Mutex::new(None),
            conversation: Arc::new(RealtimeConversationManager::new()),
            active_turn: Mutex::new(None),
            mailbox,
            mailbox_rx: Mutex::new(mailbox_rx),
            idle_pending_input: Mutex::new(Vec::new()),
            guardian_review_session: GuardianReviewSessionManager::default(),
            services,
            js_repl,
            hook_seen_terminal_turn_ids: Mutex::new(HashSet::new()),
            hook_auto_reply_internal_submission_ids: StdMutex::new(HashSet::new()),
            hook_nero_msg_throttle: StdMutex::new(HashMap::new()),
            nero_auto_bridge_warning_emitted: StdMutex::new(false),
            hook_auto_reply_guard_state: StdMutex::new(HookAutoReplyGuardState::default()),
            out_of_band_elicitation_paused,
            next_internal_sub_id: AtomicU64::new(0),
        };

        (session, turn_context)
    }

    #[tokio::test]
    async fn submit_with_id_captures_current_span_trace_context() {
        let (session, _turn_context) = make_session_and_context().await;
        let (tx_sub, rx_sub) = async_channel::bounded(1);
        let (_tx_event, rx_event) = async_channel::unbounded();
        let (_agent_status_tx, agent_status) = watch::channel(AgentStatus::PendingInit);
        let codex = Codex {
            tx_sub,
            rx_event,
            agent_status,
            session: Arc::new(session),
            session_loop_termination: completed_session_loop_termination(),
        };

        init_test_tracing();

        let request_parent = W3cTraceContext {
            traceparent: Some("00-00000000000000000000000000000011-0000000000000022-01".into()),
            tracestate: Some("vendor=value".into()),
        };
        let request_span = info_span!("app_server.request");
        assert!(set_parent_from_w3c_trace_context(
            &request_span,
            &request_parent
        ));

        let expected_trace = async {
            let expected_trace =
                current_span_w3c_trace_context().expect("current span should have trace context");
            codex
                .submit_with_id(Submission {
                    id: "sub-1".into(),
                    op: Op::Interrupt,
                    trace: None,
                })
                .await
                .expect("submit should succeed");
            expected_trace
        }
        .instrument(request_span)
        .await;

        let submitted = rx_sub.recv().await.expect("submission");
        assert_eq!(submitted.trace, Some(expected_trace));
    }

    #[tokio::test]
    async fn new_default_turn_captures_current_span_trace_id() {
        let (session, _turn_context) = make_session_and_context().await;

        init_test_tracing();

        let request_parent = W3cTraceContext {
            traceparent: Some("00-00000000000000000000000000000011-0000000000000022-01".into()),
            tracestate: Some("vendor=value".into()),
        };
        let request_span = info_span!("app_server.request");
        assert!(set_parent_from_w3c_trace_context(
            &request_span,
            &request_parent
        ));

        let turn_context_item = async {
            let expected_trace_id = Span::current()
                .context()
                .span()
                .span_context()
                .trace_id()
                .to_string();
            let turn_context = session.new_default_turn().await;
            let turn_context_item = turn_context.to_turn_context_item();
            assert_eq!(turn_context_item.trace_id, Some(expected_trace_id));
            turn_context_item
        }
        .instrument(request_span)
        .await;

        assert_eq!(
            turn_context_item.trace_id.as_deref(),
            Some("00000000000000000000000000000011")
        );
    }

    #[test]
    fn submission_dispatch_span_prefers_submission_trace_context() {
        init_test_tracing();

        let ambient_parent = W3cTraceContext {
            traceparent: Some("00-00000000000000000000000000000033-0000000000000044-01".into()),
            tracestate: None,
        };
        let ambient_span = info_span!("ambient");
        assert!(set_parent_from_w3c_trace_context(
            &ambient_span,
            &ambient_parent
        ));

        let submission_trace = W3cTraceContext {
            traceparent: Some("00-00000000000000000000000000000055-0000000000000066-01".into()),
            tracestate: Some("vendor=value".into()),
        };
        let dispatch_span = ambient_span.in_scope(|| {
            submission_dispatch_span(&Submission {
                id: "sub-1".into(),
                op: Op::Interrupt,
                trace: Some(submission_trace),
            })
        });

        let trace_id = dispatch_span.context().span().span_context().trace_id();
        assert_eq!(
            trace_id,
            TraceId::from_hex("00000000000000000000000000000055").expect("trace id")
        );
    }

    #[test]
    fn submission_dispatch_span_uses_debug_for_realtime_audio() {
        init_test_tracing();

        let dispatch_span = submission_dispatch_span(&Submission {
            id: "sub-1".into(),
            op: Op::RealtimeConversationAudio(ConversationAudioParams {
                frame: RealtimeAudioFrame {
                    data: "ZmFrZQ==".into(),
                    sample_rate: 16_000,
                    num_channels: 1,
                    samples_per_channel: Some(160),
                    item_id: None,
                },
            }),
            trace: None,
        });

        assert_eq!(
            dispatch_span.metadata().expect("span metadata").level(),
            &tracing::Level::DEBUG
        );
    }

    #[tokio::test]
    async fn spawn_task_turn_span_inherits_dispatch_trace_context() {
        struct TraceCaptureTask {
            captured_trace: Arc<std::sync::Mutex<Option<W3cTraceContext>>>,
        }

        #[async_trait::async_trait]
        impl SessionTask for TraceCaptureTask {
            fn kind(&self) -> TaskKind {
                TaskKind::Regular
            }

            fn span_name(&self) -> &'static str {
                "session_task.trace_capture"
            }

            async fn run(
                self: Arc<Self>,
                _session: Arc<SessionTaskContext>,
                _ctx: Arc<TurnContext>,
                _input: Vec<UserInput>,
                _cancellation_token: CancellationToken,
            ) -> Option<String> {
                let mut trace = self
                    .captured_trace
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *trace = current_span_w3c_trace_context();
                None
            }
        }

        init_test_tracing();

        let request_parent = W3cTraceContext {
            traceparent: Some("00-00000000000000000000000000000011-0000000000000022-01".into()),
            tracestate: Some("vendor=value".into()),
        };
        let request_span = tracing::info_span!("app_server.request");
        assert!(set_parent_from_w3c_trace_context(
            &request_span,
            &request_parent
        ));

        let submission_trace = async {
            current_span_w3c_trace_context().expect("request span should have trace context")
        }
        .instrument(request_span)
        .await;

        let dispatch_span = submission_dispatch_span(&Submission {
            id: "sub-1".into(),
            op: Op::Interrupt,
            trace: Some(submission_trace.clone()),
        });
        let dispatch_span_id = dispatch_span.context().span().span_context().span_id();

        let (sess, tc, rx) = make_session_and_context_with_rx().await;
        let captured_trace = Arc::new(std::sync::Mutex::new(None));

        async {
            sess.spawn_task(
                Arc::clone(&tc),
                vec![UserInput::Text {
                    text: "hello".to_string(),
                    text_elements: Vec::new(),
                }],
                TraceCaptureTask {
                    captured_trace: Arc::clone(&captured_trace),
                },
            )
            .await;
        }
        .instrument(dispatch_span)
        .await;

        let evt = tokio::time::timeout(StdDuration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for turn completion")
            .expect("event");
        assert!(matches!(evt.msg, EventMsg::TurnComplete(_)));

        let task_trace = captured_trace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("turn task should capture the current span trace context");
        let submission_context =
            codex_otel::context_from_w3c_trace_context(&submission_trace).expect("submission");
        let task_context =
            codex_otel::context_from_w3c_trace_context(&task_trace).expect("task trace");

        assert_eq!(
            task_context.span().span_context().trace_id(),
            submission_context.span().span_context().trace_id()
        );
        assert_ne!(
            task_context.span().span_context().span_id(),
            dispatch_span_id
        );
    }

    pub(crate) async fn make_session_and_context_with_dynamic_tools_and_rx(
        dynamic_tools: Vec<DynamicToolSpec>,
    ) -> (
        Arc<Session>,
        Arc<TurnContext>,
        async_channel::Receiver<Event>,
    ) {
        let (tx_event, rx_event) = async_channel::unbounded();
        let (tx_sub, _rx_sub) = async_channel::bounded::<Submission>(SUBMISSION_CHANNEL_CAPACITY);
        let codex_home = tempfile::tempdir().expect("create temp dir");
        let config = build_test_config(codex_home.path()).await;
        let config = Arc::new(config);
        let conversation_id = ThreadId::default();
        let auth_manager =
            AuthManager::from_auth_for_testing(CodexAuth::from_api_key("Test API Key"));
        let models_manager = Arc::new(ModelsManager::new(
            config.codex_home.clone(),
            auth_manager.clone(),
            None,
            CollaborationModesConfig::default(),
        ));
        let agent_control = AgentControl::default();
        let exec_policy = Arc::new(ExecPolicyManager::default());
        let (agent_status_tx, _agent_status_rx) = watch::channel(AgentStatus::PendingInit);
        let model = ModelsManager::get_model_offline_for_tests(config.model.as_deref());
        let model_info =
            ModelsManager::construct_model_info_offline_for_tests(model.as_str(), &config);
        let reasoning_effort = config.model_reasoning_effort;
        let collaboration_mode = CollaborationMode {
            mode: ModeKind::Default,
            settings: Settings {
                model,
                reasoning_effort,
                developer_instructions: None,
            },
        };
        let session_configuration = SessionConfiguration {
            provider: config.model_provider.clone(),
            collaboration_mode,
            model_reasoning_summary: config.model_reasoning_summary,
            user_instructions: config.user_instructions.clone(),
            service_tier: None,
            personality: config.personality,
            base_instructions: config
                .base_instructions
                .clone()
                .unwrap_or_else(|| model_info.get_model_instructions(config.personality)),
            compact_prompt: config.compact_prompt.clone(),
            approval_policy: config.permissions.approval_policy.clone(),
            approvals_reviewer: config.approvals_reviewer,
            sandbox_policy: config.permissions.sandbox_policy.clone(),
            file_system_sandbox_policy: config.permissions.file_system_sandbox_policy.clone(),
            network_sandbox_policy: config.permissions.network_sandbox_policy,
            windows_sandbox_level: WindowsSandboxLevel::from_config(&config),
            cwd: config.cwd.clone(),
            codex_home: config.codex_home.clone(),
            thread_name: None,
            original_config_do_not_use: Arc::clone(&config),
            metrics_service_name: None,
            app_server_client_name: None,
            session_source: SessionSource::Exec,
            nero_auto_runtime: NeroAutoRuntimeConfig::default(),
            nero_model_fallback: None,
            dynamic_tools,
            persist_extended_history: false,
            inherited_shell_snapshot: None,
            user_shell_override: None,
        };
        let per_turn_config = Session::build_per_turn_config(&session_configuration);
        let model_info = ModelsManager::construct_model_info_offline_for_tests(
            session_configuration.collaboration_mode.model(),
            &per_turn_config,
        );
        let session_telemetry = session_telemetry(
            conversation_id,
            config.as_ref(),
            &model_info,
            session_configuration.session_source.clone(),
        );

        let state = SessionState::new(session_configuration.clone());
        let plugins_manager = Arc::new(PluginsManager::new(config.codex_home.clone()));
        let mcp_manager = Arc::new(McpManager::new(Arc::clone(&plugins_manager)));
        let skills_manager = Arc::new(SkillsManager::new(
            config.codex_home.clone(),
            config.bundled_skills_enabled(),
        ));
        let network_approval = Arc::new(NetworkApprovalService::default());
        let environment = Arc::new(
            codex_exec_server::Environment::create(/*exec_server_url*/ None)
                .await
                .expect("create environment"),
        );

        let user_shell = Arc::new(default_user_shell());
        let mut hook_shell_argv = user_shell.derive_exec_args("", false);
        let hook_shell_program = hook_shell_argv.remove(0);
        let _ = hook_shell_argv.pop();
        let skills_watcher = Arc::new(SkillsWatcher::noop());
        let services = SessionServices {
            mcp_connection_manager: Arc::new(RwLock::new(
                McpConnectionManager::new_mcp_connection_manager_for_tests(
                    &config.permissions.approval_policy,
                ),
            )),
            mcp_startup_cancellation_token: Mutex::new(CancellationToken::new()),
            unified_exec_manager: UnifiedExecProcessManager::new(
                config.background_terminal_max_timeout,
            ),
            shell_zsh_path: None,
            main_execve_wrapper_exe: config.main_execve_wrapper_exe.clone(),
            analytics_events_client: AnalyticsEventsClient::new(
                Arc::clone(&auth_manager),
                config.chatgpt_base_url.trim_end_matches('/').to_string(),
                config.analytics_enabled,
            ),
            hooks: Hooks::new(HooksConfig {
                legacy_notify_argv: config.notify.clone(),
                feature_enabled: config.features.enabled(Feature::CodexHooks),
                config_layer_stack: Some(config.config_layer_stack.clone()),
                shell_program: Some(hook_shell_program),
                shell_args: hook_shell_argv,
            }),
            rollout: Mutex::new(None),
            user_shell: Arc::clone(&user_shell),
            shell_snapshot_tx: watch::channel(None).0,
            show_raw_agent_reasoning: config.show_raw_agent_reasoning,
            exec_policy,
            auth_manager: Arc::clone(&auth_manager),
            session_telemetry: session_telemetry.clone(),
            models_manager: Arc::clone(&models_manager),
            tool_approvals: Mutex::new(ApprovalStore::default()),
            skills_manager,
            plugins_manager,
            mcp_manager,
            skills_watcher,
            agent_control,
            network_proxy: None,
            network_approval: Arc::clone(&network_approval),
            state_db: None,
            model_client: ModelClient::new(
                Some(Arc::clone(&auth_manager)),
                conversation_id,
                session_configuration.provider.clone(),
                session_configuration.session_source.clone(),
                config.model_verbosity,
                config.features.enabled(Feature::EnableRequestCompression),
                config.features.enabled(Feature::RuntimeMetrics),
                Session::build_model_client_beta_features_header(config.as_ref()),
            ),
            code_mode_service: crate::tools::code_mode::CodeModeService::new(
                config.js_repl_node_path.clone(),
            ),
            environment: Arc::clone(&environment),
        };
        let js_repl = Arc::new(JsReplHandle::with_node_path(
            config.js_repl_node_path.clone(),
            config.js_repl_node_module_dirs.clone(),
        ));
        let (out_of_band_elicitation_paused, _out_of_band_elicitation_paused_rx) =
            watch::channel(false);
        let (mailbox, mailbox_rx) = Mailbox::new();

        let plugin_outcome = services
            .plugins_manager
            .plugins_for_config(&per_turn_config);
        let effective_skill_roots = plugin_outcome.effective_skill_roots();
        let skills_input =
            crate::skills_load_input_from_config(&per_turn_config, effective_skill_roots);
        let skills_outcome = Arc::new(services.skills_manager.skills_for_config(&skills_input));
        let turn_context = Arc::new(Session::make_turn_context(
            conversation_id,
            Some(Arc::clone(&auth_manager)),
            &session_telemetry,
            session_configuration.provider.clone(),
            &session_configuration,
            services.user_shell.as_ref(),
            services.shell_zsh_path.as_ref(),
            services.main_execve_wrapper_exe.as_ref(),
            per_turn_config,
            model_info,
            &models_manager,
            None,
            environment,
            "turn_id".to_string(),
            Arc::clone(&js_repl),
            skills_outcome,
        ));

        let session = Arc::new(Session {
            conversation_id,
            tx_sub,
            tx_event,
            agent_status: agent_status_tx,
            state: Mutex::new(state),
            features: config.features.clone(),
            pending_mcp_server_refresh_config: Mutex::new(None),
            conversation: Arc::new(RealtimeConversationManager::new()),
            active_turn: Mutex::new(None),
            mailbox,
            mailbox_rx: Mutex::new(mailbox_rx),
            idle_pending_input: Mutex::new(Vec::new()),
            guardian_review_session: GuardianReviewSessionManager::default(),
            services,
            js_repl,
            hook_seen_terminal_turn_ids: Mutex::new(HashSet::new()),
            hook_auto_reply_internal_submission_ids: StdMutex::new(HashSet::new()),
            hook_nero_msg_throttle: StdMutex::new(HashMap::new()),
            nero_auto_bridge_warning_emitted: StdMutex::new(false),
            hook_auto_reply_guard_state: StdMutex::new(HookAutoReplyGuardState::default()),
            out_of_band_elicitation_paused,
            next_internal_sub_id: AtomicU64::new(0),
        });

        (session, turn_context, rx_event)
    }

    // Like make_session_and_context, but returns Arc<Session> and the event receiver
    // so tests can assert on emitted events.
    pub(crate) async fn make_session_and_context_with_rx() -> (
        Arc<Session>,
        Arc<TurnContext>,
        async_channel::Receiver<Event>,
    ) {
        make_session_and_context_with_dynamic_tools_and_rx(Vec::new()).await
    }

    #[tokio::test]
    async fn refresh_mcp_servers_is_deferred_until_next_turn() {
        let (session, turn_context) = make_session_and_context().await;
        let old_token = session.mcp_startup_cancellation_token().await;
        assert!(!old_token.is_cancelled());

        let mcp_oauth_credentials_store_mode =
            serde_json::to_value(OAuthCredentialsStoreMode::Auto).expect("serialize store mode");
        let refresh_config = McpServerRefreshConfig {
            mcp_servers: json!({}),
            mcp_oauth_credentials_store_mode,
        };
        {
            let mut guard = session.pending_mcp_server_refresh_config.lock().await;
            *guard = Some(refresh_config);
        }

        assert!(!old_token.is_cancelled());
        assert!(
            session
                .pending_mcp_server_refresh_config
                .lock()
                .await
                .is_some()
        );

        session
            .refresh_mcp_servers_if_requested(&turn_context)
            .await;

        assert!(old_token.is_cancelled());
        assert!(
            session
                .pending_mcp_server_refresh_config
                .lock()
                .await
                .is_none()
        );
        let new_token = session.mcp_startup_cancellation_token().await;
        assert!(!new_token.is_cancelled());
    }

    #[tokio::test]
    async fn record_model_warning_appends_user_message() {
        let (mut session, turn_context) = make_session_and_context().await;
        let features = Features::with_defaults().into();
        session.features = features;

        session
            .record_model_warning("too many unified exec processes", &turn_context)
            .await;

        let history = session.clone_history().await;
        let history_items = history.raw_items();
        let last = history_items.last().expect("warning recorded");

        match last {
            ResponseItem::Message { role, content, .. } => {
                assert_eq!(role, "user");
                assert_eq!(
                    content,
                    &vec![ContentItem::InputText {
                        text: "Warning: too many unified exec processes".to_string(),
                    }]
                );
            }
            other => panic!("expected user message, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn spawn_task_does_not_update_previous_turn_settings_for_non_run_turn_tasks() {
        let (sess, tc, _rx) = make_session_and_context_with_rx().await;
        sess.set_previous_turn_settings(None).await;
        let input = vec![UserInput::Text {
            text: "hello".to_string(),
            text_elements: Vec::new(),
        }];

        sess.spawn_task(
            Arc::clone(&tc),
            input,
            NeverEndingTask {
                kind: TaskKind::Regular,
                listen_to_cancellation_token: true,
            },
        )
        .await;

        sess.abort_all_tasks(TurnAbortReason::Interrupted).await;
        assert_eq!(sess.previous_turn_settings().await, None);
    }

    #[tokio::test]
    async fn build_settings_update_items_emits_environment_item_for_network_changes() {
        let (session, previous_context) = make_session_and_context().await;
        let previous_context = Arc::new(previous_context);
        let mut current_context = previous_context
            .with_model(
                previous_context.model_info.slug.clone(),
                &session.services.models_manager,
            )
            .await;

        let mut config = (*current_context.config).clone();
        let mut requirements = config.config_layer_stack.requirements().clone();
        requirements.network = Some(Sourced::new(
            NetworkConstraints {
                domains: Some(codex_config::NetworkDomainPermissionsToml {
                    entries: BTreeMap::from([
                        (
                            "api.example.com".to_string(),
                            codex_config::NetworkDomainPermissionToml::Allow,
                        ),
                        (
                            "blocked.example.com".to_string(),
                            codex_config::NetworkDomainPermissionToml::Deny,
                        ),
                    ]),
                }),
                ..Default::default()
            },
            RequirementSource::CloudRequirements,
        ));
        let layers = config
            .config_layer_stack
            .get_layers(ConfigLayerStackOrdering::LowestPrecedenceFirst, true)
            .into_iter()
            .cloned()
            .collect();
        config.config_layer_stack = ConfigLayerStack::new(
            layers,
            requirements,
            config.config_layer_stack.requirements_toml().clone(),
        )
        .expect("rebuild config layer stack with network requirements");
        current_context.config = Arc::new(config);

        let reference_context_item = previous_context.to_turn_context_item();
        let update_items = session
            .build_settings_update_items(Some(&reference_context_item), &current_context)
            .await;

        let environment_update = update_items
            .iter()
            .find_map(|item| match item {
                ResponseItem::Message { role, content, .. } if role == "user" => {
                    let [ContentItem::InputText { text }] = content.as_slice() else {
                        return None;
                    };
                    text.contains("<environment_context>").then_some(text)
                }
                _ => None,
            })
            .expect("environment update item should be emitted");
        assert!(environment_update.contains("<network enabled=\"true\">"));
        assert!(environment_update.contains("<allowed>api.example.com</allowed>"));
        assert!(environment_update.contains("<denied>blocked.example.com</denied>"));
    }

    #[tokio::test]
    async fn build_settings_update_items_emits_environment_item_for_time_changes() {
        let (session, previous_context) = make_session_and_context().await;
        let previous_context = Arc::new(previous_context);
        let mut current_context = previous_context
            .with_model(
                previous_context.model_info.slug.clone(),
                &session.services.models_manager,
            )
            .await;
        current_context.current_date = Some("2026-02-27".to_string());
        current_context.timezone = Some("Europe/Berlin".to_string());

        let reference_context_item = previous_context.to_turn_context_item();
        let update_items = session
            .build_settings_update_items(Some(&reference_context_item), &current_context)
            .await;

        let environment_update = update_items
            .iter()
            .find_map(|item| match item {
                ResponseItem::Message { role, content, .. } if role == "user" => {
                    let [ContentItem::InputText { text }] = content.as_slice() else {
                        return None;
                    };
                    text.contains("<environment_context>").then_some(text)
                }
                _ => None,
            })
            .expect("environment update item should be emitted");
        assert!(environment_update.contains("<current_date>2026-02-27</current_date>"));
        assert!(environment_update.contains("<timezone>Europe/Berlin</timezone>"));
    }

    #[tokio::test]
    async fn build_settings_update_items_emits_realtime_start_when_session_becomes_live() {
        let (session, previous_context) = make_session_and_context().await;
        let previous_context = Arc::new(previous_context);
        let mut current_context = previous_context
            .with_model(
                previous_context.model_info.slug.clone(),
                &session.services.models_manager,
            )
            .await;
        current_context.realtime_active = true;

        let update_items = session
            .build_settings_update_items(
                Some(&previous_context.to_turn_context_item()),
                &current_context,
            )
            .await;

        let developer_texts = developer_input_texts(&update_items);
        assert!(
            developer_texts
                .iter()
                .any(|text| text.contains("<realtime_conversation>")),
            "expected a realtime start update, got {developer_texts:?}"
        );
    }

    #[tokio::test]
    async fn build_settings_update_items_emits_realtime_end_when_session_stops_being_live() {
        let (session, mut previous_context) = make_session_and_context().await;
        previous_context.realtime_active = true;
        let mut current_context = previous_context
            .with_model(
                previous_context.model_info.slug.clone(),
                &session.services.models_manager,
            )
            .await;
        current_context.realtime_active = false;

        let update_items = session
            .build_settings_update_items(
                Some(&previous_context.to_turn_context_item()),
                &current_context,
            )
            .await;

        let developer_texts = developer_input_texts(&update_items);
        assert!(
            developer_texts
                .iter()
                .any(|text| text.contains("Reason: inactive")),
            "expected a realtime end update, got {developer_texts:?}"
        );
    }

    #[tokio::test]
    async fn build_settings_update_items_uses_previous_turn_settings_for_realtime_end() {
        let (session, previous_context) = make_session_and_context().await;
        let mut previous_context_item = previous_context.to_turn_context_item();
        previous_context_item.realtime_active = None;
        let previous_turn_settings = PreviousTurnSettings {
            model: previous_context.model_info.slug.clone(),
            realtime_active: Some(true),
        };
        let mut current_context = previous_context
            .with_model(
                previous_context.model_info.slug.clone(),
                &session.services.models_manager,
            )
            .await;
        current_context.realtime_active = false;

        session
            .set_previous_turn_settings(Some(previous_turn_settings))
            .await;
        let update_items = session
            .build_settings_update_items(Some(&previous_context_item), &current_context)
            .await;

        let developer_texts = developer_input_texts(&update_items);
        assert!(
            developer_texts
                .iter()
                .any(|text| text.contains("Reason: inactive")),
            "expected a realtime end update from previous turn settings, got {developer_texts:?}"
        );
    }

    #[tokio::test]
    async fn build_settings_update_items_emits_runtime_developer_instruction_update_when_changed() {
        let (session, mut previous_context) = make_session_and_context().await;
        previous_context.developer_instructions = Some("base instructions".to_string());
        let mut current_context = previous_context
            .with_model(
                previous_context.model_info.slug.clone(),
                &session.services.models_manager,
            )
            .await;
        current_context.developer_instructions = Some(
            "base instructions\n\n## NERO-SYSTEM v1\nEmit the strict JSON block below.".to_string(),
        );

        let update_items = session
            .build_settings_update_items(
                Some(&previous_context.to_turn_context_item()),
                &current_context,
            )
            .await;

        let developer_texts = developer_input_texts(&update_items);
        assert!(
            developer_texts.iter().any(|text| {
                text.contains("Runtime developer instructions update for this session.")
                    && text.contains("## NERO-SYSTEM v1")
                    && text.contains("Emit the strict JSON block below.")
            }),
            "expected a runtime developer instructions update, got {developer_texts:?}"
        );
    }

    #[tokio::test]
    async fn build_settings_update_items_emits_runtime_developer_instruction_clear_note_when_removed()
     {
        let (session, mut previous_context) = make_session_and_context().await;
        previous_context.developer_instructions = Some(
            "base instructions\n\n## NERO-SYSTEM v1\nEmit the strict JSON block below.".to_string(),
        );
        let mut current_context = previous_context
            .with_model(
                previous_context.model_info.slug.clone(),
                &session.services.models_manager,
            )
            .await;
        current_context.developer_instructions = None;

        let update_items = session
            .build_settings_update_items(
                Some(&previous_context.to_turn_context_item()),
                &current_context,
            )
            .await;

        let developer_texts = developer_input_texts(&update_items);
        assert!(
            developer_texts.iter().any(|text| {
                text.contains("Runtime developer instructions update for this session.")
                    && text
                        .contains("No session-local runtime developer instructions are active now")
            }),
            "expected a runtime developer instructions clear note, got {developer_texts:?}"
        );
    }

    #[tokio::test]
    async fn build_initial_context_uses_previous_realtime_state() {
        let (session, mut turn_context) = make_session_and_context().await;
        turn_context.realtime_active = true;

        let initial_context = session.build_initial_context(&turn_context).await;
        let developer_texts = developer_input_texts(&initial_context);
        assert!(
            developer_texts
                .iter()
                .any(|text| text.contains("<realtime_conversation>")),
            "expected initial context to describe active realtime state, got {developer_texts:?}"
        );

        let previous_context_item = turn_context.to_turn_context_item();
        {
            let mut state = session.state.lock().await;
            state.set_reference_context_item(Some(previous_context_item));
        }
        let resumed_context = session.build_initial_context(&turn_context).await;
        let resumed_developer_texts = developer_input_texts(&resumed_context);
        assert!(
            !resumed_developer_texts
                .iter()
                .any(|text| text.contains("<realtime_conversation>")),
            "did not expect a duplicate realtime update, got {resumed_developer_texts:?}"
        );
    }

    #[tokio::test]
    async fn build_initial_context_uses_previous_turn_settings_for_realtime_end() {
        let (session, turn_context) = make_session_and_context().await;
        let previous_turn_settings = PreviousTurnSettings {
            model: turn_context.model_info.slug.clone(),
            realtime_active: Some(true),
        };

        session
            .set_previous_turn_settings(Some(previous_turn_settings))
            .await;
        let initial_context = session.build_initial_context(&turn_context).await;
        let developer_texts = developer_input_texts(&initial_context);
        assert!(
            developer_texts
                .iter()
                .any(|text| text.contains("Reason: inactive")),
            "expected initial context to describe an ended realtime session, got {developer_texts:?}"
        );
    }

    #[tokio::test]
    async fn build_initial_context_restates_realtime_start_when_reference_context_is_missing() {
        let (session, mut turn_context) = make_session_and_context().await;
        turn_context.realtime_active = true;
        let previous_turn_settings = PreviousTurnSettings {
            model: turn_context.model_info.slug.clone(),
            realtime_active: Some(true),
        };

        session
            .set_previous_turn_settings(Some(previous_turn_settings))
            .await;
        let initial_context = session.build_initial_context(&turn_context).await;
        let developer_texts = developer_input_texts(&initial_context);
        assert!(
            developer_texts
                .iter()
                .any(|text| text.contains("<realtime_conversation>")),
            "expected initial context to restate active realtime when the reference context is missing, got {developer_texts:?}"
        );
    }

    #[tokio::test]
    async fn record_context_updates_and_set_reference_context_item_injects_full_context_when_baseline_missing()
     {
        let (session, turn_context) = make_session_and_context().await;
        session
            .record_context_updates_and_set_reference_context_item(&turn_context)
            .await;
        let history = session.clone_history().await;
        let initial_context = session.build_initial_context(&turn_context).await;
        assert_eq!(history.raw_items().to_vec(), initial_context);

        let current_context = session.reference_context_item().await;
        assert_eq!(
            serde_json::to_value(current_context).expect("serialize current context item"),
            serde_json::to_value(Some(turn_context.to_turn_context_item()))
                .expect("serialize expected context item")
        );
    }

    #[tokio::test]
    async fn record_context_updates_and_set_reference_context_item_reinjects_full_context_after_clear()
     {
        let (session, turn_context) = make_session_and_context().await;
        let compacted_summary = ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: format!("{}\nsummary", crate::compact::SUMMARY_PREFIX),
            }],
            end_turn: None,
            phase: None,
        };
        session
            .record_into_history(std::slice::from_ref(&compacted_summary), &turn_context)
            .await;
        session
            .record_context_updates_and_set_reference_context_item(&turn_context)
            .await;
        {
            let mut state = session.state.lock().await;
            state.set_reference_context_item(None);
        }
        session
            .replace_history(vec![compacted_summary.clone()], None)
            .await;

        session
            .record_context_updates_and_set_reference_context_item(&turn_context)
            .await;

        let history = session.clone_history().await;
        let mut expected_history = vec![compacted_summary];
        expected_history.extend(session.build_initial_context(&turn_context).await);
        assert_eq!(history.raw_items().to_vec(), expected_history);
    }

    #[tokio::test]
    async fn record_context_updates_and_set_reference_context_item_persists_baseline_without_emitting_diffs()
     {
        let (session, previous_context) = make_session_and_context().await;
        let next_model = if previous_context.model_info.slug == "gpt-5.1" {
            "gpt-5"
        } else {
            "gpt-5.1"
        };
        let turn_context = previous_context
            .with_model(next_model.to_string(), &session.services.models_manager)
            .await;
        let previous_context_item = previous_context.to_turn_context_item();
        {
            let mut state = session.state.lock().await;
            state.set_reference_context_item(Some(previous_context_item.clone()));
        }
        let config = session.get_config().await;
        let recorder = RolloutRecorder::new(
            config.as_ref(),
            RolloutRecorderParams::new(
                ThreadId::default(),
                None,
                SessionSource::Exec,
                BaseInstructions::default(),
                Vec::new(),
                EventPersistenceMode::Limited,
            ),
            None,
            None,
        )
        .await
        .expect("create rollout recorder");
        let rollout_path = recorder.rollout_path().to_path_buf();
        {
            let mut rollout = session.services.rollout.lock().await;
            *rollout = Some(recorder);
        }

        let update_items = session
            .build_settings_update_items(Some(&previous_context_item), &turn_context)
            .await;
        assert_eq!(update_items, Vec::new());

        session
            .record_context_updates_and_set_reference_context_item(&turn_context)
            .await;

        assert_eq!(
            session.clone_history().await.raw_items().to_vec(),
            Vec::new()
        );
        assert_eq!(
            serde_json::to_value(session.reference_context_item().await)
                .expect("serialize current context item"),
            serde_json::to_value(Some(turn_context.to_turn_context_item()))
                .expect("serialize expected context item")
        );
        session.ensure_rollout_materialized().await;
        session.flush_rollout().await;

        let InitialHistory::Resumed(resumed) = RolloutRecorder::get_rollout_history(&rollout_path)
            .await
            .expect("read rollout history")
        else {
            panic!("expected resumed rollout history");
        };
        let persisted_turn_context = resumed.history.iter().find_map(|item| match item {
            RolloutItem::TurnContext(ctx) => Some(ctx.clone()),
            _ => None,
        });
        assert_eq!(
            serde_json::to_value(persisted_turn_context)
                .expect("serialize persisted turn context item"),
            serde_json::to_value(Some(turn_context.to_turn_context_item()))
                .expect("serialize expected turn context item")
        );
    }

    #[tokio::test]
    async fn build_initial_context_prepends_model_switch_message() {
        let (session, turn_context) = make_session_and_context().await;
        let previous_turn_settings = PreviousTurnSettings {
            model: "previous-regular-model".to_string(),
            realtime_active: None,
        };

        session
            .set_previous_turn_settings(Some(previous_turn_settings))
            .await;
        let initial_context = session.build_initial_context(&turn_context).await;

        let ResponseItem::Message { role, content, .. } = &initial_context[0] else {
            panic!("expected developer message");
        };
        assert_eq!(role, "developer");
        let [ContentItem::InputText { text }, ..] = content.as_slice() else {
            panic!("expected developer text");
        };
        assert!(text.contains("<model_switch>"));
    }

    #[tokio::test]
    async fn record_context_updates_and_set_reference_context_item_persists_full_reinjection_to_rollout()
     {
        let (session, previous_context) = make_session_and_context().await;
        let next_model = if previous_context.model_info.slug == "gpt-5.1" {
            "gpt-5"
        } else {
            "gpt-5.1"
        };
        let turn_context = previous_context
            .with_model(next_model.to_string(), &session.services.models_manager)
            .await;
        let config = session.get_config().await;
        let recorder = RolloutRecorder::new(
            config.as_ref(),
            RolloutRecorderParams::new(
                ThreadId::default(),
                None,
                SessionSource::Exec,
                BaseInstructions::default(),
                Vec::new(),
                EventPersistenceMode::Limited,
            ),
            None,
            None,
        )
        .await
        .expect("create rollout recorder");
        let rollout_path = recorder.rollout_path().to_path_buf();
        {
            let mut rollout = session.services.rollout.lock().await;
            *rollout = Some(recorder);
        }

        session
            .persist_rollout_items(&[RolloutItem::EventMsg(EventMsg::UserMessage(
                UserMessageEvent {
                    message: "seed rollout".to_string(),
                    images: None,
                    local_images: Vec::new(),
                    text_elements: Vec::new(),
                },
            ))])
            .await;
        {
            let mut state = session.state.lock().await;
            state.set_reference_context_item(None);
        }

        session
            .set_previous_turn_settings(Some(PreviousTurnSettings {
                model: previous_context.model_info.slug.clone(),
                realtime_active: Some(previous_context.realtime_active),
            }))
            .await;
        session
            .record_context_updates_and_set_reference_context_item(&turn_context)
            .await;
        session.ensure_rollout_materialized().await;
        session.flush_rollout().await;

        let InitialHistory::Resumed(resumed) = RolloutRecorder::get_rollout_history(&rollout_path)
            .await
            .expect("read rollout history")
        else {
            panic!("expected resumed rollout history");
        };
        let persisted_turn_context = resumed.history.iter().find_map(|item| match item {
            RolloutItem::TurnContext(ctx) => Some(ctx.clone()),
            _ => None,
        });

        assert_eq!(
            serde_json::to_value(persisted_turn_context)
                .expect("serialize persisted turn context item"),
            serde_json::to_value(Some(turn_context.to_turn_context_item()))
                .expect("serialize expected turn context item")
        );
    }

    #[tokio::test]
    async fn run_user_shell_command_does_not_set_reference_context_item() {
        let (session, _turn_context, rx) = make_session_and_context_with_rx().await;
        {
            let mut state = session.state.lock().await;
            state.set_reference_context_item(None);
        }

        handlers::run_user_shell_command(&session, "sub-id".to_string(), "echo shell".to_string())
            .await;

        let deadline = StdDuration::from_secs(15);
        let start = std::time::Instant::now();
        loop {
            let remaining = deadline.saturating_sub(start.elapsed());
            let evt = tokio::time::timeout(remaining, rx.recv())
                .await
                .expect("timeout waiting for event")
                .expect("event");
            if matches!(evt.msg, EventMsg::TurnComplete(_)) {
                break;
            }
        }

        assert!(
            session.reference_context_item().await.is_none(),
            "standalone shell tasks should not mutate previous context"
        );
    }

    #[derive(Clone, Copy)]
    struct NeverEndingTask {
        kind: TaskKind,
        listen_to_cancellation_token: bool,
    }

    #[async_trait::async_trait]
    impl SessionTask for NeverEndingTask {
        fn kind(&self) -> TaskKind {
            self.kind
        }

        fn span_name(&self) -> &'static str {
            "session_task.never_ending"
        }

        async fn run(
            self: Arc<Self>,
            _session: Arc<SessionTaskContext>,
            _ctx: Arc<TurnContext>,
            _input: Vec<UserInput>,
            cancellation_token: CancellationToken,
        ) -> Option<String> {
            if self.listen_to_cancellation_token {
                cancellation_token.cancelled().await;
                return None;
            }
            loop {
                sleep(Duration::from_secs(60)).await;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[test_log::test]
    async fn abort_regular_task_emits_turn_aborted_only() {
        let (sess, tc, rx) = make_session_and_context_with_rx().await;
        let input = vec![UserInput::Text {
            text: "hello".to_string(),
            text_elements: Vec::new(),
        }];
        sess.spawn_task(
            Arc::clone(&tc),
            input,
            NeverEndingTask {
                kind: TaskKind::Regular,
                listen_to_cancellation_token: false,
            },
        )
        .await;

        sess.abort_all_tasks(TurnAbortReason::Interrupted).await;

        // Interrupts persist a model-visible `<turn_aborted>` marker into history, but there is no
        // separate client-visible event for that marker (only `EventMsg::TurnAborted`).
        let evt = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for event")
            .expect("event");
        match evt.msg {
            EventMsg::TurnAborted(e) => assert_eq!(TurnAbortReason::Interrupted, e.reason),
            other => panic!("unexpected event: {other:?}"),
        }
        // No extra events should be emitted after an abort.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn abort_gracefully_emits_turn_aborted_only() {
        let (sess, tc, rx) = make_session_and_context_with_rx().await;
        let input = vec![UserInput::Text {
            text: "hello".to_string(),
            text_elements: Vec::new(),
        }];
        sess.spawn_task(
            Arc::clone(&tc),
            input,
            NeverEndingTask {
                kind: TaskKind::Regular,
                listen_to_cancellation_token: true,
            },
        )
        .await;

        sess.abort_all_tasks(TurnAbortReason::Interrupted).await;

        // Even if tasks handle cancellation gracefully, interrupts still result in `TurnAborted`
        // being the only client-visible signal.
        let evt = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for event")
            .expect("event");
        match evt.msg {
            EventMsg::TurnAborted(e) => assert_eq!(TurnAbortReason::Interrupted, e.reason),
            other => panic!("unexpected event: {other:?}"),
        }
        // No extra events should be emitted after an abort.
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn task_finish_emits_turn_item_lifecycle_for_leftover_pending_user_input() {
        let (sess, tc, rx) = make_session_and_context_with_rx().await;
        let input = vec![UserInput::Text {
            text: "hello".to_string(),
            text_elements: Vec::new(),
        }];
        sess.spawn_task(
            Arc::clone(&tc),
            input,
            NeverEndingTask {
                kind: TaskKind::Regular,
                listen_to_cancellation_token: false,
            },
        )
        .await;

        while rx.try_recv().is_ok() {}

        sess.inject_response_items(vec![ResponseInputItem::Message {
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "late pending input".to_string(),
            }],
        }])
        .await
        .expect("inject pending input into active turn");

        sess.on_task_finished(Arc::clone(&tc), None).await;

        let history = sess.clone_history().await;
        let expected = ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "late pending input".to_string(),
            }],
            end_turn: None,
            phase: None,
        };
        assert!(
            history.raw_items().iter().any(|item| item == &expected),
            "expected pending input to be persisted into history on turn completion"
        );

        let first = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("expected raw response item event")
            .expect("channel open");
        assert!(matches!(first.msg, EventMsg::RawResponseItem(_)));

        let second = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("expected item started event")
            .expect("channel open");
        assert!(matches!(
            second.msg,
            EventMsg::ItemStarted(ItemStartedEvent {
                item: TurnItem::UserMessage(UserMessageItem { content, .. }),
                ..
            }) if content == vec![UserInput::Text {
                text: "late pending input".to_string(),
                text_elements: Vec::new(),
            }]
        ));

        let third = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("expected item completed event")
            .expect("channel open");
        assert!(matches!(
            third.msg,
            EventMsg::ItemCompleted(ItemCompletedEvent {
                item: TurnItem::UserMessage(UserMessageItem { content, .. }),
                ..
            }) if content == vec![UserInput::Text {
                text: "late pending input".to_string(),
                text_elements: Vec::new(),
            }]
        ));

        let fourth = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("expected legacy user message event")
            .expect("channel open");
        assert!(matches!(
            fourth.msg,
            EventMsg::UserMessage(UserMessageEvent {
                message,
                images,
                text_elements,
                local_images,
            }) if message == "late pending input"
                && images == Some(Vec::new())
                && text_elements.is_empty()
                && local_images.is_empty()
        ));

        let fifth = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("expected turn complete event")
            .expect("channel open");
        assert!(matches!(
            fifth.msg,
            EventMsg::TurnComplete(TurnCompleteEvent {
                turn_id,
                last_agent_message: None,
            }) if turn_id == tc.sub_id
        ));
    }

    #[tokio::test]
    async fn steer_input_requires_active_turn() {
        let (sess, _tc, _rx) = make_session_and_context_with_rx().await;
        let input = vec![UserInput::Text {
            text: "steer".to_string(),
            text_elements: Vec::new(),
        }];

        let err = sess
            .steer_input(input, None)
            .await
            .expect_err("steering without active turn should fail");

        assert!(matches!(err, SteerInputError::NoActiveTurn(_)));
    }

    #[tokio::test]
    async fn steer_input_enforces_expected_turn_id() {
        let (sess, tc, _rx) = make_session_and_context_with_rx().await;
        let input = vec![UserInput::Text {
            text: "hello".to_string(),
            text_elements: Vec::new(),
        }];
        sess.spawn_task(
            Arc::clone(&tc),
            input,
            NeverEndingTask {
                kind: TaskKind::Regular,
                listen_to_cancellation_token: false,
            },
        )
        .await;

        let steer_input = vec![UserInput::Text {
            text: "steer".to_string(),
            text_elements: Vec::new(),
        }];
        let err = sess
            .steer_input(steer_input, Some("different-turn-id"))
            .await
            .expect_err("mismatched expected turn id should fail");

        match err {
            SteerInputError::ExpectedTurnMismatch { expected, actual } => {
                assert_eq!(
                    (expected, actual),
                    ("different-turn-id".to_string(), tc.sub_id.clone())
                );
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn steer_input_returns_active_turn_id() {
        let (sess, tc, _rx) = make_session_and_context_with_rx().await;
        let input = vec![UserInput::Text {
            text: "hello".to_string(),
            text_elements: Vec::new(),
        }];
        sess.spawn_task(
            Arc::clone(&tc),
            input,
            NeverEndingTask {
                kind: TaskKind::Regular,
                listen_to_cancellation_token: false,
            },
        )
        .await;

        let steer_input = vec![UserInput::Text {
            text: "steer".to_string(),
            text_elements: Vec::new(),
        }];
        let turn_id = sess
            .steer_input(steer_input, Some(&tc.sub_id))
            .await
            .expect("steering with matching expected turn id should succeed");

        assert_eq!(turn_id, tc.sub_id);
        assert!(sess.has_pending_input().await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn abort_review_task_emits_exited_then_aborted_and_records_history() {
        let (sess, tc, rx) = make_session_and_context_with_rx().await;
        let input = vec![UserInput::Text {
            text: "start review".to_string(),
            text_elements: Vec::new(),
        }];
        sess.spawn_task(Arc::clone(&tc), input, ReviewTask::new())
            .await;

        sess.abort_all_tasks(TurnAbortReason::Interrupted).await;

        // Aborting a review task should exit review mode before surfacing the abort to the client.
        // We scan for these events (rather than relying on fixed ordering) since unrelated events
        // may interleave.
        let mut exited_review_mode_idx = None;
        let mut turn_aborted_idx = None;
        let mut idx = 0usize;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let evt = tokio::time::timeout(remaining, rx.recv())
                .await
                .expect("timeout waiting for event")
                .expect("event");
            let event_idx = idx;
            idx = idx.saturating_add(1);
            match evt.msg {
                EventMsg::ExitedReviewMode(ev) => {
                    assert!(ev.review_output.is_none());
                    exited_review_mode_idx = Some(event_idx);
                }
                EventMsg::TurnAborted(ev) => {
                    assert_eq!(TurnAbortReason::Interrupted, ev.reason);
                    turn_aborted_idx = Some(event_idx);
                    break;
                }
                _ => {}
            }
        }
        assert!(
            exited_review_mode_idx.is_some(),
            "expected ExitedReviewMode after abort"
        );
        assert!(
            turn_aborted_idx.is_some(),
            "expected TurnAborted after abort"
        );
        assert!(
            exited_review_mode_idx.unwrap() < turn_aborted_idx.unwrap(),
            "expected ExitedReviewMode before TurnAborted"
        );

        let history = sess.clone_history().await;
        // The `<turn_aborted>` marker is silent in the event stream, so verify it is still
        // recorded in history for the model.
        assert!(
            history.raw_items().iter().any(|item| {
                let ResponseItem::Message { role, content, .. } = item else {
                    return false;
                };
                if role != "user" {
                    return false;
                }
                content.iter().any(|content_item| {
                    let ContentItem::InputText { text } = content_item else {
                        return false;
                    };
                    text.contains(crate::contextual_user_message::TURN_ABORTED_OPEN_TAG)
                })
            }),
            "expected a model-visible turn aborted marker in history after interrupt"
        );
    }

    #[tokio::test]
    async fn fatal_tool_error_stops_turn_and_reports_error() {
        let (session, turn_context, _rx) = make_session_and_context_with_rx().await;
        let tools = {
            session
                .services
                .mcp_connection_manager
                .read()
                .await
                .list_all_tools()
                .await
        };
        let app_tools = Some(tools.clone());
        let router = ToolRouter::from_config(
            &turn_context.tools_config,
            ToolRouterParams {
                mcp_tools: Some(
                    tools
                        .into_iter()
                        .map(|(name, tool)| (name, tool.tool))
                        .collect(),
                ),
                app_tools,
                discoverable_tools: None,
                dynamic_tools: turn_context.dynamic_tools.as_slice(),
            },
        );
        let item = ResponseItem::CustomToolCall {
            id: None,
            status: None,
            call_id: "call-1".to_string(),
            name: "shell".to_string(),
            input: "{}".to_string(),
        };

        let call = ToolRouter::build_tool_call(session.as_ref(), item.clone())
            .await
            .expect("build tool call")
            .expect("tool call present");
        let tracker = Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new()));
        let err = router
            .dispatch_tool_call_with_code_mode_result(
                Arc::clone(&session),
                Arc::clone(&turn_context),
                tracker,
                call,
                ToolCallSource::Direct,
            )
            .await
            .err()
            .expect("expected fatal error");

        match err {
            FunctionCallError::Fatal(message) => {
                assert_eq!(message, "tool shell invoked with incompatible payload");
            }
            other => panic!("expected FunctionCallError::Fatal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn hook_auto_reply_guard_state_transitions_are_stable() {
        let (sess, _tc, _rx) = make_session_and_context_with_rx().await;

        let first = sess
            .try_reserve_hook_auto_reply_chain_slot()
            .expect("reserve first slot");
        assert_eq!(first.0, 1);
        assert_eq!(
            sess.try_reserve_hook_auto_reply_chain_slot(),
            Some((2, first.1))
        );

        sess.release_hook_auto_reply_chain_slot_for_epoch(first.1);
        let second = sess
            .try_reserve_hook_auto_reply_chain_slot()
            .expect("reserve second slot");
        assert_eq!(second.0, 2);
        assert_eq!(second.1, first.1);

        sess.mark_turn_terminal_event_emitted("turn-a").await;
        {
            let seen = sess.hook_seen_terminal_turn_ids.lock().await;
            assert!(seen.contains("turn-a"));
        }
        sess.clear_turn_terminal_marker("turn-a").await;
        {
            let seen = sess.hook_seen_terminal_turn_ids.lock().await;
            assert!(!seen.contains("turn-a"));
        }

        let next_epoch = sess.note_user_input_activity().await;
        assert!(next_epoch > second.1);
        let stale_epoch = second.1;
        sess.release_hook_auto_reply_chain_slot_for_epoch(stale_epoch);
        let after_bump = sess
            .try_reserve_hook_auto_reply_chain_slot()
            .expect("reserve slot in newer generation");
        assert_eq!(after_bump.0, 1);
        assert_eq!(after_bump.1, next_epoch);
    }

    #[tokio::test]
    async fn nero_hook_msg_throttle_cache_can_be_reset() {
        let (sess, _tc, _rx) = make_session_and_context_with_rx().await;

        let remaining = sess.nero_hook_msg_throttle_remaining(
            "test-hook",
            &NeroHookMsgMode::TuiShort,
            &NeroHookMsgFormat::Block,
            true,
            true,
            "full",
            "short",
            None,
            120,
        );
        assert!(remaining.is_none(), "first emit should not be throttled");

        let remaining = sess.nero_hook_msg_throttle_remaining(
            "test-hook",
            &NeroHookMsgMode::TuiShort,
            &NeroHookMsgFormat::Block,
            true,
            true,
            "full",
            "short",
            None,
            120,
        );
        assert!(
            remaining.is_some(),
            "second immediate emit should be throttled"
        );

        sess.reset_nero_hook_msg_throttle();

        let remaining = sess.nero_hook_msg_throttle_remaining(
            "test-hook",
            &NeroHookMsgMode::TuiShort,
            &NeroHookMsgFormat::Block,
            true,
            true,
            "full",
            "short",
            None,
            120,
        );
        assert!(
            remaining.is_none(),
            "emit after reset should not be throttled (compaction reset semantics)"
        );
    }

    #[tokio::test]
    async fn nero_auto_session_auto_read_warning_emits_once_until_reset() {
        let (sess, tc, rx) = make_session_and_context_with_rx().await;

        sess.maybe_emit_nero_auto_session_auto_read_warning(&tc, "session-auto read failed")
            .await;
        let first = rx.recv().await.expect("first warning event");
        let first_message = match first.msg {
            EventMsg::Warning(WarningEvent { message }) => message,
            other => panic!("expected warning event, got {other:?}"),
        };
        assert!(first_message.contains("session-auto read failed"));

        sess.maybe_emit_nero_auto_session_auto_read_warning(&tc, "should be suppressed")
            .await;
        assert!(
            matches!(rx.try_recv(), Err(async_channel::TryRecvError::Empty)),
            "second warning should be suppressed until reset"
        );

        {
            let mut guard = match sess.nero_auto_bridge_warning_emitted.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            *guard = false;
        }

        sess.maybe_emit_nero_auto_session_auto_read_warning(&tc, "after reset")
            .await;
        let second = rx.recv().await.expect("warning event after reset");
        let second_message = match second.msg {
            EventMsg::Warning(WarningEvent { message }) => message,
            other => panic!("expected warning event after reset, got {other:?}"),
        };
        assert!(second_message.contains("after reset"));
    }

    #[test]
    fn runtime_delivery_contract_satisfied_matches_expected_delivery_matrix() {
        assert!(runtime_delivery_contract_satisfied(false, false));
        assert!(runtime_delivery_contract_satisfied(false, true));
        assert!(runtime_delivery_contract_satisfied(true, true));
        assert!(!runtime_delivery_contract_satisfied(true, false));
    }

    #[test]
    fn normalized_nero_hook_status_kind_normalizes_case_and_spacing() {
        let state_status = codex_hooks::NeroHookMsgStatus {
            kind: " State ".to_string(),
            text: "healthy".to_string(),
            meta: None,
        };
        let auto_status = codex_hooks::NeroHookMsgStatus {
            kind: "AUTO".to_string(),
            text: "continue".to_string(),
            meta: None,
        };
        let warning_status = codex_hooks::NeroHookMsgStatus {
            kind: "warning".to_string(),
            text: "double-check".to_string(),
            meta: None,
        };

        let state_kind = normalized_nero_hook_status_kind(Some(&state_status));
        let auto_kind = normalized_nero_hook_status_kind(Some(&auto_status));
        let warning_kind = normalized_nero_hook_status_kind(Some(&warning_status));

        assert_eq!(state_kind.as_deref(), Some("state"));
        assert_eq!(auto_kind.as_deref(), Some("auto"));
        assert_eq!(warning_kind.as_deref(), Some("warning"));
    }

    #[test]
    fn status_nero_hook_tui_delivery_in_block_mode_prefers_warning_block() {
        let status = codex_hooks::NeroHookMsgStatus {
            kind: "state".to_string(),
            text: "healthy".to_string(),
            meta: None,
        };

        let delivery = nero_hook_tui_delivery(
            "NERO HOOK SYSTEM",
            NeroHookMsgFormat::Block,
            Some(&status),
            Some("state"),
        );

        assert!(delivery.hook_summary.is_none());
        let warning = delivery.warning.expect("block delivery warning");
        assert!(warning.contains("content = NERO HOOK SYSTEM"));
        assert!(warning.contains("status = state: healthy"));
    }

    #[test]
    fn warning_status_nero_hook_tui_delivery_uses_warning_summary_entry() {
        let status = codex_hooks::NeroHookMsgStatus {
            kind: "warning".to_string(),
            text: "campaign unresolved".to_string(),
            meta: None,
        };

        let delivery = nero_hook_tui_delivery(
            "NERO HOOK SYSTEM",
            NeroHookMsgFormat::Inline,
            Some(&status),
            Some("warning"),
        );

        assert_eq!(
            delivery.hook_summary,
            Some(codex_protocol::protocol::HookOutputEntry {
                kind: codex_protocol::protocol::HookOutputEntryKind::Warning,
                text: "NERO HOOK SYSTEM [warning: campaign unresolved]".to_string(),
            })
        );
        assert!(delivery.warning.is_none());
    }

    #[test]
    fn after_agent_summary_entries_build_hook_completed_event() {
        let entries = vec![codex_protocol::protocol::HookOutputEntry {
            kind: codex_protocol::protocol::HookOutputEntryKind::Context,
            text: "NERO HOOK SYSTEM [state: healthy]".to_string(),
        }];

        let event = after_agent_runtime_hook_completed_event(
            "turn-1",
            "nero-hook-runtime",
            codex_protocol::protocol::HookRunStatus::Completed,
            Some("after_agent runtime status".to_string()),
            Some(json!({
                "status": {
                    "kind_normalized": "state",
                    "meta": null,
                },
                "protocol": {
                    "status": "ok",
                    "stop_checkpoint_expected": true,
                    "stop_checkpoint_delivered": true,
                    "contract_satisfied": true,
                    "nero_hook_msg_total": 1,
                    "nero_hook_msg_throttled": 0,
                    "auto_user_replies_blocked": 0,
                },
                "follow_up": {
                    "status": "queued",
                    "queued_count": 1,
                    "blocked_count": 0,
                },
            })),
            entries.clone(),
        )
        .expect("expected hook completed event");

        assert_eq!(event.turn_id.as_deref(), Some("turn-1"));
        assert_eq!(event.run.id, "after-agent:nero-hook-runtime:turn-1");
        assert_eq!(
            event.run.event_name,
            codex_protocol::protocol::HookEventName::AfterAgent
        );
        assert_eq!(
            event.run.handler_type,
            codex_protocol::protocol::HookHandlerType::Agent
        );
        assert_eq!(
            event.run.execution_mode,
            codex_protocol::protocol::HookExecutionMode::Sync
        );
        assert_eq!(event.run.scope, codex_protocol::protocol::HookScope::Turn);
        assert_eq!(
            event.run.source_path,
            PathBuf::from("hook://after_agent/nero-hook-runtime")
        );
        assert_eq!(
            event.run.status,
            codex_protocol::protocol::HookRunStatus::Completed
        );
        assert_eq!(
            event.run.status_message.as_deref(),
            Some("after_agent runtime status")
        );
        assert_eq!(event.run.entries, entries);
        assert_eq!(
            event.run.meta,
            Some(json!({
                "status": {
                    "kind_normalized": "state",
                    "meta": null,
                },
                "protocol": {
                    "status": "ok",
                    "stop_checkpoint_expected": true,
                    "stop_checkpoint_delivered": true,
                    "contract_satisfied": true,
                    "nero_hook_msg_total": 1,
                    "nero_hook_msg_throttled": 0,
                    "auto_user_replies_blocked": 0,
                },
                "follow_up": {
                    "status": "queued",
                    "queued_count": 1,
                    "blocked_count": 0,
                },
            }))
        );
        assert_eq!(event.run.completed_at, Some(event.run.started_at));
        assert_eq!(event.run.duration_ms, Some(0));
    }

    #[test]
    fn after_agent_runtime_meta_only_still_builds_hook_completed_event() {
        let event = after_agent_runtime_hook_completed_event(
            "turn-2",
            "nero-hook-runtime",
            codex_protocol::protocol::HookRunStatus::Completed,
            Some("after_agent runtime status".to_string()),
            Some(json!({
                "protocol": {
                    "status": "ok",
                    "stop_checkpoint_expected": true,
                    "stop_checkpoint_delivered": true,
                    "contract_satisfied": true,
                    "nero_hook_msg_total": 1,
                    "nero_hook_msg_throttled": 0,
                    "auto_user_replies_blocked": 0,
                },
                "follow_up": {
                    "status": "queued",
                    "queued_count": 1,
                    "blocked_count": 0,
                },
            })),
            Vec::new(),
        )
        .expect("expected meta-only hook completed event");

        assert_eq!(event.turn_id.as_deref(), Some("turn-2"));
        assert!(event.run.entries.is_empty());
        assert!(event.run.meta.is_some());
    }

    #[test]
    fn after_agent_runtime_hook_summary_meta_omits_expected_wait_when_unset() {
        let summary = after_agent_runtime_hook_summary_meta(
            "nero-hook-runtime",
            Some("state".to_string()),
            /*runtime_status_meta*/ None,
            "ok",
            /*stop_checkpoint_expected*/ true,
            /*stop_checkpoint_delivered*/ true,
            /*runtime_stop_command_delivered*/ true,
            /*nero_hook_msg_total*/ 1,
            /*nero_hook_msg_throttled*/ 0,
            /*follow_up_queued_count*/ 1,
            /*follow_up_blocked_count*/ 0,
            /*follow_up_expected_wait_seconds*/ None,
            /*follow_up_generation_epoch*/ None,
        );

        assert_eq!(
            summary["follow_up"],
            json!({
                "status": "queued",
                "queued_count": 1,
                "blocked_count": 0,
            })
        );
    }

    #[test]
    fn after_agent_runtime_hook_summary_meta_includes_expected_wait_when_set() {
        let summary = after_agent_runtime_hook_summary_meta(
            "nero-hook-runtime",
            Some("state".to_string()),
            /*runtime_status_meta*/ None,
            "ok",
            /*stop_checkpoint_expected*/ true,
            /*stop_checkpoint_delivered*/ true,
            /*runtime_stop_command_delivered*/ true,
            /*nero_hook_msg_total*/ 1,
            /*nero_hook_msg_throttled*/ 0,
            /*follow_up_queued_count*/ 1,
            /*follow_up_blocked_count*/ 0,
            /*follow_up_expected_wait_seconds*/ Some(2),
            /*follow_up_generation_epoch*/ Some(7),
        );

        assert_eq!(
            summary["follow_up"],
            json!({
                "status": "queued",
                "queued_count": 1,
                "blocked_count": 0,
                "expected_wait_seconds": 2,
                "generation_epoch": 7,
            })
        );
    }

    #[test]
    fn stop_delivery_contract_status_marks_unsatisfied_contract_as_failed() {
        assert_eq!(stop_delivery_contract_status(true, 0), "ok");
        assert_eq!(
            stop_delivery_contract_status(false, 2),
            "fail-closed-blocked"
        );
        assert_eq!(
            stop_delivery_contract_status(false, 0),
            "failed-stop-checkpoint-missing"
        );
    }

    #[test]
    fn align_auto_decision_meta_with_delivery_contract_overrides_continue_when_blocked() {
        let mut status_meta = Some(json!({
            "auto_stage": {
                "stage": "follow_up",
            },
            "auto_decision": {
                "decision": "continue",
                "reason_code": "continue",
                "score_explanation": "planned low-risk next step",
                "score": 7
            }
        }));

        align_auto_decision_meta_with_delivery_contract(&mut status_meta, false, 1);

        assert_eq!(
            status_meta,
            Some(json!({
                "auto_stage": {
                    "stage": "follow_up",
                },
                "auto_decision": {
                    "decision": "blocked-delivery-contract",
                    "reason_code": "delivery-contract-blocked",
                    "score_explanation": "STOP checkpoint was not delivered in this turn.",
                    "score": 7
                }
            }))
        );
    }

    #[test]
    fn align_auto_decision_meta_with_delivery_contract_keeps_continue_when_contract_satisfied() {
        let mut status_meta = Some(json!({
            "auto_decision": {
                "decision": "continue",
                "reason_code": "continue",
                "score_explanation": "planned low-risk next step"
            }
        }));

        align_auto_decision_meta_with_delivery_contract(&mut status_meta, true, 1);

        assert_eq!(
            status_meta,
            Some(json!({
                "auto_decision": {
                    "decision": "continue",
                    "reason_code": "continue",
                    "score_explanation": "planned low-risk next step"
                }
            }))
        );
    }

    #[test]
    fn align_auto_decision_meta_with_delivery_contract_builds_meta_when_shape_is_non_object() {
        let mut status_meta = Some(json!("invalid-meta-shape"));

        align_auto_decision_meta_with_delivery_contract(&mut status_meta, false, 1);

        assert_eq!(
            status_meta,
            Some(json!({
                "auto_decision": {
                    "decision": "blocked-delivery-contract",
                    "reason_code": "delivery-contract-blocked",
                    "score_explanation": "STOP checkpoint was not delivered in this turn."
                }
            }))
        );
    }

    #[test]
    fn sanitize_nero_hook_status_meta_for_audit_keeps_allowlisted_fields() {
        let long_explanation = "x".repeat(NERO_HOOK_STATUS_META_MAX_STRING_CHARS + 32);
        let input = Some(json!({
            "auto_stage": {
                "stage": "decision",
                "private": "drop-me",
            },
            "auto_decision": {
                "decision": "continue",
                "reason_code": "continue",
                "campaign_id": "A",
                "campaign_status": "active",
                "score": 9,
                "effective_threshold": 5,
                "score_explanation": long_explanation,
                "flags": {
                    "emergency_flag": false,
                    "gates_done_observed": true,
                    "user_collaboration_required": false,
                    "unknown_flag": true
                },
                "session_auto_policy_override": {
                    "autonomy_level": 5,
                    "autonomy_step_per_round": 1,
                    "max_auto_rounds": 4,
                    "unknown": 99
                },
                "private_blob": {
                    "secret": "dont-log-me"
                }
            },
            "extra_top_level": {
                "secret": "drop-me"
            }
        }));

        let sanitized = sanitize_nero_hook_status_meta_for_audit(input).expect("sanitized meta");
        let auto_stage = sanitized
            .get("auto_stage")
            .and_then(|value| value.as_object())
            .expect("auto_stage object");
        let auto = sanitized
            .get("auto_decision")
            .and_then(|value| value.as_object())
            .expect("auto_decision object");

        assert_eq!(
            auto_stage.get("stage"),
            Some(&Value::String("decision".to_string()))
        );
        assert_eq!(
            auto.get("decision"),
            Some(&Value::String("continue".to_string()))
        );
        assert_eq!(
            auto.get("campaign_id"),
            Some(&Value::String("A".to_string()))
        );
        assert_eq!(
            auto.get("campaign_status"),
            Some(&Value::String("active".to_string()))
        );
        assert!(auto.get("private_blob").is_none());
        assert!(sanitized.get("extra_top_level").is_none());

        let score_explanation = auto
            .get("score_explanation")
            .and_then(|value| value.as_str())
            .expect("score_explanation string");
        assert!(
            score_explanation.ends_with("...[truncated]"),
            "long strings should be truncated in audit metadata"
        );

        let flags = auto
            .get("flags")
            .and_then(|value| value.as_object())
            .expect("flags object");
        assert!(flags.get("unknown_flag").is_none());
        assert_eq!(flags.get("emergency_flag"), Some(&Value::Bool(false)));
    }

    #[test]
    fn sanitize_nero_hook_status_meta_for_audit_drops_non_object_shapes() {
        assert!(sanitize_nero_hook_status_meta_for_audit(Some(json!(["array"]))).is_none());
        assert!(sanitize_nero_hook_status_meta_for_audit(Some(json!({"noop": true}))).is_none());
        assert!(
            sanitize_nero_hook_status_meta_for_audit(Some(json!({"auto_decision": "x"}))).is_none()
        );
    }

    #[test]
    fn merge_after_agent_runtime_status_meta_adds_disjoint_keys_and_ignores_none() {
        let mut accumulated = Some(json!({
            "auto_stage": {
                "stage": "decision",
            }
        }));

        merge_after_agent_runtime_status_meta(
            &mut accumulated,
            Some(json!({
                "auto_decision": {
                    "decision": "continue",
                }
            })),
        );
        merge_after_agent_runtime_status_meta(&mut accumulated, None);

        assert_eq!(
            accumulated,
            Some(json!({
                "auto_stage": {
                    "stage": "decision",
                },
                "auto_decision": {
                    "decision": "continue",
                }
            }))
        );
    }

    #[test]
    fn merge_after_agent_runtime_status_meta_keeps_last_write_on_same_key() {
        let mut accumulated = Some(json!({
            "auto_decision": {
                "decision": "continue",
                "campaign_status": "active",
            }
        }));

        merge_after_agent_runtime_status_meta(
            &mut accumulated,
            Some(json!({
                "auto_decision": {
                    "decision": "block",
                }
            })),
        );

        assert_eq!(
            accumulated,
            Some(json!({
                "auto_decision": {
                    "decision": "block",
                }
            }))
        );
    }

    #[test]
    fn merge_after_agent_runtime_status_meta_replaces_same_key_auto_decision_whole_object() {
        let mut accumulated = Some(json!({
            "auto_decision": {
                "decision": "continue",
                "flags": {
                    "emergency_flag": false,
                    "user_collaboration_required": false,
                },
                "session_auto_policy_override": {
                    "autonomy_level": 5,
                    "max_auto_rounds": 4,
                },
            }
        }));

        merge_after_agent_runtime_status_meta(
            &mut accumulated,
            Some(json!({
                "auto_decision": {
                    "decision": "block",
                    "flags": {
                        "emergency_flag": true,
                    },
                    "session_auto_policy_override": {
                        "max_auto_rounds": 1,
                    },
                }
            })),
        );

        assert_eq!(
            accumulated,
            Some(json!({
                "auto_decision": {
                    "decision": "block",
                    "flags": {
                        "emergency_flag": true,
                    },
                    "session_auto_policy_override": {
                        "max_auto_rounds": 1,
                    },
                }
            }))
        );
    }

    #[test]
    fn normalize_hook_auto_reply_wait_seconds_uses_existing_runtime_budget() {
        let max_wait_seconds = HOOK_AUTO_REPLY_WAIT_FOR_TERMINAL_TIMEOUT_MS.div_ceil(1000);

        assert_eq!(Session::normalize_hook_auto_reply_wait_seconds(None), None);
        assert_eq!(
            Session::normalize_hook_auto_reply_wait_seconds(Some(0)),
            None
        );
        assert_eq!(
            Session::normalize_hook_auto_reply_wait_seconds(Some(1)),
            Some(1)
        );
        assert_eq!(
            Session::normalize_hook_auto_reply_wait_seconds(Some(max_wait_seconds)),
            Some(max_wait_seconds)
        );
        assert_eq!(
            Session::normalize_hook_auto_reply_wait_seconds(Some(
                max_wait_seconds.saturating_add(9)
            )),
            Some(max_wait_seconds)
        );
    }

    async fn sample_rollout(
        session: &Session,
        _turn_context: &TurnContext,
    ) -> (Vec<RolloutItem>, Vec<ResponseItem>) {
        let mut rollout_items = Vec::new();
        let mut live_history = ContextManager::new();

        // Use the same turn_context source as record_initial_history so model_info (and thus
        // personality_spec) matches reconstruction.
        let reconstruction_turn = session.new_default_turn().await;
        let mut initial_context = session
            .build_initial_context(reconstruction_turn.as_ref())
            .await;
        // Ensure personality_spec is present when Personality is enabled, so expected matches
        // what reconstruction produces (build_initial_context may omit it when baked into model).
        if !initial_context.iter().any(|m| {
            matches!(m, ResponseItem::Message { role, content, .. }
                if role == "developer"
                    && content.iter().any(|c| {
                        matches!(c, ContentItem::InputText { text } if text.contains("<personality_spec>"))
                    }))
        })
            && let Some(p) = reconstruction_turn.personality
            && session.features.enabled(Feature::Personality)
            && let Some(personality_message) = reconstruction_turn
                .model_info
                .model_messages
                .as_ref()
                .and_then(|m| m.get_personality_message(Some(p)).filter(|s| !s.is_empty()))
        {
            let msg =
                DeveloperInstructions::personality_spec_message(personality_message).into();
            let insert_at = initial_context
                .iter()
                .position(|m| matches!(m, ResponseItem::Message { role, .. } if role == "developer"))
                .map(|i| i + 1)
                .unwrap_or(0);
            initial_context.insert(insert_at, msg);
        }
        for item in &initial_context {
            rollout_items.push(RolloutItem::ResponseItem(item.clone()));
        }
        live_history.record_items(
            initial_context.iter(),
            reconstruction_turn.truncation_policy,
        );

        let user1 = ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "first user".to_string(),
            }],
            end_turn: None,
            phase: None,
        };
        live_history.record_items(
            std::iter::once(&user1),
            reconstruction_turn.truncation_policy,
        );
        rollout_items.push(RolloutItem::ResponseItem(user1.clone()));

        let assistant1 = ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "assistant reply one".to_string(),
            }],
            end_turn: None,
            phase: None,
        };
        live_history.record_items(
            std::iter::once(&assistant1),
            reconstruction_turn.truncation_policy,
        );
        rollout_items.push(RolloutItem::ResponseItem(assistant1.clone()));

        let summary1 = "summary one";
        let snapshot1 = live_history
            .clone()
            .for_prompt(&reconstruction_turn.model_info.input_modalities);
        let user_messages1 = collect_user_messages(&snapshot1);
        let rebuilt1 = compact::build_compacted_history(Vec::new(), &user_messages1, summary1);
        live_history.replace(rebuilt1);
        rollout_items.push(RolloutItem::Compacted(CompactedItem {
            message: summary1.to_string(),
            replacement_history: None,
        }));

        let user2 = ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "second user".to_string(),
            }],
            end_turn: None,
            phase: None,
        };
        live_history.record_items(
            std::iter::once(&user2),
            reconstruction_turn.truncation_policy,
        );
        rollout_items.push(RolloutItem::ResponseItem(user2.clone()));

        let assistant2 = ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "assistant reply two".to_string(),
            }],
            end_turn: None,
            phase: None,
        };
        live_history.record_items(
            std::iter::once(&assistant2),
            reconstruction_turn.truncation_policy,
        );
        rollout_items.push(RolloutItem::ResponseItem(assistant2.clone()));

        let summary2 = "summary two";
        let snapshot2 = live_history
            .clone()
            .for_prompt(&reconstruction_turn.model_info.input_modalities);
        let user_messages2 = collect_user_messages(&snapshot2);
        let rebuilt2 = compact::build_compacted_history(Vec::new(), &user_messages2, summary2);
        live_history.replace(rebuilt2);
        rollout_items.push(RolloutItem::Compacted(CompactedItem {
            message: summary2.to_string(),
            replacement_history: None,
        }));

        let user3 = ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "third user".to_string(),
            }],
            end_turn: None,
            phase: None,
        };
        live_history.record_items(
            std::iter::once(&user3),
            reconstruction_turn.truncation_policy,
        );
        rollout_items.push(RolloutItem::ResponseItem(user3));

        let assistant3 = ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "assistant reply three".to_string(),
            }],
            end_turn: None,
            phase: None,
        };
        live_history.record_items(
            std::iter::once(&assistant3),
            reconstruction_turn.truncation_policy,
        );
        rollout_items.push(RolloutItem::ResponseItem(assistant3));

        (
            rollout_items,
            live_history.for_prompt(&reconstruction_turn.model_info.input_modalities),
        )
    }

    #[tokio::test]
    async fn rejects_escalated_permissions_when_policy_not_on_request() {
        use crate::exec::ExecParams;
        use crate::protocol::AskForApproval;
        use crate::protocol::SandboxPolicy;
        use crate::sandboxing::SandboxPermissions;
        use crate::turn_diff_tracker::TurnDiffTracker;
        use std::collections::HashMap;

        let (session, mut turn_context_raw) = make_session_and_context().await;
        // Ensure policy is NOT OnRequest so the early rejection path triggers
        turn_context_raw
            .approval_policy
            .set(AskForApproval::OnFailure)
            .expect("test setup should allow updating approval policy");
        let session = Arc::new(session);
        let mut turn_context = Arc::new(turn_context_raw);

        let timeout_ms = 1000;
        let sandbox_permissions = SandboxPermissions::RequireEscalated;
        let params = ExecParams {
            command: if cfg!(windows) {
                vec![
                    "cmd.exe".to_string(),
                    "/C".to_string(),
                    "echo hi".to_string(),
                ]
            } else {
                vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "echo hi".to_string(),
                ]
            },
            cwd: turn_context.cwd.clone().to_path_buf(),
            expiration: timeout_ms.into(),
            capture_policy: ExecCapturePolicy::ShellTool,
            env: HashMap::new(),
            network: None,
            sandbox_permissions,
            windows_sandbox_level: turn_context.windows_sandbox_level,
            windows_sandbox_private_desktop: turn_context
                .config
                .permissions
                .windows_sandbox_private_desktop,
            justification: Some("test".to_string()),
            arg0: None,
        };

        let params2 = ExecParams {
            sandbox_permissions: SandboxPermissions::UseDefault,
            command: params.command.clone(),
            cwd: params.cwd.clone(),
            expiration: timeout_ms.into(),
            capture_policy: ExecCapturePolicy::ShellTool,
            env: HashMap::new(),
            network: None,
            windows_sandbox_level: turn_context.windows_sandbox_level,
            windows_sandbox_private_desktop: turn_context
                .config
                .permissions
                .windows_sandbox_private_desktop,
            justification: params.justification.clone(),
            arg0: None,
        };

        let turn_diff_tracker = Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new()));

        let tool_name = "shell";
        let call_id = "test-call".to_string();

        let handler = ShellHandler;
        let resp = handler
            .handle(ToolInvocation {
                session: Arc::clone(&session),
                turn: Arc::clone(&turn_context),
                tracker: Arc::clone(&turn_diff_tracker),
                call_id,
                tool_name: tool_name.to_string(),
                tool_namespace: None,
                payload: ToolPayload::Function {
                    arguments: serde_json::json!({
                        "command": params.command.clone(),
                        "workdir": Some(turn_context.cwd.to_string_lossy().to_string()),
                        "timeout_ms": params.expiration.timeout_ms(),
                        "sandbox_permissions": params.sandbox_permissions,
                        "justification": params.justification.clone(),
                    })
                    .to_string(),
                },
            })
            .await;

        let Err(FunctionCallError::RespondToModel(output)) = resp else {
            panic!("expected error result");
        };

        let expected = format!(
            "approval policy is {policy:?}; reject command — you should not ask for escalated permissions if the approval policy is {policy:?}",
            policy = turn_context.approval_policy.value()
        );

        pretty_assertions::assert_eq!(output, expected);

        // Now retry the same command WITHOUT escalated permissions; should succeed.
        // Force DangerFullAccess to avoid platform sandbox dependencies in tests.
        let turn_context_mut = Arc::get_mut(&mut turn_context).expect("unique turn context Arc");
        turn_context_mut
            .sandbox_policy
            .set(SandboxPolicy::DangerFullAccess)
            .expect("test setup should allow updating sandbox policy");
        turn_context_mut.file_system_sandbox_policy =
            FileSystemSandboxPolicy::from(turn_context_mut.sandbox_policy.get());
        turn_context_mut.network_sandbox_policy =
            NetworkSandboxPolicy::from(turn_context_mut.sandbox_policy.get());

        let resp2 = handler
            .handle(ToolInvocation {
                session: Arc::clone(&session),
                turn: Arc::clone(&turn_context),
                tracker: Arc::clone(&turn_diff_tracker),
                call_id: "test-call-2".to_string(),
                tool_name: tool_name.to_string(),
                tool_namespace: None,
                payload: ToolPayload::Function {
                    arguments: serde_json::json!({
                        "command": params2.command.clone(),
                        "workdir": Some(turn_context.cwd.to_string_lossy().to_string()),
                        "timeout_ms": params2.expiration.timeout_ms(),
                        "sandbox_permissions": params2.sandbox_permissions,
                        "justification": params2.justification.clone(),
                    })
                    .to_string(),
                },
            })
            .await;

        let output = resp2.expect("expected Ok result").into_text();

        #[derive(Deserialize, PartialEq, Eq, Debug)]
        struct ResponseExecMetadata {
            exit_code: i32,
        }

        #[derive(Deserialize)]
        struct ResponseExecOutput {
            output: String,
            metadata: ResponseExecMetadata,
        }

        let exec_output: ResponseExecOutput =
            serde_json::from_str(&output).expect("valid exec output json");

        pretty_assertions::assert_eq!(exec_output.metadata, ResponseExecMetadata { exit_code: 0 });
        assert!(exec_output.output.contains("hi"));
    }
    #[tokio::test]
    async fn unified_exec_rejects_escalated_permissions_when_policy_not_on_request() {
        use crate::protocol::AskForApproval;
        use crate::sandboxing::SandboxPermissions;
        use crate::turn_diff_tracker::TurnDiffTracker;

        let (session, mut turn_context_raw) = make_session_and_context().await;
        turn_context_raw
            .approval_policy
            .set(AskForApproval::OnFailure)
            .expect("test setup should allow updating approval policy");
        let session = Arc::new(session);
        let turn_context = Arc::new(turn_context_raw);
        let tracker = Arc::new(tokio::sync::Mutex::new(TurnDiffTracker::new()));

        let handler = UnifiedExecHandler;
        let resp = handler
            .handle(ToolInvocation {
                session: Arc::clone(&session),
                turn: Arc::clone(&turn_context),
                tracker: Arc::clone(&tracker),
                call_id: "exec-call".to_string(),
                tool_name: "exec_command".to_string(),
                tool_namespace: None,
                payload: ToolPayload::Function {
                    arguments: serde_json::json!({
                        "cmd": "echo hi",
                        "sandbox_permissions": SandboxPermissions::RequireEscalated,
                        "justification": "need unsandboxed execution",
                    })
                    .to_string(),
                },
            })
            .await;

        let Err(FunctionCallError::RespondToModel(output)) = resp else {
            panic!("expected error result");
        };

        let expected = format!(
            "approval policy is {policy:?}; reject command — you cannot ask for escalated permissions if the approval policy is {policy:?}",
            policy = turn_context.approval_policy.value()
        );

        pretty_assertions::assert_eq!(output, expected);
    }

    #[test]
    fn model_fallback_trigger_policy_is_conservative_for_503() {
        assert!(should_trigger_model_fallback(&CodexErr::ServerOverloaded));

        let controlled_503 = crate::error::UnexpectedResponseError {
            status: http::StatusCode::SERVICE_UNAVAILABLE,
            body: r#"{"error":{"message":"provider temporarily unavailable due to high demand"}}"#
                .to_string(),
            url: None,
            cf_ray: None,
            request_id: None,
            identity_authorization_error: None,
            identity_error_code: None,
        };
        assert!(should_trigger_model_fallback(&CodexErr::UnexpectedStatus(
            controlled_503
        )));

        let unrelated_503 = crate::error::UnexpectedResponseError {
            status: http::StatusCode::SERVICE_UNAVAILABLE,
            body: "maintenance window".to_string(),
            url: None,
            cf_ray: None,
            request_id: None,
            identity_authorization_error: None,
            identity_error_code: None,
        };
        assert!(!should_trigger_model_fallback(&CodexErr::UnexpectedStatus(
            unrelated_503
        )));
    }

    #[test]
    fn next_available_model_fallback_step_respects_rotation_and_cooldown() {
        let ladder = vec![
            NeroModelFallbackStep {
                model: "model-a".to_string(),
                reasoning_effort: ReasoningEffortConfig::High,
            },
            NeroModelFallbackStep {
                model: "model-b".to_string(),
                reasoning_effort: ReasoningEffortConfig::Medium,
            },
            NeroModelFallbackStep {
                model: "model-c".to_string(),
                reasoning_effort: ReasoningEffortConfig::Low,
            },
        ];
        let now = StdInstant::now();
        let mut cooldown = std::collections::HashMap::<String, StdInstant>::new();
        cooldown.insert("model-b".to_string(), now + StdDuration::from_secs(20));

        let selected =
            next_available_model_fallback_step(&ladder, Some("model-a"), now, |model, now| {
                cooldown
                    .get(model)
                    .and_then(|until| until.checked_duration_since(now))
            });
        assert_eq!(
            selected.as_ref().map(|step| step.model.as_str()),
            Some("model-c")
        );
    }
}
