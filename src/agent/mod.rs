mod operations;

pub use operations::{AgentOperations, AgentRegistry, CodingAgent, RealAgentRegistry};

#[cfg(feature = "test-mocks")]
pub use operations::{MockAgentOperations, MockAgentRegistry};

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

    /// Build the shell command to resume the agent's most recent session
    /// in the current working directory. Used to recover from tmux/server restarts.
    pub fn build_resume_command(&self) -> String {
        match self.name.as_str() {
            "claude" => "claude --dangerously-skip-permissions --continue".to_string(),
            "codex" => "codex resume --last".to_string(),
            "copilot" => "copilot --allow-all-tools --continue".to_string(),
            "gemini" => "gemini --approval-mode yolo --resume".to_string(),
            "opencode" => "opencode --continue".to_string(),
            "cursor" => "agent --yolo --continue".to_string(),
            _ => self.build_interactive_command(""),
        }
    }

    /// Build the shell command to start the agent interactively.
    /// When prompt is empty, the agent starts with no initial message
    /// (task content and skill commands are sent later via tmux send_keys).
    pub fn build_interactive_command(&self, prompt: &str) -> String {
        if prompt.is_empty() {
            return match self.name.as_str() {
                "claude" => "claude --dangerously-skip-permissions".to_string(),
                "codex" => "codex --full-auto".to_string(),
                "copilot" => "copilot --allow-all-tools".to_string(),
                "gemini" => "gemini --approval-mode yolo".to_string(),
                "opencode" => "opencode".to_string(),
                "cursor" => "agent --yolo".to_string(),
                _ => self.command.clone(),
            };
        }

        let escaped_prompt = prompt.replace('\'', "'\"'\"'");
        match self.name.as_str() {
            "claude" => format!("claude --dangerously-skip-permissions '{}'", escaped_prompt),
            "codex" => format!("codex --full-auto '{}'", escaped_prompt),
            "copilot" => format!("copilot --allow-all-tools -p '{}'", escaped_prompt),
            "gemini" => format!("gemini --approval-mode yolo -i '{}'", escaped_prompt),
            "opencode" => format!("opencode -p '{}'", escaped_prompt),
            "cursor" => format!("agent --yolo '{}'", escaped_prompt),
            _ => format!("{} '{}'", self.command, escaped_prompt),
        }
    }
}

/// Get the list of known agents
pub fn known_agents() -> Vec<Agent> {
    vec![
        Agent::new(
            "claude",
            "claude",
            "Anthropic's Claude Code CLI",
            "Claude <noreply@anthropic.com>",
        ),
        Agent::new(
            "codex",
            "codex",
            "OpenAI's Codex CLI",
            "Codex <noreply@openai.com>",
        ),
        Agent::new(
            "copilot",
            "copilot",
            "GitHub Copilot CLI",
            "GitHub Copilot <noreply@github.com>",
        ),
        Agent::new(
            "gemini",
            "gemini",
            "Google Gemini CLI",
            "Gemini <noreply@google.com>",
        ),
        Agent::new(
            "opencode",
            "opencode",
            "AI-powered coding assistant",
            "OpenCode <noreply@opencode.ai>",
        ),
        Agent::new(
            "cursor",
            "agent",
            "Cursor Agent CLI",
            "Cursor Agent <noreply@cursor.com>",
        ),
        // TODO: investigate CLI usage before enabling
        // Agent::new("aider", "aider", "AI pair programming in your terminal", "Aider <noreply@aider.chat>"),
        // Agent::new("cline", "cline", "AI coding assistant for VS Code", "Cline <noreply@cline.bot>"),
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

/// Parse user input for agent selection.
/// Returns the index (0-based) of the selected agent, or None for invalid input.
/// Empty input returns Some(0) (first agent as default).
pub fn parse_agent_selection(input: &str, agent_count: usize) -> Option<usize> {
    let input = input.trim();
    if input.is_empty() {
        return Some(0);
    }
    if let Ok(num) = input.parse::<usize>() {
        if num >= 1 && num <= agent_count {
            return Some(num - 1);
        }
    }
    None
}

