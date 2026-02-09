mod agent;
mod config;
mod db;
mod git;
mod tmux;
mod tui;

use anyhow::Result;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();

    let mode = match args.get(1).map(|s| s.as_str()) {
        Some("-g") => AppMode::Dashboard,
        Some(".") => AppMode::Project(std::env::current_dir()?),
        Some(path) => AppMode::Project(PathBuf::from(path)),
        None => {
            // Default: if in git repo, use project mode; otherwise dashboard
            let current_dir = std::env::current_dir()?;
            if git::is_git_repo(&current_dir) {
                AppMode::Project(current_dir)
            } else {
                AppMode::Dashboard
            }
        }
    };

    // Initialize and run the app
    let mut app = tui::App::new(mode)?;
    app.run().await?;

    Ok(())
}

#[derive(Debug, Clone)]
pub enum AppMode {
    Dashboard,
    Project(PathBuf),
}
