use assert_cmd::Command;
use pretty_assertions::assert_eq;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use tempfile::tempdir;

fn run_apply_patch_in_dir(dir: &Path, patch: &str) -> anyhow::Result<assert_cmd::assert::Assert> {
    let mut cmd = Command::new(codex_utils_cargo_bin::cargo_bin("apply_patch")?);
    cmd.current_dir(dir);
    Ok(cmd.arg(patch).assert())
}

fn apply_patch_command(dir: &Path) -> anyhow::Result<Command> {
    let mut cmd = Command::new(codex_utils_cargo_bin::cargo_bin("apply_patch")?);
    cmd.current_dir(dir);
    Ok(cmd)
}

#[test]
fn test_apply_patch_cli_applies_multiple_operations() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let modify_path = tmp.path().join("modify.txt");
    let delete_path = tmp.path().join("delete.txt");

    fs::write(&modify_path, "line1\nline2\n")?;
    fs::write(&delete_path, "obsolete\n")?;

    let patch = "*** Begin Patch\n*** Add File: nested/new.txt\n+created\n*** Delete File: delete.txt\n*** Update File: modify.txt\n@@\n-line2\n+changed\n*** End Patch";

    run_apply_patch_in_dir(tmp.path(), patch)?.success().stdout(
        "Success. Updated the following files:\nA nested/new.txt\nM modify.txt\nD delete.txt\n",
    );

    assert_eq!(
        fs::read_to_string(tmp.path().join("nested/new.txt"))?,
        "created\n"
    );
    assert_eq!(fs::read_to_string(&modify_path)?, "line1\nchanged\n");
    assert!(!delete_path.exists());

    Ok(())
}

#[test]
fn test_apply_patch_cli_applies_multiple_chunks() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let target_path = tmp.path().join("multi.txt");
    fs::write(&target_path, "line1\nline2\nline3\nline4\n")?;

    let patch = "*** Begin Patch\n*** Update File: multi.txt\n@@\n-line2\n+changed2\n@@\n-line4\n+changed4\n*** End Patch";

    run_apply_patch_in_dir(tmp.path(), patch)?
        .success()
        .stdout("Success. Updated the following files:\nM multi.txt\n");

    assert_eq!(
        fs::read_to_string(&target_path)?,
        "line1\nchanged2\nline3\nchanged4\n"
    );

    Ok(())
}

#[test]
fn test_apply_patch_cli_applies_add_then_update_to_same_path() -> anyhow::Result<()> {
    let tmp = tempdir()?;

    run_apply_patch_in_dir(
        tmp.path(),
        "*** Begin Patch\n*** Add File: nested/generated.txt\n+line1\n*** Update File: nested/generated.txt\n@@\n-line1\n+line2\n*** End Patch",
    )?
    .success()
    .stdout("Success. Updated the following files:\nA nested/generated.txt\n");

    assert_eq!(
        fs::read_to_string(tmp.path().join("nested/generated.txt"))?,
        "line2\n"
    );

    Ok(())
}

#[test]
fn test_apply_patch_cli_applies_multiple_updates_to_same_path() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let target_path = tmp.path().join("multi-update.txt");
    fs::write(&target_path, "alpha\nbeta\ngamma\n")?;

    run_apply_patch_in_dir(
        tmp.path(),
        "*** Begin Patch\n*** Update File: multi-update.txt\n@@\n-beta\n+delta\n*** Update File: multi-update.txt\n@@\n-gamma\n+omega\n*** End Patch",
    )?
    .success()
    .stdout("Success. Updated the following files:\nM multi-update.txt\n");

    assert_eq!(fs::read_to_string(&target_path)?, "alpha\ndelta\nomega\n");

    Ok(())
}

#[test]
fn test_apply_patch_cli_moves_file_to_new_directory() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let original_path = tmp.path().join("old/name.txt");
    let new_path = tmp.path().join("renamed/dir/name.txt");
    fs::create_dir_all(original_path.parent().expect("parent should exist"))?;
    fs::write(&original_path, "old content\n")?;

    let patch = "*** Begin Patch\n*** Update File: old/name.txt\n*** Move to: renamed/dir/name.txt\n@@\n-old content\n+new content\n*** End Patch";

    run_apply_patch_in_dir(tmp.path(), patch)?
        .success()
        .stdout("Success. Updated the following files:\nM renamed/dir/name.txt\n");

    assert!(!original_path.exists());
    assert_eq!(fs::read_to_string(&new_path)?, "new content\n");

    Ok(())
}

#[test]
fn test_apply_patch_cli_rejects_empty_patch() -> anyhow::Result<()> {
    let tmp = tempdir()?;

    apply_patch_command(tmp.path())?
        .arg("*** Begin Patch\n*** End Patch")
        .assert()
        .failure()
        .stderr("No files were modified.\n");

    Ok(())
}

#[test]
fn test_apply_patch_cli_reports_missing_context() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let target_path = tmp.path().join("modify.txt");
    fs::write(&target_path, "line1\nline2\n")?;

    let output = apply_patch_command(tmp.path())?
        .arg("*** Begin Patch\n*** Update File: modify.txt\n@@\n-missing\n+changed\n*** End Patch")
        .output()?;
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(
        stderr,
        format!(
            "Failed to find expected lines in modify.txt (cwd: {}, resolved: {}):\nmissing\n",
            tmp.path().display(),
            tmp.path().join("modify.txt").display()
        )
    );
    assert_eq!(fs::read_to_string(&target_path)?, "line1\nline2\n");

    Ok(())
}

#[test]
fn test_apply_patch_cli_rejects_missing_file_delete() -> anyhow::Result<()> {
    let tmp = tempdir()?;

    let output = apply_patch_command(tmp.path())?
        .arg("*** Begin Patch\n*** Delete File: missing.txt\n*** End Patch")
        .output()?;
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(
        stderr,
        format!(
            "Failed to delete file missing.txt (cwd: {}, resolved: {}): No such file or directory (os error 2)\n",
            tmp.path().display(),
            tmp.path().join("missing.txt").display()
        )
    );

    Ok(())
}

#[test]
fn test_apply_patch_cli_rejects_empty_update_hunk() -> anyhow::Result<()> {
    let tmp = tempdir()?;

    apply_patch_command(tmp.path())?
        .arg("*** Begin Patch\n*** Update File: foo.txt\n*** End Patch")
        .assert()
        .failure()
        .stderr("Invalid patch hunk on line 2: Update file hunk for path 'foo.txt' is empty\n");

    Ok(())
}

#[test]
fn test_apply_patch_cli_requires_existing_file_for_update() -> anyhow::Result<()> {
    let tmp = tempdir()?;

    let output = apply_patch_command(tmp.path())?
        .arg("*** Begin Patch\n*** Update File: missing.txt\n@@\n-old\n+new\n*** End Patch")
        .output()?;
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(
        stderr,
        format!(
            "Failed to read file to update missing.txt (cwd: {}, resolved: {}): No such file or directory (os error 2)\n",
            tmp.path().display(),
            tmp.path().join("missing.txt").display()
        )
    );

    Ok(())
}

#[test]
fn test_apply_patch_cli_move_overwrites_existing_destination() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let original_path = tmp.path().join("old/name.txt");
    let destination = tmp.path().join("renamed/dir/name.txt");
    fs::create_dir_all(original_path.parent().expect("parent should exist"))?;
    fs::create_dir_all(destination.parent().expect("parent should exist"))?;
    fs::write(&original_path, "from\n")?;
    fs::write(&destination, "existing\n")?;

    run_apply_patch_in_dir(
        tmp.path(),
        "*** Begin Patch\n*** Update File: old/name.txt\n*** Move to: renamed/dir/name.txt\n@@\n-from\n+new\n*** End Patch",
    )?
    .success()
    .stdout("Success. Updated the following files:\nM renamed/dir/name.txt\n");

    assert!(!original_path.exists());
    assert_eq!(fs::read_to_string(&destination)?, "new\n");

    Ok(())
}

#[test]
fn test_apply_patch_cli_move_to_same_normalized_path_updates_in_place() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let path = tmp.path().join("same.txt");
    fs::write(&path, "before\n")?;

    run_apply_patch_in_dir(
        tmp.path(),
        "*** Begin Patch\n*** Update File: same.txt\n*** Move to: ./same.txt\n@@\n-before\n+after\n*** End Patch",
    )?
    .success()
    .stdout("Success. Updated the following files:\nM same.txt\n");

    assert_eq!(fs::read_to_string(&path)?, "after\n");

    Ok(())
}
#[test]
fn test_apply_patch_cli_move_from_same_normalized_path_updates_in_place() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let path = tmp.path().join("same.txt");
    fs::write(&path, "before\n")?;

    run_apply_patch_in_dir(
        tmp.path(),
        "*** Begin Patch\n*** Update File: ./same.txt\n*** Move to: same.txt\n@@\n-before\n+after\n*** End Patch",
    )?
    .success()
    .stdout("Success. Updated the following files:\nM ./same.txt\n");

    assert_eq!(fs::read_to_string(&path)?, "after\n");

    Ok(())
}

#[test]
fn test_apply_patch_cli_add_overwrites_existing_file() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let path = tmp.path().join("duplicate.txt");
    fs::write(&path, "old content\n")?;

    run_apply_patch_in_dir(
        tmp.path(),
        "*** Begin Patch\n*** Add File: duplicate.txt\n+new content\n*** End Patch",
    )?
    .success()
    .stdout("Success. Updated the following files:\nA duplicate.txt\n");

    assert_eq!(fs::read_to_string(&path)?, "new content\n");

    Ok(())
}

#[test]
fn test_apply_patch_cli_delete_directory_fails() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    fs::create_dir(tmp.path().join("dir"))?;

    let output = apply_patch_command(tmp.path())?
        .arg("*** Begin Patch\n*** Delete File: dir\n*** End Patch")
        .output()?;
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(
        stderr,
        format!(
            "Failed to delete file dir (cwd: {}, resolved: {}): target is not a file\n",
            tmp.path().display(),
            tmp.path().join("dir").display()
        )
    );

    Ok(())
}

#[test]
fn test_apply_patch_cli_rejects_invalid_hunk_header() -> anyhow::Result<()> {
    let tmp = tempdir()?;

    apply_patch_command(tmp.path())?
        .arg("*** Begin Patch\n*** Frobnicate File: foo\n*** End Patch")
        .assert()
        .failure()
        .stderr("Invalid patch hunk on line 2: '*** Frobnicate File: foo' is not a valid hunk header. Valid hunk headers: '*** Add File: {path}', '*** Delete File: {path}', '*** Update File: {path}'\n");

    Ok(())
}

#[test]
fn test_apply_patch_cli_updates_file_appends_trailing_newline() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let target_path = tmp.path().join("no_newline.txt");
    fs::write(&target_path, "no newline at end")?;

    run_apply_patch_in_dir(
        tmp.path(),
        "*** Begin Patch\n*** Update File: no_newline.txt\n@@\n-no newline at end\n+first line\n+second line\n*** End Patch",
    )?
    .success()
    .stdout("Success. Updated the following files:\nM no_newline.txt\n");

    let contents = fs::read_to_string(&target_path)?;
    assert!(contents.ends_with('\n'));
    assert_eq!(contents, "first line\nsecond line\n");

    Ok(())
}

#[test]
fn test_apply_patch_cli_failure_before_writes_leaves_filesystem_unchanged() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let new_file = tmp.path().join("created.txt");

    let output = apply_patch_command(tmp.path())?
        .arg("*** Begin Patch\n*** Add File: created.txt\n+hello\n*** Update File: missing.txt\n@@\n-old\n+new\n*** End Patch")
        .output()?;
    assert!(!output.status.success());
    assert_eq!(String::from_utf8(output.stdout)?, "");
    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(
        stderr,
        format!(
            "Failed to read file to update missing.txt (cwd: {}, resolved: {}): No such file or directory (os error 2)\n",
            tmp.path().display(),
            tmp.path().join("missing.txt").display()
        )
    );

    assert!(!new_file.exists());

    Ok(())
}

#[test]
fn test_apply_patch_cli_rolls_back_after_commit_failure() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let created_file = tmp.path().join("created.txt");
    let blocking_file = tmp.path().join("blocking");
    fs::write(&blocking_file, "not a directory\n")?;

    let output = apply_patch_command(tmp.path())?
        .arg("*** Begin Patch\n*** Add File: created.txt\n+hello\n*** Add File: blocking/child.txt\n+world\n*** End Patch")
        .output()?;
    assert!(!output.status.success());

    assert!(!created_file.exists());
    assert_eq!(fs::read_to_string(&blocking_file)?, "not a directory\n");

    Ok(())
}

#[cfg(unix)]
#[test]
fn test_apply_patch_cli_deletes_dangling_symlink() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let link_path = tmp.path().join("dangling-link");
    std::os::unix::fs::symlink("missing-target", &link_path)?;

    run_apply_patch_in_dir(
        tmp.path(),
        "*** Begin Patch\n*** Delete File: dangling-link\n*** End Patch",
    )?
    .success()
    .stdout("Success. Updated the following files:\nD dangling-link\n");

    assert!(!link_path.exists());
    assert!(std::fs::symlink_metadata(&link_path).is_err());

    Ok(())
}

#[cfg(unix)]
#[test]
fn test_apply_patch_cli_deletes_live_symlink_without_touching_target() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let target_path = tmp.path().join("target.txt");
    fs::write(&target_path, "target contents\n")?;
    let link_path = tmp.path().join("live-link");
    std::os::unix::fs::symlink("target.txt", &link_path)?;

    run_apply_patch_in_dir(
        tmp.path(),
        "*** Begin Patch\n*** Delete File: live-link\n*** End Patch",
    )?
    .success()
    .stdout("Success. Updated the following files:\nD live-link\n");

    assert_eq!(fs::read_to_string(&target_path)?, "target contents\n");
    assert!(std::fs::symlink_metadata(&link_path).is_err());

    Ok(())
}

#[cfg(unix)]
#[test]
fn test_apply_patch_cli_rolls_back_symlink_target_after_later_failure() -> anyhow::Result<()> {
    let tmp = tempdir()?;
    let target_path = tmp.path().join("target.txt");
    fs::write(&target_path, "original\n")?;
    let link_path = tmp.path().join("live-link");
    std::os::unix::fs::symlink("target.txt", &link_path)?;

    let output = apply_patch_command(tmp.path())?
        .arg(
            "*** Begin Patch\n*** Update File: live-link\n@@\n-original\n+changed\n*** Delete File: missing.txt\n*** End Patch",
        )
        .output()?;
    assert!(!output.status.success());

    assert_eq!(fs::read_to_string(&target_path)?, "original\n");
    assert!(
        std::fs::symlink_metadata(&link_path)?
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read_link(&link_path)?, PathBuf::from("target.txt"));

    Ok(())
}
