pub mod agent;
pub mod config;
pub mod db;
pub mod git;
pub mod tmux;
pub mod tui;

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum AppMode {
    Dashboard,
    Project(PathBuf),
}
