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

    /// Build the shell command to start the agent interactively.
    /// When prompt is empty, the agent starts with no initial message.
    fn build_interactive_command(&self, prompt: &str) -> String;

    /// Build the shell command to resume the agent's most recent session
    /// in the current working directory. Used to recover from tmux/server restarts.
    fn build_resume_command(&self) -> String;

    /// Build the full shell command to run this agent as an orchestrator.
    /// Includes MCP registration (if supported by the agent) and cleanup on exit.
    /// Default implementation: no MCP, just launches the agent interactively.
    fn build_orchestrator_command(&self, mcp_json: &str, agtx_bin: &str) -> String {
        let _ = (mcp_json, agtx_bin);
        self.build_interactive_command("")
    }
}

/// Generic agent implementation that works with any Agent config
pub struct CodingAgent {
    agent: Agent,
}

impl CodingAgent {
    pub fn new(agent: Agent) -> Self {
        Self { agent }
    }
}

impl AgentOperations for CodingAgent {
    fn generate_text(&self, working_dir: &Path, prompt: &str) -> Result<String> {
        // Build the command based on agent type
        let (cmd, args) = match self.agent.name.as_str() {
            "claude" => ("claude", vec!["--print", prompt]),
            "codex" => (
                "codex",
                vec!["exec", "--dangerously-bypass-approvals-and-sandbox", prompt],
            ),
            "copilot" => ("copilot", vec!["-p", prompt]),
            "gemini" => ("gemini", vec!["-p", prompt]),
            "cursor" => ("agent", vec!["--print", "--yolo", prompt]),
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

    fn build_resume_command(&self) -> String {
        self.agent.build_resume_command()
    }

    fn build_orchestrator_command(&self, mcp_json: &str, _agtx_bin: &str) -> String {
        match self.agent.name.as_str() {
            // Pre-remove any stale `agtx` registration (last run crashed before
            // its own `mcp remove`) so `add-json` doesn't fail with "already
            // exists" and short-circuit the `&&` into an empty shell.
            "claude" => format!(
                "claude mcp remove agtx --scope local 2>/dev/null || true; \
                 claude mcp add-json agtx '{}' --scope local && {}; \
                 claude mcp remove agtx --scope local",
                mcp_json,
                self.build_interactive_command("")
            ),
            "codex" => format!(
                "codex mcp remove agtx --scope local 2>/dev/null || true; \
                 codex mcp add-json agtx '{}' --scope local && {}; \
                 codex mcp remove agtx --scope local",
                mcp_json,
                self.build_interactive_command("")
            ),
            "copilot" => format!(
                "copilot mcp remove agtx --scope local 2>/dev/null || true; \
                 copilot mcp add-json agtx '{}' --scope local && {}; \
                 copilot mcp remove agtx --scope local",
                mcp_json,
                self.build_interactive_command("")
            ),
            "gemini" => format!(
                "gemini mcp remove agtx --scope local 2>/dev/null || true; \
                 gemini mcp add-json agtx '{}' --scope local && {}; \
                 gemini mcp remove agtx --scope local",
                mcp_json,
                self.build_interactive_command("")
            ),
            "opencode" => format!(
                "opencode mcp remove agtx --scope local 2>/dev/null || true; \
                 opencode mcp add-json agtx '{}' --scope local && {}; \
                 opencode mcp remove agtx --scope local",
                mcp_json,
                self.build_interactive_command("")
            ),
            "cursor" => format!(
                "agent mcp remove agtx --scope local 2>/dev/null || true; \
                 agent mcp add-json agtx '{}' --scope local && {}; \
                 agent mcp remove agtx --scope local",
                mcp_json,
                self.build_interactive_command("")
            ),
            _ => self.build_interactive_command(""),
        }
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
        self.agents.get(agent_name).cloned().unwrap_or_else(|| {
            self.agents
                .get(&self.default_name)
                .cloned()
                .expect("Default agent must exist in registry")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;

    #[test]
    fn test_build_orchestrator_command_codex_includes_mcp_lifecycle() {
        let agent = Agent::new(
            "codex",
            "codex",
            "OpenAI's Codex CLI",
            "Codex <noreply@openai.com>",
        );
        let ops = CodingAgent::new(agent);
        let cmd = ops.build_orchestrator_command("{\\\"type\\\":\\\"stdio\\\"}", "agtx");

        assert!(cmd.contains("mcp"));
        assert!(cmd.contains("remove") && cmd.contains("agtx"));
        assert!(cmd.contains("add") || cmd.contains("add-json"));
        assert!(cmd.contains("codex --dangerously-bypass-approvals-and-sandbox"));
    }

    #[test]
    fn test_build_orchestrator_command_gemini_includes_mcp_lifecycle() {
        let agent = Agent::new(
            "gemini",
            "gemini",
            "Google Gemini CLI",
            "Gemini <noreply@google.com>",
        );
        let ops = CodingAgent::new(agent);
        let cmd = ops.build_orchestrator_command("{\\\"type\\\":\\\"stdio\\\"}", "agtx");

        assert!(cmd.contains("mcp"));
        assert!(cmd.contains("remove") && cmd.contains("agtx"));
        assert!(cmd.contains("add") || cmd.contains("add-json"));
        assert!(cmd.contains("gemini --approval-mode yolo"));
    }

    #[test]
    fn test_build_orchestrator_command_opencode_includes_mcp_lifecycle() {
        let agent = Agent::new(
            "opencode",
            "opencode",
            "AI-powered coding assistant",
            "OpenCode <noreply@opencode.ai>",
        );
        let ops = CodingAgent::new(agent);
        let cmd = ops.build_orchestrator_command("{\\\"type\\\":\\\"stdio\\\"}", "agtx");

        assert!(cmd.contains("mcp"));
        assert!(cmd.contains("remove") && cmd.contains("agtx"));
        assert!(cmd.contains("add") || cmd.contains("add-json"));
        assert!(cmd.contains("opencode"));
    }

    #[test]
    fn test_build_orchestrator_command_cursor_includes_mcp_lifecycle() {
        let agent = Agent::new(
            "cursor",
            "agent",
            "Cursor Agent CLI",
            "Cursor Agent <noreply@cursor.com>",
        );
        let ops = CodingAgent::new(agent);
        let cmd = ops.build_orchestrator_command("{\\\"type\\\":\\\"stdio\\\"}", "agtx");

        assert!(cmd.contains("mcp"));
        assert!(cmd.contains("remove") && cmd.contains("agtx"));
        assert!(cmd.contains("add") || cmd.contains("add-json"));
        assert!(cmd.contains("agent --yolo"));
    }

    #[test]
    fn test_build_orchestrator_command_copilot_includes_mcp_lifecycle() {
        let agent = Agent::new(
            "copilot",
            "copilot",
            "GitHub Copilot CLI",
            "GitHub Copilot <noreply@github.com>",
        );
        let ops = CodingAgent::new(agent);
        let cmd = ops.build_orchestrator_command("{\\\"type\\\":\\\"stdio\\\"}", "agtx");

        assert!(cmd.contains("mcp"));
        assert!(cmd.contains("remove") && cmd.contains("agtx"));
        assert!(cmd.contains("add") || cmd.contains("add-json"));
        assert!(cmd.contains("copilot --allow-all-tools"));
    }

    #[test]
    fn test_build_orchestrator_command_unknown_agent_falls_back_to_interactive() {
        let agent = Agent::new("custom", "custom-agent", "Custom", "Custom <noreply@example.com>");
        let ops = CodingAgent::new(agent);
        let cmd = ops.build_orchestrator_command("{}", "agtx");

        assert_eq!(cmd, "custom-agent");
    }
}
