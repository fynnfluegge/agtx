mod operations;
mod provider;
mod worktree;

pub use operations::*;
pub use provider::{GitProviderOperations, PullRequestState, RealGitHubOps};
pub use worktree::*;

#[cfg(feature = "test-mocks")]
pub use operations::MockGitOperations;
#[cfg(feature = "test-mocks")]
pub use provider::MockGitProviderOperations;

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Check if a path is inside a git repository
pub fn is_git_repo(path: &Path) -> bool {
    Command::new("git")
        .current_dir(path)
        .args(["rev-parse", "--git-dir"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get the root directory of the git repository
pub fn repo_root(path: &Path) -> Result<std::path::PathBuf> {
    let output = Command::new("git")
        .current_dir(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("Failed to get git root")?;

    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();

    Ok(std::path::PathBuf::from(root))
}

/// Get current branch name
pub fn current_branch(path: &Path) -> Result<String> {
    let output = Command::new("git")
        .current_dir(path)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .context("Failed to get current branch")?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// How many commits `branch` has that `base` does not.
pub fn commits_ahead(path: &Path, base: &str, branch: &str) -> Result<usize> {
    let output = Command::new("git")
        .current_dir(path)
        .args(["rev-list", "--count", &format!("{base}..{branch}")])
        .output()
        .context("Failed to count commits ahead of the base branch")?;

    if !output.status.success() {
        anyhow::bail!(
            "Failed to compare '{branch}' with '{base}': {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap_or(0))
}

/// The commit HEAD currently points at, as a full SHA.
pub fn head_sha(path: &Path) -> Result<String> {
    let output = Command::new("git")
        .current_dir(path)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("Failed to read HEAD")?;

    if !output.status.success() {
        anyhow::bail!(
            "Failed to read HEAD: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Whether `rev` resolves to a commit in the repository at `path`.
///
/// Needed because [`diff_stat`] and [`diff_full`] only fail when git cannot be
/// *spawned*: a nonzero exit — `fatal: ambiguous argument 'main'` for a base
/// branch that does not exist — leaves stdout empty and returns `Ok("")`. A
/// caller that renders that as "no changes" is asserting something it was never
/// told, which on a review screen is a claim someone may merge on.
pub fn ref_exists(path: &Path, rev: &str) -> bool {
    Command::new("git")
        .current_dir(path)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get the diff between two branches (stat format)
pub fn diff_stat(path: &Path, base: &str, target: &str) -> Result<String> {
    let output = Command::new("git")
        .current_dir(path)
        .args(["diff", base, target, "--stat"])
        .output()
        .context("Failed to get diff")?;

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Get the full diff between two branches
pub fn diff_full(path: &Path, base: &str, target: &str) -> Result<String> {
    let output = Command::new("git")
        .current_dir(path)
        .args(["diff", base, target])
        .output()
        .context("Failed to get diff")?;

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Merge a branch into the current branch
pub fn merge_branch(path: &Path, branch: &str, message: &str) -> Result<()> {
    let output = Command::new("git")
        .current_dir(path)
        .args(["merge", branch, "--no-ff", "-m", message])
        .output()
        .context("Failed to merge branch")?;

    if !output.status.success() {
        // A conflicted merge leaves MERGE_HEAD and a half-written index behind.
        // Leaving that for the user to find is how a failed automatic merge
        // turns into a repository nobody can explain — every later git command
        // in the project root reports a merge in progress.
        let _ = Command::new("git")
            .current_dir(path)
            .args(["merge", "--abort"])
            .output();
        anyhow::bail!("Merge failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(())
}

/// What happened when a task branch was merged into its base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    /// The merge commit was made.
    Merged,
    /// The branch has no commits the base does not already have, so there is
    /// nothing to land.
    ///
    /// Distinguished from [`Merged`](Self::Merged) because `git merge` reports
    /// success for this case — "Already up to date", exit 0 — and a caller that
    /// believes it would conclude the work is safely on the base branch. It is
    /// not: the usual cause is an agent that wrote files and never committed
    /// them, so the work exists only in the worktree's working directory, which
    /// is about to be deleted.
    NothingToMerge,
    /// The branches conflict. Nothing was changed; the files are the ones the
    /// virtual merge reported.
    Conflicts(Vec<String>),
    /// The merge was not attempted, because the working tree is not in a state
    /// where merging into it would be safe. Carries the reason, for a user.
    Refused(String),
}

/// Merge a task branch into `base` in the project's own working tree.
///
/// Merging needs a working tree, and the project root is the only one that
/// belongs to `base`. That makes this the one agtx operation that writes to the
/// user's own checkout, so it is deliberately narrow: it merges when the
/// checkout is *already* on `base` with no tracked modifications, and refuses
/// otherwise rather than stashing, switching branches, or moving HEAD out from
/// under whatever the user is doing.
///
/// The alternatives were considered and are worse. `git fetch . branch:base`
/// needs no working tree but git refuses it while `base` is checked out, which
/// is the normal case here. `git update-ref` evades that check but leaves the
/// user's index disagreeing with HEAD — the working tree silently reads as
/// "everything deleted". A detached integration worktree merges cleanly but
/// then faces the same problem moving `base` to the result.
///
/// Untracked files are not part of the gate: they do not block a merge, and the
/// project root always has some (worktrees live under `.agtx/`). git refuses on
/// its own if the merge would overwrite one, and that arrives as `Refused`.
pub fn merge_task_branch(
    project_path: &Path,
    base: &str,
    branch: &str,
    message: &str,
) -> Result<MergeOutcome> {
    let current = current_branch(project_path)?;
    if current != base {
        return Ok(MergeOutcome::Refused(format!(
            "project checkout is on '{current}', not the base branch '{base}'"
        )));
    }

    let dirty = Command::new("git")
        .current_dir(project_path)
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .context("Failed to check the project working tree")?;
    if !dirty.stdout.is_empty() {
        return Ok(MergeOutcome::Refused(format!(
            "project checkout has uncommitted changes on '{base}'"
        )));
    }

    // A branch with nothing on it is not a merge, whatever git's exit status
    // says. Reported rather than performed, because the caller's next step is
    // to delete the worktree and an empty branch means the work — if there is
    // any — is still sitting in it uncommitted.
    if commits_ahead(project_path, base, branch)? == 0 {
        return Ok(MergeOutcome::NothingToMerge);
    }

    // Ask before acting: `git merge` on a conflict leaves the working tree mid-
    // merge, and this is the user's checkout. The virtual merge touches nothing
    // and names the files, which is what the agent needs to resolve them.
    let (conflicts, files) = check_merge_conflicts(project_path, base, branch)?;
    if conflicts {
        return Ok(MergeOutcome::Conflicts(files));
    }

    // The check above is not a guarantee — the branch can move between the two
    // commands — so a merge that fails anyway is aborted and reported as a
    // conflict rather than trusted to have left nothing behind.
    match merge_branch(project_path, branch, message) {
        Ok(()) => Ok(MergeOutcome::Merged),
        Err(e) => {
            let (conflicts, files) = check_merge_conflicts(project_path, base, branch)
                .unwrap_or((false, Vec::new()));
            if conflicts {
                Ok(MergeOutcome::Conflicts(files))
            } else {
                Ok(MergeOutcome::Refused(e.to_string()))
            }
        }
    }
}

/// Check if merging a branch into the base branch would produce conflicts.
/// Uses `git merge-tree --write-tree` (Git 2.38+) for a non-destructive check.
/// Returns Ok((has_conflicts, conflicting_files)).
pub fn check_merge_conflicts(path: &Path, base: &str, branch: &str) -> Result<(bool, Vec<String>)> {
    let output = Command::new("git")
        .current_dir(path)
        .args(["merge-tree", "--write-tree", base, branch])
        .output()
        .context("Failed to run git merge-tree")?;

    if output.status.success() {
        return Ok((false, vec![]));
    }

    // Non-zero exit: parse structured output for conflicting files.
    // git merge-tree outputs lines like "100644 <hash> <stage> <tab><filename>"
    // where stage 1/2/3 indicates conflict (base/ours/theirs).
    // This format is locale-independent (unlike the human-readable CONFLICT messages).
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut seen = std::collections::HashSet::new();
    let conflicting_files: Vec<String> = stdout
        .lines()
        .filter_map(|line| {
            // Match lines like: "100644 abc123 1\tpath/to/file" or "100644 abc123 2\tpath/to/file"
            let parts: Vec<&str> = line.splitn(4, |c: char| c.is_whitespace()).collect();
            if parts.len() == 4 {
                let stage = parts[2];
                if matches!(stage, "1" | "2" | "3") {
                    let filename = parts[3].trim();
                    if !filename.is_empty() && seen.insert(filename.to_string()) {
                        return Some(filename.to_string());
                    }
                }
            }
            None
        })
        .collect();

    Ok((true, conflicting_files))
}

/// Delete a branch
pub fn delete_branch(path: &Path, branch: &str, force: bool) -> Result<()> {
    let flag = if force { "-D" } else { "-d" };

    Command::new("git")
        .current_dir(path)
        .args(["branch", flag, branch])
        .output()
        .context("Failed to delete branch")?;

    Ok(())
}
