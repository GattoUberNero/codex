use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use crate::ApplyPatchError;
use crate::Hunk;
use crate::IoError;
use crate::UpdateFileChunk;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ResolvedPath {
    requested: PathBuf,
    absolute: PathBuf,
    base_dir: PathBuf,
}

impl ResolvedPath {
    pub(crate) fn requested(&self) -> &Path {
        &self.requested
    }

    pub(crate) fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub(crate) fn absolute(&self) -> &Path {
        &self.absolute
    }

    pub(crate) fn describe_for_error(&self) -> String {
        if self.requested.is_absolute() {
            return self.requested.display().to_string();
        }

        format!(
            "{} (cwd: {}, resolved: {})",
            self.requested.display(),
            self.base_dir.display(),
            self.absolute.display()
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ResolvedHunk {
    Add {
        path: ResolvedPath,
        contents: String,
    },
    Delete {
        path: ResolvedPath,
    },
    Update {
        path: ResolvedPath,
        move_path: Option<ResolvedPath>,
        chunks: Vec<UpdateFileChunk>,
    },
}

pub(crate) fn resolve_hunks(
    hunks: &[Hunk],
    base_dir: &Path,
) -> Result<Vec<ResolvedHunk>, ApplyPatchError> {
    validate_absolute_base_dir(base_dir)?;

    hunks
        .iter()
        .map(|hunk| match hunk {
            Hunk::AddFile { path, contents } => Ok(ResolvedHunk::Add {
                path: resolve_path(path, base_dir)?,
                contents: contents.clone(),
            }),
            Hunk::DeleteFile { path } => Ok(ResolvedHunk::Delete {
                path: resolve_path(path, base_dir)?,
            }),
            Hunk::UpdateFile {
                path,
                move_path,
                chunks,
            } => Ok(ResolvedHunk::Update {
                path: resolve_path(path, base_dir)?,
                move_path: match move_path.as_ref() {
                    Some(path) => Some(resolve_path(path, base_dir)?),
                    None => None,
                },
                chunks: chunks.clone(),
            }),
        })
        .collect()
}

pub(crate) fn resolve_base_dir(
    base_dir: &Path,
    relative_or_absolute: &Path,
) -> Result<PathBuf, ApplyPatchError> {
    validate_absolute_base_dir(base_dir)?;
    normalize_path_against_base(relative_or_absolute, base_dir)
}

fn resolve_path(path: &Path, base_dir: &Path) -> Result<ResolvedPath, ApplyPatchError> {
    let absolute = normalize_path_against_base(path, base_dir)?;

    Ok(ResolvedPath {
        requested: path.to_path_buf(),
        absolute,
        base_dir: base_dir.to_path_buf(),
    })
}

fn validate_absolute_base_dir(base_dir: &Path) -> Result<(), ApplyPatchError> {
    if base_dir.is_absolute() {
        return Ok(());
    }

    Err(ApplyPatchError::IoError(IoError::new(
        format!(
            "path normalization requires an absolute base_dir, got {}",
            base_dir.display()
        ),
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "base_dir must be absolute",
        ),
    )))
}

fn normalize_path_against_base(path: &Path, base_dir: &Path) -> Result<PathBuf, ApplyPatchError> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    };

    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(ApplyPatchError::IoError(IoError::new(
                        format!(
                            "failed to normalize patch path {} against base_dir {}",
                            path.display(),
                            base_dir.display()
                        ),
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "path normalization escaped root",
                        ),
                    )));
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    if normalized.is_absolute() {
        Ok(normalized)
    } else {
        Err(ApplyPatchError::IoError(IoError::new(
            format!(
                "failed to normalize patch path {} against base_dir {}",
                path.display(),
                base_dir.display()
            ),
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "normalized path is not absolute",
            ),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[test]
    fn resolve_hunks_rejects_relative_base_dir() {
        let hunks = vec![Hunk::DeleteFile {
            path: PathBuf::from("file.txt"),
        }];

        let error = resolve_hunks(&hunks, Path::new("relative")).expect_err("must fail");

        assert_eq!(
            error,
            ApplyPatchError::IoError(IoError::new(
                "path normalization requires an absolute base_dir, got relative".to_string(),
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "base_dir must be absolute"
                ),
            ))
        );
    }

    #[test]
    fn resolve_hunks_keeps_absolute_paths() {
        let dir = tempdir().expect("tempdir");
        let absolute = dir.path().join("file.txt");
        let hunks = vec![Hunk::DeleteFile {
            path: absolute.clone(),
        }];

        let resolved = resolve_hunks(&hunks, dir.path()).expect("resolve");

        assert_eq!(
            resolved,
            vec![ResolvedHunk::Delete {
                path: ResolvedPath {
                    requested: absolute.clone(),
                    absolute,
                    base_dir: dir.path().to_path_buf(),
                },
            }]
        );
    }

    #[test]
    fn resolve_hunks_joins_relative_paths_against_base_dir() {
        let dir = tempdir().expect("tempdir");
        let hunks = vec![Hunk::UpdateFile {
            path: PathBuf::from("old.txt"),
            move_path: Some(PathBuf::from("new.txt")),
            chunks: vec![],
        }];

        let resolved = resolve_hunks(&hunks, dir.path()).expect("resolve");

        assert_eq!(
            resolved,
            vec![ResolvedHunk::Update {
                path: ResolvedPath {
                    requested: PathBuf::from("old.txt"),
                    absolute: dir.path().join("old.txt"),
                    base_dir: dir.path().to_path_buf(),
                },
                move_path: Some(ResolvedPath {
                    requested: PathBuf::from("new.txt"),
                    absolute: dir.path().join("new.txt"),
                    base_dir: dir.path().to_path_buf(),
                }),
                chunks: vec![],
            }]
        );
    }

    #[test]
    fn resolve_hunks_normalizes_parent_segments_without_expanding_home() {
        let dir = tempdir().expect("tempdir");
        let hunks = vec![Hunk::DeleteFile {
            path: PathBuf::from("sub/../file.txt"),
        }];

        let resolved = resolve_hunks(&hunks, dir.path()).expect("resolve");

        assert_eq!(
            resolved,
            vec![ResolvedHunk::Delete {
                path: ResolvedPath {
                    requested: PathBuf::from("sub/../file.txt"),
                    absolute: dir.path().join("file.txt"),
                    base_dir: dir.path().to_path_buf(),
                },
            }]
        );
    }

    #[test]
    fn resolve_hunks_does_not_expand_home_prefix() {
        let dir = tempdir().expect("tempdir");
        let hunks = vec![Hunk::DeleteFile {
            path: PathBuf::from("~/file.txt"),
        }];

        let resolved = resolve_hunks(&hunks, dir.path()).expect("resolve");

        assert_eq!(
            resolved,
            vec![ResolvedHunk::Delete {
                path: ResolvedPath {
                    requested: PathBuf::from("~/file.txt"),
                    absolute: dir.path().join("~/file.txt"),
                    base_dir: dir.path().to_path_buf(),
                },
            }]
        );
    }
}
