use crate::web::handlers::{self, AppState};
use anyhow::Result;
use axum::{routing::get, Router};

pub fn router(state: AppState) -> Router {
    Router::new()
        // API routes
        .route("/api/projects", get(handlers::list_projects))
        .route(
            "/api/projects/{project_id}/tasks",
            get(handlers::project_tasks),
        )
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
        .route(
            "/project/{pid}/task/{tid}/{artifact}",
            get(handlers::page_artifact),
        )
        // Catch-all: serve the HTML shell for any other path
        .fallback(get(handlers::index))
        .with_state(state)
}

pub fn parse_web_port(args: &[String], env_port: Option<&str>) -> u16 {
    args.windows(2)
        .find(|w| w[0] == "--port")
        .and_then(|w| w[1].parse().ok())
        .or_else(|| env_port.and_then(|v| v.parse().ok()))
        .unwrap_or(3000)
}

pub async fn serve(port: u16) -> Result<()> {
    let config_dir = crate::config::GlobalConfig::config_dir()?;
    let app = router(AppState::new(config_dir));

    let addr = format!("127.0.0.1:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("agtx web → http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_web_port;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parse_web_port_defaults_to_3000() {
        assert_eq!(parse_web_port(&args(&["agtx", "web-serve"]), None), 3000);
    }

    #[test]
    fn parse_web_port_uses_port_flag_before_env() {
        assert_eq!(
            parse_web_port(
                &args(&["agtx", "web-serve", "--port", "4321"]),
                Some("1234")
            ),
            4321
        );
    }

    #[test]
    fn parse_web_port_uses_env_when_flag_is_absent() {
        assert_eq!(
            parse_web_port(&args(&["agtx", "web-serve"]), Some("1234")),
            1234
        );
    }

    #[test]
    fn parse_web_port_falls_back_for_invalid_values() {
        assert_eq!(
            parse_web_port(&args(&["agtx", "web-serve", "--port", "nope"]), Some("bad")),
            3000
        );
    }
}
