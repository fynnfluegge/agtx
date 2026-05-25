use anyhow::Result;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use crate::agent::known_agents;
use crate::config::{GlobalConfig, ProjectConfig};
use crate::db::Database;

pub fn run(args: &[&str]) -> Result<()> {
    match args.first().copied() {
        Some("list") => cmd_list(&args[1..]),
        Some("set") => cmd_set(&args[1..]),
        Some("projects") => cmd_projects(),
        Some("delete-project") => cmd_delete_project(&args[1..]),
        Some("model") => cmd_model(&args[1..]),
        _ => {
            print_help();
            Ok(())
        }
    }
}

fn print_help() {
    println!("Usage: agtx config <subcommand> [options]");
    println!();
    println!("Subcommands:");
    println!("  list                                     Show global config");
    println!("  list --project [path]                    Show project config (default: cwd)");
    println!("  set <key> <value>                        Set a global config value");
    println!("  set --project [path] <key> <value>       Set a project config value");
    println!("  model <agent>                            Set the default agent (global)");
    println!("  model --project [path] <agent>           Set the default agent (project)");
    println!("  projects                                 List all known projects");
    println!("  delete-project <id|name|path>            Delete a project from the index");
    println!();
    println!("Global keys: default_agent, fullscreen_on_enter, worktree.enabled,");
    println!("  worktree.auto_cleanup, worktree.dir, worktree.base_branch,");
    println!("  worktree.branch_prefix, agents.research, agents.planning,");
    println!("  agents.running, agents.review");
    println!();
    println!("Project keys: default_agent, base_branch, github_url, worktree_dir,");
    println!("  workflow_plugin, branch_prefix, init_script, cleanup_script,");
    println!("  agents.research, agents.planning, agents.running, agents.review");
    println!();
    println!("Set an Option field to 'none' to clear it.");
}

fn cmd_list(args: &[&str]) -> Result<()> {
    if args.contains(&"--project") {
        let project_path = project_path_from_args(args)?;
        let config = ProjectConfig::load(&project_path)?;
        let config_file = project_path.join(".agtx").join("config.toml");
        println!("# Project config: {}", config_file.display());
        println!();
        println!("{}", toml::to_string_pretty(&config)?);
    } else {
        let config = GlobalConfig::load()?;
        let config_path = GlobalConfig::config_path()?;
        println!("# Global config: {}", config_path.display());
        println!();
        println!("{}", toml::to_string_pretty(&config)?);
    }
    Ok(())
}

fn cmd_set(args: &[&str]) -> Result<()> {
    let (is_project, project_path, remaining) = extract_project_flag(args);

    if remaining.len() < 2 {
        eprintln!("Usage: agtx config set [--project [path]] <key> <value>");
        return Ok(());
    }

    let key = remaining[0];
    let value = remaining[1];

    if is_project {
        let path = project_path.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let mut config = ProjectConfig::load(&path)?;
        apply_project_key(&mut config, key, value)?;
        config.save(&path)?;
        println!("✓ Set {} = {}", key, value);
    } else {
        let mut config = GlobalConfig::load()?;
        apply_global_key(&mut config, key, value)?;
        config.save()?;
        println!("✓ Set {} = {}", key, value);
    }
    Ok(())
}

fn cmd_model(args: &[&str]) -> Result<()> {
    let (is_project, project_path, remaining) = extract_project_flag(args);

    if remaining.is_empty() {
        println!("Available agents:");
        for a in known_agents() {
            println!("  {}  - {}", a.name, a.description);
        }
        return Ok(());
    }

    let agent_name = remaining[0];
    let agents = known_agents();

    if !agents.iter().any(|a| a.name == agent_name) {
        let known: Vec<_> = agents.iter().map(|a| a.name.as_str()).collect();
        eprintln!("Warning: '{}' is not a recognized agent", agent_name);
        eprintln!("Known agents: {}", known.join(", "));
    }

    if is_project {
        let path = project_path.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let mut config = ProjectConfig::load(&path)?;
        config.default_agent = Some(agent_name.to_string());
        config.save(&path)?;
        println!("✓ Set default_agent = {}", agent_name);
    } else {
        let mut config = GlobalConfig::load()?;
        config.default_agent = agent_name.to_string();
        config.save()?;
        println!("✓ Set default_agent = {}", agent_name);
    }

    Ok(())
}

fn cmd_projects() -> Result<()> {
    let db = Database::open_global()?;
    let projects = db.get_all_projects()?;

    if projects.is_empty() {
        println!("No projects found.");
        return Ok(());
    }

    println!(
        "{:<12}  {:<20}  {:<40}  {}",
        "ID (prefix)", "Name", "Path", "Last opened"
    );
    println!("{}", "\u{2500}".repeat(82));

    for p in &projects {
        let id_prefix = &p.id[..8.min(p.id.len())];
        let last_opened = p.last_opened.format("%Y-%m-%d").to_string();
        println!(
            "{:<12}  {:<20}  {:<40}  {}",
            id_prefix,
            truncate(&p.name, 20),
            truncate(&p.path, 40),
            last_opened
        );
    }

    Ok(())
}

fn cmd_delete_project(args: &[&str]) -> Result<()> {
    if args.is_empty() {
        eprintln!("Usage: agtx config delete-project <id|name|path>");
        return Ok(());
    }

    let query = args[0];
    let db = Database::open_global()?;
    let projects = db.get_all_projects()?;

    let matches: Vec<_> = projects
        .iter()
        .filter(|p| p.id.starts_with(query) || p.name == query || p.path.contains(query))
        .collect();

    match matches.len() {
        0 => anyhow::bail!("No project found matching '{}'", query),
        n if n > 1 => {
            eprintln!("Multiple projects match '{}'. Be more specific:", query);
            for p in &matches {
                eprintln!("  {}  {}  {}", &p.id[..8.min(p.id.len())], p.name, p.path);
            }
            return Ok(());
        }
        _ => {}
    }

    let project = matches[0];

    println!("Project: {} ({})", project.name, project.path);
    print!("Delete? [y/N] ");
    io::stdout().flush()?;

    let mut response = String::new();
    io::stdin().lock().read_line(&mut response)?;

    if response.trim().eq_ignore_ascii_case("y") {
        db.delete_project(&project.id)?;
        let db_path = Database::project_db_path(&project.path)?;
        if db_path.exists() {
            std::fs::remove_file(&db_path)?;
        } else {
            eprintln!(
                "Warning: database file not found at {}",
                db_path.display()
            );
        }
        println!("✓ Deleted project '{}'", project.name);
    } else {
        println!("Cancelled.");
    }

    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Parse `--project [path]` flag from args. Returns (found, path, remaining_args).
/// Path is optional — if next arg after --project doesn't look like a path, uses None.
fn extract_project_flag<'a>(args: &[&'a str]) -> (bool, Option<PathBuf>, Vec<&'a str>) {
    let mut i = 0;
    let mut found = false;
    let mut path: Option<PathBuf> = None;
    let mut remaining = Vec::new();

    while i < args.len() {
        if args[i] == "--project" {
            found = true;
            i += 1;
            if i < args.len() && looks_like_path(args[i]) {
                path = Some(PathBuf::from(args[i]));
                i += 1;
            }
        } else {
            remaining.push(args[i]);
            i += 1;
        }
    }

    (found, path, remaining)
}

/// Derive project path from args containing `--project [path]`.
fn project_path_from_args(args: &[&str]) -> Result<PathBuf> {
    let (_, path, _) = extract_project_flag(args);
    Ok(path.unwrap_or_else(|| std::env::current_dir().unwrap_or_default()))
}

fn looks_like_path(s: &str) -> bool {
    s.starts_with('/') || s.starts_with("./") || s.starts_with("../") || s.starts_with('~')
}

fn parse_bool(value: &str) -> Result<bool> {
    match value.to_lowercase().as_str() {
        "true" | "yes" | "1" | "on" => Ok(true),
        "false" | "no" | "0" | "off" => Ok(false),
        _ => anyhow::bail!("Invalid boolean value: '{}' (use true/false)", value),
    }
}

fn parse_optional_string(value: &str) -> Option<String> {
    if value.eq_ignore_ascii_case("none") || value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn apply_global_key(config: &mut GlobalConfig, key: &str, value: &str) -> Result<()> {
    match key {
        "default_agent" => config.default_agent = value.to_string(),
        "fullscreen_on_enter" => config.fullscreen_on_enter = parse_bool(value)?,
        "worktree.enabled" => config.worktree.enabled = parse_bool(value)?,
        "worktree.auto_cleanup" => config.worktree.auto_cleanup = parse_bool(value)?,
        "worktree.dir" => config.worktree.worktree_dir = value.to_string(),
        "worktree.base_branch" => config.worktree.base_branch = value.to_string(),
        "worktree.branch_prefix" => config.worktree.branch_prefix = value.to_string(),
        "agents.research" => config.agents.research = parse_optional_string(value),
        "agents.planning" => config.agents.planning = parse_optional_string(value),
        "agents.running" => config.agents.running = parse_optional_string(value),
        "agents.review" => config.agents.review = parse_optional_string(value),
        _ => anyhow::bail!(
            "Unknown global key: '{}'\nRun 'agtx config' to see available keys.",
            key
        ),
    }
    Ok(())
}

fn apply_project_key(config: &mut ProjectConfig, key: &str, value: &str) -> Result<()> {
    match key {
        "default_agent" => config.default_agent = parse_optional_string(value),
        "base_branch" => config.base_branch = parse_optional_string(value),
        "github_url" => config.github_url = parse_optional_string(value),
        "worktree_dir" => config.worktree_dir = parse_optional_string(value),
        "workflow_plugin" => config.workflow_plugin = parse_optional_string(value),
        "branch_prefix" => config.branch_prefix = parse_optional_string(value),
        "init_script" => config.init_script = parse_optional_string(value),
        "cleanup_script" => config.cleanup_script = parse_optional_string(value),
        "agents.research" => {
            config
                .agents
                .get_or_insert_with(Default::default)
                .research = parse_optional_string(value);
        }
        "agents.planning" => {
            config
                .agents
                .get_or_insert_with(Default::default)
                .planning = parse_optional_string(value);
        }
        "agents.running" => {
            config.agents.get_or_insert_with(Default::default).running =
                parse_optional_string(value);
        }
        "agents.review" => {
            config.agents.get_or_insert_with(Default::default).review =
                parse_optional_string(value);
        }
        _ => anyhow::bail!(
            "Unknown project key: '{}'\nRun 'agtx config' to see available keys.",
            key
        ),
    }
    Ok(())
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Ensure we don't split a multi-byte char boundary
        let end = s
            .char_indices()
            .map(|(i, _)| i)
            .take(max_len - 1)
            .last()
            .unwrap_or(0);
        format!("{}\u{2026}", &s[..end])
    }
}
