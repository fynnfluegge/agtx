//! Traits for git provider operations to enable testing with mocks.
//!
//! This module provides a generic interface for interacting with git hosting
//! providers like GitHub, GitLab, Bitbucket, etc.

use anyhow::Result;
use std::path::Path;

#[cfg(feature = "test-mocks")]
use mockall::automock;

/// State of a pull/merge request
#[derive(Debug, Clone, PartialEq)]
pub enum PullRequestState {
    Open,
    Merged,
    Closed,
    Unknown,
}

/// Operations for git hosting providers (GitHub, GitLab, etc.)
#[cfg_attr(feature = "test-mocks", automock)]
pub trait GitProviderOperations: Send + Sync {
    /// Get the state of a pull/merge request
    fn get_pr_state(&self, project_path: &Path, pr_number: i32) -> Result<PullRequestState>;

    /// Create a pull/merge request
    /// Returns (pr_number, pr_url)
    /// If `base_branch` is Some, uses `--base` to target that branch (for stacked PRs).
    fn create_pr(
        &self,
        project_path: &Path,
        title: &str,
        body: &str,
        head_branch: &str,
        base_branch: Option<String>,
    ) -> Result<(i32, String)>;
}

/// GitHub implementation using the `gh` CLI
pub struct RealGitHubOps;

fn normalize_repo_selector(remote: &str) -> Option<String> {
    let remote = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let selector = if let Some(rest) = remote.strip_prefix("git@") {
        rest.replacen(':', "/", 1)
    } else if let Some(rest) = remote.strip_prefix("ssh://git@") {
        rest.replacen(':', "/", 1)
    } else if let Some(rest) = remote
        .strip_prefix("https://")
        .or_else(|| remote.strip_prefix("http://"))
    {
        rest.to_string()
    } else {
        remote.to_string()
    };
    (selector.split('/').count() >= 3).then_some(selector)
}

fn repo_selector(project_path: &Path) -> Option<String> {
    if let Some(remote) = crate::config::ProjectConfig::load(project_path)
        .ok()
        .and_then(|config| config.github_url)
    {
        if let Some(selector) = normalize_repo_selector(&remote) {
            return Some(selector);
        }
    }
    if super::is_git_repo(project_path) {
        return None;
    }
    let output = std::process::Command::new("jj")
        .current_dir(project_path)
        .args(["git", "remote", "list"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            (fields.next() == Some("origin"))
                .then(|| fields.next())
                .flatten()
                .and_then(normalize_repo_selector)
        })
}

impl GitProviderOperations for RealGitHubOps {
    fn get_pr_state(&self, project_path: &Path, pr_number: i32) -> Result<PullRequestState> {
        let mut command = std::process::Command::new("gh");
        command.current_dir(project_path).args([
            "pr",
            "view",
            &pr_number.to_string(),
            "--json",
            "state",
        ]);
        if let Some(repo) = repo_selector(project_path) {
            command.args(["--repo", &repo]);
        }
        let output = command.output()?;

        if !output.status.success() {
            return Ok(PullRequestState::Unknown);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("MERGED") {
            Ok(PullRequestState::Merged)
        } else if stdout.contains("CLOSED") {
            Ok(PullRequestState::Closed)
        } else if stdout.contains("OPEN") {
            Ok(PullRequestState::Open)
        } else {
            Ok(PullRequestState::Unknown)
        }
    }

    fn create_pr(
        &self,
        project_path: &Path,
        title: &str,
        body: &str,
        head_branch: &str,
        base_branch: Option<String>,
    ) -> Result<(i32, String)> {
        let mut args = vec![
            "pr",
            "create",
            "--title",
            title,
            "--body",
            body,
            "--head",
            head_branch,
        ];
        if let Some(ref base) = base_branch {
            args.push("--base");
            args.push(base);
        }
        let repo = repo_selector(project_path);
        if let Some(ref repo) = repo {
            args.push("--repo");
            args.push(repo);
        }
        let output = std::process::Command::new("gh")
            .current_dir(project_path)
            .args(&args)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to create PR: {}", stderr);
        }

        let pr_url = String::from_utf8_lossy(&output.stdout).trim().to_string();

        // Extract PR number from URL (e.g., https://github.com/owner/repo/pull/123)
        let pr_number = pr_url
            .split('/')
            .last()
            .and_then(|s| s.parse::<i32>().ok())
            .unwrap_or(0);

        Ok((pr_number, pr_url))
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_repo_selector;

    #[test]
    fn normalizes_https_and_ssh_github_remotes() {
        assert_eq!(
            normalize_repo_selector("https://github.com/acme/widgets.git").as_deref(),
            Some("github.com/acme/widgets")
        );
        assert_eq!(
            normalize_repo_selector("git@github.example.com:acme/widgets.git").as_deref(),
            Some("github.example.com/acme/widgets")
        );
    }
}
