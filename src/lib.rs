pub mod agent;
pub mod config;
pub mod db;
pub mod git;
pub mod mcp;
pub mod skills;
pub mod tmux;
pub mod tui;

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum AppMode {
    Dashboard,
    Project(PathBuf),
}

/// Feature flags parsed from CLI arguments
#[derive(Debug, Clone, Default)]
pub struct FeatureFlags {
    /// Enable experimental features (orchestrator agent, etc.)
    pub experimental: bool,
}
