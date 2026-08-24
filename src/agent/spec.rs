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
