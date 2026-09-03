//! Domain logic with no terminal, no HTTP and no agent transport attached.
//!
//! Everything here is shared by callers that drive the board from outside the
//! TUI — the MCP server today, the web API alongside it. Step 8 of the mobile
//! plan moves the transition machinery in beside it.

pub mod actions;
pub mod input;
