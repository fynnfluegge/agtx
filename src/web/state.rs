//! What a request handler is allowed to reach.

use std::collections::{HashMap, VecDeque};
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

/// Whether a task's branch still merges cleanly into its base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictState {
    pub conflicted: bool,
    pub files: Vec<String>,
}

/// Merge-conflict results, cached because computing one is a `git merge-tree`
/// subprocess.
///
/// The board is polled every two seconds and a project can hold several Review
/// tasks, so checking on the request path would put N git invocations on every
/// poll — the same cost decision 3 refuses for phase status, and for the same
/// reason. Instead the board serves whatever is cached (`None` the first time)
/// and a background pass fills it in, so the chip appears a poll later and the
/// board never waits.
#[derive(Default)]
pub struct ConflictCache {
    entries: Mutex<HashMap<String, (Instant, ConflictState)>>,
    /// One refresh at a time. Without this every poll would spawn another pass
    /// over the same tasks while the first was still running.
    refreshing: Mutex<bool>,
}

/// How long a conflict answer is trusted.
///
/// Generous: it changes only when someone commits to the branch or moves the
/// base, neither of which happens between two board polls.
const CONFLICT_TTL: Duration = Duration::from_secs(30);

impl ConflictCache {
    pub fn get(&self, task_id: &str) -> Option<ConflictState> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.get(task_id).map(|(_, state)| state.clone())
    }

    pub fn is_stale(&self, task_id: &str) -> bool {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        match entries.get(task_id) {
            Some((at, _)) => at.elapsed() > CONFLICT_TTL,
            None => true,
        }
    }

    pub fn put(&self, task_id: String, state: ConflictState) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.insert(task_id, (Instant::now(), state));
    }

    /// Claim the right to run a refresh pass; `false` means one is already
    /// going. The guard releases it on drop, including on panic.
    pub fn try_claim(&self) -> Option<RefreshGuard<'_>> {
        let mut flag = self.refreshing.lock().unwrap_or_else(|e| e.into_inner());
        if *flag {
            return None;
        }
        *flag = true;
        Some(RefreshGuard { cache: self })
    }
}

pub struct RefreshGuard<'a> {
    cache: &'a ConflictCache,
}

impl Drop for RefreshGuard<'_> {
    fn drop(&mut self) {
        let mut flag = self
            .cache
            .refreshing
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *flag = false;
    }
}

pub struct ServerState {
    pub mode: ServeMode,
    /// Whether `/api/*` and `/ws` require a paired device.
    ///
    /// Off for a loopback bind, which is its own boundary: reaching it already
    /// means being on this machine. On for anything wider, where it is the only
    /// thing standing between the network and an agent's composer.
    pub require_auth: bool,
    /// Codes waiting to be exchanged for a device token.
    pub pairing: super::auth::PairingCodes,
    /// Device ids seen recently, so `last_seen` is not written on every poll.
    seen: Mutex<HashMap<String, Instant>>,
    /// Projects whose board was recently asked for, so the watch marker is not
    /// rewritten on every request.
    watched: Mutex<HashMap<String, Instant>>,
    /// How long a TUI heartbeat stays trusted. Three beats of
    /// `TRANSITION_POLL_INTERVAL`, so one missed tick is not a disconnect.
    pub heartbeat_ttl: chrono::Duration,
    /// Ceiling on writes — actions and task CRUD together.
    pub writes: RateLimiter,
    pub conflicts: ConflictCache,
}

impl ServerState {
    pub fn new(mode: ServeMode) -> Arc<Self> {
        Self::with_auth(mode, false)
    }

    pub fn with_auth(mode: ServeMode, require_auth: bool) -> Arc<Self> {
        Arc::new(Self {
            mode,
            require_auth,
            pairing: super::auth::PairingCodes::default(),
            seen: Mutex::new(HashMap::new()),
            watched: Mutex::new(HashMap::new()),
            heartbeat_ttl: chrono::Duration::seconds(6),
            // Generous for a person tapping, ruinous only for a loop.
            writes: RateLimiter::new(120, Duration::from_secs(60)),
            conflicts: ConflictCache::default(),
        })
    }

    /// Whether `presented` belongs to a paired device.
    ///
    /// The lookup is by hash, so the token itself is never read back out of the
    /// database to be compared — and a hash lookup is constant-time in the way
    /// that matters here, since the secret is not part of the comparison.
    pub fn token_ok(&self, presented: Option<&str>) -> bool {
        if !self.require_auth {
            return true;
        }
        let Some(token) = presented else {
            return false;
        };
        match super::auth::device_for_token(token) {
            Some(device) => {
                self.touch(&device.id);
                true
            }
            None => false,
        }
    }

    /// Record a device as active, at most once a minute.
    ///
    /// `last_seen` exists so a person can recognise which row is which phone
    /// when revoking. That does not need second-level accuracy, and writing on
    /// every request would mean a database write every two seconds per polling
    /// phone.
    fn touch(&self, device_id: &str) {
        const TOUCH_INTERVAL: Duration = Duration::from_secs(60);
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        let fresh = seen
            .get(device_id)
            .is_some_and(|at| at.elapsed() < TOUCH_INTERVAL);
        if fresh {
            return;
        }
        seen.insert(device_id.to_string(), Instant::now());
        if let Ok(db) = Database::open_global() {
            let _ = db.touch_mobile_device(device_id);
        }
    }

    /// Mark this project's board as being looked at.
    ///
    /// This is what lets the TUI publish `task_runtime` only while someone is
    /// actually reading it — a board nobody has opened costs no writes at all.
    /// Throttled, because it answers a question whose useful resolution is
    /// minutes and it is on the path of the request a phone makes most.
    pub fn note_board_watched(&self, project_path: &str) {
        const NOTE_INTERVAL: Duration = Duration::from_secs(30);
        let mut watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
        let fresh = watched
            .get(project_path)
            .is_some_and(|at| at.elapsed() < NOTE_INTERVAL);
        if fresh {
            return;
        }
        watched.insert(project_path.to_string(), Instant::now());
        if let Ok(db) = Database::open_global() {
            let _ = db.note_board_watched(project_path);
        }
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
