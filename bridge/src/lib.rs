//! Build bridge — the device daemon.
//!
//! Everything is files + git + PTY. The bridge owns git worktrees, spawns agent
//! harnesses in full PTYs, serves a single-tool (`done`) MCP server per task,
//! watches git for legibility, and drives the task lifecycle:
//!
//! ```text
//! created → planning → plan_review → building → review → merged
//! ```
//!
//! Enforcement is by observation, not permission: the bridge makes what an agent
//! did legible (the git diff) and lets the human decide at the review gates.

pub mod pty;
pub mod task;
pub mod worktree;
