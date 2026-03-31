use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::Product;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SkillScope;
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct SkillManagedNetworkOverride {
    pub allowed_domains: Option<Vec<String>>,
    pub denied_domains: Option<Vec<String>>,
}

impl SkillManagedNetworkOverride {
    pub fn has_domain_overrides(&self) -> bool {
        self.allowed_domains.is_some() || self.denied_domains.is_some()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
    pub short_description: Option<String>,
    pub interface: Option<SkillInterface>,
    pub dependencies: Option<SkillDependencies>,
    pub policy: Option<SkillPolicy>,
    pub permission_profile: Option<PermissionProfile>,
    pub managed_network_override: Option<SkillManagedNetworkOverride>,
    /// Path to the SKILLS.md file that declares this skill.
    pub path_to_skills_md: PathBuf,
    pub scope: SkillScope,
}

impl SkillMetadata {
    fn allow_implicit_invocation(&self) -> bool {
        self.policy
            .as_ref()
            .and_then(|policy| policy.allow_implicit_invocation)
            .unwrap_or(true)
    }

    fn is_allowed_for_agent_identity(
        &self,
        agent_identity: &str,
        defaults: SkillAgentFilterDefaults,
        source: SkillAgentFilterSource,
    ) -> bool {
        let policy = self.policy.as_ref();
        let mode = policy
            .map(|value| value.effective_agent_filter_mode(defaults, source))
            .unwrap_or(defaults.mode_for_source(source));
        let filter_list = policy
            .and_then(|value| value.allowed_agent_types.as_deref())
            .unwrap_or(&[]);
        let normalized_identity = normalize_agent_identity_token(agent_identity);
        match mode {
            SkillAgentFilterMode::Off | SkillAgentFilterMode::AllowAll => true,
            SkillAgentFilterMode::DenyAll => false,
            SkillAgentFilterMode::Whitelist => match normalized_identity.as_deref() {
                Some(identity) => filter_list.iter().any(|entry| entry == identity),
                None => false,
            },
            SkillAgentFilterMode::Blacklist => match normalized_identity.as_deref() {
                Some(identity) => !filter_list.iter().any(|entry| entry == identity),
                None => true,
            },
        }
    }

    pub fn matches_product_restriction(&self, session_source: &SessionSource) -> bool {
        self.matches_product_restriction_for_product(session_source.restriction_product())
    }

    pub fn matches_product_restriction_for_product(
        &self,
        restriction_product: Option<Product>,
    ) -> bool {
        match &self.policy {
            Some(policy) => {
                policy.products.is_empty()
                    || restriction_product.is_some_and(|product| {
                        product.matches_product_restriction(&policy.products)
                    })
            }
            None => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SkillPolicy {
    pub allow_implicit_invocation: Option<bool>,
    // TODO: Enforce product gating in Codex skill selection/injection instead of only parsing and
    // storing this metadata.
    pub products: Vec<Product>,
    pub agent_filter_mode: Option<SkillAgentFilterMode>,
    pub allow_agent_whitelist: Option<bool>,
    pub allowed_agent_types: Option<Vec<String>>,
}

impl SkillPolicy {
    fn effective_agent_filter_mode(
        &self,
        defaults: SkillAgentFilterDefaults,
        source: SkillAgentFilterSource,
    ) -> SkillAgentFilterMode {
        if let Some(mode) = self.agent_filter_mode {
            return mode;
        }
        if let Some(allow_agent_whitelist) = self.allow_agent_whitelist {
            return if allow_agent_whitelist {
                SkillAgentFilterMode::Whitelist
            } else {
                SkillAgentFilterMode::Off
            };
        }
        defaults.mode_for_source(source)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillAgentFilterMode {
    #[default]
    Off,
    AllowAll,
    DenyAll,
    Whitelist,
    Blacklist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillAgentFilterSource {
    Global,
    Local,
    Explicit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SkillAgentFilterDefaults {
    pub global_mode: SkillAgentFilterMode,
    pub local_mode: SkillAgentFilterMode,
    pub explicit_mode: SkillAgentFilterMode,
}

impl SkillAgentFilterDefaults {
    pub const fn mode_for_source(self, source: SkillAgentFilterSource) -> SkillAgentFilterMode {
        match source {
            SkillAgentFilterSource::Global => self.global_mode,
            SkillAgentFilterSource::Local => self.local_mode,
            SkillAgentFilterSource::Explicit => self.explicit_mode,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInterface {
    pub display_name: Option<String>,
    pub short_description: Option<String>,
    pub icon_small: Option<PathBuf>,
    pub icon_large: Option<PathBuf>,
    pub brand_color: Option<String>,
    pub default_prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillDependencies {
    pub tools: Vec<SkillToolDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillToolDependency {
    pub r#type: String,
    pub value: String,
    pub description: Option<String>,
    pub transport: Option<String>,
    pub command: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillError {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct SkillLoadOutcome {
    pub skills: Vec<SkillMetadata>,
    pub errors: Vec<SkillError>,
    pub disabled_paths: HashSet<PathBuf>,
    pub agent_filter_defaults: SkillAgentFilterDefaults,
    pub explicit_skill_paths: HashSet<PathBuf>,
    pub(crate) implicit_skills_by_scripts_dir: Arc<HashMap<PathBuf, SkillMetadata>>,
    pub(crate) implicit_skills_by_doc_path: Arc<HashMap<PathBuf, SkillMetadata>>,
}

impl SkillLoadOutcome {
    pub fn is_skill_enabled(&self, skill: &SkillMetadata) -> bool {
        !self.disabled_paths.contains(&skill.path_to_skills_md)
    }

    pub fn is_skill_allowed_for_implicit_invocation(&self, skill: &SkillMetadata) -> bool {
        self.is_skill_enabled(skill) && skill.allow_implicit_invocation()
    }

    pub fn allowed_skills_for_implicit_invocation(&self) -> Vec<SkillMetadata> {
        self.skills
            .iter()
            .filter(|skill| self.is_skill_allowed_for_implicit_invocation(skill))
            .cloned()
            .collect()
    }

    pub fn filter_for_session_source(&self, session_source: &SessionSource) -> SkillLoadOutcome {
        let agent_identity = effective_skill_agent_identity(session_source);
        filter_skill_load_outcome_for_session_source(self.clone(), session_source)
            .filter_for_agent_identity(agent_identity.as_str())
    }

    pub fn filter_for_agent_identity(&self, agent_identity: &str) -> SkillLoadOutcome {
        let filtered_skills: Vec<SkillMetadata> = self
            .skills
            .iter()
            .filter(|skill| {
                let source = self.source_for_skill(skill);
                skill.is_allowed_for_agent_identity(
                    agent_identity,
                    self.agent_filter_defaults,
                    source,
                )
            })
            .cloned()
            .collect();

        let kept_paths: HashSet<PathBuf> = filtered_skills
            .iter()
            .map(|skill| skill.path_to_skills_md.clone())
            .collect();
        let filtered_disabled_paths: HashSet<PathBuf> = self
            .disabled_paths
            .iter()
            .filter(|path| kept_paths.contains(*path))
            .cloned()
            .collect();
        let filtered_explicit_paths: HashSet<PathBuf> = self
            .explicit_skill_paths
            .iter()
            .filter(|path| kept_paths.contains(*path))
            .cloned()
            .collect();

        let mut filtered = SkillLoadOutcome {
            skills: filtered_skills,
            errors: self.errors.clone(),
            disabled_paths: filtered_disabled_paths,
            agent_filter_defaults: self.agent_filter_defaults,
            explicit_skill_paths: filtered_explicit_paths,
            implicit_skills_by_scripts_dir: Arc::new(HashMap::new()),
            implicit_skills_by_doc_path: Arc::new(HashMap::new()),
        };
        let (by_scripts_dir, by_doc_path) = crate::build_implicit_skill_path_indexes(
            filtered.allowed_skills_for_implicit_invocation(),
        );
        filtered.implicit_skills_by_scripts_dir = Arc::new(by_scripts_dir);
        filtered.implicit_skills_by_doc_path = Arc::new(by_doc_path);
        filtered
    }

    pub fn skills_with_enabled(&self) -> impl Iterator<Item = (&SkillMetadata, bool)> {
        self.skills
            .iter()
            .map(|skill| (skill, self.is_skill_enabled(skill)))
    }

    fn source_for_skill(&self, skill: &SkillMetadata) -> SkillAgentFilterSource {
        if skill.scope == SkillScope::Repo {
            return SkillAgentFilterSource::Local;
        }
        if self.explicit_skill_paths.contains(&skill.path_to_skills_md) {
            return SkillAgentFilterSource::Explicit;
        }
        SkillAgentFilterSource::Global
    }
}

pub const MAIN_SKILL_AGENT_IDENTITY: &str = "architect";
pub const DEFAULT_SUBAGENT_SKILL_IDENTITY: &str = "default";

pub fn effective_skill_agent_identity(session_source: &SessionSource) -> String {
    match session_source {
        SessionSource::SubAgent(_) => session_source
            .get_agent_role()
            .and_then(|role| normalize_agent_identity_token(role.as_str()))
            .unwrap_or_else(|| DEFAULT_SUBAGENT_SKILL_IDENTITY.to_string()),
        _ => MAIN_SKILL_AGENT_IDENTITY.to_string(),
    }
}

fn normalize_agent_identity_token(raw: &str) -> Option<String> {
    let token = raw.trim().to_ascii_lowercase();
    if token.is_empty() { None } else { Some(token) }
}

pub fn filter_skill_load_outcome_for_product(
    mut outcome: SkillLoadOutcome,
    restriction_product: Option<Product>,
) -> SkillLoadOutcome {
    outcome
        .skills
        .retain(|skill| skill.matches_product_restriction_for_product(restriction_product));
    let kept_paths: HashSet<PathBuf> = outcome
        .skills
        .iter()
        .map(|skill| skill.path_to_skills_md.clone())
        .collect();
    outcome
        .disabled_paths
        .retain(|path| kept_paths.contains(path));
    outcome
        .explicit_skill_paths
        .retain(|path| kept_paths.contains(path));
    outcome.implicit_skills_by_scripts_dir = Arc::new(
        outcome
            .implicit_skills_by_scripts_dir
            .iter()
            .filter(|(_, skill)| skill.matches_product_restriction_for_product(restriction_product))
            .map(|(path, skill)| (path.clone(), skill.clone()))
            .collect(),
    );
    outcome.implicit_skills_by_doc_path = Arc::new(
        outcome
            .implicit_skills_by_doc_path
            .iter()
            .filter(|(_, skill)| skill.matches_product_restriction_for_product(restriction_product))
            .map(|(path, skill)| (path.clone(), skill.clone()))
            .collect(),
    );
    outcome
}

pub fn filter_skill_load_outcome_for_session_source(
    mut outcome: SkillLoadOutcome,
    session_source: &SessionSource,
) -> SkillLoadOutcome {
    outcome
        .skills
        .retain(|skill| skill.matches_product_restriction(session_source));
    let kept_paths: HashSet<PathBuf> = outcome
        .skills
        .iter()
        .map(|skill| skill.path_to_skills_md.clone())
        .collect();
    outcome
        .disabled_paths
        .retain(|path| kept_paths.contains(path));
    outcome
        .explicit_skill_paths
        .retain(|path| kept_paths.contains(path));
    outcome.implicit_skills_by_scripts_dir = Arc::new(
        outcome
            .implicit_skills_by_scripts_dir
            .iter()
            .filter(|(_, skill)| skill.matches_product_restriction(session_source))
            .map(|(path, skill)| (path.clone(), skill.clone()))
            .collect(),
    );
    outcome.implicit_skills_by_doc_path = Arc::new(
        outcome
            .implicit_skills_by_doc_path
            .iter()
            .filter(|(_, skill)| skill.matches_product_restriction(session_source))
            .map(|(path, skill)| (path.clone(), skill.clone()))
            .collect(),
    );
    outcome
}

pub fn filter_skills_for_session_source(
    skills: Vec<SkillMetadata>,
    session_source: &SessionSource,
) -> Vec<SkillMetadata> {
    skills
        .into_iter()
        .filter(|skill| skill.matches_product_restriction(session_source))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::ThreadId;
    use codex_protocol::protocol::SubAgentSource;

    fn skill_with_policy(
        path: &str,
        agent_filter_mode: Option<SkillAgentFilterMode>,
        allow_agent_whitelist: Option<bool>,
        allowed_agent_types: Option<Vec<&str>>,
    ) -> SkillMetadata {
        SkillMetadata {
            name: "demo".to_string(),
            description: "demo".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: Some(SkillPolicy {
                allow_implicit_invocation: Some(true),
                products: vec![],
                agent_filter_mode,
                allow_agent_whitelist,
                allowed_agent_types: allowed_agent_types
                    .map(|types| types.into_iter().map(str::to_ascii_lowercase).collect()),
            }),
            permission_profile: None,
            managed_network_override: None,
            path_to_skills_md: PathBuf::from(path),
            scope: SkillScope::User,
        }
    }

    fn defaults_for_all_sources(mode: SkillAgentFilterMode) -> SkillAgentFilterDefaults {
        SkillAgentFilterDefaults {
            global_mode: mode,
            local_mode: mode,
            explicit_mode: mode,
        }
    }

    #[test]
    fn effective_skill_agent_identity_defaults_to_architect_for_main_sessions() {
        assert_eq!(
            effective_skill_agent_identity(&SessionSource::Cli),
            MAIN_SKILL_AGENT_IDENTITY
        );
        assert_eq!(
            effective_skill_agent_identity(&SessionSource::VSCode),
            MAIN_SKILL_AGENT_IDENTITY
        );
    }

    #[test]
    fn effective_skill_agent_identity_uses_subagent_role_when_present() {
        let source = SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
            parent_thread_id: ThreadId::new(),
            depth: 1,
            agent_path: None,
            agent_nickname: None,
            agent_role: Some("Explorer".to_string()),
        });
        assert_eq!(effective_skill_agent_identity(&source), "explorer");
    }

    #[test]
    fn effective_skill_agent_identity_defaults_for_subagent_without_role() {
        let source = SessionSource::SubAgent(SubAgentSource::Other("job".to_string()));
        assert_eq!(
            effective_skill_agent_identity(&source),
            DEFAULT_SUBAGENT_SKILL_IDENTITY
        );
    }

    #[test]
    fn filter_for_agent_identity_keeps_only_whitelisted_skills() {
        let allowed = skill_with_policy(
            "/tmp/allowed/SKILL.md",
            Some(SkillAgentFilterMode::Whitelist),
            Some(true),
            Some(vec!["architect", "explorer"]),
        );
        let blocked = skill_with_policy(
            "/tmp/blocked/SKILL.md",
            Some(SkillAgentFilterMode::Whitelist),
            Some(true),
            Some(vec!["reviewer"]),
        );
        let unguarded = skill_with_policy("/tmp/unguarded/SKILL.md", None, Some(false), None);
        let outcome = SkillLoadOutcome {
            skills: vec![allowed.clone(), blocked, unguarded.clone()],
            errors: Vec::new(),
            disabled_paths: HashSet::new(),
            agent_filter_defaults: SkillAgentFilterDefaults::default(),
            explicit_skill_paths: HashSet::new(),
            implicit_skills_by_scripts_dir: Arc::new(HashMap::new()),
            implicit_skills_by_doc_path: Arc::new(HashMap::new()),
        };

        let filtered = outcome.filter_for_agent_identity("architect");
        let paths: HashSet<PathBuf> = filtered
            .skills
            .iter()
            .map(|skill| skill.path_to_skills_md.clone())
            .collect();
        assert!(paths.contains(&allowed.path_to_skills_md));
        assert!(paths.contains(&unguarded.path_to_skills_md));
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn filter_for_agent_identity_applies_blacklist_mode() {
        let blocked = skill_with_policy(
            "/tmp/blocked/SKILL.md",
            Some(SkillAgentFilterMode::Blacklist),
            None,
            Some(vec!["architect"]),
        );
        let allowed = skill_with_policy(
            "/tmp/allowed/SKILL.md",
            Some(SkillAgentFilterMode::Blacklist),
            None,
            Some(vec!["explorer"]),
        );
        let outcome = SkillLoadOutcome {
            skills: vec![blocked, allowed.clone()],
            errors: Vec::new(),
            disabled_paths: HashSet::new(),
            agent_filter_defaults: SkillAgentFilterDefaults::default(),
            explicit_skill_paths: HashSet::new(),
            implicit_skills_by_scripts_dir: Arc::new(HashMap::new()),
            implicit_skills_by_doc_path: Arc::new(HashMap::new()),
        };

        let filtered = outcome.filter_for_agent_identity("architect");
        assert_eq!(filtered.skills.len(), 1);
        assert_eq!(
            filtered.skills[0].path_to_skills_md,
            allowed.path_to_skills_md
        );
    }

    #[test]
    fn filter_for_agent_identity_uses_default_mode_for_unguarded_skills() {
        let unguarded = skill_with_policy("/tmp/unguarded/SKILL.md", None, None, None);
        let outcome = SkillLoadOutcome {
            skills: vec![unguarded],
            errors: Vec::new(),
            disabled_paths: HashSet::new(),
            agent_filter_defaults: defaults_for_all_sources(SkillAgentFilterMode::Whitelist),
            explicit_skill_paths: HashSet::new(),
            implicit_skills_by_scripts_dir: Arc::new(HashMap::new()),
            implicit_skills_by_doc_path: Arc::new(HashMap::new()),
        };

        let filtered = outcome.filter_for_agent_identity("architect");
        assert!(filtered.skills.is_empty());
    }

    #[test]
    fn filter_for_agent_identity_uses_default_mode_when_policy_is_missing() {
        let skill_without_policy = SkillMetadata {
            name: "demo".to_string(),
            description: "demo".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            permission_profile: None,
            managed_network_override: None,
            path_to_skills_md: PathBuf::from("/tmp/no-policy/SKILL.md"),
            scope: SkillScope::User,
        };
        let outcome = SkillLoadOutcome {
            skills: vec![skill_without_policy],
            errors: Vec::new(),
            disabled_paths: HashSet::new(),
            agent_filter_defaults: defaults_for_all_sources(SkillAgentFilterMode::Whitelist),
            explicit_skill_paths: HashSet::new(),
            implicit_skills_by_scripts_dir: Arc::new(HashMap::new()),
            implicit_skills_by_doc_path: Arc::new(HashMap::new()),
        };

        let filtered = outcome.filter_for_agent_identity("architect");
        assert!(filtered.skills.is_empty());
    }

    #[test]
    fn filter_for_agent_identity_supports_allow_all_and_deny_all_modes() {
        let allow_all = skill_with_policy(
            "/tmp/allow-all/SKILL.md",
            Some(SkillAgentFilterMode::AllowAll),
            None,
            Some(vec!["reviewer"]),
        );
        let deny_all = skill_with_policy(
            "/tmp/deny-all/SKILL.md",
            Some(SkillAgentFilterMode::DenyAll),
            None,
            Some(vec!["architect"]),
        );
        let outcome = SkillLoadOutcome {
            skills: vec![allow_all.clone(), deny_all],
            errors: Vec::new(),
            disabled_paths: HashSet::new(),
            agent_filter_defaults: SkillAgentFilterDefaults::default(),
            explicit_skill_paths: HashSet::new(),
            implicit_skills_by_scripts_dir: Arc::new(HashMap::new()),
            implicit_skills_by_doc_path: Arc::new(HashMap::new()),
        };

        let filtered = outcome.filter_for_agent_identity("architect");
        assert_eq!(filtered.skills.len(), 1);
        assert_eq!(
            filtered.skills[0].path_to_skills_md,
            allow_all.path_to_skills_md
        );
    }

    #[test]
    fn filter_for_agent_identity_uses_source_specific_defaults() {
        let global_skill = SkillMetadata {
            name: "global".to_string(),
            description: "global".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            permission_profile: None,
            managed_network_override: None,
            path_to_skills_md: PathBuf::from("/tmp/global/SKILL.md"),
            scope: SkillScope::User,
        };
        let local_skill = SkillMetadata {
            name: "local".to_string(),
            description: "local".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            permission_profile: None,
            managed_network_override: None,
            path_to_skills_md: PathBuf::from("/tmp/local/SKILL.md"),
            scope: SkillScope::Repo,
        };
        let explicit_skill = SkillMetadata {
            name: "explicit".to_string(),
            description: "explicit".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            permission_profile: None,
            managed_network_override: None,
            path_to_skills_md: PathBuf::from("/tmp/explicit/SKILL.md"),
            scope: SkillScope::User,
        };
        let outcome = SkillLoadOutcome {
            skills: vec![global_skill, local_skill.clone(), explicit_skill.clone()],
            errors: Vec::new(),
            disabled_paths: HashSet::new(),
            agent_filter_defaults: SkillAgentFilterDefaults {
                global_mode: SkillAgentFilterMode::DenyAll,
                local_mode: SkillAgentFilterMode::AllowAll,
                explicit_mode: SkillAgentFilterMode::AllowAll,
            },
            explicit_skill_paths: HashSet::from([explicit_skill.path_to_skills_md.clone()]),
            implicit_skills_by_scripts_dir: Arc::new(HashMap::new()),
            implicit_skills_by_doc_path: Arc::new(HashMap::new()),
        };

        let filtered = outcome.filter_for_agent_identity("architect");
        let paths: HashSet<PathBuf> = filtered
            .skills
            .iter()
            .map(|skill| skill.path_to_skills_md.clone())
            .collect();
        assert!(paths.contains(&local_skill.path_to_skills_md));
        assert!(paths.contains(&explicit_skill.path_to_skills_md));
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn filter_for_agent_identity_prefers_local_scope_over_explicit_marker() {
        let local_skill = SkillMetadata {
            name: "local".to_string(),
            description: "local".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            permission_profile: None,
            managed_network_override: None,
            path_to_skills_md: PathBuf::from("/tmp/local/SKILL.md"),
            scope: SkillScope::Repo,
        };
        let outcome = SkillLoadOutcome {
            skills: vec![local_skill.clone()],
            errors: Vec::new(),
            disabled_paths: HashSet::new(),
            agent_filter_defaults: SkillAgentFilterDefaults {
                global_mode: SkillAgentFilterMode::AllowAll,
                local_mode: SkillAgentFilterMode::AllowAll,
                explicit_mode: SkillAgentFilterMode::DenyAll,
            },
            explicit_skill_paths: HashSet::from([local_skill.path_to_skills_md]),
            implicit_skills_by_scripts_dir: Arc::new(HashMap::new()),
            implicit_skills_by_doc_path: Arc::new(HashMap::new()),
        };

        let filtered = outcome.filter_for_agent_identity("architect");
        assert_eq!(filtered.skills.len(), 1);
    }
}
