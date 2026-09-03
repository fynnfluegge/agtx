//! The read-only half of the HTTP surface.
//!
//! Every handler here reads: SQLite, git and tmux, the same three things the
//! MCP server talks to. Nothing in this module links `App` or mutates the
//! board — writes arrive in Step 3 of the mobile plan.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header, Uri},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::core::actions::{allowed_actions, CallerKind};
use crate::db::{Database, PhaseStatus, Task};

use super::state::{ApiError, ApiResult, ServeMode, ServerState};

pub fn router(state: Arc<ServerState>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/projects", get(projects))
        .route("/api/projects/{pid}/tasks", get(tasks))
        .route("/api/projects/{pid}/tasks/{tid}", get(task_detail))
        .route("/api/projects/{pid}/tasks/{tid}/diff", get(task_diff))
        .route("/api/projects/{pid}/tasks/{tid}/pane", get(task_pane))
        // Writes. None of these execute anything: an action becomes a queued
        // transition request, and task CRUD touches only the task table.
        .route(
            "/api/projects/{pid}/tasks/{tid}/action",
            post(super::writes::task_action),
        )
        .route(
            "/api/projects/{pid}/requests/{rid}",
            get(super::writes::request_status),
        )
        .route(
            "/api/projects/{pid}/tasks/{tid}/input",
            post(super::writes::task_input),
        )
        .route(
            "/api/projects/{pid}/tasks",
            post(super::writes::create_task),
        )
        // Live pane frames. Not under `/api` because it is not a JSON endpoint
        // and the fallback's `/api/` guard would have to special-case it.
        .route("/ws", get(super::ws::handler))
        .route(
            "/api/projects/{pid}/tasks/{tid}",
            patch(super::writes::update_task).delete(super::writes::delete_task),
        )
        .with_state(state)
        // The PWA, last: `/api/*` is matched first, so an asset can never
        // shadow a route, and an unknown `/api/...` path still 404s as JSON
        // rather than being answered with the HTML shell.
        .fallback(asset)
}

// ── the embedded PWA ────────────────────────────────────────────────────

async fn asset(uri: Uri) -> Response {
    let path = uri.path();
    // An unmatched `/api/...` is a client bug, not a deep link. Answering it
    // with `index.html` would hand a `fetch` a 200 full of HTML, which fails
    // as a JSON parse error somewhere unrelated.
    if path.starts_with("/api/") {
        return ApiError::NotFound(format!("no such endpoint: {path}")).into_response();
    }

    match super::assets::find(path) {
        Some(a) => (
            [
                (header::CONTENT_TYPE, a.mime),
                // The shell is embedded in the binary, so its lifetime is the
                // binary's: `agtx update` replaces both together. Revalidating
                // costs one conditional request and removes any chance of a
                // cached UI outliving the API it was built against.
                (header::CACHE_CONTROL, "no-cache"),
            ],
            a.body,
        )
            .into_response(),
        // Hash routing means every real screen is `/#/...`, so any other path
        // is a typo or a probe — but returning the shell keeps a deep link
        // working if routing ever moves into the path.
        None => match super::assets::find(super::assets::INDEX) {
            Some(index) => (
                [
                    (header::CONTENT_TYPE, index.mime),
                    (header::CACHE_CONTROL, "no-cache"),
                ],
                index.body,
            )
                .into_response(),
            None => ApiError::Internal("web assets missing from this build".into()).into_response(),
        },
    }
}

// ── /api/health ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct Health {
    version: &'static str,
    mode: &'static str,
    project_count: usize,
    /// Whether a TUI is currently draining the transition queue. In global mode
    /// this is true when *any* indexed project has a live heartbeat — a phone
    /// on the projects screen is asking "will anything happen if I tap", and
    /// per-project truth is on the board response.
    tui_connected: bool,
}

async fn health(State(state): State<Arc<ServerState>>) -> ApiResult<Json<Health>> {
    let global =
        Database::open_global().map_err(|e| ApiError::Internal(format!("global database: {e}")))?;
    let projects = visible_projects(&state, &global)?;

    let live = match &state.mode {
        ServeMode::Project(path) => global
            .tui_is_live(&path.to_string_lossy(), state.heartbeat_ttl)
            .unwrap_or(false),
        ServeMode::Global => projects.iter().any(|p| {
            global
                .tui_is_live(&p.path, state.heartbeat_ttl)
                .unwrap_or(false)
        }),
    };

    Ok(Json(Health {
        version: env!("CARGO_PKG_VERSION"),
        mode: match state.mode {
            ServeMode::Project(_) => "project",
            ServeMode::Global => "global",
        },
        project_count: projects.len(),
        tui_connected: live,
    }))
}

// ── /api/projects ───────────────────────────────────────────────────────

#[derive(Serialize)]
struct ProjectSummary {
    id: String,
    name: String,
    path: String,
    last_opened: String,
    tui_connected: bool,
}

async fn projects(State(state): State<Arc<ServerState>>) -> ApiResult<Json<Vec<ProjectSummary>>> {
    let global =
        Database::open_global().map_err(|e| ApiError::Internal(format!("global database: {e}")))?;

    Ok(Json(
        visible_projects(&state, &global)?
            .into_iter()
            .map(|p| ProjectSummary {
                tui_connected: global
                    .tui_is_live(&p.path, state.heartbeat_ttl)
                    .unwrap_or(false),
                id: p.id,
                name: p.name,
                path: p.path,
                last_opened: p.last_opened.to_rfc3339(),
            })
            .collect(),
    ))
}

/// The projects this server will talk about at all.
///
/// In project mode the picker still exists on the phone, but it has exactly one
/// entry: serving one project must not advertise the whole index. Shared by
/// both handlers so `/api/health`'s count and `/api/projects`'s length cannot
/// disagree — reporting five on one and one on the other is the kind of
/// mismatch a client rebuilds its state machine around.
fn visible_projects(state: &ServerState, global: &Database) -> ApiResult<Vec<crate::db::Project>> {
    let all = global
        .get_all_projects()
        .map_err(|e| ApiError::Internal(format!("listing projects: {e}")))?;
    Ok(match &state.mode {
        ServeMode::Project(path) => all
            .into_iter()
            .filter(|p| std::path::Path::new(&p.path) == path)
            .collect(),
        ServeMode::Global => all,
    })
}

// ── /api/projects/:pid/tasks ────────────────────────────────────────────

#[derive(Serialize)]
struct TaskCard {
    id: String,
    title: String,
    status: String,
    agent: String,
    plugin: Option<String>,
    cycle: i32,
    branch_name: Option<String>,
    pr_number: Option<i32>,
    escalation_note: Option<String>,
    updated_at: String,
    deps_satisfied: bool,
    allowed_actions: Vec<String>,
    /// The TUI's published phase status, or `None` when nothing has been
    /// observed for this task. A client must weigh it against `phase_age_secs`
    /// rather than treating it as live — with no TUI running, nothing refreshes
    /// it.
    phase_status: Option<String>,
    phase_age_secs: Option<i64>,
}

fn card(db: &Database, t: Task, runtime: Option<&crate::db::TaskRuntime>) -> TaskCard {
    let deps_ok = db.deps_satisfied(&t);
    TaskCard {
        allowed_actions: allowed_actions(&t, deps_ok, CallerKind::Human),
        deps_satisfied: deps_ok,
        phase_status: runtime.map(|r| r.phase_status.as_str().to_string()),
        phase_age_secs: runtime.map(|r| (chrono::Utc::now() - r.updated_at).num_seconds()),
        id: t.id,
        title: t.title,
        status: t.status.as_str().to_string(),
        agent: t.agent,
        plugin: t.plugin,
        cycle: t.cycle,
        branch_name: t.branch_name,
        pr_number: t.pr_number,
        escalation_note: t.escalation_note,
        updated_at: t.updated_at.to_rfc3339(),
    }
}

async fn tasks(
    State(state): State<Arc<ServerState>>,
    Path(pid): Path<String>,
) -> ApiResult<Json<Vec<TaskCard>>> {
    let db = state.project_db(&pid)?;
    let all = db
        .get_all_tasks()
        .map_err(|e| ApiError::Internal(format!("listing tasks: {e}")))?;

    // One query for the whole board rather than one per card: the board is the
    // request a phone makes most, and it is the one that grows with the project.
    let runtime = db.list_task_runtime().unwrap_or_default();

    Ok(Json(
        all.into_iter()
            .map(|t| {
                let rt = runtime.iter().find(|r| r.task_id == t.id);
                card(&db, t, rt)
            })
            .collect(),
    ))
}

// ── /api/projects/:pid/tasks/:tid ───────────────────────────────────────

#[derive(Serialize)]
struct TaskDetail {
    #[serde(flatten)]
    card: TaskCard,
    description: Option<String>,
    worktree_path: Option<String>,
    session_name: Option<String>,
    pr_url: Option<String>,
    base_branch: Option<String>,
    referenced_tasks: Option<String>,
    created_at: String,
    /// The agent's own hook-reported state, read from its status file. Distinct
    /// from `phase_status`, which is the TUI's verdict: this one is available
    /// cross-process precisely because it is a file rather than TUI state.
    agent_state: Option<String>,
    blocked_reason: Option<String>,
}

async fn task_detail(
    State(state): State<Arc<ServerState>>,
    Path((pid, tid)): Path<(String, String)>,
) -> ApiResult<Json<TaskDetail>> {
    let db = state.project_db(&pid)?;
    let t = load_task(&db, &tid)?;
    let runtime = db.get_task_runtime(&tid).ok().flatten();

    let hook = t.worktree_path.as_ref().and_then(|wt| {
        crate::agent::hook_status::read_status(
            std::path::Path::new(wt),
            &t.id,
            chrono::Utc::now().timestamp(),
        )
    });

    let description = t.description.clone();
    let worktree_path = t.worktree_path.clone();
    let session_name = t.session_name.clone();
    let pr_url = t.pr_url.clone();
    let base_branch = t.base_branch.clone();
    let referenced_tasks = t.referenced_tasks.clone();
    let created_at = t.created_at.to_rfc3339();

    Ok(Json(TaskDetail {
        card: card(&db, t, runtime.as_ref()),
        description,
        worktree_path,
        session_name,
        pr_url,
        base_branch,
        referenced_tasks,
        created_at,
        agent_state: hook.as_ref().map(|h| {
            match h.state {
                crate::agent::hook_status::HookState::Working => "working",
                crate::agent::hook_status::HookState::Blocked => "blocked",
                crate::agent::hook_status::HookState::Waiting => "waiting",
                crate::agent::hook_status::HookState::Ended => "ended",
            }
            .to_string()
        }),
        blocked_reason: hook.and_then(|h| h.message),
    }))
}

// ── /api/projects/:pid/tasks/:tid/diff ──────────────────────────────────

#[derive(Serialize)]
struct DiffResponse {
    base: String,
    stat: String,
    patch: String,
}

async fn task_diff(
    State(state): State<Arc<ServerState>>,
    Path((pid, tid)): Path<(String, String)>,
) -> ApiResult<Json<DiffResponse>> {
    let project = state.project_path(&pid)?;
    let db = state.project_db(&pid)?;
    let t = load_task(&db, &tid)?;

    let worktree = t.worktree_path.clone().ok_or_else(|| {
        ApiError::NotFound(format!("task {tid} has no worktree, so there is no diff"))
    })?;
    let worktree = std::path::PathBuf::from(worktree);
    if !worktree.exists() {
        return Err(ApiError::NotFound(format!(
            "worktree for task {tid} no longer exists"
        )));
    }

    // The task's own base branch when it has one — a task cut from a release
    // branch diffed against `main` shows every unrelated commit as its work.
    //
    // But the recorded base has to be *checked*, not trusted. `diff_stat` only
    // fails when git cannot be spawned, so a base that no longer resolves comes
    // back as an empty diff, and the UI renders "no changes" for a task whose
    // work is entirely intact. Falling back to detection and reporting which
    // base was actually used keeps the answer honest — the client reads `base`.
    let base = t
        .base_branch
        .clone()
        .filter(|b| !b.is_empty() && crate::git::ref_exists(&worktree, b))
        .or_else(|| {
            crate::git::detect_main_branch(&project)
                .ok()
                .filter(|b| crate::git::ref_exists(&worktree, b))
        })
        .ok_or_else(|| {
            ApiError::NotFound(format!(
                "no base branch to diff against: task {tid} records {:?}, and neither it nor a \
                 detected default resolves in {}",
                t.base_branch.as_deref().unwrap_or("none"),
                worktree.display()
            ))
        })?;

    // `HEAD` rather than the branch name: the branch is what the worktree has
    // checked out, and naming it would miss nothing but costs a lookup.
    let stat = crate::git::diff_stat(&worktree, &base, "HEAD")
        .map_err(|e| ApiError::Internal(format!("git diff --stat: {e}")))?;
    let patch = crate::git::diff_full(&worktree, &base, "HEAD")
        .map_err(|e| ApiError::Internal(format!("git diff: {e}")))?;

    Ok(Json(DiffResponse { base, stat, patch }))
}

// ── /api/projects/:pid/tasks/:tid/pane ──────────────────────────────────

#[derive(Deserialize)]
struct PaneQuery {
    lines: Option<i32>,
}

#[derive(Serialize)]
struct PaneResponse {
    session_name: String,
    lines: i32,
    content: String,
}

async fn task_pane(
    State(state): State<Arc<ServerState>>,
    Path((pid, tid)): Path<(String, String)>,
    Query(q): Query<PaneQuery>,
) -> ApiResult<Json<PaneResponse>> {
    let db = state.project_db(&pid)?;
    let t = load_task(&db, &tid)?;

    let session_name = t
        .session_name
        .clone()
        .ok_or_else(|| ApiError::NotFound(format!("task {tid} has no active session")))?;

    let lines = q.lines.unwrap_or(100).clamp(1, 10_000);
    let content = capture_pane(&session_name, lines)
        .ok_or_else(|| ApiError::Internal("tmux capture-pane failed".to_string()))?;

    Ok(Json(PaneResponse {
        session_name,
        lines,
        content,
    }))
}

/// A one-shot capture, for a client with no WebSocket.
///
/// Plain text, unlike the live view: a caller that asked for JSON did not ask
/// for SGR escapes in the middle of it. `/ws` is where the coloured frames go.
///
/// Goes through `TmuxOperations` rather than its own `tmux` invocation so the
/// socket name lives in exactly one place — `RealTmuxOps` hard-codes the `agtx`
/// server, and a second spelling here would be a second thing to update.
fn capture_pane(session_name: &str, lines: i32) -> Option<String> {
    let ops = crate::tmux::RealTmuxOps;
    if !crate::tmux::TmuxOperations::window_exists(&ops, session_name).unwrap_or(false) {
        return None;
    }
    let bytes = crate::tmux::TmuxOperations::capture_pane_with_history(&ops, session_name, lines);
    let text = String::from_utf8_lossy(&bytes);
    Some(strip_sgr(&text))
}

/// Remove SGR sequences, leaving the text a JSON caller expects.
fn strip_sgr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            // Consume up to and including the final byte of the sequence.
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        }
    }
    out
}

fn load_task(db: &Database, tid: &str) -> ApiResult<Task> {
    match db.get_task(tid) {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Err(ApiError::NotFound(format!("task {tid}"))),
        Err(e) => Err(ApiError::Internal(format!("loading task: {e}"))),
    }
}

/// Re-exported so the phase vocabulary is compiled against, not copied: a
/// client reads these strings and `PhaseStatus` is where they are defined.
#[allow(dead_code)]
fn phase_vocabulary() -> [&'static str; 5] {
    [
        PhaseStatus::Working.as_str(),
        PhaseStatus::Blocked.as_str(),
        PhaseStatus::Idle.as_str(),
        PhaseStatus::Ready.as_str(),
        PhaseStatus::Exited.as_str(),
    ]
}
