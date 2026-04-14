use std::path::Path;
use std::sync::LazyLock;

use tree_sitter::Parser;
use tree_sitter::Query;
use tree_sitter::QueryCursor;
use tree_sitter::StreamingIterator;
use tree_sitter_bash::LANGUAGE as BASH;

use crate::ApplyPatchAction;
use crate::ApplyPatchArgs;
use crate::ApplyPatchError;
use crate::MaybeApplyPatchVerified;
use crate::execution::preview_file_changes;
#[cfg(test)]
use crate::parser::Hunk;
use crate::parser::ParseError;
use crate::parser::parse_patch;
use crate::resolved::resolve_base_dir;
use crate::resolved::resolve_hunks;
use std::str::Utf8Error;
use tree_sitter::LanguageError;

const APPLY_PATCH_COMMANDS: [&str; 2] = ["apply_patch", "applypatch"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApplyPatchShell {
    Unix,
    PowerShell,
    Cmd,
}

#[derive(Debug, PartialEq)]
pub enum MaybeApplyPatch {
    Body(ApplyPatchArgs),
    ShellParseError(ExtractHeredocError),
    PatchParseError(ParseError),
    NotApplyPatch,
}

#[derive(Debug, PartialEq)]
pub enum ExtractHeredocError {
    CommandDidNotStartWithApplyPatch,
    FailedToLoadBashGrammar(LanguageError),
    HeredocNotUtf8(Utf8Error),
    FailedToParsePatchIntoAst,
    FailedToFindHeredocBody,
}

fn classify_shell_name(shell: &str) -> Option<String> {
    std::path::Path::new(shell)
        .file_stem()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase)
}

fn classify_shell(shell: &str, flag: &str) -> Option<ApplyPatchShell> {
    classify_shell_name(shell).and_then(|name| match name.as_str() {
        "bash" | "zsh" | "sh" if matches!(flag, "-lc" | "-c") => Some(ApplyPatchShell::Unix),
        "pwsh" | "powershell" if flag.eq_ignore_ascii_case("-command") => {
            Some(ApplyPatchShell::PowerShell)
        }
        "cmd" if flag.eq_ignore_ascii_case("/c") => Some(ApplyPatchShell::Cmd),
        _ => None,
    })
}

fn can_skip_flag(shell: &str, flag: &str) -> bool {
    classify_shell_name(shell).is_some_and(|name| {
        matches!(name.as_str(), "pwsh" | "powershell") && flag.eq_ignore_ascii_case("-noprofile")
    })
}

fn parse_shell_script(argv: &[String]) -> Option<(ApplyPatchShell, &str)> {
    match argv {
        [shell, flag, script] => classify_shell(shell, flag).map(|shell_type| {
            let script = script.as_str();
            (shell_type, script)
        }),
        [shell, skip_flag, flag, script] if can_skip_flag(shell, skip_flag) => {
            classify_shell(shell, flag).map(|shell_type| {
                let script = script.as_str();
                (shell_type, script)
            })
        }
        _ => None,
    }
}

fn extract_apply_patch_from_shell(
    shell: ApplyPatchShell,
    script: &str,
) -> std::result::Result<(String, Option<String>), ExtractHeredocError> {
    match shell {
        ApplyPatchShell::Unix | ApplyPatchShell::PowerShell | ApplyPatchShell::Cmd => {
            extract_apply_patch_from_bash(script)
        }
    }
}

// TODO: make private once we remove tests in lib.rs
pub fn maybe_parse_apply_patch(argv: &[String]) -> MaybeApplyPatch {
    match argv {
        // Direct invocation: apply_patch <patch>
        [cmd, body] if APPLY_PATCH_COMMANDS.contains(&cmd.as_str()) => match parse_patch(body) {
            Ok(source) => MaybeApplyPatch::Body(source),
            Err(e) => MaybeApplyPatch::PatchParseError(e),
        },
        // Shell heredoc form: (optional `cd <path> &&`) apply_patch <<'EOF' ...
        _ => match parse_shell_script(argv) {
            Some((shell, script)) => match extract_apply_patch_from_shell(shell, script) {
                Ok((body, workdir)) => match parse_patch(&body) {
                    Ok(mut source) => {
                        source.workdir = workdir;
                        MaybeApplyPatch::Body(source)
                    }
                    Err(e) => MaybeApplyPatch::PatchParseError(e),
                },
                Err(ExtractHeredocError::CommandDidNotStartWithApplyPatch) => {
                    MaybeApplyPatch::NotApplyPatch
                }
                Err(e) => MaybeApplyPatch::ShellParseError(e),
            },
            None => MaybeApplyPatch::NotApplyPatch,
        },
    }
}

/// cwd must be an absolute path so that we can resolve relative paths in the
/// patch.
pub fn maybe_parse_apply_patch_verified(argv: &[String], cwd: &Path) -> MaybeApplyPatchVerified {
    // Detect a raw patch body passed directly as the command or as the body of a shell
    // script. In these cases, report an explicit error rather than applying the patch.
    if let [body] = argv
        && parse_patch(body).is_ok()
    {
        return MaybeApplyPatchVerified::CorrectnessError(ApplyPatchError::ImplicitInvocation);
    }
    if let Some((_, script)) = parse_shell_script(argv)
        && parse_patch(script).is_ok()
    {
        return MaybeApplyPatchVerified::CorrectnessError(ApplyPatchError::ImplicitInvocation);
    }

    match maybe_parse_apply_patch(argv) {
        MaybeApplyPatch::Body(ApplyPatchArgs {
            patch,
            hunks,
            workdir,
        }) => {
            let effective_cwd = match workdir.as_ref() {
                Some(dir) => match resolve_base_dir(cwd, Path::new(dir)) {
                    Ok(path) => path,
                    Err(error) => return MaybeApplyPatchVerified::CorrectnessError(error),
                },
                None => cwd.to_path_buf(),
            };
            let resolved_hunks = match resolve_hunks(&hunks, &effective_cwd) {
                Ok(resolved_hunks) => resolved_hunks,
                Err(error) => return MaybeApplyPatchVerified::CorrectnessError(error),
            };
            let changes = match preview_file_changes(&resolved_hunks) {
                Ok(changes) => changes,
                Err(error) => {
                    let error = match error.downcast::<ApplyPatchError>() {
                        Ok(error) => error,
                        Err(error) => ApplyPatchError::ComputeReplacements(error.to_string()),
                    };
                    return MaybeApplyPatchVerified::CorrectnessError(error);
                }
            };
            MaybeApplyPatchVerified::Body(ApplyPatchAction {
                changes,
                patch,
                cwd: effective_cwd,
            })
        }
        MaybeApplyPatch::ShellParseError(e) => MaybeApplyPatchVerified::ShellParseError(e),
        MaybeApplyPatch::PatchParseError(e) => MaybeApplyPatchVerified::CorrectnessError(e.into()),
        MaybeApplyPatch::NotApplyPatch => MaybeApplyPatchVerified::NotApplyPatch,
    }
}

/// Extract the heredoc body (and optional `cd` workdir) from a `bash -lc` script
/// that invokes the apply_patch tool using a heredoc.
///
/// Supported top‑level forms (must be the only top‑level statement):
/// - `apply_patch <<'EOF'\n...\nEOF`
/// - `cd <path> && apply_patch <<'EOF'\n...\nEOF`
///
/// Notes about matching:
/// - Parsed with Tree‑sitter Bash and a strict query that uses anchors so the
///   heredoc‑redirected statement is the only top‑level statement.
/// - The connector between `cd` and `apply_patch` must be `&&` (not `|` or `||`).
/// - Exactly one positional `word` argument is allowed for `cd` (no flags, no quoted
///   strings, no second argument).
/// - The apply command is validated in‑query via `#any-of?` to allow `apply_patch`
///   or `applypatch`.
/// - Preceding or trailing commands (e.g., `echo ...;` or `... && echo done`) do not match.
///
/// Returns `(heredoc_body, Some(path))` when the `cd` variant matches, or
/// `(heredoc_body, None)` for the direct form. Errors are returned if the script
/// cannot be parsed or does not match the allowed patterns.
fn extract_apply_patch_from_bash(
    src: &str,
) -> std::result::Result<(String, Option<String>), ExtractHeredocError> {
    // This function uses a Tree-sitter query to recognize one of two
    // whole-script forms, each expressed as a single top-level statement:
    //
    // 1. apply_patch <<'EOF'\n...\nEOF
    // 2. cd <path> && apply_patch <<'EOF'\n...\nEOF
    //
    // Key ideas when reading the query:
    // - dots (`.`) between named nodes enforces adjacency among named children and
    //   anchor to the start/end of the expression.
    // - we match a single redirected_statement directly under program with leading
    //   and trailing anchors (`.`). This ensures it is the only top-level statement
    //   (so prefixes like `echo ...;` or suffixes like `... && echo done` do not match).
    //
    // Overall, we want to be conservative and only match the intended forms, as other
    // forms are likely to be model errors, or incorrectly interpreted by later code.
    //
    // If you're editing this query, it's helpful to start by creating a debugging binary
    // which will let you see the AST of an arbitrary bash script passed in, and optionally
    // also run an arbitrary query against the AST. This is useful for understanding
    // how tree-sitter parses the script and whether the query syntax is correct. Be sure
    // to test both positive and negative cases.
    static APPLY_PATCH_QUERY: LazyLock<Query> = LazyLock::new(|| {
        let language = BASH.into();
        #[expect(clippy::expect_used)]
        Query::new(
            &language,
            r#"
            (
              program
                . (redirected_statement
                    body: (command
                            name: (command_name (word) @apply_name) .)
                    (#any-of? @apply_name "apply_patch" "applypatch")
                    redirect: (heredoc_redirect
                                . (heredoc_start)
                                . (heredoc_body) @heredoc
                                . (heredoc_end)
                                .))
                .)

            (
              program
                . (redirected_statement
                    body: (list
                            . (command
                                name: (command_name (word) @cd_name) .
                                argument: [
                                  (word) @cd_path
                                  (string (string_content) @cd_path)
                                  (raw_string) @cd_raw_string
                                ] .)
                            "&&"
                            . (command
                                name: (command_name (word) @apply_name))
                            .)
                    (#eq? @cd_name "cd")
                    (#any-of? @apply_name "apply_patch" "applypatch")
                    redirect: (heredoc_redirect
                                . (heredoc_start)
                                . (heredoc_body) @heredoc
                                . (heredoc_end)
                                .))
                .)
            "#,
        )
        .expect("valid bash query")
    });

    let lang = BASH.into();
    let mut parser = Parser::new();
    parser
        .set_language(&lang)
        .map_err(ExtractHeredocError::FailedToLoadBashGrammar)?;
    let tree = parser
        .parse(src, None)
        .ok_or(ExtractHeredocError::FailedToParsePatchIntoAst)?;

    let bytes = src.as_bytes();
    let root = tree.root_node();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&APPLY_PATCH_QUERY, root, bytes);
    while let Some(m) = matches.next() {
        let mut heredoc_text: Option<String> = None;
        let mut cd_path: Option<String> = None;

        for capture in m.captures.iter() {
            let name = APPLY_PATCH_QUERY.capture_names()[capture.index as usize];
            match name {
                "heredoc" => {
                    let text = capture
                        .node
                        .utf8_text(bytes)
                        .map_err(ExtractHeredocError::HeredocNotUtf8)?
                        .trim_end_matches('\n')
                        .to_string();
                    heredoc_text = Some(text);
                }
                "cd_path" => {
                    let text = capture
                        .node
                        .utf8_text(bytes)
                        .map_err(ExtractHeredocError::HeredocNotUtf8)?
                        .to_string();
                    cd_path = Some(text);
                }
                "cd_raw_string" => {
                    let raw = capture
                        .node
                        .utf8_text(bytes)
                        .map_err(ExtractHeredocError::HeredocNotUtf8)?;
                    let trimmed = raw
                        .strip_prefix('\'')
                        .and_then(|s| s.strip_suffix('\''))
                        .unwrap_or(raw);
                    cd_path = Some(trimmed.to_string());
                }
                _ => {}
            }
        }

        if let Some(heredoc) = heredoc_text {
            return Ok((heredoc, cd_path));
        }
    }

    Err(ExtractHeredocError::CommandDidNotStartWithApplyPatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ApplyPatchFileChange;
    use crate::ApplyPatchFileUpdate;
    use crate::unified_diff_from_chunks;
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::string::ToString;
    use tempfile::tempdir;

    /// Helper to construct a patch with the given body.
    fn wrap_patch(body: &str) -> String {
        format!("*** Begin Patch\n{body}\n*** End Patch")
    }

    fn strs_to_strings(strs: &[&str]) -> Vec<String> {
        strs.iter().map(ToString::to_string).collect()
    }

    // Test helpers to reduce repetition when building bash -lc heredoc scripts
    fn args_bash(script: &str) -> Vec<String> {
        strs_to_strings(&["bash", "-lc", script])
    }

    fn args_powershell(script: &str) -> Vec<String> {
        strs_to_strings(&["powershell.exe", "-Command", script])
    }

    fn args_powershell_no_profile(script: &str) -> Vec<String> {
        strs_to_strings(&["powershell.exe", "-NoProfile", "-Command", script])
    }

    fn args_pwsh(script: &str) -> Vec<String> {
        strs_to_strings(&["pwsh", "-NoProfile", "-Command", script])
    }

    fn args_cmd(script: &str) -> Vec<String> {
        strs_to_strings(&["cmd.exe", "/c", script])
    }

    fn heredoc_script(prefix: &str) -> String {
        format!(
            "{prefix}apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch\nPATCH"
        )
    }

    fn heredoc_script_ps(prefix: &str, suffix: &str) -> String {
        format!(
            "{prefix}apply_patch <<'PATCH'\n*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch\nPATCH{suffix}"
        )
    }

    fn expected_single_add() -> Vec<Hunk> {
        vec![Hunk::AddFile {
            path: PathBuf::from("foo"),
            contents: "hi\n".to_string(),
        }]
    }

    fn assert_match_args(args: Vec<String>, expected_workdir: Option<&str>) {
        match maybe_parse_apply_patch(&args) {
            MaybeApplyPatch::Body(ApplyPatchArgs { hunks, workdir, .. }) => {
                assert_eq!(workdir.as_deref(), expected_workdir);
                assert_eq!(hunks, expected_single_add());
            }
            result => panic!("expected MaybeApplyPatch::Body got {result:?}"),
        }
    }

    fn assert_match(script: &str, expected_workdir: Option<&str>) {
        let args = args_bash(script);
        assert_match_args(args, expected_workdir);
    }

    fn assert_not_match(script: &str) {
        let args = args_bash(script);
        assert_matches!(
            maybe_parse_apply_patch(&args),
            MaybeApplyPatch::NotApplyPatch
        );
    }

    #[test]
    fn test_implicit_patch_single_arg_is_error() {
        let patch = "*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch".to_string();
        let args = vec![patch];
        let dir = tempdir().unwrap();
        assert_matches!(
            maybe_parse_apply_patch_verified(&args, dir.path()),
            MaybeApplyPatchVerified::CorrectnessError(ApplyPatchError::ImplicitInvocation)
        );
    }

    #[test]
    fn test_implicit_patch_bash_script_is_error() {
        let script = "*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch";
        let args = args_bash(script);
        let dir = tempdir().unwrap();
        assert_matches!(
            maybe_parse_apply_patch_verified(&args, dir.path()),
            MaybeApplyPatchVerified::CorrectnessError(ApplyPatchError::ImplicitInvocation)
        );
    }

    #[test]
    fn test_literal() {
        let args = strs_to_strings(&[
            "apply_patch",
            r#"*** Begin Patch
*** Add File: foo
+hi
*** End Patch
"#,
        ]);

        match maybe_parse_apply_patch(&args) {
            MaybeApplyPatch::Body(ApplyPatchArgs { hunks, .. }) => {
                assert_eq!(
                    hunks,
                    vec![Hunk::AddFile {
                        path: PathBuf::from("foo"),
                        contents: "hi\n".to_string()
                    }]
                );
            }
            result => panic!("expected MaybeApplyPatch::Body got {result:?}"),
        }
    }

    #[test]
    fn test_literal_applypatch() {
        let args = strs_to_strings(&[
            "applypatch",
            r#"*** Begin Patch
*** Add File: foo
+hi
*** End Patch
"#,
        ]);

        match maybe_parse_apply_patch(&args) {
            MaybeApplyPatch::Body(ApplyPatchArgs { hunks, .. }) => {
                assert_eq!(
                    hunks,
                    vec![Hunk::AddFile {
                        path: PathBuf::from("foo"),
                        contents: "hi\n".to_string()
                    }]
                );
            }
            result => panic!("expected MaybeApplyPatch::Body got {result:?}"),
        }
    }

    #[test]
    fn test_heredoc() {
        assert_match(&heredoc_script(""), /*expected_workdir*/ None);
    }

    #[test]
    fn test_heredoc_non_login_shell() {
        let script = heredoc_script("");
        let args = strs_to_strings(&["bash", "-c", &script]);
        assert_match_args(args, /*expected_workdir*/ None);
    }

    #[test]
    fn test_heredoc_applypatch() {
        let args = strs_to_strings(&[
            "bash",
            "-lc",
            r#"applypatch <<'PATCH'
*** Begin Patch
*** Add File: foo
+hi
*** End Patch
PATCH"#,
        ]);

        match maybe_parse_apply_patch(&args) {
            MaybeApplyPatch::Body(ApplyPatchArgs { hunks, workdir, .. }) => {
                assert_eq!(workdir, None);
                assert_eq!(
                    hunks,
                    vec![Hunk::AddFile {
                        path: PathBuf::from("foo"),
                        contents: "hi\n".to_string()
                    }]
                );
            }
            result => panic!("expected MaybeApplyPatch::Body got {result:?}"),
        }
    }

    #[test]
    fn test_powershell_heredoc() {
        let script = heredoc_script("");
        assert_match_args(args_powershell(&script), /*expected_workdir*/ None);
    }
    #[test]
    fn test_powershell_heredoc_no_profile() {
        let script = heredoc_script("");
        assert_match_args(
            args_powershell_no_profile(&script),
            /*expected_workdir*/ None,
        );
    }
    #[test]
    fn test_pwsh_heredoc() {
        let script = heredoc_script("");
        assert_match_args(args_pwsh(&script), /*expected_workdir*/ None);
    }

    #[test]
    fn test_cmd_heredoc_with_cd() {
        let script = heredoc_script("cd foo && ");
        assert_match_args(args_cmd(&script), Some("foo"));
    }

    #[test]
    fn test_heredoc_with_leading_cd() {
        assert_match(&heredoc_script("cd foo && "), Some("foo"));
    }

    #[test]
    fn test_cd_with_semicolon_is_ignored() {
        assert_not_match(&heredoc_script("cd foo; "));
    }

    #[test]
    fn test_cd_or_apply_patch_is_ignored() {
        assert_not_match(&heredoc_script("cd bar || "));
    }

    #[test]
    fn test_cd_pipe_apply_patch_is_ignored() {
        assert_not_match(&heredoc_script("cd bar | "));
    }

    #[test]
    fn test_cd_single_quoted_path_with_spaces() {
        assert_match(&heredoc_script("cd 'foo bar' && "), Some("foo bar"));
    }

    #[test]
    fn test_cd_double_quoted_path_with_spaces() {
        assert_match(&heredoc_script("cd \"foo bar\" && "), Some("foo bar"));
    }

    #[test]
    fn test_echo_and_apply_patch_is_ignored() {
        assert_not_match(&heredoc_script("echo foo && "));
    }

    #[test]
    fn test_apply_patch_with_arg_is_ignored() {
        let script = "apply_patch foo <<'PATCH'\n*** Begin Patch\n*** Add File: foo\n+hi\n*** End Patch\nPATCH";
        assert_not_match(script);
    }

    #[test]
    fn test_double_cd_then_apply_patch_is_ignored() {
        assert_not_match(&heredoc_script("cd foo && cd bar && "));
    }

    #[test]
    fn test_cd_two_args_is_ignored() {
        assert_not_match(&heredoc_script("cd foo bar && "));
    }

    #[test]
    fn test_cd_then_apply_patch_then_extra_is_ignored() {
        let script = heredoc_script_ps("cd bar && ", " && echo done");
        assert_not_match(&script);
    }

    #[test]
    fn test_echo_then_cd_and_apply_patch_is_ignored() {
        // Ensure preceding commands before the `cd && apply_patch <<...` sequence do not match.
        assert_not_match(&heredoc_script("echo foo; cd bar && "));
    }

    #[test]
    fn test_unified_diff_last_line_replacement() {
        // Replace the very last line of the file.
        let dir = tempdir().unwrap();
        let path = dir.path().join("last.txt");
        fs::write(&path, "foo\nbar\nbaz\n").unwrap();

        let patch = wrap_patch(&format!(
            r#"*** Update File: {}
@@
 foo
 bar
-baz
+BAZ
"#,
            path.display()
        ));

        let patch = parse_patch(&patch).unwrap();
        let chunks = match patch.hunks.as_slice() {
            [Hunk::UpdateFile { chunks, .. }] => chunks,
            _ => panic!("Expected a single UpdateFile hunk"),
        };

        let diff = unified_diff_from_chunks(&path, chunks).unwrap();
        let expected_diff = r#"@@ -2,2 +2,2 @@
 bar
-baz
+BAZ
"#;
        let expected = ApplyPatchFileUpdate {
            unified_diff: expected_diff.to_string(),
            content: "foo\nbar\nBAZ\n".to_string(),
        };
        assert_eq!(expected, diff);
    }

    #[test]
    fn test_unified_diff_insert_at_eof() {
        // Insert a new line at end‑of‑file.
        let dir = tempdir().unwrap();
        let path = dir.path().join("insert.txt");
        fs::write(&path, "foo\nbar\nbaz\n").unwrap();

        let patch = wrap_patch(&format!(
            r#"*** Update File: {}
@@
+quux
*** End of File
"#,
            path.display()
        ));

        let patch = parse_patch(&patch).unwrap();
        let chunks = match patch.hunks.as_slice() {
            [Hunk::UpdateFile { chunks, .. }] => chunks,
            _ => panic!("Expected a single UpdateFile hunk"),
        };

        let diff = unified_diff_from_chunks(&path, chunks).unwrap();
        let expected_diff = r#"@@ -3 +3,2 @@
 baz
+quux
"#;
        let expected = ApplyPatchFileUpdate {
            unified_diff: expected_diff.to_string(),
            content: "foo\nbar\nbaz\nquux\n".to_string(),
        };
        assert_eq!(expected, diff);
    }

    #[test]
    fn test_apply_patch_should_resolve_absolute_paths_in_cwd() {
        let session_dir = tempdir().unwrap();
        let relative_path = "source.txt";

        // Note that we need this file to exist for the patch to be "verified"
        // and parsed correctly.
        let session_file_path = session_dir.path().join(relative_path);
        fs::write(&session_file_path, "session directory content\n").unwrap();

        let argv = vec![
            "apply_patch".to_string(),
            r#"*** Begin Patch
*** Update File: source.txt
@@
-session directory content
+updated session directory content
*** End Patch"#
                .to_string(),
        ];

        let result = maybe_parse_apply_patch_verified(&argv, session_dir.path());

        // Verify the patch contents - as otherwise we may have pulled contents
        // from the wrong file (as we're using relative paths)
        assert_eq!(
            result,
            MaybeApplyPatchVerified::Body(ApplyPatchAction {
                changes: HashMap::from([(
                    session_dir.path().join(relative_path),
                    ApplyPatchFileChange::Update {
                        unified_diff: r#"@@ -1 +1 @@
-session directory content
+updated session directory content
"#
                        .to_string(),
                        move_path: None,
                        new_content: "updated session directory content\n".to_string(),
                    },
                )]),
                patch: argv[1].clone(),
                cwd: session_dir.path().to_path_buf(),
            })
        );
    }

    #[test]
    fn test_apply_patch_resolves_move_path_with_effective_cwd() {
        let session_dir = tempdir().unwrap();
        let worktree_rel = "alt";
        let worktree_dir = session_dir.path().join(worktree_rel);
        fs::create_dir_all(&worktree_dir).unwrap();

        let source_name = "old.txt";
        let dest_name = "renamed.txt";
        let source_path = worktree_dir.join(source_name);
        fs::write(&source_path, "before\n").unwrap();

        let patch = wrap_patch(&format!(
            r#"*** Update File: {source_name}
*** Move to: {dest_name}
@@
-before
+after"#
        ));

        let shell_script = format!("cd {worktree_rel} && apply_patch <<'PATCH'\n{patch}\nPATCH");
        let argv = vec!["bash".into(), "-lc".into(), shell_script];

        let result = maybe_parse_apply_patch_verified(&argv, session_dir.path());
        let action = match result {
            MaybeApplyPatchVerified::Body(action) => action,
            other => panic!("expected verified body, got {other:?}"),
        };

        assert_eq!(action.cwd, worktree_dir);

        let change = action
            .changes()
            .get(&worktree_dir.join(source_name))
            .expect("source file change present");

        match change {
            ApplyPatchFileChange::Update { move_path, .. } => {
                assert_eq!(
                    move_path.as_deref(),
                    Some(worktree_dir.join(dest_name).as_path())
                );
            }
            other => panic!("expected update change, got {other:?}"),
        }
    }

    #[test]
    fn test_apply_patch_verified_merges_multiple_updates_for_same_file() {
        let session_dir = tempdir().unwrap();
        let target_path = session_dir.path().join("multi-update.txt");
        fs::write(
            &target_path,
            "alpha
beta
gamma
",
        )
        .unwrap();

        let patch = wrap_patch(
            "*** Update File: multi-update.txt
@@
-beta
+delta
*** Update File: multi-update.txt
@@
-gamma
+omega",
        );
        let argv = vec!["apply_patch".to_string(), patch];

        let result = maybe_parse_apply_patch_verified(&argv, session_dir.path());
        let action = match result {
            MaybeApplyPatchVerified::Body(action) => action,
            other => panic!("expected verified body, got {other:?}"),
        };

        assert_eq!(action.cwd, session_dir.path());
        assert_eq!(action.changes().len(), 1);
        let change = action
            .changes()
            .get(&target_path)
            .expect("merged update present");
        match change {
            ApplyPatchFileChange::Update {
                unified_diff,
                move_path,
                new_content,
            } => {
                assert_eq!(move_path, &None);
                assert_eq!(
                    new_content,
                    "alpha
delta
omega
"
                );
                assert!(unified_diff.contains("-beta"));
                assert!(unified_diff.contains("-gamma"));
                assert!(unified_diff.contains("+delta"));
                assert!(unified_diff.contains("+omega"));
            }
            other => panic!("expected merged update, got {other:?}"),
        }
    }

    #[test]
    fn test_apply_patch_verified_merges_add_then_update_same_file() {
        let session_dir = tempdir().unwrap();
        let target_path = session_dir.path().join("nested").join("generated.txt");

        let patch = wrap_patch(
            "*** Add File: nested/generated.txt
+line1
*** Update File: nested/generated.txt
@@
-line1
+line2",
        );
        let argv = vec!["apply_patch".to_string(), patch];

        let result = maybe_parse_apply_patch_verified(&argv, session_dir.path());
        let action = match result {
            MaybeApplyPatchVerified::Body(action) => action,
            other => panic!("expected verified body, got {other:?}"),
        };

        assert_eq!(action.changes().len(), 1);
        assert_eq!(
            action.changes().get(&target_path),
            Some(&ApplyPatchFileChange::Add {
                content: "line2
"
                .to_string(),
            })
        );
    }

    #[test]
    fn test_apply_patch_verified_merges_move_then_followup_update() {
        let session_dir = tempdir().unwrap();
        let source_path = session_dir.path().join("old.txt");
        let dest_path = session_dir.path().join("renamed.txt");
        fs::write(
            &source_path,
            "before
",
        )
        .unwrap();

        let patch = wrap_patch(
            "*** Update File: old.txt
*** Move to: renamed.txt
@@
-before
+after1
*** Update File: renamed.txt
@@
-after1
+after2",
        );
        let argv = vec!["apply_patch".to_string(), patch];

        let result = maybe_parse_apply_patch_verified(&argv, session_dir.path());
        let action = match result {
            MaybeApplyPatchVerified::Body(action) => action,
            other => panic!("expected verified body, got {other:?}"),
        };

        assert_eq!(action.changes().len(), 1);
        let change = action
            .changes()
            .get(&source_path)
            .expect("merged move update present");
        match change {
            ApplyPatchFileChange::Update {
                unified_diff,
                move_path,
                new_content,
            } => {
                assert_eq!(move_path.as_ref(), Some(&dest_path));
                assert_eq!(
                    new_content,
                    "after2
"
                );
                assert!(unified_diff.contains("-before"));
                assert!(unified_diff.contains("+after2"));
            }
            other => panic!("expected merged move update, got {other:?}"),
        }
    }

    #[test]
    fn test_apply_patch_verified_move_away_then_back_keeps_final_update() {
        let session_dir = tempdir().unwrap();
        let source_path = session_dir.path().join("old.txt");
        let dest_path = session_dir.path().join("renamed.txt");
        fs::write(&source_path, "before\n").unwrap();

        let patch = wrap_patch(
            "*** Update File: old.txt\n*** Move to: renamed.txt\n@@\n-before\n+moved\n*** Update File: renamed.txt\n*** Move to: old.txt\n@@\n-moved\n+restored",
        );
        let argv = vec!["apply_patch".to_string(), patch];

        let result = maybe_parse_apply_patch_verified(&argv, session_dir.path());
        let action = match result {
            MaybeApplyPatchVerified::Body(action) => action,
            other => panic!("expected verified body, got {other:?}"),
        };

        assert_eq!(action.changes().len(), 1);
        let change = action
            .changes()
            .get(&source_path)
            .expect("move-away-then-back update present");
        match change {
            ApplyPatchFileChange::Update {
                unified_diff,
                move_path,
                new_content,
            } => {
                assert_eq!(move_path, &None);
                assert_eq!(new_content, "restored\n");
                assert!(unified_diff.contains("-before"));
                assert!(unified_diff.contains("+restored"));
            }
            other => panic!("expected merged move-back update, got {other:?}"),
        }
        assert_eq!(action.changes().get(&dest_path), None);
    }

    #[test]
    fn test_apply_patch_verified_overwrite_then_delete_preserves_original_delete() {
        let session_dir = tempdir().unwrap();
        let target_path = session_dir.path().join("existing.txt");
        fs::write(
            &target_path,
            "original
",
        )
        .unwrap();

        let patch = wrap_patch(
            "*** Add File: existing.txt
+replacement
*** Delete File: existing.txt",
        );
        let argv = vec!["apply_patch".to_string(), patch];

        let result = maybe_parse_apply_patch_verified(&argv, session_dir.path());
        let action = match result {
            MaybeApplyPatchVerified::Body(action) => action,
            other => panic!("expected verified body, got {other:?}"),
        };

        assert_eq!(action.changes().len(), 1);
        assert_eq!(
            action.changes().get(&target_path),
            Some(&ApplyPatchFileChange::Delete {
                content: "original
"
                .to_string(),
            })
        );
    }

    #[test]
    fn test_apply_patch_verified_move_into_existing_destination_then_delete_keeps_both_deletes() {
        let session_dir = tempdir().unwrap();
        let source_path = session_dir.path().join("source.txt");
        let dest_path = session_dir.path().join("dest.txt");
        fs::write(
            &source_path,
            "source
",
        )
        .unwrap();
        fs::write(
            &dest_path, "dest
",
        )
        .unwrap();

        let patch = wrap_patch(
            "*** Update File: source.txt
*** Move to: dest.txt
@@
-source
+moved
*** Delete File: dest.txt",
        );
        let argv = vec!["apply_patch".to_string(), patch];

        let result = maybe_parse_apply_patch_verified(&argv, session_dir.path());
        let action = match result {
            MaybeApplyPatchVerified::Body(action) => action,
            other => panic!("expected verified body, got {other:?}"),
        };

        assert_eq!(action.changes().len(), 2);
        assert_eq!(
            action.changes().get(&source_path),
            Some(&ApplyPatchFileChange::Delete {
                content: "source
"
                .to_string(),
            })
        );
        assert_eq!(
            action.changes().get(&dest_path),
            Some(&ApplyPatchFileChange::Delete {
                content: "dest
"
                .to_string(),
            })
        );
    }

    #[test]
    fn test_apply_patch_resolves_literal_tilde_workdir_without_home_expansion() {
        let session_dir = tempdir().unwrap();
        let worktree_dir = session_dir.path().join("~").join("literal");
        fs::create_dir_all(&worktree_dir).unwrap();

        let source_path = worktree_dir.join("old.txt");
        fs::write(&source_path, "before\n").unwrap();

        let patch = wrap_patch("*** Update File: old.txt\n@@\n-before\n+after");
        let shell_script = format!("cd ~/literal && apply_patch <<'PATCH'\n{patch}\nPATCH");
        let argv = vec!["bash".into(), "-lc".into(), shell_script];

        let result = maybe_parse_apply_patch_verified(&argv, session_dir.path());
        let action = match result {
            MaybeApplyPatchVerified::Body(action) => action,
            other => panic!("expected verified body, got {other:?}"),
        };

        assert_eq!(action.cwd, worktree_dir);
        assert!(action.changes().contains_key(&source_path));
    }

    #[test]
    fn test_apply_patch_reports_missing_context_with_effective_cwd() {
        let session_dir = tempdir().unwrap();
        let worktree_dir = session_dir.path().join("alt");
        fs::create_dir_all(&worktree_dir).unwrap();
        fs::write(worktree_dir.join("modify.txt"), "line1\nline2\n").unwrap();

        let patch = wrap_patch("*** Update File: modify.txt\n@@\n-missing\n+changed");
        let shell_script = format!("cd alt && apply_patch <<'PATCH'\n{patch}\nPATCH");
        let argv = vec!["bash".into(), "-lc".into(), shell_script];

        let result = maybe_parse_apply_patch_verified(&argv, session_dir.path());
        let error = match result {
            MaybeApplyPatchVerified::CorrectnessError(error) => error,
            other => panic!("expected correctness error, got {other:?}"),
        };

        assert_eq!(
            error,
            ApplyPatchError::ComputeReplacements(format!(
                "Failed to find expected lines in modify.txt (cwd: {}, resolved: {}):\nmissing",
                worktree_dir.display(),
                worktree_dir.join("modify.txt").display()
            ))
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_apply_patch_verified_accepts_delete_of_dangling_symlink() {
        let session_dir = tempdir().unwrap();
        let link_path = session_dir.path().join("dangling-link");
        std::os::unix::fs::symlink("missing-target", &link_path).unwrap();

        let argv = vec![
            "apply_patch".to_string(),
            "*** Begin Patch\n*** Delete File: dangling-link\n*** End Patch".to_string(),
        ];

        let result = maybe_parse_apply_patch_verified(&argv, session_dir.path());
        let action = match result {
            MaybeApplyPatchVerified::Body(action) => action,
            other => panic!("expected verified body, got {other:?}"),
        };

        assert_eq!(action.cwd, session_dir.path());
        assert_eq!(
            action.changes().get(&link_path),
            Some(&ApplyPatchFileChange::Delete {
                content: String::new(),
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_apply_patch_verified_reads_live_symlink_delete_contents() {
        let session_dir = tempdir().unwrap();
        let target_path = session_dir.path().join("target.txt");
        fs::write(&target_path, "target contents\n").unwrap();
        let link_path = session_dir.path().join("live-link");
        std::os::unix::fs::symlink("target.txt", &link_path).unwrap();

        let argv = vec![
            "apply_patch".to_string(),
            "*** Begin Patch\n*** Delete File: live-link\n*** End Patch".to_string(),
        ];

        let result = maybe_parse_apply_patch_verified(&argv, session_dir.path());
        let action = match result {
            MaybeApplyPatchVerified::Body(action) => action,
            other => panic!("expected verified body, got {other:?}"),
        };

        assert_eq!(
            action.changes().get(&link_path),
            Some(&ApplyPatchFileChange::Delete {
                content: "target contents\n".to_string(),
            })
        );
    }
}
