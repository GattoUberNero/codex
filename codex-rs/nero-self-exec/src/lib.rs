#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::ffi::OsStr;
use std::fmt;
use std::io;
use std::path::Path;
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelfExecPaths {
    pub primary: Option<PathBuf>,
    pub fallback: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelfExecProgramSource {
    ConfiguredPrimary,
    ConfiguredFallback,
    CurrentExe,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelfExecCandidateDiagnostic {
    NotConfigured {
        label: &'static str,
    },
    InvalidConfiguredPath {
        label: &'static str,
        path: PathBuf,
        reason: String,
    },
    PathStatus {
        label: &'static str,
        path: PathBuf,
        exists: bool,
        is_file: bool,
        is_dir: bool,
        metadata_error: Option<String>,
    },
    LookupError {
        label: &'static str,
        error: String,
    },
}

impl SelfExecCandidateDiagnostic {
    pub fn render(&self) -> String {
        match self {
            Self::NotConfigured { label } => format!("self_exec_candidate.{label}=not_configured"),
            Self::InvalidConfiguredPath {
                label,
                path,
                reason,
            } => format!(
                "self_exec_candidate.{label}=invalid_configured_path path={} reason={reason}",
                path.display()
            ),
            Self::PathStatus {
                label,
                path,
                exists,
                is_file,
                is_dir,
                metadata_error,
            } => format!(
                "self_exec_candidate.{label}={} exists={exists} is_file={is_file} is_dir={is_dir} metadata_error={}",
                path.display(),
                metadata_error.as_deref().unwrap_or("none")
            ),
            Self::LookupError { label, error } => {
                format!("self_exec_candidate.{label}=lookup_error error={error}")
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedSelfExec {
    pub path: PathBuf,
    pub source: SelfExecProgramSource,
    pub diagnostics: Vec<SelfExecCandidateDiagnostic>,
    pub retry_fallback: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelfExecResolveError {
    pub diagnostics: Vec<SelfExecCandidateDiagnostic>,
}

impl fmt::Display for SelfExecResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (idx, diagnostic) in self.diagnostics.iter().enumerate() {
            if idx > 0 {
                writeln!(f)?;
            }
            write!(f, "{}", diagnostic.render())?;
        }
        Ok(())
    }
}

impl std::error::Error for SelfExecResolveError {}

pub fn resolve_self_exec(paths: &SelfExecPaths) -> Result<ResolvedSelfExec, SelfExecResolveError> {
    resolve_self_exec_with_lookup(paths, std::env::current_exe())
}

pub fn resolve_self_exec_with_lookup(
    paths: &SelfExecPaths,
    current_exe: io::Result<PathBuf>,
) -> Result<ResolvedSelfExec, SelfExecResolveError> {
    let mut diagnostics = Vec::new();
    let primary = evaluate_primary(paths.primary.as_ref(), &mut diagnostics);
    let fallback = evaluate_fallback(
        paths.fallback.as_ref(),
        paths.primary.as_ref(),
        &mut diagnostics,
    );
    let current = evaluate_current_exe(current_exe, &mut diagnostics);

    if let Some(path) = primary {
        return Ok(ResolvedSelfExec {
            path,
            source: SelfExecProgramSource::ConfiguredPrimary,
            diagnostics,
            retry_fallback: fallback,
        });
    }

    if let Some(path) = fallback {
        return Ok(ResolvedSelfExec {
            path,
            source: SelfExecProgramSource::ConfiguredFallback,
            diagnostics,
            retry_fallback: None,
        });
    }

    if let Some(path) = current {
        return Ok(ResolvedSelfExec {
            path,
            source: SelfExecProgramSource::CurrentExe,
            diagnostics,
            retry_fallback: None,
        });
    }

    Err(SelfExecResolveError { diagnostics })
}

fn evaluate_primary(
    path: Option<&PathBuf>,
    diagnostics: &mut Vec<SelfExecCandidateDiagnostic>,
) -> Option<PathBuf> {
    evaluate_configured_candidate("primary", path, diagnostics)
}

fn evaluate_fallback(
    fallback: Option<&PathBuf>,
    configured_primary: Option<&PathBuf>,
    diagnostics: &mut Vec<SelfExecCandidateDiagnostic>,
) -> Option<PathBuf> {
    let Some(fallback) = fallback else {
        diagnostics.push(SelfExecCandidateDiagnostic::NotConfigured { label: "fallback" });
        return None;
    };

    if !fallback.is_absolute() {
        diagnostics.push(SelfExecCandidateDiagnostic::InvalidConfiguredPath {
            label: "fallback",
            path: fallback.clone(),
            reason: "absolute_path_required".to_string(),
        });
        return None;
    }

    if let Some(primary) = configured_primary {
        let normalized_primary = path_for_comparison(primary);
        let normalized_fallback = path_for_comparison(fallback);
        if normalized_primary == normalized_fallback {
            diagnostics.push(SelfExecCandidateDiagnostic::InvalidConfiguredPath {
                label: "fallback",
                path: fallback.clone(),
                reason: "same_path_as_primary".to_string(),
            });
            return None;
        }
        if in_same_cargo_target_tree(&normalized_primary, &normalized_fallback) {
            diagnostics.push(SelfExecCandidateDiagnostic::InvalidConfiguredPath {
                label: "fallback",
                path: fallback.clone(),
                reason: "same_cargo_target_tree_as_primary".to_string(),
            });
            return None;
        }
    }

    evaluate_path_status("fallback", fallback, diagnostics)
}

fn evaluate_configured_candidate(
    label: &'static str,
    path: Option<&PathBuf>,
    diagnostics: &mut Vec<SelfExecCandidateDiagnostic>,
) -> Option<PathBuf> {
    let Some(path) = path else {
        diagnostics.push(SelfExecCandidateDiagnostic::NotConfigured { label });
        return None;
    };

    if !path.is_absolute() {
        diagnostics.push(SelfExecCandidateDiagnostic::InvalidConfiguredPath {
            label,
            path: path.clone(),
            reason: "absolute_path_required".to_string(),
        });
        return None;
    }

    evaluate_path_status(label, path, diagnostics)
}

fn evaluate_current_exe(
    current_exe: io::Result<PathBuf>,
    diagnostics: &mut Vec<SelfExecCandidateDiagnostic>,
) -> Option<PathBuf> {
    match current_exe {
        Ok(path) => evaluate_path_status("current_exe", &path, diagnostics),
        Err(error) => {
            diagnostics.push(SelfExecCandidateDiagnostic::LookupError {
                label: "current_exe",
                error: error.to_string(),
            });
            None
        }
    }
}

fn evaluate_path_status(
    label: &'static str,
    path: &Path,
    diagnostics: &mut Vec<SelfExecCandidateDiagnostic>,
) -> Option<PathBuf> {
    let diagnostic = match std::fs::metadata(path) {
        Ok(metadata) => SelfExecCandidateDiagnostic::PathStatus {
            label,
            path: path.to_path_buf(),
            exists: true,
            is_file: metadata.is_file(),
            is_dir: metadata.is_dir(),
            metadata_error: None,
        },
        Err(error) => SelfExecCandidateDiagnostic::PathStatus {
            label,
            path: path.to_path_buf(),
            exists: false,
            is_file: false,
            is_dir: false,
            metadata_error: Some(error.to_string()),
        },
    };
    let usable = matches!(
        &diagnostic,
        SelfExecCandidateDiagnostic::PathStatus {
            exists: true,
            is_file: true,
            ..
        }
    );
    diagnostics.push(diagnostic);
    usable.then(|| path.to_path_buf())
}

fn normalize_absolute_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => {
                normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR))
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                let _ = normalized.pop();
            }
            std::path::Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn path_for_comparison(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(|canonical| normalize_absolute_path(&canonical))
        .unwrap_or_else(|_| normalize_absolute_path(path))
}

fn in_same_cargo_target_tree(a: &Path, b: &Path) -> bool {
    cargo_target_root(a)
        .zip(cargo_target_root(b))
        .is_some_and(|(left, right)| left == right)
}

fn cargo_target_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| ancestor.file_name() == Some(OsStr::new("target")))
        .map(normalize_absolute_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[test]
    fn selects_primary_and_keeps_retry_fallback() {
        let dir = tempdir().expect("tempdir");
        let primary = dir.path().join("primary-bin");
        let fallback = dir.path().join("fallback-bin");
        std::fs::write(&primary, "x").expect("write primary");
        std::fs::write(&fallback, "x").expect("write fallback");

        let resolved = resolve_self_exec_with_lookup(
            &SelfExecPaths {
                primary: Some(primary.clone()),
                fallback: Some(fallback.clone()),
            },
            Ok(dir.path().join("current")),
        )
        .expect("resolve");

        assert_eq!(resolved.path, primary);
        assert_eq!(resolved.source, SelfExecProgramSource::ConfiguredPrimary);
        assert_eq!(resolved.retry_fallback, Some(fallback));
    }

    #[test]
    fn selects_fallback_when_primary_missing() {
        let dir = tempdir().expect("tempdir");
        let fallback = dir.path().join("fallback-bin");
        std::fs::write(&fallback, "x").expect("write fallback");

        let resolved = resolve_self_exec_with_lookup(
            &SelfExecPaths {
                primary: Some(dir.path().join("missing-primary")),
                fallback: Some(fallback.clone()),
            },
            Ok(dir.path().join("missing-current")),
        )
        .expect("resolve");

        assert_eq!(resolved.path, fallback);
        assert_eq!(resolved.source, SelfExecProgramSource::ConfiguredFallback);
        assert_eq!(resolved.retry_fallback, None);
    }

    #[test]
    fn rejects_relative_fallback() {
        let dir = tempdir().expect("tempdir");
        let primary = dir.path().join("primary-bin");
        std::fs::write(&primary, "x").expect("write primary");

        let error = resolve_self_exec_with_lookup(
            &SelfExecPaths {
                primary: None,
                fallback: Some(PathBuf::from("relative/fallback")),
            },
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "missing current_exe",
            )),
        )
        .expect_err("should fail");

        assert!(error
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic, SelfExecCandidateDiagnostic::InvalidConfiguredPath { label, reason, .. } if *label == "fallback" && reason == "absolute_path_required")));
    }

    #[test]
    fn rejects_same_target_tree_fallback() {
        let error = resolve_self_exec_with_lookup(
            &SelfExecPaths {
                primary: Some(PathBuf::from("/tmp/work/codex-rs/target/debug/codex")),
                fallback: Some(PathBuf::from("/tmp/work/codex-rs/target/release/codex")),
            },
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "missing current_exe",
            )),
        )
        .expect_err("should fail");

        assert!(error
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic, SelfExecCandidateDiagnostic::InvalidConfiguredPath { label, reason, .. } if *label == "fallback" && reason == "same_cargo_target_tree_as_primary")));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_fallback_into_primary_target_tree() {
        let dir = tempdir().expect("tempdir");
        let target_release = dir.path().join("work/codex-rs/target/release");
        std::fs::create_dir_all(&target_release).expect("create target release dir");
        let release_binary = target_release.join("codex");
        std::fs::write(&release_binary, "x").expect("write release binary");

        let fallback_symlink = dir.path().join("stable-codex");
        std::os::unix::fs::symlink(&release_binary, &fallback_symlink)
            .expect("create fallback symlink");

        let error = resolve_self_exec_with_lookup(
            &SelfExecPaths {
                primary: Some(dir.path().join("work/codex-rs/target/debug/codex")),
                fallback: Some(fallback_symlink),
            },
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "missing current_exe",
            )),
        )
        .expect_err("should fail");

        assert!(error
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic, SelfExecCandidateDiagnostic::InvalidConfiguredPath { label, reason, .. } if *label == "fallback" && reason == "same_cargo_target_tree_as_primary")));
    }

    #[test]
    fn falls_back_to_current_exe() {
        let dir = tempdir().expect("tempdir");
        let current = dir.path().join("current-bin");
        std::fs::write(&current, "x").expect("write current");

        let resolved =
            resolve_self_exec_with_lookup(&SelfExecPaths::default(), Ok(current.clone()))
                .expect("resolve");

        assert_eq!(resolved.path, current);
        assert_eq!(resolved.source, SelfExecProgramSource::CurrentExe);
    }
}
