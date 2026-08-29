//! Traits for tmux operations to enable testing with mocks.

use anyhow::Result;

#[cfg(feature = "test-mocks")]
use mockall::automock;

/// Operations for tmux window management
#[cfg_attr(feature = "test-mocks", automock)]
pub trait TmuxOperations: Send + Sync {
    /// Create a new tmux window. `keep_shell_on_exit=true` drops to a shell
    /// after `command` exits (task panes); `false` lets tmux close the window
    /// (orchestrator, where a leftover shell looks like a zombie).
    /// Create a window. `env` is set on the **window** (tmux `-e`), so anything
    /// started in it later — a resumed agent, a switched agent — inherits it,
    /// which a per-command `env VAR=x` prefix would not.
    fn create_window(
        &self,
        session: &str,
        window_name: &str,
        working_dir: &str,
        command: Option<String>,
        keep_shell_on_exit: bool,
        env: &[(String, String)],
    ) -> Result<()>;

    /// Kill a tmux window
    fn kill_window(&self, target: &str) -> Result<()>;

    /// Check if a window exists
    fn window_exists(&self, target: &str) -> Result<bool>;

    /// Send keys to a window (with Enter at the end).
    ///
    /// Like [`send_key`](Self::send_key), this goes through
    /// tmux's **key-name** lookup — see the note there.
    fn send_keys(&self, target: &str, keys: &str) -> Result<()>;

    /// Send a **key name** to a window, without a trailing Enter.
    ///
    /// This deliberately does *not* pass `-l`, so tmux resolves the argument as a
    /// key when it matches a key name: `"Enter"`, `"C-c"`, `"C-d"`, `"1"`.
    /// Callers answering a dialog or pressing a control key want exactly that.
    ///
    /// Never use it for text, because the same lookup applies: verified against
    /// tmux 3.5a, `"Space"` arrives as `0x20`, `"Escape"` as `ESC`, and `"Up"` as
    /// `\033[A`. Use [`send_text`](Self::send_text) — or, for a whole message,
    /// [`paste_text`](Self::paste_text) — instead.
    fn send_key(&self, target: &str, keys: &str) -> Result<()>;

    /// Send literal text to a window (`send-keys -l`), without a trailing Enter.
    ///
    /// Unlike [`send_key`](Self::send_key) the argument is never
    /// interpreted as a key name, so text that happens to spell one survives
    /// intact. Use this for anything user- or task-derived.
    fn send_text(&self, target: &str, text: &str) -> Result<()>;

    /// Paste a block of text into a pane using tmux load-buffer + paste-buffer.
    /// This sends proper bracketed paste sequences to the target pane.
    fn paste_text(&self, target: &str, text: &str) -> Result<()>;

    /// Capture pane content
    fn capture_pane(&self, target: &str) -> Result<String>;

    /// Capture pane content with history (returns raw bytes for ANSI parsing)
    fn capture_pane_with_history(&self, target: &str, history_lines: i32) -> Vec<u8>;

    /// Get cursor position and pane height: (cursor_x, cursor_y, pane_height)
    fn get_cursor_info(&self, target: &str) -> Option<(usize, usize, usize)>;

    /// Resize a tmux window
    fn resize_window(&self, target: &str, width: u16, height: u16) -> Result<()>;

    /// Get the current command running in a pane (e.g. "claude", "bash", "zsh")
    fn pane_current_command(&self, target: &str) -> Option<String>;

    /// Check if a session exists
    fn has_session(&self, session: &str) -> bool;

    /// Create a new detached session
    fn create_session(&self, session: &str, working_dir: &str) -> Result<()>;
}

/// Wrap `text` as one POSIX single-quoted shell word.
///
/// [`create_window`](TmuxOperations::create_window) nests the agent's launch
/// command inside `sh -c …`, and that command has **already** been quoted by
/// [`compose_command`](crate::agent::spec::compose_command) — which single-quotes
/// the task prompt. Interpolating it into another pair of single quotes ends the
/// outer word at the first inner quote, and the shell then parses the prompt as
/// code: verified against tmux 3.5a, `claude --dangerously-skip-permissions
/// '/agtx:plan <id>\n\n<task>'` reached the process as argv
/// `["--dangerously-skip-permissions", "/agtx:plan"]` — the id and the whole task
/// silently gone — and codex's `$agtx-plan` fared worse still, with `$agtx`
/// expanded to nothing and only `-plan` surviving.
///
/// So the inner command is quoted here rather than interpolated raw, closing the
/// quote and reopening it around each literal `'` in the usual POSIX way.
fn single_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

/// The shell command tmux is asked to run for an agent pane.
///
/// `keep_shell_on_exit` drops to a login shell afterwards, so a task pane
/// survives the agent quitting and can be inspected.
fn wrap_launch_command(shell_cmd: &str, keep_shell_on_exit: bool) -> String {
    let inner = single_quote(shell_cmd);
    if keep_shell_on_exit {
        format!("env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT sh -c {inner}; exec $SHELL")
    } else {
        format!("env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT sh -c {inner}")
    }
}

/// Real implementation using actual tmux commands
pub struct RealTmuxOps;

impl TmuxOperations for RealTmuxOps {
    fn create_window(
        &self,
        session: &str,
        window_name: &str,
        working_dir: &str,
        command: Option<String>,
        keep_shell_on_exit: bool,
        env: &[(String, String)],
    ) -> Result<()> {
        let mut cmd = std::process::Command::new("tmux");
        let target = format!("{}:", session);
        cmd.args(["-L", super::AGENT_SERVER])
            .args(["new-window", "-d", "-t", &target, "-n", window_name])
            .args(["-c", working_dir]);

        // tmux 3.0+; sets the variable on the window so every process started in
        // it inherits it, including agents launched later by resume or switch.
        for (key, value) in env {
            cmd.args(["-e", &format!("{}={}", key, value)]);
        }

        if let Some(ref shell_cmd) = command {
            // Unset Claude Code's nesting-detection env vars so that agents
            // launched in task panes are not blocked by the "launched inside
            // another Claude Code session" check.
            let wrapped = wrap_launch_command(shell_cmd, keep_shell_on_exit);
            cmd.args(["sh", "-c", &wrapped]);
        }

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut details = String::new();
            if !stderr.trim().is_empty() {
                details.push_str(stderr.trim());
            }
            if !stdout.trim().is_empty() {
                if !details.is_empty() {
                    details.push_str(" | ");
                }
                details.push_str(stdout.trim());
            }
            if details.is_empty() {
                anyhow::bail!("Failed to create tmux window");
            } else {
                anyhow::bail!("Failed to create tmux window: {}", details);
            }
        }
        Ok(())
    }

    fn kill_window(&self, target: &str) -> Result<()> {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["kill-window", "-t", target])
            .output()?;
        Ok(())
    }

    fn window_exists(&self, target: &str) -> Result<bool> {
        let output = std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["list-windows", "-t", target])
            .output()?;
        Ok(output.status.success())
    }

    fn send_keys(&self, target: &str, keys: &str) -> Result<()> {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["send-keys", "-t", target, keys])
            .output()?;
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["send-keys", "-t", target, "Enter"])
            .output()?;
        Ok(())
    }

    fn send_key(&self, target: &str, keys: &str) -> Result<()> {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["send-keys", "-t", target, keys])
            .output()?;
        Ok(())
    }

    fn send_text(&self, target: &str, text: &str) -> Result<()> {
        // `-l` disables key-name lookup; `--` stops tmux parsing text that begins
        // with a dash as flags.
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["send-keys", "-t", target, "-l", "--", text])
            .output()?;
        Ok(())
    }

    fn paste_text(&self, target: &str, text: &str) -> Result<()> {
        use std::io::Write;
        let mut child = std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["load-buffer", "-"])
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["paste-buffer", "-p", "-t", target])
            .output()?;
        Ok(())
    }

    fn capture_pane(&self, target: &str) -> Result<String> {
        let output = std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["capture-pane", "-t", target, "-p"])
            .output()?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn capture_pane_with_history(&self, target: &str, history_lines: i32) -> Vec<u8> {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            // Keep wrapped rows separate: tmux reports cursor_y in physical
            // pane rows, so joining wrapped rows would misalign the cursor.
            .args(["capture-pane", "-t", target, "-p", "-e"])
            .args(["-S", &format!("-{}", history_lines)])
            .output()
            .map(|o| o.stdout)
            .unwrap_or_default()
    }

    fn get_cursor_info(&self, target: &str) -> Option<(usize, usize, usize)> {
        let output = std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args([
                "display",
                "-p",
                "-t",
                target,
                "#{cursor_x} #{cursor_y} #{pane_height}",
            ])
            .output()
            .ok()?;

        if output.status.success() {
            let output_str = String::from_utf8_lossy(&output.stdout);
            let parts: Vec<&str> = output_str.trim().split_whitespace().collect();
            if parts.len() == 3 {
                let cursor_x: usize = parts[0].parse().ok()?;
                let cursor_y: usize = parts[1].parse().ok()?;
                let pane_height: usize = parts[2].parse().ok()?;
                return Some((cursor_x, cursor_y, pane_height));
            }
        }
        None
    }

    fn resize_window(&self, target: &str, width: u16, height: u16) -> Result<()> {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["resize-window", "-t", target])
            .args(["-x", &width.to_string()])
            .args(["-y", &height.to_string()])
            .output()?;
        Ok(())
    }

    fn pane_current_command(&self, target: &str) -> Option<String> {
        let output = std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["display", "-p", "-t", target, "#{pane_current_command}"])
            .output()
            .ok()?;
        if output.status.success() {
            let cmd = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !cmd.is_empty() {
                Some(cmd)
            } else {
                None
            }
        } else {
            None
        }
    }

    fn has_session(&self, session: &str) -> bool {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["has-session", "-t", session])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn create_session(&self, session: &str, working_dir: &str) -> Result<()> {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["new-session", "-d", "-s", session])
            .args(["-c", working_dir])
            .output()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run the wrapper through a real `sh`, exactly as tmux does, and report the
    /// argv the launched process actually received.
    fn argv_delivered_by(shell_cmd: &str) -> Vec<String> {
        let wrapped = wrap_launch_command(shell_cmd, false);
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&wrapped)
            .output()
            .expect("sh should run");
        String::from_utf8_lossy(&out.stdout)
            .split('\u{1}')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    /// `printf` with a NUL-free separator, so an argument containing newlines
    /// stays one argument in the parsed output.
    fn dump_argv_command(args: &[&str]) -> String {
        let quoted: Vec<String> = args.iter().map(|a| single_quote(a)).collect();
        format!("printf '%s\u{1}' {}", quoted.join(" "))
    }

    #[test]
    fn a_launch_prompt_reaches_the_process_intact() {
        // The shape `compose_command` produces for the launch lane: flags, then
        // the whole opening message as one single-quoted word.
        let prompt = "/agtx:plan 57d57fe8-5990\n\nSMOKE TEST line two";
        let cmd = dump_argv_command(&["--dangerously-skip-permissions", prompt]);
        assert_eq!(
            argv_delivered_by(&cmd),
            vec![
                "--dangerously-skip-permissions".to_string(),
                prompt.to_string()
            ],
        );
    }

    #[test]
    fn a_dollar_command_is_not_expanded_away() {
        // Codex's skill syntax is `$agtx-plan`. Interpolated into the wrapper
        // unquoted, the shell expanded `$agtx` to nothing and only `-plan`
        // survived — the whole task text with it.
        let prompt = "$agtx-plan 468b79fa\n\nSMOKE TEST";
        let cmd = dump_argv_command(&["--sandbox", "workspace-write", prompt]);
        assert_eq!(
            argv_delivered_by(&cmd),
            vec![
                "--sandbox".to_string(),
                "workspace-write".to_string(),
                prompt.to_string(),
            ],
        );
    }

    #[test]
    fn a_quote_in_the_task_survives_both_quoting_layers() {
        let prompt = "fix the parser so it's correct";
        let cmd = dump_argv_command(&[prompt]);
        assert_eq!(argv_delivered_by(&cmd), vec![prompt.to_string()]);
    }

    /// Run the wrapped command with a fake `claude` on `PATH` that echoes its
    /// argv, and return what that binary actually received.
    ///
    /// `sh -n` is not enough here: it parses only the *outer* layer, and the
    /// wrapper is always outer-valid. A mis-quoted inner command parses fine and
    /// then dies at run time with "unexpected EOF while looking for matching `''"
    /// — verified, and the reason the first version of this test had no teeth.
    #[cfg(unix)]
    fn argv_seen_by_fake_claude(shell_cmd: &str) -> (bool, String) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let fake = dir.path().join("claude");
        std::fs::write(&fake, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").expect("write fake");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let path = format!(
            "{}:{}",
            dir.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(wrap_launch_command(shell_cmd, false))
            .env("PATH", path)
            .output()
            .expect("sh should run");
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).to_string(),
        )
    }

    /// The orchestrator command is the other thing `create_window` wraps, and it
    /// carries a single-quoted JSON blob for `claude mcp add-json`.
    ///
    /// It used to pre-escape its own wrapping quotes because it was interpolated
    /// raw. Now that `single_quote` quotes the whole command, that pre-escape
    /// double-escapes: the pane dies on an unterminated quote before `add-json`
    /// runs, silently, because the window is created either way. Nothing else
    /// covers this path — the TUI tests mock the command builder.
    #[cfg(unix)]
    #[test]
    fn the_orchestrator_command_reaches_claude_with_its_json_intact() {
        let agent = crate::agent::Agent::new("claude", "claude", "", "");
        let ops = crate::agent::CodingAgent::new(agent);
        let json = r#"{"type":"stdio","command":"/bin/agtx","args":["mcp-serve","/tmp/repo"]}"#;
        let cmd =
            crate::agent::AgentOperations::build_orchestrator_command(&ops, json, "/bin/agtx");
        let (ok, seen) = argv_seen_by_fake_claude(&cmd);
        assert!(ok, "the wrapped orchestrator command must run: {seen}");
        assert!(
            seen.lines().any(|l| l == json),
            "claude must receive the JSON as one intact argument; got:\n{seen}"
        );
    }

    /// ...and a project path containing an apostrophe still arrives intact,
    /// because the caller escapes it for the single-quoted word it lands in.
    #[cfg(unix)]
    #[test]
    fn an_apostrophe_in_the_mcp_json_survives_both_layers() {
        let agent = crate::agent::Agent::new("claude", "claude", "", "");
        let ops = crate::agent::CodingAgent::new(agent);
        let raw = r#"{"args":["mcp-serve","/tmp/it's a repo"]}"#;
        let escaped = raw.replace('\'', "'\"'\"'");
        let cmd =
            crate::agent::AgentOperations::build_orchestrator_command(&ops, &escaped, "/bin/agtx");
        let (ok, seen) = argv_seen_by_fake_claude(&cmd);
        assert!(ok, "the wrapped orchestrator command must run: {seen}");
        assert!(
            seen.lines().any(|l| l == raw),
            "claude must receive the unescaped JSON; got:\n{seen}"
        );
    }

    #[test]
    fn the_pane_drops_to_a_shell_only_when_asked() {
        assert!(wrap_launch_command("claude", true).ends_with("; exec $SHELL"));
        assert!(!wrap_launch_command("claude", false).contains("exec $SHELL"));
    }
}
