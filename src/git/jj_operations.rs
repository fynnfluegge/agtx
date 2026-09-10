//! Jujutsu implementation of the checkout operations agtx needs.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::operations::GitOperations;

pub struct RealJjOps;

impl RealJjOps {
    pub fn remove_workspace(
        &self,
        project_path: &Path,
        workspace_path: &Path,
        workspace_name: Option<&str>,
    ) -> Result<()> {
        let project_root = project_path
            .canonicalize()
            .context("Jujutsu project root does not resolve")?;
        let workspace_root = workspace_path.canonicalize().ok();
        if workspace_root.as_ref() == Some(&project_root) {
            return Ok(());
        }
        let name = workspace_name
            .or_else(|| workspace_path.file_name().and_then(|part| part.to_str()))
            .context("Jujutsu workspace has no recorded name")?;

        // `workspace forget` succeeds even for an unknown name. Resolve the
        // recorded registration first and prove that it names this directory;
        // otherwise a stale task record could delete an unrelated checkout.
        let registered = successful(project_path, &["workspace", "root", "--name", name])?;
        let registered = PathBuf::from(String::from_utf8_lossy(&registered.stdout).trim());
        let registered = if registered.is_absolute() {
            registered
        } else {
            project_root.join(registered)
        };
        let expected = if workspace_path.is_absolute() {
            workspace_path.to_path_buf()
        } else {
            project_root.join(workspace_path)
        };
        let registered_matches = match (registered.canonicalize(), workspace_root.as_ref()) {
            (Ok(registered), Some(workspace)) => registered == *workspace,
            _ => registered == expected,
        };
        if !registered_matches {
            bail!(
                "Jujutsu workspace '{name}' is registered at '{}', not '{}'",
                registered.display(),
                workspace_path.display()
            );
        }

        successful(project_path, &["workspace", "forget", name])?;
        if workspace_path.exists() {
            std::fs::remove_dir_all(workspace_path).with_context(|| {
                format!("failed to remove workspace '{}'", workspace_path.display())
            })?;
        }
        Ok(())
    }
}

fn run(path: &Path, args: &[&str]) -> Result<Output> {
    Command::new("jj")
        .current_dir(path)
        .args(args)
        .output()
        .with_context(|| format!("failed to run jj {}", args.join(" ")))
}

fn successful(path: &Path, args: &[&str]) -> Result<Output> {
    let output = run(path, args)?;
    if !output.status.success() {
        bail!(
            "jj {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output)
}

/// Resolve a configured revision to exactly one commit.
pub fn resolve_revision(repo: &Path, revision: &str) -> Result<String> {
    let output = successful(
        repo,
        &[
            "log",
            "-r",
            revision,
            "--no-graph",
            "-T",
            "commit_id ++ \"\\n\"",
        ],
    )?;
    let commits: Vec<_> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect();
    match commits.as_slice() {
        [commit] => Ok(commit.clone()),
        [] => bail!("Jujutsu revision '{revision}' did not resolve to a commit"),
        _ => bail!("Jujutsu revision '{revision}' resolved to multiple commits"),
    }
}

fn workspace_path(project: &Path, slug: &str, worktree_dir: &str) -> PathBuf {
    project.join(worktree_dir).join(slug)
}

impl GitOperations for RealJjOps {
    fn create_worktree(
        &self,
        project_path: &Path,
        task_slug: &str,
        base_branch: &str,
        worktree_dir: &str,
        _branch_prefix: &str,
    ) -> Result<String> {
        let path = workspace_path(project_path, task_slug, worktree_dir);
        if path.exists() {
            if super::is_jj_repo(&path) {
                return Ok(path.to_string_lossy().into_owned());
            }
            bail!(
                "refusing to replace existing non-Jujutsu directory '{}'",
                path.display()
            );
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let requested_base = if base_branch.trim().is_empty() {
            "trunk()"
        } else {
            base_branch.trim()
        };
        let base = resolve_revision(project_path, requested_base)?;
        let output = Command::new("jj")
            .current_dir(project_path)
            .args(["workspace", "add", "--name", task_slug, "--revision"])
            .arg(&base)
            .arg(&path)
            .output()
            .context("failed to create Jujutsu workspace")?;
        if !output.status.success() {
            bail!(
                "failed to create Jujutsu workspace: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(path.to_string_lossy().into_owned())
    }

    fn remove_worktree(&self, project_path: &Path, worktree_path: &str) -> Result<()> {
        self.remove_workspace(project_path, Path::new(worktree_path), None)
    }

    fn worktree_exists(&self, project_path: &Path, task_slug: &str, worktree_dir: &str) -> bool {
        super::is_jj_repo(&workspace_path(project_path, task_slug, worktree_dir))
    }

    fn delete_branch(&self, project_path: &Path, branch_name: &str) -> Result<()> {
        let output = run(project_path, &["bookmark", "forget", branch_name])?;
        if output.status.success()
            || String::from_utf8_lossy(&output.stderr).contains("No matching bookmarks")
        {
            Ok(())
        } else {
            bail!(
                "failed to forget Jujutsu bookmark '{branch_name}': {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
        }
    }

    fn diff(&self, _worktree_path: &Path) -> String {
        // There is no unstaged/staged split in Jujutsu.
        String::new()
    }

    fn diff_cached(&self, worktree_path: &Path) -> String {
        run(worktree_path, &["diff", "--git"])
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default()
    }

    fn list_untracked_files(&self, _worktree_path: &Path) -> String {
        String::new()
    }

    fn diff_untracked_file(&self, worktree_path: &Path, file: &str) -> String {
        Command::new("jj")
            .current_dir(worktree_path)
            .args(["diff", "--git", "--"])
            .arg(file)
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
            .unwrap_or_default()
    }

    fn diff_stat_from_main(&self, worktree_path: &Path) -> String {
        run(
            worktree_path,
            &["diff", "--stat", "--from", "trunk()", "--to", "@"],
        )
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
    }

    fn add_all(&self, worktree_path: &Path) -> Result<()> {
        successful(worktree_path, &["st"])?;
        Ok(())
    }

    fn has_changes(&self, worktree_path: &Path) -> bool {
        run(worktree_path, &["diff", "--summary"])
            .map(|output| output.status.success() && !output.stdout.is_empty())
            .unwrap_or(false)
    }

    fn commit(&self, worktree_path: &Path, message: &str) -> Result<()> {
        if !self.has_changes(worktree_path) {
            return Ok(());
        }
        successful(worktree_path, &["describe", "-m", message])?;
        // Seal the described change so later review edits get their own change.
        successful(worktree_path, &["new"])?;
        Ok(())
    }

    fn push(&self, worktree_path: &Path, branch: &str, _set_upstream: bool) -> Result<()> {
        // `commit()` leaves an empty working copy at @, so publish its parent.
        // If the agent already sealed its own work, the same rule selects the
        // most recent meaningful change.
        let target = if self.has_changes(worktree_path) {
            "@"
        } else {
            "@-"
        };
        successful(
            worktree_path,
            &["bookmark", "set", "--allow-backwards", branch, "-r", target],
        )?;
        let exact = format!("exact:{branch}");
        successful(
            worktree_path,
            &["git", "push", "--remote", "origin", "--bookmark", &exact],
        )?;
        Ok(())
    }

    fn fetch_and_check_conflicts(&self, worktree_path: &Path) -> Result<bool> {
        successful(worktree_path, &["git", "fetch", "--remote", "origin"])?;
        Ok(conflict_probe(worktree_path, "@", "trunk()")?.0)
    }

    fn list_files(&self, project_path: &Path) -> Vec<String> {
        run(project_path, &["file", "list"])
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn initialize_worktree(
        &self,
        project_path: &Path,
        worktree_path: &Path,
        copy_files: Option<String>,
        init_script: Option<String>,
        copy_dirs: Vec<String>,
    ) -> Vec<String> {
        super::initialize_worktree(
            project_path,
            worktree_path,
            copy_files.as_deref(),
            init_script.as_deref(),
            &copy_dirs,
        )
    }
}

/// Probe a hypothetical merge without editing a workspace or integrating the
/// operation. Returns whether it conflicts and the conflicting file names.
pub fn conflict_probe(repo: &Path, left: &str, right: &str) -> Result<(bool, Vec<String>)> {
    let heads = format!("heads(({left}) | ({right}))");
    let heads = successful(
        repo,
        &[
            "log",
            "-r",
            &heads,
            "--no-graph",
            "-T",
            "commit_id ++ \"\\n\"",
        ],
    )?;
    if String::from_utf8_lossy(&heads.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
        <= 1
    {
        return Ok((false, Vec::new()));
    }

    let marker = format!("agtx-conflict-probe-{}", uuid::Uuid::new_v4());
    let output = successful(
        repo,
        &[
            "new",
            "--no-edit",
            "--no-integrate-operation",
            "-m",
            &marker,
            left,
            right,
        ],
    )?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let operation = combined
        .lines()
        .find_map(|line| {
            line.strip_prefix(
                "Operation left uncommitted because --no-integrate-operation was requested: ",
            )
        })
        .map(str::trim)
        .context("jj did not report the isolated conflict-probe operation")?;

    let at_op = format!("--at-op={operation}");
    let listing = successful(
        repo,
        &[
            &at_op,
            "log",
            "-r",
            "all()",
            "--no-graph",
            "-T",
            "change_id ++ \"\\t\" ++ conflict ++ \"\\t\" ++ description.first_line() ++ \"\\n\"",
        ],
    )?;
    let row = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .find(|line| line.ends_with(&format!("\t{marker}")))
        .map(str::to_owned)
        .context("could not find the isolated conflict-probe change")?;
    let mut fields = row.splitn(3, '\t');
    let change_id = fields.next().context("probe row omitted change id")?;
    let conflicted = fields.next() == Some("true");
    if !conflicted {
        return Ok((false, Vec::new()));
    }

    let conflicts = successful(repo, &[&at_op, "resolve", "--list", "-r", change_id])?;
    let files = String::from_utf8_lossy(&conflicts.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.rsplit_once("    ")
                .map_or(line, |(path, _kind)| path)
                .to_owned()
        })
        .collect();
    Ok((true, files))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn jj_available() -> bool {
        Command::new("jj")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn jj(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("jj")
            .current_dir(repo)
            .args(args)
            .output()
            .expect("run jj");
        assert!(
            output.status.success(),
            "jj {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_repo() -> Option<tempfile::TempDir> {
        if !jj_available() {
            return None;
        }
        let temp = tempfile::tempdir().expect("temp repo");
        let output = Command::new("jj")
            .args(["git", "init"])
            .arg(temp.path())
            .output()
            .expect("initialize jj repo");
        assert!(
            output.status.success(),
            "jj git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Some(temp)
    }

    #[test]
    fn workspace_lifecycle_uses_explicit_revision() {
        let Some(repo) = init_repo() else { return };
        fs::write(repo.path().join("base.txt"), "base\n").unwrap();
        jj(repo.path(), &["describe", "-m", "base"]);
        jj(repo.path(), &["new"]);

        let ops = RealJjOps;
        let workspace = ops
            .create_worktree(repo.path(), "task-one", "@-", "worktrees", "task")
            .expect("create workspace");
        let workspace = PathBuf::from(workspace);
        assert!(workspace.join("base.txt").exists());
        assert!(jj(repo.path(), &["workspace", "list"]).contains("task-one"));

        ops.remove_worktree(repo.path(), workspace.to_str().unwrap())
            .expect("remove workspace");
        assert!(!workspace.exists());
        assert!(!jj(repo.path(), &["workspace", "list"]).contains("task-one"));
    }

    #[test]
    fn nested_directory_is_not_a_workspace_and_is_not_reused() {
        let Some(repo) = init_repo() else { return };
        let partial = repo.path().join("worktrees").join("partial");
        fs::create_dir_all(&partial).unwrap();

        assert!(!crate::git::is_jj_repo(&partial));
        let error = RealJjOps
            .create_worktree(repo.path(), "partial", "root()", "worktrees", "task")
            .unwrap_err();
        assert!(error.to_string().contains("non-Jujutsu directory"));
        assert!(partial.exists());
    }

    #[test]
    fn removal_requires_the_recorded_workspace_to_match_the_path() {
        let Some(repo) = init_repo() else { return };
        let ops = RealJjOps;
        let workspace = PathBuf::from(
            ops.create_worktree(repo.path(), "task-one", "root()", "worktrees", "task")
                .unwrap(),
        );

        let error = ops
            .remove_workspace(repo.path(), &workspace, Some("default"))
            .unwrap_err();
        assert!(error.to_string().contains("not"));
        assert!(workspace.exists());
        assert!(jj(repo.path(), &["workspace", "list"]).contains("task-one"));
    }

    #[test]
    fn removal_never_deletes_the_project_workspace() {
        let Some(repo) = init_repo() else { return };
        RealJjOps
            .remove_workspace(repo.path(), repo.path(), None)
            .unwrap();
        assert!(repo.path().exists());
        assert!(crate::git::is_jj_repo(repo.path()));
    }

    #[test]
    fn file_listing_works_without_a_colocated_git_repository() {
        if !jj_available() {
            return;
        }
        let repo = tempfile::tempdir().unwrap();
        let output = Command::new("jj")
            .args(["git", "init", "--no-colocate"])
            .arg(repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        fs::write(repo.path().join("listed.txt"), "content\n").unwrap();

        assert!(RealJjOps
            .list_files(repo.path())
            .iter()
            .any(|path| path == "listed.txt"));
    }

    #[test]
    fn conflict_probe_isolated_operation_does_not_move_checkout() {
        let Some(repo) = init_repo() else { return };
        fs::write(repo.path().join("shared.txt"), "base\n").unwrap();
        jj(repo.path(), &["describe", "-m", "base"]);
        let base = jj(
            repo.path(),
            &["log", "-r", "@", "--no-graph", "-T", "commit_id"],
        );

        jj(repo.path(), &["new", &base]);
        fs::write(repo.path().join("shared.txt"), "left\n").unwrap();
        jj(repo.path(), &["describe", "-m", "left"]);
        let left = jj(
            repo.path(),
            &["log", "-r", "@", "--no-graph", "-T", "commit_id"],
        );

        jj(repo.path(), &["new", &base]);
        fs::write(repo.path().join("shared.txt"), "right\n").unwrap();
        jj(repo.path(), &["describe", "-m", "right"]);
        let right = jj(
            repo.path(),
            &["log", "-r", "@", "--no-graph", "-T", "commit_id"],
        );
        let before = jj(
            repo.path(),
            &["log", "-r", "@", "--no-graph", "-T", "change_id"],
        );

        let (conflicted, files) = conflict_probe(repo.path(), &left, &right).expect("probe merge");

        assert!(conflicted);
        assert!(
            files.iter().any(|path| path == "shared.txt"),
            "unexpected conflict listing: {files:?}"
        );
        assert_eq!(
            jj(
                repo.path(),
                &["log", "-r", "@", "--no-graph", "-T", "change_id"]
            ),
            before
        );
    }
}
