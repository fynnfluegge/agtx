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

    let root = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_string();

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
        anyhow::bail!(
            "Merge failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
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
