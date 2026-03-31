use anyhow::Context;
use anyhow::Result;
use codex_app_server_protocol::SessionSource;
use codex_app_server_protocol::ThreadRolloutAnalyzeResponse;
use codex_app_server_protocol::ThreadRolloutBackupDeleteResponse;
use codex_app_server_protocol::ThreadRolloutBackupInfo;
use codex_app_server_protocol::ThreadRolloutBackupRestoreResponse;
use codex_app_server_protocol::ThreadRolloutStats;
use codex_app_server_protocol::ThreadRolloutTrimPreview;
use codex_app_server_protocol::ThreadRolloutTrimResponse;
use codex_core::parse_turn_item;
use codex_protocol::items::TurnItem;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::RolloutItem;
use codex_protocol::protocol::SessionMetaLine;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::io::copy;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const BACKUP_ROOT_DIR: &str = ".nerobar-session-rollout-backups";

#[derive(Debug, Clone)]
pub(crate) struct RolloutLocation {
    pub path: PathBuf,
    pub archived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupManifest {
    thread_id: String,
    original_rollout_path: PathBuf,
    created_at: String,
    source: Option<SessionSource>,
    forked_from_id: Option<String>,
    #[serde(default)]
    is_subagent: bool,
    original_total_lines: u64,
    original_total_bytes: u64,
    original_parse_errors: Option<u64>,
    trimmed_total_lines: u64,
    trimmed_total_bytes: u64,
    trimmed_parse_errors: Option<u64>,
    protected_head_end_line: u64,
    requested_tail_lines: u32,
    actual_tail_start_line: u64,
    analysis_fingerprint: String,
}

#[derive(Debug, Clone)]
struct ScanSummary {
    session_meta: Option<SessionMetaLine>,
    total_lines: u64,
    total_bytes: u64,
    parse_errors: u64,
    content_sha256: String,
    prefix_bytes: Vec<u64>,
    effective_user_boundaries: Vec<u64>,
    compaction_anchor_lines: Vec<u64>,
}

#[derive(Debug, Clone)]
struct BuiltTrimPreview {
    preview: ThreadRolloutTrimPreview,
    fingerprint: Option<String>,
}

#[derive(Debug, Clone)]
struct RolloutIdentity {
    len: u64,
    modified_nanos: u128,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

fn backup_root(codex_home: &Path, thread_id: &str) -> PathBuf {
    codex_home.join(BACKUP_ROOT_DIR).join(thread_id)
}

fn backup_manifest_path(codex_home: &Path, thread_id: &str) -> PathBuf {
    backup_root(codex_home, thread_id).join("manifest.json")
}

fn backup_rollout_path(codex_home: &Path, thread_id: &str) -> PathBuf {
    backup_root(codex_home, thread_id).join("original-rollout.jsonl")
}

fn backup_staging_root(codex_home: &Path, thread_id: &str) -> PathBuf {
    codex_home.join(BACKUP_ROOT_DIR).join(format!(
        "{thread_id}.staging-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ))
}

fn timestamp_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn file_timestamp_nanos(system_time: SystemTime) -> u128 {
    system_time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn read_rollout_identity(path: &Path) -> Result<RolloutIdentity> {
    let metadata =
        fs::metadata(path).with_context(|| format!("failed to stat rollout {}", path.display()))?;
    Ok(read_rollout_identity_from_metadata(&metadata))
}

fn read_rollout_identity_from_metadata(metadata: &fs::Metadata) -> RolloutIdentity {
    let modified_nanos = metadata
        .modified()
        .ok()
        .map(file_timestamp_nanos)
        .unwrap_or_default();
    RolloutIdentity {
        len: metadata.len(),
        modified_nanos,
        #[cfg(unix)]
        dev: metadata.dev(),
        #[cfg(unix)]
        ino: metadata.ino(),
    }
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    let mut output = String::with_capacity(bytes.as_ref().len() * 2);
    for byte in bytes.as_ref() {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn file_name_is_supported_rollout(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
}

fn canonicalize_if_exists(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(fs::canonicalize(path).with_context(|| {
        format!("failed to canonicalize {}", path.display())
    })?))
}

fn is_supported_rollout_path(codex_home: &Path, path: &Path) -> Result<bool> {
    if !file_name_is_supported_rollout(path) {
        return Ok(false);
    }
    let Some(canonical_path) = canonicalize_if_exists(path)? else {
        return Ok(false);
    };
    let sessions_root = codex_home.join("sessions");
    let archived_root = codex_home.join("archived_sessions");
    let sessions_root = canonicalize_if_exists(&sessions_root)?.unwrap_or(sessions_root);
    let archived_root = canonicalize_if_exists(&archived_root)?.unwrap_or(archived_root);
    Ok(canonical_path.starts_with(&sessions_root) || canonical_path.starts_with(&archived_root))
}

fn resolve_rollout_target_path(path: &Path) -> Result<PathBuf> {
    if let Some(canonical_path) = canonicalize_if_exists(path)? {
        return Ok(canonical_path);
    }
    let file_name = path
        .file_name()
        .context("rollout target path is missing a file name")?;
    let parent = path
        .parent()
        .context("rollout target path is missing a parent directory")?;
    let resolved_parent = canonicalize_if_exists(parent)?.unwrap_or_else(|| parent.to_path_buf());
    Ok(resolved_parent.join(file_name))
}

fn is_supported_rollout_target_path(codex_home: &Path, path: &Path) -> Result<bool> {
    if !file_name_is_supported_rollout(path) {
        return Ok(false);
    }
    let resolved_path = resolve_rollout_target_path(path)?;
    let sessions_root = codex_home.join("sessions");
    let archived_root = codex_home.join("archived_sessions");
    let sessions_root = canonicalize_if_exists(&sessions_root)?.unwrap_or(sessions_root);
    let archived_root = canonicalize_if_exists(&archived_root)?.unwrap_or(archived_root);
    Ok(resolved_path.starts_with(&sessions_root) || resolved_path.starts_with(&archived_root))
}

fn read_existing_backup(
    codex_home: &Path,
    thread_id: &str,
) -> Result<Option<ThreadRolloutBackupInfo>> {
    let manifest_path = backup_manifest_path(codex_home, thread_id);
    let rollout_path = backup_rollout_path(codex_home, thread_id);
    if !manifest_path.exists() && !rollout_path.exists() {
        return Ok(None);
    }
    let manifest_text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read backup manifest {}", manifest_path.display()))?;
    let manifest: BackupManifest = serde_json::from_str(&manifest_text).with_context(|| {
        format!(
            "failed to parse backup manifest {}",
            manifest_path.display()
        )
    })?;
    if !rollout_path.exists() {
        anyhow::bail!("backup rollout is missing at {}", rollout_path.display());
    }
    Ok(Some(ThreadRolloutBackupInfo {
        backup_root: backup_root(codex_home, thread_id),
        manifest_path,
        backup_rollout_path: rollout_path,
        original_rollout_path: manifest.original_rollout_path,
        created_at: manifest.created_at,
        original_total_lines: manifest.original_total_lines,
        original_total_bytes: manifest.original_total_bytes,
        trimmed_total_lines: manifest.trimmed_total_lines,
        trimmed_total_bytes: manifest.trimmed_total_bytes,
        protected_head_end_line: manifest.protected_head_end_line,
        requested_tail_lines: manifest.requested_tail_lines,
        actual_tail_start_line: manifest.actual_tail_start_line,
        analysis_fingerprint: manifest.analysis_fingerprint,
    }))
}

fn scan_rollout_reader<R: BufRead>(mut reader: R, path: &Path) -> Result<ScanSummary> {
    let mut buffer = Vec::new();
    let mut hasher = Sha256::new();
    let mut summary = ScanSummary {
        session_meta: None,
        total_lines: 0,
        total_bytes: 0,
        parse_errors: 0,
        content_sha256: String::new(),
        prefix_bytes: vec![0],
        effective_user_boundaries: Vec::new(),
        compaction_anchor_lines: Vec::new(),
    };

    loop {
        buffer.clear();
        let read = reader
            .read_until(b'\n', &mut buffer)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer);
        summary.total_lines += 1;
        summary.total_bytes += u64::try_from(read).unwrap_or(u64::MAX);
        summary.prefix_bytes.push(summary.total_bytes);

        let mut parse_slice = buffer.as_slice();
        while parse_slice
            .last()
            .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
        {
            parse_slice = &parse_slice[..parse_slice.len().saturating_sub(1)];
        }
        if parse_slice.is_empty() {
            summary.parse_errors += 1;
            continue;
        }

        match serde_json::from_slice::<RolloutItem>(parse_slice) {
            Ok(item) => match item {
                RolloutItem::SessionMeta(meta) => {
                    if summary.session_meta.is_none() {
                        summary.session_meta = Some(meta);
                    }
                }
                RolloutItem::ResponseItem(message) => {
                    if matches!(parse_turn_item(&message), Some(TurnItem::UserMessage(_))) {
                        summary.effective_user_boundaries.push(summary.total_lines);
                    }
                }
                RolloutItem::EventMsg(EventMsg::ThreadRolledBack(event)) => {
                    let shrink_by = usize::try_from(event.num_turns).unwrap_or(usize::MAX);
                    let new_len = summary
                        .effective_user_boundaries
                        .len()
                        .saturating_sub(shrink_by);
                    summary.effective_user_boundaries.truncate(new_len);
                }
                RolloutItem::Compacted(compacted) => {
                    if compacted.replacement_history.is_some() {
                        summary.compaction_anchor_lines.push(summary.total_lines);
                    }
                }
                RolloutItem::TurnContext(_) | RolloutItem::EventMsg(_) => {}
            },
            Err(_) => {
                summary.parse_errors += 1;
            }
        }
    }

    summary.content_sha256 = hex_digest(hasher.finalize());
    Ok(summary)
}

fn scan_rollout(path: &Path) -> Result<ScanSummary> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    scan_rollout_reader(BufReader::new(file), path)
}

fn compute_analysis_fingerprint(
    thread_id: &str,
    path: &Path,
    scan: &ScanSummary,
    identity: &RolloutIdentity,
    preview: &ThreadRolloutTrimPreview,
) -> String {
    #[cfg(unix)]
    let identity_dev = identity.dev;
    #[cfg(unix)]
    let identity_ino = identity.ino;
    #[cfg(not(unix))]
    let identity_dev = 0u64;
    #[cfg(not(unix))]
    let identity_ino = 0u64;

    let digest = Sha256::digest(
        format!(
            "{thread_id}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}",
            path.display(),
            scan.total_lines,
            scan.total_bytes,
            scan.content_sha256,
            identity.len,
            identity.modified_nanos,
            identity_dev,
            identity_ino,
            preview.protected_head_end_line,
            preview.requested_tail_lines,
            preview.requested_tail_start_line,
            preview.actual_tail_start_line,
            preview.removed_middle_lines,
            preview.estimated_removed_bytes,
        )
        .as_bytes(),
    );
    hex_digest(digest)
}

fn build_trim_preview(
    thread_id: &str,
    path: &Path,
    scan: &ScanSummary,
    keep_tail_lines: u32,
) -> BuiltTrimPreview {
    let protected_head_end_line = scan
        .effective_user_boundaries
        .first()
        .copied()
        .map(|line| line.saturating_sub(1))
        .unwrap_or(scan.total_lines);
    let requested_tail_lines = keep_tail_lines;
    let requested_tail_start_line = if scan.total_lines > u64::from(keep_tail_lines) {
        scan.total_lines - u64::from(keep_tail_lines) + 1
    } else {
        1
    };
    let mut actual_tail_start_line = scan
        .effective_user_boundaries
        .iter()
        .copied()
        .take_while(|line| *line <= requested_tail_start_line)
        .last()
        .unwrap_or(protected_head_end_line.saturating_add(1));
    let compaction_anchor_line = scan
        .compaction_anchor_lines
        .iter()
        .copied()
        .take_while(|line| *line <= requested_tail_start_line)
        .last();
    if let Some(anchor_line) = compaction_anchor_line {
        actual_tail_start_line = actual_tail_start_line.min(anchor_line);
    }
    actual_tail_start_line = actual_tail_start_line.max(protected_head_end_line.saturating_add(1));

    let removed_middle_start_line =
        if actual_tail_start_line > protected_head_end_line.saturating_add(1) {
            Some(protected_head_end_line.saturating_add(1))
        } else {
            None
        };
    let removed_middle_end_line =
        if actual_tail_start_line > protected_head_end_line.saturating_add(1) {
            Some(actual_tail_start_line.saturating_sub(1))
        } else {
            None
        };
    let removed_middle_lines = match (removed_middle_start_line, removed_middle_end_line) {
        (Some(start), Some(end)) if end >= start => end - start + 1,
        _ => 0,
    };
    let estimated_removed_bytes = match (removed_middle_start_line, removed_middle_end_line) {
        (Some(start), Some(end)) if end >= start => {
            let start_idx = usize::try_from(start.saturating_sub(1)).unwrap_or(0);
            let end_idx = usize::try_from(end).unwrap_or(scan.prefix_bytes.len().saturating_sub(1));
            scan.prefix_bytes[end_idx].saturating_sub(scan.prefix_bytes[start_idx])
        }
        _ => 0,
    };
    let preserved_tail_lines = if actual_tail_start_line <= scan.total_lines {
        scan.total_lines - actual_tail_start_line + 1
    } else {
        0
    };

    let preview = ThreadRolloutTrimPreview {
        protected_head_end_line,
        requested_tail_lines,
        requested_tail_start_line,
        actual_tail_start_line,
        preserved_tail_lines,
        removed_middle_start_line,
        removed_middle_end_line,
        removed_middle_lines,
        estimated_removed_bytes,
        compaction_anchor_line,
    };
    let fingerprint = if removed_middle_lines > 0 {
        let rollout_identity = read_rollout_identity(path).ok();
        rollout_identity.map(|identity| {
            compute_analysis_fingerprint(thread_id, path, scan, &identity, &preview)
        })
    } else {
        None
    };

    BuiltTrimPreview {
        preview,
        fingerprint,
    }
}

fn scan_session_identity(scan: &ScanSummary) -> (Option<SessionSource>, Option<String>, bool) {
    let source = scan
        .session_meta
        .as_ref()
        .map(|meta| SessionSource::from(meta.meta.source.clone()));
    let forked_from_id = scan
        .session_meta
        .as_ref()
        .and_then(|meta| meta.meta.forked_from_id.as_ref())
        .map(ToString::to_string);
    let is_subagent = matches!(source, Some(SessionSource::SubAgent(_)));
    (source, forked_from_id, is_subagent)
}

pub(crate) fn analyze_rollout(
    codex_home: &Path,
    thread_id: &str,
    thread_name: Option<String>,
    location: Option<RolloutLocation>,
    keep_tail_lines: u32,
    loaded: bool,
) -> Result<ThreadRolloutAnalyzeResponse> {
    let backup = read_existing_backup(codex_home, thread_id)?;
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    if loaded {
        blockers.push("thread_loaded".to_string());
    }
    if backup.is_some() {
        blockers.push("existing_backup_present".to_string());
    }

    let Some(location) = location else {
        blockers.push("thread_rollout_not_found".to_string());
        return Ok(ThreadRolloutAnalyzeResponse {
            thread_id: thread_id.to_string(),
            thread_name,
            rollout_path: None,
            archived: false,
            source: None,
            forked_from_id: None,
            is_subagent: false,
            loaded,
            eligible: false,
            blockers,
            warnings,
            stats: None,
            trim: None,
            analysis_fingerprint: None,
            backup,
        });
    };

    if !is_supported_rollout_path(codex_home, &location.path)? {
        blockers.push("unsupported_rollout_path".to_string());
        return Ok(ThreadRolloutAnalyzeResponse {
            thread_id: thread_id.to_string(),
            thread_name,
            rollout_path: Some(location.path),
            archived: location.archived,
            source: None,
            forked_from_id: None,
            is_subagent: false,
            loaded,
            eligible: false,
            blockers,
            warnings,
            stats: None,
            trim: None,
            analysis_fingerprint: None,
            backup,
        });
    }

    let scan = scan_rollout(&location.path)?;
    let (source, forked_from_id, is_subagent) = scan_session_identity(&scan);
    if is_subagent {
        blockers.push("subagent_session_not_supported".to_string());
    }
    if scan.parse_errors > 0 {
        warnings.push(format!(
            "rollout_parse_errors:{count}",
            count = scan.parse_errors
        ));
    }

    let BuiltTrimPreview {
        preview,
        fingerprint,
    } = build_trim_preview(thread_id, &location.path, &scan, keep_tail_lines);
    if preview.removed_middle_lines == 0 {
        blockers.push("nothing_to_trim".to_string());
    }

    Ok(ThreadRolloutAnalyzeResponse {
        thread_id: thread_id.to_string(),
        thread_name,
        rollout_path: Some(location.path),
        archived: location.archived,
        source,
        forked_from_id,
        is_subagent,
        loaded,
        eligible: blockers.is_empty(),
        blockers,
        warnings,
        stats: Some(ThreadRolloutStats {
            total_lines: scan.total_lines,
            total_bytes: scan.total_bytes,
            parse_errors: scan.parse_errors,
        }),
        trim: Some(preview),
        analysis_fingerprint: fingerprint,
        backup,
    })
}

fn create_backup_manifest(
    original_path: &Path,
    analysis: &ThreadRolloutAnalyzeResponse,
    trimmed_stats: Option<&ThreadRolloutStats>,
) -> Result<BackupManifest> {
    let stats = analysis
        .stats
        .as_ref()
        .context("analysis stats missing for backup manifest")?;
    let trim = analysis
        .trim
        .as_ref()
        .context("analysis trim preview missing for backup manifest")?;
    let fingerprint = analysis
        .analysis_fingerprint
        .clone()
        .context("analysis fingerprint missing for backup manifest")?;
    let (trimmed_total_lines, trimmed_total_bytes, trimmed_parse_errors) = match trimmed_stats {
        Some(trimmed_stats) => (
            trimmed_stats.total_lines,
            trimmed_stats.total_bytes,
            Some(trimmed_stats.parse_errors),
        ),
        None => (
            stats.total_lines.saturating_sub(trim.removed_middle_lines),
            stats
                .total_bytes
                .saturating_sub(trim.estimated_removed_bytes),
            None,
        ),
    };
    Ok(BackupManifest {
        thread_id: analysis.thread_id.clone(),
        original_rollout_path: original_path.to_path_buf(),
        created_at: timestamp_rfc3339(),
        source: analysis.source.clone(),
        forked_from_id: analysis.forked_from_id.clone(),
        is_subagent: analysis.is_subagent,
        original_total_lines: stats.total_lines,
        original_total_bytes: stats.total_bytes,
        original_parse_errors: Some(stats.parse_errors),
        trimmed_total_lines,
        trimmed_total_bytes,
        trimmed_parse_errors,
        protected_head_end_line: trim.protected_head_end_line,
        requested_tail_lines: trim.requested_tail_lines,
        actual_tail_start_line: trim.actual_tail_start_line,
        analysis_fingerprint: fingerprint,
    })
}

fn write_backup_manifest(manifest_path: &Path, manifest: &BackupManifest) -> Result<()> {
    fs::write(
        manifest_path,
        format!("{}\n", serde_json::to_string_pretty(manifest)?),
    )
    .with_context(|| format!("failed to write {}", manifest_path.display()))
}

fn build_restore_fallback_analysis(
    thread_id: &str,
    thread_name: Option<String>,
    expected_target: PathBuf,
    archived: bool,
    loaded: bool,
    backup: &ThreadRolloutBackupInfo,
    error: &anyhow::Error,
) -> ThreadRolloutAnalyzeResponse {
    let manifest = fs::read_to_string(&backup.manifest_path)
        .ok()
        .and_then(|manifest_text| serde_json::from_str::<BackupManifest>(&manifest_text).ok());
    let manifest_parse_errors = manifest
        .as_ref()
        .and_then(|manifest| manifest.original_parse_errors);
    let fallback_scan = scan_rollout(&expected_target).ok();
    let (source, forked_from_id, is_subagent) = fallback_scan
        .as_ref()
        .map(scan_session_identity)
        .unwrap_or_else(|| {
            (
                manifest
                    .as_ref()
                    .and_then(|manifest| manifest.source.clone()),
                manifest
                    .as_ref()
                    .and_then(|manifest| manifest.forked_from_id.clone()),
                manifest
                    .as_ref()
                    .is_some_and(|manifest| manifest.is_subagent),
            )
        });
    let mut warnings = vec![format!(
        "restore completed but post-restore analysis failed: {error}"
    )];
    if let Some(parse_errors) = fallback_scan
        .as_ref()
        .map(|scan| scan.parse_errors)
        .or(manifest_parse_errors)
        .filter(|count| *count > 0)
    {
        warnings.push(format!("rollout_parse_errors:{parse_errors}"));
    }
    ThreadRolloutAnalyzeResponse {
        thread_id: thread_id.to_string(),
        thread_name,
        rollout_path: Some(expected_target),
        archived,
        source,
        forked_from_id,
        is_subagent,
        loaded,
        eligible: false,
        blockers: vec!["existing_backup_present".to_string()],
        warnings,
        stats: fallback_scan
            .as_ref()
            .map(|scan| ThreadRolloutStats {
                total_lines: scan.total_lines,
                total_bytes: scan.total_bytes,
                parse_errors: scan.parse_errors,
            })
            .or_else(|| {
                manifest_parse_errors.map(|parse_errors| ThreadRolloutStats {
                    total_lines: backup.original_total_lines,
                    total_bytes: backup.original_total_bytes,
                    parse_errors,
                })
            }),
        trim: None,
        analysis_fingerprint: None,
        backup: Some(backup.clone()),
    }
}

fn copy_trimmed_rollout_from_reader<R: BufRead>(
    mut reader: R,
    temp_path: &Path,
    trim: &ThreadRolloutTrimPreview,
) -> Result<ThreadRolloutStats> {
    let target = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(temp_path)
        .with_context(|| format!("failed to create {}", temp_path.display()))?;
    let mut writer = BufWriter::new(target);
    let mut buffer = Vec::new();
    let mut line_no = 0u64;
    let mut kept_stats = ThreadRolloutStats {
        total_lines: 0,
        total_bytes: 0,
        parse_errors: 0,
    };

    loop {
        buffer.clear();
        let read = reader.read_until(b'\n', &mut buffer).with_context(|| {
            format!("failed to read rollout stream for {}", temp_path.display())
        })?;
        if read == 0 {
            break;
        }
        line_no += 1;
        if line_no <= trim.protected_head_end_line || line_no >= trim.actual_tail_start_line {
            kept_stats.total_lines += 1;
            kept_stats.total_bytes += u64::try_from(read).unwrap_or(u64::MAX);
            let mut parse_slice = buffer.as_slice();
            while parse_slice
                .last()
                .is_some_and(|byte| *byte == b'\n' || *byte == b'\r')
            {
                parse_slice = &parse_slice[..parse_slice.len().saturating_sub(1)];
            }
            if parse_slice.is_empty() || serde_json::from_slice::<RolloutItem>(parse_slice).is_err()
            {
                kept_stats.parse_errors += 1;
            }
            writer
                .write_all(&buffer)
                .with_context(|| format!("failed to write {}", temp_path.display()))?;
        }
    }
    writer
        .flush()
        .with_context(|| format!("failed to flush {}", temp_path.display()))?;
    writer
        .get_ref()
        .sync_all()
        .with_context(|| format!("failed to sync {}", temp_path.display()))?;
    Ok(kept_stats)
}

pub(crate) fn trim_rollout(
    codex_home: &Path,
    thread_id: &str,
    thread_name: Option<String>,
    location: RolloutLocation,
    keep_tail_lines: u32,
    expected_fingerprint: &str,
    loaded: bool,
) -> Result<ThreadRolloutTrimResponse> {
    let analysis = analyze_rollout(
        codex_home,
        thread_id,
        thread_name,
        Some(location.clone()),
        keep_tail_lines,
        loaded,
    )?;
    if !analysis.eligible {
        anyhow::bail!(
            "thread rollout trim is blocked: {}",
            analysis.blockers.join(",")
        );
    }
    let analysis_fingerprint = analysis
        .analysis_fingerprint
        .clone()
        .context("analysis fingerprint missing")?;
    if analysis_fingerprint != expected_fingerprint {
        anyhow::bail!("analysis fingerprint mismatch");
    }
    let source_file = File::open(&location.path)
        .with_context(|| format!("failed to open {}", location.path.display()))?;
    let verified_identity = read_rollout_identity_from_metadata(
        &source_file
            .metadata()
            .with_context(|| format!("failed to stat {}", location.path.display()))?,
    );
    let mut scan_file = source_file
        .try_clone()
        .context("failed to clone rollout file handle for scan")?;
    scan_file
        .seek(SeekFrom::Start(0))
        .context("failed to rewind rollout file handle for scan")?;
    let verified_scan = scan_rollout_reader(BufReader::new(scan_file), &location.path)?;
    let verified_preview =
        build_trim_preview(thread_id, &location.path, &verified_scan, keep_tail_lines);
    let verified_fingerprint = verified_preview
        .fingerprint
        .clone()
        .context("analysis fingerprint missing")?;
    if verified_fingerprint != expected_fingerprint {
        anyhow::bail!("analysis fingerprint mismatch");
    }
    let trim = &verified_preview.preview;
    let backup_root = backup_root(codex_home, thread_id);
    if backup_root.exists() {
        anyhow::bail!("thread rollout trim is blocked: existing_backup_present");
    }
    let staging_root = backup_staging_root(codex_home, thread_id);
    if staging_root.exists() {
        fs::remove_dir_all(&staging_root)
            .with_context(|| format!("failed to clear stale staging {}", staging_root.display()))?;
    }
    fs::create_dir_all(&staging_root)
        .with_context(|| format!("failed to create {}", staging_root.display()))?;
    let backup_rollout = staging_root.join("original-rollout.jsonl");
    let manifest_path = staging_root.join("manifest.json");
    let backup_result: Result<()> = (|| {
        let mut backup_file = source_file
            .try_clone()
            .context("failed to clone rollout file handle for backup")?;
        backup_file
            .seek(SeekFrom::Start(0))
            .context("failed to rewind rollout file handle for backup")?;
        let mut backup_reader = BufReader::new(backup_file);
        let backup_target = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&backup_rollout)
            .with_context(|| format!("failed to create {}", backup_rollout.display()))?;
        let mut backup_writer = BufWriter::new(backup_target);
        copy(&mut backup_reader, &mut backup_writer).with_context(|| {
            format!(
                "failed to create backup {} from {}",
                backup_rollout.display(),
                location.path.display()
            )
        })?;
        backup_writer
            .flush()
            .with_context(|| format!("failed to flush {}", backup_rollout.display()))?;
        backup_writer
            .get_ref()
            .sync_all()
            .with_context(|| format!("failed to sync {}", backup_rollout.display()))?;
        let manifest = create_backup_manifest(&location.path, &analysis, None)?;
        write_backup_manifest(&manifest_path, &manifest)?;
        fs::rename(&staging_root, &backup_root).with_context(|| {
            format!(
                "failed to finalize backup {} from staging {}",
                backup_root.display(),
                staging_root.display()
            )
        })?;
        Ok(())
    })();
    if let Err(error) = backup_result {
        if staging_root.exists() {
            let _ = fs::remove_dir_all(&staging_root);
        }
        return Err(error);
    }

    let temp_path = location.path.with_extension(format!(
        "trim-tmp-{}-{}.jsonl",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let rewrite_result: Result<ThreadRolloutStats> = (|| {
        let trimmed_stats = copy_trimmed_rollout_from_reader(
            {
                let mut trim_file = source_file
                    .try_clone()
                    .context("failed to clone rollout file handle for trim")?;
                trim_file
                    .seek(SeekFrom::Start(0))
                    .context("failed to rewind rollout file handle for trim")?;
                BufReader::new(trim_file)
            },
            &temp_path,
            trim,
        )?;
        let current_identity = read_rollout_identity(&location.path)?;
        if current_identity.len != verified_identity.len
            || current_identity.modified_nanos != verified_identity.modified_nanos
            || {
                #[cfg(unix)]
                {
                    current_identity.dev != verified_identity.dev
                        || current_identity.ino != verified_identity.ino
                }
                #[cfg(not(unix))]
                {
                    false
                }
            }
        {
            anyhow::bail!("analysis fingerprint mismatch");
        }
        fs::rename(&temp_path, &location.path).with_context(|| {
            format!(
                "failed to replace {} with {}",
                location.path.display(),
                temp_path.display()
            )
        })?;
        Ok(trimmed_stats)
    })();
    if let Err(error) = rewrite_result {
        if temp_path.exists() {
            let _ = fs::remove_file(&temp_path);
        }
        if backup_root.exists() {
            let _ = fs::remove_dir_all(&backup_root);
        }
        return Err(error);
    }

    let trimmed_stats = rewrite_result?;
    let final_manifest = create_backup_manifest(&location.path, &analysis, Some(&trimmed_stats))?;
    let mut warnings = Vec::new();
    if trimmed_stats.parse_errors > 0 {
        warnings.push(format!(
            "rollout_parse_errors:{}",
            trimmed_stats.parse_errors
        ));
    }
    let final_manifest_path = backup_manifest_path(codex_home, thread_id);
    if let Err(error) = write_backup_manifest(&final_manifest_path, &final_manifest) {
        warnings.push(format!(
            "trim completed but backup manifest refresh failed: {error}"
        ));
    }
    let backup = ThreadRolloutBackupInfo {
        backup_root,
        manifest_path: final_manifest_path,
        backup_rollout_path: backup_rollout_path(codex_home, thread_id),
        original_rollout_path: final_manifest.original_rollout_path.clone(),
        created_at: final_manifest.created_at.clone(),
        original_total_lines: final_manifest.original_total_lines,
        original_total_bytes: final_manifest.original_total_bytes,
        trimmed_total_lines: final_manifest.trimmed_total_lines,
        trimmed_total_bytes: final_manifest.trimmed_total_bytes,
        protected_head_end_line: final_manifest.protected_head_end_line,
        requested_tail_lines: final_manifest.requested_tail_lines,
        actual_tail_start_line: final_manifest.actual_tail_start_line,
        analysis_fingerprint: final_manifest.analysis_fingerprint,
    };
    let analysis = ThreadRolloutAnalyzeResponse {
        thread_id: thread_id.to_string(),
        thread_name: analysis.thread_name.clone(),
        rollout_path: Some(location.path),
        archived: location.archived,
        source: analysis.source.clone(),
        forked_from_id: analysis.forked_from_id.clone(),
        is_subagent: analysis.is_subagent,
        loaded,
        eligible: false,
        blockers: vec!["existing_backup_present".to_string()],
        warnings,
        stats: Some(trimmed_stats),
        trim: analysis.trim,
        analysis_fingerprint: None,
        backup: Some(backup.clone()),
    };
    Ok(ThreadRolloutTrimResponse {
        thread_id: thread_id.to_string(),
        backup,
        analysis,
    })
}

pub(crate) fn restore_rollout_backup(
    codex_home: &Path,
    thread_id: &str,
    thread_name: Option<String>,
    expected_location: Option<RolloutLocation>,
    loaded: bool,
) -> Result<ThreadRolloutBackupRestoreResponse> {
    if loaded {
        anyhow::bail!("thread rollout restore is blocked: thread_loaded");
    }
    let backup = read_existing_backup(codex_home, thread_id)?
        .context("no rollout backup exists for this thread")?;
    let original_path = backup.original_rollout_path.clone();
    let requested_path = expected_location
        .as_ref()
        .map(|location| location.path.clone())
        .unwrap_or_else(|| original_path.clone());
    if !is_supported_rollout_target_path(codex_home, &original_path)?
        || !is_supported_rollout_target_path(codex_home, &requested_path)?
    {
        anyhow::bail!(
            "backup restore target is not a supported rollout path for thread {thread_id}: {}",
            original_path.display()
        );
    }
    let expected_target = resolve_rollout_target_path(&requested_path)?;
    let manifest_target = resolve_rollout_target_path(&original_path)?;
    if manifest_target != expected_target {
        anyhow::bail!(
            "backup restore target does not match the current rollout path for thread {thread_id}: manifest={}, expected={}",
            original_path.display(),
            requested_path.display()
        );
    }
    let restore_parent = expected_target
        .parent()
        .context("restore target is missing a parent directory")?;
    fs::create_dir_all(restore_parent).with_context(|| {
        format!(
            "failed to create restore parent directory {}",
            restore_parent.display()
        )
    })?;
    let temp_path = expected_target.with_extension(format!(
        "restore-tmp-{}-{}.jsonl",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::copy(&backup.backup_rollout_path, &temp_path).with_context(|| {
        format!(
            "failed to stage restore from {} to {}",
            backup.backup_rollout_path.display(),
            temp_path.display()
        )
    })?;
    fs::rename(&temp_path, &expected_target).with_context(|| {
        format!(
            "failed to restore {} from {}",
            expected_target.display(),
            temp_path.display()
        )
    })?;
    let archived = expected_location
        .as_ref()
        .map(|location| location.archived)
        .unwrap_or_else(|| requested_path.starts_with(codex_home.join("archived_sessions")));
    let analysis = match analyze_rollout(
        codex_home,
        thread_id,
        thread_name.clone(),
        Some(RolloutLocation {
            archived,
            path: expected_target.clone(),
        }),
        backup.requested_tail_lines,
        loaded,
    ) {
        Ok(analysis) => analysis,
        Err(error) => build_restore_fallback_analysis(
            thread_id,
            thread_name,
            expected_target,
            archived,
            loaded,
            &backup,
            &error,
        ),
    };
    Ok(ThreadRolloutBackupRestoreResponse {
        thread_id: thread_id.to_string(),
        backup,
        analysis,
    })
}

pub(crate) fn delete_rollout_backup(
    codex_home: &Path,
    thread_id: &str,
) -> Result<ThreadRolloutBackupDeleteResponse> {
    let root = backup_root(codex_home, thread_id);
    if !root.exists() {
        anyhow::bail!("no rollout backup exists for this thread");
    }
    fs::remove_dir_all(&root).with_context(|| format!("failed to delete {}", root.display()))?;
    Ok(ThreadRolloutBackupDeleteResponse {
        thread_id: thread_id.to_string(),
        deleted: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use codex_protocol::ThreadId;
    use codex_protocol::models::ContentItem;
    use codex_protocol::models::ResponseItem;
    use codex_protocol::protocol::CompactedItem;
    use codex_protocol::protocol::SessionMeta;
    use codex_protocol::protocol::SessionMetaLine;
    use codex_protocol::protocol::SessionSource as CoreSessionSource;
    use codex_protocol::protocol::SubAgentSource;
    use codex_protocol::protocol::ThreadRolledBackEvent;
    use tempfile::TempDir;
    use uuid::Uuid;

    fn test_rollout_path(home: &Path, thread_id: &str) -> PathBuf {
        home.join("sessions/2026/03/29")
            .join(format!("rollout-2026-03-29T12-00-00-{thread_id}.jsonl"))
    }

    fn write_rollout(home: &Path, source: CoreSessionSource) -> Result<(String, PathBuf)> {
        let thread_id = Uuid::new_v4().to_string();
        let thread_uuid = ThreadId::from_string(&thread_id)?;
        let path = test_rollout_path(home, &thread_id);
        fs::create_dir_all(path.parent().context("missing rollout parent")?)?;
        let lines = vec![
            serde_json::to_string(&RolloutItem::SessionMeta(SessionMetaLine {
                meta: SessionMeta {
                    id: thread_uuid,
                    forked_from_id: None,
                    timestamp: "2026-03-29T12:00:00Z".to_string(),
                    cwd: PathBuf::from("/workspace/test"),
                    originator: "codex".to_string(),
                    cli_version: "0.0.0".to_string(),
                    source,
                    agent_nickname: None,
                    agent_role: None,
                    model_provider: Some("mock".to_string()),
                    base_instructions: None,
                    dynamic_tools: None,
                    memory_mode: None,
                },
                git: None,
            }))?,
            serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
                id: None,
                role: "developer".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "seed".to_string(),
                }],
                end_turn: None,
                phase: None,
            }))?,
            serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "<environment_context>ignored</environment_context>".to_string(),
                }],
                end_turn: None,
                phase: None,
            }))?,
            serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "real user one".to_string(),
                }],
                end_turn: None,
                phase: None,
            }))?,
            serde_json::to_string(&RolloutItem::Compacted(CompactedItem {
                message: "checkpoint".to_string(),
                replacement_history: Some(vec![ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "summary".to_string(),
                    }],
                    end_turn: None,
                    phase: None,
                }]),
            }))?,
            serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "assistant one".to_string(),
                }],
                end_turn: None,
                phase: None,
            }))?,
            serde_json::to_string(&RolloutItem::EventMsg(EventMsg::ThreadRolledBack(
                ThreadRolledBackEvent { num_turns: 0 },
            )))?,
            serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "real user two".to_string(),
                }],
                end_turn: None,
                phase: None,
            }))?,
            serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "assistant two".to_string(),
                }],
                end_turn: None,
                phase: None,
            }))?,
            serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: "real user three".to_string(),
                }],
                end_turn: None,
                phase: None,
            }))?,
            serde_json::to_string(&RolloutItem::ResponseItem(ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: "assistant three".to_string(),
                }],
                end_turn: None,
                phase: None,
            }))?,
        ];
        fs::write(&path, lines.join("\n") + "\n")?;
        Ok((thread_id, path))
    }

    fn set_rollout_forked_from_id(path: &Path, forked_from_id: &str) -> Result<()> {
        let text = fs::read_to_string(path)?;
        let mut lines: Vec<String> = text.lines().map(ToString::to_string).collect();
        let session_meta: RolloutItem = serde_json::from_str(
            lines
                .first()
                .context("rollout is missing session meta line")?,
        )?;
        let RolloutItem::SessionMeta(mut session_meta) = session_meta else {
            anyhow::bail!("first rollout line is not session meta");
        };
        session_meta.meta.forked_from_id = Some(ThreadId::from_string(forked_from_id)?);
        lines[0] = serde_json::to_string(&RolloutItem::SessionMeta(session_meta))?;
        fs::write(path, lines.join("\n") + "\n")?;
        Ok(())
    }

    fn remove_manifest_identity_fields(manifest_path: &Path) -> Result<()> {
        let mut manifest_value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(manifest_path)?)?;
        let manifest_object = manifest_value
            .as_object_mut()
            .context("backup manifest should be a JSON object")?;
        manifest_object.remove("source");
        manifest_object.remove("forked_from_id");
        manifest_object.remove("is_subagent");
        fs::write(
            manifest_path,
            format!("{}\n", serde_json::to_string_pretty(&manifest_value)?),
        )?;
        Ok(())
    }

    fn remove_manifest_parse_error_fields(manifest_path: &Path) -> Result<()> {
        let mut manifest_value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(manifest_path)?)?;
        let manifest_object = manifest_value
            .as_object_mut()
            .context("backup manifest should be a JSON object")?;
        manifest_object.remove("original_parse_errors");
        manifest_object.remove("trimmed_parse_errors");
        fs::write(
            manifest_path,
            format!("{}\n", serde_json::to_string_pretty(&manifest_value)?),
        )?;
        Ok(())
    }

    #[test]
    fn analyze_detects_head_tail_and_compaction_anchor() -> Result<()> {
        let home = TempDir::new()?;
        let (thread_id, path) = write_rollout(home.path(), CoreSessionSource::Cli)?;

        let analysis = analyze_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            Some(RolloutLocation {
                path,
                archived: false,
            }),
            3,
            false,
        )?;

        assert!(analysis.eligible);
        let trim = analysis.trim.context("trim preview missing")?;
        assert_eq!(trim.protected_head_end_line, 3);
        assert_eq!(trim.requested_tail_lines, 3);
        assert_eq!(trim.actual_tail_start_line, 5);
        assert_eq!(trim.compaction_anchor_line, Some(5));
        assert_eq!(trim.removed_middle_start_line, Some(4));
        assert_eq!(trim.removed_middle_end_line, Some(4));
        assert_eq!(trim.removed_middle_lines, 1);
        Ok(())
    }

    #[test]
    fn trim_restore_and_delete_backup_work() -> Result<()> {
        let home = TempDir::new()?;
        let (thread_id, path) = write_rollout(home.path(), CoreSessionSource::Cli)?;
        let original = fs::read_to_string(&path)?;

        let analysis = analyze_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            Some(RolloutLocation {
                path: path.clone(),
                archived: false,
            }),
            3,
            false,
        )?;
        let fingerprint = analysis
            .analysis_fingerprint
            .context("fingerprint missing")?;
        let trimmed = trim_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            RolloutLocation {
                path: path.clone(),
                archived: false,
            },
            3,
            &fingerprint,
            false,
        )?;
        let trimmed_text = fs::read_to_string(&path)?;
        assert!(trimmed_text.len() < original.len());
        assert!(trimmed.backup.backup_rollout_path.exists());
        assert_eq!(trimmed.analysis.blockers, vec!["existing_backup_present"]);
        assert!(!trimmed.analysis.eligible);
        assert_eq!(
            trimmed.analysis.stats,
            Some(ThreadRolloutStats {
                total_lines: trimmed.backup.trimmed_total_lines,
                total_bytes: trimmed.backup.trimmed_total_bytes,
                parse_errors: 0,
            })
        );
        let manifest_text = fs::read_to_string(backup_manifest_path(home.path(), &thread_id))?;
        let manifest: BackupManifest = serde_json::from_str(&manifest_text)?;
        assert_eq!(manifest.source, Some(SessionSource::Cli));
        assert_eq!(manifest.forked_from_id, None);
        assert!(!manifest.is_subagent);
        assert_eq!(manifest.original_parse_errors, Some(0));
        assert_eq!(manifest.trimmed_parse_errors, Some(0));

        let restored = restore_rollout_backup(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            Some(RolloutLocation {
                path: path.clone(),
                archived: false,
            }),
            false,
        )?;
        let restored_text = fs::read_to_string(&path)?;
        assert_eq!(restored_text, original);
        assert_eq!(restored.backup.original_rollout_path, path);
        assert_eq!(restored.analysis.blockers, vec!["existing_backup_present"]);
        assert!(!restored.analysis.eligible);

        let deleted = delete_rollout_backup(home.path(), &thread_id)?;
        assert!(deleted.deleted);
        assert!(!backup_root(home.path(), &thread_id).exists());
        Ok(())
    }

    #[test]
    fn trim_rejects_rollout_identity_change_between_analysis_and_apply() -> Result<()> {
        let home = TempDir::new()?;
        let (thread_id, path) = write_rollout(home.path(), CoreSessionSource::Cli)?;

        let analysis = analyze_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            Some(RolloutLocation {
                path: path.clone(),
                archived: false,
            }),
            3,
            false,
        )?;
        let fingerprint = analysis
            .analysis_fingerprint
            .context("fingerprint missing")?;

        let replacement =
            fs::read_to_string(&path)?.replace("assistant three", "assistant replacement");
        fs::remove_file(&path)?;
        fs::write(&path, replacement)?;

        let error = trim_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            RolloutLocation {
                path,
                archived: false,
            },
            3,
            &fingerprint,
            false,
        )
        .expect_err("trim should reject rollout identity changes");

        assert!(error.to_string().contains("analysis fingerprint mismatch"));
        Ok(())
    }

    #[test]
    fn trim_rejects_same_size_content_change_between_analysis_and_apply() -> Result<()> {
        let home = TempDir::new()?;
        let (thread_id, path) = write_rollout(home.path(), CoreSessionSource::Cli)?;

        let analysis = analyze_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            Some(RolloutLocation {
                path: path.clone(),
                archived: false,
            }),
            3,
            false,
        )?;
        let fingerprint = analysis
            .analysis_fingerprint
            .context("fingerprint missing")?;

        let original_text = fs::read_to_string(&path)?;
        let replacement = original_text.replace("assistant three", "assistant there");
        assert_eq!(replacement.len(), original_text.len());
        fs::write(&path, replacement)?;

        let error = trim_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            RolloutLocation {
                path,
                archived: false,
            },
            3,
            &fingerprint,
            false,
        )
        .expect_err("trim should reject same-size content changes");

        assert!(error.to_string().contains("analysis fingerprint mismatch"));
        Ok(())
    }

    #[test]
    fn restore_recreates_missing_current_rollout_from_backup() -> Result<()> {
        let home = TempDir::new()?;
        let (thread_id, path) = write_rollout(home.path(), CoreSessionSource::Cli)?;
        let original = fs::read_to_string(&path)?;

        let analysis = analyze_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            Some(RolloutLocation {
                path: path.clone(),
                archived: false,
            }),
            3,
            false,
        )?;
        let fingerprint = analysis
            .analysis_fingerprint
            .context("fingerprint missing")?;
        trim_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            RolloutLocation {
                path: path.clone(),
                archived: false,
            },
            3,
            &fingerprint,
            false,
        )?;
        fs::remove_file(&path)?;

        let restored = restore_rollout_backup(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            None,
            false,
        )?;

        assert_eq!(fs::read_to_string(&path)?, original);
        assert_eq!(restored.backup.original_rollout_path, path);
        Ok(())
    }

    #[test]
    fn restore_rejects_tampered_backup_target_path() -> Result<()> {
        let home = TempDir::new()?;
        let (thread_id, path) = write_rollout(home.path(), CoreSessionSource::Cli)?;

        let analysis = analyze_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            Some(RolloutLocation {
                path: path.clone(),
                archived: false,
            }),
            3,
            false,
        )?;
        let fingerprint = analysis
            .analysis_fingerprint
            .context("fingerprint missing")?;
        trim_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            RolloutLocation {
                path: path.clone(),
                archived: false,
            },
            3,
            &fingerprint,
            false,
        )?;

        let manifest_path = backup_manifest_path(home.path(), &thread_id);
        let manifest_text = fs::read_to_string(&manifest_path)?;
        let mut manifest: BackupManifest = serde_json::from_str(&manifest_text)?;
        manifest.original_rollout_path = home.path().join("outside-target.jsonl");
        fs::write(
            &manifest_path,
            format!("{}\n", serde_json::to_string_pretty(&manifest)?),
        )?;

        let error = restore_rollout_backup(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            Some(RolloutLocation {
                path,
                archived: false,
            }),
            false,
        )
        .expect_err("restore should reject tampered backup target path");

        assert!(
            error
                .to_string()
                .contains("backup restore target is not a supported rollout path")
        );
        Ok(())
    }

    #[test]
    fn restore_rejects_supported_root_path_with_wrong_thread_file_name() -> Result<()> {
        let home = TempDir::new()?;
        let (thread_id, path) = write_rollout(home.path(), CoreSessionSource::Cli)?;

        let analysis = analyze_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            Some(RolloutLocation {
                path: path.clone(),
                archived: false,
            }),
            3,
            false,
        )?;
        let fingerprint = analysis
            .analysis_fingerprint
            .context("fingerprint missing")?;
        trim_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            RolloutLocation {
                path,
                archived: false,
            },
            3,
            &fingerprint,
            false,
        )?;

        let other_path = home
            .path()
            .join("sessions/2026/03/29/rollout-2026-03-29T12-00-00-thread-other.jsonl");
        fs::create_dir_all(other_path.parent().context("missing parent")?)?;
        fs::write(&other_path, "{\"event\":\"other\"}\n")?;

        let manifest_path = backup_manifest_path(home.path(), &thread_id);
        let manifest_text = fs::read_to_string(&manifest_path)?;
        let mut manifest: BackupManifest = serde_json::from_str(&manifest_text)?;
        manifest.original_rollout_path = other_path;
        fs::write(
            &manifest_path,
            format!("{}\n", serde_json::to_string_pretty(&manifest)?),
        )?;

        let error = restore_rollout_backup(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            Some(RolloutLocation {
                path: test_rollout_path(home.path(), &thread_id),
                archived: false,
            }),
            false,
        )
        .expect_err("restore should reject wrong-thread rollout target path");

        assert!(
            error
                .to_string()
                .contains("backup restore target does not match the current rollout path")
        );
        Ok(())
    }

    #[test]
    fn backup_manifest_deserializes_without_new_identity_fields() -> Result<()> {
        let legacy_manifest = serde_json::json!({
            "thread_id": "thread-1",
            "original_rollout_path": "/tmp/rollout.jsonl",
            "created_at": "2026-03-31T00:00:00Z",
            "original_total_lines": 10,
            "original_total_bytes": 100,
            "original_parse_errors": 0,
            "trimmed_total_lines": 6,
            "trimmed_total_bytes": 60,
            "trimmed_parse_errors": 0,
            "protected_head_end_line": 3,
            "requested_tail_lines": 3,
            "actual_tail_start_line": 8,
            "analysis_fingerprint": "fp-1"
        });

        let manifest: BackupManifest = serde_json::from_value(legacy_manifest)?;

        assert_eq!(manifest.source, None);
        assert_eq!(manifest.forked_from_id, None);
        assert!(!manifest.is_subagent);
        Ok(())
    }

    #[test]
    fn restore_fallback_prefers_rescanned_rollout_identity_for_legacy_manifest() -> Result<()> {
        let home = TempDir::new()?;
        let (thread_id, path) = write_rollout(home.path(), CoreSessionSource::Cli)?;
        let forked_from_id = Uuid::new_v4().to_string();
        set_rollout_forked_from_id(&path, &forked_from_id)?;

        let analysis = analyze_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            Some(RolloutLocation {
                path: path.clone(),
                archived: false,
            }),
            3,
            false,
        )?;
        let fingerprint = analysis
            .analysis_fingerprint
            .context("fingerprint missing")?;
        trim_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            RolloutLocation {
                path: path.clone(),
                archived: false,
            },
            3,
            &fingerprint,
            false,
        )?;
        let backup = read_existing_backup(home.path(), &thread_id)?
            .context("backup should exist after trim")?;
        fs::copy(&backup.backup_rollout_path, &path)?;
        remove_manifest_identity_fields(&backup.manifest_path)?;

        let response = build_restore_fallback_analysis(
            &thread_id,
            Some("Test".to_string()),
            path,
            false,
            false,
            &backup,
            &anyhow::anyhow!("simulated analyze failure"),
        );

        assert_eq!(response.source, Some(SessionSource::Cli));
        assert_eq!(response.forked_from_id, Some(forked_from_id));
        assert!(!response.is_subagent);
        assert_eq!(
            response.stats,
            Some(ThreadRolloutStats {
                total_lines: backup.original_total_lines,
                total_bytes: backup.original_total_bytes,
                parse_errors: 0,
            })
        );
        assert_eq!(response.blockers, vec!["existing_backup_present"]);
        assert!(
            response
                .warnings
                .iter()
                .any(|warning| warning.contains("simulated analyze failure"))
        );
        Ok(())
    }

    #[test]
    fn restore_fallback_uses_manifest_identity_when_rescan_unavailable() -> Result<()> {
        let home = TempDir::new()?;
        let (thread_id, path) = write_rollout(home.path(), CoreSessionSource::Cli)?;
        let forked_from_id = Uuid::new_v4().to_string();
        set_rollout_forked_from_id(&path, &forked_from_id)?;

        let analysis = analyze_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            Some(RolloutLocation {
                path: path.clone(),
                archived: false,
            }),
            3,
            false,
        )?;
        let fingerprint = analysis
            .analysis_fingerprint
            .context("fingerprint missing")?;
        trim_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            RolloutLocation {
                path: path.clone(),
                archived: false,
            },
            3,
            &fingerprint,
            false,
        )?;
        let backup = read_existing_backup(home.path(), &thread_id)?
            .context("backup should exist after trim")?;
        fs::remove_file(&path)?;

        let response = build_restore_fallback_analysis(
            &thread_id,
            Some("Test".to_string()),
            path,
            false,
            false,
            &backup,
            &anyhow::anyhow!("simulated analyze failure"),
        );

        assert_eq!(response.source, Some(SessionSource::Cli));
        assert_eq!(response.forked_from_id, Some(forked_from_id));
        assert!(!response.is_subagent);
        assert_eq!(
            response.stats,
            Some(ThreadRolloutStats {
                total_lines: backup.original_total_lines,
                total_bytes: backup.original_total_bytes,
                parse_errors: 0,
            })
        );
        assert_eq!(response.blockers, vec!["existing_backup_present"]);
        assert!(
            response
                .warnings
                .iter()
                .any(|warning| warning.contains("simulated analyze failure"))
        );
        Ok(())
    }

    #[test]
    fn restore_fallback_handles_legacy_manifest_without_identity_or_parse_stats() -> Result<()> {
        let home = TempDir::new()?;
        let (thread_id, path) = write_rollout(home.path(), CoreSessionSource::Cli)?;

        let analysis = analyze_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            Some(RolloutLocation {
                path: path.clone(),
                archived: false,
            }),
            3,
            false,
        )?;
        let fingerprint = analysis
            .analysis_fingerprint
            .context("fingerprint missing")?;
        trim_rollout(
            home.path(),
            &thread_id,
            Some("Test".to_string()),
            RolloutLocation {
                path: path.clone(),
                archived: false,
            },
            3,
            &fingerprint,
            false,
        )?;
        let backup = read_existing_backup(home.path(), &thread_id)?
            .context("backup should exist after trim")?;
        remove_manifest_identity_fields(&backup.manifest_path)?;
        remove_manifest_parse_error_fields(&backup.manifest_path)?;
        fs::remove_file(&path)?;

        let response = build_restore_fallback_analysis(
            &thread_id,
            Some("Test".to_string()),
            path,
            false,
            false,
            &backup,
            &anyhow::anyhow!("simulated analyze failure"),
        );

        assert_eq!(response.source, None);
        assert_eq!(response.forked_from_id, None);
        assert!(!response.is_subagent);
        assert_eq!(response.stats, None);
        assert_eq!(response.blockers, vec!["existing_backup_present"]);
        assert_eq!(response.warnings.len(), 1);
        assert!(
            response
                .warnings
                .iter()
                .any(|warning| warning.contains("simulated analyze failure"))
        );
        Ok(())
    }

    #[test]
    fn analyze_blocks_subagent_sources() -> Result<()> {
        let home = TempDir::new()?;
        let (thread_id, path) = write_rollout(
            home.path(),
            CoreSessionSource::SubAgent(SubAgentSource::Other("review".to_string())),
        )?;
        let analysis = analyze_rollout(
            home.path(),
            &thread_id,
            Some("Sub".to_string()),
            Some(RolloutLocation {
                path,
                archived: false,
            }),
            3,
            false,
        )?;
        assert!(!analysis.eligible);
        assert!(
            analysis
                .blockers
                .iter()
                .any(|item| item == "subagent_session_not_supported")
        );
        Ok(())
    }
}
