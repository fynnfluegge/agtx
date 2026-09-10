//! Stable, VCS-neutral commands used by agtx's agent prompts.

use crate::config::ProjectConfig;
use crate::db::Database;
use crate::git::{self, VcsKind};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct TaskContext {
    project: PathBuf,
    checkout: PathBuf,
    task: crate::db::Task,
    kind: VcsKind,
    base: String,
}

impl TaskContext {
    fn discover() -> Result<Self> {
        let project = std::env::var_os("AGTX_PROJECT_ROOT")
            .map(PathBuf::from)
            .context("not in an agtx task: AGTX_PROJECT_ROOT is not set")?
            .canonicalize()
            .context("AGTX_PROJECT_ROOT does not resolve")?;
        let task_id = std::env::var("AGTX_TASK_ID")
            .context("not in an agtx task: AGTX_TASK_ID is not set")?;
        let checkout_env = std::env::var_os("AGTX_WORKTREE")
            .map(PathBuf::from)
            .context("not in an agtx task: AGTX_WORKTREE is not set")?;
        let checkout = checkout_env
            .canonicalize()
            .context("AGTX_WORKTREE does not resolve")?;
        let cwd = std::env::current_dir()?.canonicalize()?;
        if !cwd.starts_with(&checkout) {
            bail!(
                "current directory '{}' is outside task checkout '{}'",
                cwd.display(),
                checkout.display()
            );
        }

        let db = Database::open_project(&project)?;
        let task = db
            .get_task(&task_id)?
            .with_context(|| format!("task '{task_id}' is not present in the project database"))?;
        let recorded = task
            .worktree_path
            .as_deref()
            .context("task has no recorded checkout")?;
        if Path::new(recorded).canonicalize()? != checkout {
            bail!("task database checkout does not match AGTX_WORKTREE");
        }

        // A missing value is always Git. Selection is never inferred from .jj.
        let kind = task.vcs.unwrap_or_default();
        if !git::is_repo_for(&checkout, kind) {
            bail!(
                "task records backend '{}', but '{}' is not a {} checkout",
                kind,
                checkout.display(),
                kind
            );
        }
        let configured_base = ProjectConfig::load(&project)?
            .base_branch
            .filter(|base| !base.trim().is_empty());
        let base = task
            .base_branch
            .as_deref()
            .map(str::trim)
            .filter(|base| !base.is_empty())
            .map(str::to_string)
            .or(configured_base)
            .map(Ok)
            .unwrap_or_else(|| match kind {
                VcsKind::Git => git::detect_main_branch(&project),
                VcsKind::Jj => Ok("trunk()".to_string()),
            })?;
        Ok(Self {
            project,
            checkout,
            task,
            kind,
            base,
        })
    }
}

fn output(command: &mut Command, label: &str) -> Result<Output> {
    let result = command
        .output()
        .with_context(|| format!("failed to run {label}"))?;
    if !result.status.success() {
        bail!(
            "{label} failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(result)
}

fn print_output(result: Output) {
    print!("{}", String::from_utf8_lossy(&result.stdout));
    eprint!("{}", String::from_utf8_lossy(&result.stderr));
}

fn git_command(ctx: &TaskContext, args: &[&str]) -> Result<Output> {
    output(
        Command::new("git").current_dir(&ctx.checkout).args(args),
        "git",
    )
}

fn jj_command(ctx: &TaskContext, args: &[&str]) -> Result<Output> {
    output(
        Command::new("jj").current_dir(&ctx.checkout).args(args),
        "jj",
    )
}

fn git_ref_exists(ctx: &TaskContext, revision: &str) -> bool {
    Command::new("git")
        .current_dir(&ctx.checkout)
        .args(["rev-parse", "--verify", "--quiet", revision])
        .output()
        .map(|result| result.status.success())
        .unwrap_or(false)
}

/// Prefer a fetched remote-tracking revision when the configured Git base is
/// a branch. Commit IDs and other revision expressions fall back unchanged.
fn git_remote_base(ctx: &TaskContext) -> String {
    let base = ctx.base.strip_prefix("refs/heads/").unwrap_or(&ctx.base);
    let candidate = if let Some(remote) = base.strip_prefix("refs/remotes/") {
        remote.to_string()
    } else if base.starts_with("origin/") {
        base.to_string()
    } else {
        format!("origin/{base}")
    };
    if git_ref_exists(ctx, &candidate) {
        candidate
    } else {
        ctx.base.clone()
    }
}

/// Avoid invoking `jj resolve --list` for a clean revision: jj uses exit 2 for
/// that ordinary state. The revset check gives the command an unambiguous
/// success path and reserves nonzero exits for actual failures.
fn jj_conflicts(ctx: &TaskContext) -> Result<Vec<String>> {
    let conflicted = jj_command(
        ctx,
        &[
            "log",
            "-r",
            "@ & conflicts()",
            "--no-graph",
            "-T",
            "commit_id",
        ],
    )?;
    if conflicted.stdout.is_empty() {
        return Ok(Vec::new());
    }
    let result = jj_command(ctx, &["resolve", "--list"])?;
    Ok(String::from_utf8_lossy(&result.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

fn jj_has_revision(ctx: &TaskContext, revset: &str) -> Result<bool> {
    let result = jj_command(ctx, &["log", "-r", revset, "--no-graph", "-T", "commit_id"])?;
    Ok(!result.stdout.is_empty())
}

fn task_diff(ctx: &TaskContext, extra: &[String]) -> Result<()> {
    let mut command = match ctx.kind {
        VcsKind::Git => {
            let mut cmd = Command::new("git");
            cmd.current_dir(&ctx.checkout)
                .args(["diff", &format!("{}...HEAD", ctx.base)]);
            cmd
        }
        VcsKind::Jj => {
            let mut cmd = Command::new("jj");
            let fork = format!("fork_point(({}) | @)", ctx.base);
            cmd.current_dir(&ctx.checkout)
                .args(["diff", "--git", "--from", &fork, "--to", "@"]);
            cmd
        }
    };
    if extra.iter().any(|arg| arg == "--stat") {
        command.arg("--stat");
    }
    if let Some(separator) = extra.iter().position(|arg| arg == "--") {
        command.arg("--").args(&extra[separator + 1..]);
    }
    print_output(output(&mut command, "task diff")?);
    Ok(())
}

fn task_log(ctx: &TaskContext) -> Result<()> {
    let result = match ctx.kind {
        VcsKind::Git => git_command(ctx, &["log", "--oneline", &format!("{}..HEAD", ctx.base)]),
        VcsKind::Jj => jj_command(
            ctx,
            &[
                "log",
                "-r",
                &format!("{}..@", ctx.base),
                "-T",
                "builtin_log_compact",
            ],
        ),
    }?;
    print_output(result);
    Ok(())
}

fn checkpoint(ctx: &TaskContext, message: &str) -> Result<()> {
    match ctx.kind {
        VcsKind::Git => {
            let dirty = git_command(ctx, &["status", "--porcelain"])?;
            if dirty.stdout.is_empty() {
                return Ok(());
            }
            git_command(ctx, &["add", "-A"])?;
            git_command(ctx, &["commit", "-m", message])?;
        }
        VcsKind::Jj => {
            let dirty = jj_command(ctx, &["diff", "--summary"])?;
            if dirty.stdout.is_empty() {
                return Ok(());
            }
            jj_command(ctx, &["describe", "-m", message])?;
            jj_command(ctx, &["new"])?;
        }
    }
    Ok(())
}

fn conflicts(ctx: &TaskContext) -> Result<()> {
    match ctx.kind {
        VcsKind::Git => {
            let current = git_command(ctx, &["diff", "--name-only", "--diff-filter=U"])?;
            if !current.stdout.is_empty() {
                println!("conflicts: yes");
                print!("{}", String::from_utf8_lossy(&current.stdout));
                return Ok(());
            }
        }
        VcsKind::Jj => {
            let current = jj_conflicts(ctx)?;
            if !current.is_empty() {
                println!("conflicts: yes");
                for path in current {
                    println!("{path}");
                }
                return Ok(());
            }
        }
    }
    let (conflicted, files) = match ctx.kind {
        VcsKind::Git => {
            let branch = ctx.task.branch_name.as_deref().unwrap_or("HEAD");
            git::check_merge_conflicts(&ctx.checkout, &git_remote_base(ctx), branch)?
        }
        VcsKind::Jj => git::jj_conflict_probe(&ctx.checkout, "@", &ctx.base)?,
    };
    if conflicted {
        println!("conflicts: yes");
        for file in files {
            println!("{file}");
        }
    } else {
        println!("conflicts: no");
    }
    Ok(())
}

fn integrate_base(ctx: &TaskContext, fetch: bool) -> Result<()> {
    if fetch {
        match ctx.kind {
            VcsKind::Git => {
                git_command(ctx, &["fetch", "origin"])?;
            }
            VcsKind::Jj => {
                jj_command(ctx, &["git", "fetch", "--remote", "origin"])?;
            }
        }
    }
    match ctx.kind {
        VcsKind::Git => {
            let base = if fetch {
                git_remote_base(ctx)
            } else {
                ctx.base.clone()
            };
            print_output(git_command(
                ctx,
                &["merge", "--no-ff", &base, "-m", &format!("Merge {base}")],
            )?);
        }
        VcsKind::Jj => {
            let base_is_ancestor = format!("({}) & ::@", ctx.base);
            if jj_has_revision(ctx, &base_is_ancestor)? {
                return Ok(());
            }
            print_output(jj_command(
                ctx,
                &["new", "@", &ctx.base, "-m", &format!("Merge {}", ctx.base)],
            )?);
        }
    }
    Ok(())
}

fn finish_integration(ctx: &TaskContext) -> Result<()> {
    match ctx.kind {
        VcsKind::Git => {
            let unresolved = git_command(ctx, &["diff", "--name-only", "--diff-filter=U"])?;
            if !unresolved.stdout.is_empty() {
                bail!(
                    "unresolved conflicts remain:\n{}",
                    String::from_utf8_lossy(&unresolved.stdout)
                );
            }
            // A clean Git merge commits immediately. Only finish a commit when
            // an in-progress merge remains after conflict repair.
            if git_ref_exists(ctx, "MERGE_HEAD") {
                git_command(ctx, &["add", "-A"])?;
                git_command(ctx, &["commit", "--no-edit"])?;
            }
        }
        VcsKind::Jj => {
            let unresolved = jj_conflicts(ctx)?;
            if !unresolved.is_empty() {
                bail!("unresolved conflicts remain:\n{}", unresolved.join("\n"));
            }
            jj_command(ctx, &["st"])?;
        }
    }
    Ok(())
}

fn usage() -> &'static str {
    "agtx vcs — VCS-neutral operations for an agtx task\n\n\
USAGE\n  agtx vcs info\n  agtx vcs status\n  agtx vcs diff --task [--stat] [-- <paths>]\n  agtx vcs log --task\n  agtx vcs checkpoint -m <message>\n  agtx vcs conflicts\n  agtx vcs integrate-base [--fetch]\n  agtx vcs finish-integration"
}

pub fn run(args: &[String]) -> Result<()> {
    let Some(command) = args.first().map(String::as_str) else {
        println!("{}", usage());
        return Ok(());
    };
    if matches!(command, "help" | "--help" | "-h") {
        println!("{}", usage());
        return Ok(());
    }
    let ctx = TaskContext::discover()?;
    match command {
        "info" => {
            println!("task:       {}", ctx.task.id);
            println!("backend:    {}", ctx.kind);
            println!("project:    {}", ctx.project.display());
            println!("workspace:  {}", ctx.checkout.display());
            println!("base:       {}", ctx.base);
            println!(
                "publish:    {}",
                ctx.task.branch_name.as_deref().unwrap_or("(unset)")
            );
        }
        "status" => print_output(match ctx.kind {
            VcsKind::Git => git_command(&ctx, &["status", "--short"]),
            VcsKind::Jj => jj_command(&ctx, &["st"]),
        }?),
        "diff" => task_diff(&ctx, &args[1..])?,
        "log" => task_log(&ctx)?,
        "checkpoint" => {
            let message = args
                .windows(2)
                .find(|pair| pair[0] == "-m" || pair[0] == "--message")
                .map(|pair| pair[1].as_str())
                .context("checkpoint requires -m <message>")?;
            checkpoint(&ctx, message)?;
        }
        "conflicts" => conflicts(&ctx)?,
        "integrate-base" => integrate_base(&ctx, args.iter().any(|arg| arg == "--fetch"))?,
        "finish-integration" => finish_integration(&ctx)?,
        other => bail!("unknown agtx vcs command '{other}'\n\n{}", usage()),
    }
    Ok(())
}
