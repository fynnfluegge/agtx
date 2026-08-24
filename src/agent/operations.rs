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

    /// Whether this agent takes the opening message at launch; see
    /// [`crate::agent::PromptInjection`].
    fn prompt_injection(&self) -> crate::agent::PromptInjection {
        crate::agent::PromptInjection::Unknown
    }

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
            "codex" => ("codex", vec!["exec", "--sandbox", "workspace-write", prompt]),
            "copilot" => ("copilot", vec!["-p", prompt]),
            "gemini" => ("gemini", vec!["-p", prompt]),
            "cursor" => ("agent", vec!["--print", "--yolo", prompt]),
            "grok" => ("grok", vec!["-p", prompt]),
            "antigravity" => ("agy", vec!["-p", prompt]),
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

    fn prompt_injection(&self) -> crate::agent::PromptInjection {
        self.agent.prompt_injection()
    }

    fn build_orchestrator_command(&self, mcp_json: &str, _agtx_bin: &str) -> String {
        match self.agent.name.as_str() {
            // Register under a unique name (`agtx-orchestrator`) rather than
            // `agtx` to avoid colliding with an `agtx` server already defined in
            // another scope (the Claude Code plugin's `plugin:agtx:agtx`, a
            // user-scope entry in ~/.claude.json, or the repo's .mcp.json).
            // A same-named server across scopes with different endpoints makes
            // Claude Code report a configuration conflict and refuse to connect.
            //
            // Pre-remove any stale registration (last run crashed before its own
            // `mcp remove`) so `add-json` doesn't fail with "already exists" and
            // short-circuit the `&&` into an empty shell.
            //
            // The JSON must be single-quoted for `add-json`, but this whole
            // command is itself wrapped in `sh -c '...'` by create_window. A bare
            // `'{json}'` would close that outer quote and let the shell mangle the
            // JSON (yielding "Invalid configuration: Invalid input"). Escape the
            // wrapping quotes as `'\''` (the POSIX single-quote-in-single-quote
            // idiom, same convention build_interactive_command uses for prompts)
            // so they survive the outer `sh -c` layer intact.
            "claude" => format!(
                "claude mcp remove agtx-orchestrator --scope local 2>/dev/null || true; \
                 claude mcp add-json agtx-orchestrator '\\''{}'\\'' --scope local && {}; \
                 claude mcp remove agtx-orchestrator --scope local",
                mcp_json,
                self.build_interactive_command("")
            ),
            // To add a new orchestrator agent, add a match arm here.
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
