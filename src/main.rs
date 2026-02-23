use agtx::{agent, config::{self, GlobalConfig}, git, tui, AppMode};
use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode},
    style::{self, Stylize},
    terminal, ExecutableCommand,
};
use std::io::{self, Write};
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

    // First-run: determine action based on config/data state
    let config_path = GlobalConfig::config_path()?;
    let config_exists = config_path.exists();
    let migrated = if !config_exists {
        migrate_old_config(&config_path)
    } else {
        false
    };
    let db_exists = GlobalConfig::data_dir()
        .map(|d| d.join("index.db").exists())
        .unwrap_or(false);

    match config::determine_first_run_action(config_exists, migrated, db_exists) {
        config::FirstRunAction::ConfigExists | config::FirstRunAction::Migrated => {}
        config::FirstRunAction::ExistingUserSaveDefaults => {
            GlobalConfig::default().save()?;
        }
        config::FirstRunAction::NewUserPrompt => {
            let available = agent::detect_available_agents();
            if !available.is_empty() {
                let selected = prompt_agent_selection(&available)?;
                let mut cfg = GlobalConfig::default();
                cfg.default_agent = selected.name.clone();
                cfg.save()?;
            }
        }
    }

    // Initialize and run the app
    let mut app = tui::App::new(mode)?;
    app.run().await?;

    Ok(())
}

/// Migrate config from the old location (directories crate config_dir) to the new one (~/.config/agtx/).
/// Returns true if migration was performed.
fn migrate_old_config(new_path: &std::path::Path) -> bool {
    let old_path = directories::ProjectDirs::from("", "", "agtx")
        .map(|dirs| dirs.config_dir().join("config.toml"));

    if let Some(old_path) = old_path {
        if old_path != *new_path && old_path.exists() {
            // Create parent directory for new path
            if let Some(parent) = new_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // Copy old config to new location
            if std::fs::copy(&old_path, new_path).is_ok() {
                // Remove old file after successful copy
                let _ = std::fs::remove_file(&old_path);
                return true;
            }
        }
    }
    false
}

fn prompt_agent_selection(agents: &[agent::Agent]) -> Result<&agent::Agent> {
    let mut stdout = io::stdout();
    let mut selected: usize = 0;

    // Enter raw mode for arrow key handling
    terminal::enable_raw_mode()?;
    stdout.execute(cursor::Hide)?;

    // Print ASCII art banner
    let banner: &[(&str, &str)] = &[
        ("  █████╗  ██████╗ ████████╗██╗  ██╗", ""),
        (" ██╔══██╗██╔════╝ ╚══██╔══╝╚██╗██╔╝", ""),
        (" ███████║██║  ███╗   ██║    ╚███╔╝ ", "  Autonomous multi-session"),
        (" ██╔══██║██║   ██║   ██║    ██╔██╗ ", "  AI coding in the terminal"),
        (" ██║  ██║╚██████╔╝   ██║   ██╔╝ ██╗", ""),
        (" ╚═╝  ╚═╝ ╚═════╝    ╚═╝   ╚═╝  ╚═╝", ""),
    ];
    stdout.execute(style::Print("\r\n"))?;
    for (art, tagline) in banner {
        stdout.execute(style::PrintStyledContent((*art).cyan()))?;
        if !tagline.is_empty() {
            stdout.execute(style::PrintStyledContent((*tagline).dark_grey()))?;
        }
        stdout.execute(style::Print("\r\n"))?;
    }
    stdout.execute(style::Print("\r\n"))?;
    stdout.execute(style::Print("  Select your default coding agent "))?;
    stdout.execute(style::PrintStyledContent("(can be changed later via config)\r\n\r\n".dark_grey()))?;

    // Draw the list
    let draw = |stdout: &mut io::Stdout, selected: usize| -> Result<()> {
        for (i, a) in agents.iter().enumerate() {
            if i == selected {
                stdout.execute(style::PrintStyledContent("  > ".cyan()))?;
                stdout.execute(style::PrintStyledContent(a.name.as_str().cyan().bold()))?;
                let desc = format!(" - {}", a.description);
                stdout.execute(style::PrintStyledContent(desc.as_str().dark_grey()))?;
            } else {
                stdout.execute(style::Print("    "))?;
                stdout.execute(style::Print(&a.name))?;
                let desc = format!(" - {}", a.description);
                stdout.execute(style::PrintStyledContent(desc.as_str().dark_grey()))?;
            }
            stdout.execute(style::Print("\r\n"))?;
        }
        stdout.execute(style::Print("\r\n"))?;
        stdout.execute(style::PrintStyledContent("\n".dark_grey()))?;
        stdout.flush()?;
        Ok(())
    };

    draw(&mut stdout, selected)?;

    let result = loop {
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if selected > 0 {
                        selected -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if selected < agents.len() - 1 {
                        selected += 1;
                    }
                }
                KeyCode::Enter => break Ok(selected),
                KeyCode::Esc | KeyCode::Char('q') => {
                    break Err(anyhow::anyhow!("Selection cancelled"));
                }
                KeyCode::Char('c')
                    if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                {
                    break Err(anyhow::anyhow!("Selection cancelled"));
                }
                _ => continue,
            }

            // Move cursor back up to redraw
            let lines_to_move_up = agents.len() + 2; // agents + blank + hint
            stdout.execute(cursor::MoveUp(lines_to_move_up as u16))?;
            stdout.execute(cursor::MoveToColumn(0))?;
            draw(&mut stdout, selected)?;
        }
    };

    // Restore terminal
    stdout.execute(cursor::Show)?;
    terminal::disable_raw_mode()?;

    let idx = result?;
    println!("\n  Selected: {}\n", agents[idx].name);
    Ok(&agents[idx])
}
