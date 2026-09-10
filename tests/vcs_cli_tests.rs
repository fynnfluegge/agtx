use agtx::db::{Database, Task};
use agtx::git::VcsKind;
use std::process::{Command, Output};
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn run_ok(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn jj_available() -> bool {
    Command::new("jj")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[test]
fn vcs_cli_uses_task_identity_and_never_guesses_jj() {
    let _env = ENV_LOCK.lock().unwrap();
    if !jj_available() {
        return;
    }
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    let data = tempfile::tempdir().unwrap();
    let init = Command::new("jj")
        .args(["git", "init", "--no-colocate"])
        .arg(project.path())
        .output()
        .unwrap();
    assert!(init.status.success());
    std::fs::create_dir_all(project_path.join(".agtx")).unwrap();
    std::fs::write(
        project_path.join(".agtx/config.toml"),
        "vcs = \"jj\"\nbase_branch = \"root()\"\n",
    )
    .unwrap();

    std::env::set_var("AGTX_DATA_DIR", data.path());
    let db = Database::open_project(&project_path).unwrap();
    let mut task = Task::new("CLI context", "codex", "test-project");
    task.worktree_path = Some(project_path.to_string_lossy().into_owned());
    db.create_task(&task).unwrap();
    std::env::remove_var("AGTX_DATA_DIR");

    let run = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agtx"));
        command
            .arg("vcs")
            .args(args)
            .current_dir(&project_path)
            .env("AGTX_DATA_DIR", data.path())
            .env("AGTX_CONFIG_DIR", data.path())
            .env("AGTX_PROJECT_ROOT", &project_path)
            .env("AGTX_WORKTREE", &project_path)
            .env("AGTX_TASK_ID", &task.id);
        command.output().unwrap()
    };

    let default_git = run(&["info"]);
    assert!(!default_git.status.success());
    assert!(
        String::from_utf8_lossy(&default_git.stderr).contains("not a git checkout"),
        "{}",
        String::from_utf8_lossy(&default_git.stderr)
    );

    task.vcs = Some(VcsKind::Jj);
    std::env::set_var("AGTX_DATA_DIR", data.path());
    db.update_task(&task).unwrap();
    std::env::remove_var("AGTX_DATA_DIR");

    let explicit_jj = run(&["info"]);
    assert!(
        explicit_jj.status.success(),
        "{}",
        String::from_utf8_lossy(&explicit_jj.stderr)
    );
    let stdout = String::from_utf8_lossy(&explicit_jj.stdout);
    assert!(stdout.contains("backend:    jj"), "{stdout}");
    assert!(stdout.contains("base:       root()"), "{stdout}");

    let clean_conflicts = run(&["conflicts"]);
    assert!(
        clean_conflicts.status.success(),
        "{}",
        String::from_utf8_lossy(&clean_conflicts.stderr)
    );
    assert!(String::from_utf8_lossy(&clean_conflicts.stdout).contains("conflicts: no"));

    let diff = run(&["diff", "--task"]);
    assert!(
        diff.status.success(),
        "{}",
        String::from_utf8_lossy(&diff.stderr)
    );

    let integrate = run(&["integrate-base"]);
    assert!(
        integrate.status.success(),
        "{}",
        String::from_utf8_lossy(&integrate.stderr)
    );

    let finish = run(&["finish-integration"]);
    assert!(
        finish.status.success(),
        "{}",
        String::from_utf8_lossy(&finish.stderr)
    );
}

#[test]
fn git_diff_uses_the_fork_point_and_fetch_integrates_the_remote_base() {
    let _env = ENV_LOCK.lock().unwrap();
    if !git_available() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let remote = root.path().join("remote.git");
    let project = root.path().join("project");
    let upstream = root.path().join("upstream");
    let data = tempfile::tempdir().unwrap();

    run_ok(
        Command::new("git")
            .args(["init", "--bare", "-b", "main"])
            .arg(&remote),
    );
    run_ok(
        Command::new("git")
            .args(["init", "-b", "main"])
            .arg(&project),
    );
    run_ok(Command::new("git").current_dir(&project).args([
        "config",
        "user.email",
        "agtx@example.invalid",
    ]));
    run_ok(
        Command::new("git")
            .current_dir(&project)
            .args(["config", "user.name", "agtx test"]),
    );

    std::fs::write(project.join("base.txt"), "base\n").unwrap();
    run_ok(
        Command::new("git")
            .current_dir(&project)
            .args(["add", "base.txt"]),
    );
    run_ok(
        Command::new("git")
            .current_dir(&project)
            .args(["commit", "-m", "base"]),
    );
    run_ok(
        Command::new("git")
            .current_dir(&project)
            .args(["remote", "add", "origin"])
            .arg(&remote),
    );
    run_ok(
        Command::new("git")
            .current_dir(&project)
            .args(["push", "-u", "origin", "main"]),
    );
    run_ok(
        Command::new("git")
            .args(["clone"])
            .arg(&remote)
            .arg(&upstream),
    );
    run_ok(Command::new("git").current_dir(&upstream).args([
        "config",
        "user.email",
        "agtx@example.invalid",
    ]));
    run_ok(
        Command::new("git")
            .current_dir(&upstream)
            .args(["config", "user.name", "agtx test"]),
    );
    run_ok(
        Command::new("git")
            .current_dir(&project)
            .args(["switch", "-c", "feature"]),
    );
    std::fs::write(project.join("feature.txt"), "feature\n").unwrap();
    run_ok(
        Command::new("git")
            .current_dir(&project)
            .args(["add", "feature.txt"]),
    );
    run_ok(
        Command::new("git")
            .current_dir(&project)
            .args(["commit", "-m", "feature"]),
    );

    std::fs::write(upstream.join("upstream-one.txt"), "one\n").unwrap();
    run_ok(
        Command::new("git")
            .current_dir(&upstream)
            .args(["add", "."]),
    );
    run_ok(
        Command::new("git")
            .current_dir(&upstream)
            .args(["commit", "-m", "upstream one"]),
    );
    run_ok(
        Command::new("git")
            .current_dir(&upstream)
            .args(["push", "origin", "main"]),
    );
    run_ok(
        Command::new("git")
            .current_dir(&project)
            .args(["fetch", "origin"]),
    );
    run_ok(
        Command::new("git")
            .current_dir(&project)
            .args(["branch", "-f", "main", "origin/main"]),
    );

    let project_path = project.canonicalize().unwrap();
    std::fs::create_dir_all(project_path.join(".agtx")).unwrap();
    std::env::set_var("AGTX_DATA_DIR", data.path());
    let db = Database::open_project(&project_path).unwrap();
    let mut task = Task::new("Git CLI context", "codex", "test-project");
    task.worktree_path = Some(project_path.to_string_lossy().into_owned());
    task.branch_name = Some("feature".to_string());
    task.base_branch = Some("main".to_string());
    task.vcs = Some(VcsKind::Git);
    db.create_task(&task).unwrap();
    std::env::remove_var("AGTX_DATA_DIR");

    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_agtx"))
            .arg("vcs")
            .args(args)
            .current_dir(&project_path)
            .env("AGTX_DATA_DIR", data.path())
            .env("AGTX_CONFIG_DIR", data.path())
            .env("AGTX_PROJECT_ROOT", &project_path)
            .env("AGTX_WORKTREE", &project_path)
            .env("AGTX_TASK_ID", &task.id)
            .output()
            .unwrap()
    };

    let diff = run(&["diff", "--task"]);
    assert!(diff.status.success());
    let diff = String::from_utf8_lossy(&diff.stdout);
    assert!(diff.contains("feature.txt"), "{diff}");
    assert!(!diff.contains("upstream-one.txt"), "{diff}");

    std::fs::write(upstream.join("upstream-two.txt"), "two\n").unwrap();
    run_ok(
        Command::new("git")
            .current_dir(&upstream)
            .args(["add", "."]),
    );
    run_ok(
        Command::new("git")
            .current_dir(&upstream)
            .args(["commit", "-m", "upstream two"]),
    );
    run_ok(
        Command::new("git")
            .current_dir(&upstream)
            .args(["push", "origin", "main"]),
    );

    let integrate = run(&["integrate-base", "--fetch"]);
    assert!(
        integrate.status.success(),
        "{}",
        String::from_utf8_lossy(&integrate.stderr)
    );
    assert!(project_path.join("upstream-two.txt").exists());

    let finish = run(&["finish-integration"]);
    assert!(
        finish.status.success(),
        "{}",
        String::from_utf8_lossy(&finish.stderr)
    );
}
