use crate::web::handlers;
use anyhow::Result;
use axum::{routing::get, Router};

pub async fn serve(port: u16) -> Result<()> {
    let app = Router::new()
        // API routes
        .route("/api/projects", get(handlers::list_projects))
        .route("/api/projects/{project_id}/tasks", get(handlers::project_tasks))
        .route(
            "/api/projects/{project_id}/tasks/{task_id}",
            get(handlers::task_detail),
        )
        .route(
            "/api/projects/{project_id}/tasks/{task_id}/artifacts/{name}",
            get(handlers::get_artifact),
        )
        // Page routes (server sets <title> and embeds initial state)
        .route("/project/{pid}", get(handlers::page_project))
        .route("/project/{pid}/task/{tid}", get(handlers::page_task))
        .route("/project/{pid}/task/{tid}/{artifact}", get(handlers::page_artifact))
        // Catch-all: serve the HTML shell for any other path
        .fallback(get(handlers::index));

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("agtx web → http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
