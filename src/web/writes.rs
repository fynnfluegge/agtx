//! The write half of the HTTP surface.
//!
//! **Nothing here executes a transition.** An action becomes a row in
//! `transition_requests`, which only a running TUI drains — the mobile plan's
//! "one hard constraint". So every action response says `queued`, never `done`,
//! and carries the request id the client polls. Reporting otherwise would give
//! a phone a board that looks responsive while the laptop lid is shut.
//!
//! Task CRUD is the exception: create, edit and delete write the task table
//! directly, because they need no worktree, no agent and no tmux. They are
//! restricted to Backlog for the same reason the MCP tools are — editing a task
//! whose agent is mid-phase would change the work under it.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::core::actions::{validate_action, ActionRefusal, CallerKind, MAX_TASK_TITLE_CHARS};
use crate::core::input::{agent_needs_paste, send_user_key, send_user_text, PaneKey};
use crate::db::{Database, Task, TaskStatus, TransitionRequest};
use crate::web::auth::PairError;

use super::state::{ApiError, ApiResult, ServerState};

// ── POST /api/projects/:pid/tasks/:tid/action ───────────────────────────

#[derive(Deserialize)]
pub struct ActionRequest {
    pub action: String,
    pub reason: Option<String>,
}

#[derive(Serialize)]
pub struct ActionResponse {
    request_id: String,
    /// Always `queued`. The board moves when a TUI picks the request up, which
    /// is what `GET .../requests/{id}` reports.
    status: &'static str,
    /// Whether anything is currently draining the queue. A client uses this to
    /// decide between "working…" and "queued — no agtx running", rather than
    /// leaving a spinner up forever.
    tui_connected: bool,
}

pub async fn task_action(
    State(state): State<Arc<ServerState>>,
    Path((pid, tid)): Path<(String, String)>,
    Json(body): Json<ActionRequest>,
) -> ApiResult<Json<ActionResponse>> {
    state.check_write_budget()?;

    let path = state.project_path(&pid)?;
    let db = state.project_db(&pid)?;
    let task = load_task(&db, &tid)?;

    // The same policy the board rendered its buttons from, checked again here.
    // A client is not the authority on what it may do: its copy of the task can
    // be seconds old, and the phase it was showing may already have moved.
    validate_action(
        &task,
        db.deps_satisfied(&task),
        CallerKind::Human,
        &body.action,
    )
    .map_err(|refusal| match refusal {
        ActionRefusal::UnknownAction(m) => ApiError::BadRequest(m),
        ActionRefusal::NotPermitted(m) => ApiError::Conflict(m),
    })?;

    let mut req = TransitionRequest::new(&tid, &body.action);
    req.reason = body.reason.clone();
    let request_id = req.id.clone();
    db.create_transition_request(&req)
        .map_err(|e| ApiError::Internal(format!("queueing the transition: {e}")))?;

    tracing::info!(task = %tid, action = %body.action, request = %request_id, "queued from web");

    let global =
        Database::open_global().map_err(|e| ApiError::Internal(format!("global database: {e}")))?;
    Ok(Json(ActionResponse {
        request_id,
        status: "queued",
        tui_connected: global
            .tui_is_live(&path.to_string_lossy(), state.heartbeat_ttl)
            .unwrap_or(false),
    }))
}

// ── GET /api/projects/:pid/requests/:rid ────────────────────────────────

#[derive(Serialize)]
pub struct RequestStatus {
    request_id: String,
    /// `pending`, `completed` or `error` — the same three words
    /// `get_transition_status` reports, so a client reading both agrees.
    status: &'static str,
    error: Option<String>,
}

/// Note the shape: `/api/projects/{pid}/requests/{rid}`, not `/api/requests/{rid}`.
///
/// `transition_requests` lives in the *project* database, so a request id alone
/// does not identify a row — the plan's original route had no way to find one
/// without searching every indexed project.
pub async fn request_status(
    State(state): State<Arc<ServerState>>,
    Path((pid, rid)): Path<(String, String)>,
) -> ApiResult<Json<RequestStatus>> {
    let db = state.project_db(&pid)?;
    match db.get_transition_request(&rid) {
        Ok(Some(req)) => Ok(Json(RequestStatus {
            request_id: req.id,
            status: match (&req.processed_at, &req.error) {
                (Some(_), Some(_)) => "error",
                (Some(_), None) => "completed",
                (None, _) => "pending",
            },
            error: req.error,
        })),
        // Requests are reaped an hour after processing, so a client polling an
        // old id gets a 404 rather than a wrong answer.
        Ok(None) => Err(ApiError::NotFound(format!(
            "no transition request {rid}; it may have completed and been cleaned up"
        ))),
        Err(e) => Err(ApiError::Internal(format!("reading the request: {e}"))),
    }
}

// ── task CRUD (Backlog only) ────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateTask {
    pub title: String,
    pub description: Option<String>,
    pub plugin: Option<String>,
    pub referenced_tasks: Option<String>,
    pub base_branch: Option<String>,
}

#[derive(Serialize)]
pub struct CreatedTask {
    id: String,
    title: String,
    status: String,
}

pub async fn create_task(
    State(state): State<Arc<ServerState>>,
    Path(pid): Path<String>,
    Json(body): Json<CreateTask>,
) -> ApiResult<Json<CreatedTask>> {
    state.check_write_budget()?;

    let db = state.project_db(&pid)?;
    let title = validate_title(&body.title)?;
    validate_refs(&db, body.referenced_tasks.as_deref())?;

    // The project's own defaults, so a task created from a phone is the task
    // the desktop would have created — the agent in particular is what every
    // later phase reads.
    let (agent, plugin) = defaults_for(&state, &pid);
    let project_name = project_name_for(&state, &pid);

    let mut task = Task::new(title, &agent, &project_name);
    task.description = body.description;
    task.plugin = body.plugin.or(plugin);
    task.referenced_tasks = body.referenced_tasks;
    task.base_branch = body.base_branch;

    db.create_task(&task)
        .map_err(|e| ApiError::Internal(format!("creating the task: {e}")))?;
    tracing::info!(task = %task.id, "created from web");

    Ok(Json(CreatedTask {
        id: task.id,
        title: task.title,
        status: task.status.as_str().to_string(),
    }))
}

#[derive(Deserialize)]
pub struct UpdateTask {
    pub title: Option<String>,
    pub description: Option<String>,
    pub plugin: Option<String>,
    pub referenced_tasks: Option<String>,
    pub base_branch: Option<String>,
}

pub async fn update_task(
    State(state): State<Arc<ServerState>>,
    Path((pid, tid)): Path<(String, String)>,
    Json(body): Json<UpdateTask>,
) -> ApiResult<Json<CreatedTask>> {
    state.check_write_budget()?;

    let db = state.project_db(&pid)?;
    let mut task = load_task(&db, &tid)?;
    backlog_only(&task, "edit")?;

    if let Some(title) = body.title {
        task.title = validate_title(&title)?.to_string();
    }
    if let Some(description) = body.description {
        task.description = Some(description);
    }
    if let Some(plugin) = body.plugin {
        task.plugin = Some(plugin);
    }
    if let Some(refs) = body.referenced_tasks {
        validate_refs(&db, Some(&refs))?;
        task.referenced_tasks = Some(refs);
    }
    if let Some(base) = body.base_branch {
        task.base_branch = Some(base);
    }

    db.update_task(&task)
        .map_err(|e| ApiError::Internal(format!("updating the task: {e}")))?;

    Ok(Json(CreatedTask {
        id: task.id,
        title: task.title,
        status: task.status.as_str().to_string(),
    }))
}

#[derive(Serialize)]
pub struct DeletedTask {
    id: String,
    title: String,
}

pub async fn delete_task(
    State(state): State<Arc<ServerState>>,
    Path((pid, tid)): Path<(String, String)>,
) -> ApiResult<Json<DeletedTask>> {
    state.check_write_budget()?;

    let db = state.project_db(&pid)?;
    let task = load_task(&db, &tid)?;
    backlog_only(&task, "delete")?;

    db.delete_task(&tid)
        .map_err(|e| ApiError::Internal(format!("deleting the task: {e}")))?;
    tracing::info!(task = %tid, "deleted from web");

    Ok(Json(DeletedTask {
        id: task.id,
        title: task.title,
    }))
}

// ── shared checks ───────────────────────────────────────────────────────

fn load_task(db: &Database, tid: &str) -> ApiResult<Task> {
    match db.get_task(tid) {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Err(ApiError::NotFound(format!("task {tid}"))),
        Err(e) => Err(ApiError::Internal(format!("loading task: {e}"))),
    }
}

/// Editing or deleting a task that has left Backlog would change the work under
/// a running agent, so both are refused past that point — the same rule the MCP
/// tools enforce.
fn backlog_only(task: &Task, verb: &str) -> ApiResult<()> {
    if task.status == TaskStatus::Backlog {
        Ok(())
    } else {
        Err(ApiError::Conflict(format!(
            "can only {verb} a Backlog task; {:?} is in {}",
            task.title,
            task.status.as_str()
        )))
    }
}

fn validate_title(title: &str) -> ApiResult<&str> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err(ApiError::BadRequest("a task needs a title".to_string()));
    }
    let chars = trimmed.chars().count();
    if chars > MAX_TASK_TITLE_CHARS {
        return Err(ApiError::BadRequest(format!(
            "title is {chars} characters; the maximum is {MAX_TASK_TITLE_CHARS}"
        )));
    }
    Ok(trimmed)
}

/// A reference to a task that does not exist would silently become a dependency
/// nothing can satisfy, leaving the referring task unable to leave Backlog with
/// no visible cause.
fn validate_refs(db: &Database, refs: Option<&str>) -> ApiResult<()> {
    let Some(refs) = refs else { return Ok(()) };
    for id in refs.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match db.get_task(id) {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(ApiError::BadRequest(format!(
                    "referenced task {id} not found"
                )))
            }
            Err(e) => return Err(ApiError::Internal(format!("checking reference {id}: {e}"))),
        }
    }
    Ok(())
}

/// The project's configured default agent and plugin, merged over the global
/// config exactly as the TUI does.
fn defaults_for(state: &ServerState, pid: &str) -> (String, Option<String>) {
    let global = crate::config::GlobalConfig::load().unwrap_or_default();
    match state.project_path(pid) {
        Ok(path) => {
            let project = crate::config::ProjectConfig::load(&path).unwrap_or_default();
            let merged = crate::config::MergedConfig::merge(&global, &project);
            (merged.default_agent.clone(), merged.workflow_plugin.clone())
        }
        Err(_) => (global.default_agent.clone(), None),
    }
}

fn project_name_for(state: &ServerState, pid: &str) -> String {
    state
        .project_path(pid)
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| pid.to_string())
}

// ── POST /api/projects/:pid/tasks/:tid/input ────────────────────────────

/// Two fields, exactly one of which must be set.
///
/// Not one field the server disambiguates: `send-keys` without `-l` resolves an
/// argument matching a key name *as that key*, so a message of `"Space"` would
/// arrive as `0x20` and `"Escape"` as an actual Escape. The caller has to say
/// which it meant, and this is the shape that makes them say it.
#[derive(Deserialize)]
pub struct InputRequest {
    pub text: Option<String>,
    pub key: Option<String>,
}

#[derive(Serialize)]
pub struct InputResponse {
    delivered: bool,
}

/// The cap and null-byte rule from `send_to_task`, so MCP and HTTP refuse the
/// same messages.
const MAX_INPUT_BYTES: usize = 4096;

pub async fn task_input(
    State(state): State<Arc<ServerState>>,
    Path((pid, tid)): Path<(String, String)>,
    Json(body): Json<InputRequest>,
) -> ApiResult<Json<InputResponse>> {
    state.check_write_budget()?;

    let db = state.project_db(&pid)?;
    let task = load_task(&db, &tid)?;

    // Research keeps its task in Backlog; review and conflict resolution also
    // retain an agent. Every phase except Done can have an active session.
    if task.status == TaskStatus::Done {
        return Err(ApiError::Conflict(format!(
            "cannot send input to a completed task; {:?} is in {}",
            task.title,
            task.status.as_str()
        )));
    }
    let session = task
        .session_name
        .clone()
        .ok_or_else(|| ApiError::NotFound(format!("task {tid} has no active session")))?;

    let tmux_ops: Arc<dyn crate::tmux::TmuxOperations> = Arc::new(crate::tmux::RealTmuxOps);

    let delivered = match (body.text, body.key) {
        (Some(_), Some(_)) => {
            return Err(ApiError::BadRequest(
                "send either text or key, not both".to_string(),
            ))
        }
        (None, None) => {
            return Err(ApiError::BadRequest(
                "nothing to send: set text or key".to_string(),
            ))
        }
        (Some(text), None) => {
            if text.len() > MAX_INPUT_BYTES {
                return Err(ApiError::BadRequest(format!(
                    "message is {} bytes; the maximum is {MAX_INPUT_BYTES}",
                    text.len()
                )));
            }
            if text.contains('\0') {
                return Err(ApiError::BadRequest(
                    "message contains null bytes".to_string(),
                ));
            }
            // Blocking work — a paste plus `submit_message`'s bounded Enter
            // loop, which polls the pane. Running it on the async runtime would
            // stall every other request on this thread for up to a few seconds.
            let agent = task.agent.clone();
            tokio::task::spawn_blocking(move || {
                send_user_text(&tmux_ops, &session, &text, agent_needs_paste(&agent))
            })
            .await
            .map_err(|e| ApiError::Internal(format!("sending input: {e}")))?
        }
        (None, Some(name)) => {
            let key = PaneKey::parse(&name).ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "unsupported key {name:?}. A closed set is deliberate: forwarding arbitrary \
                     key names would let a request send C-d, which ends the agent's session."
                ))
            })?;
            tokio::task::spawn_blocking(move || send_user_key(&tmux_ops, &session, key))
                .await
                .map_err(|e| ApiError::Internal(format!("sending key: {e}")))?
        }
    };

    tracing::info!(task = %tid, delivered, "input from web");
    Ok(Json(InputResponse { delivered }))
}

// ── POST /api/pair ──────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PairRequest {
    pub code: String,
    /// What to call this device in the list. Free text, shown and never matched
    /// on, so it needs no validation beyond not being able to flood a display.
    pub label: Option<String>,
}

#[derive(Serialize)]
pub struct PairResponse {
    token: String,
    label: String,
}

/// Exchange a pairing code for a device token.
///
/// The one unauthenticated route, so everything that guards it lives here: the
/// code is single-use and short-lived, failures are counted toward a lockout,
/// and the write budget applies. The token is returned exactly once — only its
/// hash is stored — so a device that loses it pairs again rather than asking
/// for it back.
pub async fn pair(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<PairRequest>,
) -> ApiResult<Json<PairResponse>> {
    state.check_write_budget()?;

    // Nothing to pair *with* on a loopback bind: there is no auth to hold a
    // token against, so issuing one would imply a protection that is not there.
    if !state.require_auth {
        return Err(ApiError::Conflict(
            "this server is not requiring authentication, so there is nothing to pair".to_string(),
        ));
    }

    state
        .pairing
        .redeem(body.code.trim())
        .map_err(|e| match e {
            PairError::LockedOut => ApiError::TooManyRequests(e.message().to_string()),
            other => ApiError::BadRequest(other.message().to_string()),
        })?;

    let label = body.label.unwrap_or_default();
    let token = crate::web::auth::pair_device(&label)
        .map_err(|e| ApiError::Internal(format!("pairing: {e}")))?;

    let stored = crate::web::auth::device_for_token(&token)
        .map(|d| d.label)
        .unwrap_or_else(|| "phone".to_string());
    tracing::info!(label = %stored, "device paired");

    Ok(Json(PairResponse {
        token,
        label: stored,
    }))
}
