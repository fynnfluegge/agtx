use agtx::{
    db::{Database, Project, Task, TaskStatus},
    web::server::router,
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use tempfile::TempDir;
use tower::ServiceExt;

struct ConfigEnv {
    _dir: TempDir,
    previous: Option<String>,
}

impl ConfigEnv {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        Self {
            _dir: dir,
            previous,
        }
    }
}

impl Drop for ConfigEnv {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
}

struct Fixture {
    _config: ConfigEnv,
    _dir: TempDir,
    project: Project,
    backlog_task: Task,
}

fn setup_fixture() -> Fixture {
    let config = ConfigEnv::new();
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("project");
    let worktree_path = dir.path().join("worktree");

    std::fs::create_dir_all(&project_path).unwrap();
    std::fs::create_dir_all(worktree_path.join(".agtx")).unwrap();
    std::fs::write(
        worktree_path.join(".agtx").join("plan.md"),
        "# Plan\n\nThis is **covered**.\n",
    )
    .unwrap();

    let project = Project::new("Web Project", project_path.to_string_lossy().to_string());
    let global_db = Database::open_global().unwrap();
    global_db.upsert_project(&project).unwrap();

    let project_db = Database::open_project(&project_path).unwrap();

    let mut backlog_task = Task::new("Web Task", "codex", &project.id);
    backlog_task.description = Some("Task details".to_string());
    backlog_task.branch_name = Some("task/web-task".to_string());
    backlog_task.worktree_path = Some(worktree_path.to_string_lossy().to_string());
    project_db.create_task(&backlog_task).unwrap();

    let mut running_task = Task::new("Running Task", "claude", &project.id);
    running_task.status = TaskStatus::Running;
    project_db.create_task(&running_task).unwrap();

    Fixture {
        _config: config,
        _dir: dir,
        project,
        backlog_task,
    }
}

async fn get(uri: &str) -> (StatusCode, String) {
    let response = router()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn get_json(uri: &str) -> (StatusCode, Value) {
    let (status, body) = get(uri).await;
    let json = serde_json::from_str(&body).unwrap_or_else(|_| panic!("invalid JSON: {body}"));
    (status, json)
}

#[tokio::test]
async fn web_routes_return_project_task_artifact_and_page_data() {
    let fixture = setup_fixture();

    let (status, projects) = get_json("/api/projects").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(projects.as_array().unwrap().len(), 1);
    assert_eq!(projects[0]["id"], fixture.project.id);
    assert_eq!(projects[0]["task_counts"]["backlog"], json!(1));
    assert_eq!(projects[0]["task_counts"]["planning"], json!(0));
    assert_eq!(projects[0]["task_counts"]["running"], json!(1));
    assert_eq!(projects[0]["task_counts"]["review"], json!(0));
    assert_eq!(projects[0]["task_counts"]["done"], json!(0));

    let tasks_uri = format!("/api/projects/{}/tasks", fixture.project.id);
    let (status, board) = get_json(&tasks_uri).await;
    assert_eq!(status, StatusCode::OK);
    let statuses: Vec<&str> = board["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|column| column["status"].as_str().unwrap())
        .collect();
    assert_eq!(
        statuses,
        vec!["backlog", "planning", "running", "review", "done"]
    );
    assert_eq!(board["columns"][0]["tasks"][0]["title"], "Web Task");

    let task_uri = format!(
        "/api/projects/{}/tasks/{}",
        fixture.project.id, fixture.backlog_task.id
    );
    let (status, task) = get_json(&task_uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(task["title"], "Web Task");
    assert!(task["pr_url"].is_null());
    assert!(task["pr_number"].is_null());
    assert_eq!(task["artifacts"][1]["name"], "plan");
    assert_eq!(task["artifacts"][1]["available"], true);

    let artifact_uri = format!("{task_uri}/artifacts/plan");
    let (status, artifact_html) = get(&artifact_uri).await;
    assert_eq!(status, StatusCode::OK);
    assert!(artifact_html.contains("<h1>Plan</h1>"));
    assert!(artifact_html.contains("<strong>covered</strong>"));

    let missing_artifact_uri = format!("{task_uri}/artifacts/research");
    let (status, _) = get(&missing_artifact_uri).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let missing_task_uri = format!("/api/projects/{}/tasks/missing", fixture.project.id);
    let (status, _) = get(&missing_task_uri).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);

    let page_uri = format!(
        "/project/{}/task/{}/plan",
        fixture.project.id, fixture.backlog_task.id
    );
    let (status, page) = get(&page_uri).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("Web Task"));
    assert!(page.contains(&format!("\"project_id\":\"{}\"", fixture.project.id)));
    assert!(page.contains(&format!("\"task_id\":\"{}\"", fixture.backlog_task.id)));
    assert!(page.contains("\"artifact\":\"plan\""));
}
