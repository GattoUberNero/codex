use super::*;
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
#[test]
fn build_sandbox_command_prefers_configured_codex_self_exe_for_apply_patch() {
    let path = std::env::temp_dir().join("apply-patch-current-exe-test.txt");
    let action = ApplyPatchAction::new_add_for_test(&path, "hello".to_string());
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
    let codex_self_exe = PathBuf::from("/tmp/codex");

    let (command, source, pre_sandbox_program) =
        ApplyPatchRuntime::build_sandbox_command(&request, Some(&codex_self_exe))
            .expect("build sandbox command");

    assert_eq!(source, ApplyPatchProgramSource::ConfiguredCodexSelfExe);
    assert_eq!(pre_sandbox_program, codex_self_exe);
    assert_eq!(command.program, pre_sandbox_program.into_os_string());
}

#[cfg(not(target_os = "windows"))]
#[test]
fn build_sandbox_command_falls_back_to_current_exe_for_apply_patch() {
    let path = std::env::temp_dir().join("apply-patch-current-exe-test.txt");
    let action = ApplyPatchAction::new_add_for_test(&path, "hello".to_string());
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

    let (command, source, pre_sandbox_program) =
        ApplyPatchRuntime::build_sandbox_command(&request, /*codex_self_exe*/ None)
            .expect("build sandbox command");

    assert_eq!(source, ApplyPatchProgramSource::CurrentExe);
    assert_eq!(
        command.program,
        std::env::current_exe()
            .expect("current exe")
            .into_os_string()
    );
    assert_eq!(pre_sandbox_program, PathBuf::from(command.program));
}

#[test]
fn launch_context_skips_non_pathlike_pre_sandbox_program() {
    let cwd = tempfile::tempdir().expect("tempdir");
    let context = ApplyPatchLaunchContext::new(
        ApplyPatchProgramSource::CurrentExe,
        "codex.exe".to_string(),
        None,
        ApplyPatchPathStatus::from_path(cwd.path()),
        SandboxType::None,
        "codex.exe".to_string(),
        None,
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
    let context = ApplyPatchLaunchContext::new(
        ApplyPatchProgramSource::ConfiguredCodexSelfExe,
        "/missing/pre-sandbox-program".to_string(),
        Some(ApplyPatchPathStatus::from_path(Path::new(
            "/missing/pre-sandbox-program",
        ))),
        ApplyPatchPathStatus::from_path(Path::new("/missing/pre-sandbox-cwd")),
        SandboxType::None,
        "/missing/final-program".to_string(),
        Some(ApplyPatchPathStatus::from_path(Path::new(
            "/missing/final-program",
        ))),
    );

    assert!(context.has_preflight_problem());
    let rendered = context.render();
    assert!(rendered.contains("program_source=codex_self_exe"));
    assert!(rendered.contains("pre_sandbox_program=/missing/pre-sandbox-program"));
    assert!(rendered.contains("pre_sandbox_cwd=/missing/pre-sandbox-cwd"));
    assert!(rendered.contains("final_program=/missing/final-program"));
    assert!(rendered.contains("sandbox=None"));
}
