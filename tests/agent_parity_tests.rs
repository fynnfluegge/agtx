//! Byte-for-byte lock on every pure per-agent function, for all supported agents.
//!
//! Written against the behaviour that existed *before* the `AgentSpec` table,
//! and must not change value while that refactor proceeds: a diff in this file
//! is the signal that a launch string, skill path or command syntax silently
//! changed, which is exactly the failure mode this refactor risks — an agent
//! that still compiles and still starts, but misbehaves inside a worktree.
//!
//! Expected values are written as literals on purpose. Deriving them from the
//! same table the code reads would assert nothing.

use agtx::agent::{get_agent, known_agents, Agent, PromptInjection};
use agtx::skills::{agent_native_skill_dir, skill_dir_to_filename, transform_plugin_command};

/// Every agent agtx ships with. Parity is asserted for all of them, and the
/// list itself is asserted against `known_agents()` so adding an agent without
/// extending this file fails loudly.
const AGENTS: &[&str] = &[
    "claude",
    "codex",
    "copilot",
    "gemini",
    "opencode",
    "cursor",
    "grok",
    "antigravity",
];

fn agent(name: &str) -> Agent {
    get_agent(name).unwrap_or_else(|| panic!("{name} missing from known_agents()"))
}

/// An agent agtx has never heard of, exercising the generic fallback arms.
fn unknown() -> Agent {
    Agent::new(
        "mistral",
        "mistral-vibe",
        "Not a known agent",
        "Mistral <noreply@mistral.ai>",
    )
}

#[test]
fn parity_covers_every_known_agent() {
    let known: Vec<String> = known_agents().into_iter().map(|a| a.name).collect();
    let covered: Vec<String> = AGENTS.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        known, covered,
        "known_agents() changed — extend tests/agent_parity_tests.rs with the new agent \
         before migrating it"
    );
}

// =============================================================================
// build_interactive_command
// =============================================================================

#[test]
fn interactive_command_parity_without_prompt() {
    let expected: &[(&str, &str)] = &[
        ("claude", "claude --dangerously-skip-permissions"),
        ("codex", "codex --sandbox workspace-write"),
        ("copilot", "copilot --allow-all-tools"),
        (
            "gemini",
            "GEMINI_TRUST_WORKSPACE=true gemini --approval-mode yolo",
        ),
        ("opencode", "opencode"),
        ("cursor", "agent --yolo"),
        ("grok", "grok --yolo --trust"),
        (
            "antigravity",
            "agy --dangerously-skip-permissions --mode accept-edits",
        ),
    ];
    for (name, want) in expected {
        assert_eq!(&agent(name).build_interactive_command(""), want, "{name}");
    }
    assert_eq!(unknown().build_interactive_command(""), "mistral-vibe");
}

#[test]
fn interactive_command_parity_with_prompt() {
    let expected: &[(&str, &str)] = &[
        ("claude", "claude --dangerously-skip-permissions 'hi'"),
        ("codex", "codex --sandbox workspace-write 'hi'"),
        // `-i` keeps the session interactive; `-p` is print mode and exits,
        // which killed every copilot task until it was fixed. Do not "simplify"
        // this back to `-p`.
        ("copilot", "copilot --allow-all-tools -i 'hi'"),
        (
            "gemini",
            "GEMINI_TRUST_WORKSPACE=true gemini --approval-mode yolo -i 'hi'",
        ),
        ("opencode", "opencode -p 'hi'"),
        ("cursor", "agent --yolo 'hi'"),
        ("grok", "grok --yolo --trust 'hi'"),
        (
            "antigravity",
            "agy --dangerously-skip-permissions --mode accept-edits -i 'hi'",
        ),
    ];
    for (name, want) in expected {
        assert_eq!(&agent(name).build_interactive_command("hi"), want, "{name}");
    }
    assert_eq!(
        unknown().build_interactive_command("hi"),
        "mistral-vibe 'hi'"
    );
}

#[test]
fn interactive_command_escapes_single_quotes_for_every_agent() {
    // The command is wrapped in `sh -c '...'` downstream, so a prompt quote must
    // survive as the POSIX '"'"' idiom for every agent, including the fallback.
    for name in AGENTS.iter().copied() {
        let cmd = agent(name).build_interactive_command("it's");
        assert!(
            cmd.ends_with(r#"'it'"'"'s'"#),
            "{name} lost single-quote escaping: {cmd}"
        );
    }
    assert!(unknown()
        .build_interactive_command("it's")
        .ends_with(r#"'it'"'"'s'"#));
}

// =============================================================================
// build_resume_command
// =============================================================================

#[test]
fn resume_command_parity() {
    let expected: &[(&str, &str)] = &[
        ("claude", "claude --dangerously-skip-permissions --continue"),
        // Codex resume *replaces* the launch flags rather than appending to
        // them: there is no `--sandbox` here.
        ("codex", "codex resume --last"),
        ("copilot", "copilot --allow-all-tools --continue"),
        (
            "gemini",
            "GEMINI_TRUST_WORKSPACE=true gemini --approval-mode yolo --resume",
        ),
        ("opencode", "opencode --continue"),
        ("cursor", "agent --yolo --continue"),
        ("grok", "grok --yolo --trust --continue"),
        (
            "antigravity",
            "agy --dangerously-skip-permissions --mode accept-edits --continue",
        ),
    ];
    for (name, want) in expected {
        assert_eq!(&agent(name).build_resume_command(), want, "{name}");
    }
    // Unknown agents have no resume form and fall back to a bare launch.
    assert_eq!(unknown().build_resume_command(), "mistral-vibe");
}

// =============================================================================
// headless (print) invocation — generate_text
// =============================================================================

#[test]
fn headless_invocation_parity() {
    let expected: &[(&str, &str, &[&str])] = &[
        ("claude", "claude", &["--print"]),
        ("codex", "codex", &["exec", "--sandbox", "workspace-write"]),
        ("copilot", "copilot", &["-p"]),
        ("gemini", "gemini", &["-p"]),
        ("opencode", "opencode", &[]),
        ("cursor", "agent", &["--print", "--yolo"]),
        ("grok", "grok", &["-p"]),
        ("antigravity", "agy", &["-p"]),
    ];
    for (name, bin, flags) in expected {
        let a = agent(name);
        let (cmd, args) = a.headless_invocation("PROMPT");
        assert_eq!(&cmd, bin, "{name} binary");
        let mut want: Vec<&str> = flags.to_vec();
        want.push("PROMPT");
        assert_eq!(args, want, "{name} args");
    }
    let fallback = unknown();
    let (cmd, args) = fallback.headless_invocation("PROMPT");
    assert_eq!(cmd, "mistral-vibe");
    assert_eq!(args, vec!["PROMPT"]);
}

#[test]
fn headless_invocation_does_not_carry_launch_env() {
    // Gemini's GEMINI_TRUST_WORKSPACE belongs to the interactive launch only;
    // headless runs never touch the workspace. Locked because the table makes
    // it tempting to apply `env` uniformly.
    let gemini = agent("gemini");
    let (cmd, _) = gemini.headless_invocation("PROMPT");
    assert_eq!(cmd, "gemini");
}

// =============================================================================
// prompt_injection — which agents take the opening message at launch
// =============================================================================

/// Which agents may take the opening message at launch.
///
/// Each `Argv`/`FlagInteractive` entry below was checked against the real binary
/// in a scratch worktree: the skill command had to expand, the skill body had to
/// run, and the session had to stay usable afterwards. Everything else keeps the
/// send-after-ready path — guessing here silently swallows the task text.
#[test]
fn prompt_injection_parity() {
    let verified: &[(&str, PromptInjection)] = &[
        // claude 2.1.241
        ("claude", PromptInjection::Argv),
        // codex-cli 0.144.5
        ("codex", PromptInjection::Argv),
        // Grok 4.6
        ("grok", PromptInjection::Argv),
    ];
    for (name, want) in verified {
        assert_eq!(agent(name).prompt_injection(), *want, "{name}");
    }

    let unverified = ["copilot", "gemini", "opencode", "cursor", "antigravity"];
    for name in unverified {
        assert_eq!(
            agent(name).prompt_injection(),
            PromptInjection::Unknown,
            "{name} must stay unverified until checked against the real binary"
        );
    }
    // antigravity is the interesting one: `agy … -i` *does* deliver the prompt,
    // but the session is then parked on its trust dialog, which agtx does not
    // answer by design. Delivering into a blocked session is not a working lane.

    assert_eq!(
        verified.len() + unverified.len(),
        AGENTS.len(),
        "every agent must be classified"
    );
    assert_eq!(unknown().prompt_injection(), PromptInjection::Unknown);
}

// =============================================================================
// agent_native_skill_dir
// =============================================================================

#[test]
fn native_skill_dir_parity() {
    let expected: &[(&str, Option<(&str, &str)>)] = &[
        ("claude", Some((".claude/commands", "agtx"))),
        ("codex", Some((".codex/skills", ""))),
        ("copilot", Some((".github/agents", "agtx"))),
        ("gemini", Some((".gemini/commands", "agtx"))),
        ("opencode", Some((".opencode/command", ""))),
        ("cursor", Some((".cursor/skills", ""))),
        ("grok", Some((".grok/skills", ""))),
        // Vendor-neutral tree, not an agent dotdir.
        ("antigravity", Some((".agents/skills", ""))),
    ];
    for (name, want) in expected {
        assert_eq!(agent_native_skill_dir(name), *want, "{name}");
    }
    assert_eq!(agent_native_skill_dir("mistral"), None);
}

#[test]
fn native_skill_dirs_are_relative_and_contained() {
    for name in AGENTS.iter().copied() {
        let (base, _) = agent_native_skill_dir(name).unwrap();
        assert!(!base.starts_with('/'), "{name}: {base} must be relative");
        assert!(!base.contains(".."), "{name}: {base} must stay in-worktree");
    }
}

// =============================================================================
// skill_dir_to_filename
// =============================================================================

#[test]
fn skill_filename_parity() {
    let expected: &[(&str, &str)] = &[
        ("claude", "plan.md"),
        ("codex", "plan.md"),
        ("copilot", "plan.md"),
        ("gemini", "plan.toml"),
        // Flat directory, so the full prefixed name is the filename.
        ("opencode", "agtx-plan.md"),
        ("cursor", "plan.md"),
        ("grok", "plan.md"),
        ("antigravity", "plan.md"),
    ];
    for (name, want) in expected {
        assert_eq!(&skill_dir_to_filename("agtx-plan", name), want, "{name}");
    }
    assert_eq!(skill_dir_to_filename("agtx-plan", "mistral"), "plan.md");
    // A name without the agtx- prefix is passed through unshortened.
    assert_eq!(skill_dir_to_filename("custom", "claude"), "custom.md");
    assert_eq!(skill_dir_to_filename("custom", "gemini"), "custom.toml");
    assert_eq!(skill_dir_to_filename("custom", "opencode"), "custom.md");
}

// =============================================================================
// transform_plugin_command
// =============================================================================

#[test]
fn plugin_command_parity() {
    let expected: &[(&str, Option<&str>)] = &[
        ("claude", Some("/gsd:plan-phase 1")),
        ("codex", Some("$gsd-plan-phase 1")),
        // Quirk-lock: copilot has no interactive skill invocation, so it gets a
        // file-path reference instead. Giving it a syntax here is a behaviour
        // change, not a migration.
        ("copilot", None),
        ("gemini", Some("/gsd:plan-phase 1")),
        ("opencode", Some("/gsd-plan-phase 1")),
        ("cursor", Some("/gsd-plan-phase 1")),
        ("grok", Some("/gsd-plan-phase 1")),
        ("antigravity", Some("/gsd-plan-phase 1")),
    ];
    for (name, want) in expected {
        assert_eq!(
            transform_plugin_command("/gsd:plan-phase 1", name).as_deref(),
            *want,
            "{name}"
        );
    }
    assert_eq!(
        transform_plugin_command("/gsd:plan-phase 1", "mistral"),
        None
    );
}

#[test]
fn plugin_command_replaces_only_the_first_colon() {
    // Arguments may contain colons; only the namespace separator is rewritten.
    assert_eq!(
        transform_plugin_command("/gsd:plan a:b", "grok").as_deref(),
        Some("/gsd-plan a:b")
    );
    assert_eq!(
        transform_plugin_command("/gsd:plan a:b", "codex").as_deref(),
        Some("$gsd-plan a:b")
    );
}

// =============================================================================
// identity
// =============================================================================

#[test]
fn identity_parity() {
    let expected: &[(&str, &str, &str, &str)] = &[
        (
            "claude",
            "claude",
            "Anthropic's Claude Code CLI",
            "Claude <noreply@anthropic.com>",
        ),
        (
            "codex",
            "codex",
            "OpenAI's Codex CLI",
            "Codex <noreply@openai.com>",
        ),
        (
            "copilot",
            "copilot",
            "GitHub Copilot CLI",
            "GitHub Copilot <noreply@github.com>",
        ),
        (
            "gemini",
            "gemini",
            "Google Gemini CLI",
            "Gemini <noreply@google.com>",
        ),
        (
            "opencode",
            "opencode",
            "AI-powered coding assistant",
            "OpenCode <noreply@opencode.ai>",
        ),
        (
            "cursor",
            "agent",
            "Cursor Agent CLI",
            "Cursor Agent <noreply@cursor.com>",
        ),
        (
            "grok",
            "grok",
            "xAI's Grok Build CLI",
            "Grok <noreply@x.ai>",
        ),
        (
            "antigravity",
            "agy",
            "Google's Antigravity CLI",
            "Antigravity <noreply@google.com>",
        ),
    ];
    for (name, binary, description, co_author) in expected {
        let a = agent(name);
        assert_eq!(&a.command, binary, "{name} binary");
        assert_eq!(&a.description, description, "{name} description");
        assert_eq!(&a.co_author, co_author, "{name} co-author");
    }
}

// =============================================================================
// scan_agent_skills — discovery of an agent's pre-existing skills
// =============================================================================

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

const MD_SKILL: &str = "---\ndescription: Plan the work\n---\n\nbody\n";

#[test]
fn scan_agent_skills_parity() {
    // One project carrying every agent's layout at once, so each assertion also
    // proves an agent scans *only* its own tree.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, ".claude/commands/agtx/plan.md", MD_SKILL);
    write(root, ".github/agents/agtx/plan.md", MD_SKILL);
    write(
        root,
        ".gemini/commands/agtx/plan.toml",
        "description = \"Plan the work\"\n\nprompt = \"\"\"\nbody\n\"\"\"\n",
    );
    write(root, ".codex/skills/agtx-plan/SKILL.md", MD_SKILL);
    write(root, ".cursor/skills/agtx-plan/SKILL.md", MD_SKILL);
    write(root, ".grok/skills/agtx-plan/SKILL.md", MD_SKILL);
    write(root, ".agents/skills/agtx-plan/SKILL.md", MD_SKILL);
    // OpenCode's project commands live under .config/, not in the tree agtx
    // deploys into (.opencode/command). Both are written here to lock which one
    // the scan reads.
    write(root, ".config/opencode/command/agtx-plan.md", "body\n");
    write(root, ".opencode/command/ignored.md", "body\n");

    let expected: &[(&str, &[(&str, &str)])] = &[
        ("claude", &[("/agtx:plan", "Plan the work")]),
        ("codex", &[("$agtx-plan", "Plan the work")]),
        // Copilot cannot invoke a skill interactively, but its skills are still
        // discovered and offered by the `/` picker.
        ("copilot", &[("/agtx:plan", "Plan the work")]),
        ("gemini", &[("/agtx:plan", "Plan the work")]),
        // Flat layout reads no frontmatter: the description is the stem.
        ("opencode", &[("/agtx-plan", "agtx plan")]),
        ("cursor", &[("/agtx-plan", "Plan the work")]),
        ("grok", &[("/agtx-plan", "Plan the work")]),
        ("antigravity", &[("/agtx-plan", "Plan the work")]),
    ];
    for (name, want) in expected {
        let got = agtx::skills::scan_agent_skills(name, root);
        let want: Vec<(String, String)> = want
            .iter()
            .map(|(c, d)| (c.to_string(), d.to_string()))
            .collect();
        assert_eq!(got, want, "{name}");
    }
    assert!(agtx::skills::scan_agent_skills("mistral", root).is_empty());
}

#[test]
fn scan_agent_skills_falls_back_to_the_name_without_frontmatter() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        ".claude/commands/agtx/plan-phase.md",
        "no frontmatter\n",
    );
    write(
        root,
        ".grok/skills/agtx-plan-phase/SKILL.md",
        "no frontmatter\n",
    );
    assert_eq!(
        agtx::skills::scan_agent_skills("claude", root),
        vec![("/agtx:plan-phase".to_string(), "plan phase".to_string())]
    );
    // The skill-directory layout falls back to the full directory name, which
    // still carries the `agtx-` prefix.
    assert_eq!(
        agtx::skills::scan_agent_skills("grok", root),
        vec![(
            "/agtx-plan-phase".to_string(),
            "agtx plan phase".to_string()
        )]
    );
}

#[test]
fn scan_agent_skills_is_sorted_and_skips_malformed_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, ".claude/commands/agtx/zeta.md", MD_SKILL);
    write(root, ".claude/commands/agtx/alpha.md", MD_SKILL);
    // Wrong extension, and a loose file where a namespace directory belongs.
    write(root, ".claude/commands/agtx/notes.txt", MD_SKILL);
    write(root, ".claude/commands/stray.md", MD_SKILL);
    // A skill directory without a SKILL.md is not a skill.
    std::fs::create_dir_all(root.join(".grok/skills/empty")).unwrap();

    let commands: Vec<String> = agtx::skills::scan_agent_skills("claude", root)
        .into_iter()
        .map(|(c, _)| c)
        .collect();
    assert_eq!(commands, vec!["/agtx:alpha", "/agtx:zeta"]);
    assert!(agtx::skills::scan_agent_skills("grok", root).is_empty());
}

// =============================================================================
// Prompt normalization and the launch-lane size cap
// =============================================================================

use agtx::agent::spec::{can_launch_with_prompt, normalize_prompt, MAX_LAUNCH_PROMPT_BYTES};

#[test]
fn shell_metacharacters_survive_as_literal_text() {
    // The command is wrapped in `sh -c '…'`, so anything that could end the
    // quoted word or start a substitution must come back out verbatim. This is
    // the case that turns a task description into executed code if it regresses.
    let nasty = r#"fix `rm -rf /` and $(whoami) and ${HOME} and "quotes" and it's"#;
    let cmd = agent("claude").build_interactive_command(nasty);

    // Exactly one opening quote, and the payload is escaped, not interpreted.
    assert!(cmd.starts_with("claude --dangerously-skip-permissions '"));
    assert!(cmd.ends_with('\''));
    assert!(cmd.contains("`rm -rf /`"), "backticks kept literal: {cmd}");
    assert!(
        cmd.contains("$(whoami)"),
        "substitution kept literal: {cmd}"
    );
    assert!(cmd.contains("${HOME}"), "expansion kept literal: {cmd}");
    // Every single quote from the input is escaped by the POSIX idiom; no bare
    // quote can appear except the two wrapping ones.
    let inner = &cmd["claude --dangerously-skip-permissions '".len()..cmd.len() - 1];
    for (i, _) in inner.match_indices('\'') {
        let around = &inner[i.saturating_sub(2)..(i + 3).min(inner.len())];
        assert!(
            around.contains(r#"'"'"#) || around.contains(r#""'"#),
            "unescaped quote at {i} in {inner:?}"
        );
    }
}

#[test]
fn newlines_and_tabs_survive_normalization() {
    // Ordinary structure in a task description, and just bytes inside a quoted
    // argv word — plan D phase 4 depends on them reaching the agent intact.
    let text = "line one\n\n- bullet\n\tindented";
    assert_eq!(normalize_prompt(text), text);
    assert!(agent("claude")
        .build_interactive_command(text)
        .contains("\n\n- bullet"));
}

#[test]
fn normalization_strips_characters_that_cannot_survive_argv() {
    // NUL truncates the C string, silently dropping the rest of the task.
    assert_eq!(normalize_prompt("before\u{0}after"), "beforeafter");
    // CR is a paste artifact from CRLF sources; it would move the agent's cursor
    // to column zero and corrupt the echoed prompt.
    assert_eq!(
        normalize_prompt("line one\r\nline two"),
        "line one\nline two"
    );
    // ESC would let pasted text drive the agent's TUI.
    assert_eq!(normalize_prompt("plain\u{1b}[31mred"), "plain[31mred");
    // A prompt of nothing but control characters collapses to empty, which the
    // launch check then treats as "no prompt".
    assert_eq!(normalize_prompt("\u{0}\r\u{1b}"), "");
}

#[test]
fn launch_lane_is_declined_for_oversized_prompts() {
    use agtx::agent::PromptInjection;

    let ok = "x".repeat(MAX_LAUNCH_PROMPT_BYTES);
    let too_big = "x".repeat(MAX_LAUNCH_PROMPT_BYTES + 1);

    assert!(can_launch_with_prompt(PromptInjection::Argv, &ok));
    assert!(
        !can_launch_with_prompt(PromptInjection::Argv, &too_big),
        "oversized prompts must fall back to the mid-session lane"
    );
    // Unverified agents never take the lane, whatever the size.
    assert!(!can_launch_with_prompt(PromptInjection::Unknown, &ok));
    // Neither does an empty prompt — there would be nothing to deliver.
    assert!(!can_launch_with_prompt(PromptInjection::Argv, ""));
}

#[test]
fn the_size_cap_is_counted_in_bytes_not_chars() {
    use agtx::agent::PromptInjection;
    // `execve` counts bytes. A multi-byte character must not let a prompt slip
    // past the cap by being counted as one.
    let wide = "é".repeat(MAX_LAUNCH_PROMPT_BYTES); // 2 bytes each
    assert!(wide.chars().count() <= MAX_LAUNCH_PROMPT_BYTES);
    assert!(wide.len() > MAX_LAUNCH_PROMPT_BYTES);
    assert!(!can_launch_with_prompt(PromptInjection::Argv, &wide));
}
