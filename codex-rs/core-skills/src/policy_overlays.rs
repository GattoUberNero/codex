use std::collections::HashMap;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use codex_app_server_protocol::ConfigLayerSource;
use codex_config::ConfigLayerStack;
use codex_config::ConfigLayerStackOrdering;
use codex_protocol::protocol::SkillScope;
use dunce::canonicalize as canonicalize_path;
use serde::Deserialize;
use tracing::warn;

use crate::model::SkillAgentFilterMode;
use crate::model::SkillError;
use crate::model::SkillLoadOutcome;
use crate::model::SkillPolicy;

pub(crate) const PROJECT_SKILL_POLICY_FILE: &str = "skills.policy.toml";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ProjectSkillPolicyOverlay {
    pub source_file: PathBuf,
    pub name: String,
    pub source: ProjectSkillPolicySource,
    pub source_path: Option<PathBuf>,
    pub enabled: Option<bool>,
    pub agent_filter_mode: Option<SkillAgentFilterMode>,
    pub allowed_agent_types: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ProjectSkillPolicySource {
    User,
}

#[derive(Debug, Default)]
pub(crate) struct ProjectSkillPolicyOverlaySet {
    pub overlays: Vec<ProjectSkillPolicyOverlay>,
    pub errors: Vec<SkillError>,
}

#[derive(Debug, Deserialize)]
struct ProjectSkillPolicyFile {
    #[serde(default)]
    overrides: Vec<ProjectSkillPolicyRecord>,
}

#[derive(Debug, Deserialize)]
struct ProjectSkillPolicyRecord {
    name: Option<String>,
    source: Option<String>,
    source_path: Option<PathBuf>,
    enabled: Option<bool>,
    agent_filter_mode: Option<SkillAgentFilterMode>,
    allowed_agent_types: Option<Vec<String>>,
}

pub(crate) fn project_skill_policy_overlays_from_stack(
    config_layer_stack: &ConfigLayerStack,
) -> ProjectSkillPolicyOverlaySet {
    let mut set = ProjectSkillPolicyOverlaySet::default();
    for layer in config_layer_stack.get_layers(
        ConfigLayerStackOrdering::LowestPrecedenceFirst,
        /*include_disabled*/ false,
    ) {
        if !matches!(layer.name, ConfigLayerSource::Project { .. }) {
            continue;
        }
        let Some(config_folder) = layer.config_folder() else {
            continue;
        };
        let policy_path = config_folder.as_path().join(PROJECT_SKILL_POLICY_FILE);
        if !policy_path.exists() {
            continue;
        }
        match read_project_skill_policy_overlays(&policy_path) {
            Ok(mut overlays) => set.overlays.append(&mut overlays),
            Err(message) => set.errors.push(SkillError {
                path: policy_path,
                message,
            }),
        }
    }
    set
}

pub(crate) fn apply_project_skill_policy_overlays(
    mut outcome: SkillLoadOutcome,
    overlays: &ProjectSkillPolicyOverlaySet,
    codex_home: &Path,
) -> SkillLoadOutcome {
    outcome.errors.extend(overlays.errors.clone());
    if overlays.overlays.is_empty() {
        return outcome;
    }

    let user_skills_root = codex_home.join("skills");
    let mut skills_by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, skill) in outcome.skills.iter().enumerate() {
        skills_by_name
            .entry(skill.name.to_ascii_lowercase())
            .or_default()
            .push(index);
    }

    for overlay in &overlays.overlays {
        let Some(index) = resolve_overlay_skill_index(
            &outcome,
            &skills_by_name,
            overlay,
            user_skills_root.as_path(),
        ) else {
            outcome.errors.push(SkillError {
                path: overlay.source_file.clone(),
                message: format!(
                    "project skill policy override for `{}` did not match a loaded global user skill",
                    overlay.name
                ),
            });
            continue;
        };

        let skill = &mut outcome.skills[index];
        skill.policy = Some(merge_project_overlay_policy(
            skill.policy.clone().unwrap_or_default(),
            overlay,
        ));
        if let Some(enabled) = overlay.enabled {
            if enabled {
                outcome.disabled_paths.remove(&skill.path_to_skills_md);
            } else {
                outcome
                    .disabled_paths
                    .insert(skill.path_to_skills_md.clone());
            }
        }
    }

    outcome
}

fn read_project_skill_policy_overlays(
    policy_path: &Path,
) -> Result<Vec<ProjectSkillPolicyOverlay>, String> {
    let contents = fs::read_to_string(policy_path)
        .map_err(|error| format!("failed to read project skill policy file: {error}"))?;
    let parsed: ProjectSkillPolicyFile = toml::from_str(&contents)
        .map_err(|error| format!("invalid project skill policy file: {error}"))?;
    let mut overlays = Vec::new();
    for record in parsed.overrides {
        match resolve_project_skill_policy_record(policy_path, record) {
            Ok(overlay) => overlays.push(overlay),
            Err(message) => warn!(
                path = %policy_path.display(),
                "ignored project skill policy override: {message}"
            ),
        }
    }
    Ok(overlays)
}

fn resolve_project_skill_policy_record(
    policy_path: &Path,
    record: ProjectSkillPolicyRecord,
) -> Result<ProjectSkillPolicyOverlay, String> {
    let name = record
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "missing override name".to_string())?
        .to_string();
    let source = match record
        .source
        .as_deref()
        .map(str::trim)
        .unwrap_or("user")
        .to_ascii_lowercase()
        .as_str()
    {
        "user" => ProjectSkillPolicySource::User,
        other => return Err(format!("unsupported override source `{other}`")),
    };
    let allowed_agent_types = record
        .allowed_agent_types
        .map(|types| normalize_agent_types(&types));
    if let Some(source_path) = record.source_path.as_deref()
        && (source_path.is_absolute()
            || source_path
                .components()
                .any(|component| matches!(component, Component::ParentDir)))
    {
        return Err(format!(
            "override `{name}` uses a source_path outside the user skills root"
        ));
    }
    if record.agent_filter_mode == Some(SkillAgentFilterMode::Whitelist)
        && allowed_agent_types
            .as_ref()
            .is_none_or(std::vec::Vec::is_empty)
    {
        return Err(format!(
            "override `{name}` uses whitelist without allowed_agent_types"
        ));
    }
    Ok(ProjectSkillPolicyOverlay {
        source_file: policy_path.to_path_buf(),
        name,
        source,
        source_path: record.source_path,
        enabled: record.enabled,
        agent_filter_mode: record.agent_filter_mode,
        allowed_agent_types,
    })
}

fn normalize_agent_types(raw_types: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for raw_type in raw_types {
        let value = raw_type.trim().to_ascii_lowercase();
        if value.is_empty() || out.contains(&value) {
            continue;
        }
        out.push(value);
    }
    out
}

fn resolve_overlay_skill_index(
    outcome: &SkillLoadOutcome,
    skills_by_name: &HashMap<String, Vec<usize>>,
    overlay: &ProjectSkillPolicyOverlay,
    user_skills_root: &Path,
) -> Option<usize> {
    let candidates = skills_by_name.get(&overlay.name.to_ascii_lowercase())?;
    if let Some(source_path) = overlay.source_path.as_deref() {
        let target = user_skills_root.join(source_path);
        let target_skill = if target.file_name().is_some_and(|name| name == "SKILL.md") {
            target
        } else {
            target.join("SKILL.md")
        };
        let target_skill = canonicalize_path(&target_skill).unwrap_or(target_skill);
        return candidates.iter().copied().find(|index| {
            let skill = &outcome.skills[*index];
            skill.scope == SkillScope::User && skill.path_to_skills_md == target_skill
        });
    }
    if candidates.len() != 1 {
        return None;
    }
    let index = candidates[0];
    (outcome.skills[index].scope == SkillScope::User).then_some(index)
}

fn merge_project_overlay_policy(
    base: SkillPolicy,
    overlay: &ProjectSkillPolicyOverlay,
) -> SkillPolicy {
    SkillPolicy {
        allow_implicit_invocation: base.allow_implicit_invocation,
        products: base.products,
        agent_filter_mode: overlay.agent_filter_mode.or(base.agent_filter_mode),
        allow_agent_whitelist: base.allow_agent_whitelist,
        allowed_agent_types: overlay
            .allowed_agent_types
            .clone()
            .or(base.allowed_agent_types),
    }
}
