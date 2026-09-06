//! Route tests for `agtx serve`.
//!
//! Most tests use `tower`'s `oneshot` without a listener. The revocation
//! regression uses an ephemeral loopback listener to exercise existing sockets.
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

/// Every file in `web/` is in the asset table.
///
/// `include_bytes!` guarantees the other direction — a *listed* file must exist
/// or the build fails — but nothing makes every file on disk listed. A new
/// module left out 404s to the SPA shell, so an `import` receives HTML, the
/// module graph fails to parse, and the whole app renders a blank page with the
/// error only in a console nobody is looking at. That is not hypothetical: it
/// is exactly what `ansi.js` did when it was added.
#[test]
fn the_asset_table_covers_the_web_directory() {
    let listed: std::collections::HashSet<&str> =
        agtx::web::assets::ASSETS.iter().map(|a| a.path).collect();

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("web");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("web/ exists") {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if !listed.contains(name.as_str()) {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "these files are in web/ but not in ASSETS, so they 404 to the shell: {missing:?}"
    );
}

/// The service worker precaches the shell by name, so its list has the same
/// drift risk as the asset table — a module missing from it simply is not
/// available offline.
#[tokio::test]
async fn the_service_worker_precaches_every_script() {
    let f = fixture();
    let (_, _, body) = raw(state_for(&f, ServeMode::Global), "/sw.js").await;
    let js = String::from_utf8_lossy(&body);
    for asset in agtx::web::assets::ASSETS {
        if asset.path.ends_with(".js") && asset.path != "sw.js" {
            assert!(
                js.contains(asset.path),
                "{} is served but not precached by the service worker",
                asset.path
            );
        }
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

// ── live pane socket ────────────────────────────────────────────────────

use agtx::web::ws::{ClientMessage, ServerMessage, Subscription};

/// The browser writes these by hand, so a rename that nothing pins breaks the
/// live view silently: the socket connects, the subscribe is rejected as a bad
/// message, and the pane just stays empty.
#[test]
fn the_socket_wire_format_is_what_the_client_writes() {
    let sub: ClientMessage =
        serde_json::from_str(r#"{"type":"subscribe","project_id":"p","task_id":"t"}"#).unwrap();
    assert!(matches!(
        sub,
        ClientMessage::Subscribe { project_id, task_id } if project_id == "p" && task_id == "t"
    ));

    let un: ClientMessage = serde_json::from_str(r#"{"type":"unsubscribe"}"#).unwrap();
    assert!(matches!(un, ClientMessage::Unsubscribe));

    // And the three the client switches on.
    let frame = serde_json::to_string(&ServerMessage::Frame {
        task_id: "t",
        content: "hi",
    })
    .unwrap();
    assert_eq!(frame, r#"{"type":"frame","task_id":"t","content":"hi"}"#);
    assert_eq!(
        serde_json::to_string(&ServerMessage::Gone { task_id: "t" }).unwrap(),
        r#"{"type":"gone","task_id":"t"}"#
    );
    assert!(serde_json::to_string(&ServerMessage::Error {
        message: "nope".into()
    })
    .unwrap()
    .starts_with(r#"{"type":"error""#));
}

/// `Subscription` holds a `dyn TmuxOperations`, which has no business
/// implementing `Debug` just so a test can call `unwrap_err`.
fn subscribe_err(state: &ServerState, pid: &str, tid: &str) -> String {
    Subscription::open(state, pid, tid)
        .err()
        .expect("expected a refusal")
}

/// Subscribing is where every "why is my terminal blank" question gets its
/// answer, so each refusal has to name its own cause rather than share one.
#[tokio::test]
async fn subscribing_refuses_with_a_specific_reason() {
    let f = fixture();
    let state = state_for(&f, ServeMode::Global);

    let err = subscribe_err(&state, "no-such-project", "t");
    assert!(err.contains("no-such-project"), "got {err}");

    let err = subscribe_err(&state, &f.project_id, "no-such-task");
    assert!(err.contains("no-such-task"), "got {err}");

    // A task with no tmux session has no pane to stream — the common case for
    // anything in Backlog.
    let idle = add_task(&f, "Never started", TaskStatus::Backlog);
    let err = subscribe_err(&state, &f.project_id, &idle.id);
    assert!(err.contains("no active session"), "got {err}");
}

/// `/ws` must be a real route rather than falling through to the SPA shell —
/// a plain GET without upgrade headers is a 400 from axum, not a 404 or HTML.
#[tokio::test]
async fn the_socket_route_exists() {
    let f = fixture();
    let app = agtx::web::routes::router(state_for(&f, ServeMode::Global));
    let res = app
        .oneshot(Request::builder().uri("/ws").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_ne!(res.status(), StatusCode::NOT_FOUND);
    let mime = res
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(!mime.contains("text/html"), "/ws fell through to the shell");
}

// ── /input ──────────────────────────────────────────────────────────────

fn running_task(f: &Fixture) -> Task {
    let db = Database::open_project(&f.project_path).unwrap();
    let mut t = Task::new("Running", "claude", &f.project_id);
    t.status = TaskStatus::Running;
    t.session_name = Some("agtx-test-nonexistent:pane".to_string());
    db.create_task(&t).unwrap();
    t
}

/// Text and keys are different requests, and the server must not have to guess
/// which was meant: `send-keys` without `-l` resolves anything matching a key
/// name *as that key*, so a message of "Space" would arrive as `0x20`.
#[tokio::test]
async fn input_needs_exactly_one_of_text_or_key() {
    let f = fixture();
    let t = running_task(&f);
    let uri = format!("/api/projects/{}/tasks/{}/input", f.project_id, t.id);

    for body in [
        serde_json::json!({}),
        serde_json::json!({ "text": "hi", "key": "Enter" }),
    ] {
        let (status, _) = send(state_for(&f, ServeMode::Global), "POST", &uri, Some(body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}

/// A closed key set is deliberate: forwarding arbitrary tmux key names would
/// let a request send `C-d`, an EOF that ends the agent's session, or `C-u`,
/// which kills the composer line.
#[tokio::test]
async fn only_known_keys_are_forwarded() {
    let f = fixture();
    let t = running_task(&f);
    let uri = format!("/api/projects/{}/tasks/{}/input", f.project_id, t.id);

    for key in ["C-d", "C-u", "M-x", "F13", "PageUp", ""] {
        let (status, body) = send(
            state_for(&f, ServeMode::Global),
            "POST",
            &uri,
            Some(serde_json::json!({ "key": key })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{key:?} was accepted");
        assert!(body["error"].as_str().unwrap().contains("unsupported key"));
    }

    // What the chips actually send is all accepted by the parser.
    for key in [
        "y", "n", "1", "2", "3", "Enter", "Escape", "Up", "Down", "C-c",
    ] {
        assert!(
            agtx::core::input::PaneKey::parse(key).is_some(),
            "the UI offers {key:?} but the server rejects it"
        );
    }
}

/// Research and review are input-capable, but still require a session.
#[tokio::test]
async fn input_only_reaches_an_active_phase() {
    let f = fixture();
    for status in [
        TaskStatus::Backlog,
        TaskStatus::Planning,
        TaskStatus::Running,
        TaskStatus::Review,
        TaskStatus::Done,
    ] {
        let mut t = add_task(&f, &format!("Task {}", status.as_str()), status);
        let uri = format!("/api/projects/{}/tasks/{}/input", f.project_id, t.id);
        let (code, _) = send(
            state_for(&f, ServeMode::Global),
            "POST",
            &uri,
            Some(serde_json::json!({ "text": "hello" })),
        )
        .await;
        assert_eq!(
            code,
            if status == TaskStatus::Done {
                StatusCode::CONFLICT
            } else {
                StatusCode::NOT_FOUND
            }
        );
        t.session_name = Some("test:agent".into());
        Database::open_project(&f.project_path)
            .unwrap()
            .update_task(&t)
            .unwrap();
        // Invalid input reaches payload validation without invoking real tmux.
        let (code, _) = send(
            state_for(&f, ServeMode::Global),
            "POST",
            &uri,
            Some(serde_json::json!({ "text": "hello", "key": "Enter" })),
        )
        .await;
        assert_eq!(
            code,
            if status == TaskStatus::Done {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            }
        );
    }
}

/// The same 4096-byte cap and null-byte rule `send_to_task` enforces, so the
/// two callers refuse the same messages.
#[tokio::test]
async fn oversized_and_null_bearing_messages_are_refused() {
    let f = fixture();
    let t = running_task(&f);
    let uri = format!("/api/projects/{}/tasks/{}/input", f.project_id, t.id);

    let (status, body) = send(
        state_for(&f, ServeMode::Global),
        "POST",
        &uri,
        Some(serde_json::json!({ "text": "x".repeat(4097) })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("4096"));

    let (status, body) = send(
        state_for(&f, ServeMode::Global),
        "POST",
        &uri,
        Some(serde_json::json!({ "text": format!("before{}after", char::from(0u8)) })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("null"));
}

/// The one-shot `/pane` answers JSON, so it must not carry the SGR escapes the
/// live socket deliberately keeps.
#[test]
fn the_one_shot_capture_is_plain_text() {
    let coloured = "\u{1b}[1;31mRED\u{1b}[0m plain \u{1b}[32mgreen\u{1b}[0m";
    assert_eq!(agtx::web::routes::strip_sgr(coloured), "RED plain green");
    // Text with no escapes is untouched, including the box-drawing an agent TUI
    // is full of.
    let plain = "│ ✓ done │\n└────────┘";
    assert_eq!(agtx::web::routes::strip_sgr(plain), plain);
}

// ── access token ────────────────────────────────────────────────────────

/// A state that requires pairing, so the auth layer is actually exercised —
/// the other tests run unauthenticated, as a loopback bind does.
fn guarded(f: &Fixture) -> std::sync::Arc<ServerState> {
    let _ = f;
    ServerState::with_auth(ServeMode::Global, true)
}

/// Pair a device directly and return its token.
fn paired_token(label: &str) -> String {
    agtx::web::auth::pair_device(label, None).expect("pairing")
}

async fn with_auth(
    state: std::sync::Arc<ServerState>,
    uri: &str,
    bearer: Option<&str>,
) -> StatusCode {
    let app = agtx::web::routes::router(state);
    let mut req = Request::builder().uri(uri);
    if let Some(b) = bearer {
        req = req.header("authorization", format!("Bearer {b}"));
    }
    app.oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn the_api_requires_the_token() {
    let f = fixture();
    assert_eq!(
        with_auth(guarded(&f), "/api/health", None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        with_auth(guarded(&f), "/api/health", Some("wrong")).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        with_auth(guarded(&f), "/api/health", Some(&paired_token("test"))).await,
        StatusCode::OK
    );
}

/// Writes are behind the same gate as reads. Worth its own assertion because
/// the middleware is layered once and a route registered outside that layer
/// would be open with nothing to notice it.
#[tokio::test]
async fn every_api_route_is_behind_the_token() {
    let f = fixture();
    let t = add_task(&f, "Task", TaskStatus::Backlog);
    let pid = &f.project_id;

    for (method, uri) in [
        ("GET", format!("/api/projects")),
        ("GET", format!("/api/projects/{pid}/tasks")),
        ("GET", format!("/api/projects/{pid}/tasks/{}", t.id)),
        ("GET", format!("/api/projects/{pid}/tasks/{}/diff", t.id)),
        ("GET", format!("/api/projects/{pid}/tasks/{}/pane", t.id)),
        ("POST", format!("/api/projects/{pid}/tasks/{}/action", t.id)),
        ("POST", format!("/api/projects/{pid}/tasks/{}/input", t.id)),
        ("POST", format!("/api/projects/{pid}/tasks")),
        ("PATCH", format!("/api/projects/{pid}/tasks/{}", t.id)),
        ("DELETE", format!("/api/projects/{pid}/tasks/{}", t.id)),
    ] {
        let app = agtx::web::routes::router(guarded(&f));
        let res = app
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(&uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri} answered without a token"
        );
    }
}

/// The shell must stay open. A browser cannot put a header on the initial
/// navigation, so gating the assets would mean carrying the token in the URL of
/// every page load — and they are this app's own JS and CSS, not board data.
#[tokio::test]
async fn the_shell_loads_without_a_token() {
    let f = fixture();
    for path in [
        "/",
        "/index.html",
        "/app.js",
        "/api.js",
        "/ansi.js",
        "/app.css",
    ] {
        assert_eq!(
            with_auth(guarded(&f), path, None).await,
            StatusCode::OK,
            "{path} required a token"
        );
    }
}

/// A real handshake, minus the token.
///
/// The `/ws` route is exempt from the header middleware — a browser cannot put
/// `Authorization` on an upgrade — so this is the assertion that the exemption
/// did not simply leave the socket open. It has to be a *genuine* handshake:
/// `WebSocketUpgrade` is an extractor, so a plain GET is rejected 400 before
/// the handler's token check ever runs, and testing that proves nothing.
fn ws_handshake(uri: &str, protocol: Option<&str>) -> Request<Body> {
    let mut req = Request::builder()
        .uri(uri)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
    if let Some(p) = protocol {
        req = req.header("sec-websocket-protocol", p);
    }
    req.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn the_socket_requires_the_token_by_subprotocol() {
    let f = fixture();

    // No subprotocol at all.
    let res = agtx::web::routes::router(guarded(&f))
        .oneshot(ws_handshake("/ws", None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // A subprotocol carrying the wrong token.
    let res = agtx::web::routes::router(guarded(&f))
        .oneshot(ws_handshake("/ws", Some("agtx.token.wrong")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // The accepting case is deliberately not asserted here. `WebSocketUpgrade`
    // needs a real hyper `OnUpgrade` extension, which `oneshot` cannot supply —
    // a correct token gets 426 Upgrade Required from the extractor rather than
    // 101, so an assertion here would be testing the harness. What matters is
    // that the *refusals* happen before the upgrade, which is what the two
    // above pin. The accept path and the subprotocol echo were verified against
    // a browser.
}

/// Comparison is constant-time, so it must still be *correct* — the obvious way
/// to write that loop is to get the length check wrong.
#[test]
fn token_comparison_accepts_only_an_exact_match() {
    use agtx::web::auth::token_matches;
    assert!(token_matches("abc123", "abc123"));
    assert!(!token_matches("abc123", "abc124"));
    assert!(!token_matches("abc123", "abc12"));
    assert!(!token_matches("abc123", "abc1234"));
    assert!(!token_matches("abc123", ""));
    assert!(!token_matches("", "abc123"));
}

// ── pairing QR ──────────────────────────────────────────────────────────

/// Decode the rendered half-blocks back into a module grid.
///
/// Each cell is `ESC[<fg>m ESC[<bg>m ▀`, where the foreground paints the upper
/// module and the background the lower one.
fn qr_grid(rendered: &str) -> Vec<Vec<bool>> {
    let mut grid: Vec<Vec<bool>> = Vec::new();
    for line in rendered.lines() {
        let (mut upper, mut lower) = (Vec::new(), Vec::new());
        // Split on the block character; each chunk before one holds its colours.
        for cell in line.split('▀').filter(|c| !c.is_empty()) {
            if cell.contains("[0m") {
                continue; // the row reset
            }
            upper.push(cell.contains("[30m"));
            lower.push(cell.contains("[40m"));
        }
        if !upper.is_empty() {
            grid.push(upper);
            grid.push(lower);
        }
    }
    grid
}

/// The two ways a QR that *looks* fine fails to scan: no quiet zone, and
/// finder patterns that are wrong or in the wrong corners. Neither is visible
/// by eye in a terminal, and both make a phone simply do nothing.
#[test]
fn the_pairing_qr_is_scannable() {
    let url = "http://192.168.178.26:8787/#token=be29ea5a70c64030a63526d89f24ba070866a9e273664767";
    let rendered = agtx::web::qr::render(url).expect("a URL always fits a QR");
    let grid = qr_grid(&rendered);

    let w = grid[0].len();
    let h = grid.len();
    assert!(
        w >= 21 + 8,
        "grid is {w} wide, smaller than the smallest QR plus its quiet zone"
    );
    assert!(grid.iter().all(|r| r.len() == w), "ragged grid");

    // Quiet zone: four light modules on every side. Without it a decoder cannot
    // find the code's edges and gives up silently.
    for y in 0..4 {
        assert!(
            !grid[y].iter().any(|m| *m),
            "row {y} of the quiet zone has a dark module"
        );
        assert!(
            !grid[h - 1 - y].iter().any(|m| *m),
            "bottom quiet zone is not clear"
        );
    }
    for (y, row) in grid.iter().enumerate() {
        assert!(
            !row[..4].iter().any(|m| *m),
            "left quiet zone dirty at row {y}"
        );
        assert!(
            !row[w - 4..].iter().any(|m| *m),
            "right quiet zone dirty at row {y}"
        );
    }

    // Finder patterns in three corners. Checking all three is what catches a
    // transposed x/y, which is otherwise a perfectly plausible-looking square.
    const FINDER: [&str; 7] = [
        "#######", "#.....#", "#.###.#", "#.###.#", "#.###.#", "#.....#", "#######",
    ];
    let finder_at = |ox: usize, oy: usize| -> bool {
        FINDER.iter().enumerate().all(|(dy, row)| {
            row.chars()
                .enumerate()
                .all(|(dx, ch)| grid[oy + dy][ox + dx] == (ch == '#'))
        })
    };
    let size = w - 8;
    assert!(finder_at(4, 4), "no finder pattern top-left");
    assert!(finder_at(4 + size - 7, 4), "no finder pattern top-right");
    assert!(finder_at(4, 4 + size - 7), "no finder pattern bottom-left");
}

/// The QR is for a phone, and a phone cannot reach `127.0.0.1` on another
/// machine. Printing one there would scan perfectly and go nowhere.
#[test]
fn a_long_url_still_encodes() {
    // Longer than any realistic pairing URL, to prove the renderer picks a
    // bigger version rather than failing.
    let long = format!("http://192.168.178.26:8787/#token={}", "a".repeat(200));
    assert!(agtx::web::qr::render(&long).is_some());
}

// ── merge-conflict triage ───────────────────────────────────────────────

/// Give the fixture repo a branch that conflicts with `main`, and one that
/// does not, so both answers are exercised against real git.
fn seed_conflict(dir: &Path) {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .expect("git");
    };
    // `task/demo` already edited a.txt; move main underneath it so the two
    // disagree on the same line.
    git(&["checkout", "-q", "main"]);
    std::fs::write(dir.join("a.txt"), "hello\nfrom-main\n").unwrap();
    git(&["commit", "-qam", "main moves"]);

    // And a branch that only adds a file, which merges cleanly.
    git(&["checkout", "-qb", "task/clean", "main"]);
    std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "clean"]);
    git(&["checkout", "-q", "task/demo"]);
}

fn review_task(f: &Fixture, title: &str, branch: &str) -> Task {
    let db = Database::open_project(&f.project_path).unwrap();
    let mut t = Task::new(title, "claude", &f.project_id);
    t.status = TaskStatus::Review;
    t.worktree_path = Some(f.project_path.to_string_lossy().to_string());
    t.branch_name = Some(branch.to_string());
    t.base_branch = Some("main".to_string());
    db.create_task(&t).unwrap();
    t
}

/// "Merges cleanly" and "not checked yet" must not look alike — one of them is
/// an answer someone merges on.
#[tokio::test]
async fn the_diff_reports_conflicts_against_the_base() {
    let f = fixture();
    seed_conflict(&f.project_path);

    let conflicting = review_task(&f, "Conflicting", "task/demo");
    let (status, body) = get(
        state_for(&f, ServeMode::Global),
        &format!(
            "/api/projects/{}/tasks/{}/diff",
            f.project_id, conflicting.id
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["conflicted"], true);
    let files: Vec<&str> = body["conflicting_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(files, vec!["a.txt"], "only the genuinely conflicting file");

    let clean = review_task(&f, "Clean", "task/clean");
    let (_, body) = get(
        state_for(&f, ServeMode::Global),
        &format!("/api/projects/{}/tasks/{}/diff", f.project_id, clean.id),
    )
    .await;
    assert_eq!(body["conflicted"], false);
    assert!(body["conflicting_files"].as_array().unwrap().is_empty());
}

/// A task that is not in Review is not asked about: the question is only
/// meaningful once work is up for merging, and a branch still being written to
/// would answer differently many times before anyone cared.
#[tokio::test]
async fn only_review_tasks_are_checked_for_conflicts() {
    let f = fixture();
    seed_conflict(&f.project_path);

    let db = Database::open_project(&f.project_path).unwrap();
    let mut running = Task::new("Still running", "claude", &f.project_id);
    running.status = TaskStatus::Running;
    running.worktree_path = Some(f.project_path.to_string_lossy().to_string());
    running.branch_name = Some("task/demo".to_string());
    db.create_task(&running).unwrap();

    let (_, body) = get(
        state_for(&f, ServeMode::Global),
        &format!("/api/projects/{}/tasks", f.project_id),
    )
    .await;
    let card = body
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == running.id)
        .unwrap();
    assert!(
        card["conflicted"].is_null(),
        "a Running task should not carry a conflict verdict"
    );
}

/// The board must not block on git. The first ask reports `null` and a
/// background pass fills the cache — checking N branches inside a request that
/// is polled every two seconds is the cost decision 3 refuses for phase status.
#[tokio::test]
async fn the_board_never_waits_on_the_conflict_check() {
    let f = fixture();
    seed_conflict(&f.project_path);
    let task = review_task(&f, "Conflicting", "task/demo");
    let state = state_for(&f, ServeMode::Global);

    let started = std::time::Instant::now();
    let (status, body) = get(
        state.clone(),
        &format!("/api/projects/{}/tasks", f.project_id),
    )
    .await;
    let first_ask = started.elapsed();
    assert_eq!(status, StatusCode::OK);
    assert!(
        first_ask < std::time::Duration::from_millis(500),
        "the board waited {first_ask:?} — it should answer from cache and refresh behind"
    );

    let card = body
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == task.id)
        .unwrap();
    assert!(card["conflicted"].is_null(), "first ask should be unknown");

    // The background pass lands within a poll or two.
    let mut seen = None;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let (_, body) = get(
            state.clone(),
            &format!("/api/projects/{}/tasks", f.project_id),
        )
        .await;
        let card = body
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == task.id)
            .cloned()
            .unwrap();
        if !card["conflicted"].is_null() {
            seen = Some(card);
            break;
        }
    }
    let card = seen.expect("the background pass never filled the conflict cache");
    assert_eq!(card["conflicted"], true);
}

// ── pairing ─────────────────────────────────────────────────────────────

#[test]
fn a_pairing_code_is_single_use() {
    let codes = agtx::web::auth::PairingCodes::default();
    let code = codes.issue();

    assert!(codes.redeem(&code).is_ok());
    // Spent. A code that could be replayed is a credential anyone who saw the
    // screen keeps.
    assert_eq!(
        codes.redeem(&code),
        Err(agtx::web::auth::PairError::Unknown)
    );
}

/// Wrong codes are counted, and enough of them shut pairing until the next
/// launch. `/api/pair` is the one unauthenticated route, so it is the one that
/// must not be hammerable.
#[test]
fn repeated_bad_codes_lock_pairing_out() {
    let codes = agtx::web::auth::PairingCodes::default();
    let real = codes.issue();

    for _ in 0..agtx::web::auth::PAIRING_MAX_FAILURES {
        assert_eq!(
            codes.redeem("not-a-code"),
            Err(agtx::web::auth::PairError::Unknown)
        );
    }

    // Even the genuine code is refused now: the lockout is about the endpoint,
    // not about the code being wrong.
    assert_eq!(
        codes.redeem(&real),
        Err(agtx::web::auth::PairError::LockedOut)
    );
}

/// A device token is minted once and only its hash is kept, so the database
/// cannot hand the credential back to anyone who reads it.
#[tokio::test]
async fn a_paired_device_stores_only_a_hash() {
    let _f = fixture();
    let token = paired_token("Test phone");

    let db = Database::open_global().unwrap();
    let devices = db.list_mobile_devices().unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].label, "Test phone");
    assert_ne!(devices[0].token_hash, token, "the token itself was stored");
    assert_eq!(devices[0].token_hash, agtx::web::auth::sha(&token));
}

/// Revoking one device must not disturb the others — the whole reason this
/// replaced a single shared secret.
#[tokio::test]
async fn revoking_one_device_leaves_the_rest() {
    let f = fixture();
    let keep = paired_token("Keeper");
    let lose = paired_token("Lost phone");

    let db = Database::open_global().unwrap();
    let lost_id = db
        .list_mobile_devices()
        .unwrap()
        .into_iter()
        .find(|d| d.label == "Lost phone")
        .unwrap()
        .id;
    assert!(db.revoke_mobile_device(&lost_id).unwrap());
    // A second revoke reports false rather than succeeding twice.
    assert!(!db.revoke_mobile_device(&lost_id).unwrap());

    assert_eq!(
        with_auth(guarded(&f), "/api/health", Some(&lose)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        with_auth(guarded(&f), "/api/health", Some(&keep)).await,
        StatusCode::OK
    );
}

/// A pairing outlives the server that issued it. The device row is in the
/// global database and the token is in the phone's storage, so a home-screen
/// app reconnects to a later `agtx serve` without a new scan.
///
/// Locked down because the opposite — dropping devices on shutdown — is a
/// reasonable-looking change that would silently make every phone re-scan.
#[tokio::test]
async fn a_pairing_survives_the_server_that_issued_it() {
    let f = fixture();
    let token = agtx::web::auth::pair_device("Phone", Some("session-a")).expect("pairing");

    // A fresh state and router: the same thing a restarted `agtx serve` builds.
    assert_eq!(
        with_auth(guarded(&f), "/api/health", Some(&token)).await,
        StatusCode::OK,
        "the device had to pair again after a restart"
    );
    assert_eq!(Database::open_global().unwrap().list_mobile_devices().unwrap().len(), 1);
}

/// `revoke_session_devices` is scoped to one session, which is what would let a
/// future expiry or forget-on-exit policy be safe: `mobile_devices` is global,
/// so a second agtx serving another project must keep its devices.
#[tokio::test]
async fn revoking_a_session_leaves_another_sessions_devices() {
    let f = fixture();
    let mine = agtx::web::auth::pair_device("Mine", Some("session-a")).expect("pairing");
    let theirs = agtx::web::auth::pair_device("Theirs", Some("session-b")).expect("pairing");

    let db = Database::open_global().unwrap();
    assert_eq!(db.revoke_session_devices("session-a").unwrap(), 1);
    // Idempotent, and it never reaches beyond its own session.
    assert_eq!(db.revoke_session_devices("session-a").unwrap(), 0);

    assert_eq!(
        with_auth(guarded(&f), "/api/health", Some(&mine)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        with_auth(guarded(&f), "/api/health", Some(&theirs)).await,
        StatusCode::OK,
        "another agtx instance lost its device"
    );
}

/// A device paired through the API records the server's session. Pairing and
/// the scoped revoke live in different files, so a missing stamp would leave
/// the provenance blank with nothing failing.
#[tokio::test]
async fn pairing_over_the_api_records_the_serving_session() {
    let _f = fixture();
    let state = agtx::web::state::ServerState::with_session(
        ServeMode::Global,
        true,
        Some("session-a".to_string()),
    );
    let code = state.pairing.issue();
    let app = agtx::web::routes::router(state);

    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/pair")
                .header("content-type", "application/json")
                .body(Body::from(format!(
                    r#"{{"code":"{code}","label":"Phone"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let db = Database::open_global().unwrap();
    assert_eq!(
        db.list_mobile_devices().unwrap()[0].session_id.as_deref(),
        Some("session-a")
    );
}

/// An existing `serve-token` becomes a paired device rather than silently
/// ceasing to work — the phone that was already set up keeps connecting.
#[tokio::test]
async fn the_legacy_token_is_adopted_once() {
    let f = fixture();
    let cfg = tempfile::TempDir::new().unwrap();
    std::env::set_var("AGTX_CONFIG_DIR", cfg.path());

    let legacy = "legacy-token-value";
    std::fs::write(agtx::web::auth::token_path().unwrap(), legacy).unwrap();

    let migrated = agtx::web::auth::migrate_legacy_token(None).unwrap();
    assert_eq!(migrated.as_deref(), Some(legacy));
    assert_eq!(
        with_auth(guarded(&f), "/api/health", Some(legacy)).await,
        StatusCode::OK,
        "the previously paired device stopped working"
    );

    // The file is gone, so there is exactly one credential path, and a second
    // run is a no-op rather than a duplicate row.
    assert!(!agtx::web::auth::token_path().unwrap().exists());
    assert!(agtx::web::auth::migrate_legacy_token(None).unwrap().is_none());
    assert_eq!(
        Database::open_global()
            .unwrap()
            .list_mobile_devices()
            .unwrap()
            .len(),
        1
    );

    std::env::remove_var("AGTX_CONFIG_DIR");
}

// ── tunnel planning ─────────────────────────────────────────────────────

use agtx::web::tunnel::{plan, TunnelScope};

/// `private` is `tailscale serve`; `public` is `funnel` or `cloudflared`. They
/// are a tailnet and the open internet, and Tailscale's own naming invites
/// confusing them.
#[test]
fn private_never_reaches_for_funnel() {
    let only_tailscale = |p: &str| p == "tailscale";
    let p = plan(TunnelScope::Private, 8787, &only_tailscale).unwrap();
    assert_eq!(p.program, "tailscale");
    assert!(p.args.contains(&"serve".to_string()));
    assert!(
        !p.args.contains(&"funnel".to_string()),
        "private must never use funnel — that is the public one"
    );
    assert!(p.args.iter().any(|a| a.contains("127.0.0.1:8787")));

    // And it undoes itself: `tailscale serve` configures the daemon, so it
    // outlives this process unless explicitly turned off.
    let teardown = p.teardown.expect("tailscale serve must be torn down");
    assert!(teardown.contains(&"off".to_string()));
}

#[test]
fn public_prefers_cloudflared_and_says_where_it_reaches() {
    let both = |_: &str| true;
    let p = plan(TunnelScope::Public, 9000, &both).unwrap();
    assert_eq!(p.program, "cloudflared");
    // The child *is* the tunnel, so dying is the teardown.
    assert!(p.teardown.is_none());
    assert!(p.reach.contains("internet"));

    let only_tailscale = |x: &str| x == "tailscale";
    let p = plan(TunnelScope::Public, 9000, &only_tailscale).unwrap();
    assert!(p.args.contains(&"funnel".to_string()));
    assert!(p.reach.contains("internet"));
}

#[test]
fn a_missing_provider_explains_the_alternative() {
    let none = |_: &str| false;
    let err = plan(TunnelScope::Private, 8787, &none)
        .unwrap_err()
        .to_string();
    assert!(err.contains("Tailscale"), "got {err}");

    let err = plan(TunnelScope::Public, 8787, &none)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("--tunnel private"),
        "public should point at the safer option: {err}"
    );
}

#[test]
fn tunnel_scope_is_never_guessed() {
    assert_eq!(TunnelScope::parse("private"), Some(TunnelScope::Private));
    assert_eq!(TunnelScope::parse("public"), Some(TunnelScope::Public));
    for bad in ["", "yes", "PUBLIC", "prviate", "funnel"] {
        assert_eq!(TunnelScope::parse(bad), None, "{bad:?} was accepted");
    }
}

/// A tunnel is exposure even though the listener stays on loopback, so pairing
/// must be required. Reading only the bind address is how a tunnelled server
/// ends up open.
#[test]
fn a_tunnel_counts_as_off_loopback() {
    let mut opts = agtx::web::ServeOptions::default();
    assert!(opts.is_loopback(), "a plain serve is loopback");
    opts.tunnel = Some(TunnelScope::Private);
    assert!(
        !opts.is_loopback(),
        "a tunnelled server must require pairing even though it binds 127.0.0.1"
    );
}

// ── the publish gate ────────────────────────────────────────────────────

/// `task_runtime` exists for a reader outside the TUI process. With no such
/// reader the write is pure cost — a transaction every couple of seconds for
/// the life of every session, on the chance a phone might one day connect. So
/// a board request is what turns publishing on.
#[tokio::test]
async fn asking_for_a_board_marks_it_as_watched() {
    let f = fixture();
    let db = Database::open_global().unwrap();
    let path = f.project_path.to_string_lossy().to_string();
    let window = chrono::Duration::seconds(600);

    assert!(
        !db.board_watched_recently(&path, window).unwrap(),
        "nothing has asked for this board yet"
    );

    let (status, _) = get(
        state_for(&f, ServeMode::Global),
        &format!("/api/projects/{}/tasks", f.project_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    assert!(
        db.board_watched_recently(&path, window).unwrap(),
        "a board request should mark the project as watched"
    );
}

/// The marker ages out, so publishing stops again once nobody is reading.
#[tokio::test]
async fn a_stale_watch_does_not_keep_publishing_alive() {
    let f = fixture();
    let db = Database::open_global().unwrap();
    let path = f.project_path.to_string_lossy().to_string();

    db.note_board_watched(&path).unwrap();
    assert!(db
        .board_watched_recently(&path, chrono::Duration::seconds(600))
        .unwrap());
    // Zero window: every marker is already expired.
    assert!(!db
        .board_watched_recently(&path, chrono::Duration::zero())
        .unwrap());
    // And a project nobody ever asked about is never watched.
    assert!(!db
        .board_watched_recently("/not/a/project", chrono::Duration::seconds(600))
        .unwrap());
}

/// Reads other than the board must not switch publishing on. Opening one task
/// detail is not a reason to start mirroring the whole project's phase status.
#[tokio::test]
async fn only_a_board_request_counts_as_watching() {
    let f = fixture();
    let task = add_task(&f, "A task", TaskStatus::Backlog);
    let db = Database::open_global().unwrap();
    let path = f.project_path.to_string_lossy().to_string();
    let window = chrono::Duration::seconds(600);

    for uri in [
        "/api/health".to_string(),
        "/api/projects".to_string(),
        format!("/api/projects/{}/tasks/{}", f.project_id, task.id),
    ] {
        let (status, _) = get(state_for(&f, ServeMode::Global), &uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }

    assert!(
        !db.board_watched_recently(&path, window).unwrap(),
        "a non-board read switched publishing on"
    );
}

/// Real upgrades exercise the running socket, not just the HTTP auth gate.
#[tokio::test]
async fn revoking_a_device_closes_its_existing_socket_only() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::{timeout, Duration};
    let f = fixture();
    let lost = paired_token("Lost phone");
    let kept = paired_token("Kept phone");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = agtx::web::routes::router(guarded(&f));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    async fn connect(address: std::net::SocketAddr, token: &str) -> tokio::net::TcpStream {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let request = format!("GET /ws HTTP/1.1\r\nHost: {address}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: agtx.token.{token}\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\n") {
            response.push(stream.read_u8().await.unwrap());
        }
        assert!(String::from_utf8(response)
            .unwrap()
            .starts_with("HTTP/1.1 101"));
        stream
    }
    let mut lost_socket = timeout(Duration::from_secs(3), connect(address, &lost))
        .await
        .unwrap();
    let mut kept_socket = timeout(Duration::from_secs(3), connect(address, &kept))
        .await
        .unwrap();
    let device = agtx::web::auth::device_for_token(&lost).unwrap();
    Database::open_global()
        .unwrap()
        .revoke_mobile_device(&device.id)
        .unwrap();
    // An idle socket must be closed too: no new request triggers this check.
    let opcode = timeout(Duration::from_secs(3), lost_socket.read_u8())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(opcode & 0x0f, 8, "expected a WebSocket close frame");
    // A masked ping verifies the other connection is still usable.
    kept_socket
        .write_all(&[0x89, 0x80, 0, 0, 0, 0])
        .await
        .unwrap();
    let opcode = timeout(Duration::from_secs(3), kept_socket.read_u8())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        opcode & 0x0f,
        10,
        "expected a pong from the retained device"
    );
    server.abort();
}

#[tokio::test]
async fn completion_refuses_dirty_worktrees_without_queueing() {
    let f = fixture();
    let mut task = add_task(&f, "Dirty review", TaskStatus::Review);
    task.worktree_path = Some(f.project_path.to_string_lossy().into_owned());
    let db = Database::open_project(&f.project_path).unwrap();
    db.update_task(&task).unwrap();
    // Tracked, staged and untracked changes must all block completion.
    for kind in ["tracked", "staged", "untracked"] {
        if kind == "untracked" {
            std::fs::write(f.project_path.join("new.txt"), "new work").unwrap();
        } else {
            std::fs::write(f.project_path.join("a.txt"), "unfinished work").unwrap();
            if kind == "staged" {
                assert!(std::process::Command::new("git")
                    .current_dir(&f.project_path)
                    .args(["add", "a.txt"])
                    .status()
                    .unwrap()
                    .success());
            }
        }
        let (status, body) = send(
            state_for(&f, ServeMode::Global),
            "POST",
            &format!("/api/projects/{}/tasks/{}/action", f.project_id, task.id),
            Some(serde_json::json!({ "action": "move_to_done" })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{kind}");
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("Uncommitted changes"));
        assert!(db.get_pending_transition_requests().unwrap().is_empty());
        // Restore only the tracked test file before the next variant.
        assert!(std::process::Command::new("git")
            .current_dir(&f.project_path)
            .args([
                "restore",
                "--source=HEAD",
                "--staged",
                "--worktree",
                "a.txt"
            ])
            .status()
            .unwrap()
            .success());
    }
    assert_eq!(
        db.get_task(&task.id).unwrap().unwrap().status,
        TaskStatus::Review
    );
}

#[tokio::test]
async fn completion_allows_a_clean_worktree() {
    let f = fixture();
    let mut task = add_task(&f, "Clean review", TaskStatus::Review);
    task.worktree_path = Some(f.project_path.to_string_lossy().into_owned());
    let db = Database::open_project(&f.project_path).unwrap();
    db.update_task(&task).unwrap();
    let (status, _) = send(
        state_for(&f, ServeMode::Global),
        "POST",
        &format!("/api/projects/{}/tasks/{}/action", f.project_id, task.id),
        Some(serde_json::json!({ "action": "move_to_done" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(db.get_pending_transition_requests().unwrap().len(), 1);
}
