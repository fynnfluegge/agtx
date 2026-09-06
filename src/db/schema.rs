use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;

use super::models::{
    MobileDevice, Notification, NotificationKind, PhaseStatus, Project, Task, TaskRuntime,
    TaskStatus, TransitionRequest,
};

/// Database wrapper for SQLite operations
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Root directory holding `index.db` and `projects/`.
    ///
    /// `AGTX_DATA_DIR` overrides it so tests (and the smoke runner) can open real
    /// databases without writing into the user's own store — otherwise every
    /// `Database::open_project(TempDir)` in the suite leaves an orphan DB behind,
    /// keyed by a temp path that no longer exists. Same reasoning as
    /// `AGTX_AGENT_HOME`.
    pub fn data_root() -> Result<std::path::PathBuf> {
        if let Ok(dir) = std::env::var("AGTX_DATA_DIR") {
            if !dir.is_empty() {
                return Ok(std::path::PathBuf::from(dir));
            }
        }
        let dirs = directories::ProjectDirs::from("", "", "agtx")
            .context("Could not determine config directory")?;
        Ok(dirs.config_dir().to_path_buf())
    }

    /// Open or create a project database (stored centrally in config dir)
    pub fn open_project(project_path: &Path) -> Result<Self> {
        let config_dir = Self::data_root()?;

        // Create a stable ID from the project path using a hash
        let path_str = project_path.to_string_lossy();
        let path_hash = Self::hash_path(&path_str);

        let db_path = config_dir
            .join("projects")
            .join(format!("{}.db", path_hash));

        // Ensure projects directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Migration: if the new-hash DB doesn't exist, check for an old-hash DB and rename it
        if !db_path.exists() {
            let old_hash = Self::hash_path_legacy(&path_str);
            let old_db_path = config_dir
                .join("projects")
                .join(format!("{}.db", old_hash));
            if old_db_path.exists() {
                let _ = std::fs::rename(&old_db_path, &db_path);
            }
        }

        let conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open database at {:?}", db_path))?;

        // Harden file permissions: owner-only read/write
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o600));
        }

        let db = Self { conn };
        db.init_project_schema()?;
        Ok(db)
    }

    /// Create a stable hash from a path string for database filename.
    /// Uses SHA-256 (truncated to 16 hex chars) for cross-version stability.
    fn hash_path(path: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(path.as_bytes());
        let result = hasher.finalize();
        // Take first 8 bytes (16 hex chars) — same length as the old DefaultHasher output
        format!(
            "{:016x}",
            u64::from_be_bytes(result[..8].try_into().unwrap())
        )
    }

    /// Legacy hash function (DefaultHasher). Used only for migration from pre-SHA256 databases.
    fn hash_path_legacy(path: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Open or create the global index database
    pub fn open_global() -> Result<Self> {
        let db_path = Self::data_root()?.join("index.db");

        // Ensure config directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open global database at {:?}", db_path))?;

        // Harden file permissions: owner-only read/write
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o600));
        }

        let db = Self { conn };
        db.init_global_schema()?;
        Ok(db)
    }

    /// Open an in-memory project database (for testing only)
    #[cfg(feature = "test-mocks")]
    pub fn open_in_memory_project() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.init_project_schema()?;
        Ok(db)
    }

    /// Open a project DB at an arbitrary file path (for concurrency tests — in-memory is per-connection).
    #[cfg(feature = "test-mocks")]
    pub fn open_project_at_path(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open database at {:?}", path))?;
        let db = Self { conn };
        db.init_project_schema()?;
        Ok(db)
    }

    /// Open an in-memory global database (for testing only)
    #[cfg(feature = "test-mocks")]
    pub fn open_in_memory_global() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.init_global_schema()?;
        Ok(db)
    }

    fn init_project_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;",
        )?;

        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                description TEXT,
                status TEXT NOT NULL DEFAULT 'backlog',
                agent TEXT NOT NULL,
                project_id TEXT NOT NULL,
                session_name TEXT,
                worktree_path TEXT,
                branch_name TEXT,
                pr_number INTEGER,
                pr_url TEXT,
                plugin TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
            CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks(project_id);
            "#,
        )?;

        // Migration: add new columns if they don't exist
        let _ = self
            .conn
            .execute("ALTER TABLE tasks ADD COLUMN branch_name TEXT", []);
        let _ = self
            .conn
            .execute("ALTER TABLE tasks ADD COLUMN pr_number INTEGER", []);
        let _ = self
            .conn
            .execute("ALTER TABLE tasks ADD COLUMN pr_url TEXT", []);
        let _ = self
            .conn
            .execute("ALTER TABLE tasks ADD COLUMN plugin TEXT", []);
        let _ = self.conn.execute(
            "ALTER TABLE tasks ADD COLUMN cycle INTEGER NOT NULL DEFAULT 1",
            [],
        );
        let _ = self
            .conn
            .execute("ALTER TABLE tasks ADD COLUMN referenced_tasks TEXT", []);
        let _ = self
            .conn
            .execute("ALTER TABLE tasks ADD COLUMN escalation_note TEXT", []);
        let _ = self
            .conn
            .execute("ALTER TABLE tasks ADD COLUMN base_branch TEXT", []);

        // MCP transition request queue
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS transition_requests (
                id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL,
                action TEXT NOT NULL,
                requested_at TEXT NOT NULL,
                processed_at TEXT,
                error TEXT
            );

            CREATE TABLE IF NOT EXISTS notifications (
                id TEXT PRIMARY KEY,
                message TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            -- Published mirror of the TUI's in-memory PhaseStatus, for readers
            -- in other processes. One row per task, rewritten by each session
            -- refresh; `updated_at` is what tells a reader whether anything is
            -- still refreshing it.
            CREATE TABLE IF NOT EXISTS task_runtime (
                task_id TEXT PRIMARY KEY,
                phase_status TEXT NOT NULL,
                pane_hash TEXT,
                pane_changed_at TEXT,
                updated_at TEXT NOT NULL
            );
            "#,
        )?;

        // Migration: add reason column to transition_requests if it doesn't exist
        let _ = self
            .conn
            .execute("ALTER TABLE transition_requests ADD COLUMN reason TEXT", []);

        let _ = self.conn.execute(
            "ALTER TABLE transition_requests ADD COLUMN claimed_by TEXT",
            [],
        );

        // Migration: notifications gain the two fields an out-of-process reader
        // needs to route them. Nullable, so rows written before this survive.
        let _ = self
            .conn
            .execute("ALTER TABLE notifications ADD COLUMN task_id TEXT", []);
        let _ = self
            .conn
            .execute("ALTER TABLE notifications ADD COLUMN kind TEXT", []);

        Ok(())
    }

    fn init_global_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;",
        )?;

        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                path TEXT NOT NULL UNIQUE,
                github_url TEXT,
                default_agent TEXT,
                last_opened TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS running_agents (
                session_name TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                task_id TEXT NOT NULL,
                agent_name TEXT NOT NULL,
                started_at TEXT NOT NULL,
                status TEXT NOT NULL,
                FOREIGN KEY (project_id) REFERENCES projects(id)
            );

            CREATE INDEX IF NOT EXISTS idx_running_project ON running_agents(project_id);

            -- One row per project a TUI is currently open on. Its only job is
            -- to answer "is anything draining the transition queue?" — a phone
            -- can enqueue a request whenever, but nothing executes it without a
            -- running TUI, and a board that accepts taps and silently does
            -- nothing is worse than one that says so.
            CREATE TABLE IF NOT EXISTS tui_heartbeat (
                project_path TEXT PRIMARY KEY,
                beat_at TEXT NOT NULL
            );

            -- When a phone last asked for a project's board.
            --
            -- The TUI publishes `task_runtime` only while this is fresh, so a
            -- board nobody is looking at costs no writes at all. Without it the
            -- refresh wrote a transaction every couple of seconds forever, to
            -- keep a mirror current for a reader that might never exist.
            CREATE TABLE IF NOT EXISTS board_watch (
                project_path TEXT PRIMARY KEY,
                beat_at TEXT NOT NULL
            );

            -- Devices paired with `agtx serve`. One row per phone, so a lost
            -- one can be revoked without re-pairing the rest.
            --
            -- The token is stored **hashed**. This file is 0600, but a
            -- credential that grants shell-equivalent access to the machine
            -- should not be readable from a backup, a stray copy, or anything
            -- that later gets a wider mode by accident.
            CREATE TABLE IF NOT EXISTS mobile_devices (
                id TEXT PRIMARY KEY,
                label TEXT NOT NULL,
                token_hash TEXT NOT NULL UNIQUE,
                created_at TEXT NOT NULL,
                last_seen TEXT,
                session_id TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_devices_hash ON mobile_devices(token_hash);
            "#,
        )?;

        // Migration: a device records the serve session that paired it.
        // Nullable, so a row written before this column existed survives.
        let _ = self
            .conn
            .execute("ALTER TABLE mobile_devices ADD COLUMN session_id TEXT", []);

        Ok(())
    }

    // === Task Operations ===

    pub fn create_task(&self, task: &Task) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO tasks (id, title, description, status, agent, project_id, session_name, worktree_path, branch_name, pr_number, pr_url, plugin, cycle, referenced_tasks, escalation_note, base_branch, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
            "#,
            params![
                task.id,
                task.title,
                task.description,
                task.status.as_str(),
                task.agent,
                task.project_id,
                task.session_name,
                task.worktree_path,
                task.branch_name,
                task.pr_number,
                task.pr_url,
                task.plugin,
                task.cycle,
                task.referenced_tasks,
                task.escalation_note,
                task.base_branch,
                task.created_at.to_rfc3339(),
                task.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn create_tasks_batch(&mut self, tasks: &[Task]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for task in tasks {
            tx.execute(
                r#"
                INSERT INTO tasks (id, title, description, status, agent, project_id, session_name, worktree_path, branch_name, pr_number, pr_url, plugin, cycle, referenced_tasks, escalation_note, base_branch, created_at, updated_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
                "#,
                params![
                    task.id,
                    task.title,
                    task.description,
                    task.status.as_str(),
                    task.agent,
                    task.project_id,
                    task.session_name,
                    task.worktree_path,
                    task.branch_name,
                    task.pr_number,
                    task.pr_url,
                    task.plugin,
                    task.cycle,
                    task.referenced_tasks,
                    task.escalation_note,
                    task.base_branch,
                    task.created_at.to_rfc3339(),
                    task.updated_at.to_rfc3339(),
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn update_task(&self, task: &Task) -> Result<()> {
        self.conn.execute(
            r#"
            UPDATE tasks SET
                title = ?2,
                description = ?3,
                status = ?4,
                agent = ?5,
                session_name = ?6,
                worktree_path = ?7,
                branch_name = ?8,
                pr_number = ?9,
                pr_url = ?10,
                plugin = ?11,
                cycle = ?12,
                referenced_tasks = ?13,
                escalation_note = ?14,
                base_branch = ?15,
                updated_at = ?16
            WHERE id = ?1
            "#,
            params![
                task.id,
                task.title,
                task.description,
                task.status.as_str(),
                task.agent,
                task.session_name,
                task.worktree_path,
                task.branch_name,
                task.pr_number,
                task.pr_url,
                task.plugin,
                task.cycle,
                task.referenced_tasks,
                task.escalation_note,
                task.base_branch,
                task.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn delete_task(&self, task_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM tasks WHERE id = ?1", params![task_id])?;
        Ok(())
    }

    fn task_from_row(row: &rusqlite::Row) -> rusqlite::Result<Task> {
        Ok(Task {
            id: row.get("id")?,
            title: row.get("title")?,
            description: row.get("description")?,
            status: TaskStatus::from_str(&row.get::<_, String>("status")?)
                .unwrap_or(TaskStatus::Backlog),
            agent: row.get("agent")?,
            project_id: row.get("project_id")?,
            session_name: row.get("session_name")?,
            worktree_path: row.get("worktree_path")?,
            branch_name: row.get("branch_name").ok().flatten(),
            pr_number: row.get("pr_number").ok().flatten(),
            pr_url: row.get("pr_url").ok().flatten(),
            plugin: row.get("plugin").ok().flatten(),
            cycle: row.get("cycle").unwrap_or(1),
            referenced_tasks: row.get("referenced_tasks").ok().flatten(),
            escalation_note: row.get("escalation_note").ok().flatten(),
            base_branch: row.get("base_branch").ok().flatten(),
            created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>("created_at")?)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
            updated_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>("updated_at")?)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
        })
    }

    pub fn get_task(&self, task_id: &str) -> Result<Option<Task>> {
        let mut stmt = self.conn.prepare("SELECT * FROM tasks WHERE id = ?1")?;

        let task = stmt.query_row(params![task_id], Self::task_from_row).ok();

        Ok(task)
    }

    pub fn get_tasks_by_status(&self, status: TaskStatus) -> Result<Vec<Task>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM tasks WHERE status = ?1 ORDER BY created_at")?;

        let tasks = stmt
            .query_map(params![status.as_str()], Self::task_from_row)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(tasks)
    }

    pub fn get_all_tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM tasks ORDER BY created_at")?;

        let tasks = stmt
            .query_map([], Self::task_from_row)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(tasks)
    }

    /// Check whether all referenced_tasks (dependencies) are in Review or Done.
    /// Returns true if the task has no dependencies or all deps are satisfied.
    pub fn deps_satisfied(&self, task: &Task) -> bool {
        let refs_str = match &task.referenced_tasks {
            Some(s) if !s.is_empty() => s,
            _ => return true,
        };
        refs_str.split(',').filter(|s| !s.is_empty()).all(|ref_id| {
            self.get_task(ref_id).ok().flatten().map_or(true, |t| {
                matches!(t.status, TaskStatus::Review | TaskStatus::Done)
            })
        })
    }

    // === Project Operations (for global db) ===

    pub fn upsert_project(&self, project: &Project) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO projects (id, name, path, github_url, default_agent, last_opened)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(path) DO UPDATE SET
                name = excluded.name,
                github_url = excluded.github_url,
                default_agent = excluded.default_agent,
                last_opened = excluded.last_opened
            "#,
            params![
                project.id,
                project.name,
                project.path,
                project.github_url,
                project.default_agent,
                project.last_opened.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_project_by_id(&self, id: &str) -> Result<Option<Project>> {
        let mut stmt = self.conn.prepare("SELECT * FROM projects WHERE id = ?1")?;

        let project = stmt
            .query_row(params![id], |row| {
                Ok(Project {
                    id: row.get("id")?,
                    name: row.get("name")?,
                    path: row.get("path")?,
                    github_url: row.get("github_url")?,
                    default_agent: row.get("default_agent")?,
                    last_opened: chrono::DateTime::parse_from_rfc3339(
                        &row.get::<_, String>("last_opened")?,
                    )
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
                })
            })
            .ok();

        Ok(project)
    }

    pub fn get_all_projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM projects ORDER BY last_opened DESC")?;

        let projects = stmt
            .query_map([], |row| {
                Ok(Project {
                    id: row.get("id")?,
                    name: row.get("name")?,
                    path: row.get("path")?,
                    github_url: row.get("github_url")?,
                    default_agent: row.get("default_agent")?,
                    last_opened: chrono::DateTime::parse_from_rfc3339(
                        &row.get::<_, String>("last_opened")?,
                    )
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(projects)
    }

    // === Transition Request Operations (MCP command queue) ===

    pub fn create_transition_request(&self, req: &TransitionRequest) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO transition_requests (id, task_id, action, reason, requested_at, processed_at, error)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                req.id,
                req.task_id,
                req.action,
                req.reason,
                req.requested_at.to_rfc3339(),
                req.processed_at.map(|dt| dt.to_rfc3339()),
                req.error,
            ],
        )?;
        Ok(())
    }

    pub fn get_transition_request(&self, id: &str) -> Result<Option<TransitionRequest>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM transition_requests WHERE id = ?1")?;

        let req = stmt
            .query_row(params![id], Self::transition_request_from_row)
            .ok();

        Ok(req)
    }

    pub fn get_pending_transition_requests(&self) -> Result<Vec<TransitionRequest>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM transition_requests
             WHERE processed_at IS NULL AND claimed_by IS NULL
             ORDER BY requested_at ASC",
        )?;

        let requests = stmt
            .query_map([], Self::transition_request_from_row)?
            .filter_map(|r| r.ok())
            .collect();

        Ok(requests)
    }

    pub fn mark_transition_processed(&self, id: &str, error: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE transition_requests SET processed_at = ?1, error = ?2 WHERE id = ?3",
            params![chrono::Utc::now().to_rfc3339(), error, id],
        )?;
        Ok(())
    }

    /// Atomically claim a pending request. Returns true iff this caller won.
    pub fn claim_transition_request(&self, id: &str, claimant: &str) -> Result<bool> {
        let rows = self.conn.execute(
            "UPDATE transition_requests
             SET claimed_by = ?1
             WHERE id = ?2 AND claimed_by IS NULL AND processed_at IS NULL",
            params![claimant, id],
        )?;
        Ok(rows == 1)
    }

    pub fn cleanup_old_transition_requests(&self) -> Result<()> {
        let cutoff = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        self.conn.execute(
            "DELETE FROM transition_requests
             WHERE (processed_at IS NOT NULL AND processed_at < ?1)
                OR (processed_at IS NULL AND claimed_by IS NOT NULL AND requested_at < ?1)",
            params![cutoff],
        )?;
        Ok(())
    }

    /// Directly set processed_at to an arbitrary timestamp (for testing cleanup logic only).
    #[cfg(feature = "test-mocks")]
    pub fn backdate_transition_processed_at(&self, id: &str, processed_at: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE transition_requests SET processed_at = ?1 WHERE id = ?2",
            params![processed_at, id],
        )?;
        Ok(())
    }

    /// Directly set requested_at to an arbitrary timestamp (for testing cleanup logic only).
    #[cfg(feature = "test-mocks")]
    pub fn backdate_transition_requested_at(&self, id: &str, requested_at: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE transition_requests SET requested_at = ?1 WHERE id = ?2",
            params![requested_at, id],
        )?;
        Ok(())
    }

    fn transition_request_from_row(row: &rusqlite::Row) -> rusqlite::Result<TransitionRequest> {
        Ok(TransitionRequest {
            id: row.get("id")?,
            task_id: row.get("task_id")?,
            action: row.get("action")?,
            reason: row.get("reason").ok().flatten(),
            requested_at: chrono::DateTime::parse_from_rfc3339(
                &row.get::<_, String>("requested_at")?,
            )
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now()),
            processed_at: row.get::<_, Option<String>>("processed_at")?.and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(&s)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .ok()
            }),
            error: row.get("error")?,
        })
    }

    // ── Notifications ───────────────────────────────────────────────────

    pub fn create_notification(&self, notif: &Notification) -> Result<()> {
        self.conn.execute(
            "INSERT INTO notifications (id, message, created_at, task_id, kind)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                notif.id,
                notif.message,
                notif.created_at.to_rfc3339(),
                notif.task_id,
                notif.kind.map(|k| k.as_str()),
            ],
        )?;
        Ok(())
    }

    /// Peek at pending notifications without consuming them.
    pub fn peek_notifications(&self) -> Result<Vec<Notification>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM notifications ORDER BY created_at ASC")?;

        let notifs: Vec<Notification> = stmt
            .query_map([], |row| {
                Ok(Notification {
                    id: row.get("id")?,
                    message: row.get("message")?,
                    created_at: chrono::DateTime::parse_from_rfc3339(
                        &row.get::<_, String>("created_at")?,
                    )
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
                    task_id: row.get("task_id")?,
                    kind: row
                        .get::<_, Option<String>>("kind")?
                        .as_deref()
                        .and_then(NotificationKind::from_str),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(notifs)
    }

    /// Atomic fetch-and-delete via `DELETE ... RETURNING`.
    pub fn consume_notifications(&self) -> Result<Vec<Notification>> {
        let mut stmt = self.conn.prepare(
            "DELETE FROM notifications
                 RETURNING id, message, created_at, task_id, kind",
        )?;

        let mut notifs: Vec<Notification> = stmt
            .query_map([], |row| {
                Ok(Notification {
                    id: row.get("id")?,
                    message: row.get("message")?,
                    created_at: chrono::DateTime::parse_from_rfc3339(
                        &row.get::<_, String>("created_at")?,
                    )
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now()),
                    task_id: row.get("task_id")?,
                    kind: row
                        .get::<_, Option<String>>("kind")?
                        .as_deref()
                        .and_then(NotificationKind::from_str),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        // `RETURNING` doesn't guarantee any particular row order — sort in Rust.
        notifs.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(notifs)
    }

    // ── Paired devices ──────────────────────────────────────────────────

    pub fn add_mobile_device(&self, device: &MobileDevice) -> Result<()> {
        self.conn.execute(
            "INSERT INTO mobile_devices (id, label, token_hash, created_at, last_seen, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                device.id,
                device.label,
                device.token_hash,
                device.created_at.to_rfc3339(),
                device.last_seen.map(|t| t.to_rfc3339()),
                device.session_id,
            ],
        )?;
        Ok(())
    }

    /// The device holding this token hash, if any.
    ///
    /// Looked up by hash rather than compared in Rust so the secret itself
    /// never has to be read out of the database to check a request.
    pub fn mobile_device_by_hash(&self, token_hash: &str) -> Result<Option<MobileDevice>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM mobile_devices WHERE token_hash = ?1")?;
        let mut rows = stmt.query_map(params![token_hash], Self::mobile_device_from_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn list_mobile_devices(&self) -> Result<Vec<MobileDevice>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM mobile_devices ORDER BY created_at ASC")?;
        let rows = stmt
            .query_map([], Self::mobile_device_from_row)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Record that a device was used. Callers should throttle: this is a write,
    /// and a phone polling the board would otherwise cause one every two
    /// seconds to store a timestamp nobody reads that often.
    pub fn touch_mobile_device(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE mobile_devices SET last_seen = ?1 WHERE id = ?2",
            params![chrono::Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// Returns whether a row was removed, so a caller can tell "revoked" from
    /// "no such device" rather than reporting success either way.
    pub fn revoke_mobile_device(&self, id: &str) -> Result<bool> {
        let rows = self
            .conn
            .execute("DELETE FROM mobile_devices WHERE id = ?1", params![id])?;
        Ok(rows > 0)
    }

    /// Drop every device paired by one serve session.
    ///
    /// Nothing calls this on shutdown — pairings persist until revoked. It is
    /// the scoped primitive an expiry or forget-on-exit policy needs, and the
    /// scoping is the point: `mobile_devices` is global, so a blanket delete
    /// would cut off a second agtx serving another project.
    ///
    /// A `NULL` session_id is never matched, so a row written before the
    /// column existed is left for the user to revoke by hand.
    pub fn revoke_session_devices(&self, session_id: &str) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM mobile_devices WHERE session_id = ?1",
            params![session_id],
        )?)
    }

    /// Returns how many were removed.
    pub fn revoke_all_mobile_devices(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM mobile_devices", [])?)
    }

    fn mobile_device_from_row(row: &rusqlite::Row) -> rusqlite::Result<MobileDevice> {
        let parse = |s: String| {
            chrono::DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .ok()
        };
        Ok(MobileDevice {
            id: row.get("id")?,
            label: row.get("label")?,
            token_hash: row.get("token_hash")?,
            created_at: row
                .get::<_, Option<String>>("created_at")?
                .and_then(parse)
                .unwrap_or_else(chrono::Utc::now),
            last_seen: row.get::<_, Option<String>>("last_seen")?.and_then(parse),
            session_id: row.get("session_id")?,
        })
    }

    // ── TUI heartbeat ───────────────────────────────────────────────────

    /// Record that a TUI is live on `project_path`, as of now.
    ///
    /// Called on the transition-poll cadence rather than per event-loop
    /// iteration: the loop wakes on `HOUSEKEEPING_TICK` (100 ms) and on every
    /// keystroke, so beating per iteration would be a write ten-plus times a
    /// second to answer a question whose useful resolution is seconds.
    pub fn beat_tui_heartbeat(&self, project_path: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tui_heartbeat (project_path, beat_at) VALUES (?1, ?2)
             ON CONFLICT(project_path) DO UPDATE SET beat_at = excluded.beat_at",
            params![project_path, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Record that something asked for this project's board.
    ///
    /// Written by the web server on a board request. Callers should throttle:
    /// this is a write, and it answers a question whose useful resolution is
    /// minutes.
    pub fn note_board_watched(&self, project_path: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO board_watch (project_path, beat_at) VALUES (?1, ?2)
             ON CONFLICT(project_path) DO UPDATE SET beat_at = excluded.beat_at",
            params![project_path, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Whether a board was asked for within `stale_after`.
    ///
    /// The gate on publishing `task_runtime`. Absent and stale are the same
    /// answer — nobody is reading — so a missing row is not an error.
    pub fn board_watched_recently(
        &self,
        project_path: &str,
        stale_after: chrono::Duration,
    ) -> Result<bool> {
        let beat: Option<String> = self
            .conn
            .query_row(
                "SELECT beat_at FROM board_watch WHERE project_path = ?1",
                params![project_path],
                |row| row.get(0),
            )
            .ok();
        Ok(beat
            .and_then(|b| chrono::DateTime::parse_from_rfc3339(&b).ok())
            .is_some_and(|t| chrono::Utc::now() - t.with_timezone(&chrono::Utc) < stale_after))
    }

    /// Whether a TUI has beaten for `project_path` within `stale_after`.
    ///
    /// Absent and stale are the same answer — nothing is draining the queue —
    /// so a missing row is not an error.
    pub fn tui_is_live(&self, project_path: &str, stale_after: chrono::Duration) -> Result<bool> {
        let beat: Option<String> = self
            .conn
            .query_row(
                "SELECT beat_at FROM tui_heartbeat WHERE project_path = ?1",
                params![project_path],
                |row| row.get(0),
            )
            .ok();
        Ok(beat
            .and_then(|b| chrono::DateTime::parse_from_rfc3339(&b).ok())
            .is_some_and(|t| chrono::Utc::now() - t.with_timezone(&chrono::Utc) < stale_after))
    }

    // ── Task runtime (published phase status) ───────────────────────────

    /// Publish one refresh pass as a whole: upsert every live task's row and
    /// drop rows for tasks that no longer exist, in one transaction.
    ///
    /// The refresh writes every live task on every pass — that is what makes
    /// `updated_at` mean "last observed" rather than "last changed", which is
    /// the distinction a reader needs to tell a steadily-working task from a
    /// board nothing has refreshed since the TUI died. Batching is what keeps
    /// that affordable: one commit per pass instead of one per task.
    ///
    /// Pruning removes rows for tasks that no longer *exist*, not rows absent
    /// from this pass: the refresh tracks only active phases, so a task that
    /// reaches Done stops being published and keeps its last row. That row is
    /// stale rather than wrong, and `updated_at` is what a reader checks — the
    /// same contract that covers a board nothing is refreshing at all. Pruning
    /// to the published set instead would empty the table on any pass that
    /// transiently reported nothing.
    ///
    /// Uses an unchecked transaction because the caller holds `&Database`.
    pub fn publish_task_runtime(&self, rows: &[TaskRuntime]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for rt in rows {
            tx.execute(
                "INSERT INTO task_runtime
                     (task_id, phase_status, pane_hash, pane_changed_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(task_id) DO UPDATE SET
                     phase_status    = excluded.phase_status,
                     pane_hash       = excluded.pane_hash,
                     pane_changed_at = excluded.pane_changed_at,
                     updated_at      = excluded.updated_at",
                params![
                    rt.task_id,
                    rt.phase_status.as_str(),
                    rt.pane_hash,
                    rt.pane_changed_at.map(|t| t.to_rfc3339()),
                    rt.updated_at.to_rfc3339(),
                ],
            )?;
        }
        // A deleted task's last status would otherwise be served to readers
        // forever as current. In the same transaction so a reader never sees a
        // pass applied without its prune.
        tx.execute(
            "DELETE FROM task_runtime WHERE task_id NOT IN (SELECT id FROM tasks)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_task_runtime(&self, task_id: &str) -> Result<Option<TaskRuntime>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM task_runtime WHERE task_id = ?1")?;
        let mut rows = stmt.query_map(params![task_id], Self::task_runtime_from_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn list_task_runtime(&self) -> Result<Vec<TaskRuntime>> {
        let mut stmt = self.conn.prepare("SELECT * FROM task_runtime")?;
        let rows = stmt
            .query_map([], Self::task_runtime_from_row)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    fn task_runtime_from_row(row: &rusqlite::Row) -> rusqlite::Result<TaskRuntime> {
        let parse = |s: String| {
            chrono::DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .ok()
        };
        Ok(TaskRuntime {
            task_id: row.get("task_id")?,
            // An unparseable status means a writer from a newer version wrote a
            // variant this build has no arm for. Treating that as `Working` is
            // the honest fallback: it is the state that claims the least.
            phase_status: PhaseStatus::from_str(&row.get::<_, String>("phase_status")?)
                .unwrap_or(PhaseStatus::Working),
            pane_hash: row.get("pane_hash")?,
            pane_changed_at: row
                .get::<_, Option<String>>("pane_changed_at")?
                .and_then(parse),
            updated_at: row
                .get::<_, Option<String>>("updated_at")?
                .and_then(parse)
                .unwrap_or_else(chrono::Utc::now),
        })
    }
}
