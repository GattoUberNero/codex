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
    ConfiguredCodexSelfExe,
    CurrentExe,
    #[cfg(target_os = "windows")]
    WindowsResolvedLaunchExe,
}

impl ApplyPatchProgramSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::ConfiguredCodexSelfExe => "codex_self_exe",
            Self::CurrentExe => "current_exe",
            #[cfg(target_os = "windows")]
            Self::WindowsResolvedLaunchExe => "windows_resolved_launch_exe",
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
    pre_sandbox_program: String,
    pre_sandbox_program_status: Option<ApplyPatchPathStatus>,
    pre_sandbox_cwd: ApplyPatchPathStatus,
    sandbox: SandboxType,
    final_program: String,
    final_program_status: Option<ApplyPatchPathStatus>,
}

impl ApplyPatchLaunchContext {
    fn new(
        program_source: ApplyPatchProgramSource,
        pre_sandbox_program: String,
        pre_sandbox_program_status: Option<ApplyPatchPathStatus>,
        pre_sandbox_cwd: ApplyPatchPathStatus,
        sandbox: SandboxType,
        final_program: String,
        final_program_status: Option<ApplyPatchPathStatus>,
    ) -> Self {
        Self {
            program_source,
            pre_sandbox_program,
            pre_sandbox_program_status,
            pre_sandbox_cwd,
            sandbox,
            final_program,
            final_program_status,
        }
    }

    fn from_exec_request(
        program_source: ApplyPatchProgramSource,
        pre_sandbox_program: &Path,
        pre_sandbox_cwd: &Path,
        exec_request: &crate::sandboxing::ExecRequest,
    ) -> Self {
        let final_program = exec_request.command.first().cloned().unwrap_or_default();
        let final_program_status = apply_patch_program_path(&final_program)
            .map(|path| ApplyPatchPathStatus::from_path(path.as_path()));

        Self::new(
            program_source,
            pre_sandbox_program.display().to_string(),
            apply_patch_program_path(pre_sandbox_program.to_string_lossy().as_ref())
                .map(|path| ApplyPatchPathStatus::from_path(path.as_path())),
            ApplyPatchPathStatus::from_path(pre_sandbox_cwd),
            exec_request.sandbox,
            final_program,
            final_program_status,
        )
    }

    fn has_preflight_problem(&self) -> bool {
        self.pre_sandbox_program_status
            .as_ref()
            .is_some_and(ApplyPatchPathStatus::missing_file)
            || self.pre_sandbox_cwd.missing_dir()
            || self
                .final_program_status
                .as_ref()
                .is_some_and(ApplyPatchPathStatus::missing_file)
    }

    fn render(&self) -> String {
        let mut lines = vec![
            format!("program_source={}", self.program_source.as_str()),
            if let Some(status) = &self.pre_sandbox_program_status {
                status.render("pre_sandbox_program")
            } else {
                format!(
                    "pre_sandbox_program={} metadata_error=not_checked",
                    self.pre_sandbox_program
                )
            },
            self.pre_sandbox_cwd.render("pre_sandbox_cwd"),
            format!("sandbox={:?}", self.sandbox),
        ];
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
        req: &ApplyPatchRequest,
        codex_self_exe: Option<&PathBuf>,
    ) -> Result<(SandboxCommand, ApplyPatchProgramSource, PathBuf), ToolError> {
        let (exe, source) = Self::resolve_apply_patch_program(codex_self_exe)?;
        Ok((
            Self::build_sandbox_command_with_program(req, exe.clone()),
            source,
            exe,
        ))
    }

    #[cfg(not(target_os = "windows"))]
    fn resolve_apply_patch_program(
        codex_self_exe: Option<&PathBuf>,
    ) -> Result<(PathBuf, ApplyPatchProgramSource), ToolError> {
        if let Some(path) = codex_self_exe {
            return Ok((
                path.clone(),
                ApplyPatchProgramSource::ConfiguredCodexSelfExe,
            ));
        }

        std::env::current_exe()
            .map(|path| (path, ApplyPatchProgramSource::CurrentExe))
            .map_err(|e| ToolError::Message(format!("failed to determine codex exe: {e}")))
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
        let (command, program_source, pre_sandbox_program) =
            Self::build_sandbox_command(req, &ctx.turn.config.codex_home)?;
        #[cfg(not(target_os = "windows"))]
        let (command, program_source, pre_sandbox_program) =
            Self::build_sandbox_command(req, ctx.turn.codex_self_exe.as_ref())?;
        let pre_sandbox_program_text = pre_sandbox_program.display().to_string();
        let pre_sandbox_program_status =
            apply_patch_program_path(pre_sandbox_program.to_string_lossy().as_ref())
                .map(|path| ApplyPatchPathStatus::from_path(path.as_path()));
        let pre_sandbox_cwd_status = ApplyPatchPathStatus::from_path(req.action.cwd.as_path());
        if pre_sandbox_program_status
            .as_ref()
            .is_some_and(ApplyPatchPathStatus::missing_file)
            || pre_sandbox_cwd_status.missing_dir()
        {
            let launch_context = ApplyPatchLaunchContext::new(
                program_source,
                pre_sandbox_program_text,
                pre_sandbox_program_status,
                pre_sandbox_cwd_status,
                attempt.sandbox,
                pre_sandbox_program.display().to_string(),
                apply_patch_program_path(pre_sandbox_program.to_string_lossy().as_ref())
                    .map(|path| ApplyPatchPathStatus::from_path(path.as_path())),
            );
            return Err(ToolError::Message(format!(
                "apply_patch launch preflight failed:\n{}",
                launch_context.render()
            )));
        }

        let options = ExecOptions {
            expiration: req.timeout_ms.into(),
            capture_policy: ExecCapturePolicy::ShellTool,
        };
        let env = attempt
            .env_for(command, options, /*network*/ None)
            .map_err(|err| {
                ToolError::Message(format!(
                    "apply_patch launch preparation failed:\nprogram_source={}\npre_sandbox_program={}\npre_sandbox_cwd={}\nsandbox={:?}\ntransform_error={err}",
                    program_source.as_str(),
                    pre_sandbox_program.display(),
                    req.action.cwd.display(),
                    attempt.sandbox,
                ))
            })?;
        let launch_context = ApplyPatchLaunchContext::from_exec_request(
            program_source,
            pre_sandbox_program.as_path(),
            req.action.cwd.as_path(),
            &env,
        );
        if launch_context.has_preflight_problem() {
            return Err(ToolError::Message(format!(
                "apply_patch launch preflight failed:\n{}",
                launch_context.render()
            )));
        }
        let launch_context_text = launch_context.render();
        let out = execute_env(env, Self::stdout_stream(ctx))
            .await
            .map_err(|err| match err {
                CodexErr::Sandbox(SandboxErr::Timeout { .. })
                | CodexErr::Sandbox(SandboxErr::Denied { .. }) => ToolError::Codex(err),
                other => ToolError::Message(format!(
                    "apply_patch launch failed:\n{launch_context_text}\nsource_error={other:?}"
                )),
            })?;
        Ok(out)
    }
}

#[cfg(test)]
#[path = "apply_patch_tests.rs"]
mod tests;
