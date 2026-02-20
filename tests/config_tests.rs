use agtx::config::{GlobalConfig, ProjectConfig, WorktreeConfig, ThemeConfig, MergedConfig};

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
    assert_eq!(config.worktree.base_branch, "main");
}

// === WorktreeConfig Tests ===

#[test]
fn test_worktree_config_default() {
    let config = WorktreeConfig::default();

    assert!(config.enabled);
    assert!(config.auto_cleanup);
    assert_eq!(config.base_branch, "main");
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
}

// === MergedConfig Tests ===

#[test]
fn test_merged_config_uses_global_defaults() {
    let global = GlobalConfig::default();
    let project = ProjectConfig::default();

    let merged = MergedConfig::merge(&global, &project);

    assert_eq!(merged.default_agent, "claude");
    assert_eq!(merged.base_branch, "main");
    assert!(merged.worktree_enabled);
    assert!(merged.auto_cleanup);
    assert!(merged.copy_files.is_none());
    assert!(merged.init_script.is_none());
}

#[test]
fn test_merged_config_project_overrides() {
    let global = GlobalConfig::default();
    let project = ProjectConfig {
        default_agent: Some("codex".to_string()),
        base_branch: Some("develop".to_string()),
        github_url: Some("https://github.com/user/repo".to_string()),
        copy_files: Some(".env, .env.local".to_string()),
        init_script: Some("npm install".to_string()),
    };

    let merged = MergedConfig::merge(&global, &project);

    assert_eq!(merged.default_agent, "codex");
    assert_eq!(merged.base_branch, "develop");
    assert_eq!(merged.github_url, Some("https://github.com/user/repo".to_string()));
    assert_eq!(merged.copy_files, Some(".env, .env.local".to_string()));
    assert_eq!(merged.init_script, Some("npm install".to_string()));
}
