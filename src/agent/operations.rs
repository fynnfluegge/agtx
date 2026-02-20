//! Traits for agent operations to enable testing with mocks.
//!
//! This module provides a generic interface for interacting with coding agents
//! like Claude Code, Aider, Codex, etc.

use anyhow::Result;
use std::path::Path;

#[cfg(feature = "test-mocks")]
use mockall::automock;

use super::Agent;

/// Operations for coding agents (Claude, Aider, Codex, etc.)
#[cfg_attr(feature = "test-mocks", automock)]
pub trait AgentOperations: Send + Sync {
    /// Generate text using the agent's print/non-interactive mode
    /// Used for tasks like generating PR descriptions
    fn generate_text(&self, working_dir: &Path, prompt: &str) -> Result<String>;

    /// Get the co-author string for git commits
    /// e.g., "Claude <noreply@anthropic.com>"
    fn co_author_string(&self) -> &str;
}

/// Generic agent implementation that works with any Agent config
pub struct CodingAgent {
    agent: Agent,
}

impl CodingAgent {
    pub fn new(agent: Agent) -> Self {
        Self { agent }
    }

    /// Create a CodingAgent for the default available agent
    pub fn default() -> Self {
        let agent = super::default_agent()
            .unwrap_or_else(|| super::get_agent("claude").unwrap());
        Self::new(agent)
    }
}

impl AgentOperations for CodingAgent {
    fn generate_text(&self, working_dir: &Path, prompt: &str) -> Result<String> {
        // Build the command based on agent type
        let (cmd, args) = match self.agent.name.as_str() {
            "claude" => ("claude", vec!["--print", prompt]),
            "aider" => ("aider", vec!["--message", prompt, "--no-auto-commits"]),
            "codex" => ("codex", vec!["--print", prompt]),
            "gh-copilot" => ("gh", vec!["copilot", "explain", prompt]),
            "opencode" => ("opencode", vec!["--print", prompt]),
            "cline" => ("cline", vec!["--print", prompt]),
            "q" => ("q", vec!["chat", "--no-interactive", prompt]),
            _ => (self.agent.command.as_str(), vec![prompt]),
        };

        let output = std::process::Command::new(cmd)
            .current_dir(working_dir)
            .args(&args)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("{} command failed: {}", self.agent.name, stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn co_author_string(&self) -> &str {
        &self.agent.co_author
    }
}
