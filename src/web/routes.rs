//! The read-only half of the HTTP surface.
//!
//! Every handler here reads: SQLite, git and tmux, the same three things the
//! MCP server talks to. Nothing in this module links `App` or mutates the
//! board — writes arrive in Step 3 of the mobile plan.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, Request, State},
    http::{header, StatusCode, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::core::actions::{allowed_actions, CallerKind};
use crate::db::{Database, PhaseStatus, Task};

use super::state::{ApiError, ApiResult, ConflictState, ServeMode, ServerState};

/// Reject an unauthenticated `/api/*` or `/ws` request.
///
/// Layered over the API routes only. The static assets stay open because a
/// browser cannot put a header on the initial page load, and gating them would
/// mean carrying the token in the URL of every navigation — see
/// [`super::auth`]. They are this app's own JS and CSS and expose no board.
async fn require_token(
    State(state): State<Arc<ServerState>>,
    req: Request,
    next: Next,
) -> Response {
    // Two places a token can be, because a browser cannot put `Authorization`
    // on a WebSocket handshake — the subprotocol is the one field it *can* set
    // there. Both are checked here rather than one here and one in the socket
    // handler: `WebSocketUpgrade` is an extractor, so a handler-side check runs
    // only after the upgrade has been accepted, and an exemption in this layer
    // to let that happen is an open socket for as long as anyone forgets why.
    let headers = req.headers();
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| {
            headers
                .get(header::SEC_WEBSOCKET_PROTOCOL)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| {
                    v.split(',')
                        .map(str::trim)
                        .find_map(|p| p.strip_prefix(super::auth::WS_TOKEN_PREFIX))
                })
        });

    if state.token_ok(presented) {
        next.run(req).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "missing or wrong token. Open the URL agtx printed at startup, \
                          which carries it in the fragment."
            })),
        )
            .into_response()
    }
}

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
        // Inside the auth layer, unlike the static assets: the token check has
        // to happen before the upgrade is accepted.
        .route("/ws", get(super::ws::handler))
        .route(
            "/api/projects/{pid}/tasks/{tid}",
            patch(super::writes::update_task).delete(super::writes::delete_task),
        )
        // Pairing is the one unauthenticated route — it is how a device gets a
        // credential in the first place — so it is registered *outside* the
        // auth layer, alongside the static shell.
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .route("/api/pair", post(super::writes::pair))
        .with_state(state)
        // The PWA, last: `/api/*` is matched first, so an asset can never
        // shadow a route, and an unknown `/api/...` path still 404s as HTML —
        // and the fallback sits *outside* the auth layer, so the shell loads
        // before the page has a token to present.
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
    /// Whether this branch still merges cleanly into its base. `None` means
    /// "not checked yet" — a Review task's first appearance, before the
    /// background pass has run — and a client must render that as unknown
    /// rather than as clean, which is the answer someone might merge on.
    conflicted: Option<bool>,
    conflicting_files: Vec<String>,
}

fn card(
    db: &Database,
    t: Task,
    runtime: Option<&crate::db::TaskRuntime>,
    conflict: Option<ConflictState>,
) -> TaskCard {
    let deps_ok = db.deps_satisfied(&t);
    TaskCard {
        conflicted: conflict.as_ref().map(|c| c.conflicted),
        conflicting_files: conflict.map(|c| c.files).unwrap_or_default(),
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
    // Tell the TUI someone is reading, so it starts publishing phase status.
    // Without a reader it publishes nothing, which is the point.
    if let Ok(path) = state.project_path(&pid) {
        state.note_board_watched(&path.to_string_lossy());
    }
    let all = db
        .get_all_tasks()
        .map_err(|e| ApiError::Internal(format!("listing tasks: {e}")))?;

    // One query for the whole board rather than one per card: the board is the
    // request a phone makes most, and it is the one that grows with the project.
    let runtime = db.list_task_runtime().unwrap_or_default();

    // Kick off a conflict pass for anything in Review whose answer is missing
    // or stale. It runs *after* this response: the board never waits on git.
    spawn_conflict_refresh(state.clone(), &pid, &all);

    Ok(Json(
        all.into_iter()
            .map(|t| {
                let rt = runtime.iter().find(|r| r.task_id == t.id);
                let conflict = state.conflicts.get(&t.id);
                card(&db, t, rt, conflict)
            })
            .collect(),
    ))
}

/// Recompute merge-conflict state for Review tasks, off the request path.
///
/// Only Review tasks: a branch that is still being written to will conflict or
/// not many times before anyone cares, and the question is only meaningful once
/// the work is up for merging.
fn spawn_conflict_refresh(state: Arc<ServerState>, pid: &str, tasks: &[Task]) {
    let stale: Vec<Task> = tasks
        .iter()
        .filter(|t| t.status == crate::db::TaskStatus::Review)
        .filter(|t| t.worktree_path.is_some() && state.conflicts.is_stale(&t.id))
        .cloned()
        .collect();
    if stale.is_empty() {
        return;
    }

    let Ok(project) = state.project_path(pid) else {
        return;
    };

    tokio::task::spawn_blocking(move || {
        // One pass at a time. A poll every two seconds would otherwise stack
        // passes over the same tasks faster than git can answer.
        let Some(_guard) = state.conflicts.try_claim() else {
            return;
        };
        for task in stale {
            if let Some(found) = conflict_for(&project, &task) {
                state.conflicts.put(task.id.clone(), found);
            }
        }
    });
}

/// The base branch to compare against, verified to exist.
///
/// The recorded base has to be *checked*, not trusted: `git diff` and
/// `git merge-tree` only fail when git cannot be **spawned**, so a base that no
/// longer resolves comes back as an empty diff and no conflicts — which reads
/// as "nothing to do" for a task whose work is entirely intact. Falling back to
/// detection and reporting which base was used keeps the answer honest.
fn resolve_base(
    project: &std::path::Path,
    worktree: &std::path::Path,
    recorded: Option<&str>,
) -> Option<String> {
    recorded
        .filter(|b| !b.is_empty() && crate::git::ref_exists(worktree, b))
        .map(str::to_string)
        .or_else(|| {
            crate::git::detect_main_branch(project)
                .ok()
                .filter(|b| crate::git::ref_exists(worktree, b))
        })
}

/// `None` when the question cannot be asked — no worktree, no branch, or a base
/// that does not resolve. Distinct from "no conflicts", which is an answer.
fn conflict_for(project: &std::path::Path, task: &Task) -> Option<ConflictState> {
    let worktree = std::path::PathBuf::from(task.worktree_path.as_ref()?);
    if !worktree.exists() {
        return None;
    }
    let branch = task.branch_name.clone()?;
    let base = resolve_base(project, &worktree, task.base_branch.as_deref())?;

    match crate::git::check_merge_conflicts(&worktree, &base, &branch) {
        Ok((conflicted, files)) => Some(ConflictState { conflicted, files }),
        Err(_) => None,
    }
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

    // On the detail screen the answer is worth waiting for: one task, and this
    // is the screen someone is on when deciding whether to merge. The board
    // takes the cached value instead, because it asks about every card at once.
    let conflict = if t.status == crate::db::TaskStatus::Review {
        match state.conflicts.get(&tid) {
            Some(cached) if !state.conflicts.is_stale(&tid) => Some(cached),
            _ => {
                let project = state.project_path(&pid)?;
                let task = t.clone();
                let found = tokio::task::spawn_blocking(move || conflict_for(&project, &task))
                    .await
                    .ok()
                    .flatten();
                if let Some(ref c) = found {
                    state.conflicts.put(tid.clone(), c.clone());
                }
                found
            }
        }
    } else {
        None
    };

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
        card: card(&db, t, runtime.as_ref(), conflict),
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
    /// `None` when the check could not be made. The diff screen is where
    /// someone decides to merge, so "unknown" and "clean" must not look alike.
    conflicted: Option<bool>,
    conflicting_files: Vec<String>,
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

    let base = resolve_base(&project, &worktree, t.base_branch.as_deref()).ok_or_else(|| {
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

    // Computed here rather than read from the cache: this is the screen the
    // decision gets made on, and one git call is worth an honest answer.
    let conflict = {
        let project = project.clone();
        let task = t.clone();
        tokio::task::spawn_blocking(move || conflict_for(&project, &task))
            .await
            .ok()
            .flatten()
    };
    if let Some(ref c) = conflict {
        state.conflicts.put(tid.clone(), c.clone());
    }

    Ok(Json(DiffResponse {
        base,
        stat,
        patch,
        conflicted: conflict.as_ref().map(|c| c.conflicted),
        conflicting_files: conflict.map(|c| c.files).unwrap_or_default(),
    }))
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
pub fn strip_sgr(text: &str) -> String {
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
