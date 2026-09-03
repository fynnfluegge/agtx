use agtx::{
    agent,
    config::{self, GlobalConfig},
    git, tui, AppMode, FeatureFlags,
};
use anyhow::{Context, Result};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    // Fast path: `agtx hook <task-id> <worktree> [agent]` is invoked by agent
    // lifecycle hooks, potentially on every tool call. Handle it before the
    // logging and config setup below — building a daily rolling file appender
    // per invocation is pure waste, and the agent consumes this process's stdout.
    {
        let raw: Vec<String> = std::env::args().collect();
        if raw.get(1).map(String::as_str) == Some("hook") {
            return agtx::agent::hook_status::run_hook_cli(&raw[2..]);
        }
        // Same reasoning for the version/update commands: they print a line or
        // two and exit, and neither wants a daily log file created for it. They
        // are handled here rather than in the `mode` match below because that
        // match filters out every `--`-prefixed argument, so `--version` would
        // otherwise fall through and open the current directory as a project.
        match raw.get(1).map(String::as_str) {
            Some("--version" | "-V" | "version") => {
                println!("agtx {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Some("update") => return run_update(&raw[2..]),
            _ => {}
        }
    }

    // Initialize audit logging to ~/.config/agtx/logs/
    let log_dir = GlobalConfig::config_path()?
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let file_appender = tracing_appender::rolling::daily(&log_dir, "agtx.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .json()
        .init();

    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();

    // Extract flags from any position
    let experimental = args.iter().any(|a| a == "--experimental");
    let no_init_scripts = args.iter().any(|a| a == "--no-init-scripts");
    let positional_args: Vec<&str> = args
        .iter()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .map(|s| s.as_str())
        .collect();

    let mode = match positional_args.first().copied() {
        Some("mcp-serve") => {
            let project_path = positional_args.get(1).map(PathBuf::from);
            let project_path = match project_path {
                Some(p) => {
                    let p = p.canonicalize()?;
                    if !git::is_git_repo(&p) {
                        anyhow::bail!("mcp-serve requires a git project directory");
                    }
                    Some(p)
                }
                None => None, // global mode
            };
            return agtx::mcp::serve(project_path).await;
        }
        Some("serve") => return run_serve(&args[2..]).await,
        Some("trust") => {
            let project_path = std::env::current_dir()?.canonicalize()?;
            let mut store = config::TrustStore::load().unwrap_or_default();
            store.trust_project(&project_path)?;
            println!("Trusted project config at {}", project_path.display());
            return Ok(());
        }
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

    let mut flags = FeatureFlags {
        experimental,
        no_init_scripts,
        first_run: false,
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
            // Write the defaults so a config file always exists after a first
            // launch, then let the TUI ask which agent to use: the config editor
            // already *is* that question, plus every other one, and answering it
            // there keeps first run looking like the rest of the app.
            let mut cfg = GlobalConfig::default();
            if let Some(first) = agent::detect_available_agents().first() {
                cfg.default_agent = first.name.clone();
            }
            cfg.save()?;
            flags.first_run = true;
        }
    }

    // Initialize and run the app
    let mut app = tui::App::new(mode, flags)?;
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

/// `agtx update [--check]`
///
/// The dedicated command the header notice points at. `--check` only reports,
/// and exits 1 when an update is available so it can drive a script.
/// `agtx serve [path] [--port N] [--host ADDR]`.
///
/// Lives in the `mode` match rather than the early fast path beside `hook` and
/// `--version`: it is long-running and wants the log appender those two skip.
/// The `cfg(not(...))` arm is not decoration — the match below ends with
/// `Some(path) => AppMode::Project(...)`, and `serve` is not `--`-prefixed, so
/// without it a default build would open a directory named `serve` as a project.
#[cfg(feature = "serve")]
async fn run_serve(args: &[String]) -> Result<()> {
    let parsed = parse_serve_args(args)?;
    if parsed.tunnel {
        anyhow::bail!(
            "--tunnel is not implemented yet: it needs the per-device tokens that pairing \
             introduces, and a tunnel without them would publish this machine's agent panes. \
             Use an SSH tunnel in the meantime."
        );
    }
    agtx::web::serve(parsed.opts).await
}

#[cfg(not(feature = "serve"))]
async fn run_serve(_args: &[String]) -> Result<()> {
    anyhow::bail!(
        "this agtx was built without the web server. Rebuild with `--features serve` \
         (the published binaries have it) to use `agtx serve`."
    )
}

#[cfg(feature = "serve")]
struct ParsedServeArgs {
    opts: agtx::web::ServeOptions,
    tunnel: bool,
}

/// Parse `serve`'s own arguments.
///
/// Deliberately *not* built on the `positional_args` filter above. That filter
/// drops `--`-prefixed tokens, which leaves a flag's **value** looking exactly
/// like a positional: `serve --port 8791` would take `8791` as the project
/// path and fail with "resolving 8791: no such file or directory". A flag that
/// takes a value has to consume it.
#[cfg(feature = "serve")]
fn parse_serve_args(args: &[String]) -> Result<ParsedServeArgs> {
    let mut opts = agtx::web::ServeOptions::default();
    let mut tunnel = false;
    let mut iter = args.iter().peekable();

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--port" => {
                let v = iter
                    .next()
                    .context("--port needs a value, e.g. `--port 8787`")?;
                opts.port = v
                    .parse()
                    .with_context(|| format!("--port expects a number, got {v:?}"))?;
            }
            "--host" => {
                let v = iter
                    .next()
                    .context("--host needs a value, e.g. `--host 127.0.0.1`")?;
                opts.host = v
                    .parse()
                    .with_context(|| format!("--host expects an IP address, got {v:?}"))?;
            }
            "--tunnel" => tunnel = true,
            other if other.starts_with("--") => {
                anyhow::bail!("unknown option for `agtx serve`: {other}")
            }
            path => {
                if opts.project_path.is_some() {
                    anyhow::bail!("`agtx serve` takes at most one project path, got {path:?}");
                }
                opts.project_path = Some(PathBuf::from(path));
            }
        }
    }

    Ok(ParsedServeArgs { opts, tunnel })
}

fn run_update(args: &[String]) -> Result<()> {
    use agtx::update;

    let check_only = args.iter().any(|a| a == "--check");
    let current = update::Version::current();

    // Ask GitHub directly rather than going through the cached path: someone
    // typing `agtx update` wants the answer now, not yesterday's answer.
    let release = update::github::fetch_latest_release(&update::release::repo())
        .context("could not reach GitHub to check for a new release")?;
    let latest = update::Version::parse(&release.tag_name)
        .with_context(|| format!("unrecognised release tag: {}", release.tag_name))?;

    if !latest.supersedes(&current) {
        println!("agtx {current} is up to date (latest release: {latest})");
        return Ok(());
    }

    println!("  current {current}  →  latest {latest}");
    if check_only {
        if !release.html_url.is_empty() {
            println!("  {}", release.html_url);
        }
        println!("  run `agtx update` to install it");
        std::process::exit(1);
    }

    let installed = update::install::install_release(&release.tag_name, &mut |step| {
        println!("  {step}");
    })?;

    println!();
    println!("  agtx {latest} installed to {}", installed.path.display());
    // The running process still holds the old inode; tmux sessions and their
    // agents are untouched. Say so rather than implying the swap took effect.
    println!("  restart any running agtx to pick it up");
    Ok(())
}
