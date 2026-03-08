# MCP Server for agtx

## Context

External agents (e.g. Claude Code) should be able to read and control the agtx kanban board programmatically. This adds an MCP (Model Context Protocol) server to agtx, launched via `agtx mcp-serve`, communicating over stdio. Read operations access the DB directly. Write operations (task transitions) use a command queue table that the TUI polls and executes with full side effects (worktree creation, agent spawning, etc.).

## Files to Modify/Create

| File | Action |
|------|--------|
| `Cargo.toml` | Add `rmcp`, `schemars` dependencies |
| `src/db/models.rs` | Add `TransitionRequest` struct |
| `src/db/schema.rs` | Add `transition_requests` table + CRUD methods |
| `src/db/mod.rs` | No change needed (`pub use models::*` already exports everything) |
| `src/mcp/mod.rs` | **New** — module re-exports + `serve()` entry point |
| `src/mcp/server.rs` | **New** — MCP server struct with `#[tool]` methods |
| `src/lib.rs` | Add `pub mod mcp;` |
| `src/main.rs` | Add `mcp-serve` subcommand match arm |
| `src/tui/app.rs` | Add `process_transition_requests()` to event loop |
| `tests/mcp_tests.rs` | **New** — DB-layer integration tests |
| `src/tui/app_tests.rs` | Add transition request processing tests |

## Step 1: Add Dependencies

**`Cargo.toml`** — add under `[dependencies]`:
```toml
rmcp = { version = "0.16", features = ["server", "macros", "transport-io"] }
schemars = "0.8"
```

## Step 2: Add `TransitionRequest` Model

**`src/db/models.rs`** — add after `Project` struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionRequest {
    pub id: String,
    pub task_id: String,
    pub action: String,  // "move_forward", "move_to_planning", "move_to_running", "move_to_review", "move_to_done", "resume"
    pub requested_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}
```

With a `TransitionRequest::new(task_id, action)` constructor that generates a UUID.

## Step 3: Add DB Schema + CRUD

**`src/db/schema.rs`**:

3a. Add table creation in `init_project_schema()`:
```sql
CREATE TABLE IF NOT EXISTS transition_requests (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    action TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    processed_at TEXT,
    error TEXT
);
```

3b. Add methods to `Database`:
- `create_transition_request(&self, req: &TransitionRequest) -> Result<()>`
- `get_pending_transition_requests(&self) -> Result<Vec<TransitionRequest>>` — `WHERE processed_at IS NULL ORDER BY requested_at ASC`
- `mark_transition_processed(&self, id: &str, error: Option<&str>) -> Result<()>`
- `get_transition_request(&self, id: &str) -> Result<Option<TransitionRequest>>`
- `cleanup_old_transition_requests(&self) -> Result<()>` — delete processed requests older than 1 hour

## Step 4: Create MCP Server Module

**`src/mcp/mod.rs`** — re-exports + `pub async fn serve(project_path: PathBuf)`

**`src/mcp/server.rs`** — `AgtxMcpServer` struct holding `project_path: PathBuf`. Opens fresh DB connections per tool call (separate process from TUI, avoids SQLite locking issues).

### MCP Tools

**`list_projects`** — opens global DB, returns `Vec<{id, name, path}>`

**`list_tasks`** — opens project DB, optional `status` filter, returns `Vec<{id, title, description, status, agent, branch_name, pr_url, plugin}>`

**`get_task`** — opens project DB, returns full task details as JSON

**`move_task(task_id, action)`** — validates action string, creates `TransitionRequest`, inserts into DB, returns `{request_id, message}`. Supported actions: `move_forward`, `move_to_planning`, `move_to_running`, `move_to_review`, `move_to_done`, `resume`

**`get_transition_status(request_id)`** — returns `{request_id, status: "pending"|"completed"|"error", error: Option<String>}`

### rmcp Pattern
```rust
#[derive(Clone)]
pub struct AgtxMcpServer {
    project_path: PathBuf,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl AgtxMcpServer {
    #[tool(description = "List all tasks for the current project")]
    fn list_tasks(&self, Parameters(params): Parameters<ListTasksParams>) -> String {
        // open DB, query, return JSON
    }
    // ... other tools
}

#[tool_handler]
impl ServerHandler for AgtxMcpServer {
    fn get_info(&self) -> ServerInfo { ... }
}
```

Serve via stdio:
```rust
pub async fn serve(project_path: PathBuf) -> Result<()> {
    let server = AgtxMcpServer::new(project_path);
    let transport = (tokio::io::stdin(), tokio::io::stdout());
    let service = server.serve(transport).await?;
    service.waiting().await?;
    Ok(())
}
```

## Step 5: Wire Up CLI + Module

**`src/lib.rs`** — add `pub mod mcp;`

**`src/main.rs`** — add match arm before existing `Some("-g")`:
```rust
Some("mcp-serve") => {
    let project_path = args.get(2)
        .map(|p| PathBuf::from(p))
        .unwrap_or(std::env::current_dir()?);
    let project_path = project_path.canonicalize()?;
    if !git::is_git_repo(&project_path) {
        anyhow::bail!("mcp-serve requires a git project directory");
    }
    return agtx::mcp::serve(project_path).await;
}
```

Early return — skips TUI initialization entirely.

## Step 6: TUI Polls Transition Requests

**`src/tui/app.rs`**:

6a. Add `process_transition_requests(&mut self)` method — called in event loop after `setup_rx`/`pr_creation_rx` checks, before keyboard polling. Calls `db.get_pending_transition_requests()`, processes each one.

6b. Add `execute_transition_request(&mut self, req)` method — dispatches based on `req.action`:

| Action | Validates | Calls |
|--------|-----------|-------|
| `move_forward` | task exists | `execute_forward_transition(task, project_path)` |
| `move_to_planning` | status == Backlog | `transition_to_planning(task, project_path)` |
| `move_to_running` | status == Planning | `transition_to_running(task)` |
| `move_to_review` | status == Running | `mcp_transition_to_review(task)` — sends review prompt, no PR popup |
| `move_to_done` | status == Review | `force_move_to_done(task_id)` — reuses existing method (line 2009) |
| `resume` | status == Review | `move_review_to_running(task_id)` — reuses existing method (line 3643) |

6c. Add `mcp_transition_to_review(task)` — same as `transition_to_review` but skips the `ReviewConfirmPopup` / PR push logic. Just sends review skill+prompt and updates status.

6d. Add `execute_forward_transition(task, project_path)` — mirrors `move_task_right` logic: determines next status, calls appropriate transition function. For running→review uses `mcp_transition_to_review`, for review→done uses `force_move_to_done`.

6e. Guard: if `self.state.setup_rx.is_some()`, skip processing (another setup is in flight). Requests stay pending and get picked up next cycle.

6f. After processing each request, call `db.mark_transition_processed(id, error)` and `refresh_tasks()`.

## Step 7: Tests

**`tests/mcp_tests.rs`** — DB integration tests:
- Create/get transition request
- Get pending (only unprocessed)
- Mark processed with success/error
- Cleanup old requests

**`src/tui/app_tests.rs`** — `#[cfg(feature = "test-mocks")]` tests:
- Process `move_forward` from Backlog (verifies transition_to_planning called)
- Invalid action returns error
- Wrong status returns error (e.g. move_to_running on Backlog task)

## Verification

1. `cargo build --release` — compiles with new deps
2. `cargo test` + `cargo test --features test-mocks` — all tests pass
3. Manual: start `agtx` TUI in one terminal, then in another terminal in the same project:
   ```bash
   # Register the MCP server with Claude Code
   claude mcp add --transport stdio agtx -- ./target/release/agtx mcp-serve .
   # Then in Claude Code, use the tools:
   # list_tasks, get_task, move_task, get_transition_status
   ```
4. Verify: after `move_task`, the TUI reflects the transition within ~100ms
