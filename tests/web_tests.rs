//! Route tests for `agtx serve`.
//!
//! Driven through `tower`'s `oneshot`, so there is no listener, no port and no
//! network — the `Router` is exercised exactly as `axum::serve` would call it.
//!
//! These run only with `--features serve`; the whole module compiles away
//! otherwise, the same way the server does.
#![cfg(all(feature = "serve", feature = "test-mocks"))]

use std::path::{Path, PathBuf};
use std::sync::MutexGuard;

use agtx::db::{Database, Project, Task, TaskRuntime, TaskStatus};
use agtx::web::{ServeMode, ServerState};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

/// Point `AGTX_DATA_DIR` at a scratch directory of this test's own.
///
/// A fresh directory per test rather than one shared for the run, because
/// several of these assert on the *whole* project list and a project left
/// behind by another test would be read as this one's. The lock is still
/// required: the redirect is a process-global env var, so two tests running
/// concurrently would each see the other's directory.
///
/// The returned `TempDir` must outlive the test — dropping it deletes the
/// database out from under the handlers.
fn redirect_data_dir() -> (TempDir, MutexGuard<'static, ()>) {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // A panicking test poisons the lock; the data is unit, so recover and go on.
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new().unwrap();
    std::env::set_var("AGTX_DATA_DIR", dir.path());
    (dir, guard)
}

/// A git repo with one commit on the base branch and one on a task branch, so
/// a diff between them is non-empty.
fn seed_repo(dir: &Path) -> String {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git");
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@example.com"]);
    git(&["config", "user.name", "t"]);
    std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    git(&["checkout", "-qb", "task/demo"]);
    std::fs::write(dir.join("a.txt"), "hello\nworld\n").unwrap();
    git(&["commit", "-qam", "work"]);
    "main".to_string()
}

struct Fixture {
    project_id: String,
    project_path: PathBuf,
    _repo: TempDir,
    _data: TempDir,
    _guard: MutexGuard<'static, ()>,
}

fn fixture() -> Fixture {
    let (data, guard) = redirect_data_dir();
    let repo = TempDir::new().unwrap();
    // Canonicalize: macOS hands out `/var/...` which is a symlink to
    // `/private/var/...`, and the server canonicalizes what it is given. An
    // uncanonicalized path here makes project-mode matching fail on macOS only.
    let path = repo.path().canonicalize().unwrap();
    seed_repo(&path);

    let global = Database::open_global().unwrap();
    let project = Project::new("demo", path.to_string_lossy().to_string());
    global.upsert_project(&project).unwrap();

    Fixture {
        project_id: project.id,
        project_path: path,
        _repo: repo,
        _data: data,
        _guard: guard,
    }
}

fn state_for(f: &Fixture, mode: ServeMode) -> std::sync::Arc<ServerState> {
    let _ = f;
    ServerState::new(mode)
}

async fn get(state: std::sync::Arc<ServerState>, uri: &str) -> (StatusCode, serde_json::Value) {
    let app = agtx::web::routes::router(state);
    let res = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

// ── health ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn health_reports_no_tui_when_nothing_is_beating() {
    let f = fixture();
    let state = state_for(&f, ServeMode::Global);
    let (status, body) = get(state, "/api/health").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["project_count"], 1);
    assert_eq!(body["mode"], "global");
    // Nothing has beaten, so a tap would queue and sit there. This is the flag
    // the "no agtx instance connected" banner is built on, and defaulting it to
    // true would make the banner never appear.
    assert_eq!(body["tui_connected"], false);
}

#[tokio::test]
async fn health_sees_a_live_heartbeat() {
    let f = fixture();
    Database::open_global()
        .unwrap()
        .beat_tui_heartbeat(&f.project_path.to_string_lossy())
        .unwrap();

    let state = state_for(&f, ServeMode::Global);
    let (_, body) = get(state, "/api/health").await;
    assert_eq!(body["tui_connected"], true);
}

/// A heartbeat older than the TTL is the same answer as no heartbeat: a TUI
/// that exited without cleaning up must not read as connected forever.
#[tokio::test]
async fn a_stale_heartbeat_is_not_a_connection() {
    let f = fixture();
    let global = Database::open_global().unwrap();
    global
        .beat_tui_heartbeat(&f.project_path.to_string_lossy())
        .unwrap();

    // Zero TTL: every beat is already expired.
    assert!(!global
        .tui_is_live(&f.project_path.to_string_lossy(), chrono::Duration::zero())
        .unwrap());
    assert!(global
        .tui_is_live(
            &f.project_path.to_string_lossy(),
            chrono::Duration::seconds(6)
        )
        .unwrap());
}

// ── projects ────────────────────────────────────────────────────────────

#[tokio::test]
async fn global_mode_lists_every_project() {
    let f = fixture();
    let state = state_for(&f, ServeMode::Global);
    let (status, body) = get(state, "/api/projects").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], f.project_id);
}

/// Serving one project must not advertise the whole index — the picker exists
/// either way, but in project mode it has exactly one entry.
#[tokio::test]
async fn project_mode_lists_only_its_own() {
    let f = fixture();
    let global = Database::open_global().unwrap();
    let other = Project::new("other", "/somewhere/else");
    global.upsert_project(&other).unwrap();

    let state = state_for(&f, ServeMode::Project(f.project_path.clone()));
    let (_, body) = get(state, "/api/projects").await;

    assert_eq!(body.as_array().unwrap().len(), 1);
    assert_eq!(body[0]["id"], f.project_id);
}

/// `/api/health`'s count and `/api/projects`'s length must be the same number.
/// They are two reads of one rule, and a client that sees five on one and one
/// on the other has no way to decide which is the board.
#[tokio::test]
async fn health_count_matches_the_project_list() {
    let f = fixture();
    let global = Database::open_global().unwrap();
    for name in ["other-a", "other-b"] {
        global
            .upsert_project(&Project::new(name, format!("/elsewhere/{name}")))
            .unwrap();
    }

    for mode in [
        ServeMode::Global,
        ServeMode::Project(f.project_path.clone()),
    ] {
        let (_, health) = get(state_for(&f, mode.clone()), "/api/health").await;
        let (_, list) = get(state_for(&f, mode.clone()), "/api/projects").await;
        assert_eq!(
            health["project_count"].as_u64().unwrap() as usize,
            list.as_array().unwrap().len(),
            "health and /api/projects disagree in {mode:?}"
        );
    }
}

/// A phone holding a bookmark for another project gets a 404 rather than this
/// project's board under the wrong name.
#[tokio::test]
async fn project_mode_refuses_another_projects_id() {
    let f = fixture();
    let global = Database::open_global().unwrap();
    let other = Project::new("other", "/somewhere/else");
    global.upsert_project(&other).unwrap();

    let state = state_for(&f, ServeMode::Project(f.project_path.clone()));
    let (status, _) = get(state, &format!("/api/projects/{}/tasks", other.id)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_project_is_a_404() {
    let f = fixture();
    let state = state_for(&f, ServeMode::Global);
    let (status, body) = get(state, "/api/projects/no-such-project/tasks").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().unwrap().contains("no-such-project"));
}

// ── tasks ───────────────────────────────────────────────────────────────

fn add_task(f: &Fixture, title: &str, status: TaskStatus) -> Task {
    let db = Database::open_project(&f.project_path).unwrap();
    let mut t = Task::new(title, "claude", &f.project_id);
    t.status = status;
    db.create_task(&t).unwrap();
    t
}

#[tokio::test]
async fn the_board_carries_actions_and_phase_status() {
    let f = fixture();
    let planning = add_task(&f, "Planning task", TaskStatus::Planning);

    let db = Database::open_project(&f.project_path).unwrap();
    db.publish_task_runtime(&[TaskRuntime {
        task_id: planning.id.clone(),
        phase_status: agtx::db::PhaseStatus::Blocked,
        pane_hash: None,
        pane_changed_at: None,
        updated_at: chrono::Utc::now(),
    }])
    .unwrap();

    let state = state_for(&f, ServeMode::Global);
    let (status, body) = get(state, &format!("/api/projects/{}/tasks", f.project_id)).await;

    assert_eq!(status, StatusCode::OK);
    let card = &body[0];
    assert_eq!(card["status"], "planning");
    assert_eq!(card["phase_status"], "blocked");
    assert!(card["phase_age_secs"].as_i64().unwrap() < 5);
    let actions: Vec<&str> = card["allowed_actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(actions.contains(&"move_forward"));
}

/// Decision 9: the phone is a human caller, and a Backlog card with no actions
/// is a dead end — triage is most of what mobile is for. The orchestrator gets
/// the opposite answer for the same task.
#[tokio::test]
async fn backlog_offers_a_human_the_triage_actions() {
    let f = fixture();
    let task = add_task(&f, "Backlog task", TaskStatus::Backlog);

    let state = state_for(&f, ServeMode::Global);
    let (_, body) = get(state, &format!("/api/projects/{}/tasks", f.project_id)).await;

    let actions: Vec<&str> = body[0]["allowed_actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(actions.contains(&"research"), "got {actions:?}");
    assert!(actions.contains(&"move_to_planning"), "got {actions:?}");
    assert!(actions.contains(&"move_to_running"), "got {actions:?}");

    // The same task, asked as the orchestrator, offers nothing.
    let db = Database::open_project(&f.project_path).unwrap();
    let t = db.get_task(&task.id).unwrap().unwrap();
    assert!(agtx::core::actions::allowed_actions(
        &t,
        true,
        agtx::core::actions::CallerKind::Orchestrator
    )
    .is_empty());
}

/// An unsatisfied dependency blocks the forward moves and nothing else. This
/// path is only reachable for a human caller, which is why it needs a test:
/// with the orchestrator it was unreachable code.
#[tokio::test]
async fn unsatisfied_dependencies_hide_the_forward_moves() {
    let f = fixture();
    let db = Database::open_project(&f.project_path).unwrap();
    let dep = Task::new("Dependency", "claude", &f.project_id);
    db.create_task(&dep).unwrap();
    let mut blocked = Task::new("Blocked", "claude", &f.project_id);
    blocked.referenced_tasks = Some(dep.id.clone());
    db.create_task(&blocked).unwrap();

    let state = state_for(&f, ServeMode::Global);
    let (_, body) = get(state, &format!("/api/projects/{}/tasks", f.project_id)).await;

    let card = body
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == blocked.id)
        .unwrap();
    assert_eq!(card["deps_satisfied"], false);
    let actions: Vec<&str> = card["allowed_actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(!actions.contains(&"move_to_planning"), "got {actions:?}");
    assert!(!actions.contains(&"move_to_running"), "got {actions:?}");
    // `research` is not a forward transition and stays available.
    assert!(actions.contains(&"research"), "got {actions:?}");
}

/// A task with no published runtime row reports `null`, not a made-up status —
/// "nothing has observed this" and "this is working" are different claims.
#[tokio::test]
async fn a_task_with_no_runtime_row_reports_none() {
    let f = fixture();
    add_task(&f, "Never observed", TaskStatus::Backlog);

    let state = state_for(&f, ServeMode::Global);
    let (_, body) = get(state, &format!("/api/projects/{}/tasks", f.project_id)).await;
    assert!(body[0]["phase_status"].is_null());
    assert!(body[0]["phase_age_secs"].is_null());
}

#[tokio::test]
async fn task_detail_flattens_the_card_and_adds_the_rest() {
    let f = fixture();
    let db = Database::open_project(&f.project_path).unwrap();
    let mut t = Task::new("Detailed", "codex", &f.project_id);
    t.status = TaskStatus::Review;
    t.description = Some("the description".to_string());
    t.branch_name = Some("task/demo".to_string());
    db.create_task(&t).unwrap();

    let state = state_for(&f, ServeMode::Global);
    let (status, body) = get(
        state,
        &format!("/api/projects/{}/tasks/{}", f.project_id, t.id),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    // Flattened, so a client reads one shape whether it came from the board or
    // the detail view.
    assert_eq!(body["id"], t.id);
    assert_eq!(body["agent"], "codex");
    assert_eq!(body["description"], "the description");
    assert_eq!(body["branch_name"], "task/demo");
    assert!(body["allowed_actions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a == "move_to_done"));
}

#[tokio::test]
async fn unknown_task_is_a_404() {
    let f = fixture();
    let state = state_for(&f, ServeMode::Global);
    let (status, _) = get(state, &format!("/api/projects/{}/tasks/nope", f.project_id)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── diff ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn diff_returns_stat_and_patch_against_the_base_branch() {
    let f = fixture();
    let db = Database::open_project(&f.project_path).unwrap();
    let mut t = Task::new("Has a worktree", "claude", &f.project_id);
    t.status = TaskStatus::Review;
    // The repo itself stands in for the worktree: it is checked out on
    // `task/demo`, which is what a real worktree would be.
    t.worktree_path = Some(f.project_path.to_string_lossy().to_string());
    t.base_branch = Some("main".to_string());
    db.create_task(&t).unwrap();

    let state = state_for(&f, ServeMode::Global);
    let (status, body) = get(
        state,
        &format!("/api/projects/{}/tasks/{}/diff", f.project_id, t.id),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["base"], "main");
    assert!(body["stat"].as_str().unwrap().contains("a.txt"));
    assert!(body["patch"].as_str().unwrap().contains("+world"));
}

/// A recorded base branch that no longer resolves must not read as "no
/// changes".
///
/// `git::diff_stat` only fails when git cannot be *spawned*: `git diff main
/// HEAD` in a repo with no `main` exits 128 with an empty stdout, which the
/// handler would otherwise render as an empty diff. On a review screen that is
/// a claim someone may merge on, so the base is verified and the response says
/// which one was actually used.
#[tokio::test]
async fn a_base_branch_that_does_not_resolve_falls_back_and_says_so() {
    let f = fixture();
    let db = Database::open_project(&f.project_path).unwrap();
    let mut t = Task::new("Wrong base", "claude", &f.project_id);
    t.worktree_path = Some(f.project_path.to_string_lossy().to_string());
    t.base_branch = Some("no-such-branch".to_string());
    db.create_task(&t).unwrap();

    let (status, body) = get(
        state_for(&f, ServeMode::Global),
        &format!("/api/projects/{}/tasks/{}/diff", f.project_id, t.id),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    // Fell back to the repo's real default rather than reporting the missing
    // branch as an empty diff, and named what it used.
    assert_eq!(body["base"], "main");
    assert!(body["patch"].as_str().unwrap().contains("+world"));
}

/// A Backlog task has no worktree, and asking for its diff is a 404 rather
/// than a 500 — there is nothing wrong, there is just nothing there.
#[tokio::test]
async fn diff_without_a_worktree_is_a_404() {
    let f = fixture();
    let t = add_task(&f, "No worktree", TaskStatus::Backlog);

    let state = state_for(&f, ServeMode::Global);
    let (status, _) = get(
        state,
        &format!("/api/projects/{}/tasks/{}/diff", f.project_id, t.id),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// A worktree recorded in the database but gone from disk — the task was
/// cleaned up, or the directory was removed by hand.
#[tokio::test]
async fn diff_with_a_vanished_worktree_is_a_404() {
    let f = fixture();
    let db = Database::open_project(&f.project_path).unwrap();
    let mut t = Task::new("Gone", "claude", &f.project_id);
    t.worktree_path = Some("/nonexistent/worktree".to_string());
    db.create_task(&t).unwrap();

    let state = state_for(&f, ServeMode::Global);
    let (status, _) = get(
        state,
        &format!("/api/projects/{}/tasks/{}/diff", f.project_id, t.id),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── pane ────────────────────────────────────────────────────────────────

/// No session means no pane. Tested rather than assumed because the handler's
/// other path shells out to tmux, which a test must not depend on.
#[tokio::test]
async fn pane_without_a_session_is_a_404() {
    let f = fixture();
    let t = add_task(&f, "Not running", TaskStatus::Backlog);

    let state = state_for(&f, ServeMode::Global);
    let (status, _) = get(
        state,
        &format!("/api/projects/{}/tasks/{}/pane", f.project_id, t.id),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── the embedded PWA ────────────────────────────────────────────────────

async fn raw(state: std::sync::Arc<ServerState>, uri: &str) -> (StatusCode, String, Vec<u8>) {
    let app = agtx::web::routes::router(state);
    let res = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let mime = res
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, mime, body)
}

/// Every declared asset is reachable and non-empty.
///
/// The table is `include_bytes!`, so a missing *file* is a compile error — but
/// a path that never gets routed is not, and a PWA missing one file is a blank
/// screen with a console error nobody sees.
#[tokio::test]
async fn every_asset_is_served() {
    let f = fixture();
    for asset in agtx::web::assets::ASSETS {
        let (status, mime, body) = raw(
            state_for(&f, ServeMode::Global),
            &format!("/{}", asset.path),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{} status", asset.path);
        assert_eq!(mime, asset.mime, "{} content-type", asset.path);
        assert!(!body.is_empty(), "{} is empty", asset.path);
    }
}

/// A module script whose MIME is not a JavaScript type is refused outright by
/// the browser, which fails the entire app with a blank page.
#[tokio::test]
async fn scripts_are_served_as_javascript() {
    let f = fixture();
    for path in ["/app.js", "/api.js", "/sw.js"] {
        let (_, mime, _) = raw(state_for(&f, ServeMode::Global), path).await;
        assert!(
            mime.starts_with("text/javascript"),
            "{path} served as {mime}, which a browser will refuse for type=module"
        );
    }
}

#[tokio::test]
async fn the_root_serves_the_shell() {
    let f = fixture();
    let (status, mime, body) = raw(state_for(&f, ServeMode::Global), "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(mime.starts_with("text/html"));
    let html = String::from_utf8_lossy(&body);
    assert!(html.contains("<title>agtx</title>"));
    assert!(html.contains(r#"type="module""#));
    assert!(html.contains("manifest.webmanifest"));
}

/// An unmatched `/api/...` must stay JSON. Answering it with the HTML shell
/// hands `fetch` a 200 full of markup, which surfaces as a JSON parse error
/// somewhere unrelated to the actual mistake.
#[tokio::test]
async fn an_unknown_api_path_is_json_not_the_shell() {
    let f = fixture();
    let (status, mime, body) = raw(state_for(&f, ServeMode::Global), "/api/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(mime.contains("json"), "got {mime}");
    assert!(!String::from_utf8_lossy(&body).contains("<html"));
}

/// Assets must not shadow the API: both are on the same router, and a
/// fallback that caught `/api/health` would be invisible until a client asked.
#[tokio::test]
async fn the_api_still_wins_over_the_fallback() {
    let f = fixture();
    let (status, mime, _) = raw(state_for(&f, ServeMode::Global), "/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert!(mime.contains("json"), "got {mime}");
}

/// The manifest has to parse and name icons that exist, or the install prompt
/// never appears — and on iOS an uninstallable PWA cannot receive Web Push.
#[tokio::test]
async fn the_manifest_is_valid_and_its_icons_resolve() {
    let f = fixture();
    let (_, _, body) = raw(state_for(&f, ServeMode::Global), "/manifest.webmanifest").await;
    let manifest: serde_json::Value = serde_json::from_slice(&body).expect("manifest is JSON");

    assert_eq!(manifest["display"], "standalone");
    assert!(manifest["name"].is_string());

    for icon in manifest["icons"].as_array().unwrap() {
        let src = icon["src"].as_str().unwrap();
        let (status, mime, bytes) = raw(state_for(&f, ServeMode::Global), &format!("/{src}")).await;
        assert_eq!(status, StatusCode::OK, "icon {src}");
        assert_eq!(mime, "image/png", "icon {src}");
        // PNG magic: a manifest pointing at something that is not an image is
        // accepted by the manifest parser and rejected by the installer.
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "icon {src} is not a PNG");
    }
}

/// The service worker must not cache `/api/*`. A cached board shows a stale
/// phase status with a fresh-looking timestamp, which is worse than an error.
#[tokio::test]
async fn the_service_worker_leaves_the_api_alone() {
    let f = fixture();
    let (_, _, body) = raw(state_for(&f, ServeMode::Global), "/sw.js").await;
    let js = String::from_utf8_lossy(&body);
    assert!(
        js.contains("/api/"),
        "the worker must mention /api/ to exclude it"
    );
    assert!(
        js.contains("startsWith(\"/api/\")"),
        "the worker no longer excludes /api/ from caching"
    );
}

// ── writes ──────────────────────────────────────────────────────────────

async fn send(
    state: std::sync::Arc<ServerState>,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let app = agtx::web::routes::router(state);
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let res = app.oneshot(req.body(body).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

/// An action queues a row and says so. It must never report the board as moved:
/// only a running TUI drains `transition_requests`.
#[tokio::test]
async fn an_action_queues_rather_than_executing() {
    let f = fixture();
    let task = add_task(&f, "To plan", TaskStatus::Backlog);

    let (status, body) = send(
        state_for(&f, ServeMode::Global),
        "POST",
        &format!("/api/projects/{}/tasks/{}/action", f.project_id, task.id),
        Some(serde_json::json!({ "action": "move_to_planning" })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "queued");
    assert_eq!(body["tui_connected"], false);

    // The row is really there, and the task itself has not moved.
    let db = Database::open_project(&f.project_path).unwrap();
    let queued = db.get_pending_transition_requests().unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].action, "move_to_planning");
    assert_eq!(
        db.get_task(&task.id).unwrap().unwrap().status,
        TaskStatus::Backlog,
        "the API must not move the task itself"
    );

    // And the client can poll it.
    let rid = body["request_id"].as_str().unwrap();
    let (status, poll) = get(
        state_for(&f, ServeMode::Global),
        &format!("/api/projects/{}/requests/{rid}", f.project_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(poll["status"], "pending");
}

/// A client is not the authority on what it may do. Its copy of the task can be
/// seconds old, so the same policy that rendered the buttons is enforced again
/// on arrival.
#[tokio::test]
async fn an_action_the_task_does_not_permit_is_refused() {
    let f = fixture();
    let task = add_task(&f, "Backlog task", TaskStatus::Backlog);

    let (status, body) = send(
        state_for(&f, ServeMode::Global),
        "POST",
        &format!("/api/projects/{}/tasks/{}/action", f.project_id, task.id),
        Some(serde_json::json!({ "action": "move_to_done" })),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"].as_str().unwrap().contains("not available"));
    let db = Database::open_project(&f.project_path).unwrap();
    assert!(db.get_pending_transition_requests().unwrap().is_empty());
}

/// `escalate_to_user` means "stop and ask the person". Offering it to the
/// person is incoherent, so it is the orchestrator's alone.
#[tokio::test]
async fn a_human_cannot_escalate_to_themselves() {
    let f = fixture();
    let task = add_task(&f, "Running task", TaskStatus::Running);

    let (status, _) = send(
        state_for(&f, ServeMode::Global),
        "POST",
        &format!("/api/projects/{}/tasks/{}/action", f.project_id, task.id),
        Some(serde_json::json!({ "action": "escalate_to_user" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // The orchestrator still has it.
    let db = Database::open_project(&f.project_path).unwrap();
    let t = db.get_task(&task.id).unwrap().unwrap();
    assert!(agtx::core::actions::allowed_actions(
        &t,
        true,
        agtx::core::actions::CallerKind::Orchestrator
    )
    .contains(&"escalate_to_user".to_string()));
}

/// An unknown verb is malformed and will never work; a verb the task does not
/// currently permit may work later. A client that cannot tell them apart either
/// retries forever or gives up too early.
#[tokio::test]
async fn an_unknown_verb_is_a_400_not_a_409() {
    let f = fixture();
    let task = add_task(&f, "Task", TaskStatus::Backlog);

    let (status, body) = send(
        state_for(&f, ServeMode::Global),
        "POST",
        &format!("/api/projects/{}/tasks/{}/action", f.project_id, task.id),
        Some(serde_json::json!({ "action": "rm_minus_rf" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("unknown action"));
}

/// Dependencies are the one refusal that clears on its own, so it says so
/// rather than reading as an invalid request.
#[tokio::test]
async fn a_dependency_refusal_explains_itself() {
    let f = fixture();
    let db = Database::open_project(&f.project_path).unwrap();
    let dep = Task::new("Dependency", "claude", &f.project_id);
    db.create_task(&dep).unwrap();
    let mut blocked = Task::new("Blocked", "claude", &f.project_id);
    blocked.referenced_tasks = Some(dep.id.clone());
    db.create_task(&blocked).unwrap();

    let (status, body) = send(
        state_for(&f, ServeMode::Global),
        "POST",
        &format!("/api/projects/{}/tasks/{}/action", f.project_id, blocked.id),
        Some(serde_json::json!({ "action": "move_to_planning" })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["error"].as_str().unwrap().contains("dependencies"),
        "got {}",
        body["error"]
    );

    // `research` is not a forward transition and stays available.
    let (status, _) = send(
        state_for(&f, ServeMode::Global),
        "POST",
        &format!("/api/projects/{}/tasks/{}/action", f.project_id, blocked.id),
        Some(serde_json::json!({ "action": "research" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ── task CRUD ───────────────────────────────────────────────────────────

#[tokio::test]
async fn a_task_can_be_created_edited_and_deleted() {
    let f = fixture();
    let base = format!("/api/projects/{}/tasks", f.project_id);

    let (status, created) = send(
        state_for(&f, ServeMode::Global),
        "POST",
        &base,
        Some(serde_json::json!({ "title": "  From the phone  ", "description": "hi" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Trimmed, not stored with its whitespace.
    assert_eq!(created["title"], "From the phone");
    assert_eq!(created["status"], "backlog");
    let id = created["id"].as_str().unwrap().to_string();

    let (status, edited) = send(
        state_for(&f, ServeMode::Global),
        "PATCH",
        &format!("{base}/{id}"),
        Some(serde_json::json!({ "title": "Renamed" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(edited["title"], "Renamed");

    let (status, _) = send(
        state_for(&f, ServeMode::Global),
        "DELETE",
        &format!("{base}/{id}"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let db = Database::open_project(&f.project_path).unwrap();
    assert!(db.get_task(&id).unwrap().is_none());
}

/// Editing or deleting a task past Backlog would change the work under a
/// running agent — the same rule the MCP tools enforce.
#[tokio::test]
async fn only_backlog_tasks_can_be_edited_or_deleted() {
    let f = fixture();
    let running = add_task(&f, "In flight", TaskStatus::Running);
    let base = format!("/api/projects/{}/tasks/{}", f.project_id, running.id);

    for (method, body) in [
        ("PATCH", Some(serde_json::json!({ "title": "nope" }))),
        ("DELETE", None),
    ] {
        let (status, json) = send(state_for(&f, ServeMode::Global), method, &base, body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{method} was allowed");
        assert!(json["error"].as_str().unwrap().contains("Backlog"));
    }

    let db = Database::open_project(&f.project_path).unwrap();
    assert!(db.get_task(&running.id).unwrap().is_some());
}

#[tokio::test]
async fn a_task_needs_a_usable_title() {
    let f = fixture();
    let base = format!("/api/projects/{}/tasks", f.project_id);

    for title in ["", "   ", "\t\n "] {
        let (status, _) = send(
            state_for(&f, ServeMode::Global),
            "POST",
            &base,
            Some(serde_json::json!({ "title": title })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{title:?} was accepted");
    }

    let too_long = "x".repeat(agtx::core::actions::MAX_TASK_TITLE_CHARS + 1);
    let (status, body) = send(
        state_for(&f, ServeMode::Global),
        "POST",
        &base,
        Some(serde_json::json!({ "title": too_long })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("maximum"));
}

/// A reference to a task that does not exist becomes a dependency nothing can
/// satisfy, leaving the referring task stuck in Backlog with no visible cause.
#[tokio::test]
async fn a_reference_to_a_missing_task_is_refused() {
    let f = fixture();
    let (status, body) = send(
        state_for(&f, ServeMode::Global),
        "POST",
        &format!("/api/projects/{}/tasks", f.project_id),
        Some(serde_json::json!({ "title": "Refers to a ghost", "referenced_tasks": "nope" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("not found"));
}

/// A runaway client — an optimistic UI retrying a failing action in a render
/// loop — must not be able to queue unbounded work for the TUI.
#[tokio::test]
async fn writes_are_rate_limited() {
    let f = fixture();
    let state = state_for(&f, ServeMode::Global);
    let base = format!("/api/projects/{}/tasks", f.project_id);

    let mut limited = false;
    for i in 0..200 {
        let (status, _) = send(
            state.clone(),
            "POST",
            &base,
            Some(serde_json::json!({ "title": format!("task {i}") })),
        )
        .await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
    }
    assert!(limited, "the write budget never kicked in");

    // Reads are unaffected: a client that hit the ceiling must still be able to
    // see the board it just changed.
    let (status, _) = get(state, &format!("/api/projects/{}/tasks", f.project_id)).await;
    assert_eq!(status, StatusCode::OK);
}
