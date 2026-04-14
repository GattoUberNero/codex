use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use similar::TextDiff;

use crate::ApplyPatchError;
use crate::ApplyPatchFileChange;
use crate::IoError;
use crate::parser::UpdateFileChunk;
use crate::resolved::ResolvedHunk;
use crate::resolved::ResolvedPath;
use crate::seek_sequence;

/// Tracks file paths affected by applying a patch.
pub struct AffectedPaths {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

struct PatchPlan {
    operations: Vec<PlannedOperation>,
    final_states: HashMap<PathBuf, VirtualPathState>,
    explicit_adds: BTreeSet<PathBuf>,
    moved_sources: BTreeSet<PathBuf>,
    moved_destinations: BTreeSet<PathBuf>,
}

#[derive(Clone)]
enum VirtualPathState {
    Missing,
    File(String),
}

#[derive(Clone)]
enum OriginalPathState {
    Missing,
    File(String),
    Symlink {
        target: PathBuf,
        referent_path: Option<PathBuf>,
        referent_contents: Option<String>,
    },
    Directory,
}

enum AffectedPathStatus {
    Added,
    Modified,
    Deleted,
}

enum PlannedOperation {
    Add {
        path: ResolvedPath,
        contents: String,
    },
    Delete {
        path: ResolvedPath,
    },
    Update {
        path: ResolvedPath,
        new_contents: String,
    },
    Move {
        source: ResolvedPath,
        dest: ResolvedPath,
        new_contents: String,
    },
}

#[derive(Clone)]
enum PreviewOriginState {
    Missing,
    File(String),
}

struct PreviewEntity {
    origin_path: PathBuf,
    origin_state: PreviewOriginState,
    current_path: Option<PathBuf>,
    current_content: Option<String>,
}

pub(crate) fn preview_file_changes(
    hunks: &[ResolvedHunk],
) -> anyhow::Result<HashMap<PathBuf, ApplyPatchFileChange>> {
    let plan = plan_hunks(hunks)?;
    let original_states = capture_original_states(&plan)?;
    Ok(build_preview_file_changes(&plan, &original_states))
}

pub fn apply_hunks_to_files(hunks: &[ResolvedHunk]) -> Result<AffectedPaths> {
    if hunks.is_empty() {
        anyhow::bail!("No files were modified.");
    }

    let plan = plan_hunks(hunks)?;
    let original_states = capture_original_states(&plan)?;
    let affected = determine_affected_paths(&plan, &original_states);
    let mut created_dirs = Vec::new();

    for operation in &plan.operations {
        let result = match operation {
            PlannedOperation::Add { path, contents } => {
                create_parent_dirs_if_needed(path.absolute(), &mut created_dirs)?;
                write_file(path.absolute(), contents)
            }
            PlannedOperation::Delete { path } => delete_file(path),
            PlannedOperation::Update { path, new_contents } => {
                write_file(path.absolute(), new_contents)
            }
            PlannedOperation::Move {
                source,
                dest,
                new_contents,
            } => {
                create_parent_dirs_if_needed(dest.absolute(), &mut created_dirs)?;
                write_file(dest.absolute(), new_contents)?;
                remove_original(source)
            }
        };

        if let Err(err) = result {
            if let Err(rollback_err) = rollback_changes(&original_states, &created_dirs) {
                return Err(anyhow::anyhow!(
                    "Failed to apply patch: {err}; rollback failed: {rollback_err}"
                ));
            }
            return Err(err);
        }
    }

    Ok(affected)
}

fn plan_hunks(hunks: &[ResolvedHunk]) -> Result<PatchPlan> {
    let mut operations = Vec::with_capacity(hunks.len());
    let mut final_states = HashMap::new();
    let mut explicit_adds = BTreeSet::new();
    let mut moved_sources = BTreeSet::new();
    let mut moved_destinations = BTreeSet::new();

    for hunk in hunks {
        match hunk {
            ResolvedHunk::Add { path, contents } => {
                final_states.insert(
                    path.absolute().to_path_buf(),
                    VirtualPathState::File(contents.clone()),
                );
                explicit_adds.insert(path.absolute().to_path_buf());
                operations.push(PlannedOperation::Add {
                    path: path.clone(),
                    contents: contents.clone(),
                });
            }
            ResolvedHunk::Delete { path } => {
                preflight_delete(path, &final_states)?;
                final_states.insert(path.absolute().to_path_buf(), VirtualPathState::Missing);
                operations.push(PlannedOperation::Delete { path: path.clone() });
            }
            ResolvedHunk::Update {
                path,
                move_path,
                chunks,
            } => {
                let current_contents = resolve_current_contents(path, &final_states)?;
                let AppliedPatch { new_contents, .. } = derive_new_contents_from_text(
                    &path.describe_for_error(),
                    current_contents,
                    chunks,
                )?;
                if let Some(dest) = move_path
                    && dest.absolute() != path.absolute()
                {
                    final_states.insert(path.absolute().to_path_buf(), VirtualPathState::Missing);
                    final_states.insert(
                        dest.absolute().to_path_buf(),
                        VirtualPathState::File(new_contents.clone()),
                    );
                    moved_sources.insert(path.absolute().to_path_buf());
                    moved_destinations.insert(dest.absolute().to_path_buf());
                    operations.push(PlannedOperation::Move {
                        source: path.clone(),
                        dest: dest.clone(),
                        new_contents,
                    });
                } else {
                    final_states.insert(
                        path.absolute().to_path_buf(),
                        VirtualPathState::File(new_contents.clone()),
                    );
                    operations.push(PlannedOperation::Update {
                        path: path.clone(),
                        new_contents,
                    });
                }
            }
        }
    }

    Ok(PatchPlan {
        operations,
        final_states,
        explicit_adds,
        moved_sources,
        moved_destinations,
    })
}

fn preflight_delete(
    path: &ResolvedPath,
    final_states: &HashMap<PathBuf, VirtualPathState>,
) -> Result<()> {
    if let Some(state) = final_states.get(path.absolute()) {
        return match state {
            VirtualPathState::File(_) => Ok(()),
            VirtualPathState::Missing => Err(anyhow::Error::new(read_file_error(
                path.requested(),
                path.absolute(),
                Some(path.base_dir()),
                std::io::Error::from(std::io::ErrorKind::NotFound),
                "Failed to delete file",
            ))),
        };
    }

    match std::fs::symlink_metadata(path.absolute()) {
        Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => Ok(()),
        Ok(_) => anyhow::bail!(
            "Failed to delete file {}: target is not a file",
            path.describe_for_error()
        ),
        Err(err) => Err(anyhow::Error::new(read_file_error(
            path.requested(),
            path.absolute(),
            Some(path.base_dir()),
            err,
            "Failed to delete file",
        ))),
    }
}

fn create_parent_dirs_if_needed(path: &Path, created_dirs: &mut Vec<PathBuf>) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        let missing_dirs = missing_parent_dirs(parent);
        std::fs::create_dir_all(parent).map_err(|err| {
            anyhow::anyhow!(
                "Failed to create parent directories for {}: {err}",
                describe_path_for_error(path)
            )
        })?;
        for dir in missing_dirs {
            if !created_dirs.contains(&dir) {
                created_dirs.push(dir);
            }
        }
    }
    Ok(())
}

fn write_file(path: &Path, contents: &str) -> Result<()> {
    std::fs::write(path, contents).map_err(|err| {
        anyhow::anyhow!(
            "Failed to write file {}: {err}",
            describe_path_for_error(path)
        )
    })
}

fn delete_file(path: &ResolvedPath) -> Result<()> {
    remove_file_or_symlink(path.absolute()).map_err(|err| {
        anyhow::anyhow!("Failed to delete file {}: {err}", path.describe_for_error())
    })
}

fn remove_original(path: &ResolvedPath) -> Result<()> {
    remove_file_or_symlink(path.absolute()).map_err(|err| {
        anyhow::anyhow!(
            "Failed to remove original {}: {err}",
            path.describe_for_error()
        )
    })
}

fn missing_parent_dirs(parent: &Path) -> Vec<PathBuf> {
    let mut missing = Vec::new();
    let mut current = Some(parent);

    while let Some(path) = current {
        if path.as_os_str().is_empty() || path.exists() {
            break;
        }
        missing.push(path.to_path_buf());
        current = path.parent();
    }

    missing.reverse();
    missing
}

fn describe_path_for_error(path: &Path) -> String {
    if path.is_absolute() {
        return path.display().to_string();
    }

    match std::env::current_dir() {
        Ok(cwd) => format!(
            "{} (cwd: {}, resolved: {})",
            path.display(),
            cwd.display(),
            cwd.join(path).display()
        ),
        Err(_) => path.display().to_string(),
    }
}

fn describe_path_for_error_with_resolved(
    display_path: &Path,
    resolved_path: &Path,
    base_dir: Option<&Path>,
) -> String {
    if display_path.is_absolute() || display_path == resolved_path {
        return describe_path_for_error(display_path);
    }

    if let Some(cwd) = base_dir {
        return format!(
            "{} (cwd: {}, resolved: {})",
            display_path.display(),
            cwd.display(),
            resolved_path.display()
        );
    }

    match std::env::current_dir() {
        Ok(cwd) => format!(
            "{} (cwd: {}, resolved: {})",
            display_path.display(),
            cwd.display(),
            resolved_path.display()
        ),
        Err(_) => resolved_path.display().to_string(),
    }
}

struct AppliedPatch {
    original_contents: String,
    new_contents: String,
}

fn resolve_current_contents(
    path: &ResolvedPath,
    final_states: &HashMap<PathBuf, VirtualPathState>,
) -> std::result::Result<String, ApplyPatchError> {
    if let Some(state) = final_states.get(path.absolute()) {
        return match state {
            VirtualPathState::File(contents) => Ok(contents.clone()),
            VirtualPathState::Missing => Err(read_file_error(
                path.requested(),
                path.absolute(),
                Some(path.base_dir()),
                std::io::Error::from(std::io::ErrorKind::NotFound),
                "Failed to read file to update",
            )),
        };
    }

    read_file_to_string(
        path.requested(),
        path.absolute(),
        Some(path.base_dir()),
        "Failed to read file to update",
    )
}

fn read_file_to_string(
    display_path: &Path,
    fs_path: &Path,
    base_dir: Option<&Path>,
    context: &str,
) -> std::result::Result<String, ApplyPatchError> {
    std::fs::read_to_string(fs_path)
        .map_err(|err| read_file_error(display_path, fs_path, base_dir, err, context))
}

fn read_file_error(
    display_path: &Path,
    fs_path: &Path,
    base_dir: Option<&Path>,
    source: std::io::Error,
    context: &str,
) -> ApplyPatchError {
    ApplyPatchError::IoError(IoError {
        context: format!(
            "{context} {}",
            describe_path_for_error_with_resolved(display_path, fs_path, base_dir)
        ),
        source,
    })
}

/// Return *only* the new file contents (joined into a single `String`) after
/// applying the chunks to the file at `path`.
fn derive_new_contents_from_chunks_with_path_for_error(
    path: &Path,
    path_for_error: &str,
    chunks: &[UpdateFileChunk],
) -> std::result::Result<AppliedPatch, ApplyPatchError> {
    let original_contents = read_file_to_string(
        path,
        path,
        /*base_dir*/ None,
        "Failed to read file to update",
    )?;
    derive_new_contents_from_text(path_for_error, original_contents, chunks)
}

fn derive_new_contents_from_text(
    path_for_error: &str,
    original_contents: String,
    chunks: &[UpdateFileChunk],
) -> std::result::Result<AppliedPatch, ApplyPatchError> {
    let mut original_lines: Vec<String> = original_contents.split('\n').map(String::from).collect();

    // Drop the trailing empty element that results from the final newline so
    // that line counts match the behaviour of standard `diff`.
    if original_lines.last().is_some_and(String::is_empty) {
        original_lines.pop();
    }

    let replacements = compute_replacements(&original_lines, path_for_error, chunks)?;
    let new_lines = apply_replacements(original_lines, &replacements);
    let mut new_lines = new_lines;
    if !new_lines.last().is_some_and(String::is_empty) {
        new_lines.push(String::new());
    }
    let new_contents = new_lines.join("\n");
    Ok(AppliedPatch {
        original_contents,
        new_contents,
    })
}

fn capture_original_states(plan: &PatchPlan) -> Result<BTreeMap<PathBuf, OriginalPathState>> {
    let mut states = BTreeMap::new();

    for path in plan
        .operations
        .iter()
        .flat_map(|operation| match operation {
            PlannedOperation::Add { path, .. }
            | PlannedOperation::Delete { path }
            | PlannedOperation::Update { path, .. } => vec![path.clone()],
            PlannedOperation::Move { source, dest, .. } => vec![source.clone(), dest.clone()],
        })
    {
        states
            .entry(path.absolute().to_path_buf())
            .or_insert(capture_original_state(path.absolute())?);
    }

    Ok(states)
}

fn capture_original_state(path: &Path) -> Result<OriginalPathState> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = std::fs::read_link(path).map_err(|err| {
                anyhow::anyhow!(
                    "Failed to inspect symlink {}: {err}",
                    describe_path_for_error(path)
                )
            })?;
            let referent_path = resolve_symlink_referent_path(path, &target);
            Ok(OriginalPathState::Symlink {
                target,
                referent_path: referent_path.clone(),
                referent_contents: referent_path
                    .as_ref()
                    .and_then(|referent_path| std::fs::read_to_string(referent_path).ok()),
            })
        }
        Ok(metadata) if metadata.is_file() => Ok(OriginalPathState::File(
            std::fs::read_to_string(path).map_err(|err| {
                anyhow::anyhow!(
                    "Failed to read original file {}: {err}",
                    describe_path_for_error(path)
                )
            })?,
        )),
        Ok(metadata) if metadata.is_dir() => Ok(OriginalPathState::Directory),
        Ok(_) => Ok(OriginalPathState::Missing),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(OriginalPathState::Missing),
        Err(err) => Err(anyhow::anyhow!(
            "Failed to inspect file {}: {err}",
            describe_path_for_error(path)
        )),
    }
}

fn preview_origin_state(state: &OriginalPathState) -> PreviewOriginState {
    match state {
        OriginalPathState::Missing | OriginalPathState::Directory => PreviewOriginState::Missing,
        OriginalPathState::File(contents) => PreviewOriginState::File(contents.clone()),
        OriginalPathState::Symlink {
            referent_contents, ..
        } => PreviewOriginState::File(referent_contents.clone().unwrap_or_default()),
    }
}

fn ensure_preview_entity(
    path: &Path,
    original_states: &BTreeMap<PathBuf, OriginalPathState>,
    entities: &mut Vec<PreviewEntity>,
    current_entities: &mut HashMap<PathBuf, usize>,
) -> usize {
    if let Some(index) = current_entities.get(path) {
        return *index;
    }

    if let Some(index) = entities
        .iter()
        .position(|entity| entity.origin_path == path)
    {
        return index;
    }

    let origin_state = original_states
        .get(path)
        .map(preview_origin_state)
        .unwrap_or(PreviewOriginState::Missing);
    let current_content = match &origin_state {
        PreviewOriginState::Missing => None,
        PreviewOriginState::File(contents) => Some(contents.clone()),
    };
    let current_path = current_content.as_ref().map(|_| path.to_path_buf());
    let index = entities.len();
    entities.push(PreviewEntity {
        origin_path: path.to_path_buf(),
        origin_state,
        current_path,
        current_content,
    });
    if entities[index].current_path.is_some() {
        current_entities.insert(path.to_path_buf(), index);
    }
    index
}

fn evict_preview_path(
    path: &Path,
    original_states: &BTreeMap<PathBuf, OriginalPathState>,
    entities: &mut Vec<PreviewEntity>,
    current_entities: &mut HashMap<PathBuf, usize>,
) {
    let index = ensure_preview_entity(path, original_states, entities, current_entities);
    if entities[index].current_path.is_some() {
        current_entities.remove(path);
        entities[index].current_path = None;
        entities[index].current_content = None;
    }
}

fn build_preview_file_changes(
    plan: &PatchPlan,
    original_states: &BTreeMap<PathBuf, OriginalPathState>,
) -> HashMap<PathBuf, ApplyPatchFileChange> {
    let mut entities = Vec::new();
    let mut current_entities = HashMap::new();

    for operation in &plan.operations {
        match operation {
            PlannedOperation::Add { path, contents } => {
                evict_preview_path(
                    path.absolute(),
                    original_states,
                    &mut entities,
                    &mut current_entities,
                );
                let index = ensure_preview_entity(
                    path.absolute(),
                    original_states,
                    &mut entities,
                    &mut current_entities,
                );
                entities[index].current_path = Some(path.absolute().to_path_buf());
                entities[index].current_content = Some(contents.clone());
                current_entities.insert(path.absolute().to_path_buf(), index);
            }
            PlannedOperation::Delete { path } => {
                let index = ensure_preview_entity(
                    path.absolute(),
                    original_states,
                    &mut entities,
                    &mut current_entities,
                );
                current_entities.remove(path.absolute());
                entities[index].current_path = None;
                entities[index].current_content = None;
            }
            PlannedOperation::Update { path, new_contents } => {
                let index = ensure_preview_entity(
                    path.absolute(),
                    original_states,
                    &mut entities,
                    &mut current_entities,
                );
                entities[index].current_path = Some(path.absolute().to_path_buf());
                entities[index].current_content = Some(new_contents.clone());
                current_entities.insert(path.absolute().to_path_buf(), index);
            }
            PlannedOperation::Move {
                source,
                dest,
                new_contents,
            } => {
                let index = ensure_preview_entity(
                    source.absolute(),
                    original_states,
                    &mut entities,
                    &mut current_entities,
                );
                current_entities.remove(source.absolute());
                evict_preview_path(
                    dest.absolute(),
                    original_states,
                    &mut entities,
                    &mut current_entities,
                );
                entities[index].current_path = Some(dest.absolute().to_path_buf());
                entities[index].current_content = Some(new_contents.clone());
                current_entities.insert(dest.absolute().to_path_buf(), index);
            }
        }
    }

    let mut result = HashMap::new();
    for entity in entities {
        match entity.origin_state {
            PreviewOriginState::Missing => {
                if let (Some(path), Some(content)) = (entity.current_path, entity.current_content) {
                    result.insert(path, ApplyPatchFileChange::Add { content });
                }
            }
            PreviewOriginState::File(original_content) => {
                match (entity.current_path, entity.current_content) {
                    (None, _) => {
                        result.insert(
                            entity.origin_path,
                            ApplyPatchFileChange::Delete {
                                content: original_content,
                            },
                        );
                    }
                    (Some(path), Some(content)) => {
                        if path == entity.origin_path && content == original_content {
                            continue;
                        }
                        let unified_diff = TextDiff::from_lines(&original_content, &content)
                            .unified_diff()
                            .context_radius(1)
                            .to_string();
                        result.insert(
                            entity.origin_path.clone(),
                            ApplyPatchFileChange::Update {
                                unified_diff,
                                move_path: (path != entity.origin_path).then_some(path),
                                new_content: content,
                            },
                        );
                    }
                    (Some(_), None) => {}
                }
            }
        }
    }

    result
}

fn determine_affected_paths(
    plan: &PatchPlan,
    original_states: &BTreeMap<PathBuf, OriginalPathState>,
) -> AffectedPaths {
    let mut statuses = HashMap::new();

    for (path, final_state) in &plan.final_states {
        let Some(original_state) = original_states.get(path) else {
            continue;
        };
        let status = if plan.explicit_adds.contains(path) {
            match final_state {
                VirtualPathState::Missing => None,
                VirtualPathState::File(_) => Some(AffectedPathStatus::Added),
            }
        } else if plan.moved_destinations.contains(path) {
            match final_state {
                VirtualPathState::Missing => None,
                VirtualPathState::File(_) => Some(AffectedPathStatus::Modified),
            }
        } else {
            match (original_state, final_state) {
                (OriginalPathState::Missing, VirtualPathState::Missing)
                | (OriginalPathState::Directory, VirtualPathState::Missing)
                | (OriginalPathState::Directory, VirtualPathState::File(_)) => None,
                (OriginalPathState::Missing, VirtualPathState::File(_)) => {
                    Some(AffectedPathStatus::Added)
                }
                (OriginalPathState::File(_), VirtualPathState::Missing)
                | (OriginalPathState::Symlink { .. }, VirtualPathState::Missing) => {
                    if plan.moved_sources.contains(path) {
                        None
                    } else {
                        Some(AffectedPathStatus::Deleted)
                    }
                }
                (OriginalPathState::File(_), VirtualPathState::File(_))
                | (OriginalPathState::Symlink { .. }, VirtualPathState::File(_)) => {
                    Some(AffectedPathStatus::Modified)
                }
            }
        };

        if let Some(status) = status {
            statuses.insert(path.clone(), status);
        }
    }

    let mut added = Vec::new();
    let mut modified = Vec::new();
    let mut deleted = Vec::new();
    let mut emitted = BTreeSet::new();

    for operation in &plan.operations {
        let (absolute_path, display_path) = match operation {
            PlannedOperation::Add { path, .. }
            | PlannedOperation::Delete { path }
            | PlannedOperation::Update { path, .. } => (
                path.absolute().to_path_buf(),
                path.requested().to_path_buf(),
            ),
            PlannedOperation::Move { dest, .. } => (
                dest.absolute().to_path_buf(),
                dest.requested().to_path_buf(),
            ),
        };

        if !emitted.insert(absolute_path.clone()) {
            continue;
        }

        match statuses.get(&absolute_path) {
            Some(AffectedPathStatus::Added) => added.push(display_path),
            Some(AffectedPathStatus::Modified) => modified.push(display_path),
            Some(AffectedPathStatus::Deleted) => deleted.push(display_path),
            None => {}
        }
    }

    AffectedPaths {
        added,
        modified,
        deleted,
    }
}

fn rollback_changes(
    original_states: &BTreeMap<PathBuf, OriginalPathState>,
    created_dirs: &[PathBuf],
) -> Result<()> {
    let mut first_error = None;

    for (path, state) in original_states {
        if let Err(err) = restore_original_state(path, state)
            && first_error.is_none()
        {
            first_error = Some(err);
        }
    }

    for dir in created_dirs.iter().rev() {
        if let Err(err) = std::fs::remove_dir(dir)
            && err.kind() != std::io::ErrorKind::NotFound
            && err.kind() != std::io::ErrorKind::DirectoryNotEmpty
            && first_error.is_none()
        {
            first_error = Some(anyhow::anyhow!(
                "Failed to remove created directory {}: {err}",
                dir.display()
            ));
        }
    }

    if let Some(err) = first_error {
        return Err(err);
    }

    Ok(())
}

fn restore_original_state(path: &Path, state: &OriginalPathState) -> Result<()> {
    match state {
        OriginalPathState::Missing => remove_path_if_present(path),
        OriginalPathState::File(contents) => {
            remove_path_if_present(path)?;
            create_parent_dirs_if_needed(path, &mut Vec::new())?;
            write_file(path, contents)
        }
        OriginalPathState::Symlink {
            target,
            referent_path,
            referent_contents,
        } => {
            remove_path_if_present(path)?;
            if let (Some(referent_path), Some(contents)) =
                (referent_path.as_ref(), referent_contents.as_ref())
            {
                create_parent_dirs_if_needed(referent_path, &mut Vec::new())?;
                write_file(referent_path, contents)?;
            }
            create_parent_dirs_if_needed(path, &mut Vec::new())?;
            recreate_symlink(target, path)?;
            Ok(())
        }
        OriginalPathState::Directory => Ok(()),
    }
}

fn remove_path_if_present(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            std::fs::remove_dir_all(path).map_err(|err| {
                anyhow::anyhow!(
                    "Failed to remove directory {} during rollback: {err}",
                    describe_path_for_error(path)
                )
            })
        }
        Ok(_) => remove_file_or_symlink(path).map_err(|err| {
            anyhow::anyhow!(
                "Failed to remove file {} during rollback: {err}",
                describe_path_for_error(path)
            )
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(anyhow::anyhow!(
            "Failed to inspect {} during rollback: {err}",
            describe_path_for_error(path)
        )),
    }
}

fn recreate_symlink(target: &Path, path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, path).map_err(|err| {
            anyhow::anyhow!(
                "Failed to recreate symlink {} -> {}: {err}",
                describe_path_for_error(path),
                target.display()
            )
        })?;
    }

    #[cfg(windows)]
    {
        let create = if target.is_dir() {
            std::os::windows::fs::symlink_dir
        } else {
            std::os::windows::fs::symlink_file
        };
        create(target, path).map_err(|err| {
            anyhow::anyhow!(
                "Failed to recreate symlink {} -> {}: {err}",
                describe_path_for_error(path),
                target.display()
            )
        })?;
    }

    Ok(())
}

fn resolve_symlink_referent_path(path: &Path, target: &Path) -> Option<PathBuf> {
    if target.as_os_str().is_empty() {
        return None;
    }

    Some(if target.is_absolute() {
        target.to_path_buf()
    } else {
        path.parent().unwrap_or_else(|| Path::new("")).join(target)
    })
}

fn remove_file_or_symlink(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::fs::remove_file(path)
            .or_else(|file_err| std::fs::remove_dir(path).map_err(|_| file_err))
    }

    #[cfg(not(windows))]
    {
        std::fs::remove_file(path)
    }
}

/// Compute a list of replacements needed to transform `original_lines` into the
/// new lines, given the patch `chunks`. Each replacement is returned as
/// `(start_index, old_len, new_lines)`.
fn compute_replacements(
    original_lines: &[String],
    path_for_error: &str,
    chunks: &[UpdateFileChunk],
) -> std::result::Result<Vec<(usize, usize, Vec<String>)>, ApplyPatchError> {
    let mut replacements: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut line_index: usize = 0;

    for chunk in chunks {
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence::seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                /*eof*/ false,
            ) {
                line_index = idx + 1;
            } else {
                return Err(ApplyPatchError::ComputeReplacements(format!(
                    "Failed to find context '{ctx_line}' in {path_for_error}"
                )));
            }
        }

        if chunk.old_lines.is_empty() {
            let insertion_idx = if original_lines.last().is_some_and(String::is_empty) {
                original_lines.len() - 1
            } else {
                original_lines.len()
            };
            replacements.push((insertion_idx, 0, chunk.new_lines.clone()));
            continue;
        }

        let mut pattern: &[String] = &chunk.old_lines;
        let mut found =
            seek_sequence::seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);

        let mut new_slice: &[String] = &chunk.new_lines;

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }

            found = seek_sequence::seek_sequence(
                original_lines,
                pattern,
                line_index,
                chunk.is_end_of_file,
            );
        }

        if let Some(start_idx) = found {
            replacements.push((start_idx, pattern.len(), new_slice.to_vec()));
            line_index = start_idx + pattern.len();
        } else {
            return Err(ApplyPatchError::ComputeReplacements(format!(
                "Failed to find expected lines in {}:\n{}",
                path_for_error,
                chunk.old_lines.join("\n"),
            )));
        }
    }

    replacements.sort_by(|(lhs_idx, _, _), (rhs_idx, _, _)| lhs_idx.cmp(rhs_idx));

    Ok(replacements)
}

/// Apply the `(start_index, old_len, new_lines)` replacements to `original_lines`,
/// returning the modified file contents as a vector of lines.
fn apply_replacements(
    mut lines: Vec<String>,
    replacements: &[(usize, usize, Vec<String>)],
) -> Vec<String> {
    for (start_idx, old_len, new_segment) in replacements.iter().rev() {
        let start_idx = *start_idx;
        let old_len = *old_len;

        for _ in 0..old_len {
            if start_idx < lines.len() {
                lines.remove(start_idx);
            }
        }

        for (offset, new_line) in new_segment.iter().enumerate() {
            lines.insert(start_idx + offset, new_line.clone());
        }
    }

    lines
}

/// Intended result of a file update for apply_patch.
#[derive(Debug, Eq, PartialEq)]
pub struct ApplyPatchFileUpdate {
    pub unified_diff: String,
    pub content: String,
}

pub fn unified_diff_from_chunks(
    path: &Path,
    chunks: &[UpdateFileChunk],
) -> std::result::Result<ApplyPatchFileUpdate, ApplyPatchError> {
    unified_diff_from_chunks_with_context(path, chunks, /*context*/ 1)
}

pub fn unified_diff_from_chunks_with_context(
    path: &Path,
    chunks: &[UpdateFileChunk],
    context: usize,
) -> std::result::Result<ApplyPatchFileUpdate, ApplyPatchError> {
    unified_diff_from_chunks_with_context_and_path(
        path,
        &describe_path_for_error(path),
        chunks,
        context,
    )
}

fn unified_diff_from_chunks_with_context_and_path(
    path: &Path,
    path_for_error: &str,
    chunks: &[UpdateFileChunk],
    context: usize,
) -> std::result::Result<ApplyPatchFileUpdate, ApplyPatchError> {
    let AppliedPatch {
        original_contents,
        new_contents,
    } = derive_new_contents_from_chunks_with_path_for_error(path, path_for_error, chunks)?;
    let text_diff = TextDiff::from_lines(&original_contents, &new_contents);
    let unified_diff = text_diff.unified_diff().context_radius(context).to_string();
    Ok(ApplyPatchFileUpdate {
        unified_diff,
        content: new_contents,
    })
}

/// Write a summary of changes to the given writer.
pub fn print_summary(
    affected: &AffectedPaths,
    out: &mut impl std::io::Write,
) -> std::io::Result<()> {
    writeln!(out, "Success. Updated the following files:")?;
    for path in &affected.added {
        writeln!(out, "A {}", path.display())?;
    }
    for path in &affected.modified {
        writeln!(out, "M {}", path.display())?;
    }
    for path in &affected.deleted {
        writeln!(out, "D {}", path.display())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::describe_path_for_error;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[test]
    fn relative_path_diagnostic_includes_cwd_and_resolved_path() {
        let tempdir = tempdir().expect("tempdir");
        let original = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(tempdir.path()).expect("set_current_dir");

        let diagnostic = describe_path_for_error(std::path::Path::new("target.txt"));

        std::env::set_current_dir(original).expect("restore current_dir");
        assert_eq!(
            diagnostic,
            format!(
                "target.txt (cwd: {}, resolved: {})",
                tempdir.path().display(),
                tempdir.path().join("target.txt").display()
            )
        );
    }
}
