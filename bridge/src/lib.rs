//! Build bridge — the device daemon.
//!
//! Everything is files + git + PTY. The bridge owns git worktrees, spawns agent
//! harnesses in full PTYs, serves a conversation-aware MCP server per entity,
//! watches git for legibility, and drives the **plan/run split**: two entities,
//! each with its own lifecycle, instead of one fused task.
//!
//! A [`plan`](crate::plan) is project-scoped; its canonical docs live in the
//! store. Its agent runs in the project's primary checkout, writing docs into a
//! scratch dir outside the repo, and once approved the plan can be implemented
//! later, never, or repeatedly:
//!
//! ```text
//! created → drafting → plan_review → approved
//!               │  ▲                     (blocked / failed / idle_unreported /
//!               │  └── notes (batch) ─┐   interrupted during drafting; abandoned)
//!               └──────── revision ───┘
//! ```
//!
//! A [`run`](crate::run) is worktree-scoped: one implementation attempt on a
//! `build/<slug>` branch, carrying an optional `plan_id` (a Quick task is a run
//! with none). Dispatching a run materializes the plan docs into the worktree:
//!
//! ```text
//! created → building → review → merged
//!              │  ▲       │      (blocked / failed / idle_unreported /
//!              │  └ changes┘      interrupted during building; abandoned /
//!              └─ stage_gate ─┐   archived when the worktree vanishes)
//!                (multi-stage)┘
//! ```
//!
//! Enforcement is by observation, not permission: the bridge makes what an agent
//! did legible (the git diff) and lets the human decide at the review gates.
//! State lives in one SQLite database under the bridge state dir; the canonical
//! plan docs an agent reads and writes stay beside it as files.

// The app mutex is a std lock guarding the whole daemon: everything a frame
// touches is behind it. Holding it across an await parks it on a task that may
// not be scheduled again for as long as the runtime likes — the shape of the
// 2026-08-13 wedge, where a poll held it through a multi-second worktree diff
// and the relay's read loop starved behind it. The git work now runs on threads
// that hold no lock; this keeps it that way.
#![deny(clippy::await_holding_lock)]

pub mod agent;
pub mod app;
pub mod attention;
pub mod backoff;
pub mod branch;
pub mod capture;
pub mod carrier;
pub mod changes;
pub mod config;
pub mod delivery;
pub mod diff;
#[cfg(test)]
pub mod git_fixture;
pub mod git_process;
pub mod gitgui;
pub mod harness;
pub mod identity;
pub mod isolation;
pub mod lifecycle;
pub mod mcp;
pub mod models;
pub mod notify;
pub mod orchestrator;
pub mod pairing;
pub mod plan;
pub mod pty;
pub mod reaper;
pub mod relay;
pub mod relay_server;
pub mod review_rules;
pub mod router;
pub mod rtc;
pub mod run;
pub mod screen;
pub mod service;
pub mod store;
pub mod templates;
pub mod thread;
pub mod timing;
pub mod transport;
pub mod transport_ledger;
pub mod transport_report;
pub mod worktree;
