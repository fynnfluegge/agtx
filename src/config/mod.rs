use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Global configuration (stored in ~/.config/agtx/)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    /// Default agent for new tasks
    #[serde(default = "default_agent")]
    pub default_agent: String,

    /// Worktree settings
    #[serde(default)]
    pub worktree: WorktreeConfig,

    /// UI theme/colors
    #[serde(default)]
    pub theme: ThemeConfig,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            default_agent: default_agent(),
            worktree: WorktreeConfig::default(),
            theme: ThemeConfig::default(),
        }
    }
}

/// Theme configuration with hex colors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    /// Border color for selected elements (hex, e.g. "#FFFF00")
    #[serde(default = "default_color_selected")]
    pub color_selected: String,

    /// Border color for normal/unselected elements (hex, e.g. "#00FFFF")
    #[serde(default = "default_color_normal")]
    pub color_normal: String,

    /// Border color for dimmed/inactive elements (hex, e.g. "#666666")
    #[serde(default = "default_color_dimmed")]
    pub color_dimmed: String,

    /// Text color for titles (hex, e.g. "#FFFFFF")
    #[serde(default = "default_color_text")]
    pub color_text: String,

    /// Accent color for highlights (hex, e.g. "#00FFFF")
    #[serde(default = "default_color_accent")]
    pub color_accent: String,

    /// Color for task descriptions (hex, e.g. "#FFB6C1")
    #[serde(default = "default_color_description")]
    pub color_description: String,

    /// Color for column headers when not selected (hex, e.g. "#AAAAAA")
    #[serde(default = "default_color_column_header")]
    pub color_column_header: String,

    /// Color for popup borders (hex, e.g. "#00FF00")
    #[serde(default = "default_color_popup_border")]
    pub color_popup_border: String,

    /// Background color for popup headers (hex, e.g. "#00FFFF")
    #[serde(default = "default_color_popup_header")]
    pub color_popup_header: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            color_selected: default_color_selected(),
            color_normal: default_color_normal(),
            color_dimmed: default_color_dimmed(),
            color_text: default_color_text(),
            color_accent: default_color_accent(),
            color_description: default_color_description(),
            color_column_header: default_color_column_header(),
            color_popup_border: default_color_popup_border(),
            color_popup_header: default_color_popup_header(),
        }
    }
}

fn default_color_selected() -> String {
    "#ead49a".to_string() // Yellow
}

fn default_color_normal() -> String {
    "#5cfff7".to_string() // Cyan
}

fn default_color_dimmed() -> String {
    "#9C9991".to_string() // Dark Gray
}

fn default_color_text() -> String {
    "#f2ece6".to_string() // Light Rose
}

fn default_color_accent() -> String {
    "#5cfff7".to_string() // Cyan
}

fn default_color_description() -> String {
    "#C4B0AC".to_string() // Rose (dimmed 80%)
}

fn default_color_column_header() -> String {
    "#a0d2fa".to_string() // Light Blue Gray
}

fn default_color_popup_border() -> String {
    "#9ffcf8".to_string() // Light Cyan
}

fn default_color_popup_header() -> String {
    "#69fae7".to_string() // Light Cyan
}

impl ThemeConfig {
    /// Parse a hex color string to RGB tuple
    pub fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
        let hex = hex.trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some((r, g, b))
    }
}

fn default_agent() -> String {
    "claude".to_string()
}

/// Worktree configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeConfig {
    /// Whether to use git worktrees for task isolation
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Automatically clean up worktrees after merge/reject
    #[serde(default = "default_true")]
    pub auto_cleanup: bool,

    /// Base branch to create worktrees from
    #[serde(default = "default_base_branch")]
    pub base_branch: String,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_cleanup: true,
            base_branch: "main".to_string(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_base_branch() -> String {
    "main".to_string()
}

/// Project-specific configuration (stored in .agtx/config.toml)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    /// Override default agent for this project
    pub default_agent: Option<String>,

    /// Override base branch for this project
    pub base_branch: Option<String>,

    /// GitHub URL for this project
    pub github_url: Option<String>,

    /// Comma-separated list of files to copy from project root into worktrees
    pub copy_files: Option<String>,

    /// Shell command to run inside the worktree after creation and file copying
    pub init_script: Option<String>,
}

impl GlobalConfig {
    /// Load global config from default location
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config from {:?}", config_path))?;
            toml::from_str(&content).context("Failed to parse global config")
        } else {
            Ok(Self::default())
        }
    }

    /// Save global config to default location
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, content)?;

        Ok(())
    }

    /// Get the path to the global config file
    pub fn config_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "agtx")
            .context("Could not determine config directory")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Get the path to the global data directory
    pub fn data_dir() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "agtx")
            .context("Could not determine data directory")?;
        Ok(dirs.data_dir().to_path_buf())
    }
}

impl ProjectConfig {
    /// Load project config from a project directory
    pub fn load(project_path: &Path) -> Result<Self> {
        let config_path = project_path.join(".agtx").join("config.toml");

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .with_context(|| format!("Failed to read config from {:?}", config_path))?;
            toml::from_str(&content).context("Failed to parse project config")
        } else {
            Ok(Self::default())
        }
    }

    /// Save project config
    pub fn save(&self, project_path: &Path) -> Result<()> {
        let config_path = project_path.join(".agtx").join("config.toml");

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, content)?;

        Ok(())
    }
}

/// Merged configuration (global + project)
#[derive(Debug, Clone)]
pub struct MergedConfig {
    pub default_agent: String,
    pub worktree_enabled: bool,
    pub auto_cleanup: bool,
    pub base_branch: String,
    pub github_url: Option<String>,
    pub theme: ThemeConfig,
    pub copy_files: Option<String>,
    pub init_script: Option<String>,
}

impl MergedConfig {
    /// Create merged config from global and project configs
    pub fn merge(global: &GlobalConfig, project: &ProjectConfig) -> Self {
        Self {
            default_agent: project
                .default_agent
                .clone()
                .unwrap_or_else(|| global.default_agent.clone()),
            worktree_enabled: global.worktree.enabled,
            auto_cleanup: global.worktree.auto_cleanup,
            base_branch: project
                .base_branch
                .clone()
                .unwrap_or_else(|| global.worktree.base_branch.clone()),
            github_url: project.github_url.clone(),
            theme: global.theme.clone(),
            copy_files: project.copy_files.clone(),
            init_script: project.init_script.clone(),
        }
    }
}
