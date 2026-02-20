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
    fn create_pr(
        &self,
        project_path: &Path,
        title: &str,
        body: &str,
        head_branch: &str,
    ) -> Result<(i32, String)>;
}

/// GitHub implementation using the `gh` CLI
pub struct RealGitHubOps;

impl GitProviderOperations for RealGitHubOps {
    fn get_pr_state(&self, project_path: &Path, pr_number: i32) -> Result<PullRequestState> {
        let output = std::process::Command::new("gh")
            .current_dir(project_path)
            .args(["pr", "view", &pr_number.to_string(), "--json", "state"])
            .output()?;

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
    ) -> Result<(i32, String)> {
        let output = std::process::Command::new("gh")
            .current_dir(project_path)
            .args([
                "pr", "create",
                "--title", title,
                "--body", body,
                "--head", head_branch,
            ])
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
