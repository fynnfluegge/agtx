mod operations;

pub use operations::{AgentOperations, CodingAgent};

#[cfg(feature = "test-mocks")]
pub use operations::MockAgentOperations;

use serde::{Deserialize, Serialize};

/// Known coding agents that agtx can work with
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub description: String,
    pub co_author: String,
}

impl Agent {
    pub fn new(name: &str, command: &str, description: &str, co_author: &str) -> Self {
        Self {
            name: name.to_string(),
            command: command.to_string(),
            args: vec![],
            description: description.to_string(),
            co_author: co_author.to_string(),
        }
    }

    /// Check if this agent is installed on the system
    pub fn is_available(&self) -> bool {
        which::which(&self.command).is_ok()
    }

    /// Build the shell command to start the agent interactively with a prompt
    pub fn build_interactive_command(&self, prompt: &str) -> String {
        // Escape single quotes in prompt for shell
        let escaped_prompt = prompt.replace('\'', "'\"'\"'");

        match self.name.as_str() {
            "claude" => {
                format!("claude --dangerously-skip-permissions '{}'", escaped_prompt)
            }
            "aider" => {
                format!("aider --message '{}'", escaped_prompt)
            }
            "codex" => {
                format!("codex '{}'", escaped_prompt)
            }
            "gh-copilot" => {
                format!("gh copilot suggest '{}'", escaped_prompt)
            }
            "opencode" => {
                format!("opencode '{}'", escaped_prompt)
            }
            "cline" => {
                format!("cline '{}'", escaped_prompt)
            }
            "q" => {
                format!("q chat '{}'", escaped_prompt)
            }
            _ => {
                format!("{} '{}'", self.command, escaped_prompt)
            }
        }
    }
}

/// Get the list of known agents
pub fn known_agents() -> Vec<Agent> {
    vec![
        Agent::new("claude", "claude", "Anthropic's Claude Code CLI", "Claude <noreply@anthropic.com>"),
        Agent::new("aider", "aider", "AI pair programming in your terminal", "Aider <noreply@aider.chat>"),
        Agent::new("codex", "codex", "OpenAI's Codex CLI", "Codex <noreply@openai.com>"),
        Agent::new("gh-copilot", "gh", "GitHub Copilot CLI", "GitHub Copilot <noreply@github.com>"),
        Agent::new("opencode", "opencode", "AI-powered coding assistant", "OpenCode <noreply@opencode.ai>"),
        Agent::new("cline", "cline", "AI coding assistant for VS Code", "Cline <noreply@cline.bot>"),
        Agent::new("q", "q", "Amazon Q Developer CLI", "Amazon Q <noreply@amazon.com>"),
    ]
}

/// Detect which agents are available on the system
pub fn detect_available_agents() -> Vec<Agent> {
    known_agents()
        .into_iter()
        .filter(|a| a.is_available())
        .collect()
}

/// Get a specific agent by name
pub fn get_agent(name: &str) -> Option<Agent> {
    known_agents().into_iter().find(|a| a.name == name)
}

/// Find the default agent (first available from preference order)
pub fn default_agent() -> Option<Agent> {
    let preference_order = ["claude", "aider", "codex", "gh-copilot", "opencode", "cline", "q"];

    for name in preference_order {
        if let Some(agent) = get_agent(name) {
            if agent.is_available() {
                return Some(agent);
            }
        }
    }

    None
}

/// Agent availability status for display
#[derive(Debug)]
pub struct AgentStatus {
    pub agent: Agent,
    pub available: bool,
}

/// Get status of all known agents
pub fn all_agent_status() -> Vec<AgentStatus> {
    known_agents()
        .into_iter()
        .map(|agent| {
            let available = agent.is_available();
            AgentStatus { agent, available }
        })
        .collect()
}

/// Build the command arguments for spawning an agent
pub fn build_spawn_args(agent: &Agent, prompt: &str, task_id: &str) -> Vec<String> {
    let mut args = agent.args.clone();

    match agent.name.as_str() {
        "claude" => {
            // Claude Code supports session persistence
            args.extend(["--session".to_string(), task_id.to_string()]);
            args.push(prompt.to_string());
        }
        "aider" => {
            // Aider uses --message for the initial prompt
            args.extend(["--message".to_string(), prompt.to_string()]);
        }
        "gh-copilot" => {
            // GitHub Copilot needs subcommand
            args.extend(["copilot".to_string(), "suggest".to_string()]);
            args.push(prompt.to_string());
        }
        _ => {
            // Default: just pass the prompt
            args.push(prompt.to_string());
        }
    }

    args
}
