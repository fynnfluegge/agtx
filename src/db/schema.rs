use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::Path;

use super::models::{Project, Task, TaskStatus};

/// Database wrapper for SQLite operations
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open or create a project database (stored centrally in config dir)
    pub fn open_project(project_path: &Path) -> Result<Self> {
        let config_dir = directories::ProjectDirs::from("", "", "agtx")
            .context("Could not determine config directory")?;

        // Create a stable ID from the project path using a hash
        let path_str = project_path.to_string_lossy();
        let path_hash = Self::hash_path(&path_str);

        let db_path = config_dir
            .config_dir()
            .join("projects")
            .join(format!("{}.db", path_hash));

        // Ensure projects directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open database at {:?}", db_path))?;

        let db = Self { conn };
        db.init_project_schema()?;
        Ok(db)
    }

    /// Create a stable hash from a path string for database filename
    fn hash_path(path: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Open or create the global index database
    pub fn open_global() -> Result<Self> {
        let config_dir = directories::ProjectDirs::from("", "", "agtx")
            .context("Could not determine config directory")?;
        let db_path = config_dir.config_dir().join("index.db");

        // Ensure config directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open global database at {:?}", db_path))?;

        let db = Self { conn };
        db.init_global_schema()?;
        Ok(db)
    }

    fn init_project_schema(&self) -> Result<()> {
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
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
            CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks(project_id);
            "#,
        )?;

        // Migration: add new columns if they don't exist
        let _ = self.conn.execute("ALTER TABLE tasks ADD COLUMN branch_name TEXT", []);
        let _ = self.conn.execute("ALTER TABLE tasks ADD COLUMN pr_number INTEGER", []);
        let _ = self.conn.execute("ALTER TABLE tasks ADD COLUMN pr_url TEXT", []);

        Ok(())
    }

    fn init_global_schema(&self) -> Result<()> {
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
            "#,
        )?;
        Ok(())
    }

    // === Task Operations ===

    pub fn create_task(&self, task: &Task) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO tasks (id, title, description, status, agent, project_id, session_name, worktree_path, branch_name, pr_number, pr_url, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
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
                task.created_at.to_rfc3339(),
                task.updated_at.to_rfc3339(),
            ],
        )?;
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
                updated_at = ?11
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
            created_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>("created_at")?)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
            updated_at: chrono::DateTime::parse_from_rfc3339(&row.get::<_, String>("updated_at")?)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
        })
    }

    pub fn get_task(&self, task_id: &str) -> Result<Option<Task>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM tasks WHERE id = ?1")?;

        let task = stmt
            .query_row(params![task_id], Self::task_from_row)
            .ok();

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
}
