//! What a request handler is allowed to reach.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::db::Database;

/// How the server resolves which project a request is about.
///
/// Mirrors [`crate::mcp::ServerMode`] deliberately: the two servers answer the
/// same questions about the same board, and a phone that has to think about
/// projects differently from the orchestrator is a second model to keep true.
#[derive(Debug, Clone)]
pub enum ServeMode {
    /// One project, fixed at startup. `project_id` in a path is checked against
    /// it rather than used to look anything up.
    Project(PathBuf),
    /// Every project in the global index; `project_id` selects.
    Global,
}

/// A task lost between listing and reading it is a 404, not a 500; a database
/// that will not open is the reverse. Handlers return this so the mapping is
/// made once, in [`super::routes`], rather than guessed per endpoint.
#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    Internal(String),
    BadRequest(String),
    /// The request was well-formed and the task exists, but the board does not
    /// permit this right now — a Backlog task whose dependencies have not
    /// cleared, say. Distinct from `BadRequest` because retrying later can
    /// succeed, and a client should say "not yet" rather than "you broke it".
    Conflict(String),
    TooManyRequests(String),
}

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, message) = match self {
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            ApiError::TooManyRequests(m) => (StatusCode::TOO_MANY_REQUESTS, m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (code, Json(ErrorBody { error: message })).into_response()
    }
}

/// A fixed-size sliding window over recent writes.
///
/// Not an anti-abuse measure — there is nothing to abuse on loopback with one
/// user. It is a ceiling on a *runaway client*: an optimistic UI that retries a
/// failing action in a render loop would otherwise queue thousands of
/// transition requests for the TUI to work through. It has to exist before
/// pairing opens this to a network, and it is cheaper to have it from the
/// start than to remember later.
pub struct RateLimiter {
    hits: Mutex<VecDeque<Instant>>,
    max: usize,
    window: Duration,
}

impl RateLimiter {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            hits: Mutex::new(VecDeque::new()),
            max,
            window,
        }
    }

    /// Record an attempt; `false` means the caller is over the limit.
    pub fn allow(&self) -> bool {
        // A poisoned lock holds only timestamps, so recovering loses nothing
        // and refusing every later write would be a worse failure than the
        // panic that poisoned it.
        let mut hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        while hits
            .front()
            .is_some_and(|t| now.duration_since(*t) > self.window)
        {
            hits.pop_front();
        }
        if hits.len() >= self.max {
            return false;
        }
        hits.push_back(now);
        true
    }
}

pub struct ServerState {
    pub mode: ServeMode,
    /// How long a TUI heartbeat stays trusted. Three beats of
    /// `TRANSITION_POLL_INTERVAL`, so one missed tick is not a disconnect.
    pub heartbeat_ttl: chrono::Duration,
    /// Ceiling on writes — actions and task CRUD together.
    pub writes: RateLimiter,
}

impl ServerState {
    pub fn new(mode: ServeMode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            heartbeat_ttl: chrono::Duration::seconds(6),
            // Generous for a person tapping, ruinous only for a loop.
            writes: RateLimiter::new(120, Duration::from_secs(60)),
        })
    }

    pub fn check_write_budget(&self) -> ApiResult<()> {
        if self.writes.allow() {
            Ok(())
        } else {
            Err(ApiError::TooManyRequests(
                "too many writes in the last minute; slow down".to_string(),
            ))
        }
    }

    /// Resolve a `project_id` from the path to a project directory.
    ///
    /// In `Project` mode the id is still checked rather than ignored: a phone
    /// holding a stale bookmark for another project must get a 404 instead of
    /// silently being served this one's board.
    pub fn project_path(&self, project_id: &str) -> ApiResult<PathBuf> {
        match &self.mode {
            ServeMode::Project(path) => {
                let global = Database::open_global()
                    .map_err(|e| ApiError::Internal(format!("global database: {e}")))?;
                match global.get_project_by_id(project_id) {
                    Ok(Some(p)) if std::path::Path::new(&p.path) == path => Ok(path.clone()),
                    // Unknown to the index but this server serves exactly one
                    // project — accept the id only if it is that project's.
                    Ok(_) => Err(ApiError::NotFound(format!(
                        "project {project_id} is not the project this server was started on"
                    ))),
                    Err(e) => Err(ApiError::Internal(format!("project lookup: {e}"))),
                }
            }
            ServeMode::Global => {
                let global = Database::open_global()
                    .map_err(|e| ApiError::Internal(format!("global database: {e}")))?;
                match global.get_project_by_id(project_id) {
                    Ok(Some(p)) => Ok(PathBuf::from(p.path)),
                    Ok(None) => Err(ApiError::NotFound(format!("project {project_id}"))),
                    Err(e) => Err(ApiError::Internal(format!("project lookup: {e}"))),
                }
            }
        }
    }

    pub fn project_db(&self, project_id: &str) -> ApiResult<Database> {
        let path = self.project_path(project_id)?;
        Database::open_project(&path)
            .map_err(|e| ApiError::Internal(format!("project database: {e}")))
    }
}
