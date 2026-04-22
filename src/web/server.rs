use crate::web::handlers;
use anyhow::Result;
use axum::{routing::get, Router};

pub async fn serve(port: u16) -> Result<()> {
    let app = Router::new()
        .route("/", get(handlers::index))
        .route("/api/projects", get(handlers::list_projects))
        .route("/api/projects/{project_id}/tasks", get(handlers::project_tasks))
        .route(
            "/api/projects/{project_id}/tasks/{task_id}",
            get(handlers::task_detail),
        )
        .route(
            "/api/projects/{project_id}/tasks/{task_id}/artifacts/{name}",
            get(handlers::get_artifact),
        );

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("agtx web → http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
