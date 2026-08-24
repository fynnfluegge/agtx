//! One declarative record per coding agent.
//!
//! Everything agtx knows about an agent that is *data* lives here; everything
//! that is *behaviour* is selected by a small closed enum stored in the record
//! rather than by re-matching the agent's name in each function. See
//! `docs/planning/agent-spec-table.md` for the full rationale — the short
//! version is that a name-match ending in `_ => {}` cannot tell you an agent was
//! only half added, and a kind-match over a closed enum can.
//!
//! The record is deliberately pure data (no closures, no `String`) so that it
//! can later be deserialized from a `[[agents]]` block in `config.toml`, which
//! is what makes user-defined agents possible.
//!
//! Adding an agent is one entry in [`AGENT_SPECS`]. If a field cannot express
//! it, the right move is one new enum variant plus one new match arm in the one
//! function that reads that enum — not a new match on the agent's name.

/// How a non-empty prompt is appended to the agent's launch command.
///
/// This is the *shape of the CLI*, not a statement about whether agtx trusts it
/// — see [`AgentSpec::launch_prompt_verified`] for that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptForm {
    /// Positional argument: `claude '<text>'`.
    Argv,
    /// Prompt flag: `gemini -i '<text>'`.
    Flag(&'static str),
}

/// How the resume flags combine with [`AgentSpec::base_args`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeArgs {
    /// Appended after the launch flags: `claude --dangerously-skip-permissions --continue`.
    Append(&'static [&'static str]),
    /// Replaces the launch flags entirely: `codex resume --last` takes no `--sandbox`.
    Replace(&'static [&'static str]),
}

/// How skill files are laid out in the agent's native discovery directory, and
/// therefore also how existing skills are found when scanning a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillLayout {
    /// `{base}/{namespace}/{short-name}.md` — Claude, Copilot.
    CommandFile,
    /// `{base}/{namespace}/{short-name}.toml` — Gemini's TOML command format.
    GeminiToml,
    /// `{base}/{full-name}/SKILL.md` — Codex, Cursor, Grok, Antigravity.
    SkillDir,
    /// `{base}/{full-name}.md`, flat, no namespace subdir — OpenCode.
    OpenCodeFlat,
}

/// Slash-command syntax the agent's TUI accepts for an invocable skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSyntax {
    /// `/ns:command` — the canonical form, unchanged.
    Colon,
    /// `/ns-command` — colon becomes a hyphen, the slash is kept.
    Hyphen,
    /// `$ns-command` — Codex's inline skill reference.
    Dollar,
    /// No interactive skill invocation; callers fall back to a file-path
    /// reference. Copilot, and any agent agtx has not been taught.
    None,
}

/// When an agent's dialog appears, which decides how it is matched.
///
/// The distinction is forced by what each call site knows. `wait_for_agent_ready`
/// is handed only a tmux target, so it matches `Launch` dialogs against any pane
/// regardless of agent — a false positive is possible in principle and is the
/// subject of open question 1 in docs/planning/agent-spec-table.md. The
/// session-refresh loop *does* know which agent runs in the pane, so `Session`
/// dialogs are matched only against their own agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogScope {
    /// Shown once when the agent opens in a directory it has not seen.
    Launch,
    /// Shown mid-session, e.g. the first time the agent calls an MCP tool.
    Session,
}

/// An interactive prompt that blocks the agent until it is answered.
///
/// `answer` is sent as a key — a menu digit — followed by Enter.
#[derive(Debug, Clone, Copy)]
pub struct AgentDialog {
    /// Text to look for in the pane. Combined per [`require_all`](Self::require_all).
    pub patterns: &'static [&'static str],
    /// When false the patterns are **alternatives**: any one matching is enough,
    /// which is how a dialog whose wording varies between versions is caught.
    /// When true they are a **conjunction**: all must be present, for a prompt
    /// identified by a combination of phrases rather than one distinctive string.
    pub require_all: bool,
    pub answer: &'static str,
    pub scope: DialogScope,
}

impl AgentDialog {
    /// Whether this dialog is visible in `content`.
    pub fn matches(&self, content: &str) -> bool {
        if self.require_all {
            self.patterns.iter().all(|p| content.contains(p))
        } else {
            self.patterns.iter().any(|p| content.contains(p))
        }
    }
}

/// How a mid-session message (a phase advance into a running agent) is delivered.
///
/// The plan that introduced this sketched a fourth variant, `CombinedThenPicker`,
/// for Codex's second Enter. Bracketed paste removed the need for it: Codex's
/// `$skill` picker opens on typing, not on a paste, so there is nothing to
/// dismiss. Three variants, not four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendStrategy {
    /// Send the command, optionally wait on a `prompt_trigger`, then the prompt.
    Generic,
    /// Combine skill and prompt into one bracketed paste, then a single Enter.
    /// Ink-class TUIs need this: they lose a separately-sent prompt, and a typed
    /// newline submits the message half-written.
    Combined,
    /// OpenCode's command picker strips arguments when the whole string is typed
    /// at once, so the text must arrive in two pieces by design.
    OpenCodePicker,
}

/// Where and how an agent's project-scoped MCP server config is written.
///
/// Seven variants for seven agents, which looks untidy and is: the formats
/// genuinely differ (JSON vs TOML, `mcpServers` vs `mcp_servers` vs `mcp`). What
/// the enum buys is that the mess lives in one function instead of a 170-line
/// match inside `write_skills_to_worktree`, and that the names now say *why* the
/// arms differ.
///
/// The `…Merge` variants must not overwrite: their file is vendor-neutral or
/// otherwise likely to be tracked in the repo already, so clobbering it destroys
/// the user's own settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigKind {
    /// `.mcp.json`. Also writes `.claude/settings.local.json`
    /// (`enableAllProjectMcpServers`, the bypass preflight, and agtx's hooks).
    ClaudeJson,
    /// `.codex/config.toml`. Also appends a trust entry for the worktree to the
    /// user's global `~/.codex/config.toml`, without which Codex prompts on open.
    CodexToml,
    /// `.gemini/settings.json`, which additionally needs `trust: true`.
    GeminiJson,
    /// `.cursor/mcp.json`.
    CursorJson,
    /// `.grok/config.toml` — appended if the agtx table is absent.
    GrokTomlMerge,
    /// `.agents/mcp_config.json` — parsed, agtx inserted, written back, so other
    /// servers and sibling top-level keys survive.
    AntigravityJsonMerge,
    /// `opencode.json`, whose key is `mcp` and whose entry shape differs.
    OpenCode,
}

/// Everything agtx knows about one coding agent.
#[derive(Debug, Clone, Copy)]
pub struct AgentSpec {
    // ── identity ─────────────────────────────────────────────────────────
    pub name: &'static str,
    /// Executable name, which is not always the agent's name: `cursor` ships
    /// `agent`, `antigravity` ships `agy`.
    pub binary: &'static str,
    pub description: &'static str,
    pub co_author: &'static str,

    // ── launching ────────────────────────────────────────────────────────
    /// Environment assignments prefixed to the interactive launch command.
    /// Not applied to the headless invocation, which never touches the workspace.
    pub env: &'static [(&'static str, &'static str)],
    /// Flags that make the agent run unattended (permission bypass, sandbox mode).
    pub base_args: &'static [&'static str],
    pub prompt_form: PromptForm,
    /// Whether [`prompt_form`](Self::prompt_form) has been checked against the
    /// real binary and may be used to hand the agent its opening message at
    /// launch.
    ///
    /// Deliberately conservative. Getting this wrong swallows the task text
    /// silently rather than failing loudly: OpenCode's `-p`, like Copilot's used
    /// to be, is print mode — it answers once and exits, killing the window.
    /// Flip this only after verifying against the installed CLI.
    pub launch_prompt_verified: bool,
    pub resume: ResumeArgs,
    /// Flags for the one-shot print invocation used to generate PR descriptions.
    pub headless_args: &'static [&'static str],

    // ── skills & commands ────────────────────────────────────────────────
    /// `(base_dir, namespace_subdir)` relative to the worktree, or `None` for an
    /// agent with no native skill discovery. The namespace is empty for layouts
    /// that carry the prefix in the file or directory name instead.
    pub skill_dir: Option<(&'static str, &'static str)>,
    pub skill_layout: SkillLayout,
    /// Directory scanned for the agent's *pre-existing* skills. Normally the
    /// same as `skill_dir`'s base; OpenCode is the exception.
    pub skill_scan_dir: Option<&'static str>,
    pub command_syntax: CommandSyntax,

    // ── integration ──────────────────────────────────────────────────────
    /// How this agent's project-scoped MCP config is written, or `None` for an
    /// agent agtx does not wire up to the MCP server.
    pub mcp_config: Option<McpConfigKind>,

    // ── process / liveness ───────────────────────────────────────────────
    /// Process names this agent may appear as in `pane_current_command`.
    ///
    /// Node/Ink agents often report `node` or `bash` instead of their own name,
    /// which is what `active_indicators` is for.
    pub process_names: &'static [&'static str],
    /// Strings in pane content that mean this agent's TUI is up and ready.
    ///
    /// Needed for agents that run inside bash/node and so never show their own
    /// name in `pane_current_command`.
    pub active_indicators: &'static [&'static str],
    /// Command that makes the agent exit cleanly, or `None` when Ctrl+C is the
    /// only way out.
    pub exit_command: Option<&'static str>,

    // ── display ──────────────────────────────────────────────────────────
    /// Foreground colour of the agent label on a task card.
    pub label_fg: (u8, u8, u8),
    /// Background colour, for the agents whose branding is a filled chip.
    pub label_bg: Option<(u8, u8, u8)>,

    // ── sending ──────────────────────────────────────────────────────────
    pub send_strategy: SendStrategy,
    /// Command that clears the agent's context, for `clear_context_on_advance`.
    /// `None` where no such command is known — the phase advance then just sends
    /// normally. Tracked in issue #46.
    pub clear_context_command: Option<&'static str>,

    // ── dialogs ──────────────────────────────────────────────────────────
    /// Interactive prompts this agent shows that agtx answers on the user's
    /// behalf. Missing one is silent and total: the task's prompt is delivered
    /// but never read, because the agent never reaches its composer.
    pub dialogs: &'static [AgentDialog],
}

/// Every agent agtx ships with.
///
/// Ordered by preference — [`crate::agent::known_agents`] and the agent pickers
/// present them in this order.
pub const AGENT_SPECS: &[AgentSpec] = &[
    AgentSpec {
        name: "claude",
        binary: "claude",
        description: "Anthropic's Claude Code CLI",
        co_author: "Claude <noreply@anthropic.com>",
        env: &[],
        base_args: &["--dangerously-skip-permissions"],
        prompt_form: PromptForm::Argv,
        // Verified against claude 2.1.241: `claude [prompt]` starts an
        // interactive session with the prompt submitted, and a leading
        // `/agtx:plan …` still expands as a slash command.
        launch_prompt_verified: true,
        resume: ResumeArgs::Append(&["--continue"]),
        headless_args: &["--print"],
        skill_dir: Some((".claude/commands", "agtx")),
        skill_layout: SkillLayout::CommandFile,
        skill_scan_dir: Some(".claude/commands"),
        command_syntax: CommandSyntax::Colon,
        mcp_config: Some(McpConfigKind::ClaudeJson),
        process_names: &["claude"],
        active_indicators: &["Claude Code"],
        exit_command: Some("/exit"),
        label_fg: (227, 148, 62), // orange
        label_bg: None,
        send_strategy: SendStrategy::Generic,
        clear_context_command: Some("/clear"),
        dialogs: &[
            // Workspace trust, shown before the bypass warning in any directory
            // Claude has not seen. Recorded per directory as
            // `projects."<dir>".hasTrustDialogAccepted` in ~/.claude.json, and
            // every task gets a new worktree — so this fires on the first launch
            // of nearly every Claude task, not just in containers.
            AgentDialog {
                patterns: &["Yes, I trust this folder"],
                require_all: false,
                answer: "1",
                scope: DialogScope::Launch,
            },
            // `--dangerously-skip-permissions` acceptance. Not covered by
            // hasTrustDialogAccepted, so copying ~/.claude.json (as the swebench
            // harness does) does not suppress it.
            AgentDialog {
                // Alternatives: the wording has varied across Claude versions.
                patterns: &["Yes, I accept", "I accept the risk"],
                require_all: false,
                answer: "2",
                scope: DialogScope::Launch,
            },
        ],
    },
    AgentSpec {
        name: "codex",
        binary: "codex",
        description: "OpenAI's Codex CLI",
        co_author: "Codex <noreply@openai.com>",
        env: &[],
        base_args: &["--sandbox", "workspace-write"],
        prompt_form: PromptForm::Argv,
        launch_prompt_verified: false,
        // `codex resume` is its own subcommand and rejects the launch flags.
        resume: ResumeArgs::Replace(&["resume", "--last"]),
        headless_args: &["exec", "--sandbox", "workspace-write"],
        skill_dir: Some((".codex/skills", "")),
        skill_layout: SkillLayout::SkillDir,
        skill_scan_dir: Some(".codex/skills"),
        command_syntax: CommandSyntax::Dollar,
        mcp_config: Some(McpConfigKind::CodexToml),
        process_names: &["codex"],
        active_indicators: &["OpenAI Codex"],
        exit_command: None,
        label_fg: (255, 255, 255), // white on black
        label_bg: Some((20, 20, 20)),
        send_strategy: SendStrategy::Combined,
        clear_context_command: None,
        dialogs: &[
            // MCP tool approval, shown mid-session the first time the agent calls
            // an agtx tool — not at launch, so it is matched only against a pane
            // known to be running codex.
            AgentDialog {
                // A conjunction: none of these three is distinctive alone.
                patterns: &["Allow the", "MCP server to run tool", "Always allow"],
                require_all: true,
                answer: "3",
                scope: DialogScope::Session,
            },
        ],
    },
    AgentSpec {
        name: "copilot",
        binary: "copilot",
        description: "GitHub Copilot CLI",
        co_author: "GitHub Copilot <noreply@github.com>",
        env: &[],
        base_args: &["--allow-all-tools"],
        // `-i` keeps the session interactive; `-p` is print mode and exits on
        // completion, which killed every copilot task until it was fixed.
        prompt_form: PromptForm::Flag("-i"),
        launch_prompt_verified: false,
        resume: ResumeArgs::Append(&["--continue"]),
        headless_args: &["-p"],
        skill_dir: Some((".github/agents", "agtx")),
        skill_layout: SkillLayout::CommandFile,
        skill_scan_dir: Some(".github/agents"),
        // Copilot has no interactive slash-command invocation, so skills reach
        // it as a file-path reference in the prompt instead. Its skills are
        // still discovered and listed by the `/` picker.
        command_syntax: CommandSyntax::None,
        // Copilot is not wired to the MCP server.
        mcp_config: None,
        process_names: &["copilot"],
        active_indicators: &[],
        exit_command: Some("/exit"),
        label_fg: (255, 255, 255), // default white
        label_bg: None,
        send_strategy: SendStrategy::Generic,
        clear_context_command: None,
        dialogs: &[],
    },
    AgentSpec {
        name: "gemini",
        binary: "gemini",
        description: "Google Gemini CLI",
        co_author: "Gemini <noreply@google.com>",
        env: &[("GEMINI_TRUST_WORKSPACE", "true")],
        base_args: &["--approval-mode", "yolo"],
        prompt_form: PromptForm::Flag("-i"),
        launch_prompt_verified: false,
        resume: ResumeArgs::Append(&["--resume"]),
        headless_args: &["-p"],
        skill_dir: Some((".gemini/commands", "agtx")),
        skill_layout: SkillLayout::GeminiToml,
        skill_scan_dir: Some(".gemini/commands"),
        command_syntax: CommandSyntax::Colon,
        mcp_config: Some(McpConfigKind::GeminiJson),
        process_names: &["gemini"],
        active_indicators: &["Type your message"],
        exit_command: Some("/quit"),
        label_fg: (234, 130, 180), // pink
        label_bg: None,
        send_strategy: SendStrategy::Combined,
        clear_context_command: None,
        dialogs: &[
            // Answering this restarts the process.
            AgentDialog {
                patterns: &["Do you trust the files in this folder?"],
                require_all: false,
                answer: "1",
                scope: DialogScope::Launch,
            },
        ],
    },
    AgentSpec {
        name: "opencode",
        binary: "opencode",
        description: "AI-powered coding assistant",
        co_author: "OpenCode <noreply@opencode.ai>",
        env: &[],
        base_args: &[],
        prompt_form: PromptForm::Flag("-p"),
        launch_prompt_verified: false,
        resume: ResumeArgs::Append(&["--continue"]),
        headless_args: &[],
        skill_dir: Some((".opencode/command", "")),
        skill_layout: SkillLayout::OpenCodeFlat,
        // Deliberately not `.opencode/command`: agtx deploys into the worktree
        // tree, but OpenCode's own project commands live under `.config/`.
        skill_scan_dir: Some(".config/opencode/command"),
        command_syntax: CommandSyntax::Hyphen,
        mcp_config: Some(McpConfigKind::OpenCode),
        process_names: &["opencode"],
        active_indicators: &["Ask anything"],
        exit_command: Some("/exit"),
        label_fg: (255, 255, 255), // white on grey
        label_bg: Some((80, 80, 80)),
        send_strategy: SendStrategy::OpenCodePicker,
        clear_context_command: None,
        dialogs: &[],
    },
    AgentSpec {
        name: "cursor",
        binary: "agent",
        description: "Cursor Agent CLI",
        co_author: "Cursor Agent <noreply@cursor.com>",
        env: &[],
        base_args: &["--yolo"],
        prompt_form: PromptForm::Argv,
        launch_prompt_verified: false,
        resume: ResumeArgs::Append(&["--continue"]),
        headless_args: &["--print", "--yolo"],
        skill_dir: Some((".cursor/skills", "")),
        skill_layout: SkillLayout::SkillDir,
        skill_scan_dir: Some(".cursor/skills"),
        command_syntax: CommandSyntax::Hyphen,
        mcp_config: Some(McpConfigKind::CursorJson),
        process_names: &["agent"],
        active_indicators: &["Cursor Agent"],
        exit_command: None,
        label_fg: (255, 255, 255), // default white
        label_bg: None,
        send_strategy: SendStrategy::Combined,
        clear_context_command: None,
        dialogs: &[],
    },
    AgentSpec {
        name: "grok",
        binary: "grok",
        description: "xAI's Grok Build CLI",
        co_author: "Grok <noreply@x.ai>",
        env: &[],
        // `--trust` also ungates the repo-local `.grok/config.toml` MCP server
        // and suppresses the directory-trust dialog.
        base_args: &["--yolo", "--trust"],
        prompt_form: PromptForm::Argv,
        launch_prompt_verified: false,
        resume: ResumeArgs::Append(&["--continue"]),
        headless_args: &["-p"],
        skill_dir: Some((".grok/skills", "")),
        skill_layout: SkillLayout::SkillDir,
        skill_scan_dir: Some(".grok/skills"),
        command_syntax: CommandSyntax::Hyphen,
        mcp_config: Some(McpConfigKind::GrokTomlMerge),
        process_names: &["grok"],
        active_indicators: &["Grok Build", "Shift+Tab:mode"],
        exit_command: Some("/quit"),
        label_fg: (20, 20, 20), // black on white
        label_bg: Some((255, 255, 255)),
        send_strategy: SendStrategy::Generic,
        clear_context_command: None,
        dialogs: &[],
    },
    AgentSpec {
        name: "antigravity",
        binary: "agy",
        description: "Google's Antigravity CLI",
        co_author: "Antigravity <noreply@google.com>",
        env: &[],
        // Two orthogonal controls: the flag governs shell/MCP/URL approvals,
        // `--mode` governs the file-edit diff review. Both are needed to run
        // unattended.
        base_args: &["--dangerously-skip-permissions", "--mode", "accept-edits"],
        prompt_form: PromptForm::Flag("-i"),
        launch_prompt_verified: false,
        resume: ResumeArgs::Append(&["--continue"]),
        headless_args: &["-p"],
        // Antigravity reads workspace skills from the vendor-neutral `.agents/`
        // tree, not from an agent-specific dotdir.
        skill_dir: Some((".agents/skills", "")),
        skill_layout: SkillLayout::SkillDir,
        skill_scan_dir: Some(".agents/skills"),
        command_syntax: CommandSyntax::Hyphen,
        mcp_config: Some(McpConfigKind::AntigravityJsonMerge),
        process_names: &["agy"],
        active_indicators: &[],
        exit_command: Some("/exit"),
        label_fg: (120, 190, 255), // light blue
        label_bg: None,
        send_strategy: SendStrategy::Combined,
        clear_context_command: None,
        dialogs: &[],
    },
    // TODO: investigate CLI usage before enabling
    // aider  — "AI pair programming in your terminal", Aider <noreply@aider.chat>
    // cline  — "AI coding assistant for VS Code",      Cline <noreply@cline.bot>
];

/// Look up an agent's spec by name. `None` for agents agtx has not been taught,
/// which fall back to generic behaviour throughout.
pub fn spec(name: &str) -> Option<&'static AgentSpec> {
    AGENT_SPECS.iter().find(|s| s.name == name)
}

/// Build a shell command for `spec`: env prefix, binary, the given args, then
/// the prompt in the agent's own form.
///
/// The prompt is single-quoted with the POSIX `'"'"'` idiom because the result
/// is wrapped in `sh -c '…'` by `create_window`; a bare quote would close that
/// outer quote and let the shell mangle the text.
/// Largest prompt agtx will hand to a process in argv.
///
/// `ARG_MAX` is ~1 MB on macOS and Linux and task descriptions are far below
/// that, but the limit should be explicit rather than incidental: past this the
/// caller falls back to the mid-session lane, which has no such ceiling. Counted
/// in bytes, not chars, because that is what `execve` counts.
pub const MAX_LAUNCH_PROMPT_BYTES: usize = 128 * 1024;

/// Strip characters that cannot survive the trip through argv intact.
///
/// The command string is handed to tmux, which runs it through a shell, so the
/// prompt is embedded in a single-quoted word. Single quotes are escaped by
/// [`compose_command`]; this handles the rest:
///
/// - **NUL** would truncate the argument at the C-string boundary, silently
///   discarding the rest of the task.
/// - **`\r`** renders as a carriage return, moving the agent's cursor to column
///   zero and corrupting the echoed prompt. It is almost always a paste artifact
///   from a CRLF source.
/// - **Other control characters** (ESC in particular) let pasted text move the
///   cursor or set colours in the agent's TUI.
///
/// `\n` and `\t` are kept: they are ordinary structure in a task description,
/// and inside a single-quoted argv word they are just bytes.
pub fn normalize_prompt(prompt: &str) -> String {
    prompt
        .chars()
        .filter(|c| *c == '\n' || *c == '\t' || !c.is_control())
        .collect()
}

/// Whether `text` can be delivered as a launch argument for this injection mode.
///
/// False sends the caller down the mid-session lane instead — either because the
/// agent's launch form is unverified, or because the text is too large for argv.
pub fn can_launch_with_prompt(injection: crate::agent::PromptInjection, text: &str) -> bool {
    !text.is_empty()
        && !matches!(injection, crate::agent::PromptInjection::Unknown)
        && text.len() <= MAX_LAUNCH_PROMPT_BYTES
}

pub fn compose_command(spec: &AgentSpec, args: &[&str], prompt: Option<&str>) -> String {
    let mut out = String::new();
    for (key, value) in spec.env {
        out.push_str(key);
        out.push('=');
        out.push_str(value);
        out.push(' ');
    }
    out.push_str(spec.binary);
    for arg in args {
        out.push(' ');
        out.push_str(arg);
    }
    if let Some(prompt) = prompt.filter(|p| !p.is_empty()) {
        if let PromptForm::Flag(flag) = spec.prompt_form {
            out.push(' ');
            out.push_str(flag);
        }
        // POSIX single-quote escaping: close, emit a literal quote, reopen. The
        // whole command is itself wrapped in `sh -c '…'` by create_window, so a
        // bare quote here would end that outer word and let the shell interpret
        // the rest of the task as code.
        let prompt = normalize_prompt(prompt);
        out.push_str(&format!(" '{}'", prompt.replace('\'', "'\"'\"'")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_agent_has_exactly_one_spec() {
        let names: Vec<&str> = AGENT_SPECS.iter().map(|s| s.name).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate agent name in table");

        let known: Vec<String> = crate::agent::known_agents()
            .into_iter()
            .map(|a| a.name)
            .collect();
        assert_eq!(
            known, names,
            "known_agents() must be derived from the table"
        );
    }

    #[test]
    fn spec_lookup_is_exhaustive_and_exclusive() {
        for entry in AGENT_SPECS {
            assert_eq!(spec(entry.name).map(|s| s.name), Some(entry.name));
        }
        assert!(spec("mistral").is_none());
        assert!(spec("").is_none());
    }

    #[test]
    fn skill_dirs_stay_inside_the_worktree() {
        for entry in AGENT_SPECS {
            for dir in entry
                .skill_dir
                .map(|(b, _)| b)
                .into_iter()
                .chain(entry.skill_scan_dir)
            {
                assert!(
                    !dir.starts_with('/'),
                    "{}: {dir} must be relative",
                    entry.name
                );
                assert!(
                    !dir.contains(".."),
                    "{}: {dir} must stay in-worktree",
                    entry.name
                );
            }
        }
    }

    #[test]
    fn a_verified_launch_prompt_is_never_a_print_flag() {
        // `-p` is print mode for every agent that has it: it answers once and
        // exits, killing the task window. Marking such an agent verified is the
        // bug this guard exists for.
        for entry in AGENT_SPECS.iter().filter(|s| s.launch_prompt_verified) {
            assert_ne!(
                entry.prompt_form,
                PromptForm::Flag("-p"),
                "{} cannot deliver its prompt at launch through print mode",
                entry.name
            );
        }
    }
}
