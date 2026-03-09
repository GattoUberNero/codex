use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
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
use crate::skills::model::SkillMetadata;
use crate::skills::system::install_system_skills;

const CODEXN_EXPLICIT_SKILL_ROOTS_ENV: &str = "CODEXN_EXPLICIT_SKILL_ROOTS";
const CODEXN_SKILL_ROOTS_ENV: &str = "CODEXN_SKILL_ROOTS";

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

        let configured_extra_user_roots =
            configured_extra_user_roots_from_stack(&config.config_layer_stack);
        let mut roots =
            skill_roots_from_layer_stack_with_agents(&config.config_layer_stack, &config.cwd);
        let non_explicit_roots: Vec<PathBuf> = roots.iter().map(|root| root.path.clone()).collect();
        roots.extend(
            configured_extra_user_roots
                .iter()
                .cloned()
                .map(|path| SkillRoot {
                    path,
                    scope: SkillScope::User,
                }),
        );
        let mut outcome = load_skills_from_roots(roots);
        outcome.disabled_paths = disabled_paths_from_stack(&config.config_layer_stack);
        outcome.agent_filter_defaults =
            skill_agent_filter_defaults_from_stack(&config.config_layer_stack);
        outcome.explicit_skill_paths = collect_explicit_skill_paths(
            &outcome.skills,
            &configured_extra_user_roots,
            &non_explicit_roots,
        );
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

        let configured_extra_user_roots =
            configured_extra_user_roots_from_stack(&config_layer_stack);
        let mut merged_extra_user_roots = configured_extra_user_roots.clone();
        merged_extra_user_roots.extend(normalized_extra_user_roots);
        let merged_extra_user_roots = normalize_extra_user_roots(&merged_extra_user_roots);

        let mut roots = skill_roots_from_layer_stack_with_agents(&config_layer_stack, cwd);
        let non_explicit_roots: Vec<PathBuf> = roots.iter().map(|root| root.path.clone()).collect();
        roots.extend(
            merged_extra_user_roots
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
        outcome.explicit_skill_paths = collect_explicit_skill_paths(
            &outcome.skills,
            &merged_extra_user_roots,
            &non_explicit_roots,
        );
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
    let canonical_skills = root.get("skills").and_then(TomlValue::as_table);
    let fork_alias_skills = root
        .get("nero")
        .and_then(TomlValue::as_table)
        .and_then(|nero| nero.get("skills"))
        .and_then(TomlValue::as_table);

    let fallback_mode = read_skill_filter_mode(
        canonical_skills,
        "agent_filter_default_mode",
        "skills.agent_filter_default_mode",
    )
    .or_else(|| {
        read_skill_filter_mode(
            fork_alias_skills,
            "default_mode",
            "nero.skills.default_mode",
        )
    })
    .unwrap_or(SkillAgentFilterMode::Off);

    let global_mode = read_skill_filter_mode(
        canonical_skills,
        "agent_filter_default_mode_global",
        "skills.agent_filter_default_mode_global",
    )
    .or_else(|| {
        read_skill_filter_mode(
            fork_alias_skills,
            "default_mode_global",
            "nero.skills.default_mode_global",
        )
    })
    .unwrap_or(fallback_mode);

    let local_mode = read_skill_filter_mode(
        canonical_skills,
        "agent_filter_default_mode_local",
        "skills.agent_filter_default_mode_local",
    )
    .or_else(|| {
        read_skill_filter_mode(
            fork_alias_skills,
            "default_mode_local",
            "nero.skills.default_mode_local",
        )
    })
    .unwrap_or(fallback_mode);

    let explicit_mode = read_skill_filter_mode(
        canonical_skills,
        "agent_filter_default_mode_explicit",
        "skills.agent_filter_default_mode_explicit",
    )
    .or_else(|| {
        read_skill_filter_mode(
            fork_alias_skills,
            "default_mode_explicit",
            "nero.skills.default_mode_explicit",
        )
    })
    .unwrap_or(fallback_mode);

    SkillAgentFilterDefaults {
        global_mode,
        local_mode,
        explicit_mode,
    }
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

fn read_skill_filter_mode(
    table: Option<&toml::map::Map<String, TomlValue>>,
    key: &str,
    field_name: &str,
) -> Option<SkillAgentFilterMode> {
    let raw_mode = table?.get(key)?.as_str()?;
    let mode = parse_skill_agent_filter_mode(raw_mode);
    if mode.is_none() {
        warn!(
            value = raw_mode,
            field = field_name,
            "invalid skill agent filter mode; expected one of: off, allow-all, deny-all, whitelist, blacklist"
        );
    }
    mode
}

fn configured_extra_user_roots_from_stack(
    config_layer_stack: &crate::config_loader::ConfigLayerStack,
) -> Vec<PathBuf> {
    let mut effective_config = config_layer_stack.effective_config();
    if let Err(err) = apply_codexn_extra_config_overlays(&mut effective_config) {
        warn!("failed to apply codexn extra config overlays for explicit skill roots: {err}");
    }
    let mut roots = configured_extra_user_roots_from_toml(&effective_config);
    if let Some(raw_paths) = env::var_os(CODEXN_EXPLICIT_SKILL_ROOTS_ENV) {
        roots.extend(filter_absolute_roots(
            env::split_paths(&raw_paths).collect(),
            CODEXN_EXPLICIT_SKILL_ROOTS_ENV,
        ));
    }
    if let Some(raw_paths) = env::var_os(CODEXN_SKILL_ROOTS_ENV) {
        roots.extend(filter_absolute_roots(
            env::split_paths(&raw_paths).collect(),
            CODEXN_SKILL_ROOTS_ENV,
        ));
    }
    normalize_extra_user_roots(&roots)
}

fn configured_extra_user_roots_from_toml(config_toml: &TomlValue) -> Vec<PathBuf> {
    let mut roots = Vec::<PathBuf>::new();
    let Some(root) = config_toml.as_table() else {
        return roots;
    };

    let canonical_skills = root.get("skills").and_then(TomlValue::as_table);
    roots.extend(read_path_list_from_table(
        canonical_skills,
        "extra_roots",
        "skills.extra_roots",
    ));

    let fork_alias_skills = root
        .get("nero")
        .and_then(TomlValue::as_table)
        .and_then(|nero| nero.get("skills"))
        .and_then(TomlValue::as_table);
    roots.extend(read_path_list_from_table(
        fork_alias_skills,
        "explicit_roots",
        "nero.skills.explicit_roots",
    ));
    roots.extend(read_path_list_from_table(
        fork_alias_skills,
        "extra_roots",
        "nero.skills.extra_roots",
    ));

    normalize_extra_user_roots(&roots)
}

fn read_path_list_from_table(
    table: Option<&toml::map::Map<String, TomlValue>>,
    key: &str,
    field_name: &str,
) -> Vec<PathBuf> {
    let Some(value) = table.and_then(|tbl| tbl.get(key)) else {
        return Vec::new();
    };
    let Some(entries) = value.as_array() else {
        warn!(field = field_name, "expected an array of paths");
        return Vec::new();
    };

    let mut paths = Vec::new();
    for entry in entries {
        match entry.as_str() {
            Some(raw_path) => {
                let trimmed = raw_path.trim();
                if !trimmed.is_empty() {
                    paths.push(PathBuf::from(trimmed));
                }
            }
            None => {
                warn!(
                    field = field_name,
                    "ignored non-string extra skill root entry"
                );
            }
        }
    }
    filter_absolute_roots(paths, field_name)
}

fn collect_explicit_skill_paths(
    skills: &[SkillMetadata],
    explicit_roots: &[PathBuf],
    non_explicit_roots: &[PathBuf],
) -> HashSet<PathBuf> {
    if explicit_roots.is_empty() {
        return HashSet::new();
    }
    skills
        .iter()
        .filter(|skill| {
            if non_explicit_roots
                .iter()
                .any(|root| skill.path_to_skills_md.starts_with(root))
            {
                return false;
            }
            explicit_roots
                .iter()
                .any(|root| skill.path_to_skills_md.starts_with(root))
        })
        .map(|skill| skill.path_to_skills_md.clone())
        .collect()
}

fn filter_absolute_roots(paths: Vec<PathBuf>, source_label: &str) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter_map(|path| {
            if path.is_absolute() {
                Some(path)
            } else {
                warn!(
                    source = source_label,
                    path = %path.display(),
                    "ignored non-absolute explicit skill root"
                );
                None
            }
        })
        .collect()
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
        assert_eq!(defaults.global_mode, SkillAgentFilterMode::Whitelist);
        assert_eq!(defaults.local_mode, SkillAgentFilterMode::Whitelist);
        assert_eq!(defaults.explicit_mode, SkillAgentFilterMode::Whitelist);
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
        assert_eq!(defaults.global_mode, SkillAgentFilterMode::Blacklist);
        assert_eq!(defaults.local_mode, SkillAgentFilterMode::Blacklist);
        assert_eq!(defaults.explicit_mode, SkillAgentFilterMode::Blacklist);
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
            allow_all_defaults.global_mode,
            SkillAgentFilterMode::AllowAll
        );
        assert_eq!(
            allow_all_defaults.local_mode,
            SkillAgentFilterMode::AllowAll
        );
        assert_eq!(
            allow_all_defaults.explicit_mode,
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
        assert_eq!(deny_all_defaults.global_mode, SkillAgentFilterMode::DenyAll);
        assert_eq!(deny_all_defaults.local_mode, SkillAgentFilterMode::DenyAll);
        assert_eq!(
            deny_all_defaults.explicit_mode,
            SkillAgentFilterMode::DenyAll
        );
    }

    #[test]
    fn skill_agent_filter_defaults_supports_source_specific_modes() {
        let config_toml: TomlValue = toml::from_str(
            r#"
            [nero.skills]
            default_mode_global = "deny-all"
            default_mode_local = "allow-all"
            default_mode_explicit = "allow-all"
            "#,
        )
        .expect("valid toml");
        let defaults = skill_agent_filter_defaults_from_toml(&config_toml);
        assert_eq!(defaults.global_mode, SkillAgentFilterMode::DenyAll);
        assert_eq!(defaults.local_mode, SkillAgentFilterMode::AllowAll);
        assert_eq!(defaults.explicit_mode, SkillAgentFilterMode::AllowAll);
    }

    #[test]
    fn configured_extra_user_roots_reads_nero_alias() {
        let config_toml: TomlValue = toml::from_str(
            r#"
            [nero.skills]
            explicit_roots = ["/tmp/skills-a", "/tmp/skills-b"]
            "#,
        )
        .expect("valid toml");
        let roots = configured_extra_user_roots_from_toml(&config_toml);
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/tmp/skills-a"),
                PathBuf::from("/tmp/skills-b")
            ]
        );
    }

    #[test]
    fn configured_extra_user_roots_ignores_relative_paths() {
        let config_toml: TomlValue = toml::from_str(
            r#"
            [nero.skills]
            explicit_roots = ["relative/path", "/tmp/skills-abs"]
            "#,
        )
        .expect("valid toml");
        let roots = configured_extra_user_roots_from_toml(&config_toml);
        assert_eq!(roots, vec![PathBuf::from("/tmp/skills-abs")]);
    }

    #[test]
    fn collect_explicit_skill_paths_does_not_override_non_explicit_roots() {
        let repo_skill = SkillMetadata {
            name: "repo".to_string(),
            description: "repo".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            permission_profile: None,
            permissions: None,
            path_to_skills_md: PathBuf::from("/tmp/repo/skills/repo/SKILL.md"),
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
            permissions: None,
            path_to_skills_md: PathBuf::from("/tmp/explicit/skills/x/SKILL.md"),
            scope: SkillScope::User,
        };

        let explicit_paths = collect_explicit_skill_paths(
            &[repo_skill.clone(), explicit_skill.clone()],
            &[PathBuf::from("/tmp/repo"), PathBuf::from("/tmp/explicit")],
            &[PathBuf::from("/tmp/repo")],
        );

        assert!(!explicit_paths.contains(&repo_skill.path_to_skills_md));
        assert!(explicit_paths.contains(&explicit_skill.path_to_skills_md));
    }
}
