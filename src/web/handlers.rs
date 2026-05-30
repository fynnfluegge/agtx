use anyhow::Context;
use axum::{
    extract::Path,
    http::StatusCode,
    response::{Html, Json},
};
use std::path::PathBuf;

use crate::db::{Database, Task, TaskStatus};
use crate::web::models::{
    ArtifactInfo, KanbanColumn, KanbanResponse, ProjectResponse, TaskDetailResponse, TaskSummary,
};

static INDEX_HTML: &str = include_str!("index.html");

#[derive(serde::Serialize)]
struct InitialState {
    project_id: Option<String>,
    task_id: Option<String>,
    artifact: Option<String>,
}

fn render_shell(title: &str, state: &InitialState) -> Html<String> {
    let state_json = serde_json::to_string(state).unwrap_or_else(|_| "null".to_string());
    let safe_title = title
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    Html(
        INDEX_HTML
            .replace("__TITLE__", &safe_title)
            .replace("__STATE__", &state_json),
    )
}

pub async fn index() -> Html<String> {
    render_shell(
        "agtx",
        &InitialState {
            project_id: None,
            task_id: None,
            artifact: None,
        },
    )
}

pub async fn list_projects() -> Result<Json<Vec<ProjectResponse>>, StatusCode> {
    let result = tokio::task::spawn_blocking(|| -> anyhow::Result<Vec<ProjectResponse>> {
        let global_db = Database::open_global()?;
        let projects = global_db.get_all_projects()?;

        let mut responses = Vec::new();
        for project in projects {
            let project_path = PathBuf::from(&project.path);
            let mut task_counts = std::collections::HashMap::new();
            for status in TaskStatus::columns() {
                task_counts.insert(status.as_str().to_string(), 0usize);
            }
            if let Ok(project_db) = Database::open_project(&project_path) {
                if let Ok(tasks) = project_db.get_all_tasks() {
                    for task in &tasks {
                        *task_counts
                            .entry(task.status.as_str().to_string())
                            .or_insert(0) += 1;
                    }
                }
            }
            responses.push(ProjectResponse {
                id: project.id,
                name: project.name,
                path: project.path,
                github_url: project.github_url,
                task_counts,
            });
        }
        Ok(responses)
    })
    .await
    .map_err(|e| {
        eprintln!("web: spawn error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .map_err(|e| {
        eprintln!("web: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(result))
}

pub async fn project_tasks(
    Path(project_id): Path<String>,
) -> Result<Json<KanbanResponse>, StatusCode> {
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<KanbanResponse> {
        let global_db = Database::open_global()?;
        let project = global_db
            .get_project_by_id(&project_id)?
            .ok_or_else(|| anyhow::anyhow!("Project not found"))?;
        let project_path = PathBuf::from(&project.path);

        let project_db = Database::open_project(&project_path)?;
        let tasks = project_db.get_all_tasks()?;

        let columns = TaskStatus::columns()
            .iter()
            .map(|status| {
                let col_tasks: Vec<TaskSummary> = tasks
                    .iter()
                    .filter(|t| t.status == *status)
                    .map(task_to_summary)
                    .collect();
                KanbanColumn {
                    status: status.as_str().to_string(),
                    display_name: status.display_name().to_string(),
                    tasks: col_tasks,
                }
            })
            .collect();

        Ok(KanbanResponse {
            project_id: project.id,
            columns,
        })
    })
    .await
    .map_err(|e| {
        eprintln!("web: spawn error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .map_err(|e| {
        eprintln!("web: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(result))
}

pub async fn task_detail(
    Path((project_id, task_id)): Path<(String, String)>,
) -> Result<Json<TaskDetailResponse>, StatusCode> {
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<TaskDetailResponse> {
        let (task, project_path) = fetch_task(&project_id, &task_id)?;
        let artifacts = resolve_artifacts(&task, &project_path)
            .into_iter()
            .map(|(info, _)| info)
            .collect();

        Ok(TaskDetailResponse {
            id: task.id,
            title: task.title,
            description: task.description,
            status: task.status.as_str().to_string(),
            agent: task.agent,
            project_id: task.project_id,
            branch_name: task.branch_name,
            pr_url: task.pr_url,
            pr_number: task.pr_number,
            plugin: task.plugin,
            cycle: task.cycle,
            worktree_path: task.worktree_path,
            escalation_note: task.escalation_note,
            created_at: task.created_at.to_rfc3339(),
            updated_at: task.updated_at.to_rfc3339(),
            artifacts,
        })
    })
    .await
    .map_err(|e| {
        eprintln!("web: spawn error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .map_err(|e| {
        eprintln!("web: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(result))
}

pub async fn get_artifact(
    Path((project_id, task_id, name)): Path<(String, String, String)>,
) -> Result<Html<String>, StatusCode> {
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
        let (task, project_path) = fetch_task(&project_id, &task_id)?;
        let artifacts = resolve_artifacts(&task, &project_path);

        let found = artifacts.into_iter().find(|(info, _)| info.name == name);
        match found {
            Some((_, Some(path))) => {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read {path:?}"))?;
                Ok(Some(super::markdown::render_markdown(&content)))
            }
            _ => Ok(None),
        }
    })
    .await
    .map_err(|e| {
        eprintln!("web: spawn error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?
    .map_err(|e| {
        eprintln!("web: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    match result {
        Some(html) => Ok(Html(html)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn fetch_task(project_id: &str, task_id: &str) -> anyhow::Result<(Task, PathBuf)> {
    let global_db = Database::open_global()?;
    let project = global_db
        .get_project_by_id(project_id)?
        .ok_or_else(|| anyhow::anyhow!("Project not found"))?;
    let project_path = PathBuf::from(&project.path);
    let project_db = Database::open_project(&project_path)?;
    let task = project_db
        .get_task(task_id)?
        .ok_or_else(|| anyhow::anyhow!("Task not found"))?;
    Ok((task, project_path))
}

const ARTIFACTS: &[(&str, &str, &str)] = &[
    ("research", "Research", ".agtx/research.md"),
    ("plan", "Plan", ".agtx/plan.md"),
    ("execute", "Execute", ".agtx/execute.md"),
    ("review", "Review", ".agtx/review.md"),
];

fn task_slug(task: &Task) -> String {
    task.branch_name
        .as_deref()
        .and_then(|b| b.strip_prefix("task/"))
        .unwrap_or(&task.id)
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

fn resolve_artifacts(
    task: &Task,
    project_path: &std::path::Path,
) -> Vec<(ArtifactInfo, Option<PathBuf>)> {
    let slug = task_slug(task);
    ARTIFACTS
        .iter()
        .map(|(name, label, rel_path)| {
            let active = task
                .worktree_path
                .as_ref()
                .map(|wt| PathBuf::from(wt).join(rel_path))
                .filter(|p| p.exists());

            let basename = std::path::Path::new(rel_path)
                .file_name()
                .unwrap_or_default();
            let archive = project_path
                .join(".agtx")
                .join("archive")
                .join(&slug)
                .join(basename);
            let archive = if archive.exists() {
                Some(archive)
            } else {
                None
            };

            let found = active.or(archive);
            let info = ArtifactInfo {
                name: name.to_string(),
                label: label.to_string(),
                available: found.is_some(),
            };
            (info, found)
        })
        .collect()
}

pub async fn page_project(Path(pid): Path<String>) -> Html<String> {
    let pid2 = pid.clone();
    let name = tokio::task::spawn_blocking(move || -> Option<String> {
        let db = Database::open_global().ok()?;
        db.get_project_by_id(&pid2).ok()?.map(|p| p.name)
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| pid.clone());

    render_shell(
        &format!("{name} — agtx"),
        &InitialState {
            project_id: Some(pid),
            task_id: None,
            artifact: None,
        },
    )
}

pub async fn page_task(Path((pid, tid)): Path<(String, String)>) -> Html<String> {
    let pid2 = pid.clone();
    let tid2 = tid.clone();
    let names = tokio::task::spawn_blocking(move || -> Option<(String, String)> {
        let db = Database::open_global().ok()?;
        let project = db.get_project_by_id(&pid2).ok()??;
        let project_path = PathBuf::from(&project.path);
        let pdb = Database::open_project(&project_path).ok()?;
        let task = pdb.get_task(&tid2).ok()??;
        Some((project.name, task.title))
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| (pid.clone(), tid.clone()));

    let (proj_name, task_title) = names;
    render_shell(
        &format!("{task_title} — {proj_name} — agtx"),
        &InitialState {
            project_id: Some(pid),
            task_id: Some(tid),
            artifact: None,
        },
    )
}

pub async fn page_artifact(
    Path((pid, tid, artifact)): Path<(String, String, String)>,
) -> Html<String> {
    let pid2 = pid.clone();
    let tid2 = tid.clone();
    let names = tokio::task::spawn_blocking(move || -> Option<(String, String)> {
        let db = Database::open_global().ok()?;
        let project = db.get_project_by_id(&pid2).ok()??;
        let project_path = PathBuf::from(&project.path);
        let pdb = Database::open_project(&project_path).ok()?;
        let task = pdb.get_task(&tid2).ok()??;
        Some((project.name, task.title))
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| (pid.clone(), tid.clone()));

    let art_label = ARTIFACTS
        .iter()
        .find(|(name, _, _)| *name == artifact.as_str())
        .map(|(_, label, _)| label.to_string())
        .unwrap_or_else(|| artifact.clone());
    let (proj_name, task_title) = names;
    render_shell(
        &format!("{art_label} — {task_title} — {proj_name} — agtx"),
        &InitialState {
            project_id: Some(pid),
            task_id: Some(tid),
            artifact: Some(artifact),
        },
    )
}

fn task_to_summary(task: &Task) -> TaskSummary {
    TaskSummary {
        id: task.id.clone(),
        title: task.title.clone(),
        description: task.description.clone(),
        status: task.status.as_str().to_string(),
        agent: task.agent.clone(),
        branch_name: task.branch_name.clone(),
        pr_url: task.pr_url.clone(),
        pr_number: task.pr_number,
        plugin: task.plugin.clone(),
        created_at: task.created_at.to_rfc3339(),
        updated_at: task.updated_at.to_rfc3339(),
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_artifacts, task_slug};
    use crate::db::Task;

    #[test]
    fn task_slug_uses_sanitized_branch_suffix() {
        let mut task = Task::new("Traversal", "codex", "project");
        task.branch_name = Some("task/../../evil_slug-1".to_string());

        assert_eq!(task_slug(&task), "evil_slug-1");
    }

    #[test]
    fn task_slug_falls_back_to_task_id() {
        let task = Task::new("No Branch", "codex", "project");

        assert_eq!(task_slug(&task), task.id);
    }

    #[test]
    fn resolve_artifacts_prefers_active_worktree_and_falls_back_to_archive() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path().join("project");
        let worktree_path = dir.path().join("worktree");

        std::fs::create_dir_all(worktree_path.join(".agtx")).unwrap();
        std::fs::write(worktree_path.join(".agtx").join("plan.md"), "# Plan").unwrap();

        let archive_path = project_path.join(".agtx").join("archive").join("my-task");
        std::fs::create_dir_all(&archive_path).unwrap();
        std::fs::write(archive_path.join("review.md"), "# Review").unwrap();

        let mut task = Task::new("Artifacts", "codex", "project");
        task.branch_name = Some("task/my-task".to_string());
        task.worktree_path = Some(worktree_path.to_string_lossy().to_string());

        let artifacts = resolve_artifacts(&task, &project_path);

        let plan = artifacts
            .iter()
            .find(|(info, _)| info.name == "plan")
            .unwrap();
        assert!(plan.0.available);
        assert_eq!(
            plan.1.as_ref().unwrap(),
            &worktree_path.join(".agtx").join("plan.md")
        );

        let review = artifacts
            .iter()
            .find(|(info, _)| info.name == "review")
            .unwrap();
        assert!(review.0.available);
        assert_eq!(review.1.as_ref().unwrap(), &archive_path.join("review.md"));

        let research = artifacts
            .iter()
            .find(|(info, _)| info.name == "research")
            .unwrap();
        assert!(!research.0.available);
        assert!(research.1.is_none());
    }
}
