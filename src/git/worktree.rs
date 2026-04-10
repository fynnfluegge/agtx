use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Default worktree directory relative to project root
pub const DEFAULT_WORKTREE_DIR: &str = ".agtx/worktrees";

/// Create a new git worktree for a task from the detected default branch.
pub fn create_worktree(project_path: &Path, task_slug: &str) -> Result<PathBuf> {
    let base_branch = detect_main_branch(project_path)?;
    create_worktree_from_base(project_path, task_slug, &base_branch, DEFAULT_WORKTREE_DIR)
}

/// Create a new git worktree for a task from the specified base branch.
pub fn create_worktree_from_base(
    project_path: &Path,
    task_slug: &str,
    base_branch: &str,
    worktree_dir: &str,
) -> Result<PathBuf> {
    let worktree_path = project_path
        .join(worktree_dir)
        .join(task_slug);

    // If worktree already exists and is valid, return it
    if worktree_path.exists() && worktree_path.join(".git").exists() {
        return Ok(worktree_path);
    }

    // Clean up any partial worktree
    if worktree_path.exists() {
        let _ = std::fs::remove_dir_all(&worktree_path);
    }

    // Ensure parent directory exists
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let base_branch = resolve_base_branch(project_path, base_branch)?;

    // Create worktree with a new branch based on the requested base branch
    let branch_name = format!("task/{}", task_slug);

    // First, try to delete the branch if it exists (from a previous failed attempt)
    let _ = Command::new("git")
        .current_dir(project_path)
        .args(["branch", "-D", &branch_name])
        .output();

    let output = Command::new("git")
        .current_dir(project_path)
        .args(["worktree", "add"])
        .arg(&worktree_path)
        .args(["-b", &branch_name, &base_branch])
        .output()
        .context("Failed to create git worktree")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create worktree: {}", stderr);
    }

    Ok(worktree_path)
}

fn resolve_base_branch(project_path: &Path, base_branch: &str) -> Result<String> {
    let base_branch = base_branch.trim();
    if base_branch.is_empty() {
        return detect_main_branch(project_path);
    }

    let output = Command::new("git")
        .current_dir(project_path)
        .args(["rev-parse", "--verify", base_branch])
        .output()
        .context("Failed to verify configured base branch")?;

    if output.status.success() {
        Ok(base_branch.to_string())
    } else {
        anyhow::bail!("Configured base branch '{}' was not found", base_branch);
    }
}

/// Agent config directories that are always copied from project root to worktrees.
/// These contain commands, skills, and configuration that agents need.
pub const AGENT_CONFIG_DIRS: &[&str] = &[
    ".claude",
    ".gemini",
    ".codex",
    ".github/agents",
    ".config/opencode",
];

/// Output from a shell script run inside a worktree.
#[derive(Debug)]
pub(crate) struct ScriptOutput {
    pub status: std::process::ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Run a shell script inside a worktree, capturing stdout/stderr.
pub(crate) fn run_worktree_script(
    script: &str,
    worktree_path: &Path,
    envs: &[(String, String)],
) -> Result<ScriptOutput> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(script)
        .current_dir(worktree_path)
        .envs(envs.iter().map(|(k, v)| (k, v)))
        .output()
        .with_context(|| format!("Failed to run script: {}", script))?;

    Ok(ScriptOutput {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

/// Initialize a worktree by copying agent config dirs, user-specified files, and running an init script.
///
/// Returns a Vec of warning messages for any issues encountered.
/// Does not fail fatally — errors are collected and returned for the caller to display.
pub fn initialize_worktree(
    project_path: &Path,
    worktree_path: &Path,
    copy_files: Option<&str>,
    init_script: Option<&str>,
    copy_dirs: &[String],
) -> Vec<String> {
    let mut warnings = Vec::new();

    // Always copy agent config directories
    for dir_name in AGENT_CONFIG_DIRS {
        let src = project_path.join(dir_name);
        if src.is_dir() {
            let dst = worktree_path.join(dir_name);
            if let Err(e) = copy_dir_recursive(&src, &dst) {
                warnings.push(format!("Failed to copy '{}' to worktree: {}", dir_name, e));
            }
        }
    }

    // Copy plugin-specific extra directories
    for dir_name in copy_dirs {
        let src = project_path.join(dir_name);
        if src.is_dir() {
            let dst = worktree_path.join(dir_name);
            if let Err(e) = copy_dir_recursive(&src, &dst) {
                warnings.push(format!("Failed to copy '{}' to worktree: {}", dir_name, e));
            }
        }
    }

    // Copy user-specified files/directories
    if let Some(files_str) = copy_files {
        for entry in files_str.split(',') {
            let file_name = entry.trim();
            if file_name.is_empty() {
                continue;
            }
            let src = project_path.join(file_name);
            let dst = worktree_path.join(file_name);

            if !src.exists() {
                warnings.push(format!(
                    "copy_files: '{}' not found in project root, skipping",
                    file_name
                ));
                continue;
            }

            if src.is_dir() {
                if let Err(e) = copy_dir_recursive(&src, &dst) {
                    warnings.push(format!(
                        "Failed to copy directory '{}' to worktree: {}",
                        file_name, e
                    ));
                }
            } else {
                if let Some(parent) = dst.parent() {
                    if !parent.exists() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            warnings.push(format!(
                                "Failed to create directory for '{}': {}",
                                file_name, e
                            ));
                            continue;
                        }
                    }
                }
                if let Err(e) = std::fs::copy(&src, &dst) {
                    warnings.push(format!("Failed to copy '{}' to worktree: {}", file_name, e));
                }
            }
        }
    }

    if let Some(script) = init_script {
        let script = script.trim();
        if !script.is_empty() {
            match run_worktree_script(script, worktree_path, &[]) {
                Ok(result) => {
                    if !result.status.success() {
                        warnings.push(format!(
                            "init_script exited with {}: {}",
                            result.status,
                            result.stderr.trim()
                        ));
                    }
                }
                Err(e) => warnings.push(format!("Failed to run init_script: {}", e)),
            }
        }
    }

    warnings
}

/// Recursively copy a directory and its contents.
pub fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Detect the main branch name (main or master)
pub fn detect_main_branch(project_path: &Path) -> Result<String> {
    // Check if 'main' exists
    let output = Command::new("git")
        .current_dir(project_path)
        .args(["rev-parse", "--verify", "main"])
        .output()
        .context("Failed to check for main branch")?;

    if output.status.success() {
        return Ok("main".to_string());
    }

    // Check if 'master' exists
    let output = Command::new("git")
        .current_dir(project_path)
        .args(["rev-parse", "--verify", "master"])
        .output()
        .context("Failed to check for master branch")?;

    if output.status.success() {
        return Ok("master".to_string());
    }

    // Fallback: get the current branch
    let output = Command::new("git")
        .current_dir(project_path)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .context("Failed to get current branch")?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Remove a git worktree
pub fn remove_worktree(project_path: &Path, task_id: &str, worktree_dir: &str) -> Result<()> {
    let wt_path = worktree_path(project_path, task_id, worktree_dir);

    // Remove the worktree
    let output = Command::new("git")
        .current_dir(project_path)
        .args(["worktree", "remove"])
        .arg(&wt_path)
        .args(["--force"]) // Force in case of uncommitted changes
        .output()
        .context("Failed to remove git worktree")?;

    if !output.status.success() {
        // Try to prune if remove failed
        Command::new("git")
            .current_dir(project_path)
            .args(["worktree", "prune"])
            .output()?;
    }

    Ok(())
}

/// Get the worktree path for a task
pub fn worktree_path(project_path: &Path, task_id: &str, worktree_dir: &str) -> PathBuf {
    project_path.join(worktree_dir).join(task_id)
}

/// Get the worktree path for a task using a custom worktree directory
pub fn worktree_path_with_dir(project_path: &Path, task_id: &str, worktree_dir: &str) -> PathBuf {
    worktree_path(project_path, task_id, worktree_dir)
}

/// Check if a worktree exists for a task
pub fn worktree_exists(project_path: &Path, task_id: &str) -> bool {
    worktree_path(project_path, task_id, DEFAULT_WORKTREE_DIR).exists()
}

/// Check if a worktree exists for a task using a custom worktree directory
pub fn worktree_exists_with_dir(project_path: &Path, task_id: &str, worktree_dir: &str) -> bool {
    worktree_path_with_dir(project_path, task_id, worktree_dir).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_run_worktree_script_captures_output_and_env() {
        let temp_dir = TempDir::new().unwrap();
        let envs = vec![("AGTX_TASK_ID".to_string(), "task-123".to_string())];

        let output =
            run_worktree_script("echo $AGTX_TASK_ID", temp_dir.path(), &envs).unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout.trim(), "task-123");
    }

    #[test]
    fn test_run_worktree_script_nonzero_exit() {
        let temp_dir = TempDir::new().unwrap();

        let output = run_worktree_script("exit 42", temp_dir.path(), &[]).unwrap();

        assert!(!output.status.success());
    }
}
