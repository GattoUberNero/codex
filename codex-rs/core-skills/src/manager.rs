use std::collections::HashMap;
use std::collections::HashSet;
use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;

use codex_config::ConfigLayerStack;
use codex_config::SkillsConfig;
use codex_protocol::protocol::Product;
use codex_protocol::protocol::SkillScope;
use toml::Value as TomlValue;
use tracing::info;
use tracing::warn;

use crate::SkillLoadOutcome;
use crate::build_implicit_skill_path_indexes;
use crate::config_rules::SkillConfigRules;
use crate::config_rules::resolve_disabled_skill_paths;
use crate::config_rules::skill_config_rules_from_stack;
use crate::loader::SkillRoot;
use crate::loader::load_skills_from_roots;
use crate::loader::skill_roots;
use crate::model::SkillAgentFilterDefaults;
use crate::model::SkillAgentFilterMode;
use crate::model::SkillMetadata;
use crate::system::install_system_skills;
use crate::system::uninstall_system_skills;

const CODEXN_EXPLICIT_SKILL_ROOTS_ENV: &str = "CODEXN_EXPLICIT_SKILL_ROOTS";
const CODEXN_SKILL_ROOTS_ENV: &str = "CODEXN_SKILL_ROOTS";

#[derive(Debug, Clone)]
pub struct SkillsLoadInput {
    pub cwd: PathBuf,
    pub effective_skill_roots: Vec<PathBuf>,
    pub config_layer_stack: ConfigLayerStack,
    pub bundled_skills_enabled: bool,
}

impl SkillsLoadInput {
    pub fn new(
        cwd: PathBuf,
        effective_skill_roots: Vec<PathBuf>,
        config_layer_stack: ConfigLayerStack,
        bundled_skills_enabled: bool,
    ) -> Self {
        Self {
            cwd,
            effective_skill_roots,
            config_layer_stack,
            bundled_skills_enabled,
        }
    }
}

pub struct SkillsManager {
    codex_home: PathBuf,
    restriction_product: Option<Product>,
    cache_by_cwd: RwLock<HashMap<PathBuf, SkillLoadOutcome>>,
    cache_by_config: RwLock<HashMap<ConfigSkillsCacheKey, SkillLoadOutcome>>,
}

impl SkillsManager {
    pub fn new(codex_home: PathBuf, bundled_skills_enabled: bool) -> Self {
        Self::new_with_restriction_product(codex_home, bundled_skills_enabled, Some(Product::Codex))
    }

    pub fn new_with_restriction_product(
        codex_home: PathBuf,
        bundled_skills_enabled: bool,
        restriction_product: Option<Product>,
    ) -> Self {
        let manager = Self {
            codex_home,
            restriction_product,
            cache_by_cwd: RwLock::new(HashMap::new()),
            cache_by_config: RwLock::new(HashMap::new()),
        };
        if !bundled_skills_enabled {
            uninstall_system_skills(&manager.codex_home);
        } else if let Err(err) = install_system_skills(&manager.codex_home) {
            tracing::error!("failed to install system skills: {err}");
        }
        manager
    }

    pub fn skills_for_config(&self, input: &SkillsLoadInput) -> SkillLoadOutcome {
        let configured_extra_user_roots =
            configured_extra_user_roots_from_stack(&input.config_layer_stack);
        let mut roots = self.skill_roots_for_config(input);
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
        let skill_config_rules = skill_config_rules_from_stack(&input.config_layer_stack);
        let agent_filter_defaults =
            skill_agent_filter_defaults_from_stack(&input.config_layer_stack);
        let cache_key = config_skills_cache_key(&roots, &skill_config_rules, agent_filter_defaults);
        if let Some(outcome) = self.cached_outcome_for_config(&cache_key) {
            return outcome;
        }

        let outcome = finalize_skill_outcome(
            self.build_skill_outcome(roots, &skill_config_rules),
            &input.config_layer_stack,
            &configured_extra_user_roots,
            &non_explicit_roots,
        );
        let mut cache = self
            .cache_by_config
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.insert(cache_key, outcome.clone());
        outcome
    }

    pub fn skill_roots_for_config(&self, input: &SkillsLoadInput) -> Vec<SkillRoot> {
        let mut roots = skill_roots(
            &input.config_layer_stack,
            input.cwd.as_path(),
            input.effective_skill_roots.clone(),
        );
        if !input.bundled_skills_enabled {
            roots.retain(|root| root.scope != SkillScope::System);
        }
        roots
    }

    pub async fn skills_for_cwd(
        &self,
        input: &SkillsLoadInput,
        force_reload: bool,
    ) -> SkillLoadOutcome {
        if !force_reload && let Some(outcome) = self.cached_outcome_for_cwd(input.cwd.as_path()) {
            return outcome;
        }

        self.skills_for_cwd_with_extra_user_roots(input, force_reload, &[])
            .await
    }

    pub async fn skills_for_cwd_with_extra_user_roots(
        &self,
        input: &SkillsLoadInput,
        force_reload: bool,
        extra_user_roots: &[PathBuf],
    ) -> SkillLoadOutcome {
        if !force_reload && let Some(outcome) = self.cached_outcome_for_cwd(input.cwd.as_path()) {
            return outcome;
        }

        let normalized_extra_user_roots = normalize_extra_user_roots(extra_user_roots);
        let configured_extra_user_roots =
            configured_extra_user_roots_from_stack(&input.config_layer_stack);
        let mut merged_extra_user_roots = configured_extra_user_roots;
        merged_extra_user_roots.extend(normalized_extra_user_roots);
        let merged_extra_user_roots = normalize_extra_user_roots(&merged_extra_user_roots);

        let mut roots = self.skill_roots_for_config(input);
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
        let skill_config_rules = skill_config_rules_from_stack(&input.config_layer_stack);
        let outcome = finalize_skill_outcome(
            self.build_skill_outcome(roots, &skill_config_rules),
            &input.config_layer_stack,
            &merged_extra_user_roots,
            &non_explicit_roots,
        );
        let mut cache = self
            .cache_by_cwd
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.insert(input.cwd.clone(), outcome.clone());
        outcome
    }

    fn build_skill_outcome(
        &self,
        roots: Vec<SkillRoot>,
        skill_config_rules: &SkillConfigRules,
    ) -> SkillLoadOutcome {
        let outcome = crate::filter_skill_load_outcome_for_product(
            load_skills_from_roots(roots),
            self.restriction_product,
        );
        let disabled_paths = resolve_disabled_skill_paths(&outcome.skills, skill_config_rules);
        finalize_loaded_skill_outcome(outcome, disabled_paths)
    }

    pub fn clear_cache(&self) {
        let cleared_cwd = {
            let mut cache = self
                .cache_by_cwd
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let cleared = cache.len();
            cache.clear();
            cleared
        };
        let cleared_config = {
            let mut cache = self
                .cache_by_config
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let cleared = cache.len();
            cache.clear();
            cleared
        };
        let cleared = cleared_cwd + cleared_config;
        info!("skills cache cleared ({cleared} entries)");
    }

    fn cached_outcome_for_cwd(&self, cwd: &Path) -> Option<SkillLoadOutcome> {
        match self.cache_by_cwd.read() {
            Ok(cache) => cache.get(cwd).cloned(),
            Err(err) => err.into_inner().get(cwd).cloned(),
        }
    }

    fn cached_outcome_for_config(
        &self,
        cache_key: &ConfigSkillsCacheKey,
    ) -> Option<SkillLoadOutcome> {
        match self.cache_by_config.read() {
            Ok(cache) => cache.get(cache_key).cloned(),
            Err(err) => err.into_inner().get(cache_key).cloned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConfigSkillsCacheKey {
    roots: Vec<(PathBuf, u8)>,
    skill_config_rules: SkillConfigRules,
    agent_filter_defaults: SkillAgentFilterDefaults,
}

pub fn bundled_skills_enabled_from_stack(config_layer_stack: &ConfigLayerStack) -> bool {
    let effective_config = config_layer_stack.effective_config();
    let Some(skills_value) = effective_config
        .as_table()
        .and_then(|table| table.get("skills"))
    else {
        return true;
    };

    let skills: SkillsConfig = match skills_value.clone().try_into() {
        Ok(skills) => skills,
        Err(err) => {
            warn!("invalid skills config: {err}");
            return true;
        }
    };

    skills.bundled.unwrap_or_default().enabled
}

fn config_skills_cache_key(
    roots: &[SkillRoot],
    skill_config_rules: &SkillConfigRules,
    agent_filter_defaults: SkillAgentFilterDefaults,
) -> ConfigSkillsCacheKey {
    ConfigSkillsCacheKey {
        roots: roots
            .iter()
            .map(|root| {
                let scope_rank = match root.scope {
                    SkillScope::Repo => 0,
                    SkillScope::User => 1,
                    SkillScope::System => 2,
                    SkillScope::Admin => 3,
                };
                (root.path.clone(), scope_rank)
            })
            .collect(),
        skill_config_rules: skill_config_rules.clone(),
        agent_filter_defaults,
    }
}

fn skill_agent_filter_defaults_from_stack(
    config_layer_stack: &ConfigLayerStack,
) -> SkillAgentFilterDefaults {
    skill_agent_filter_defaults_from_toml(&config_layer_stack.effective_config())
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

fn configured_extra_user_roots_from_stack(config_layer_stack: &ConfigLayerStack) -> Vec<PathBuf> {
    let effective_config = config_layer_stack.effective_config();
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
            None => warn!(
                field = field_name,
                "ignored non-string extra skill root entry"
            ),
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

fn finalize_loaded_skill_outcome(
    mut outcome: SkillLoadOutcome,
    disabled_paths: HashSet<PathBuf>,
) -> SkillLoadOutcome {
    outcome.disabled_paths = disabled_paths;
    let (by_scripts_dir, by_doc_path) =
        build_implicit_skill_path_indexes(outcome.allowed_skills_for_implicit_invocation());
    outcome.implicit_skills_by_scripts_dir = Arc::new(by_scripts_dir);
    outcome.implicit_skills_by_doc_path = Arc::new(by_doc_path);
    outcome
}

fn finalize_skill_outcome(
    mut outcome: SkillLoadOutcome,
    config_layer_stack: &ConfigLayerStack,
    explicit_roots: &[PathBuf],
    non_explicit_roots: &[PathBuf],
) -> SkillLoadOutcome {
    outcome.agent_filter_defaults = skill_agent_filter_defaults_from_stack(config_layer_stack);
    outcome.explicit_skill_paths =
        collect_explicit_skill_paths(&outcome.skills, explicit_roots, non_explicit_roots);
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
    outcome
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
#[path = "manager_tests.rs"]
mod tests;
