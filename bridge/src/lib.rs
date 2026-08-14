//! Build bridge — the device daemon.
//!
//! Everything is files + git + PTY. The bridge owns git worktrees, spawns agent
//! harnesses in full PTYs, serves a conversation-aware MCP server per entity,
//! watches git for legibility, and drives the **plan/run split**: two entities,
//! each with its own lifecycle, instead of one fused task.
//!
//! A [`plan`](crate::plan) is project-scoped; its canonical docs live in the
//! store. It is authored in a disposable `plan/<slug>` worktree and, once
//! approved, can be implemented later, never, or repeatedly:
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
//! Pre-split stores are migrated on boot ([`legacy`](crate::legacy) holds the
//! old serde shapes solely for that translation).

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
pub mod config;
pub mod diff;
pub mod gitgui;
pub mod identity;
pub mod legacy;
pub mod mcp;
pub mod models;
pub mod notify;
pub mod orchestrator;
pub mod pairing;
pub mod plan;
pub mod pty;
pub mod relay;
pub mod relay_server;
pub mod review_rules;
pub mod router;
pub mod run;
pub mod service;
pub mod store;
pub mod templates;
pub mod thread;
pub mod transport;
pub mod worktree;
