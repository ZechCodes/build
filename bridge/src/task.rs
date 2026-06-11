//! The task model and its lifecycle state machine.
//!
//! A task is a goal, a worktree + branch, a sequence of phase sessions, and a
//! state. This module is the pure domain core — no IO, no git, no PTY — so the
//! lifecycle rules are testable in isolation.
