//! Apply Patch runtime: executes verified patches under the orchestrator.
//!
//! Assumes `apply_patch` verification/approval happened upstream. Reuses that
//! decision to avoid re-prompting, builds the self-invocation command for
//! `codex --codex-run-as-apply-patch`, and runs under the current
//! `SandboxAttempt` with a minimal environment.
use crate::error::CodexErr;
use crate::error::SandboxErr;
use crate::exec::ExecCapturePolicy;
use crate::exec::ExecToolCallOutput;
use crate::guardian::GuardianApprovalRequest;
use crate::guardian::review_approval_request;
use crate::guardian::routes_approval_to_guardian;
use crate::sandboxing::ExecOptions;
use crate::sandboxing::execute_env;
use crate::tools::sandboxing::Approvable;
use crate::tools::sandboxing::ApprovalCtx;
use crate::tools::sandboxing::ExecApprovalRequirement;
use crate::tools::sandboxing::SandboxAttempt;
use crate::tools::sandboxing::Sandboxable;
use crate::tools::sandboxing::ToolCtx;
use crate::tools::sandboxing::ToolError;
use crate::tools::sandboxing::ToolRuntime;
use crate::tools::sandboxing::with_cached_approval;
use codex_apply_patch::ApplyPatchAction;
use codex_apply_patch::CODEX_CORE_APPLY_PATCH_ARG1;
use codex_nero_self_exec::ResolvedSelfExec;
use codex_nero_self_exec::SelfExecCandidateDiagnostic;
use codex_nero_self_exec::SelfExecPaths;
use codex_nero_self_exec::SelfExecProgramSource;
use codex_nero_self_exec::resolve_self_exec;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::ReviewDecision;
use codex_sandboxing::SandboxCommand;
use codex_sandboxing::SandboxType;
use codex_sandboxing::SandboxablePreference;
use codex_utils_absolute_path::AbsolutePathBuf;
use futures::future::BoxFuture;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug)]
pub struct ApplyPatchRequest {
    pub action: ApplyPatchAction,
    pub file_paths: Vec<AbsolutePathBuf>,
    pub changes: std::collections::HashMap<PathBuf, FileChange>,
    pub exec_approval_requirement: ExecApprovalRequirement,
    pub additional_permissions: Option<PermissionProfile>,
    pub permissions_preapproved: bool,
    pub timeout_ms: Option<u64>,
}

#[derive(Default)]
pub struct ApplyPatchRuntime;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApplyPatchProgramSource {
    ConfiguredSelfExecPrimary,
    ConfiguredSelfExecFallback,
    CurrentExe,
    #[cfg(target_os = "windows")]
    WindowsResolvedLaunchExe,
}

impl ApplyPatchProgramSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::ConfiguredSelfExecPrimary => "configured_self_exec_primary",
            Self::ConfiguredSelfExecFallback => "configured_self_exec_fallback",
            Self::CurrentExe => "current_exe",
            #[cfg(target_os = "windows")]
            Self::WindowsResolvedLaunchExe => "windows_resolved_launch_exe",
        }
    }
}

#[cfg(not(target_os = "windows"))]
impl From<SelfExecProgramSource> for ApplyPatchProgramSource {
    fn from(value: SelfExecProgramSource) -> Self {
        match value {
            SelfExecProgramSource::ConfiguredPrimary => Self::ConfiguredSelfExecPrimary,
            SelfExecProgramSource::ConfiguredFallback => Self::ConfiguredSelfExecFallback,
            SelfExecProgramSource::CurrentExe => Self::CurrentExe,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct ApplyPatchPathStatus {
    path: PathBuf,
    exists: bool,
    is_file: bool,
    is_dir: bool,
    metadata_error: Option<String>,
}

impl ApplyPatchPathStatus {
    fn from_path(path: &Path) -> Self {
        match std::fs::metadata(path) {
            Ok(metadata) => Self {
                path: path.to_path_buf(),
                exists: true,
                is_file: metadata.is_file(),
                is_dir: metadata.is_dir(),
                metadata_error: None,
            },
            Err(err) => Self {
                path: path.to_path_buf(),
                exists: false,
                is_file: false,
                is_dir: false,
                metadata_error: Some(err.to_string()),
            },
        }
    }

    fn missing_file(&self) -> bool {
        !self.exists || !self.is_file
    }

    fn missing_dir(&self) -> bool {
        !self.exists || !self.is_dir
    }

    fn render(&self, label: &str) -> String {
        format!(
            "{label}={} exists={} is_file={} is_dir={} metadata_error={}",
            self.path.display(),
            self.exists,
            self.is_file,
            self.is_dir,
            self.metadata_error.as_deref().unwrap_or("none")
        )
    }
}

#[derive(Debug)]
struct ApplyPatchLaunchContext {
    program_source: ApplyPatchProgramSource,
    resolver_diagnostics: Vec<SelfExecCandidateDiagnostic>,
    pre_sandbox_program: String,
    pre_sandbox_program_status: Option<ApplyPatchPathStatus>,
    pre_sandbox_cwd: ApplyPatchPathStatus,
    sandbox: SandboxType,
    final_program: String,
    final_program_status: Option<ApplyPatchPathStatus>,
}

impl ApplyPatchLaunchContext {
    fn from_exec_request(
        program_source: ApplyPatchProgramSource,
        resolver_diagnostics: Vec<SelfExecCandidateDiagnostic>,
        pre_sandbox_program: &Path,
        pre_sandbox_cwd: &Path,
        exec_request: &crate::sandboxing::ExecRequest,
    ) -> Self {
        let final_program = exec_request.command.first().cloned().unwrap_or_default();
        let final_program_status = apply_patch_program_path(&final_program)
            .map(|path| ApplyPatchPathStatus::from_path(path.as_path()));

        Self {
            program_source,
            resolver_diagnostics,
            pre_sandbox_program: pre_sandbox_program.display().to_string(),
            pre_sandbox_program_status: apply_patch_program_path(
                pre_sandbox_program.to_string_lossy().as_ref(),
            )
            .map(|path| ApplyPatchPathStatus::from_path(path.as_path())),
            pre_sandbox_cwd: ApplyPatchPathStatus::from_path(pre_sandbox_cwd),
            sandbox: exec_request.sandbox,
            final_program,
            final_program_status,
        }
    }

    fn has_executable_preflight_problem(&self) -> bool {
        self.pre_sandbox_program_status
            .as_ref()
            .is_some_and(ApplyPatchPathStatus::missing_file)
            || self
                .final_program_status
                .as_ref()
                .is_some_and(ApplyPatchPathStatus::missing_file)
    }

    fn has_cwd_preflight_problem(&self) -> bool {
        self.pre_sandbox_cwd.missing_dir()
    }

    fn has_preflight_problem(&self) -> bool {
        self.has_executable_preflight_problem() || self.has_cwd_preflight_problem()
    }

    fn render(&self) -> String {
        let mut lines = vec![format!("program_source={}", self.program_source.as_str())];
        lines.extend(
            self.resolver_diagnostics
                .iter()
                .map(SelfExecCandidateDiagnostic::render),
        );
        lines.push(if let Some(status) = &self.pre_sandbox_program_status {
            status.render("pre_sandbox_program")
        } else {
            format!(
                "pre_sandbox_program={} metadata_error=not_checked",
                self.pre_sandbox_program
            )
        });
        lines.push(self.pre_sandbox_cwd.render("pre_sandbox_cwd"));
        lines.push(format!("sandbox={:?}", self.sandbox));
        if let Some(status) = &self.final_program_status {
            lines.push(status.render("final_program"));
        } else {
            lines.push(format!(
                "final_program={} metadata_error=not_checked",
                self.final_program
            ));
        }
        lines.join("\n")
    }
}

#[derive(Debug)]
enum ApplyPatchPrepareError {
    ExecutablePath {
        label: &'static str,
        launch_context: Box<ApplyPatchLaunchContext>,
    },
    Other(String),
}

impl ApplyPatchPrepareError {
    fn from_launch_context(label: &'static str, launch_context: ApplyPatchLaunchContext) -> Self {
        if launch_context.has_executable_preflight_problem()
            && !launch_context.has_cwd_preflight_problem()
        {
            Self::ExecutablePath {
                label,
                launch_context: Box::new(launch_context),
            }
        } else {
            Self::Other(format!("{label}:\n{}", launch_context.render()))
        }
    }

    fn render(&self) -> String {
        match self {
            Self::ExecutablePath {
                label,
                launch_context,
            } => format!("{label}:\n{}", launch_context.render()),
            Self::Other(message) => message.clone(),
        }
    }

    fn into_tool_error(self) -> ToolError {
        ToolError::Message(self.render())
    }

    fn retryable_executable_path_context(&self) -> Option<&ApplyPatchLaunchContext> {
        match self {
            Self::ExecutablePath { launch_context, .. }
                if launch_context.program_source
                    == ApplyPatchProgramSource::ConfiguredSelfExecPrimary =>
            {
                Some(launch_context.as_ref())
            }
            _ => None,
        }
    }
}

#[cfg(not(target_os = "windows"))]
#[derive(Clone, Debug)]
struct PreparedApplyPatchCommand {
    program_source: ApplyPatchProgramSource,
    pre_sandbox_program: PathBuf,
    resolver_diagnostics: Vec<SelfExecCandidateDiagnostic>,
    retry_fallback: Option<PathBuf>,
}

fn apply_patch_program_path(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    (path.is_absolute() || program.contains(std::path::MAIN_SEPARATOR)).then(|| path.to_path_buf())
}

impl ApplyPatchRuntime {
    pub fn new() -> Self {
        Self
    }

    fn build_guardian_review_request(
        req: &ApplyPatchRequest,
        call_id: &str,
    ) -> GuardianApprovalRequest {
        GuardianApprovalRequest::ApplyPatch {
            id: call_id.to_string(),
            cwd: req.action.cwd.clone(),
            files: req.file_paths.clone(),
            change_count: req.changes.len(),
            patch: req.action.patch.clone(),
        }
    }

    #[cfg(target_os = "windows")]
    fn build_sandbox_command(
        req: &ApplyPatchRequest,
        codex_home: &std::path::Path,
    ) -> Result<(SandboxCommand, ApplyPatchProgramSource, PathBuf), ToolError> {
        let exe = codex_windows_sandbox::resolve_current_exe_for_launch(codex_home, "codex.exe");
        Ok((
            Self::build_sandbox_command_with_program(req, exe.clone()),
            ApplyPatchProgramSource::WindowsResolvedLaunchExe,
            exe,
        ))
    }

    #[cfg(not(target_os = "windows"))]
    fn build_sandbox_command(
        _req: &ApplyPatchRequest,
        self_exec_paths: &SelfExecPaths,
    ) -> Result<PreparedApplyPatchCommand, ToolError> {
        let resolved = Self::resolve_apply_patch_program(self_exec_paths)?;
        Ok(PreparedApplyPatchCommand {
            program_source: resolved.source.into(),
            pre_sandbox_program: resolved.path,
            resolver_diagnostics: resolved.diagnostics,
            retry_fallback: resolved.retry_fallback,
        })
    }

    #[cfg(not(target_os = "windows"))]
    fn resolve_apply_patch_program(
        self_exec_paths: &SelfExecPaths,
    ) -> Result<ResolvedSelfExec, ToolError> {
        resolve_self_exec(self_exec_paths).map_err(|err| {
            ToolError::Message(format!("failed to resolve apply_patch self-exec:\n{err}"))
        })
    }

    fn build_sandbox_command_with_program(req: &ApplyPatchRequest, exe: PathBuf) -> SandboxCommand {
        SandboxCommand {
            program: exe.into_os_string(),
            args: vec![
                CODEX_CORE_APPLY_PATCH_ARG1.to_string(),
                req.action.patch.clone(),
            ],
            cwd: req.action.cwd.clone(),
            // Run apply_patch with a minimal environment for determinism and to avoid leaks.
            env: HashMap::new(),
            additional_permissions: req.additional_permissions.clone(),
        }
    }

    fn prepare_exec_request(
        req: &ApplyPatchRequest,
        attempt: &SandboxAttempt<'_>,
        program_source: ApplyPatchProgramSource,
        pre_sandbox_program: PathBuf,
        resolver_diagnostics: Vec<SelfExecCandidateDiagnostic>,
    ) -> Result<(crate::sandboxing::ExecRequest, ApplyPatchLaunchContext), ApplyPatchPrepareError>
    {
        let preflight_launch_context = ApplyPatchLaunchContext {
            program_source,
            resolver_diagnostics: resolver_diagnostics.clone(),
            pre_sandbox_program: pre_sandbox_program.display().to_string(),
            pre_sandbox_program_status: apply_patch_program_path(
                pre_sandbox_program.to_string_lossy().as_ref(),
            )
            .map(|path| ApplyPatchPathStatus::from_path(path.as_path())),
            pre_sandbox_cwd: ApplyPatchPathStatus::from_path(req.action.cwd.as_path()),
            sandbox: attempt.sandbox,
            final_program: pre_sandbox_program.display().to_string(),
            final_program_status: apply_patch_program_path(
                pre_sandbox_program.to_string_lossy().as_ref(),
            )
            .map(|path| ApplyPatchPathStatus::from_path(path.as_path())),
        };
        if preflight_launch_context.has_preflight_problem() {
            return Err(ApplyPatchPrepareError::from_launch_context(
                "apply_patch launch preflight failed",
                preflight_launch_context,
            ));
        }

        let options = ExecOptions {
            expiration: req.timeout_ms.into(),
            capture_policy: ExecCapturePolicy::ShellTool,
        };
        let env = attempt
            .env_for(
                Self::build_sandbox_command_with_program(req, pre_sandbox_program.clone()),
                options,
                /*network*/ None,
            )
            .map_err(|err| {
                ApplyPatchPrepareError::Other(format!(
                    "apply_patch launch preparation failed:\nprogram_source={}\npre_sandbox_program={}\npre_sandbox_cwd={}\nsandbox={:?}\ntransform_error={err}",
                    program_source.as_str(),
                    pre_sandbox_program.display(),
                    req.action.cwd.display(),
                    attempt.sandbox,
                ))
            })?;
        let launch_context = ApplyPatchLaunchContext::from_exec_request(
            program_source,
            resolver_diagnostics,
            pre_sandbox_program.as_path(),
            req.action.cwd.as_path(),
            &env,
        );
        if launch_context.has_preflight_problem() {
            return Err(ApplyPatchPrepareError::from_launch_context(
                "apply_patch launch preflight failed",
                launch_context,
            ));
        }
        Ok((env, launch_context))
    }

    #[cfg(not(target_os = "windows"))]
    fn fallback_retry_command(
        fallback: PathBuf,
        resolver_diagnostics: Vec<SelfExecCandidateDiagnostic>,
    ) -> PreparedApplyPatchCommand {
        PreparedApplyPatchCommand {
            program_source: ApplyPatchProgramSource::ConfiguredSelfExecFallback,
            pre_sandbox_program: fallback,
            resolver_diagnostics,
            retry_fallback: None,
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn is_retryable_launch_error(err: &CodexErr, launch_context: &ApplyPatchLaunchContext) -> bool {
        if launch_context.program_source != ApplyPatchProgramSource::ConfiguredSelfExecPrimary {
            return false;
        }
        let CodexErr::Io(io_err) = err else {
            return false;
        };
        matches!(
            io_err.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
        ) || io_err
            .raw_os_error()
            .is_some_and(|code| matches!(code, libc::ENOEXEC | libc::ETXTBSY))
    }

    #[cfg(not(target_os = "windows"))]
    async fn execute_with_fallback_retry(
        req: &ApplyPatchRequest,
        attempt: &SandboxAttempt<'_>,
        ctx: &ToolCtx,
        fallback: PathBuf,
        resolver_diagnostics: Vec<SelfExecCandidateDiagnostic>,
        primary_context_text: &str,
        primary_error_text: &str,
    ) -> Result<ExecToolCallOutput, ToolError> {
        let retry_command = Self::fallback_retry_command(fallback, resolver_diagnostics);
        let (retry_env, retry_launch_context) = Self::prepare_exec_request(
            req,
            attempt,
            retry_command.program_source,
            retry_command.pre_sandbox_program,
            retry_command.resolver_diagnostics,
        )
        .map_err(|retry_prepare_err| {
            ToolError::Message(format!(
                "apply_patch launch failed after fallback retry:\nprimary_launch_context:\n{primary_context_text}\nprimary_error={primary_error_text}\nfallback_prepare_error:\n{}",
                retry_prepare_err.render()
            ))
        })?;
        let retry_launch_context_text = retry_launch_context.render();
        execute_env(retry_env, Self::stdout_stream(ctx))
            .await
            .map_err(|retry_err| {
                ToolError::Message(format!(
                    "apply_patch launch failed after fallback retry:\nprimary_launch_context:\n{primary_context_text}\nprimary_error={primary_error_text}\nfallback_launch_context:\n{retry_launch_context_text}\nfallback_source_error={retry_err:?}"
                ))
            })
    }

    fn map_launch_error(err: CodexErr, launch_context_text: &str) -> ToolError {
        match err {
            CodexErr::Sandbox(SandboxErr::Timeout { .. })
            | CodexErr::Sandbox(SandboxErr::Denied { .. }) => ToolError::Codex(err),
            other => ToolError::Message(format!(
                "apply_patch launch failed:\n{launch_context_text}\nsource_error={other:?}"
            )),
        }
    }

    fn stdout_stream(ctx: &ToolCtx) -> Option<crate::exec::StdoutStream> {
        Some(crate::exec::StdoutStream {
            sub_id: ctx.turn.sub_id.clone(),
            call_id: ctx.call_id.clone(),
            tx_event: ctx.session.get_tx_event(),
        })
    }
}

impl Sandboxable for ApplyPatchRuntime {
    fn sandbox_preference(&self) -> SandboxablePreference {
        SandboxablePreference::Auto
    }
    fn escalate_on_failure(&self) -> bool {
        true
    }
}

impl Approvable<ApplyPatchRequest> for ApplyPatchRuntime {
    type ApprovalKey = AbsolutePathBuf;

    fn approval_keys(&self, req: &ApplyPatchRequest) -> Vec<Self::ApprovalKey> {
        req.file_paths.clone()
    }

    fn start_approval_async<'a>(
        &'a mut self,
        req: &'a ApplyPatchRequest,
        ctx: ApprovalCtx<'a>,
    ) -> BoxFuture<'a, ReviewDecision> {
        let session = ctx.session;
        let turn = ctx.turn;
        let call_id = ctx.call_id.to_string();
        let retry_reason = ctx.retry_reason.clone();
        let approval_keys = self.approval_keys(req);
        let changes = req.changes.clone();
        Box::pin(async move {
            if req.permissions_preapproved && retry_reason.is_none() {
                return ReviewDecision::Approved;
            }
            if routes_approval_to_guardian(turn) {
                let action = ApplyPatchRuntime::build_guardian_review_request(req, ctx.call_id);
                return review_approval_request(session, turn, action, retry_reason).await;
            }
            if let Some(reason) = retry_reason {
                let rx_approve = session
                    .request_patch_approval(
                        turn,
                        call_id,
                        changes.clone(),
                        Some(reason),
                        /*grant_root*/ None,
                    )
                    .await;
                return rx_approve.await.unwrap_or_default();
            }

            with_cached_approval(
                &session.services,
                "apply_patch",
                approval_keys,
                || async move {
                    let rx_approve = session
                        .request_patch_approval(
                            turn, call_id, changes, /*reason*/ None, /*grant_root*/ None,
                        )
                        .await;
                    rx_approve.await.unwrap_or_default()
                },
            )
            .await
        })
    }

    fn wants_no_sandbox_approval(&self, policy: AskForApproval) -> bool {
        match policy {
            AskForApproval::Never => false,
            AskForApproval::Granular(granular_config) => granular_config.allows_sandbox_approval(),
            AskForApproval::OnFailure => true,
            AskForApproval::OnRequest => true,
            AskForApproval::UnlessTrusted => true,
        }
    }

    // apply_patch approvals are decided upstream by assess_patch_safety.
    //
    // This override ensures the orchestrator runs the patch approval flow when required instead
    // of falling back to the global exec approval policy.
    fn exec_approval_requirement(
        &self,
        req: &ApplyPatchRequest,
    ) -> Option<ExecApprovalRequirement> {
        Some(req.exec_approval_requirement.clone())
    }
}

impl ToolRuntime<ApplyPatchRequest, ExecToolCallOutput> for ApplyPatchRuntime {
    async fn run(
        &mut self,
        req: &ApplyPatchRequest,
        attempt: &SandboxAttempt<'_>,
        ctx: &ToolCtx,
    ) -> Result<ExecToolCallOutput, ToolError> {
        #[cfg(target_os = "windows")]
        let (env, launch_context) = {
            let (_command, program_source, pre_sandbox_program) =
                Self::build_sandbox_command(req, &ctx.turn.config.codex_home)?;
            Self::prepare_exec_request(
                req,
                attempt,
                program_source,
                pre_sandbox_program,
                Vec::new(),
            )
            .map_err(ApplyPatchPrepareError::into_tool_error)?
        };

        #[cfg(not(target_os = "windows"))]
        let (env, launch_context, retry_fallback, resolver_diagnostics) = {
            let prepared = Self::build_sandbox_command(req, &ctx.turn.self_exec_paths)?;
            match Self::prepare_exec_request(
                req,
                attempt,
                prepared.program_source,
                prepared.pre_sandbox_program.clone(),
                prepared.resolver_diagnostics.clone(),
            ) {
                Ok((env, launch_context)) => (
                    env,
                    launch_context,
                    prepared.retry_fallback.clone(),
                    prepared.resolver_diagnostics,
                ),
                Err(prepare_err) => {
                    if let Some(fallback) = prepared.retry_fallback
                        && let Some(primary_launch_context) =
                            prepare_err.retryable_executable_path_context()
                    {
                        return Self::execute_with_fallback_retry(
                            req,
                            attempt,
                            ctx,
                            fallback,
                            prepared.resolver_diagnostics,
                            &primary_launch_context.render(),
                            &prepare_err.render(),
                        )
                        .await;
                    }
                    return Err(prepare_err.into_tool_error());
                }
            }
        };

        let launch_context_text = launch_context.render();
        match execute_env(env, Self::stdout_stream(ctx)).await {
            Ok(out) => Ok(out),
            Err(err) => {
                #[cfg(not(target_os = "windows"))]
                {
                    if let Some(fallback) = retry_fallback
                        && Self::is_retryable_launch_error(&err, &launch_context)
                    {
                        return Self::execute_with_fallback_retry(
                            req,
                            attempt,
                            ctx,
                            fallback,
                            resolver_diagnostics,
                            &launch_context_text,
                            &format!("source_error={err:?}"),
                        )
                        .await;
                    }
                }
                Err(Self::map_launch_error(err, &launch_context_text))
            }
        }
    }
}

#[cfg(test)]
#[path = "apply_patch_tests.rs"]
mod tests;
