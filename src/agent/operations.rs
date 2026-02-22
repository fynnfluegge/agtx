//! Traits for agent operations to enable testing with mocks.
//!
//! This module provides a generic interface for interacting with coding agents
//! like Claude Code, Aider, Codex, etc.

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

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

    /// Build the shell command to start the agent interactively with a prompt
    fn build_interactive_command(&self, prompt: &str) -> String;
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

    fn build_interactive_command(&self, prompt: &str) -> String {
        self.agent.build_interactive_command(prompt)
    }
}

/// Registry that maps agent names to AgentOperations instances.
/// Enables per-stage agent selection (e.g., different agents for planning, running, review).
#[cfg_attr(feature = "test-mocks", automock)]
pub trait AgentRegistry: Send + Sync {
    /// Get the AgentOperations instance for a given agent name.
    /// Falls back to the default agent if the name is unknown or unavailable.
    fn get(&self, agent_name: &str) -> Arc<dyn AgentOperations>;
}

/// Production implementation of AgentRegistry.
/// Holds all available agents, keyed by name.
pub struct RealAgentRegistry {
    agents: HashMap<String, Arc<dyn AgentOperations>>,
    default_name: String,
}

impl RealAgentRegistry {
    /// Create a new registry populated with all available agents.
    /// `default_name` is used as the fallback when a requested name isn't found.
    pub fn new(default_name: &str) -> Self {
        let mut agents: HashMap<String, Arc<dyn AgentOperations>> = HashMap::new();

        for agent in super::known_agents() {
            if agent.is_available() {
                let name = agent.name.clone();
                agents.insert(name, Arc::new(CodingAgent::new(agent)));
            }
        }

        // Ensure we have the default agent even if not detected as available
        if !agents.contains_key(default_name) {
            if let Some(agent) = super::get_agent(default_name) {
                agents.insert(default_name.to_string(), Arc::new(CodingAgent::new(agent)));
            }
        }

        Self {
            agents,
            default_name: default_name.to_string(),
        }
    }
}

impl AgentRegistry for RealAgentRegistry {
    fn get(&self, agent_name: &str) -> Arc<dyn AgentOperations> {
        self.agents
            .get(agent_name)
            .cloned()
            .unwrap_or_else(|| {
                self.agents
                    .get(&self.default_name)
                    .cloned()
                    .expect("Default agent must exist in registry")
            })
    }
}
