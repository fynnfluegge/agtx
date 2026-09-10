mod jj_operations;
mod operations;
mod provider;
mod vcs;
mod worktree;

pub use operations::*;
pub use provider::{GitProviderOperations, PullRequestState, RealGitHubOps};
pub use vcs::VcsKind;
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

/// Check whether `path` is the root of a Jujutsu workspace. This validates an
/// explicit `vcs = "jj"` selection; it is never used to choose a backend.
///
/// Comparing the reported root matters because `jj workspace root` walks up
/// through ordinary subdirectories. A half-created task directory inside the
/// project must not be mistaken for a registered workspace.
pub fn is_jj_repo(path: &Path) -> bool {
    let Ok(requested) = path.canonicalize() else {
        return false;
    };
    let Ok(output) = Command::new("jj")
        .current_dir(path)
        .args(["workspace", "root"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    Path::new(String::from_utf8_lossy(&output.stdout).trim())
        .canonicalize()
        .map(|root| root == requested)
        .unwrap_or(false)
}

pub fn is_repo_for(path: &Path, kind: VcsKind) -> bool {
    match kind {
        VcsKind::Git => is_git_repo(path),
        VcsKind::Jj => is_jj_repo(path),
    }
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
        anyhow::bail!("Merge failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(())
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
pub use jj_operations::{
    conflict_probe as jj_conflict_probe, resolve_revision as resolve_jj_revision, RealJjOps,
};
