use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize)]
pub struct ProjectResponse {
    pub id: String,
    pub name: String,
    pub path: String,
    pub github_url: Option<String>,
    pub task_counts: HashMap<String, usize>,
}

#[derive(Serialize)]
pub struct TaskSummary {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub agent: String,
    pub branch_name: Option<String>,
    pub pr_url: Option<String>,
    pub pr_number: Option<i32>,
    pub plugin: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize)]
pub struct KanbanResponse {
    pub project_id: String,
    pub columns: Vec<KanbanColumn>,
}

#[derive(Serialize)]
pub struct KanbanColumn {
    pub status: String,
    pub display_name: String,
    pub tasks: Vec<TaskSummary>,
}

#[derive(Serialize)]
pub struct TaskDetailResponse {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub agent: String,
    pub project_id: String,
    pub branch_name: Option<String>,
    pub pr_url: Option<String>,
    pub pr_number: Option<i32>,
    pub plugin: Option<String>,
    pub cycle: i32,
    pub worktree_path: Option<String>,
    pub escalation_note: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub artifacts: Vec<ArtifactInfo>,
}

#[derive(Serialize, Clone)]
pub struct ArtifactInfo {
    pub name: String,
    pub label: String,
    pub available: bool,
}
