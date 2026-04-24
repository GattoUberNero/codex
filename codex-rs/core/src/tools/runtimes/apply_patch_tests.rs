use super::*;
use codex_nero_self_exec::SelfExecPaths;
use codex_protocol::protocol::GranularApprovalConfig;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
#[cfg(not(target_os = "windows"))]
use std::path::Path;
#[cfg(not(target_os = "windows"))]
use std::path::PathBuf;

#[test]
fn wants_no_sandbox_approval_granular_respects_sandbox_flag() {
    let runtime = ApplyPatchRuntime::new();
    assert!(runtime.wants_no_sandbox_approval(AskForApproval::OnRequest));
    assert!(
        !runtime.wants_no_sandbox_approval(AskForApproval::Granular(GranularApprovalConfig {
            sandbox_approval: false,
            rules: true,
            skill_approval: true,
            request_permissions: true,
            mcp_elicitations: true,
        }))
    );
    assert!(
        runtime.wants_no_sandbox_approval(AskForApproval::Granular(GranularApprovalConfig {
            sandbox_approval: true,
            rules: true,
            skill_approval: true,
            request_permissions: true,
            mcp_elicitations: true,
        }))
    );
}

#[test]
fn guardian_review_request_includes_patch_context() {
    let path = std::env::temp_dir().join("guardian-apply-patch-test.txt");
    let action = ApplyPatchAction::new_add_for_test(&path, "hello".to_string());
    let expected_cwd = action.cwd.clone();
    let expected_patch = action.patch.clone();
    let request = ApplyPatchRequest {
        action,
        file_paths: vec![
            AbsolutePathBuf::from_absolute_path(&path).expect("temp path should be absolute"),
        ],
        changes: HashMap::from([(
            path,
            FileChange::Add {
                content: "hello".to_string(),
            },
        )]),
        exec_approval_requirement: ExecApprovalRequirement::NeedsApproval {
            reason: None,
            proposed_execpolicy_amendment: None,
        },
        additional_permissions: None,
        permissions_preapproved: false,
        timeout_ms: None,
    };

    let guardian_request = ApplyPatchRuntime::build_guardian_review_request(&request, "call-1");

    assert_eq!(
        guardian_request,
        GuardianApprovalRequest::ApplyPatch {
            id: "call-1".to_string(),
            cwd: expected_cwd,
            files: request.file_paths,
            change_count: 1usize,
            patch: expected_patch,
        }
    );
}

#[cfg(not(target_os = "windows"))]
fn sample_request() -> ApplyPatchRequest {
    let path = std::env::temp_dir().join("apply-patch-current-exe-test.txt");
    let action = ApplyPatchAction::new_add_for_test(&path, "hello".to_string());
    ApplyPatchRequest {
        action,
        file_paths: vec![
            AbsolutePathBuf::from_absolute_path(&path).expect("temp path should be absolute"),
        ],
        changes: HashMap::from([(
            path,
            FileChange::Add {
                content: "hello".to_string(),
            },
        )]),
        exec_approval_requirement: ExecApprovalRequirement::NeedsApproval {
            reason: None,
            proposed_execpolicy_amendment: None,
        },
        additional_permissions: None,
        permissions_preapproved: false,
        timeout_ms: None,
    }
}

fn launch_context_for_test(
    program_source: ApplyPatchProgramSource,
    pre_sandbox_program: &str,
    pre_sandbox_cwd: &std::path::Path,
    final_program: &str,
) -> ApplyPatchLaunchContext {
    ApplyPatchLaunchContext {
        program_source,
        resolver_diagnostics: Vec::new(),
        pre_sandbox_program: pre_sandbox_program.to_string(),
        pre_sandbox_program_status: apply_patch_program_path(pre_sandbox_program)
            .map(|path| ApplyPatchPathStatus::from_path(path.as_path())),
        pre_sandbox_cwd: ApplyPatchPathStatus::from_path(pre_sandbox_cwd),
        sandbox: SandboxType::None,
        final_program: final_program.to_string(),
        final_program_status: apply_patch_program_path(final_program)
            .map(|path| ApplyPatchPathStatus::from_path(path.as_path())),
    }
}

#[cfg(not(target_os = "windows"))]
#[test]
fn build_sandbox_command_prefers_configured_self_exec_primary_for_apply_patch() {
    let request = sample_request();
    let tempdir = tempfile::tempdir().expect("tempdir");
    let primary = tempdir.path().join("codex-primary");
    std::fs::write(&primary, "binary").expect("write primary");

    let prepared = ApplyPatchRuntime::build_sandbox_command(
        &request,
        &SelfExecPaths {
            primary: Some(primary.clone()),
            fallback: None,
        },
    )
    .expect("build sandbox command");

    assert_eq!(
        prepared.program_source,
        ApplyPatchProgramSource::ConfiguredSelfExecPrimary
    );
    assert_eq!(prepared.pre_sandbox_program, primary);
    assert_eq!(prepared.retry_fallback, None);
}

#[cfg(not(target_os = "windows"))]
#[test]
fn build_sandbox_command_uses_configured_fallback_when_primary_is_missing() {
    let request = sample_request();
    let tempdir = tempfile::tempdir().expect("tempdir");
    let fallback = tempdir.path().join("codex-fallback");
    std::fs::write(&fallback, "binary").expect("write fallback");

    let prepared = ApplyPatchRuntime::build_sandbox_command(
        &request,
        &SelfExecPaths {
            primary: Some(tempdir.path().join("missing-primary")),
            fallback: Some(fallback.clone()),
        },
    )
    .expect("build sandbox command");

    assert_eq!(
        prepared.program_source,
        ApplyPatchProgramSource::ConfiguredSelfExecFallback
    );
    assert_eq!(prepared.pre_sandbox_program, fallback);
    assert_eq!(prepared.retry_fallback, None);
}

#[cfg(not(target_os = "windows"))]
#[test]
fn build_sandbox_command_falls_back_to_current_exe_for_apply_patch() {
    let request = sample_request();

    let prepared = ApplyPatchRuntime::build_sandbox_command(&request, &SelfExecPaths::default())
        .expect("build sandbox command");

    assert_eq!(prepared.program_source, ApplyPatchProgramSource::CurrentExe);
    assert_eq!(
        prepared.pre_sandbox_program,
        std::env::current_exe().expect("current exe")
    );
    assert_eq!(prepared.retry_fallback, None);
}

#[cfg(not(target_os = "windows"))]
#[test]
fn retryable_launch_error_requires_primary_io_error() {
    let cwd = tempfile::tempdir().expect("tempdir");
    let primary_context = launch_context_for_test(
        ApplyPatchProgramSource::ConfiguredSelfExecPrimary,
        "codex",
        cwd.path(),
        "codex",
    );
    let fallback_context = launch_context_for_test(
        ApplyPatchProgramSource::ConfiguredSelfExecFallback,
        "codex",
        cwd.path(),
        "codex",
    );
    let not_found = CodexErr::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));
    let permission_denied = CodexErr::Io(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "denied",
    ));
    let exec_format = CodexErr::Io(std::io::Error::from_raw_os_error(libc::ENOEXEC));
    let other_io = CodexErr::Io(std::io::Error::other("other"));

    assert!(ApplyPatchRuntime::is_retryable_launch_error(
        &not_found,
        &primary_context
    ));
    assert!(ApplyPatchRuntime::is_retryable_launch_error(
        &permission_denied,
        &primary_context
    ));
    assert!(ApplyPatchRuntime::is_retryable_launch_error(
        &exec_format,
        &primary_context
    ));
    assert!(!ApplyPatchRuntime::is_retryable_launch_error(
        &other_io,
        &primary_context
    ));
    assert!(!ApplyPatchRuntime::is_retryable_launch_error(
        &not_found,
        &fallback_context
    ));
}

#[test]
fn launch_context_skips_non_pathlike_pre_sandbox_program() {
    let cwd = tempfile::tempdir().expect("tempdir");
    let context = launch_context_for_test(
        ApplyPatchProgramSource::CurrentExe,
        "codex.exe",
        cwd.path(),
        "codex.exe",
    );

    assert!(!context.has_preflight_problem());
    let rendered = context.render();
    assert!(rendered.contains("pre_sandbox_program=codex.exe metadata_error=not_checked"));
}

#[test]
fn apply_patch_program_path_treats_bare_names_as_non_pathlike() {
    assert_eq!(apply_patch_program_path("codex.exe"), None);
}

#[cfg(not(target_os = "windows"))]
#[test]
fn apply_patch_program_path_treats_absolute_unix_paths_as_pathlike() {
    assert_eq!(
        apply_patch_program_path("/tmp/codex"),
        Some(PathBuf::from("/tmp/codex"))
    );
}

#[cfg(target_os = "windows")]
#[test]
fn apply_patch_program_path_treats_windows_separator_paths_as_pathlike() {
    assert_eq!(
        apply_patch_program_path(r".\\codex.exe"),
        Some(PathBuf::from(r".\\codex.exe"))
    );
}

#[cfg(not(target_os = "windows"))]
#[test]
fn launch_context_detects_missing_program_or_cwd() {
    let context = launch_context_for_test(
        ApplyPatchProgramSource::ConfiguredSelfExecPrimary,
        "/missing/pre-sandbox-program",
        Path::new("/missing/pre-sandbox-cwd"),
        "/missing/final-program",
    );

    assert!(context.has_preflight_problem());
    let rendered = context.render();
    assert!(rendered.contains("program_source=configured_self_exec_primary"));
    assert!(rendered.contains("pre_sandbox_program=/missing/pre-sandbox-program"));
    assert!(rendered.contains("pre_sandbox_cwd=/missing/pre-sandbox-cwd"));
    assert!(rendered.contains("final_program=/missing/final-program"));
    assert!(rendered.contains("sandbox=None"));
}
