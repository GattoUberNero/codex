use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

use codex_protocol::protocol::SkillScope;
use codex_utils_absolute_path::AbsolutePathBuf;
use toml::Value as TomlValue;
use tracing::info;
use tracing::warn;

use crate::config::Config;
use crate::config::apply_codexn_extra_config_overlays;
use crate::config::types::SkillsConfig;
use crate::config_loader::CloudRequirementsLoader;
use crate::config_loader::LoaderOverrides;
use crate::config_loader::load_config_layers_state;
use crate::skills::SkillLoadOutcome;
use crate::skills::build_implicit_skill_path_indexes;
use crate::skills::loader::SkillRoot;
use crate::skills::loader::load_skills_from_roots;
use crate::skills::loader::skill_roots_from_layer_stack_with_agents;
use crate::skills::model::SkillAgentFilterDefaults;
use crate::skills::model::SkillAgentFilterMode;
use crate::skills::system::install_system_skills;

pub struct SkillsManager {
    codex_home: PathBuf,
    cache_by_cwd: RwLock<HashMap<PathBuf, SkillLoadOutcome>>,
}

impl SkillsManager {
    pub fn new(codex_home: PathBuf) -> Self {
        if let Err(err) = install_system_skills(&codex_home) {
            tracing::error!("failed to install system skills: {err}");
        }

        Self {
            codex_home,
            cache_by_cwd: RwLock::new(HashMap::new()),
        }
    }

    /// Load skills for an already-constructed [`Config`], avoiding any additional config-layer
    /// loading. This also seeds the per-cwd cache for subsequent lookups.
    pub fn skills_for_config(&self, config: &Config) -> SkillLoadOutcome {
        let cwd = &config.cwd;
        if let Some(outcome) = self.cached_outcome_for_cwd(cwd) {
            return outcome;
        }

        let roots =
            skill_roots_from_layer_stack_with_agents(&config.config_layer_stack, &config.cwd);
        let mut outcome = load_skills_from_roots(roots);
        outcome.disabled_paths = disabled_paths_from_stack(&config.config_layer_stack);
        outcome.agent_filter_defaults =
            skill_agent_filter_defaults_from_stack(&config.config_layer_stack);
        let (by_scripts_dir, by_doc_path) =
            build_implicit_skill_path_indexes(outcome.allowed_skills_for_implicit_invocation());
        outcome.implicit_skills_by_scripts_dir = Arc::new(by_scripts_dir);
        outcome.implicit_skills_by_doc_path = Arc::new(by_doc_path);
        let mut cache = match self.cache_by_cwd.write() {
            Ok(cache) => cache,
            Err(err) => err.into_inner(),
        };
        cache.insert(cwd.to_path_buf(), outcome.clone());
        outcome
    }

    pub async fn skills_for_cwd(&self, cwd: &Path, force_reload: bool) -> SkillLoadOutcome {
        if !force_reload && let Some(outcome) = self.cached_outcome_for_cwd(cwd) {
            return outcome;
        }

        self.skills_for_cwd_with_extra_user_roots(cwd, force_reload, &[])
            .await
    }

    pub async fn skills_for_cwd_with_extra_user_roots(
        &self,
        cwd: &Path,
        force_reload: bool,
        extra_user_roots: &[PathBuf],
    ) -> SkillLoadOutcome {
        if !force_reload && let Some(outcome) = self.cached_outcome_for_cwd(cwd) {
            return outcome;
        }
        let normalized_extra_user_roots = normalize_extra_user_roots(extra_user_roots);

        let cwd_abs = match AbsolutePathBuf::try_from(cwd) {
            Ok(cwd_abs) => cwd_abs,
            Err(err) => {
                return SkillLoadOutcome {
                    errors: vec![crate::skills::model::SkillError {
                        path: cwd.to_path_buf(),
                        message: err.to_string(),
                    }],
                    ..Default::default()
                };
            }
        };

        let cli_overrides: Vec<(String, TomlValue)> = Vec::new();
        let config_layer_stack = match load_config_layers_state(
            &self.codex_home,
            Some(cwd_abs),
            &cli_overrides,
            LoaderOverrides::default(),
            CloudRequirementsLoader::default(),
        )
        .await
        {
            Ok(config_layer_stack) => config_layer_stack,
            Err(err) => {
                return SkillLoadOutcome {
                    errors: vec![crate::skills::model::SkillError {
                        path: cwd.to_path_buf(),
                        message: err.to_string(),
                    }],
                    ..Default::default()
                };
            }
        };

        let mut roots = skill_roots_from_layer_stack_with_agents(&config_layer_stack, cwd);
        roots.extend(
            normalized_extra_user_roots
                .iter()
                .cloned()
                .map(|path| SkillRoot {
                    path,
                    scope: SkillScope::User,
                }),
        );
        let mut outcome = load_skills_from_roots(roots);
        if !extra_user_roots.is_empty() {
            // When extra user roots are provided, skip system skills before caching the result.
            outcome
                .skills
                .retain(|skill| skill.scope != SkillScope::System);
        }
        outcome.disabled_paths = disabled_paths_from_stack(&config_layer_stack);
        outcome.agent_filter_defaults = skill_agent_filter_defaults_from_stack(&config_layer_stack);
        let (by_scripts_dir, by_doc_path) =
            build_implicit_skill_path_indexes(outcome.allowed_skills_for_implicit_invocation());
        outcome.implicit_skills_by_scripts_dir = Arc::new(by_scripts_dir);
        outcome.implicit_skills_by_doc_path = Arc::new(by_doc_path);
        let mut cache = match self.cache_by_cwd.write() {
            Ok(cache) => cache,
            Err(err) => err.into_inner(),
        };
        cache.insert(cwd.to_path_buf(), outcome.clone());
        outcome
    }

    pub fn clear_cache(&self) {
        let mut cache = match self.cache_by_cwd.write() {
            Ok(cache) => cache,
            Err(err) => err.into_inner(),
        };
        let cleared = cache.len();
        cache.clear();
        info!("skills cache cleared ({cleared} entries)");
    }

    fn cached_outcome_for_cwd(&self, cwd: &Path) -> Option<SkillLoadOutcome> {
        match self.cache_by_cwd.read() {
            Ok(cache) => cache.get(cwd).cloned(),
            Err(err) => err.into_inner().get(cwd).cloned(),
        }
    }
}

fn disabled_paths_from_stack(
    config_layer_stack: &crate::config_loader::ConfigLayerStack,
) -> HashSet<PathBuf> {
    let mut disabled = HashSet::new();
    let mut configs = HashMap::new();
    // Skills config is user-layer only for now; higher-precedence layers are ignored.
    let Some(user_layer) = config_layer_stack.get_user_layer() else {
        return disabled;
    };
    let Some(skills_value) = user_layer.config.get("skills") else {
        return disabled;
    };
    let skills: SkillsConfig = match skills_value.clone().try_into() {
        Ok(skills) => skills,
        Err(err) => {
            warn!("invalid skills config: {err}");
            return disabled;
        }
    };

    for entry in skills.config {
        let path = normalize_override_path(entry.path.as_path());
        configs.insert(path, entry.enabled);
    }

    for (path, enabled) in configs {
        if !enabled {
            disabled.insert(path);
        }
    }

    disabled
}

fn skill_agent_filter_defaults_from_stack(
    config_layer_stack: &crate::config_loader::ConfigLayerStack,
) -> SkillAgentFilterDefaults {
    let mut effective_config = config_layer_stack.effective_config();
    if let Err(err) = apply_codexn_extra_config_overlays(&mut effective_config) {
        warn!(
            "failed to apply codexn extra config overlays for skill agent filter defaults: {err}"
        );
    }
    skill_agent_filter_defaults_from_toml(&effective_config)
}

fn skill_agent_filter_defaults_from_toml(config_toml: &TomlValue) -> SkillAgentFilterDefaults {
    let Some(root) = config_toml.as_table() else {
        return SkillAgentFilterDefaults::default();
    };

    let canonical = root
        .get("skills")
        .and_then(TomlValue::as_table)
        .and_then(|skills| skills.get("agent_filter_default_mode"))
        .and_then(TomlValue::as_str);
    if let Some(raw_mode) = canonical {
        if let Some(mode) = parse_skill_agent_filter_mode(raw_mode) {
            return SkillAgentFilterDefaults { default_mode: mode };
        }
        warn!(
            value = raw_mode,
            "invalid skills.agent_filter_default_mode; expected one of: off, allow-all, deny-all, whitelist, blacklist"
        );
    }

    let fork_alias = root
        .get("nero")
        .and_then(TomlValue::as_table)
        .and_then(|nero| nero.get("skills"))
        .and_then(TomlValue::as_table)
        .and_then(|skills| skills.get("default_mode"))
        .and_then(TomlValue::as_str);
    if let Some(raw_mode) = fork_alias {
        if let Some(mode) = parse_skill_agent_filter_mode(raw_mode) {
            return SkillAgentFilterDefaults { default_mode: mode };
        }
        warn!(
            value = raw_mode,
            "invalid nero.skills.default_mode; expected one of: off, allow-all, deny-all, whitelist, blacklist"
        );
    }

    SkillAgentFilterDefaults::default()
}

fn parse_skill_agent_filter_mode(raw: &str) -> Option<SkillAgentFilterMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" => Some(SkillAgentFilterMode::Off),
        "allow-all" | "allow_all" => Some(SkillAgentFilterMode::AllowAll),
        "deny-all" | "deny_all" => Some(SkillAgentFilterMode::DenyAll),
        "whitelist" => Some(SkillAgentFilterMode::Whitelist),
        "blacklist" => Some(SkillAgentFilterMode::Blacklist),
        _ => None,
    }
}

fn normalize_override_path(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn normalize_extra_user_roots(extra_user_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut normalized: Vec<PathBuf> = extra_user_roots
        .iter()
        .map(|path| dunce::canonicalize(path).unwrap_or_else(|_| path.clone()))
        .collect();
    normalized.sort_unstable();
    normalized.dedup();
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigBuilder;
    use crate::config::ConfigOverrides;
    use pretty_assertions::assert_eq;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use toml::Value as TomlValue;

    fn write_user_skill(codex_home: &TempDir, dir: &str, name: &str, description: &str) {
        let skill_dir = codex_home.path().join("skills").join(dir);
        fs::create_dir_all(&skill_dir).unwrap();
        let content = format!("---\nname: {name}\ndescription: {description}\n---\n\n# Body\n");
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    #[tokio::test]
    async fn skills_for_config_seeds_cache_by_cwd() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let cwd = tempfile::tempdir().expect("tempdir");

        let cfg = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .harness_overrides(ConfigOverrides {
                cwd: Some(cwd.path().to_path_buf()),
                ..Default::default()
            })
            .build()
            .await
            .expect("defaults for test should always succeed");

        let skills_manager = SkillsManager::new(codex_home.path().to_path_buf());

        write_user_skill(&codex_home, "a", "skill-a", "from a");
        let outcome1 = skills_manager.skills_for_config(&cfg);
        assert!(
            outcome1.skills.iter().any(|s| s.name == "skill-a"),
            "expected skill-a to be discovered"
        );

        // Write a new skill after the first call; the second call should hit the cache and not
        // reflect the new file.
        write_user_skill(&codex_home, "b", "skill-b", "from b");
        let outcome2 = skills_manager.skills_for_config(&cfg);
        assert_eq!(outcome2.errors, outcome1.errors);
        assert_eq!(outcome2.skills, outcome1.skills);
    }

    #[tokio::test]
    async fn skills_for_cwd_reuses_cached_entry_even_when_entry_has_extra_roots() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let cwd = tempfile::tempdir().expect("tempdir");
        let extra_root = tempfile::tempdir().expect("tempdir");

        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .harness_overrides(ConfigOverrides {
                cwd: Some(cwd.path().to_path_buf()),
                ..Default::default()
            })
            .build()
            .await
            .expect("defaults for test should always succeed");

        let skills_manager = SkillsManager::new(codex_home.path().to_path_buf());
        let _ = skills_manager.skills_for_config(&config);

        write_user_skill(&extra_root, "x", "extra-skill", "from extra root");
        let extra_root_path = extra_root.path().to_path_buf();
        let outcome_with_extra = skills_manager
            .skills_for_cwd_with_extra_user_roots(
                cwd.path(),
                true,
                std::slice::from_ref(&extra_root_path),
            )
            .await;
        assert!(
            outcome_with_extra
                .skills
                .iter()
                .any(|skill| skill.name == "extra-skill")
        );

        // The cwd-only API returns the current cached entry for this cwd, even when that entry
        // was produced with extra roots.
        let outcome_without_extra = skills_manager.skills_for_cwd(cwd.path(), false).await;
        assert_eq!(outcome_without_extra.skills, outcome_with_extra.skills);
        assert_eq!(outcome_without_extra.errors, outcome_with_extra.errors);
    }

    #[tokio::test]
    async fn skills_for_cwd_with_extra_roots_only_refreshes_on_force_reload() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let cwd = tempfile::tempdir().expect("tempdir");
        let extra_root_a = tempfile::tempdir().expect("tempdir");
        let extra_root_b = tempfile::tempdir().expect("tempdir");

        let config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .harness_overrides(ConfigOverrides {
                cwd: Some(cwd.path().to_path_buf()),
                ..Default::default()
            })
            .build()
            .await
            .expect("defaults for test should always succeed");

        let skills_manager = SkillsManager::new(codex_home.path().to_path_buf());
        let _ = skills_manager.skills_for_config(&config);

        write_user_skill(&extra_root_a, "x", "extra-skill-a", "from extra root a");
        write_user_skill(&extra_root_b, "x", "extra-skill-b", "from extra root b");

        let extra_root_a_path = extra_root_a.path().to_path_buf();
        let outcome_a = skills_manager
            .skills_for_cwd_with_extra_user_roots(
                cwd.path(),
                true,
                std::slice::from_ref(&extra_root_a_path),
            )
            .await;
        assert!(
            outcome_a
                .skills
                .iter()
                .any(|skill| skill.name == "extra-skill-a")
        );
        assert!(
            outcome_a
                .skills
                .iter()
                .all(|skill| skill.name != "extra-skill-b")
        );

        let extra_root_b_path = extra_root_b.path().to_path_buf();
        let outcome_b = skills_manager
            .skills_for_cwd_with_extra_user_roots(
                cwd.path(),
                false,
                std::slice::from_ref(&extra_root_b_path),
            )
            .await;
        assert!(
            outcome_b
                .skills
                .iter()
                .any(|skill| skill.name == "extra-skill-a")
        );
        assert!(
            outcome_b
                .skills
                .iter()
                .all(|skill| skill.name != "extra-skill-b")
        );

        let outcome_reloaded = skills_manager
            .skills_for_cwd_with_extra_user_roots(
                cwd.path(),
                true,
                std::slice::from_ref(&extra_root_b_path),
            )
            .await;
        assert!(
            outcome_reloaded
                .skills
                .iter()
                .any(|skill| skill.name == "extra-skill-b")
        );
        assert!(
            outcome_reloaded
                .skills
                .iter()
                .all(|skill| skill.name != "extra-skill-a")
        );
    }

    #[test]
    fn normalize_extra_user_roots_is_stable_for_equivalent_inputs() {
        let a = PathBuf::from("/tmp/a");
        let b = PathBuf::from("/tmp/b");

        let first = normalize_extra_user_roots(&[a.clone(), b.clone(), a.clone()]);
        let second = normalize_extra_user_roots(&[b, a]);

        assert_eq!(first, second);
    }

    #[test]
    fn skill_agent_filter_defaults_reads_fork_alias() {
        let config_toml: TomlValue = toml::from_str(
            r#"
            [nero.skills]
            default_mode = "whitelist"
            "#,
        )
        .expect("valid toml");
        let defaults = skill_agent_filter_defaults_from_toml(&config_toml);
        assert_eq!(defaults.default_mode, SkillAgentFilterMode::Whitelist);
    }

    #[test]
    fn skill_agent_filter_defaults_prefers_canonical_value() {
        let config_toml: TomlValue = toml::from_str(
            r#"
            [skills]
            agent_filter_default_mode = "blacklist"

            [nero.skills]
            default_mode = "whitelist"
            "#,
        )
        .expect("valid toml");
        let defaults = skill_agent_filter_defaults_from_toml(&config_toml);
        assert_eq!(defaults.default_mode, SkillAgentFilterMode::Blacklist);
    }

    #[test]
    fn skill_agent_filter_defaults_supports_explicit_allow_all_and_deny_all() {
        let allow_all_toml: TomlValue = toml::from_str(
            r#"
            [skills]
            agent_filter_default_mode = "allow-all"
            "#,
        )
        .expect("valid toml");
        let allow_all_defaults = skill_agent_filter_defaults_from_toml(&allow_all_toml);
        assert_eq!(
            allow_all_defaults.default_mode,
            SkillAgentFilterMode::AllowAll
        );

        let deny_all_toml: TomlValue = toml::from_str(
            r#"
            [skills]
            agent_filter_default_mode = "deny-all"
            "#,
        )
        .expect("valid toml");
        let deny_all_defaults = skill_agent_filter_defaults_from_toml(&deny_all_toml);
        assert_eq!(
            deny_all_defaults.default_mode,
            SkillAgentFilterMode::DenyAll
        );
    }
}
