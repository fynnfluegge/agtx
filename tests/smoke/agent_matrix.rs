//! Dump the agent × plugin matrix as JSON, for the smoke runner.
//!
//! `agent_smoke.py`, next to this file, needs to know per agent: the binary to look
//! for on `PATH`, the dialogs agtx answers, how a skill command is spelled, and
//! how the agent's liveness is detected. Every one of those is already declared
//! in [`agtx::agent::AGENT_SPECS`], so the runner reads it from here rather than
//! keeping a second copy that can drift.
//!
//! The resolved commands are produced by the real
//! [`agtx::skills::transform_plugin_command`], so the syntax the runner looks
//! for in a pane is the syntax agtx actually sends.
//!
//! ```sh
//! cargo run --quiet --example agent_matrix
//! ```
//!
//! Placeholders (`{task}`, `{task_id}`, `{phase}`) are left unsubstituted: they
//! are runtime values the runner fills in.

use agtx::agent::{spec, AGENT_SPECS};
use agtx::skills::{transform_plugin_command, BUNDLED_PLUGINS};
use serde_json::{json, Map, Value};

/// Phases the runner can drive. `preresearch` is deliberately absent: it is a
/// one-time project-setup step, not a task phase.
const PHASES: &[&str] = &["research", "planning", "running", "review"];

fn command_for(plugin: &agtx::config::WorkflowPlugin, phase: &str) -> Option<String> {
    match phase {
        "research" => plugin.commands.research.clone(),
        "planning" => plugin.commands.planning.clone(),
        "running" => plugin.commands.running.clone(),
        "review" => plugin.commands.review.clone(),
        _ => None,
    }
}

fn prompt_for(plugin: &agtx::config::WorkflowPlugin, phase: &str) -> Option<String> {
    match phase {
        "research" => plugin.prompts.research.clone(),
        "planning" => plugin.prompts.planning.clone(),
        "running" => plugin.prompts.running.clone(),
        "review" => plugin.prompts.review.clone(),
        _ => None,
    }
}

fn artifact_for(plugin: &agtx::config::WorkflowPlugin, phase: &str) -> Option<String> {
    match phase {
        "research" => plugin.artifacts.research.clone(),
        "planning" => plugin.artifacts.planning.clone(),
        "running" => plugin.artifacts.running.clone(),
        "review" => plugin.artifacts.review.clone(),
        _ => None,
    }
}

fn main() {
    let plugins: Vec<(String, agtx::config::WorkflowPlugin)> = BUNDLED_PLUGINS
        .iter()
        .filter_map(|(name, _, content)| {
            toml::from_str::<agtx::config::WorkflowPlugin>(content)
                .ok()
                .map(|p| (name.to_string(), p))
        })
        .collect();

    let mut agents = Vec::new();
    for s in AGENT_SPECS {
        // The command syntax the runner should expect to see in the pane, per
        // plugin and phase. `None` means the agent has no interactive skill
        // invocation (copilot), so nothing command-shaped is ever sent.
        let mut commands = Map::new();
        for (plugin_name, plugin) in &plugins {
            let mut per_phase = Map::new();
            for phase in PHASES {
                if let Some(canonical) = command_for(plugin, phase) {
                    let resolved = transform_plugin_command(&canonical, s.name);
                    per_phase.insert(
                        phase.to_string(),
                        match resolved {
                            Some(cmd) => Value::String(cmd),
                            None => Value::Null,
                        },
                    );
                }
            }
            commands.insert(plugin_name.clone(), Value::Object(per_phase));
        }

        let dialogs: Vec<Value> = s
            .dialogs
            .iter()
            .map(|d| {
                json!({
                    "patterns": d.patterns,
                    "require_all": d.require_all,
                    "answer": d.answer,
                    "scope": match d.scope {
                        spec::DialogScope::Launch => "launch",
                        spec::DialogScope::Session => "session",
                    },
                })
            })
            .collect();

        agents.push(json!({
            "name": s.name,
            "binary": s.binary,
            "description": s.description,
            // The headless credential, for the CI workflow and for the runner's
            // own "is this agent runnable here?" check. Empty means no verified
            // env-var auth path — see the spec entry's comment for whether that
            // is "none" or "not measured yet".
            "api_key_env": s.api_key_env,
            "launch_prompt_verified": s.launch_prompt_verified,
            "prompt_form": match s.prompt_form {
                spec::PromptForm::Argv => Value::String("argv".into()),
                spec::PromptForm::Flag(f) => Value::String(format!("flag:{f}")),
            },
            "send_strategy": match s.send_strategy {
                spec::SendStrategy::Generic => "generic",
                spec::SendStrategy::Combined => "combined",
                spec::SendStrategy::OpenCodePicker => "opencode_picker",
            },
            "command_syntax": match s.command_syntax {
                spec::CommandSyntax::Colon => "colon",
                spec::CommandSyntax::Hyphen => "hyphen",
                spec::CommandSyntax::Dollar => "dollar",
                spec::CommandSyntax::PiSkill => "pi_skill",
                spec::CommandSyntax::None => "none",
            },
            "process_names": s.process_names,
            "active_indicators": s.active_indicators,
            // Matched only in this agent's own pane — see AgentSpec.
            "scoped_indicators": s.scoped_indicators,
            "dialogs": dialogs,
            "commands": commands,
        }));
    }

    let plugin_json: Vec<Value> = plugins
        .iter()
        .map(|(name, p)| {
            let mut artifacts = Map::new();
            let mut phase_commands = Map::new();
            let mut phase_prompts = Map::new();
            for phase in PHASES {
                if let Some(a) = artifact_for(p, phase) {
                    artifacts.insert(phase.to_string(), Value::String(a));
                }
                if let Some(c) = command_for(p, phase) {
                    phase_commands.insert(phase.to_string(), Value::String(c));
                }
                if let Some(pr) = prompt_for(p, phase) {
                    phase_prompts.insert(phase.to_string(), Value::String(pr));
                }
            }
            json!({
                "name": name,
                "description": p.description,
                "supported_agents": p.supported_agents,
                "init_script": p.init_script,
                "copy_dirs": p.copy_dirs,
                "copy_files": p.copy_files,
                "cyclic": p.cyclic,
                "clear_context_on_advance": p.clear_context_on_advance,
                "artifacts": artifacts,
                "commands": phase_commands,
                "prompts": phase_prompts,
            })
        })
        .collect();

    let out = json!({
        "phases": PHASES,
        "agents": agents,
        "plugins": plugin_json,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
